//! FTP et FTPS (F-08, SPEC §7.22, phase 7, étape 6b), derrière les trois appels du
//! transfert ([`super::Client::probe_as`], [`super::Client::get_range_as`],
//! [`super::Client::get_all_as`]) : les segments, la reprise et le redécoupage de HTTP
//! servent tels quels.
//!
//! - **Préparation** : `SIZE` (taille), `MDTM` (date, le validateur), `FEAT` (`REST STREAM` :
//!   lecture à partir d'une position, donc segments et reprise).
//! - **Plage** : `MDTM` et `SIZE` revus (un fichier changé arrête la tâche, comme `If-Range`),
//!   `REST` puis `RETR` ; le moteur lit les octets de la plage, puis `ABOR` (ou la fin du
//!   fichier) ; la connexion de commande revient à une réserve ([`Pool`]) pour la plage
//!   suivante.
//! - **Mode passif** : `EPSV` d'abord (IPv6), puis `PASV` ; la connexion de données va
//!   toujours à l'hôte de la connexion de commande, jamais à l'adresse que le serveur
//!   annonce (rebond FTP vers le réseau interne).
//! - **Chiffrement** : `ftps://` en TLS implicite ; `ftp://` avec un mot de passe exige
//!   `AUTH TLS`, sauf autorisation de l'utilisateur pour ce site (« autoriser sans
//!   chiffrement ») ; l'accès anonyme reste en clair. Certificats vérifiés comme en HTTPS
//!   (autorités de `--ca-cert` comprises).
//! - **Identifiants** : l'en-tête `Authorization` (Basic) de la tâche, où l'ajout range ceux
//!   écrits dans l'adresse ; sinon ceux de l'adresse ; sinon ceux du site (SPEC §7.14) ;
//!   sinon `anonymous`.
//! - **Proxy** : SOCKS5 seulement (commande et données) ; avec un proxy HTTP ou celui du
//!   système, en direct. Le TLS implicite ne passe pas par SOCKS5 (limite de suppaftp).
//! - **Réseau public** (connexions à jeton) : noms résolus et vérifiés ici
//!   ([`super::reach`]).
//!
//! Les messages d'erreur ne citent jamais le chemin ni le texte des réponses du serveur, qui
//! peut le contenir : seulement le code (SPEC §8.2).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use base64::Engine as _;
use bytes::{Bytes, BytesMut};
use percent_encoding::percent_decode_str;
use reqwest::Url;
use rustls::pki_types::CertificateDer;
use sha2::{Digest, Sha256};
use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream, AsyncRustlsStream};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Mode};
use tokio::io::AsyncReadExt;

use super::auth::Auth;
use super::dial::{Route, dial, net_error, resolve, route_for, route_public};
use super::{NetError, Probe, RangeRequest, filename, proxy, reach};

type Stream = suppaftp::tokio::TransferStream<AsyncRustlsStream>;

/// Connexions simultanées au plus en mode automatique vers un serveur FTP, FTPS ou SFTP
/// (décision de l'utilisateur du 2026-11-17) : beaucoup en limitent le nombre par adresse.
pub const MAX_AUTO_CONNECTIONS: u32 = 4;

/// Connexions de commande gardées par serveur et identifiant, et leur durée de vie au
/// repos (les serveurs ferment souvent une connexion muette après une à cinq minutes).
const POOL_PER_KEY: usize = 8;
const POOL_IDLE: Duration = Duration::from_secs(30);
/// Délai de `NOOP` (connexion reprise de la réserve), d'`ABOR` et de la réponse de fin.
const QUICK: Duration = Duration::from_secs(10);
/// Taille d'une lecture.
const CHUNK: usize = 64 * 1024;

/// Mot de passe de l'accès anonyme (RFC 1635).
const ANONYMOUS: (&str, &str) = ("anonymous", "anonymous@");

/// Adresse FTP ou FTPS ?
pub fn is_ftp(url: &Url) -> bool {
    matches!(url.scheme(), "ftp" | "ftps")
}

/// Port par défaut d'une adresse, FTPS (990) compris, que `url` ne connaît pas.
pub fn default_port(url: &Url) -> Option<u16> {
    url.port_or_known_default().or(match url.scheme() {
        "ftps" => Some(990),
        "sftp" => Some(22),
        _ => None,
    })
}

/// Chemin du fichier dans une adresse FTP (RFC 1738) : relatif au dossier de connexion
/// (`%2F` au début pour un chemin absolu), décodé, sans `;type=…`. Refusé : un dossier
/// (pas de liste en v1) ou un retour à la ligne (injection d'une commande).
pub fn file_path(url: &Url) -> Result<String, NetError> {
    let raw = url.path().strip_prefix('/').unwrap_or(url.path());
    let raw = match raw.rfind(";type=") {
        Some(at) if raw.len() - at == 7 => &raw[..at],
        _ => raw,
    };
    let path = percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| NetError::InvalidUrl("chemin FTP illisible (UTF-8)".into()))?
        .into_owned();
    if path.chars().any(|c| matches!(c, '\r' | '\n' | '\0')) {
        return Err(NetError::InvalidUrl("chemin FTP invalide".into()));
    }
    if path.is_empty() || path.ends_with('/') {
        return Err(NetError::InvalidUrl(
            "adresse FTP sans nom de fichier : un dossier ne se télécharge pas".into(),
        ));
    }
    Ok(path)
}

/// Retire les identifiants d'une adresse FTP (`ftp://nom:mot@hôte/…`) : l'adresse
/// nettoyée, et le nom et le mot de passe décodés s'il y en avait un. L'ajout d'une tâche
/// range le mot de passe au coffre (en-tête `Authorization`), jamais en base.
pub fn split_credentials(url: &str) -> Result<(String, Option<(String, String)>), NetError> {
    let mut parsed = Url::parse(url).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
    // Le nom seul (`sftp://moi@hôte/f`) n'est pas un secret : il reste dans l'adresse, et
    // le mot de passe ou la clé de l'identifiant du site pour ce nom servira. En faire un
    // en-tête au mot de passe vide masquait celui du site (audit F1).
    if parsed.password().is_none() {
        return Ok((url.to_owned(), None));
    }
    let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
    let user = decode(parsed.username());
    let pass = parsed.password().map(decode).unwrap_or_default();
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    Ok((parsed.to_string(), Some((user, pass))))
}

/// Valeur de l'en-tête `Authorization` qui porte des identifiants FTP.
pub fn basic_header(user: &str, pass: &str) -> String {
    let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
    format!("Basic {token}")
}

/// Identifiants d'une connexion.
#[derive(Clone, PartialEq, Eq)]
struct Creds {
    user: String,
    pass: String,
    anonymous: bool,
}

impl std::fmt::Debug for Creds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Jamais le mot de passe (SPEC §8.2).
        f.debug_struct("Creds")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// Nom et mot de passe de l'en-tête `Authorization` (Basic) d'une tâche (FTP, SFTP).
pub(crate) fn basic_credentials(task_headers: &[(String, String)]) -> Option<(String, String)> {
    let value = task_headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.trim())?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(token.trim())
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, pass) = text.split_once(':')?;
    Some((user.to_owned(), pass.to_owned()))
}

/// Accès anonyme (RFC 1635), qui part sans chiffrement exigé : le nom `anonymous`, ou le
/// nom `ftp` avec un mot de passe vide ou de forme adresse. `ftp` est aussi un vrai compte
/// sur certains serveurs (NAS) : avec un vrai mot de passe, il est traité comme tel, et ce
/// mot de passe ne part jamais en clair sans autorisation (audit F4).
fn is_anonymous(user: &str, pass: &str) -> bool {
    if user.eq_ignore_ascii_case("anonymous") {
        return true;
    }
    user.eq_ignore_ascii_case("ftp")
        && (pass.is_empty()
            || pass
                .split_once('@')
                .is_some_and(|(_, host)| !host.contains('@')))
}

fn credentials(url: &Url, task_headers: &[(String, String)], auth: Option<&Auth>) -> Creds {
    let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
    let found = basic_credentials(task_headers)
        .or_else(|| {
            if url.username().is_empty() {
                return None;
            }
            let user = decode(url.username());
            Some(match url.password() {
                Some(pass) => (user, decode(pass)),
                // Nom seul dans l'adresse : le mot de passe du site, s'il est pour ce nom
                // (audit F1) ; sinon aucun.
                None => match auth.and_then(|a| a.credentials_for(url)) {
                    Some((site_user, pass)) if site_user == user => (user, pass),
                    _ => (user, String::new()),
                },
            })
        })
        .or_else(|| auth.and_then(|a| a.credentials_for(url)));
    match found {
        Some((user, pass)) if !is_anonymous(&user, &pass) => Creds {
            user,
            pass,
            anonymous: false,
        },
        Some((user, pass)) => Creds {
            user,
            pass,
            anonymous: true,
        },
        None => Creds {
            user: ANONYMOUS.0.into(),
            pass: ANONYMOUS.1.into(),
            anonymous: true,
        },
    }
}

/// Chiffrement de la connexion de commande.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Security {
    /// `ftps://` : TLS dès la connexion.
    Implicit,
    /// `ftp://` avec un mot de passe : `AUTH TLS` exigé.
    Required,
    /// `ftp://` avec un mot de passe, site autorisé sans chiffrement : `AUTH TLS` tenté,
    /// en clair si le serveur ne le connaît pas.
    Preferred,
    /// Accès anonyme : en clair.
    Plain,
}

/// Ce qu'il faut pour joindre un serveur.
#[derive(Debug, Clone)]
struct Server {
    host: String,
    port: u16,
    /// Nom du certificat (sans crochets pour une adresse IPv6).
    tls_name: String,
    security: Security,
    creds: Creds,
}

impl Server {
    fn of(url: &Url, creds: Creds, allow_plain: bool) -> Result<Server, NetError> {
        let host = url
            .host_str()
            .map(|h| h.trim_start_matches('[').trim_end_matches(']'))
            .filter(|h| !h.is_empty())
            .ok_or(NetError::InvalidUrl("adresse FTP sans hôte".into()))?
            .to_ascii_lowercase();
        let tls_name = host.clone();
        let port = default_port(url).ok_or(NetError::InvalidUrl("port FTP".into()))?;
        let security = match (url.scheme(), creds.anonymous, allow_plain) {
            ("ftps", ..) => Security::Implicit,
            (_, true, _) => Security::Plain,
            (_, false, false) => Security::Required,
            (_, false, true) => Security::Preferred,
        };
        Ok(Server {
            host,
            port,
            tls_name,
            security,
            creds,
        })
    }

    /// Clé de la réserve : serveur, chiffrement, identifiant (mot de passe par son
    /// empreinte), chemin réseau.
    fn key(&self, route: &Route) -> String {
        let pass = Sha256::digest(self.creds.pass.as_bytes());
        format!(
            "{}|{}|{:?}|{}|{:x}|{:?}",
            self.host, self.port, self.security, self.creds.user, pass, route
        )
    }
}

/// Erreur de suppaftp, sans le texte de la réponse (il peut citer le chemin).
fn ftp_error(error: FtpError, anonymous: bool) -> NetError {
    match error {
        FtpError::ConnectionError(e) => net_error(e, false),
        FtpError::SecureError(text) => NetError::FtpTls(text),
        FtpError::UnexpectedResponse(r) => {
            let code = u16::try_from(r.status.code()).unwrap_or(0);
            match code {
                530 if anonymous => NetError::AuthRequired,
                530 => NetError::AuthRefused,
                0 => NetError::BadResponse("réponse FTP inattendue"),
                code => NetError::FtpReply(code),
            }
        }
        FtpError::BadResponse => NetError::BadResponse("réponse FTP illisible"),
        FtpError::InvalidAddress(_) => NetError::BadResponse("adresse passive illisible"),
        FtpError::DataConnectionAlreadyOpen => {
            NetError::BadResponse("connexion de données déjà ouverte")
        }
    }
}

fn is_reply(error: &FtpError, codes: &[u32]) -> bool {
    matches!(error, FtpError::UnexpectedResponse(r) if codes.contains(&r.status.code()))
}

/// Le refus vient-il d'`EPSV` (audit F10) ? `500` et `502` : commande inconnue, ce que
/// `RETR` (obligatoire, RFC 959) ne peut pas être. `501` (paramètres) vise le chemin de
/// `RETR`, sauf si la réponse nomme `EPSV` : la connexion ne passe pas en `PASV` pour rien
/// (ce qui échouerait en IPv6).
fn epsv_refused(error: &FtpError) -> bool {
    match error {
        FtpError::UnexpectedResponse(r) => match r.status.code() {
            500 | 502 => true,
            501 => String::from_utf8_lossy(&r.body)
                .to_ascii_uppercase()
                .contains("EPSV"),
            _ => false,
        },
        _ => false,
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

/// Réglages FTP d'un client, partagés par ses copies.
#[derive(Default)]
pub struct Shared {
    /// Autorités ajoutées (`--ca-cert`), pour TLS.
    roots: Vec<CertificateDer<'static>>,
    tls: OnceLock<Result<Arc<rustls::ClientConfig>, String>>,
    pool: Mutex<HashMap<String, Idle>>,
    /// Serveurs (`hôte:port`) qui ont refusé `AUTH TLS` pendant la session.
    no_tls: Mutex<std::collections::HashSet<String>>,
}

/// Connexions de commande d'un serveur et d'un identifiant, hors transfert.
#[derive(Default)]
struct Idle {
    /// Au repos, depuis l'instant noté.
    conns: Vec<(Instant, AsyncRustlsFtpStream)>,
    /// En train d'être rendues (`ABOR` d'une plage abandonnée) : le serveur les compte
    /// encore ; mieux vaut en attendre une qu'en ouvrir une de plus.
    returning: usize,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let idle: usize = self
            .pool
            .lock()
            .map(|p| p.values().map(|i| i.conns.len()).sum())
            .unwrap_or(0);
        f.debug_struct("ftp::Shared")
            .field("roots", &self.roots.len())
            .field("idle", &idle)
            .finish()
    }
}

impl Shared {
    pub fn new(roots: Vec<CertificateDer<'static>>) -> Self {
        Shared {
            roots,
            ..Shared::default()
        }
    }

    /// Configuration TLS : certificats vérifiés par le système, comme pour HTTPS, plus les
    /// autorités ajoutées. Une seule pour le client : les connexions de données
    /// reprennent la session TLS de leur connexion de commande (exigé par certains
    /// serveurs).
    fn tls(&self) -> Result<Arc<rustls::ClientConfig>, NetError> {
        if !super::tls_available() {
            return Err(NetError::TlsUnavailable);
        }
        self.tls
            .get_or_init(|| {
                let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
                let verifier = if self.roots.is_empty() {
                    rustls_platform_verifier::Verifier::new(Arc::clone(&provider))
                } else {
                    #[cfg(not(target_os = "android"))]
                    {
                        rustls_platform_verifier::Verifier::new_with_extra_roots(
                            self.roots.iter().cloned(),
                            Arc::clone(&provider),
                        )
                    }
                    #[cfg(target_os = "android")]
                    {
                        return Err("autorités ajoutées : pas sous Android".to_owned());
                    }
                }
                .map_err(|e| e.to_string())?;
                let config = rustls::ClientConfig::builder_with_provider(provider)
                    .with_safe_default_protocol_versions()
                    .map_err(|e| e.to_string())?
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(verifier))
                    .with_no_client_auth();
                Ok(Arc::new(config))
            })
            .clone()
            .map_err(NetError::FtpTls)
    }

    /// Une connexion au repos, et le nombre de celles qui reviennent.
    fn take(&self, key: &str) -> (Option<AsyncRustlsFtpStream>, usize) {
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(idle) = pool.get_mut(key) else {
            return (None, 0);
        };
        while let Some((since, conn)) = idle.conns.pop() {
            if since.elapsed() < POOL_IDLE {
                return (Some(conn), idle.returning);
            }
        }
        (None, idle.returning)
    }

    /// Rend une connexion ; `returned` : elle était comptée parmi celles qui reviennent
    /// ([`Shared::returning`]), et `conn` vaut `None` si le serveur ne l'a pas rendue.
    fn put(self: &Arc<Self>, key: String, conn: Option<AsyncRustlsFtpStream>, returned: bool) {
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        Self::purge_in(&mut pool);
        let idle = pool.entry(key).or_default();
        if returned {
            idle.returning = idle.returning.saturating_sub(1);
        }
        if let Some(conn) = conn
            && idle.conns.len() < POOL_PER_KEY
        {
            idle.conns.push((Instant::now(), conn));
            // Fermée à la fin de son repos même si plus rien ne passe par la réserve : le
            // serveur ne la garde pas ouverte pour rien (audit F8).
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let shared = Arc::downgrade(self);
                let due = Instant::now() + POOL_IDLE + Duration::from_secs(1);
                runtime.spawn(async move {
                    tokio::time::sleep_until(due).await;
                    if let Some(shared) = shared.upgrade() {
                        shared.purge();
                    }
                });
            }
        }
    }

    /// Ferme les connexions au repos depuis plus de [`POOL_IDLE`].
    fn purge(&self) {
        Self::purge_in(&mut self.pool.lock().unwrap_or_else(PoisonError::into_inner));
    }

    fn purge_in(pool: &mut HashMap<String, Idle>) {
        pool.retain(|_, idle| {
            idle.conns.retain(|(since, _)| since.elapsed() < POOL_IDLE);
            !idle.conns.is_empty() || idle.returning > 0
        });
    }

    /// Connexions au repos, tous serveurs confondus.
    #[cfg(test)]
    fn idle_count(&self) -> usize {
        self.pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|i| i.conns.len())
            .sum()
    }

    /// Le serveur `host:port` est-il sans TLS ? `mark` : le noter.
    fn without_tls(&self, host: &str, port: u16, mark: Option<bool>) -> bool {
        let mut known = self.no_tls.lock().unwrap_or_else(PoisonError::into_inner);
        let server = format!("{host}:{port}");
        if mark == Some(true) {
            known.insert(server);
            return true;
        }
        known.contains(&server)
    }

    /// Une connexion de `key` part se faire rendre.
    fn returning(&self, key: &str) {
        let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
        pool.entry(key.to_owned()).or_default().returning += 1;
    }
}

/// Ce que le client donne au module : délais, proxy, réseau public, réglages partagés.
pub(super) struct Context<'a> {
    pub connect: Duration,
    pub read: Duration,
    pub proxy: proxy::Effective,
    pub public_only: bool,
    pub shared: &'a Arc<Shared>,
}

/// Une connexion de commande ouverte et identifiée, et ce qu'il faut pour la rendre.
struct Session {
    conn: AsyncRustlsFtpStream,
    key: String,
    anonymous: bool,
    /// `EPSV` refusé : `PASV` pour cette connexion.
    passive: bool,
}

impl Context<'_> {
    fn server(
        &self,
        url: &Url,
        task_headers: &[(String, String)],
        auth: Option<&Auth>,
    ) -> Result<(Server, Route), NetError> {
        if self.public_only && reach::literal_forbidden(url) {
            return Err(NetError::AddressForbidden);
        }
        let creds = credentials(url, task_headers, auth);
        let allow_plain = auth.is_some_and(|a| a.allows_plain_for(url));
        let server = Server::of(url, creds, allow_plain)?;
        let route = route_for(&self.proxy, self.public_only, &server.host);
        if server.security == Security::Implicit && matches!(route, Route::Socks { .. }) {
            return Err(NetError::Proxy(
                "ftps:// (TLS implicite) ne passe pas par un proxy SOCKS5 : essayer \
                 l'adresse en ftp://, chiffrée par AUTH TLS"
                    .into(),
            ));
        }
        Ok((server, route))
    }

    /// Une connexion de la réserve qui répond encore, sinon une nouvelle.
    async fn session(&self, server: &Server, route: &Route) -> Result<Session, NetError> {
        let key = server.key(route);
        let deadline = Instant::now() + QUICK;
        loop {
            match self.shared.take(&key) {
                (Some(mut conn), _) => {
                    let alive = tokio::time::timeout(QUICK, conn.noop()).await;
                    if matches!(alive, Ok(Ok(()))) {
                        return Ok(Session {
                            conn,
                            key,
                            anonymous: server.creds.anonymous,
                            passive: false,
                        });
                    }
                }
                // Une connexion revient (`ABOR` en cours) : l'attendre plutôt que d'en
                // ouvrir une de plus, que le serveur pourrait refuser.
                (None, returning) if returning > 0 && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                (None, _) => break,
            }
        }
        let conn = within(self.connect + self.read, self.open(server, route)).await?;
        Ok(Session {
            conn,
            key,
            anonymous: server.creds.anonymous,
            passive: false,
        })
    }

    async fn open(&self, server: &Server, route: &Route) -> Result<AsyncRustlsFtpStream, NetError> {
        let anonymous = server.creds.anonymous;
        let err = |e| ftp_error(e, anonymous);
        let conn = match server.security {
            Security::Implicit => {
                let addrs = resolve(&server.host, server.port, route_public(route)).await?;
                let connector = AsyncRustlsConnector::from(tokio_rustls::TlsConnector::from(
                    self.shared.tls()?,
                ));
                // Chaque adresse à son tour, comme en clair (audit F7).
                let mut conn = AsyncRustlsFtpStream::connect_secure_implicit(
                    &addrs[..],
                    connector,
                    &server.tls_name,
                )
                .await
                .map_err(err)?;
                // TLS implicite : la protection des données se demande ici (suppaftp ne
                // l'envoie qu'après `AUTH TLS`).
                for command in ["PBSZ 0", "PROT P"] {
                    conn.custom_command(command, &[suppaftp::Status::CommandOk])
                        .await
                        .map_err(err)?;
                }
                conn
            }
            Security::Plain => self.plain(server, route).await?,
            // Serveur déjà vu sans TLS, site autorisé sans chiffrement : en clair d'emblée,
            // sans une connexion de plus à chaque segment.
            Security::Preferred if self.shared.without_tls(&server.host, server.port, None) => {
                self.plain(server, route).await?
            }
            Security::Required | Security::Preferred => {
                let conn = self.plain(server, route).await?;
                let connector = AsyncRustlsConnector::from(tokio_rustls::TlsConnector::from(
                    self.shared.tls()?,
                ));
                match conn.into_secure(connector, &server.tls_name).await {
                    Ok(conn) => conn,
                    // `AUTH TLS` inconnu ou refusé : la connexion n'est plus utilisable.
                    Err(e) if is_reply(&e, &[500, 501, 502, 504, 530, 534, 431]) => {
                        if server.security == Security::Required {
                            return Err(NetError::FtpTlsMissing);
                        }
                        self.shared
                            .without_tls(&server.host, server.port, Some(true));
                        self.plain(server, route).await?
                    }
                    Err(e) => return Err(err(e)),
                }
            }
        };
        let mut conn = self.with_data_route(conn, server, route);
        conn.set_mode(Mode::ExtendedPassive);
        conn.set_passive_nat_workaround(true);
        conn.login(server.creds.user.as_str(), server.creds.pass.as_str())
            .await
            .map_err(err)?;
        conn.transfer_type(FileType::Binary).await.map_err(err)?;
        tracing::debug!(host = %server.host, tls = ?server.security, "connexion FTP ouverte");
        Ok(conn)
    }

    async fn plain(
        &self,
        server: &Server,
        route: &Route,
    ) -> Result<AsyncRustlsFtpStream, NetError> {
        let stream = dial(route, &server.host, server.port, None).await?;
        AsyncRustlsFtpStream::connect_with_stream(stream)
            .await
            .map_err(|e| ftp_error(e, server.creds.anonymous))
    }

    /// Connexions de données : vers l'hôte de la commande, au port annoncé, par le même
    /// chemin.
    fn with_data_route(
        &self,
        conn: AsyncRustlsFtpStream,
        server: &Server,
        route: &Route,
    ) -> AsyncRustlsFtpStream {
        let (route, host) = (route.clone(), server.host.clone());
        conn.passive_stream_builder(move |announced: SocketAddr| {
            let (route, host) = (route.clone(), host.clone());
            // Tâche à part : le futur rendu doit être `Sync`, ce que la connexion n'est pas.
            // Arrêtée avec lui s'il est abandonné (délai de la commande dépassé, audit F5).
            let mut task = AbortOnDrop(tokio::spawn(async move {
                // En direct, l'adresse annoncée est déjà celle de la commande (`EPSV`, ou
                // `PASV` corrigé) ; par SOCKS5, c'est celle du proxy : le port seul compte.
                let peer = matches!(route, Route::Direct { .. }).then_some(announced);
                dial(&route, &host, announced.port(), peer).await
            }));
            Box::pin(async move {
                match (&mut task.0).await {
                    Ok(Ok(stream)) => Ok(stream),
                    Ok(Err(e)) => Err(FtpError::ConnectionError(std::io::Error::other(
                        e.to_string(),
                    ))),
                    Err(e) => Err(FtpError::ConnectionError(std::io::Error::other(
                        e.to_string(),
                    ))),
                }
            })
        })
    }

    /// Préparation (SPEC §7.1) : taille, date, lecture à une position.
    pub async fn probe(
        &self,
        url: &Url,
        task_headers: &[(String, String)],
        auth: Option<&Auth>,
    ) -> Result<Probe, NetError> {
        let path = file_path(url)?;
        let (server, route) = self.server(url, task_headers, auth)?;
        let mut session = self.session(&server, &route).await?;
        let found = within(self.read, Self::facts(&mut session, &path)).await?;
        let (total, modified, rest) = found;
        self.shared.put(session.key, Some(session.conn), false);
        let mut final_url = url.clone();
        let _ = final_url.set_username("");
        let _ = final_url.set_password(None);
        let name = path.rsplit('/').next().unwrap_or_default();
        Ok(Probe {
            final_url,
            total_bytes: total,
            accepts_ranges: total.is_some() && rest,
            etag: None,
            last_modified: modified,
            file_name: filename::sanitize(name, filename::NameRules::native()),
            downgraded: false,
            content_type: None,
            attachment: false,
        })
    }

    async fn facts(
        session: &mut Session,
        path: &str,
    ) -> Result<(Option<u64>, Option<String>, bool), NetError> {
        let anonymous = session.anonymous;
        let total = match session.conn.size(path).await {
            Ok(n) => Some(n as u64),
            // `SIZE` inconnu : taille inconnue, un seul flux.
            Err(e) if is_reply(&e, &[500, 502, 504]) => None,
            Err(e) => return Err(ftp_error(e, anonymous)),
        };
        let modified = Self::modified(session, path).await?;
        let rest = match session.conn.feat().await {
            Ok(features) => features.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("REST")
                    && value
                        .as_deref()
                        .is_some_and(|v| v.to_ascii_uppercase().contains("STREAM"))
            }),
            Err(e) if is_reply(&e, &[500, 502, 504]) => false,
            Err(e) => return Err(ftp_error(e, anonymous)),
        };
        Ok((total, modified, rest))
    }

    async fn modified(session: &mut Session, path: &str) -> Result<Option<String>, NetError> {
        match session.conn.mdtm(path).await {
            Ok(date) => Ok(Some(date.strftime("%Y%m%d%H%M%S").to_string())),
            Err(e) if is_reply(&e, &[500, 501, 502, 504, 550]) => Ok(None),
            Err(FtpError::BadResponse) => Ok(None),
            Err(e) => Err(ftp_error(e, session.anonymous)),
        }
    }

    /// Plage `[first, last]` d'un fichier de `total` octets, après avoir vérifié qu'il n'a
    /// pas changé (taille, et date si `if_range` la porte).
    pub async fn get_range(
        &self,
        url: &Url,
        task_headers: &[(String, String)],
        req: &RangeRequest<'_>,
        auth: Option<&Auth>,
    ) -> Result<FtpBody, NetError> {
        let path = file_path(url)?;
        let (server, route) = self.server(url, task_headers, auth)?;
        let mut session = self.session(&server, &route).await?;
        let checked = within(self.read, async {
            let size = session
                .conn
                .size(&path)
                .await
                .map_err(|e| ftp_error(e, session.anonymous))?;
            if size as u64 != req.total {
                return Err(NetError::FileChanged);
            }
            if let Some(expected) = req.if_range
                && Self::modified(&mut session, &path).await?.as_deref() != Some(expected)
            {
                return Err(NetError::FileChanged);
            }
            Ok(())
        })
        .await;
        checked?;
        let stream = within(self.read, Self::retrieve(&mut session, &path, req.first)).await?;
        Ok(FtpBody {
            session: Some(session),
            stream: Some(stream),
            remaining: Some(req.last - req.first + 1),
            to_end: req.last + 1 == req.total,
            shared: Arc::clone(self.shared),
            read: self.read,
        })
    }

    /// Le fichier entier, en un seul flux ; `expected_total` : taille connue, revue.
    pub async fn get_all(
        &self,
        url: &Url,
        task_headers: &[(String, String)],
        expected_total: Option<u64>,
        auth: Option<&Auth>,
    ) -> Result<FtpBody, NetError> {
        let path = file_path(url)?;
        let (server, route) = self.server(url, task_headers, auth)?;
        let mut session = self.session(&server, &route).await?;
        if let Some(expected) = expected_total {
            let size = within(self.read, async {
                match session.conn.size(&path).await {
                    Ok(n) => Ok(Some(n as u64)),
                    Err(e) if is_reply(&e, &[500, 502, 504]) => Ok(None),
                    Err(e) => Err(ftp_error(e, session.anonymous)),
                }
            })
            .await?;
            if size.is_some_and(|s| s != expected) {
                return Err(NetError::FileChanged);
            }
        }
        let stream = within(self.read, Self::retrieve(&mut session, &path, 0)).await?;
        Ok(FtpBody {
            session: Some(session),
            stream: Some(stream),
            remaining: expected_total,
            to_end: true,
            shared: Arc::clone(self.shared),
            read: self.read,
        })
    }

    /// `REST` puis `RETR` ; `EPSV` refusé : `PASV` pour cette connexion.
    async fn retrieve(session: &mut Session, path: &str, first: u64) -> Result<Stream, NetError> {
        let offset = usize::try_from(first)
            .map_err(|_| NetError::BadResponse("position trop grande pour ce système"))?;
        loop {
            if offset == 0 {
                // Certains serveurs (libunftp) gardent la position d'un `REST` après un
                // transfert interrompu par `ABOR` : remise à zéro explicite. Un serveur
                // sans `REST` le refuse, sans conséquence.
                let pending = [suppaftp::Status::RequestFilePending];
                match session.conn.custom_command("REST 0", &pending).await {
                    Ok(_) | Err(FtpError::UnexpectedResponse(_)) => {}
                    Err(e) => return Err(ftp_error(e, session.anonymous)),
                }
            }
            let _ = session.conn.resume_transfer(offset).await;
            match session.conn.retr_as_stream(path).await {
                Ok(stream) => return Ok(stream),
                Err(e) if !session.passive && epsv_refused(&e) => {
                    session.passive = true;
                    session.conn.set_mode(Mode::Passive);
                }
                Err(e) => return Err(ftp_error(e, session.anonymous)),
            }
        }
    }
}

/// Tâche arrêtée quand sa poignée est abandonnée.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Octets d'un `RETR` : au plus ceux de la plage, puis `ABOR` (ou la réponse de fin à la
/// fin du fichier) et la connexion de commande rendue à la réserve.
pub struct FtpBody {
    session: Option<Session>,
    stream: Option<Stream>,
    /// Octets encore attendus ; `None` : jusqu'à la fin du fichier.
    remaining: Option<u64>,
    /// La plage va jusqu'à la fin du fichier : le serveur finit de lui-même.
    to_end: bool,
    shared: Arc<Shared>,
    read: Duration,
}

impl std::fmt::Debug for FtpBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FtpBody")
            .field("remaining", &self.remaining)
            .finish_non_exhaustive()
    }
}

impl Drop for FtpBody {
    /// Plage abandonnée avant sa fin (segment redécoupé, arrêt) : `ABOR` en tâche de fond,
    /// et la connexion de commande rendue à la réserve si le serveur l'accepte.
    fn drop(&mut self) {
        let (Some(stream), Some(mut session)) = (self.stream.take(), self.session.take()) else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let shared = Arc::clone(&self.shared);
        shared.returning(&session.key);
        runtime.spawn(async move {
            let aborted = tokio::time::timeout(QUICK, session.conn.abort(stream)).await;
            let conn = matches!(aborted, Ok(Ok(()))).then_some(session.conn);
            shared.put(session.key, conn, true);
        });
    }
}

impl FtpBody {
    /// Morceau suivant ; `None` à la fin. Une connexion de données fermée avant la fin
    /// attendue est une erreur réseau ; sans taille attendue, la réponse du serveur dit si
    /// le fichier est complet.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, NetError> {
        let Some(stream) = self.stream.as_mut() else {
            return Ok(None);
        };
        let want = self
            .remaining
            .map_or(CHUNK, |r| usize::try_from(r).unwrap_or(CHUNK).min(CHUNK));
        if want == 0 {
            self.close().await?;
            return Ok(None);
        }
        let mut buf = BytesMut::with_capacity(want);
        let n = tokio::time::timeout(self.read, stream.read_buf(&mut buf))
            .await
            .map_err(|_| NetError::ReadTimeout(self.read))?
            .map_err(|e| net_error(e, false))?;
        if n == 0 {
            if self.remaining.is_some() {
                self.stream = None;
                self.session = None;
                return Err(NetError::Socket {
                    connect: false,
                    text: "connexion de données fermée avant la fin".into(),
                });
            }
            self.close().await?;
            return Ok(None);
        }
        if let Some(r) = &mut self.remaining {
            *r -= n as u64;
            if *r == 0 {
                // Fin de la plage : la connexion rendue tout de suite, sans attendre
                // l'appel suivant (que l'écrivain peut ne pas faire).
                let _ = self.close().await;
            }
        }
        Ok(Some(buf.freeze()))
    }

    /// Termine le transfert : réponse de fin (fichier lu jusqu'au bout) ou `ABOR`, puis
    /// la connexion de commande rendue à la réserve si tout s'est bien passé. Une erreur
    /// n'est rendue que pour un flux sans taille attendue (fichier peut-être incomplet).
    async fn close(&mut self) -> Result<(), NetError> {
        let (Some(stream), Some(mut session)) = (self.stream.take(), self.session.take()) else {
            return Ok(());
        };
        let anonymous = session.anonymous;
        let ended = if self.to_end {
            tokio::time::timeout(QUICK, stream.finish()).await
        } else {
            tokio::time::timeout(QUICK, session.conn.abort(stream)).await
        };
        match ended {
            Ok(Ok(())) => {
                self.shared.put(session.key, Some(session.conn), false);
                Ok(())
            }
            Ok(Err(e)) if self.remaining.is_none() => Err(ftp_error(e, anonymous)),
            Err(_) if self.remaining.is_none() => Err(NetError::ReadTimeout(QUICK)),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "ftp_tests.rs"]
mod tests;
