//! Outils de test : serveur HTTP local qui simule les cas de la SPEC §9.1.
//!
//! Écrit sur des sockets bruts pour maîtriser chaque octet des réponses : statuts,
//! en-têtes, plages, coupures en plein corps, débit bridé. Le contenu des fichiers est
//! fabriqué à la volée ([`content_range`]) : un fichier de 1 Gio ne passe pas en mémoire.
//!
//! Compilé pour les tests du moteur et, avec la fonctionnalité `testing`, pour ceux des
//! autres paquets (banc de reprise de la CLI). Ne fait pas partie de l'API du moteur.

pub mod plugin_files;
pub mod proxies;
pub mod swarm;
pub mod tools;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// Arrête le moteur d'un test et attend qu'il ait fermé sa base et ses fichiers. À appeler
/// avant la fin d'un test qui a démarré un moteur dans un dossier temporaire : sous
/// Windows, un fichier encore ouvert ne s'efface pas, et le dossier resterait dans
/// `%TEMP%`.
pub async fn stop(engine: &crate::scheduler::Engine) {
    engine.shutdown().await;
    engine.stopped().await;
}

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
    /// Nombre maximal de connexions servies en même temps.
    peak: Arc<AtomicUsize>,
}

impl Server {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
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

thread_local! {
    /// Connexions servies en même temps, celle de ce fil comprise, à l'arrivée de sa
    /// requête (chaque connexion a son fil).
    static SERVING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
    SERVING.with(|s| s.set(now));
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

/// Empreinte SHA-256 (hexadécimal) des `len` premiers octets du contenu servi, calculée
/// par morceaux : un fichier de banc peut être gros.
pub fn content_sha256(len: u64) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut first = 0;
    while first < len {
        let n = (len - first).min(1 << 20);
        hasher.update(content_range(first, n as usize));
        first += n;
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
    /// Au-delà de ce nombre de connexions servies en même temps, une requête par plage
    /// reçoit un 503 : serveur qui plafonne les connexions par client (SPEC §7.3).
    pub max_concurrent: Option<usize>,
    /// Identification exigée de chaque requête (F-06) ; `None` : aucune.
    pub auth: Option<SiteAuth>,
    /// Fichiers voisins (F-11) servis à ces chemins (`/SHA256SUMS`…) ; tout autre chemin
    /// de fichier voisin reçoit un 404, le reste le fichier.
    pub neighbors: Vec<(String, Vec<u8>)>,
    /// Chemins des fichiers voisins demandés, dans l'ordre.
    pub neighbor_requests: Vec<String>,
    /// `If-Range` n'est reconnu que sous la forme d'une étiquette conforme, entre
    /// guillemets, même si l'`ETag` envoyée n'en a pas (`upload.wikimedia.org`).
    pub strict_if_range: bool,
    /// Décalage du contenu servi : le même fichier, mais d'autres octets (miroir qui sert
    /// un autre fichier de même taille, SPEC §7.7) ; 0 par défaut.
    pub shift: u64,
    /// Octets servis faux (morceaux faux, F-12) : chacun est inversé dans les
    /// `corrupt_responses` prochaines réponses `GET` qui en contiennent un.
    pub corrupt: Vec<u64>,
    pub corrupt_responses: usize,
    /// Plages servies aux requêtes `GET` (premier et dernier octet), dans l'ordre.
    pub served: Vec<(u64, u64)>,
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
            max_concurrent: None,
            auth: None,
            neighbors: Vec::new(),
            neighbor_requests: Vec::new(),
            strict_if_range: false,
            shift: 0,
            corrupt: Vec::new(),
            corrupt_responses: 0,
            served: Vec::new(),
        }
    }
}

/// Identification exigée par le serveur de test : Basic, ou Digest avec `qop=auth`.
#[derive(Debug, Clone)]
pub struct SiteAuth {
    pub username: String,
    pub password: String,
    /// `None` : Basic ; sinon l'algorithme Digest (`MD5`, `SHA-256`…).
    pub digest: Option<&'static str>,
    pub nonce: String,
}

impl SiteAuth {
    pub fn basic(username: &str, password: &str) -> Self {
        SiteAuth {
            username: username.into(),
            password: password.into(),
            digest: None,
            nonce: String::new(),
        }
    }

    pub fn digest(username: &str, password: &str, algorithm: &'static str) -> Self {
        SiteAuth {
            username: username.into(),
            password: password.into(),
            digest: Some(algorithm),
            nonce: "nonce-de-test".into(),
        }
    }

    fn challenge(&self) -> String {
        match self.digest {
            None => "Basic realm=\"essai\"".into(),
            Some(algorithm) => format!(
                "Digest realm=\"essai\", qop=\"auth\", nonce=\"{}\", algorithm={algorithm}",
                self.nonce
            ),
        }
    }

    fn accepts(&self, r: &Request) -> bool {
        let Some(value) = r.header("authorization") else {
            return false;
        };
        match self.digest {
            None => crate::net::auth::basic_value(&self.username, &self.password) == value,
            Some(_) => crate::net::auth::digest_valid(
                value,
                &r.method,
                &self.username,
                &self.password,
                &self.nonce,
            ),
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

/// Chemin d'un fichier voisin (empreintes, F-11).
fn is_neighbor(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    [
        ".sha256",
        ".sha1",
        ".md5",
        "/SHA256SUMS",
        "/SHA1SUMS",
        "/MD5SUMS",
    ]
    .iter()
    .any(|end| path.ends_with(end))
}

pub fn file_handler(
    site: Arc<Mutex<FileSite>>,
) -> impl Fn(&Request) -> Reply + Send + Sync + 'static {
    let cuts_done = AtomicUsize::new(0);
    let busy_done = AtomicUsize::new(0);
    let corrupt_done = AtomicUsize::new(0);
    move |r: &Request| {
        let mut site = site.lock().unwrap();
        if is_neighbor(&r.path) {
            site.neighbor_requests.push(r.path.clone());
            return match site.neighbors.iter().find(|(path, _)| *path == r.path) {
                Some((_, body)) => {
                    let len = body.len().to_string();
                    let body = if r.method == "HEAD" { &[][..] } else { body };
                    reply("200 OK", &[("Content-Length", len.as_str())], body)
                }
                None => reply("404 Not Found", &[("Content-Length", "0")], b""),
            };
        }
        if let Some(auth) = &site.auth
            && !auth.accepts(r)
        {
            let challenge = auth.challenge();
            return reply(
                "401 Unauthorized",
                &[
                    ("WWW-Authenticate", challenge.as_str()),
                    ("Content-Length", "0"),
                ],
                b"",
            );
        }
        if r.method == "GET"
            && r.header("range").is_some()
            && site
                .max_concurrent
                .is_some_and(|max| SERVING.with(std::cell::Cell::get) > max)
        {
            return reply("503 Service Unavailable", &[("Content-Length", "0")], b"");
        }
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
            Some(v) if site.strict_if_range => site.etag.as_deref().is_some_and(|e| {
                v.len() >= 2
                    && v.starts_with('"')
                    && v.ends_with('"')
                    && v.trim_matches('"') == e.trim_matches('"')
            }),
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
                (
                    "206 Partial Content",
                    Body::Generated {
                        first: first + site.shift,
                        len,
                    },
                )
            }
            None => (
                "200 OK",
                Body::Generated {
                    first: site.shift,
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
        let mut body = if r.method == "HEAD" {
            Body::Bytes(Vec::new())
        } else {
            body
        };
        // Plages servies, et octets faux (F-12).
        if let Body::Generated { first, len } = body
            && len > 0
        {
            let start = first - site.shift;
            let end = start + len - 1;
            site.served.push((start, end));
            let hits: Vec<u64> = site
                .corrupt
                .iter()
                .copied()
                .filter(|b| (start..=end).contains(b))
                .collect();
            if !hits.is_empty() && corrupt_done.load(Ordering::SeqCst) < site.corrupt_responses {
                corrupt_done.fetch_add(1, Ordering::SeqCst);
                let mut bytes = content_range(first, len as usize);
                for b in hits {
                    bytes[(b - start) as usize] ^= 0xFF;
                }
                body = Body::Bytes(bytes);
            }
        }
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

/// Compare `len` octets de `file` à partir de `offset` avec le contenu servi.
pub fn same_as_served(file: &std::path::Path, offset: u64, len: u64) -> Result<(), String> {
    use std::io::{Seek, SeekFrom};
    const BLOCK: u64 = 1 << 20;
    let mut f = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut buf = vec![0; BLOCK as usize];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(BLOCK) as usize;
        f.read_exact(&mut buf[..n])
            .map_err(|e| format!("lecture à {} : {e}", offset + done))?;
        if buf[..n] != content_range(offset + done, n)[..] {
            return Err(format!(
                "octets différents entre {} et {}",
                offset + done,
                offset + done + n as u64
            ));
        }
        done += n as u64;
    }
    Ok(())
}

/// Contrôle d'une base et de ses téléchargements après un arrêt brutal (SPEC §5.6), pour
/// les bancs de reprise. Tous les fichiers servis ont la taille `size` et le contenu
/// [`content`].
pub struct Checker {
    pub db: std::path::PathBuf,
    pub downloads: std::path::PathBuf,
    pub size: u64,
    /// Tâches terminées dont le fichier a été comparé puis supprimé.
    pub verified: std::collections::HashSet<String>,
    /// Constats d'un découpage qui n'est aucun découpage initial : redécoupé (F-02).
    pub split_layouts: usize,
    /// Arrêts entre le renommage du `.part` et l'enregistrement de la fin : la reprise
    /// termine la tâche (SPEC §7.4).
    pub renamed_unrecorded: usize,
    /// Arrêts brutaux pendant le calcul de l'empreinte (`verifying`, F-11) : la reprise
    /// le refait.
    pub checking: usize,
}

/// Découpages initiaux possibles (connexions imposées ou 4 au départ en automatique).
const INITIAL_WIDTHS: [u32; 4] = [1, 2, 4, 8];

impl Checker {
    /// Fin d'un banc : chaque fichier du dossier appartient à une tâche de la base (son nom
    /// ou son `.part`) ; aucun `.part` orphelin d'un nom réservé avant un arrêt (SPEC §5.6,
    /// règle 7).
    pub fn assert_no_strays(&self) {
        let conn = crate::store::open(&self.db).unwrap();
        let mut stmt = conn
            .prepare("SELECT file_name FROM tasks WHERE file_name IS NOT NULL")
            .unwrap();
        let known: std::collections::HashSet<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .flat_map(|n| {
                let n = n.unwrap();
                [format!("{n}.part"), n]
            })
            .collect();
        let mut strays: Vec<String> = std::fs::read_dir(&self.downloads)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !known.contains(n))
            .collect();
        strays.sort();
        assert!(strays.is_empty(), "fichiers sans tâche : {strays:?}");
    }

    pub fn new(db: std::path::PathBuf, downloads: std::path::PathBuf, size: u64) -> Self {
        Checker {
            db,
            downloads,
            size,
            verified: Default::default(),
            split_layouts: 0,
            renamed_unrecorded: 0,
            checking: 0,
        }
    }

    /// Règles de la SPEC §5.6 : les octets déclarés écrits sont dans le `.part`, à leur
    /// place (règle 1) ; les segments couvrent le fichier (règle 2) ; `done_bytes` est leur
    /// somme (règle 3) ; une tâche terminée a son historique et plus de segments
    /// (règle 4). Chaque fichier terminé est comparé octet par octet, puis supprimé.
    /// `explain` complète le message d'un état inattendu (journal du programme tué).
    pub fn check(&mut self, explain: &dyn Fn() -> String) {
        use crate::io::part_path;
        use crate::segmenter::{DEFAULT_MIN_SEGMENT_SIZE, initial_split};
        use crate::store::{self, segments, tasks};

        let conn = store::open(&self.db).unwrap();
        let broken = segments::find_coverage_errors(&conn).unwrap();
        assert!(broken.is_empty(), "règle 2 : {broken:?}");

        for task in tasks::list_tasks(&conn).unwrap() {
            let segs = segments::load_segments(&conn, &task.id).unwrap();
            match task.state {
                tasks::TaskState::Completed => {
                    assert!(segs.is_empty(), "règle 4 : segments restants");
                    let history: i64 = conn
                        .query_row(
                            "SELECT COUNT(*) FROM history WHERE task_id = ?1",
                            [&task.id],
                            |r| r.get(0),
                        )
                        .unwrap();
                    assert_eq!(history, 1, "règle 4 : historique");
                    if self.verified.contains(&task.id) {
                        continue;
                    }
                    let name = task.file_name.as_deref().expect("nom du fichier");
                    let path = self.downloads.join(name);
                    let len = std::fs::metadata(&path)
                        .unwrap_or_else(|e| panic!("fichier terminé absent : {e}"))
                        .len();
                    assert_eq!(len, self.size, "taille du fichier final");
                    assert_eq!(task.done_bytes, self.size);
                    if let Err(e) = same_as_served(&path, 0, len) {
                        panic!("fichier final corrompu : {e}");
                    }
                    assert!(!part_path(&path).exists(), ".part restant");
                    std::fs::remove_file(&path).unwrap();
                    self.verified.insert(task.id.clone());
                }
                // `verifying` : arrêt brutal pendant le calcul de l'empreinte ; le `.part`
                // est complet (ou déjà renommé), et se contrôle comme celui d'une tâche en
                // cours.
                tasks::TaskState::Queued
                | tasks::TaskState::Active
                | tasks::TaskState::Verifying => {
                    if task.state == tasks::TaskState::Verifying {
                        let all_done = segs
                            .iter()
                            .all(|s| s.state == crate::segmenter::SegmentState::Done);
                        assert!(
                            all_done && !segs.is_empty(),
                            "verifying : segments inachevés"
                        );
                        self.checking += 1;
                    }
                    let sum: u64 = segs.iter().map(|s| s.written).sum();
                    let ranges: Vec<(u64, u64)> = segs.iter().map(|s| (s.start, s.end)).collect();
                    let initial = INITIAL_WIDTHS.iter().any(|&n| {
                        initial_split(self.size, n, DEFAULT_MIN_SEGMENT_SIZE)
                            .iter()
                            .map(|s| (s.start, s.end))
                            .eq(ranges.iter().copied())
                    });
                    if !segs.is_empty() && !initial {
                        self.split_layouts += 1;
                    }
                    assert_eq!(task.done_bytes, sum, "règle 3");
                    let Some(name) = task.file_name.as_deref() else {
                        assert!(segs.is_empty(), "segments sans préparation");
                        continue;
                    };
                    let dest = self.downloads.join(name);
                    let part = part_path(&dest);
                    // Arrêt entre le renommage et l'enregistrement de la fin : tous les
                    // segments sont finis et le fichier final est complet, exact.
                    let all_done = segs
                        .iter()
                        .all(|s| s.state == crate::segmenter::SegmentState::Done);
                    if all_done && !segs.is_empty() && !part.exists() {
                        let len = std::fs::metadata(&dest)
                            .unwrap_or_else(|e| panic!("ni .part ni fichier final : {e}"))
                            .len();
                        assert_eq!(len, self.size, "taille du fichier renommé");
                        if let Err(e) = same_as_served(&dest, 0, len) {
                            panic!("fichier renommé corrompu : {e}");
                        }
                        self.renamed_unrecorded += 1;
                        continue;
                    }
                    for s in segs.iter().filter(|s| s.written > 0) {
                        if let Err(e) = same_as_served(&part, s.start, s.written) {
                            panic!(
                                "règle 1 : segment {} ({}-{}, {} octets annoncés) : {e}",
                                s.idx, s.start, s.end, s.written
                            );
                        }
                    }
                }
                other => panic!(
                    "état inattendu {other:?} : {:?} {:?}\n{}",
                    task.error_code,
                    task.error_detail,
                    explain()
                ),
            }
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
