//! Outils de test : serveur HTTP local qui simule les cas de la SPEC §9.1.
//!
//! Écrit sur des sockets bruts pour maîtriser chaque octet des réponses : statuts,
//! en-têtes, plages, coupures en plein corps.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

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

pub struct Server {
    pub base: String,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

fn read_request(stream: &TcpStream) -> Option<Request> {
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

/// Serveur qui répond à chaque connexion, dans son propre fil, par `handler(requête)`,
/// puis ferme.
pub fn serve(handler: impl Fn(&Request) -> Vec<u8> + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let (log, handler) = (Arc::clone(&log), Arc::clone(&handler));
            thread::spawn(move || {
                let Some(request) = read_request(&stream) else {
                    return;
                };
                log.lock().unwrap().push(request.clone());
                let _ = stream.write_all(&handler(&request));
            });
        }
    });
    Server { base, requests }
}

/// Réponse brute ; `Connection: close` est ajouté.
pub fn reply(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    for (n, v) in headers {
        out.push_str(&format!("{n}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

/// Contenu de fichier de test : un octet qui dépend de sa position.
pub fn content(len: usize) -> Vec<u8> {
    (0..len as u64)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 9)) as u8)
        .collect()
}

/// Comportement d'un serveur de fichier, modifiable pendant le test.
pub struct FileSite {
    pub body: Vec<u8>,
    /// `None` : aucun `ETag` envoyé.
    pub etag: Option<String>,
    pub ranges: bool,
    /// Taille annoncée dans `Content-Length`.
    pub send_length: bool,
    /// Les `cut_responses` prochaines réponses à une plage s'arrêtent après `cut_after`
    /// octets du corps, connexion fermée.
    pub cut_responses: usize,
    pub cut_after: usize,
}

impl FileSite {
    pub fn new(body: Vec<u8>) -> Self {
        FileSite {
            body,
            etag: Some("\"v1\"".into()),
            ranges: true,
            send_length: true,
            cut_responses: 0,
            cut_after: 0,
        }
    }
}

/// Serveur d'un fichier qui respecte `Range` et `If-Range` (RFC 9110), selon `site`.
pub fn serve_file(site: Arc<Mutex<FileSite>>) -> Server {
    let cuts_done = AtomicUsize::new(0);
    serve(move |r| {
        let site = site.lock().unwrap();
        let total = site.body.len();
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
            .and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)))
            .filter(|_| site.ranges && validators_match);

        let (status, body): (&str, &[u8]) = match range {
            Some((first, _)) if first >= total => {
                headers.push(("Content-Range".into(), format!("bytes */{total}")));
                ("416 Range Not Satisfiable", &[])
            }
            Some((first, last)) => {
                let last = last.min(total - 1);
                headers.push((
                    "Content-Range".into(),
                    format!("bytes {first}-{last}/{total}"),
                ));
                ("206 Partial Content", &site.body[first..=last])
            }
            None => ("200 OK", &site.body[..]),
        };
        if site.send_length || range.is_some() {
            headers.push(("Content-Length".into(), body.len().to_string()));
        }
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let body = if r.method == "HEAD" { &[][..] } else { body };
        let mut out = reply(status, &header_refs, body);
        if range.is_some()
            && r.method == "GET"
            && cuts_done.load(Ordering::SeqCst) < site.cut_responses
        {
            cuts_done.fetch_add(1, Ordering::SeqCst);
            let keep = out.len() - body.len() + site.cut_after.min(body.len());
            out.truncate(keep);
        }
        out
    })
}
