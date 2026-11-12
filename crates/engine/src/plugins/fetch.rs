//! Requêtes d'un plugin (`auro.fetch`, SPEC §7.20) : faites par le moteur, vers les
//! domaines déclarés seulement (redirections comprises), réseau public, `GET` ou `POST`
//! d'un petit formulaire, en-têtes d'une liste fermée ; cookies du site tenus ici, jamais
//! montrés au plugin. Les messages d'erreur ne citent ni l'adresse ni un en-tête
//! (SPEC §8.2).

use std::time::Instant;

use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode, Url};
use serde_json::{Value, json};

use super::manifest::domain_covers;
use crate::net::NetError;

/// Requêtes par résolution, au plus.
pub const MAX_REQUESTS: u32 = 32;
/// Réponse lue, au plus.
pub const MAX_BODY: usize = 8 << 20;
/// Formulaire envoyé (`POST`), au plus.
pub const MAX_FORM: usize = 64 << 10;
/// Redirections suivies par requête, au plus.
const MAX_REDIRECTS: usize = 10;
/// Cookies gardés par résolution, au plus.
const MAX_COOKIES: usize = 64;
/// En-têtes qu'un plugin peut choisir.
pub const ALLOWED_HEADERS: &[&str] = &[
    "referer",
    "accept",
    "accept-language",
    "content-type",
    "x-requested-with",
];

/// Cookies posés par le site pendant une résolution.
#[derive(Debug, Clone, Default)]
pub struct Jar {
    cookies: Vec<Cookie>,
}

#[derive(Debug, Clone)]
struct Cookie {
    /// Hôte (cookie sans `Domain`) ou domaine.
    domain: String,
    host_only: bool,
    name: String,
    value: String,
}

impl Jar {
    /// Retient un `Set-Cookie` reçu de `host`.
    pub fn store(&mut self, host: &str, set_cookie: &str) {
        let mut parts = set_cookie.split(';');
        let Some((name, value)) = parts.next().and_then(|p| p.split_once('=')) else {
            return;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || name.len() > 256 || value.len() > 4096 {
            return;
        }
        let host = host.to_ascii_lowercase();
        let mut domain = None;
        let mut removed = false;
        for attribute in parts {
            let (key, val) = attribute.split_once('=').unwrap_or((attribute, ""));
            let key = key.trim().to_ascii_lowercase();
            let val = val.trim();
            if key == "domain" {
                domain = Some(val.trim_start_matches('.').to_ascii_lowercase());
            } else if key == "max-age" && val.parse::<i64>().is_ok_and(|n| n <= 0) {
                removed = true;
            }
        }
        let (domain, host_only) = match domain {
            // Un domaine qui ne couvre pas l'hôte qui le pose : refusé (comme un navigateur).
            Some(d) if !d.is_empty() => {
                if !domain_covers(&d, &host) {
                    return;
                }
                (d, false)
            }
            _ => (host, true),
        };
        self.cookies
            .retain(|c| !(c.name == name && c.domain == domain && c.host_only == host_only));
        if !removed && self.cookies.len() < MAX_COOKIES {
            self.cookies.push(Cookie {
                domain,
                host_only,
                name: name.to_owned(),
                value: value.to_owned(),
            });
        }
    }

    /// L'en-tête `Cookie` à envoyer à `host`, s'il y a des cookies pour lui.
    pub fn header_for(&self, host: &str) -> Option<String> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let pairs: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| {
                if c.host_only {
                    c.domain == host
                } else {
                    domain_covers(&c.domain, &host)
                }
            })
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        (!pairs.is_empty()).then(|| pairs.join("; "))
    }
}

/// Les requêtes d'une résolution : client, domaines permis, cookies, compte, échéance.
pub struct Session {
    http: reqwest::Client,
    fetch: Vec<String>,
    pub jar: Jar,
    requests: u32,
    deadline: Instant,
}

/// Encodage `application/x-www-form-urlencoded`.
fn form_encode(fields: &serde_json::Map<String, Value>) -> Result<String, String> {
    fn push(out: &mut String, text: &str) {
        for b in text.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
    }
    let mut out = String::new();
    for (key, value) in fields {
        let value = value
            .as_str()
            .ok_or("form : des textes seulement (nom = valeur)")?;
        if !out.is_empty() {
            out.push('&');
        }
        push(&mut out, key);
        out.push('=');
        push(&mut out, value);
    }
    if out.len() > MAX_FORM {
        return Err(format!("formulaire de plus de {} Kio", MAX_FORM >> 10));
    }
    Ok(out)
}

impl Session {
    pub fn new(http: reqwest::Client, fetch: Vec<String>, deadline: Instant) -> Self {
        Session {
            http,
            fetch,
            jar: Jar::default(),
            requests: 0,
            deadline,
        }
    }

    fn allowed(&self, url: &Url) -> Result<(), String> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err("seulement http et https".into());
        }
        let host = url.host_str().unwrap_or_default();
        if !self.fetch.iter().any(|d| domain_covers(d, host)) {
            return Err("domaine non déclaré par le plugin".into());
        }
        Ok(())
    }

    /// Sert une requête du plugin ; rend la réponse JSON à lui écrire.
    pub async fn serve(&mut self, request: &[u8]) -> Vec<u8> {
        let answer = match self.request(request).await {
            Ok(answer) => answer,
            Err(error) => json!({ "error": error }),
        };
        serde_json::to_vec(&answer).unwrap_or_default()
    }

    async fn request(&mut self, raw: &[u8]) -> Result<Value, String> {
        if self.requests >= MAX_REQUESTS {
            return Err(format!("plus de {MAX_REQUESTS} requêtes"));
        }
        self.requests += 1;
        let request: Value =
            serde_json::from_slice(raw).map_err(|_| "requête illisible (JSON)".to_owned())?;
        let mut url = request["url"]
            .as_str()
            .and_then(|u| Url::parse(u).ok())
            .ok_or("url invalide")?;
        self.allowed(&url)?;
        let mut method = match request["method"].as_str().unwrap_or("GET") {
            "GET" => Method::GET,
            "POST" => Method::POST,
            _ => return Err("méthode : GET ou POST".into()),
        };
        let mut headers = HeaderMap::new();
        if let Some(asked) = request.get("headers") {
            let asked = asked.as_object().ok_or("headers : objet nom → valeur")?;
            for (name, value) in asked {
                let lower = name.to_ascii_lowercase();
                if !ALLOWED_HEADERS.contains(&lower.as_str()) {
                    return Err(format!(
                        "en-tête non permis (seulement {})",
                        ALLOWED_HEADERS.join(", ")
                    ));
                }
                let value = value
                    .as_str()
                    .filter(|v| v.len() <= 1024)
                    .and_then(|v| HeaderValue::from_str(v).ok())
                    .ok_or("valeur d'en-tête invalide")?;
                let name =
                    HeaderName::from_bytes(lower.as_bytes()).map_err(|_| "en-tête invalide")?;
                headers.insert(name, value);
            }
        }
        let mut body = match request.get("form") {
            None | Some(Value::Null) => None,
            Some(Value::Object(fields)) if method == Method::POST => Some(form_encode(fields)?),
            Some(Value::Object(_)) => return Err("form : seulement avec POST".into()),
            Some(_) => return Err("form : objet nom → valeur".into()),
        };
        if body.is_some() && !headers.contains_key(header::CONTENT_TYPE) {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            );
        }
        for _ in 0..=MAX_REDIRECTS {
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("délai de la résolution dépassé".into());
            }
            let host = url.host_str().unwrap_or_default().to_owned();
            let mut builder = self
                .http
                .request(method.clone(), url.clone())
                .headers(headers.clone())
                .timeout(left);
            if let Some(cookie) = self.jar.header_for(&host)
                && let Ok(value) = HeaderValue::from_str(&cookie)
            {
                builder = builder.header(header::COOKIE, value);
            }
            if let Some(body) = &body {
                builder = builder.body(body.clone());
            }
            let mut response = builder
                .send()
                .await
                .map_err(|e| NetError::from_reqwest(e).to_string())?;
            for set in response.headers().get_all(header::SET_COOKIE) {
                if let Ok(set) = set.to_str() {
                    self.jar.store(&host, set);
                }
            }
            let status = response.status();
            if status.is_redirection() && status != StatusCode::NOT_MODIFIED {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|l| url.join(l).ok())
                    .ok_or("redirection sans destination lisible")?;
                if self.allowed(&location).is_err() {
                    return Err("redirection hors des domaines déclarés par le plugin".into());
                }
                if status == StatusCode::SEE_OTHER
                    || (method == Method::POST
                        && matches!(status, StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND))
                {
                    method = Method::GET;
                    body = None;
                    headers.remove(header::CONTENT_TYPE);
                }
                url = location;
                continue;
            }
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| NetError::from_reqwest(e).to_string())?
            {
                if bytes.len() + chunk.len() > MAX_BODY {
                    return Err(format!("réponse de plus de {} Mio", MAX_BODY >> 20));
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(json!({
                "status": status.as_u16(),
                "url": url.as_str(),
                "headers": {"content-type": content_type},
                "body": String::from_utf8_lossy(&bytes),
            }));
        }
        Err(format!("plus de {MAX_REDIRECTS} redirections"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_go_back_to_the_hosts_they_cover() {
        let mut jar = Jar::default();
        jar.store("www.fichiers.exemple", "session=abc; Path=/; HttpOnly");
        jar.store("www.fichiers.exemple", "large=1; Domain=.fichiers.exemple");
        // Un domaine qui ne couvre pas l'hôte : refusé.
        jar.store("www.fichiers.exemple", "evil=1; Domain=autre.exemple");
        assert_eq!(
            jar.header_for("www.fichiers.exemple").as_deref(),
            Some("session=abc; large=1")
        );
        assert_eq!(
            jar.header_for("cdn.fichiers.exemple").as_deref(),
            Some("large=1")
        );
        assert_eq!(jar.header_for("autre.exemple"), None);
        // Remplacé, puis effacé.
        jar.store("www.fichiers.exemple", "session=def");
        assert_eq!(
            jar.header_for("www.fichiers.exemple").as_deref(),
            Some("large=1; session=def")
        );
        jar.store("www.fichiers.exemple", "session=; Max-Age=0");
        assert_eq!(
            jar.header_for("www.fichiers.exemple").as_deref(),
            Some("large=1")
        );
        jar.store("x.exemple", "pas de signe égal");
        assert_eq!(jar.header_for("x.exemple"), None);
    }

    #[test]
    fn forms_are_url_encoded() {
        let fields = json!({"op": "télécharger", "id": "a b&c"});
        let encoded = form_encode(fields.as_object().unwrap()).unwrap();
        // L'ordre des champs suit celui de la carte JSON.
        let mut parts: Vec<&str> = encoded.split('&').collect();
        parts.sort_unstable();
        assert_eq!(parts, ["id=a+b%26c", "op=t%C3%A9l%C3%A9charger"]);
        assert!(form_encode(json!({"n": 1}).as_object().unwrap()).is_err());
    }
}
