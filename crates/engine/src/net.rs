//! Clients HTTP(S) et FTP, TLS, proxies. Voir SPEC §2.1 et §7.1.
//!
//! Fait : préparation d'une tâche HTTP(S) ([`Client::probe`], SPEC §7.1), noms de fichier
//! ([`filename`]), classement des erreurs selon la SPEC §6.8. TLS par `rustls`, avec les
//! certificats racines du système (`rustls-platform-verifier`).
//!
//! À faire : requêtes par plage des segments (vérification du `206`), `If-Range`, FTP,
//! proxies (F-07), cookies et authentification (F-06), HTTP/3 (F-05).

pub mod filename;
pub mod headers;

use std::time::{Duration, SystemTime};

use reqwest::header::{
    ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_ENCODING, CONTENT_LENGTH,
    CONTENT_RANGE, ETAG, HeaderMap, HeaderName, HeaderValue, LAST_MODIFIED, RANGE, RETRY_AFTER,
};
use reqwest::{Method, Response, StatusCode, Url, redirect};

use filename::NameRules;

/// `User-Agent` par défaut ; un en-tête de la tâche le remplace (F-06).
pub const USER_AGENT: &str = concat!("AuroDownloader/", env!("CARGO_PKG_VERSION"));

/// Délai maximal d'établissement d'une connexion (TCP et TLS).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Délai maximal sans recevoir d'octet sur une connexion établie.
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

    #[error("certificat refusé : {0}")]
    InvalidCertificate(#[source] reqwest::Error),

    #[error("serveur injoignable : {0}")]
    Unreachable(#[source] reqwest::Error),
}

impl NetError {
    fn from_reqwest(error: reqwest::Error) -> Self {
        let error = error.without_url();
        if error.is_redirect() {
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
            NetError::InvalidUrl(_) | NetError::UnsupportedScheme(_) | NetError::BadHeader(_) => {
                None
            }
            NetError::Status { .. }
            | NetError::Redirect(_)
            | NetError::RedirectNotFollowed { .. } => Some("HTTP_STATUS"),
            NetError::Timeout(_) => Some("NET_TIMEOUT"),
            NetError::InvalidCertificate(_) => Some("TLS_INVALID_CERT"),
            NetError::Unreachable(_) => Some("NET_UNREACHABLE"),
        }
    }

    /// Le moteur réessaie-t-il seul (colonne « Réessai » de la SPEC §6.8) ?
    pub fn retryable(&self) -> bool {
        match self {
            NetError::Status { status, .. } => {
                matches!(status, 408 | 429) || (500..600).contains(status)
            }
            NetError::Timeout(_) | NetError::Unreachable(_) => true,
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
    /// `ETag` utilisable avec `If-Range` pour la reprise (SPEC §7.4, étape 3).
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

/// Client HTTP(S) du moteur.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
}

impl Client {
    /// Client avec les délais par défaut.
    pub fn new() -> Result<Self, NetError> {
        Self::with_timeouts(CONNECT_TIMEOUT, READ_TIMEOUT)
    }

    /// Client avec des délais choisis (tests, réglages).
    pub fn with_timeouts(connect: Duration, read: Duration) -> Result<Self, NetError> {
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
            } else {
                attempt.follow()
            }
        });
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(connect)
            .read_timeout(read)
            .redirect(policy)
            .build()
            .map_err(NetError::from_reqwest)?;
        Ok(Client { http })
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
        let url = parse_url(url)?;
        let extra = header_map(task_headers)?;

        let head = self.send(Method::HEAD, &url, &extra, None).await?;
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
            .send(Method::GET, &url, &extra, Some("bytes=0-0"))
            .await?;
        let status = get.status();
        let get_url = get.url().clone();
        let g = Observed::from_headers(get.headers());
        let retry_after = get
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| headers::retry_after(v, SystemTime::now()));
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

    async fn send(
        &self,
        method: Method,
        url: &Url,
        extra: &HeaderMap,
        range: Option<&'static str>,
    ) -> Result<Response, NetError> {
        let mut request = self
            .http
            .request(method, url.clone())
            .headers(extra.clone());
        // Une compression ferait dériver les décalages d'octets (SPEC §7.1, étape 4).
        request = request.header(ACCEPT_ENCODING, "identity");
        if let Some(range) = range {
            request = request.header(RANGE, range);
        }
        let response = request.send().await.map_err(NetError::from_reqwest)?;
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

fn parse_url(url: &str) -> Result<Url, NetError> {
    let url = Url::parse(url).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(NetError::UnsupportedScheme(other.to_owned())),
    }
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
