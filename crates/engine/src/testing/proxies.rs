//! Proxys de test (F-07) : un proxy HTTP qui relaie les requêtes en forme absolue, et un
//! SOCKS5 (sans identifiant, ou avec). Chacun note ce qu'on lui a demandé.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::thread;

/// Une demande reçue par un proxy de test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// Proxy HTTP : la cible en forme absolue (`http://127.0.0.1:4000/f`) ; SOCKS5 :
    /// `ip:port` (adresse donnée par le client) ou `nom:port` (nom à résoudre).
    pub target: String,
    /// Proxy HTTP : `Proxy-Authorization` ; SOCKS5 : l'identifiant donné.
    pub credentials: Option<String>,
    /// SOCKS5 : le client a donné un nom (résolution par le proxy).
    pub by_name: bool,
}

/// Un proxy de test qui tourne tant qu'on le garde.
pub struct TestProxy {
    pub addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl TestProxy {
    /// Les demandes reçues, dans l'ordre.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// `http://127.0.0.1:port`, `socks5://…`.
    pub fn url(&self, scheme: &str) -> String {
        format!("{scheme}://{}", self.addr)
    }
}

fn relay(a: TcpStream, b: TcpStream) {
    let (mut a2, mut b2) = (a.try_clone().unwrap(), b.try_clone().unwrap());
    let (mut a, mut b) = (a, b);
    let up = thread::spawn(move || {
        let _ = std::io::copy(&mut a, &mut b2);
        let _ = b2.shutdown(Shutdown::Write);
    });
    let _ = std::io::copy(&mut b, &mut a2);
    let _ = a2.shutdown(Shutdown::Write);
    let _ = up.join();
}

fn listen(handle: impl Fn(TcpStream, &Mutex<Vec<Seen>>) + Send + Sync + 'static) -> TestProxy {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let handle = Arc::new(handle);
    let shared = Arc::clone(&seen);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (handle, seen) = (Arc::clone(&handle), Arc::clone(&shared));
            thread::spawn(move || handle(stream, &seen));
        }
    });
    TestProxy { addr, seen }
}

/// Proxy HTTP : `GET http://hôte:port/chemin` relayé à l'hôte, en forme d'origine, une
/// requête par connexion (`Connection: close`).
pub fn http_proxy() -> TestProxy {
    listen(|client, seen| {
        let mut reader = BufReader::new(client.try_clone().unwrap());
        let mut first = String::new();
        if reader.read_line(&mut first).is_err() {
            return;
        }
        let mut headers = Vec::new();
        let mut credentials = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("proxy-authorization:") {
                let _ = value;
                credentials = Some(line["proxy-authorization:".len()..].trim().to_owned());
                continue;
            }
            if lower.starts_with("proxy-connection:") || lower.starts_with("connection:") {
                continue;
            }
            headers.push(line);
        }
        let mut parts = first.split_whitespace();
        let (method, target) = (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
        );
        seen.lock().unwrap().push(Seen {
            target: target.to_owned(),
            credentials,
            by_name: false,
        });
        let Some(rest) = target.strip_prefix("http://") else {
            return;
        };
        let (authority, path) = rest
            .split_once('/')
            .map_or((rest, "/".to_owned()), |(a, p)| (a, format!("/{p}")));
        let Ok(mut server) = TcpStream::connect(authority) else {
            return;
        };
        let mut head = format!("{method} {path} HTTP/1.1\r\n");
        for h in headers {
            head.push_str(&h);
        }
        head.push_str("Connection: close\r\n\r\n");
        if server.write_all(head.as_bytes()).is_err() {
            return;
        }
        // Ce que le client aurait déjà envoyé au-delà de l'en-tête (corps), puis le relais.
        let buffered = reader.buffer().to_vec();
        if !buffered.is_empty() {
            let _ = server.write_all(&buffered);
        }
        relay(client, server);
    })
}

/// Proxy SOCKS5 ; `login` : l'identifiant et le mot de passe exigés.
pub fn socks5_proxy(login: Option<(&'static str, &'static str)>) -> TestProxy {
    listen(move |mut client, seen| {
        let mut greeting = [0u8; 2];
        if client.read_exact(&mut greeting).is_err() || greeting[0] != 5 {
            return;
        }
        let mut methods = vec![0u8; greeting[1] as usize];
        let _ = client.read_exact(&mut methods);
        let mut credentials = None;
        match login {
            None => {
                let _ = client.write_all(&[5, 0]);
            }
            Some((user, pass)) => {
                if !methods.contains(&2) {
                    let _ = client.write_all(&[5, 0xff]);
                    return;
                }
                let _ = client.write_all(&[5, 2]);
                let mut version = [0u8; 2];
                let _ = client.read_exact(&mut version);
                let mut name = vec![0u8; version[1] as usize];
                let _ = client.read_exact(&mut name);
                let mut len = [0u8; 1];
                let _ = client.read_exact(&mut len);
                let mut word = vec![0u8; len[0] as usize];
                let _ = client.read_exact(&mut word);
                let ok = name == user.as_bytes() && word == pass.as_bytes();
                let _ = client.write_all(&[1, if ok { 0 } else { 1 }]);
                if !ok {
                    return;
                }
                credentials = Some(user.to_owned());
            }
        }
        let mut request = [0u8; 4];
        if client.read_exact(&mut request).is_err() || request[1] != 1 {
            return;
        }
        let (host, by_name) = match request[3] {
            1 => {
                let mut ip = [0u8; 4];
                let _ = client.read_exact(&mut ip);
                (std::net::Ipv4Addr::from(ip).to_string(), false)
            }
            4 => {
                let mut ip = [0u8; 16];
                let _ = client.read_exact(&mut ip);
                (format!("[{}]", std::net::Ipv6Addr::from(ip)), false)
            }
            3 => {
                let mut len = [0u8; 1];
                let _ = client.read_exact(&mut len);
                let mut name = vec![0u8; len[0] as usize];
                let _ = client.read_exact(&mut name);
                (String::from_utf8_lossy(&name).into_owned(), true)
            }
            _ => return,
        };
        let mut port = [0u8; 2];
        let _ = client.read_exact(&mut port);
        let target = format!("{host}:{}", u16::from_be_bytes(port));
        seen.lock().unwrap().push(Seen {
            target: target.clone(),
            credentials,
            by_name,
        });
        let server = target
            .to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.find_map(|a| TcpStream::connect(a).ok()));
        let Some(server) = server else {
            let _ = client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]);
            return;
        };
        let _ = client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        relay(client, server);
    })
}
