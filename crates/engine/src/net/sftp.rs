//! SFTP (F-08, SPEC §7.22, phase 7, étape 6c), derrière les trois appels du transfert,
//! comme FTP ([`super::ftp`]) : segments, reprise et redécoupage de HTTP valent tels quels.
//!
//! - **Clé du serveur** : comparée à celle que l'utilisateur a acceptée pour `hôte:port`
//!   ([`HostKeys`]). Inconnue : la connexion s'arrête ([`NetError::HostKeyUnknown`]) et
//!   l'empreinte attend la décision de l'utilisateur ; différente : bloquée
//!   ([`NetError::HostKeyChanged`]). Le fichier `known_hosts` d'OpenSSH n'est jamais lu
//!   (décision de l'utilisateur du 2026-11-17).
//! - **Identifiants** : l'en-tête `Authorization` (Basic) de la tâche, où l'ajout range ceux
//!   de l'adresse ; sinon ceux de l'adresse ; sinon ceux du site (SPEC §7.14) : mot de
//!   passe, ou fichier de clé privée et sa phrase de passe. Sans identifiant :
//!   `AUTH_REQUIRED`.
//! - **Connexions** : une connexion SSH par segment (deux fois plus rapide qu'un canal
//!   partagé, prototype 6a), réutilisée d'une plage à l'autre ([`Shared`]).
//! - **Préparation** : `stat` (taille, date de modification, le validateur) ; une plage
//!   revoit les deux avant de lire, à partir de n'importe quelle position.
//! - **Proxy et réseau public** : comme FTP ([`super::dial`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use percent_encoding::percent_decode_str;
use reqwest::Url;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use russh_sftp::client::fs::File;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::auth::{Auth, Secret};
use super::dial::{Route, dial, route_for};
use super::{NetError, Probe, RangeRequest, filename, proxy, reach};

const POOL_PER_KEY: usize = 8;
const POOL_IDLE: Duration = Duration::from_secs(30);
/// Taille d'une lecture : les serveurs servent souvent 256 Kio par requête au plus
/// (`limits@openssh.com`), et chaque requête coûte un aller et retour.
const CHUNK: usize = 256 * 1024;

/// Adresse SFTP ?
pub fn is_sftp(url: &Url) -> bool {
    url.scheme() == "sftp"
}

/// Chemin du fichier dans une adresse SFTP : absolu, sauf `/~/…`, relatif au dossier de
/// l'utilisateur. Refusé : vide, un dossier, un caractère nul.
pub fn file_path(url: &Url) -> Result<String, NetError> {
    let path = percent_decode_str(url.path())
        .decode_utf8()
        .map_err(|_| NetError::InvalidUrl("chemin SFTP illisible (UTF-8)".into()))?
        .into_owned();
    let path = match path.strip_prefix("/~/") {
        Some(home) => home.to_owned(),
        None => path,
    };
    if path.contains('\0') {
        return Err(NetError::InvalidUrl("chemin SFTP invalide".into()));
    }
    if path.is_empty() || path == "/" || path.ends_with('/') {
        return Err(NetError::InvalidUrl(
            "adresse SFTP sans nom de fichier : un dossier ne se télécharge pas".into(),
        ));
    }
    Ok(path)
}

/// Clé d'un serveur vue sans être acceptée (inconnue ou changée), en attente de la
/// décision de l'utilisateur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingKey {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    /// `SHA256:…`, comme l'affiche OpenSSH.
    pub fingerprint: String,
    /// Une autre clé était acceptée pour ce serveur.
    pub changed: bool,
    /// Vue pour la dernière fois (millisecondes depuis 1970).
    pub seen_at: i64,
}

/// Clés des serveurs SSH : acceptées (empreinte par `hôte:port`, tenues par le planificateur
/// depuis la base) et vues en attente.
#[derive(Debug, Default)]
pub struct HostKeys {
    accepted: Mutex<HashMap<(String, u16), String>>,
    pending: Mutex<HashMap<(String, u16), PendingKey>>,
}

/// Clés en attente d'une décision, au plus.
const MAX_PENDING: usize = 256;

impl HostKeys {
    /// Remplace les clés acceptées (au démarrage).
    pub fn set_accepted(&self, keys: impl IntoIterator<Item = (String, u16, String)>) {
        let mut accepted = self.accepted.lock().unwrap_or_else(PoisonError::into_inner);
        *accepted = keys
            .into_iter()
            .map(|(h, p, f)| ((h.to_ascii_lowercase(), p), f))
            .collect();
    }

    /// Accepte `fingerprint` pour `host:port` ; elle sort de l'attente.
    pub fn accept(&self, host: &str, port: u16, fingerprint: &str) {
        let key = (host.to_ascii_lowercase(), port);
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
        self.accepted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, fingerprint.to_owned());
    }

    /// Oublie la clé acceptée de `host:port`, et celle qui attendait (audit F6).
    pub fn forget(&self, host: &str, port: u16) {
        let key = (host.to_ascii_lowercase(), port);
        self.accepted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
    }

    /// Clés en attente d'une décision.
    pub fn pending(&self) -> Vec<PendingKey> {
        let mut list: Vec<PendingKey> = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        list.sort_by(|a, b| (&a.host, a.port).cmp(&(&b.host, b.port)));
        list
    }

    /// La clé en attente pour `host:port`, si c'est bien `fingerprint`.
    pub fn pending_for(&self, host: &str, port: u16, fingerprint: &str) -> Option<PendingKey> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(host.to_ascii_lowercase(), port))
            .filter(|p| p.fingerprint == fingerprint)
            .cloned()
    }

    /// Juge la clé présentée par `host:port`.
    fn check(&self, host: &str, port: u16, algorithm: String, fingerprint: String) -> Verdict {
        let key = (host.to_ascii_lowercase(), port);
        let known = self
            .accepted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if known.as_deref() == Some(fingerprint.as_str()) {
            // La bonne clé est revenue : celle qui attendait n'est plus à décider (audit F6).
            pending.remove(&key);
            return Verdict::Trusted;
        }
        let changed = known.is_some();
        // Table bornée : un serveur qui change de clé à chaque connexion, ou beaucoup
        // d'hôtes inconnus, ne la font pas grossir sans fin ; la plus ancienne part.
        if pending.len() >= MAX_PENDING && !pending.contains_key(&key) {
            let oldest = pending
                .iter()
                .min_by_key(|(_, p)| p.seen_at)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                pending.remove(&oldest);
            }
        }
        pending.insert(
            key,
            PendingKey {
                host: host.to_ascii_lowercase(),
                port,
                algorithm: algorithm.clone(),
                fingerprint: fingerprint.clone(),
                changed,
                seen_at: crate::store::now_ms(),
            },
        );
        let host = host.to_ascii_lowercase();
        if changed {
            Verdict::Refused(NetError::HostKeyChanged {
                host,
                port,
                algorithm,
                fingerprint,
            })
        } else {
            Verdict::Refused(NetError::HostKeyUnknown {
                host,
                port,
                algorithm,
                fingerprint,
            })
        }
    }
}

enum Verdict {
    Trusted,
    Refused(NetError),
}

/// Le gestionnaire de russh d'une connexion : vérifie la clé du serveur.
struct Checker {
    host: String,
    port: u16,
    keys: Arc<HostKeys>,
    refused: Arc<Mutex<Option<NetError>>>,
}

impl russh::client::Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let public = key.public_key();
        let fingerprint = public.fingerprint(HashAlg::Sha256).to_string();
        let algorithm = public.algorithm().to_string();
        match self
            .keys
            .check(&self.host, self.port, algorithm, fingerprint)
        {
            Verdict::Trusted => Ok(true),
            Verdict::Refused(e) => {
                *self.refused.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
                Ok(false)
            }
        }
    }
}

/// Réglages SFTP d'un client, partagés par ses copies.
#[derive(Default)]
pub struct Shared {
    pub keys: Arc<HostKeys>,
    pool: Mutex<HashMap<String, Vec<(Instant, Session)>>>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("sftp::Shared").finish_non_exhaustive()
    }
}

impl Shared {
    fn take(&self, key: &str) -> Option<Session> {
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        let idle = pool.get_mut(key)?;
        while let Some((since, session)) = idle.pop() {
            if since.elapsed() < POOL_IDLE && !session.ssh.is_closed() {
                return Some(session);
            }
        }
        None
    }

    fn put(&self, session: Session) {
        if session.ssh.is_closed() {
            return;
        }
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        pool.retain(|_, idle| {
            idle.retain(|(since, _)| since.elapsed() < POOL_IDLE);
            !idle.is_empty()
        });
        let idle = pool.entry(session.key.clone()).or_default();
        if idle.len() < POOL_PER_KEY {
            idle.push((Instant::now(), session));
        }
    }
}

/// Une connexion SSH identifiée et sa session SFTP.
struct Session {
    ssh: russh::client::Handle<Checker>,
    sftp: SftpSession,
    key: String,
}

/// Identifiants d'une connexion SFTP.
enum Login {
    Password(String),
    Key { path: String, passphrase: String },
}

fn credentials(
    url: &Url,
    task_headers: &[(String, String)],
    auth: Option<&Auth>,
) -> Option<(String, Login)> {
    if let Some((user, pass)) = super::ftp::basic_credentials(task_headers) {
        return Some((user, Login::Password(pass)));
    }
    if !url.username().is_empty() {
        let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
        let user = decode(url.username());
        if let Some(pass) = url.password() {
            return Some((user, Login::Password(decode(pass))));
        }
        // Nom seul dans l'adresse : le secret du site, s'il est pour ce nom.
        return match auth.and_then(|a| a.secret_for(url)) {
            Some((site_user, secret)) if site_user == user => Some((user, login_of(secret))),
            _ => None,
        };
    }
    let (user, secret) = auth?.secret_for(url)?;
    Some((user, login_of(secret)))
}

fn login_of(secret: Secret) -> Login {
    match secret {
        Secret::Password(p) => Login::Password(p),
        Secret::Key { path, passphrase } => Login::Key { path, passphrase },
    }
}

fn ssh_error(error: russh::Error) -> NetError {
    NetError::Socket {
        connect: false,
        text: format!("SSH : {error}"),
    }
}

fn sftp_error(error: russh_sftp::client::error::Error) -> NetError {
    use russh_sftp::client::error::Error;
    match error {
        Error::Status(status) => NetError::SftpStatus(status.status_code as u32),
        Error::Timeout => NetError::Socket {
            connect: false,
            text: "délai dépassé".into(),
        },
        Error::IO(text) => NetError::Socket {
            connect: false,
            text,
        },
        _ => NetError::BadResponse("réponse SFTP inattendue"),
    }
}

async fn within<T>(
    limit: Duration,
    future: impl Future<Output = Result<T, NetError>>,
) -> Result<T, NetError> {
    tokio::time::timeout(limit, future)
        .await
        .map_err(|_| NetError::ReadTimeout(limit))?
}

/// Ce que le client donne au module.
pub(super) struct Context<'a> {
    pub connect: Duration,
    pub read: Duration,
    pub proxy: proxy::Effective,
    pub public_only: bool,
    pub shared: &'a Arc<Shared>,
}

/// Ce qu'il faut pour joindre un serveur.
struct Server {
    host: String,
    port: u16,
    user: String,
    login: Login,
    route: Route,
}

impl Server {
    fn key(&self) -> String {
        let secret = match &self.login {
            Login::Password(p) => format!("p:{p}"),
            Login::Key { path, passphrase } => format!("k:{path}:{passphrase}"),
        };
        let secret = Sha256::digest(secret.as_bytes());
        format!(
            "{}|{}|{}|{secret:x}|{:?}",
            self.host, self.port, self.user, self.route
        )
    }
}

impl Context<'_> {
    fn server(
        &self,
        url: &Url,
        task_headers: &[(String, String)],
        auth: Option<&Auth>,
    ) -> Result<Server, NetError> {
        if self.public_only && reach::literal_forbidden(url) {
            return Err(NetError::AddressForbidden);
        }
        let host = url
            .host_str()
            .map(|h| h.trim_start_matches('[').trim_end_matches(']'))
            .filter(|h| !h.is_empty())
            .ok_or(NetError::InvalidUrl("adresse SFTP sans hôte".into()))?
            .to_ascii_lowercase();
        let port = super::ftp::default_port(url).unwrap_or(22);
        let (user, login) = credentials(url, task_headers, auth).ok_or(NetError::AuthRequired)?;
        let route = route_for(&self.proxy, self.public_only, &host);
        Ok(Server {
            host,
            port,
            user,
            login,
            route,
        })
    }

    async fn session(&self, server: &Server) -> Result<Session, NetError> {
        let key = server.key();
        if let Some(session) = self.shared.take(&key) {
            return Ok(session);
        }
        within(self.connect + self.read, self.open(server, key)).await
    }

    async fn open(&self, server: &Server, key: String) -> Result<Session, NetError> {
        let stream = dial(&server.route, &server.host, server.port, None).await?;
        let refused = Arc::new(Mutex::new(None));
        let checker = Checker {
            host: server.host.clone(),
            port: server.port,
            keys: Arc::clone(&self.shared.keys),
            refused: Arc::clone(&refused),
        };
        let config = Arc::new(russh::client::Config {
            inactivity_timeout: Some(self.read * 4),
            ..russh::client::Config::default()
        });
        let connected = russh::client::connect_stream(config, stream, checker).await;
        let refusal = refused
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let mut ssh = match (connected, refusal) {
            (_, Some(e)) => return Err(e),
            (Ok(ssh), None) => ssh,
            (Err(e), None) => return Err(ssh_error(e)),
        };
        let accepted = match &server.login {
            Login::Password(password) => ssh
                .authenticate_password(server.user.as_str(), password.as_str())
                .await
                .map_err(ssh_error)?,
            Login::Key { path, passphrase } => {
                let passphrase = (!passphrase.is_empty()).then_some(passphrase.as_str());
                let private = russh::keys::load_secret_key(path, passphrase)
                    .map_err(|_| NetError::KeyUnreadable)?;
                let hash = ssh
                    .best_supported_rsa_hash()
                    .await
                    .map_err(ssh_error)?
                    .flatten();
                ssh.authenticate_publickey(
                    server.user.as_str(),
                    PrivateKeyWithHashAlg::new(Arc::new(private), hash),
                )
                .await
                .map_err(ssh_error)?
            }
        };
        if !accepted.success() {
            return Err(NetError::AuthRefused);
        }
        let channel = ssh.channel_open_session().await.map_err(ssh_error)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(ssh_error)?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(sftp_error)?;
        sftp.set_timeout(self.read.as_secs().max(1));
        tracing::debug!(host = %server.host, "connexion SFTP ouverte");
        Ok(Session { ssh, sftp, key })
    }

    /// Taille et date du fichier ; un dossier est refusé.
    async fn facts(
        session: &Session,
        path: &str,
    ) -> Result<(Option<u64>, Option<String>), NetError> {
        let attrs = session.sftp.metadata(path).await.map_err(sftp_error)?;
        if attrs.is_dir() {
            return Err(NetError::InvalidUrl(
                "adresse SFTP d'un dossier : un dossier ne se télécharge pas".into(),
            ));
        }
        Ok((attrs.size, attrs.mtime.map(|m| m.to_string())))
    }

    pub async fn probe(
        &self,
        url: &Url,
        task_headers: &[(String, String)],
        auth: Option<&Auth>,
    ) -> Result<Probe, NetError> {
        let path = file_path(url)?;
        let server = self.server(url, task_headers, auth)?;
        let session = self.session(&server).await?;
        let (total, modified) = within(self.read, Self::facts(&session, &path)).await?;
        self.shared.put(session);
        let mut final_url = url.clone();
        let _ = final_url.set_username("");
        let _ = final_url.set_password(None);
        let name = path.rsplit('/').next().unwrap_or_default();
        Ok(Probe {
            final_url,
            total_bytes: total,
            accepts_ranges: total.is_some(),
            etag: None,
            last_modified: modified,
            file_name: filename::sanitize(name, filename::NameRules::native()),
            downgraded: false,
            content_type: None,
            attachment: false,
        })
    }

    pub async fn get_range(
        &self,
        url: &Url,
        task_headers: &[(String, String)],
        req: &RangeRequest<'_>,
        auth: Option<&Auth>,
    ) -> Result<SftpBody, NetError> {
        let path = file_path(url)?;
        let server = self.server(url, task_headers, auth)?;
        let session = self.session(&server).await?;
        let file = within(self.read, async {
            let (size, modified) = Self::facts(&session, &path).await?;
            if size != Some(req.total) {
                return Err(NetError::FileChanged);
            }
            if let Some(expected) = req.if_range
                && modified.as_deref() != Some(expected)
            {
                return Err(NetError::FileChanged);
            }
            let mut file = session.sftp.open(path.as_str()).await.map_err(sftp_error)?;
            file.seek(std::io::SeekFrom::Start(req.first))
                .await
                .map_err(|e| NetError::Socket {
                    connect: false,
                    text: e.to_string(),
                })?;
            Ok(file)
        })
        .await?;
        Ok(SftpBody {
            session: Some(session),
            file: Some(file),
            remaining: Some(req.last - req.first + 1),
            shared: Arc::clone(self.shared),
            read: self.read,
        })
    }

    pub async fn get_all(
        &self,
        url: &Url,
        task_headers: &[(String, String)],
        expected_total: Option<u64>,
        auth: Option<&Auth>,
    ) -> Result<SftpBody, NetError> {
        let path = file_path(url)?;
        let server = self.server(url, task_headers, auth)?;
        let session = self.session(&server).await?;
        let file = within(self.read, async {
            let (size, _) = Self::facts(&session, &path).await?;
            if let (Some(expected), Some(size)) = (expected_total, size)
                && expected != size
            {
                return Err(NetError::FileChanged);
            }
            session.sftp.open(path.as_str()).await.map_err(sftp_error)
        })
        .await?;
        Ok(SftpBody {
            session: Some(session),
            file: Some(file),
            remaining: expected_total,
            shared: Arc::clone(self.shared),
            read: self.read,
        })
    }
}

/// Octets lus d'un fichier SFTP : au plus ceux de la plage ; la connexion revient à la
/// réserve ensuite (et aussi si la plage est abandonnée : rien n'est à interrompre).
pub struct SftpBody {
    session: Option<Session>,
    file: Option<File>,
    remaining: Option<u64>,
    shared: Arc<Shared>,
    read: Duration,
}

impl std::fmt::Debug for SftpBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpBody")
            .field("remaining", &self.remaining)
            .finish_non_exhaustive()
    }
}

impl Drop for SftpBody {
    fn drop(&mut self) {
        // Le fichier se ferme de lui-même (`close` sans attendre), puis la connexion
        // resservira.
        drop(self.file.take());
        if let Some(session) = self.session.take() {
            self.shared.put(session);
        }
    }
}

impl SftpBody {
    /// Morceau suivant ; `None` à la fin. Une fin de fichier avant la fin attendue est une
    /// erreur (fichier raccourci).
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, NetError> {
        let Some(file) = self.file.as_mut() else {
            return Ok(None);
        };
        let want = self
            .remaining
            .map_or(CHUNK, |r| usize::try_from(r).unwrap_or(CHUNK).min(CHUNK));
        if want == 0 {
            self.finish();
            return Ok(None);
        }
        let mut buf = BytesMut::zeroed(want);
        let read = tokio::time::timeout(self.read, file.read(&mut buf)).await;
        // Après un délai dépassé ou une erreur, la session ne revient pas à la réserve :
        // bloquée sans être fermée (NAT, pare-feu), elle ferait attendre tout un délai au
        // segment suivant (audit F2).
        let n = match read {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                self.abandon();
                return Err(NetError::Socket {
                    connect: false,
                    text: e.to_string(),
                });
            }
            Err(_) => {
                self.abandon();
                return Err(NetError::ReadTimeout(self.read));
            }
        };
        if n == 0 {
            if self.remaining.is_some_and(|r| r > 0) {
                self.file = None;
                self.session = None;
                return Err(NetError::FileChanged);
            }
            self.finish();
            return Ok(None);
        }
        buf.truncate(n);
        if let Some(r) = &mut self.remaining {
            *r -= n as u64;
            if *r == 0 {
                self.finish();
            }
        }
        Ok(Some(buf.freeze()))
    }

    fn finish(&mut self) {
        drop(self.file.take());
        if let Some(session) = self.session.take() {
            self.shared.put(session);
        }
    }

    /// Lecture en échec : fichier et session lâchés, la session fermée avec eux.
    fn abandon(&mut self) {
        self.file = None;
        self.session = None;
    }
}

#[cfg(test)]
#[path = "sftp_tests.rs"]
mod tests;
