//! Clients HTTP(S) et FTP, TLS, proxies. Voir SPEC §2.1 et §7.1.
//!
//! Fait : préparation d'une tâche HTTP(S) ([`Client::probe`], SPEC §7.1), requêtes de
//! segments avec `If-Range` ([`Client::get_range`], SPEC §7.4) et flux unique
//! ([`Client::get_all`]), noms de fichier ([`filename`]), classement des erreurs selon la
//! SPEC §6.8. TLS par `rustls`, avec les certificats racines du système
//! (`rustls-platform-verifier`). En-têtes d'une tâche ([`check_task_headers`]) : ceux de
//! la page transmis par l'extension, cookies compris (F-06).
//!
//! Authentification Basic et Digest avec les identifiants d'un site ([`auth`], F-06,
//! SPEC §7.14).
//!
//! À faire : FTP, proxies (F-07), HTTP/3 (F-05).

pub mod auth;
pub mod filename;
pub mod headers;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use reqwest::header::{
    ACCEPT_ENCODING, ACCEPT_RANGES, AUTHORIZATION, CONTENT_DISPOSITION, CONTENT_ENCODING,
    CONTENT_LENGTH, CONTENT_RANGE, ETAG, HeaderMap, HeaderName, HeaderValue, IF_RANGE,
    LAST_MODIFIED, RANGE, RETRY_AFTER, WWW_AUTHENTICATE,
};
use reqwest::{Method, Response, StatusCode, Url, redirect};

pub use bytes::Bytes;

use auth::{Answer, Auth};
use filename::NameRules;

/// `User-Agent` par défaut ; un en-tête de la tâche le remplace (F-06).
pub const USER_AGENT: &str = concat!("AuroDownloader/", env!("CARGO_PKG_VERSION"));

/// Délai maximal d'établissement d'une connexion (TCP et TLS).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Délai maximal sans recevoir d'octet sur une connexion établie, et pour obtenir les
/// en-têtes d'une réponse. Il ne court que pendant que le moteur attend le serveur : pas
/// pendant qu'une connexion attend son tour de débit (SPEC §7.6).
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Nombre maximal de redirections suivies.
pub const MAX_REDIRECTS: usize = 10;

/// Erreurs réseau, classées selon les codes de la SPEC §6.8.
///
/// Les messages ne contiennent jamais l'URL : elle peut porter un jeton (SPEC §8.2).
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("adresse invalide : {0}")]
    InvalidUrl(String),

    #[error("protocole non pris en charge : {0}")]
    UnsupportedScheme(String),

    #[error("en-tête de la tâche invalide : {0}")]
    BadHeader(String),

    #[error("fichier d'autorités de certification invalide : {0}")]
    BadRootCertificates(&'static str),

    #[error("le serveur a répondu {status}")]
    Status {
        status: u16,
        retry_after: Option<Duration>,
    },

    #[error("redirection refusée : {0}")]
    Redirect(#[source] reqwest::Error),

    #[error(
        "redirection non suivie (statut {status}) : destination absente ou dans un \
         protocole non pris en charge"
    )]
    RedirectNotFollowed { status: u16 },

    #[error("délai dépassé : {0}")]
    Timeout(#[source] reqwest::Error),

    #[error("délai dépassé : rien reçu du serveur pendant {} s", .0.as_secs_f32())]
    ReadTimeout(Duration),

    #[error("certificat refusé : {0}")]
    InvalidCertificate(#[source] reqwest::Error),

    #[error("HTTPS indisponible : la vérification des certificats n'est pas initialisée")]
    TlsUnavailable,

    #[error("serveur injoignable : {0}")]
    Unreachable(#[source] reqwest::Error),

    #[error("le fichier distant a changé depuis le début du téléchargement")]
    FileChanged,

    #[error("le serveur n'accepte plus les requêtes par plage : reprise impossible")]
    RangeIgnored,

    #[error("réponse incohérente du serveur : {0}")]
    BadResponse(&'static str),

    #[error(
        "le serveur demande un nom d'utilisateur et un mot de passe : les enregistrer pour \n         ce site (identifiants de sites)"
    )]
    AuthRequired,

    #[error("le serveur refuse les identifiants enregistrés pour ce site")]
    AuthRefused,

    #[error(
        "le serveur demande le mot de passe en clair (Basic) sur une connexion non \n         chiffrée : refusé, il pourrait être lu en chemin"
    )]
    AuthInsecure,
}

impl NetError {
    fn from_reqwest(error: reqwest::Error) -> Self {
        let error = error.without_url();
        if has_cause::<NoTls>(&error) {
            NetError::TlsUnavailable
        } else if error.is_redirect() {
            NetError::Redirect(error)
        } else if is_certificate_error(&error) {
            NetError::InvalidCertificate(error)
        } else if error.is_timeout() {
            NetError::Timeout(error)
        } else {
            NetError::Unreachable(error)
        }
    }

    /// Code `error_code` de la tâche (SPEC §6.8). `None` pour une demande invalide, refusée
    /// avant toute tâche (erreur de protocole, SPEC §6.7).
    pub fn error_code(&self) -> Option<&'static str> {
        match self {
            NetError::InvalidUrl(_)
            | NetError::UnsupportedScheme(_)
            | NetError::BadHeader(_)
            | NetError::BadRootCertificates(_) => None,
            NetError::Status { .. }
            | NetError::Redirect(_)
            | NetError::RedirectNotFollowed { .. }
            | NetError::BadResponse(_) => Some("HTTP_STATUS"),
            NetError::Timeout(_) | NetError::ReadTimeout(_) => Some("NET_TIMEOUT"),
            NetError::InvalidCertificate(_) => Some("TLS_INVALID_CERT"),
            NetError::TlsUnavailable => Some("TLS_UNAVAILABLE"),
            NetError::Unreachable(_) => Some("NET_UNREACHABLE"),
            NetError::FileChanged => Some("FILE_CHANGED"),
            NetError::RangeIgnored => Some("RANGE_UNSUPPORTED"),
            NetError::AuthRequired => Some("AUTH_REQUIRED"),
            NetError::AuthRefused | NetError::AuthInsecure => Some("AUTH_FAILED"),
        }
    }

    /// Le moteur réessaie-t-il seul (colonne « Réessai » de la SPEC §6.8) ?
    pub fn retryable(&self) -> bool {
        match self {
            NetError::Status { status, .. } => {
                matches!(status, 408 | 429) || (500..600).contains(status)
            }
            NetError::Timeout(_) | NetError::ReadTimeout(_) | NetError::Unreachable(_) => true,
            _ => false,
        }
    }

    /// Le serveur résiste : réponse 429 ou 503, ou connexion refusée. En mode automatique,
    /// le moteur divise alors ses connexions par deux (SPEC §7.3).
    pub fn is_overload(&self) -> bool {
        match self {
            NetError::Status { status, .. } => matches!(status, 429 | 503),
            NetError::Unreachable(e) => e.is_connect(),
            _ => false,
        }
    }

    /// Délai demandé par le serveur (`Retry-After`), qui prime sur le calcul de la
    /// SPEC §7.8.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            NetError::Status { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

/// HTTPS est-il utilisable ? Sous Android, `rustls-platform-verifier` doit d'abord être
/// initialisé par l'application (JNI et `Context`), sans quoi il panique au premier
/// certificat. Cette initialisation arrive avec l'application Android (SPEC §7.5, phase 4) :
/// d'ici là, le moteur refuse HTTPS sous Android par une erreur.
pub fn tls_available() -> bool {
    !cfg!(target_os = "android")
}

/// Cause posée par la politique de redirection quand elle refuse HTTPS faute de TLS.
#[derive(Debug)]
struct NoTls;

impl std::fmt::Display for NoTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTPS indisponible")
    }
}

impl std::error::Error for NoTls {}

/// Une erreur de type `T` figure-t-elle dans la chaîne des causes ?
fn has_cause<T: std::error::Error + 'static>(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(e) = current {
        if e.is::<T>() {
            return true;
        }
        current = e.source();
    }
    false
}

/// Cherche un certificat refusé par `rustls` dans la chaîne des causes, y compris dans
/// une erreur d'entrée-sortie qui l'enveloppe (dont `source()` saute l'erreur portée).
fn is_certificate_error(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(e) = current {
        if let Some(tls) = e.downcast_ref::<rustls::Error>() {
            if matches!(tls, rustls::Error::InvalidCertificate(_)) {
                return true;
            }
        }
        if let Some(inner) = e
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
        {
            if is_certificate_error(inner) {
                return true;
            }
        }
        current = e.source();
    }
    false
}

/// Résultat de la préparation d'une tâche (SPEC §7.1), à ranger dans `sources` et `tasks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// Adresse après les redirections : celle des requêtes suivantes.
    pub final_url: Url,
    /// Taille du fichier ; `None` si inconnue (flux unique, sans segments).
    pub total_bytes: Option<u64>,
    /// Le serveur accepte les requêtes par plage : segmentation et reprise possibles.
    pub accepts_ranges: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Nom proposé, nettoyé pour le système (F-09).
    pub file_name: String,
    /// Une redirection est passée de HTTPS à HTTP : à signaler (SPEC §8.1).
    pub downgraded: bool,
}

impl Probe {
    /// `ETag` utilisable avec `If-Range` pour la reprise (SPEC §7.4, étape 3). Le transfert
    /// applique le même filtre à l'`ETag` enregistré de la source.
    #[cfg(test)]
    pub fn strong_etag(&self) -> Option<&str> {
        self.etag.as_deref().filter(|e| headers::is_strong_etag(e))
    }
}

/// Ce qu'une réponse dit du fichier.
#[derive(Debug, Default)]
struct Observed {
    length: Option<u64>,
    encoded: bool,
    accept_ranges: Option<bool>,
    content_range: Option<headers::ContentRange>,
    etag: Option<String>,
    last_modified: Option<String>,
    disposition_name: Option<String>,
}

impl Observed {
    fn from_headers(h: &HeaderMap) -> Self {
        let text = |name| h.get(name).and_then(|v: &HeaderValue| v.to_str().ok());
        let encoded = text(CONTENT_ENCODING).is_some_and(headers::is_encoded);
        Observed {
            // Lu dans l'en-tête : pour un HEAD, la longueur du corps vaut zéro.
            length: text(CONTENT_LENGTH).and_then(headers::content_length),
            encoded,
            accept_ranges: text(ACCEPT_RANGES).and_then(headers::accept_ranges),
            content_range: text(CONTENT_RANGE).and_then(headers::content_range),
            etag: text(ETAG).map(str::to_owned),
            last_modified: text(LAST_MODIFIED).map(str::to_owned),
            disposition_name: h
                .get(CONTENT_DISPOSITION)
                .and_then(|v| headers::content_disposition_filename(v.as_bytes())),
        }
    }

    /// Taille du fichier d'après `Content-Length`, si le corps n'est pas transformé.
    fn file_length(&self) -> Option<u64> {
        self.length.filter(|_| !self.encoded)
    }
}

/// Client HTTP(S) du moteur, partagé par ses tâches pendant toute la session.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    /// Délai de lecture, appliqué ici et non par reqwest : le sien court aussi entre deux
    /// lectures, pendant l'attente d'un seau de débit.
    read: Duration,
    /// Plafond de connexions par hôte, appris d'un serveur qui a résisté (SPEC §7.3).
    host_caps: Arc<Mutex<HashMap<String, u32>>>,
}

impl Client {
    /// Client avec les délais par défaut.
    pub fn new() -> Result<Self, NetError> {
        Self::with_timeouts(CONNECT_TIMEOUT, READ_TIMEOUT)
    }

    /// Client avec des délais choisis (tests, réglages).
    pub fn with_timeouts(connect: Duration, read: Duration) -> Result<Self, NetError> {
        Self::build(connect, read, Vec::new())
    }

    /// Client qui ajoute aux racines du système les autorités d'un fichier PEM : serveur
    /// interne, banc de performance. La validation reste aussi stricte (N-07) : seules les
    /// racines de confiance changent.
    pub fn with_extra_roots(pem: &[u8]) -> Result<Self, NetError> {
        let roots = reqwest::Certificate::from_pem_bundle(pem)
            .map_err(|_| NetError::BadRootCertificates("PEM illisible"))?;
        if roots.is_empty() {
            return Err(NetError::BadRootCertificates("aucun certificat"));
        }
        // Sous Android, rustls-platform-verifier refuse aussi ces racines avant d'être
        // initialisé (voir `tls_available`).
        if !tls_available() {
            return Err(NetError::TlsUnavailable);
        }
        Self::build(CONNECT_TIMEOUT, READ_TIMEOUT, roots)
    }

    fn build(
        connect: Duration,
        read: Duration,
        roots: Vec<reqwest::Certificate>,
    ) -> Result<Self, NetError> {
        let policy = redirect::Policy::custom(|attempt| {
            // `previous()` contient aussi l'adresse de départ.
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error("trop de redirections")
            } else if !matches!(attempt.url().scheme(), "http" | "https") {
                // Jamais vers file:// ni un autre protocole (SPEC §8.1). reqwest ne
                // consulte pas la politique pour ces protocoles (voir `send`) : seconde
                // barrière, si cela changeait.
                let scheme = attempt.url().scheme().to_owned();
                attempt.error(format!("redirection vers le protocole {scheme}"))
            } else if attempt.url().scheme() == "https" && !tls_available() {
                attempt.error(NoTls)
            } else {
                attempt.follow()
            }
        });
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(connect)
            .redirect(policy)
            // Pas de `Referer` inventé après une redirection : reqwest y mettrait l'adresse
            // qui redirige, signature comprise, à l'intention d'un autre hôte. Celui de la
            // tâche (page d'origine) reste, comme dans un navigateur.
            .referer(false)
            .tls_certs_merge(roots)
            .build()
            .map_err(NetError::from_reqwest)?;
        Ok(Client {
            http,
            read,
            host_caps: Arc::default(),
        })
    }

    /// Préparation d'une tâche (SPEC §7.1) : taille, acceptation des plages, validateurs
    /// et nom du fichier.
    ///
    /// Un `HEAD` qui annonce `Accept-Ranges: bytes` et une taille suffit ; s'il échoue ou
    /// reste muet sur l'un des deux, un `GET` avec `Range: bytes=0-0` tranche : `206`
    /// prouve les plages, `200` dit que le serveur les ignore (F-04). Chaque requête de
    /// segment vérifiera son `206` de toute façon.
    pub async fn probe(
        &self,
        url: &str,
        task_headers: &[(String, String)],
    ) -> Result<Probe, NetError> {
        self.probe_as(url, task_headers, None).await
    }

    /// Comme [`Client::probe`], en répondant aux demandes d'identification avec `auth`.
    pub async fn probe_as(
        &self,
        url: &str,
        task_headers: &[(String, String)],
        auth: Option<&Auth>,
    ) -> Result<Probe, NetError> {
        let url = parse_url(url)?;
        let extra = header_map(task_headers)?;

        let head = self.send(Method::HEAD, &url, &extra, None, auth).await?;
        let head_url = head.url().clone();
        let from_head = head
            .status()
            .is_success()
            .then(|| Observed::from_headers(head.headers()));
        drop(head);

        if let Some(h) = &from_head {
            match (h.accept_ranges, h.file_length()) {
                (Some(false), length) => {
                    return Ok(conclude(&url, head_url, None, h, length, false));
                }
                (Some(true), Some(length)) if !h.encoded => {
                    return Ok(conclude(&url, head_url, None, h, Some(length), true));
                }
                _ => {}
            }
        }

        let get = self
            .send(Method::GET, &url, &extra, Some("bytes=0-0"), auth)
            .await?;
        let status = get.status();
        let get_url = get.url().clone();
        let g = Observed::from_headers(get.headers());
        let retry_after = retry_after_of(&get);
        // Le corps n'est jamais lu : le fermer coupe un serveur qui enverrait tout.
        drop(get);

        let fallback = from_head.as_ref();
        let (total, ranges) = match status {
            StatusCode::PARTIAL_CONTENT if !g.encoded => match g.content_range {
                Some(cr) if cr.range.is_some() => (cr.total, true),
                // 206 sans plage lisible : rien n'est prouvé.
                _ => (fallback.and_then(Observed::file_length), false),
            },
            StatusCode::RANGE_NOT_SATISFIABLE => match g.content_range {
                // Fichier vide : l'octet 0 n'existe pas.
                Some(headers::ContentRange {
                    range: None,
                    total: Some(total),
                }) => (Some(total), true),
                _ => {
                    return Err(NetError::Status {
                        status: status.as_u16(),
                        retry_after,
                    });
                }
            },
            s if s.is_success() => (
                g.file_length()
                    .or_else(|| fallback.and_then(Observed::file_length))
                    .filter(|_| !g.encoded),
                false,
            ),
            s => {
                return Err(NetError::Status {
                    status: s.as_u16(),
                    retry_after,
                });
            }
        };
        Ok(conclude(&url, get_url, fallback, &g, total, ranges))
    }

    /// Requête d'une plage `[first, last]` d'un fichier de `total` octets (SPEC §7.2).
    ///
    /// `if_range` porte l'`ETag` fort ou la date `Last-Modified` mémorisés (SPEC §7.4,
    /// étape 3). Seul un `206` qui commence à `first`, ne dépasse pas `last` et annonce
    /// `total` est accepté. Un `200` en réponse à `If-Range`, une autre taille ou un `416`
    /// signifient que le fichier a changé ([`NetError::FileChanged`]) ; un `200` sans
    /// `If-Range`, que le serveur ignore désormais les plages.
    pub async fn get_range(
        &self,
        url: &str,
        task_headers: &[(String, String)],
        req: &RangeRequest<'_>,
    ) -> Result<Body, NetError> {
        self.get_range_as(url, task_headers, req, None).await
    }

    /// Comme [`Client::get_range`], en répondant aux demandes d'identification avec `auth`.
    pub async fn get_range_as(
        &self,
        url: &str,
        task_headers: &[(String, String)],
        req: &RangeRequest<'_>,
        auth: Option<&Auth>,
    ) -> Result<Body, NetError> {
        let url = parse_url(url)?;
        let mut extra = header_map(task_headers)?;
        if let Some(validator) = req.if_range {
            let value = HeaderValue::from_str(validator)
                .map_err(|_| NetError::BadHeader("validateur If-Range invalide".into()))?;
            extra.insert(IF_RANGE, value);
        }
        let range = format!("bytes={}-{}", req.first, req.last);
        let response = self
            .send(Method::GET, &url, &extra, Some(&range), auth)
            .await?;
        let seen = Observed::from_headers(response.headers());
        match response.status() {
            StatusCode::PARTIAL_CONTENT => {
                if seen.encoded {
                    return Err(NetError::BadResponse("plage compressée"));
                }
                let cr = seen
                    .content_range
                    .ok_or(NetError::BadResponse("206 sans Content-Range lisible"))?;
                if cr.total.is_some_and(|t| t != req.total) {
                    return Err(NetError::FileChanged);
                }
                match (cr.range, cr.total) {
                    (Some((first, last)), Some(_)) if first == req.first && last <= req.last => {
                        Ok(Body {
                            response,
                            read: self.read,
                        })
                    }
                    _ => Err(NetError::BadResponse(
                        "plage reçue différente de la demande",
                    )),
                }
            }
            StatusCode::OK if req.if_range.is_some() => Err(NetError::FileChanged),
            StatusCode::OK => Err(NetError::RangeIgnored),
            StatusCode::RANGE_NOT_SATISFIABLE => Err(NetError::FileChanged),
            s if s.is_success() => Err(NetError::BadResponse("statut inattendu")),
            _ => Err(status_error(&response)),
        }
    }

    /// Requête du fichier entier, en un seul flux (F-04). Si `expected_total` est connu, une
    /// autre taille annoncée signifie que le fichier a changé.
    pub async fn get_all(
        &self,
        url: &str,
        task_headers: &[(String, String)],
        expected_total: Option<u64>,
    ) -> Result<Body, NetError> {
        self.get_all_as(url, task_headers, expected_total, None)
            .await
    }

    /// Comme [`Client::get_all`], en répondant aux demandes d'identification avec `auth`.
    pub async fn get_all_as(
        &self,
        url: &str,
        task_headers: &[(String, String)],
        expected_total: Option<u64>,
        auth: Option<&Auth>,
    ) -> Result<Body, NetError> {
        let url = parse_url(url)?;
        let extra = header_map(task_headers)?;
        let response = self.send(Method::GET, &url, &extra, None, auth).await?;
        if response.status() != StatusCode::OK {
            return Err(if response.status().is_success() {
                NetError::BadResponse("statut inattendu")
            } else {
                status_error(&response)
            });
        }
        let seen = Observed::from_headers(response.headers());
        if let Some(expected) = expected_total {
            if seen.encoded {
                return Err(NetError::BadResponse("corps compressé"));
            }
            if seen.length.is_some_and(|l| l != expected) {
                return Err(NetError::FileChanged);
            }
        }
        Ok(Body {
            response,
            read: self.read,
        })
    }

    /// Plafond de connexions retenu pour `host` pendant la session, s'il y en a un.
    pub fn host_cap(&self, host: &str) -> Option<u32> {
        let caps = self
            .host_caps
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        caps.get(host).copied()
    }

    /// Retient au plus `cap` connexions vers `host` pour le reste de la session ; un
    /// plafond déjà plus bas est gardé.
    pub fn limit_host(&self, host: &str, cap: u32) {
        let mut caps = self
            .host_caps
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let cap = cap.max(1);
        let entry = caps.entry(host.to_owned()).or_insert(cap);
        *entry = (*entry).min(cap);
    }

    /// Envoie une requête ; à une demande d'identification (`401` avec
    /// `WWW-Authenticate`), répond avec `auth` et recommence, à l'adresse qui l'a demandée.
    async fn send(
        &self,
        method: Method,
        url: &Url,
        extra: &HeaderMap,
        range: Option<&str>,
        auth: Option<&Auth>,
    ) -> Result<Response, NetError> {
        let mut target = url.clone();
        // Défi initial, puis au plus un nonce périmé (Digest).
        for _ in 0..3 {
            let mut headers = extra.clone();
            let authorization = auth.and_then(|a| a.authorization(&method, &target));
            let sent = authorization.is_some();
            if let Some(value) = authorization {
                headers.insert(AUTHORIZATION, value);
            }
            let response = self
                .send_once(method.clone(), &target, &headers, range)
                .await?;
            if response.status() != StatusCode::UNAUTHORIZED
                || !response.headers().contains_key(WWW_AUTHENTICATE)
            {
                return Ok(response);
            }
            let Some(auth) = auth else {
                return Err(NetError::AuthRequired);
            };
            let challenged = response.url().clone();
            // Après une redirection vers un autre hôte, rien n'a été envoyé à celui-ci.
            let refused = sent && challenged == target;
            match auth.on_challenge(&challenged, response.headers(), refused) {
                Answer::Retry => target = challenged,
                Answer::Insecure => return Err(NetError::AuthInsecure),
                Answer::Unanswered if refused => return Err(NetError::AuthRefused),
                Answer::Unanswered => return Err(NetError::AuthRequired),
            }
        }
        Err(NetError::AuthRefused)
    }

    async fn send_once(
        &self,
        method: Method,
        url: &Url,
        extra: &HeaderMap,
        range: Option<&str>,
    ) -> Result<Response, NetError> {
        if url.scheme() == "https" && !tls_available() {
            return Err(NetError::TlsUnavailable);
        }
        let mut request = self
            .http
            .request(method, url.clone())
            .headers(extra.clone());
        // Une compression ferait dériver les décalages d'octets (SPEC §7.1, étape 4).
        request = request.header(ACCEPT_ENCODING, "identity");
        if let Some(range) = range {
            request = request.header(RANGE, range);
        }
        let response = tokio::time::timeout(self.read, request.send())
            .await
            .map_err(|_| NetError::ReadTimeout(self.read))?
            .map_err(NetError::from_reqwest)?;
        // Une redirection qui arrive jusqu'ici n'a pas été suivie : reqwest rend telle
        // quelle celle qui vise un autre protocole (file://, ftp://…).
        if response.status().is_redirection() {
            return Err(NetError::RedirectNotFollowed {
                status: response.status().as_u16(),
            });
        }
        Ok(response)
    }
}

/// Plage demandée par [`Client::get_range`], bornes incluses.
#[derive(Debug, Clone, Copy)]
pub struct RangeRequest<'a> {
    pub first: u64,
    pub last: u64,
    /// Taille du fichier mémorisée à la préparation.
    pub total: u64,
    /// Valeur de `If-Range` : `ETag` fort ou date `Last-Modified`.
    pub if_range: Option<&'a str>,
}

/// Corps d'une réponse acceptée, lu morceau par morceau.
#[derive(Debug)]
pub struct Body {
    response: Response,
    read: Duration,
}

impl Body {
    /// Morceau suivant ; `None` à la fin du corps. Une coupure avant la fin annoncée par
    /// le serveur est une erreur réseau. Le délai de lecture court à partir de cet appel :
    /// le temps passé entre deux appels (attente de débit) ne compte pas.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, NetError> {
        tokio::time::timeout(self.read, self.response.chunk())
            .await
            .map_err(|_| NetError::ReadTimeout(self.read))?
            .map_err(NetError::from_reqwest)
    }
}

fn retry_after_of(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| headers::retry_after(v, SystemTime::now()))
}

fn status_error(response: &Response) -> NetError {
    NetError::Status {
        status: response.status().as_u16(),
        retry_after: retry_after_of(response),
    }
}

/// Assemble le résultat : `latest` est la dernière réponse lue, `earlier` celle du `HEAD`
/// quand un `GET` a suivi.
fn conclude(
    asked: &Url,
    final_url: Url,
    earlier: Option<&Observed>,
    latest: &Observed,
    total_bytes: Option<u64>,
    accepts_ranges: bool,
) -> Probe {
    let pick = |f: fn(&Observed) -> &Option<String>| {
        f(latest)
            .clone()
            .or_else(|| earlier.and_then(|e| f(e).clone()))
    };
    let raw_name = pick(|o| &o.disposition_name).or_else(|| filename::name_from_url(&final_url));
    Probe {
        downgraded: asked.scheme() == "https" && final_url.scheme() == "http",
        total_bytes,
        accepts_ranges,
        etag: pick(|o| &o.etag),
        last_modified: pick(|o| &o.last_modified),
        file_name: filename::sanitize(raw_name.as_deref().unwrap_or(""), NameRules::native()),
        final_url,
    }
}

/// Vérifie qu'une adresse est utilisable (HTTP ou HTTPS) avant de créer une tâche.
pub fn check_url(url: &str) -> Result<(), NetError> {
    parse_url(url).map(|_| ())
}

fn parse_url(url: &str) -> Result<Url, NetError> {
    let url = Url::parse(url).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(NetError::UnsupportedScheme(other.to_owned())),
    }
}

/// En-têtes d'une tâche, au plus (`task.add`, SPEC §6.5).
pub const MAX_TASK_HEADERS: usize = 32;

/// En-têtes que le moteur fixe lui-même (plages, validateurs, compression : SPEC §7.1 et
/// §7.4), ou propres à une connexion : une tâche ne les impose pas.
const RESERVED_HEADERS: [&str; 17] = [
    "accept-encoding",
    "connection",
    "content-length",
    "expect",
    "host",
    "if-match",
    "if-modified-since",
    "if-none-match",
    "if-range",
    "if-unmodified-since",
    "keep-alive",
    "proxy-connection",
    "range",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// En-têtes secrets (SPEC §5.1) : rangés dans le coffre du système, jamais en base.
/// reqwest les retire d'une redirection vers un autre hôte.
pub fn is_secret_header(name: &str) -> bool {
    ["cookie", "authorization", "proxy-authorization"]
        .iter()
        .any(|s| name.eq_ignore_ascii_case(s))
}

/// Vérifie les en-têtes d'une tâche : noms et valeurs valides, sans doublon (casse
/// ignorée), ni en-tête réservé au moteur, valeurs d'au plus
/// [`crate::secrets::MAX_SECRET`] octets. Le message ne cite jamais une valeur (SPEC §8.2).
pub fn check_task_headers(list: &[(String, String)]) -> Result<(), NetError> {
    if list.len() > MAX_TASK_HEADERS {
        return Err(NetError::BadHeader(format!(
            "{} en-têtes, {MAX_TASK_HEADERS} au plus",
            list.len()
        )));
    }
    let mut seen = std::collections::HashSet::new();
    for (name, value) in list {
        let lower = name.to_ascii_lowercase();
        if RESERVED_HEADERS.contains(&lower.as_str()) {
            return Err(NetError::BadHeader(format!(
                "{name} est fixé par le moteur"
            )));
        }
        if !seen.insert(lower) {
            return Err(NetError::BadHeader(format!("{name} en double")));
        }
        if value.len() > crate::secrets::MAX_SECRET {
            return Err(NetError::BadHeader(format!("valeur de {name} trop longue")));
        }
    }
    header_map(list).map(|_| ())
}

/// En-têtes de la tâche ; le message d'erreur ne cite que le nom, jamais la valeur.
fn header_map(list: &[(String, String)]) -> Result<HeaderMap, NetError> {
    let mut map = HeaderMap::with_capacity(list.len());
    for (name, value) in list {
        let key = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| NetError::BadHeader(format!("nom {name:?} invalide")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| NetError::BadHeader(format!("valeur de {name} invalide")))?;
        map.insert(key, value);
    }
    Ok(map)
}

#[cfg(test)]
mod tests;
