//! Outils de test : serveur HTTP local qui simule les cas de la SPEC §9.1.
//!
//! Écrit sur des sockets bruts pour maîtriser chaque octet des réponses : statuts,
//! en-têtes, plages, coupures en plein corps, débit bridé. Le contenu des fichiers est
//! fabriqué à la volée ([`content_range`]) : un fichier de 1 Gio ne passe pas en mémoire.
//!
//! Compilé pour les tests du moteur et, avec la fonctionnalité `testing`, pour ceux des
//! autres paquets (banc de reprise de la CLI). Ne fait pas partie de l'API du moteur.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// Requête reçue par le serveur de test.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Corps d'une réponse.
#[derive(Debug, Clone)]
pub enum Body {
    Bytes(Vec<u8>),
    /// `len` octets de [`content_range`] à partir de `first`.
    Generated {
        first: u64,
        len: u64,
    },
}

impl Body {
    fn len(&self) -> u64 {
        match self {
            Body::Bytes(b) => b.len() as u64,
            Body::Generated { len, .. } => *len,
        }
    }
}

/// Réponse du serveur de test : en-tête brut, corps, et coupure éventuelle.
#[derive(Debug, Clone)]
pub struct Reply {
    pub head: Vec<u8>,
    pub body: Body,
    /// Connexion fermée après ce nombre d'octets du corps.
    pub cut_after: Option<u64>,
}

/// Débit bridé : `chunk` octets du corps, puis une pause de `delay`.
#[derive(Debug, Clone, Copy)]
pub struct Pace {
    pub chunk: usize,
    pub delay: Duration,
}

pub struct Server {
    pub base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    /// Connexions servies en même temps : actuelles, et maximum atteint.
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl Server {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    /// Connexions servies en ce moment.
    pub fn active_connections(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// Nombre maximal de connexions servies en même temps.
    pub fn peak_connections(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

fn read_request(stream: &mut impl Read) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut words = line.split_whitespace();
    let (method, path) = (words.next()?.to_owned(), words.next()?.to_owned());
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (n, v) = line.split_once(':')?;
        headers.push((n.trim().to_owned(), v.trim().to_owned()));
    }
    Some(Request {
        method,
        path,
        headers,
    })
}

/// Envoie la réponse ; s'arrête sans bruit si le client a fermé la connexion.
fn write_reply(stream: &mut impl Write, reply: &Reply, pace: Option<Pace>) -> std::io::Result<()> {
    stream.write_all(&reply.head)?;
    let len = reply.body.len().min(reply.cut_after.unwrap_or(u64::MAX));
    let chunk = pace.map_or(64 * 1024, |p| p.chunk.max(1)) as u64;
    let mut sent = 0;
    while sent < len {
        let n = chunk.min(len - sent);
        match &reply.body {
            Body::Bytes(b) => stream.write_all(&b[sent as usize..(sent + n) as usize])?,
            Body::Generated { first, .. } => {
                stream.write_all(&content_range(first + sent, n as usize))?;
            }
        }
        sent += n;
        if let Some(p) = pace {
            thread::sleep(p.delay);
        }
    }
    stream.flush()
}

/// Autorité de certification et certificat de serveur créés pour les tests HTTPS.
pub struct TestTls {
    /// Certificat de l'autorité, en PEM : la racine à ajouter côté client.
    pub ca_pem: String,
    config: Arc<rustls::ServerConfig>,
}

impl TestTls {
    /// Autorité neuve, et certificat signé par elle pour `127.0.0.1` et `localhost`.
    pub fn new() -> Self {
        use rcgen::{
            BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose,
        };
        let ca_key = KeyPair::generate().unwrap();
        let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        ca.distinguished_name
            .push(DnType::CommonName, "AuroDownloader, autorité de test");
        let ca_pem = ca.self_signed(&ca_key).unwrap().pem();
        let issuer = Issuer::new(ca, ca_key);

        let key = KeyPair::generate().unwrap();
        let names = vec!["127.0.0.1".to_owned(), "localhost".to_owned()];
        let cert = CertificateParams::new(names)
            .unwrap()
            .signed_by(&key, &issuer)
            .unwrap();
        let private = rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], private)
        .unwrap();
        TestTls {
            ca_pem,
            config: Arc::new(config),
        }
    }
}

impl Default for TestTls {
    fn default() -> Self {
        Self::new()
    }
}

/// Une connexion : une requête, une réponse, en clair ou chiffrée.
fn answer(
    mut stream: impl Read + Write,
    handler: &(dyn Fn(&Request) -> Reply + Send + Sync),
    log: &Mutex<Vec<Request>>,
    (active, peak): (&AtomicUsize, &AtomicUsize),
    pace: Option<Pace>,
) {
    let Some(request) = read_request(&mut stream) else {
        return;
    };
    log.lock().unwrap().push(request.clone());
    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
    peak.fetch_max(now, Ordering::SeqCst);
    let _ = write_reply(&mut stream, &handler(&request), pace);
    active.fetch_sub(1, Ordering::SeqCst);
}

/// Serveur qui répond à chaque connexion, dans son propre fil, par `handler(requête)`,
/// puis ferme.
pub fn serve(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Server {
    serve_paced(handler, None)
}

/// Comme [`serve`], avec un débit bridé par connexion.
pub fn serve_paced(
    handler: impl Fn(&Request) -> Reply + Send + Sync + 'static,
    pace: Option<Pace>,
) -> Server {
    serve_with(handler, pace, None)
}

/// Serveur en clair, ou en HTTPS avec le certificat de `tls`.
fn serve_with(
    handler: impl Fn(&Request) -> Reply + Send + Sync + 'static,
    pace: Option<Pace>,
    tls: Option<&TestTls>,
) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let scheme = if tls.is_some() { "https" } else { "http" };
    let base = format!("{scheme}://{}", listener.local_addr().unwrap());
    let tls = tls.map(|t| Arc::clone(&t.config));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    let (active, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (active_count, peak_count) = (Arc::clone(&active), Arc::clone(&peak));
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let (log, handler) = (Arc::clone(&log), Arc::clone(&handler));
            let (active, peak) = (Arc::clone(&active_count), Arc::clone(&peak_count));
            let tls = tls.clone();
            thread::spawn(move || {
                let counters = (&*active, &*peak);
                match tls {
                    None => answer(stream, &*handler, &log, counters, pace),
                    Some(config) => {
                        let Ok(conn) = rustls::ServerConnection::new(config) else {
                            return;
                        };
                        let stream = rustls::StreamOwned::new(conn, stream);
                        answer(stream, &*handler, &log, counters, pace);
                    }
                }
            });
        }
    });
    Server {
        base,
        requests,
        active,
        peak,
    }
}

fn head(status: &str, headers: &[(&str, &str)]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    for (n, v) in headers {
        out.push_str(&format!("{n}: {v}\r\n"));
    }
    out.push_str("\r\n");
    out.into_bytes()
}

/// Réponse brute ; `Connection: close` est ajouté.
pub fn reply(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
    Reply {
        head: head(status, headers),
        body: Body::Bytes(body.to_vec()),
        cut_after: None,
    }
}

/// Octet de rang `i` des fichiers de test.
pub fn byte_at(i: u64) -> u8 {
    (i.wrapping_mul(31) ^ (i >> 9)) as u8
}

/// Période du contenu : l'octet de rang `i` ne dépend que de ses 17 bits de poids faible
/// (`i * 31` modulo 256 et les bits 9 à 16 de `i`).
const PERIOD: usize = 1 << 17;

fn period_table() -> &'static [u8] {
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    TABLE.get_or_init(|| (0..PERIOD as u64).map(byte_at).collect())
}

/// `len` octets des fichiers de test à partir de `first`, recopiés d'une période
/// précalculée : le serveur de test tient plus de 1 Gbit/s.
pub fn content_range(first: u64, len: usize) -> Vec<u8> {
    let table = period_table();
    let mut out = Vec::with_capacity(len);
    let mut pos = (first % PERIOD as u64) as usize;
    while out.len() < len {
        let n = (PERIOD - pos).min(len - out.len());
        out.extend_from_slice(&table[pos..pos + n]);
        pos = 0;
    }
    out
}

/// Fichier de test complet de `len` octets.
pub fn content(len: usize) -> Vec<u8> {
    content_range(0, len)
}

/// Comportement d'un serveur de fichier, modifiable pendant le test.
pub struct FileSite {
    /// Taille du fichier ; son contenu est [`content`].
    pub len: u64,
    /// `None` : aucun `ETag` envoyé.
    pub etag: Option<String>,
    pub ranges: bool,
    /// Taille annoncée dans `Content-Length`.
    pub send_length: bool,
    /// Les `cut_responses` prochaines réponses à une plage s'arrêtent après `cut_after`
    /// octets du corps, connexion fermée.
    pub cut_responses: usize,
    pub cut_after: u64,
    /// Les `busy_responses` prochaines requêtes par plage reçoivent un 503 (serveur qui
    /// résiste, SPEC §7.3).
    pub busy_responses: usize,
}

impl FileSite {
    pub fn new(len: u64) -> Self {
        FileSite {
            len,
            etag: Some("\"v1\"".into()),
            ranges: true,
            send_length: true,
            cut_responses: 0,
            cut_after: 0,
            busy_responses: 0,
        }
    }
}

/// Serveur d'un fichier qui respecte `Range` et `If-Range` (RFC 9110), selon `site`.
pub fn serve_file(site: Arc<Mutex<FileSite>>) -> Server {
    serve_file_paced(site, None)
}

/// Comme [`serve_file`], avec un débit bridé par connexion.
pub fn serve_file_paced(site: Arc<Mutex<FileSite>>, pace: Option<Pace>) -> Server {
    serve_with(file_handler(site), pace, None)
}

/// Comme [`serve_file_paced`], en HTTPS avec le certificat de `tls`.
pub fn serve_file_tls(site: Arc<Mutex<FileSite>>, pace: Option<Pace>, tls: &TestTls) -> Server {
    serve_with(file_handler(site), pace, Some(tls))
}

fn file_handler(site: Arc<Mutex<FileSite>>) -> impl Fn(&Request) -> Reply + Send + Sync + 'static {
    let cuts_done = AtomicUsize::new(0);
    let busy_done = AtomicUsize::new(0);
    move |r: &Request| {
        let site = site.lock().unwrap();
        if r.method == "GET"
            && r.header("range").is_some()
            && busy_done.load(Ordering::SeqCst) < site.busy_responses
        {
            busy_done.fetch_add(1, Ordering::SeqCst);
            return reply("503 Service Unavailable", &[("Content-Length", "0")], b"");
        }
        let total = site.len;
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(etag) = &site.etag {
            headers.push(("ETag".into(), etag.clone()));
        }
        if site.ranges {
            headers.push(("Accept-Ranges".into(), "bytes".into()));
        }
        let validators_match = match r.header("if-range") {
            None => true,
            Some(v) => site.etag.as_deref() == Some(v),
        };
        let range = r
            .header("range")
            .and_then(|v| v.strip_prefix("bytes="))
            .and_then(|v| v.split_once('-'))
            .and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?)))
            .filter(|_| site.ranges && validators_match);

        let (status, body) = match range {
            Some((first, _)) if first >= total => {
                headers.push(("Content-Range".into(), format!("bytes */{total}")));
                ("416 Range Not Satisfiable", Body::Bytes(Vec::new()))
            }
            Some((first, last)) => {
                let last = last.min(total - 1);
                headers.push((
                    "Content-Range".into(),
                    format!("bytes {first}-{last}/{total}"),
                ));
                let len = last - first + 1;
                ("206 Partial Content", Body::Generated { first, len })
            }
            None => (
                "200 OK",
                Body::Generated {
                    first: 0,
                    len: total,
                },
            ),
        };
        if site.send_length || range.is_some() {
            headers.push(("Content-Length".into(), body.len().to_string()));
        }
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let body = if r.method == "HEAD" {
            Body::Bytes(Vec::new())
        } else {
            body
        };
        let cut = range.is_some()
            && r.method == "GET"
            && cuts_done.load(Ordering::SeqCst) < site.cut_responses;
        if cut {
            cuts_done.fetch_add(1, Ordering::SeqCst);
        }
        Reply {
            head: head(status, &header_refs),
            body,
            cut_after: cut.then_some(site.cut_after),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_content_matches_the_formula() {
        for first in [0, 1, 131_071, 131_072, 1 << 30, (1 << 40) + 12_345] {
            let expected: Vec<u8> = (first..first + 300_000).map(byte_at).collect();
            assert_eq!(
                content_range(first, 300_000),
                expected,
                "à partir de {first}"
            );
        }
        assert!(content_range(7, 0).is_empty());
    }
}
