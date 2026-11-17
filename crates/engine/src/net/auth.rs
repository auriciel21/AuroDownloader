//! Authentification HTTP d'une tâche (F-06, SPEC §7.14) : Basic (RFC 7617) et Digest
//! (RFC 7616), avec les identifiants enregistrés pour un site.
//!
//! Le moteur n'envoie rien d'avance. Il attend que le serveur le demande (`401` avec
//! `WWW-Authenticate`), répond au défi, puis renvoie la même identification aux requêtes
//! suivantes de la tâche, toutes connexions confondues ([`Auth`]). Les identifiants ne
//! partent que vers une adresse de leur site ([`Site::matches`]). Basic ne part jamais sur
//! une connexion non chiffrée : le mot de passe y circulerait en clair.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, PoisonError};

use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, WWW_AUTHENTICATE};
use reqwest::{Method, Url};

/// Portée d'identifiants : un site, éventuellement restreint à un chemin.
///
/// `exemple.org` vaut pour `http`, `https`, `ftp` et `ftps` (F-08), pour `exemple.org` et ses sous-domaines
/// (`dl.exemple.org`), sur tous les ports. `https://exemple.org:8443/prive` ne vaut que pour
/// HTTPS, ce port et ce chemin (et ce qui est dessous).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    scheme: Option<String>,
    host: String,
    port: Option<u16>,
    /// Préfixe de chemin, sans `/` final ; vide : tout le site.
    path: String,
}

impl Site {
    /// Lit un site tel que l'utilisateur l'écrit : adresse complète ou nom d'hôte.
    pub fn parse(text: &str) -> Result<Site, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("site vide".into());
        }
        let explicit = text.contains("://");
        let url = Url::parse(&if explicit {
            text.to_owned()
        } else {
            format!("https://{text}")
        })
        .map_err(|e| format!("site invalide : {e}"))?;
        if !matches!(url.scheme(), "http" | "https" | "ftp" | "ftps") {
            return Err(format!("protocole non pris en charge : {}", url.scheme()));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("le site ne doit pas contenir d'identifiants".into());
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err("le site ne doit contenir ni paramètres (?) ni ancre (#)".into());
        }
        let host = url
            .host_str()
            .filter(|h| !h.is_empty())
            .ok_or("site sans nom d'hôte")?
            .to_ascii_lowercase();
        Ok(Site {
            scheme: explicit.then(|| url.scheme().to_owned()),
            host,
            port: url.port(),
            path: url.path().trim_end_matches('/').to_owned(),
        })
    }

    /// Forme normalisée, celle qui est enregistrée et affichée.
    pub fn text(&self) -> String {
        let mut text = String::new();
        if let Some(scheme) = &self.scheme {
            text.push_str(scheme);
            text.push_str("://");
        }
        text.push_str(&self.host);
        if let Some(port) = self.port {
            text.push_str(&format!(":{port}"));
        }
        text.push_str(&self.path);
        text
    }

    /// `url` est-elle de ce site ? `Some(précision)` si oui : la plus grande l'emporte
    /// quand plusieurs sites conviennent (chemin le plus long, puis hôte exact, puis
    /// protocole précisé).
    pub fn matches(&self, url: &Url) -> Option<usize> {
        if let Some(scheme) = &self.scheme
            && scheme != url.scheme()
        {
            return None;
        }
        let host = url.host_str()?.to_ascii_lowercase();
        let exact = host == self.host;
        if !exact && !host.ends_with(&format!(".{}", self.host)) {
            return None;
        }
        let port = super::ftp::default_port(url)?;
        if self.port.is_some_and(|p| p != port) {
            return None;
        }
        let path = url.path();
        let under = self.path.is_empty()
            || path == self.path
            || path
                .strip_prefix(&self.path)
                .is_some_and(|rest| rest.starts_with('/'));
        if !under {
            return None;
        }
        Some(self.path.len() * 8 + usize::from(exact) * 4 + usize::from(self.scheme.is_some()) * 2)
    }
}

/// Identifiants d'un site, mot de passe compris (lu dans le coffre du système).
#[derive(Clone, PartialEq, Eq)]
pub struct Login {
    pub site: Site,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Jamais le mot de passe dans un journal (SPEC §8.2).
        f.debug_struct("Login")
            .field("site", &self.site.text())
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// Un défi `WWW-Authenticate` : son schéma (en minuscules) et ses paramètres.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub scheme: String,
    pub params: HashMap<String, String>,
}

/// Lit les défis d'un ou plusieurs en-têtes `WWW-Authenticate` (RFC 9110 §11.6.1) :
/// `Digest realm="a", nonce="n", Basic realm="b"` en donne deux.
pub fn parse_challenges<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<Challenge> {
    let mut found = Vec::new();
    for value in values {
        let mut p = Parser::new(value);
        while let Some(scheme) = p.next_scheme() {
            let mut params = HashMap::new();
            while let Some((name, value)) = p.next_param() {
                params.insert(name.to_ascii_lowercase(), value);
            }
            found.push(Challenge {
                scheme: scheme.to_ascii_lowercase(),
                params,
            });
        }
    }
    found
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
}

fn is_tchar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Parser {
            text: text.as_bytes(),
            at: 0,
        }
    }

    fn skip(&mut self, set: &[u8]) {
        while self.at < self.text.len() && set.contains(&self.text[self.at]) {
            self.at += 1;
        }
    }

    fn token(&mut self) -> Option<String> {
        let start = self.at;
        while self.at < self.text.len() && is_tchar(self.text[self.at]) {
            self.at += 1;
        }
        (self.at > start).then(|| String::from_utf8_lossy(&self.text[start..self.at]).into_owned())
    }

    fn next_scheme(&mut self) -> Option<String> {
        self.skip(b" \t,");
        self.token()
    }

    /// Paramètre suivant du défi en cours ; `None` à la fin du défi (un jeton sans `=`
    /// commence le suivant).
    fn next_param(&mut self) -> Option<(String, String)> {
        let save = self.at;
        self.skip(b" \t,");
        let Some(name) = self.token() else {
            self.at = save;
            return None;
        };
        self.skip(b" \t");
        if self.text.get(self.at) != Some(&b'=') {
            // Nouveau défi : on revient avant son schéma.
            self.at = save;
            return None;
        }
        self.at += 1;
        self.skip(b" \t");
        let value = if self.text.get(self.at) == Some(&b'"') {
            self.at += 1;
            let mut value = Vec::new();
            while let Some(&c) = self.text.get(self.at) {
                self.at += 1;
                match c {
                    b'"' => break,
                    b'\\' => {
                        if let Some(&next) = self.text.get(self.at) {
                            value.push(next);
                            self.at += 1;
                        }
                    }
                    c => value.push(c),
                }
            }
            String::from_utf8_lossy(&value).into_owned()
        } else {
            // Jeton, ou fin d'un token68 (`=` de remplissage).
            let mut value = self.token().unwrap_or_default();
            while self.text.get(self.at) == Some(&b'=') {
                value.push('=');
                self.at += 1;
            }
            value
        };
        Some((name, value))
    }
}

/// Algorithme de hachage d'un défi Digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hash {
    Md5,
    Sha256,
}

impl Hash {
    fn hex(self, data: &str) -> String {
        use sha2::Digest as _;
        let bytes: Vec<u8> = match self {
            Hash::Md5 => md5::Md5::digest(data.as_bytes()).to_vec(),
            Hash::Sha256 => sha2::Sha256::digest(data.as_bytes()).to_vec(),
        };
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Réponse retenue à un défi, réutilisée pour les requêtes suivantes.
#[derive(Debug, Clone)]
enum Scheme {
    Basic,
    Digest {
        realm: String,
        nonce: String,
        opaque: Option<String>,
        hash: Hash,
        /// Variante `-sess` : l'empreinte des identifiants dépend aussi du nonce.
        session: bool,
        /// `qop=auth` proposé ; sinon, l'ancienne forme (RFC 2069).
        qop: bool,
        algorithm: String,
    },
}

/// Ce que le moteur fait d'une réponse `401`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Recommencer la requête : l'identification est prête.
    Retry,
    /// Le serveur ne demande rien que le moteur sache fournir pour cette adresse.
    Unanswered,
    /// Basic sur une connexion non chiffrée : refusé.
    Insecure,
}

/// Identification d'une tâche : ses identifiants et le dernier défi reçu, partagés par
/// toutes ses connexions.
#[derive(Debug)]
pub struct Auth {
    login: Login,
    scheme: Mutex<Option<Scheme>>,
    /// Compteur `nc` de Digest, pour le nonce en cours.
    count: AtomicU32,
    /// FTP sans TLS permis pour ce site (« autoriser sans chiffrement », F-08, SPEC §7.22).
    allow_plain: bool,
}

impl Auth {
    pub fn new(login: Login) -> Self {
        Auth {
            login,
            scheme: Mutex::new(None),
            count: AtomicU32::new(0),
            allow_plain: false,
        }
    }

    /// Les mêmes identifiants, avec l'autorisation de FTP sans chiffrement du site.
    pub fn allowing_plain(mut self, allow: bool) -> Self {
        self.allow_plain = allow;
        self
    }

    /// Nom et mot de passe à donner à un serveur FTP (F-08), si `url` est du site.
    pub fn credentials_for(&self, url: &Url) -> Option<(String, String)> {
        self.login.site.matches(url)?;
        Some((self.login.username.clone(), self.login.password.clone()))
    }

    /// FTP en clair permis vers `url` (site autorisé sans chiffrement) ?
    pub fn allows_plain_for(&self, url: &Url) -> bool {
        self.allow_plain && self.login.site.matches(url).is_some()
    }

    /// En-tête `Authorization` à joindre à une requête vers `url`, si un défi a déjà été
    /// reçu et que `url` est du site. `None` avant le premier défi.
    pub fn authorization(&self, method: &Method, url: &Url) -> Option<HeaderValue> {
        self.login.site.matches(url)?;
        let scheme = self
            .scheme
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()?;
        let value = match scheme {
            Scheme::Basic => {
                if url.scheme() != "https" {
                    return None;
                }
                basic_value(&self.login.username, &self.login.password)
            }
            Scheme::Digest {
                realm,
                nonce,
                opaque,
                hash,
                session,
                qop,
                algorithm,
            } => {
                let count = self.count.fetch_add(1, Ordering::SeqCst) + 1;
                let cnonce = uuid::Uuid::new_v4().simple().to_string();
                let uri = request_target(url);
                let digest = DigestInput {
                    username: &self.login.username,
                    password: &self.login.password,
                    realm: &realm,
                    nonce: &nonce,
                    method: method.as_str(),
                    uri: &uri,
                    hash,
                    session,
                    qop,
                    count,
                    cnonce: &cnonce,
                };
                let mut header = format!(
                    "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", \
                     algorithm={algorithm}, response=\"{}\"",
                    quote(&self.login.username),
                    quote(&realm),
                    quote(&nonce),
                    quote(&uri),
                    digest.response()
                );
                if qop {
                    header.push_str(&format!(", qop=auth, nc={count:08x}, cnonce=\"{cnonce}\""));
                }
                if let Some(opaque) = opaque {
                    header.push_str(&format!(", opaque=\"{}\"", quote(&opaque)));
                }
                header
            }
        };
        HeaderValue::from_str(&value).ok().map(|mut v| {
            v.set_sensitive(true);
            v
        })
    }

    /// Réponse `401` de `url` : retient le défi auquel répondre. `sent` : la requête
    /// refusée portait déjà une identification ; un nouveau refus n'est alors repris que
    /// si le serveur dit son nonce périmé (`stale=true`).
    pub fn on_challenge(&self, url: &Url, headers: &HeaderMap, sent: bool) -> Answer {
        if self.login.site.matches(url).is_none() {
            return Answer::Unanswered;
        }
        let challenges = parse_challenges(
            headers
                .get_all(WWW_AUTHENTICATE)
                .iter()
                .filter_map(|v| v.to_str().ok()),
        );
        // Digest d'abord, SHA-256 avant MD5 ; Basic en dernier.
        let digest = challenges
            .iter()
            .filter(|c| c.scheme == "digest")
            .filter_map(digest_scheme)
            .max_by_key(|(rank, _)| *rank)
            .map(|(_, s)| s);
        let chosen = match digest {
            Some(scheme) => scheme,
            None if challenges.iter().any(|c| c.scheme == "basic") => {
                if url.scheme() != "https" {
                    return Answer::Insecure;
                }
                Scheme::Basic
            }
            None => return Answer::Unanswered,
        };
        let stale = challenges.iter().any(|c| {
            c.scheme == "digest"
                && c.params
                    .get("stale")
                    .is_some_and(|v| v.eq_ignore_ascii_case("true"))
        });
        if sent && !stale {
            return Answer::Unanswered;
        }
        *self.scheme.lock().unwrap_or_else(PoisonError::into_inner) = Some(chosen);
        self.count.store(0, Ordering::SeqCst);
        Answer::Retry
    }
}

/// Défi Digest utilisable, avec son rang (SHA-256 avant MD5).
fn digest_scheme(c: &Challenge) -> Option<(u8, Scheme)> {
    let algorithm = c
        .params
        .get("algorithm")
        .cloned()
        .unwrap_or_else(|| "MD5".into());
    let (hash, session, rank) = match algorithm.to_ascii_uppercase().as_str() {
        "MD5" => (Hash::Md5, false, 1),
        "MD5-SESS" => (Hash::Md5, true, 1),
        "SHA-256" => (Hash::Sha256, false, 2),
        "SHA-256-SESS" => (Hash::Sha256, true, 2),
        // SHA-512-256 et les inconnus : non pris en charge.
        _ => return None,
    };
    let qop = match c.params.get("qop") {
        None => false,
        Some(list)
            if list
                .split(',')
                .any(|q| q.trim().eq_ignore_ascii_case("auth")) =>
        {
            true
        }
        // `auth-int` seul : non pris en charge.
        Some(_) => return None,
    };
    Some((
        rank,
        Scheme::Digest {
            realm: c.params.get("realm").cloned().unwrap_or_default(),
            nonce: c.params.get("nonce")?.clone(),
            opaque: c.params.get("opaque").cloned(),
            hash,
            session,
            qop,
            algorithm,
        },
    ))
}

/// Valeur `Authorization` de Basic (RFC 7617 : `utilisateur:mot de passe` en UTF-8, base 64).
pub(crate) fn basic_value(username: &str, password: &str) -> String {
    let raw = format!("{username}:{password}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    )
}

/// Côté serveur, pour le serveur de test : la réponse Digest `value` d'une requête
/// `method` est-elle juste pour ces identifiants et ce nonce ?
#[cfg(any(test, feature = "testing"))]
pub(crate) fn digest_valid(
    value: &str,
    method: &str,
    username: &str,
    password: &str,
    nonce: &str,
) -> bool {
    let Some(c) = parse_challenges([value])
        .into_iter()
        .find(|c| c.scheme == "digest")
    else {
        return false;
    };
    let get = |k: &str| c.params.get(k).map(String::as_str).unwrap_or_default();
    if get("username") != username || get("nonce") != nonce {
        return false;
    }
    let (hash, session) = match get("algorithm").to_ascii_uppercase().as_str() {
        "" | "MD5" => (Hash::Md5, false),
        "MD5-SESS" => (Hash::Md5, true),
        "SHA-256" => (Hash::Sha256, false),
        "SHA-256-SESS" => (Hash::Sha256, true),
        _ => return false,
    };
    let qop = get("qop") == "auth";
    let Ok(count) = u32::from_str_radix(get("nc"), 16) else {
        return !qop
            && expected(hash, session, false, 0, "", &c, method, username, password)
                == get("response");
    };
    expected(
        hash,
        session,
        qop,
        count,
        get("cnonce"),
        &c,
        method,
        username,
        password,
    ) == get("response")
}

#[cfg(any(test, feature = "testing"))]
#[allow(clippy::too_many_arguments)]
fn expected(
    hash: Hash,
    session: bool,
    qop: bool,
    count: u32,
    cnonce: &str,
    c: &Challenge,
    method: &str,
    username: &str,
    password: &str,
) -> String {
    let get = |k: &str| c.params.get(k).map(String::as_str).unwrap_or_default();
    DigestInput {
        username,
        password,
        realm: get("realm"),
        nonce: get("nonce"),
        method,
        uri: get("uri"),
        hash,
        session,
        qop,
        count,
        cnonce,
    }
    .response()
}

/// Chemin et paramètres d'une adresse, tels que la requête les porte (`uri` de Digest).
fn request_target(url: &Url) -> String {
    match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    }
}

/// Échappe `"` et `\` dans une chaîne entre guillemets.
fn quote(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Ce qu'il faut pour calculer la réponse Digest (RFC 7616 §3.4.1).
struct DigestInput<'a> {
    username: &'a str,
    password: &'a str,
    realm: &'a str,
    nonce: &'a str,
    method: &'a str,
    uri: &'a str,
    hash: Hash,
    session: bool,
    qop: bool,
    count: u32,
    cnonce: &'a str,
}

impl DigestInput<'_> {
    fn response(&self) -> String {
        let h = |s: &str| self.hash.hex(s);
        let mut ha1 = h(&format!(
            "{}:{}:{}",
            self.username, self.realm, self.password
        ));
        if self.session {
            ha1 = h(&format!("{ha1}:{}:{}", self.nonce, self.cnonce));
        }
        let ha2 = h(&format!("{}:{}", self.method, self.uri));
        if self.qop {
            h(&format!(
                "{ha1}:{}:{:08x}:{}:auth:{ha2}",
                self.nonce, self.count, self.cnonce
            ))
        } else {
            h(&format!("{ha1}:{}:{ha2}", self.nonce))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    /// Exemple de la RFC 7616 §3.9.1 : Mufasa, « Circle of Life ».
    fn rfc_input(hash: Hash) -> DigestInput<'static> {
        DigestInput {
            username: "Mufasa",
            password: "Circle of Life",
            realm: "http-auth@example.org",
            nonce: "7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v",
            method: "GET",
            uri: "/dir/index.html",
            hash,
            session: false,
            qop: true,
            count: 1,
            cnonce: "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ",
        }
    }

    #[test]
    fn digest_matches_the_rfc_7616_examples() {
        assert_eq!(
            rfc_input(Hash::Md5).response(),
            "8ca523f5e9506fed4657c9700eebdbec"
        );
        assert_eq!(
            rfc_input(Hash::Sha256).response(),
            "753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1"
        );
    }

    /// Exemple de la RFC 2617 §3.5 (MD5), toujours celui des navigateurs.
    #[test]
    fn digest_matches_the_rfc_2617_example() {
        let input = DigestInput {
            username: "Mufasa",
            password: "Circle Of Life",
            realm: "testrealm@host.com",
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093",
            method: "GET",
            uri: "/dir/index.html",
            hash: Hash::Md5,
            session: false,
            qop: true,
            count: 1,
            cnonce: "0a4f113b",
        };
        assert_eq!(input.response(), "6629fae49393a05397450978507c4ef1");
    }

    #[test]
    fn several_challenges_are_read_from_one_header() {
        let found = parse_challenges([
            r#"Digest realm="a, b", qop="auth,auth-int", nonce="n\"1", Basic realm="x""#,
        ]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].scheme, "digest");
        assert_eq!(found[0].params["realm"], "a, b");
        assert_eq!(found[0].params["qop"], "auth,auth-int");
        assert_eq!(found[0].params["nonce"], "n\"1");
        assert_eq!(found[1].scheme, "basic");
        assert_eq!(found[1].params["realm"], "x");
        let two = parse_challenges(["Basic realm=x", "Bearer"]);
        assert_eq!(
            two.iter().map(|c| c.scheme.as_str()).collect::<Vec<_>>(),
            ["basic", "bearer"]
        );
    }

    #[test]
    fn a_site_covers_its_subdomains_and_paths() {
        let site = Site::parse("Exemple.org").unwrap();
        assert_eq!(site.text(), "exemple.org");
        assert!(site.matches(&url("https://exemple.org/f.iso")).is_some());
        assert!(site.matches(&url("http://dl.exemple.org/f.iso")).is_some());
        assert!(site.matches(&url("https://autreexemple.org/f")).is_none());
        assert!(
            site.matches(&url("https://exemple.org.evil.net/f"))
                .is_none()
        );
        assert!(site.matches(&url("https://exemple.org:8443/f")).is_some());

        let narrow = Site::parse("https://exemple.org:8443/prive/").unwrap();
        assert_eq!(narrow.text(), "https://exemple.org:8443/prive");
        assert!(
            narrow
                .matches(&url("https://exemple.org:8443/prive/a"))
                .is_some()
        );
        assert!(
            narrow
                .matches(&url("https://exemple.org:8443/privee"))
                .is_none()
        );
        assert!(
            narrow
                .matches(&url("http://exemple.org:8443/prive/a"))
                .is_none()
        );
        // Le plus précis l'emporte.
        let target = url("https://exemple.org:8443/prive/a");
        let broad = Site::parse("https://exemple.org:8443").unwrap();
        assert!(narrow.matches(&target) > broad.matches(&target));

        for bad in ["", "gopher://h", "https://u:p@h", "h/?a=1", "https://"] {
            assert!(Site::parse(bad).is_err(), "{bad:?}");
        }
    }

    fn login() -> Login {
        Login {
            site: Site::parse("exemple.org").unwrap(),
            username: "moi".into(),
            password: "secret".into(),
        }
    }

    fn challenge(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(WWW_AUTHENTICATE, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn basic_is_answered_over_https_only() {
        let auth = Auth::new(login());
        let page = url("https://exemple.org/f");
        assert!(
            auth.authorization(&Method::GET, &page).is_none(),
            "rien d'avance"
        );
        assert_eq!(
            auth.on_challenge(&page, &challenge("Basic realm=\"r\""), false),
            Answer::Retry
        );
        let value = auth.authorization(&Method::GET, &page).unwrap();
        assert_eq!(value.to_str().unwrap(), "Basic bW9pOnNlY3JldA==");
        assert!(value.is_sensitive());
        // Refus avec identification : les identifiants sont faux.
        assert_eq!(
            auth.on_challenge(&page, &challenge("Basic realm=\"r\""), true),
            Answer::Unanswered
        );
        let plain = url("http://exemple.org/f");
        assert_eq!(
            Auth::new(login()).on_challenge(&plain, &challenge("Basic realm=\"r\""), false),
            Answer::Insecure
        );
        assert!(auth.authorization(&Method::GET, &plain).is_none());
        // Un autre site ne reçoit rien.
        let other = url("https://ailleurs.net/f");
        assert_eq!(
            auth.on_challenge(&other, &challenge("Basic realm=\"r\""), false),
            Answer::Unanswered
        );
        assert!(auth.authorization(&Method::GET, &other).is_none());
    }

    #[test]
    fn digest_is_preferred_counts_its_requests_and_follows_stale_nonces() {
        let auth = Auth::new(login());
        let page = url("http://exemple.org/d/f.iso?x=1");
        let offer = challenge(
            "Basic realm=\"r\", Digest realm=\"r\", qop=\"auth\", nonce=\"n1\", \
             algorithm=MD5, Digest realm=\"r\", qop=\"auth\", nonce=\"n2\", algorithm=SHA-256, \
             opaque=\"o\"",
        );
        assert_eq!(auth.on_challenge(&page, &offer, false), Answer::Retry);
        let first = auth.authorization(&Method::GET, &page).unwrap();
        let first = first.to_str().unwrap();
        assert!(first.starts_with("Digest username=\"moi\""), "{first}");
        assert!(first.contains("nonce=\"n2\"") && first.contains("algorithm=SHA-256"));
        assert!(first.contains("uri=\"/d/f.iso?x=1\"") && first.contains("opaque=\"o\""));
        assert!(first.contains("nc=00000001"));
        let second = auth.authorization(&Method::GET, &page).unwrap();
        assert!(second.to_str().unwrap().contains("nc=00000002"));
        assert!(!first.contains("secret"));
        // Nonce périmé : repris même avec une identification envoyée.
        let stale = challenge("Digest realm=\"r\", qop=\"auth\", nonce=\"n3\", stale=true");
        assert_eq!(auth.on_challenge(&page, &stale, true), Answer::Retry);
        let third = auth.authorization(&Method::GET, &page).unwrap();
        assert!(third.to_str().unwrap().contains("nonce=\"n3\""));
        assert!(third.to_str().unwrap().contains("nc=00000001"));
        // Seulement auth-int, ou un algorithme inconnu : rien à répondre.
        let fresh = Auth::new(login());
        let odd = challenge("Digest realm=\"r\", qop=\"auth-int\", nonce=\"n\"");
        assert_eq!(fresh.on_challenge(&page, &odd, false), Answer::Unanswered);
        let odd = challenge("Digest realm=\"r\", nonce=\"n\", algorithm=SHA-512-256");
        assert_eq!(fresh.on_challenge(&page, &odd, false), Answer::Unanswered);
    }

    #[test]
    fn the_password_never_shows_in_debug_output() {
        assert!(!format!("{:?}", login()).contains("secret"));
        assert!(!format!("{:?}", Auth::new(login())).contains("secret"));
    }
}
