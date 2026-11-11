//! Interface distante (SPEC §7.19) face à de vraies connexions TCP : requêtes HTTP écrites
//! à la main (en-têtes falsifiés compris) et client WebSocket.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use super::server::{self, COOKIE, MAX_FAILURES};
use super::{RemoteState, run};
use crate::net::Client;
use crate::scheduler::{Config, DISK_RECHECK, Engine, Planning};
use crate::store::settings::Flag;
use crate::transfer::Options;

const WAIT: Duration = Duration::from_secs(20);

fn engine(dir: &Path) -> Engine {
    Engine::start(Config {
        db_path: dir.join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options::default(),
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap()
}

/// Un serveur de l'interface distante sur un port libre de la boucle locale.
async fn listening(engine: &Engine) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let names = super::tls::known_names(&[]);
    let plain = server::Listening { tls: None, names };
    tokio::spawn(server::serve(engine.clone(), listener, plain));
    addr
}

struct Answer {
    status: u16,
    head: String,
    body: String,
}

impl Answer {
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|line| {
            let (n, v) = line.split_once(':')?;
            n.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
    }
}

/// Une requête HTTP/1.1 écrite telle quelle ; `Connection: close` ajouté.
async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    let stream = TcpStream::connect(addr).await.unwrap();
    exchange(stream, method, path, headers, body).await
}

/// Une requête écrite telle quelle sur une connexion déjà ouverte (TLS ou non).
async fn exchange<S>(
    mut stream: S,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut request = format!("{method} {path} HTTP/1.1\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut raw = Vec::new();
    // Une fin sans `close_notify` (ou une connexion coupée) garde ce qui a été lu.
    let _ = timeout(WAIT, stream.read_to_end(&mut raw))
        .await
        .expect("pas de réponse");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Answer {
        status,
        head: head.to_owned(),
        body: body.to_owned(),
    }
}

fn hello() -> String {
    json!({"jsonrpc": "2.0", "id": 1, "method": "engine.hello",
           "params": {"client": {"name": "test", "version": "0"},
                      "protocol": {"min": "1.0", "max": "1.2"}}})
    .to_string()
}

/// `Host`, `Origin`, authentification et en-têtes de `POST /rpc` (SPEC §7.19).
#[tokio::test]
async fn host_origin_and_token_are_checked_before_anything() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let (_, token) = engine.create_token("script", "control").await.unwrap();
    let addr = listening(&engine).await;
    let host = format!("127.0.0.1:{}", addr.port());
    let bearer = format!("Bearer {token}");
    let json = ("Content-Type", "application/json");
    let ok = [
        ("Host", host.as_str()),
        ("Authorization", bearer.as_str()),
        json,
    ];

    let answer = http(addr, "POST", "/rpc", &ok, &hello()).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let reply: Value = serde_json::from_str(&answer.body).unwrap();
    assert_eq!(reply["result"]["scope"], "control", "{reply}");
    for (name, value) in [
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        ("Referrer-Policy", "no-referrer"),
        ("Cache-Control", "no-store"),
    ] {
        assert_eq!(answer.header(name).as_deref(), Some(value), "{name}");
    }
    assert!(answer.header("Access-Control-Allow-Origin").is_none());
    // `localhost` désigne aussi ce serveur.
    let local = format!("localhost:{}", addr.port());
    let by_name = [
        ("Host", local.as_str()),
        ("Authorization", bearer.as_str()),
        json,
    ];
    assert_eq!(
        http(addr, "POST", "/rpc", &by_name, &hello()).await.status,
        200
    );

    // Hôte inconnu (DNS rebinding), origine étrangère ou `null` : refusés.
    let rebound = [
        ("Host", "attaquant.example"),
        ("Authorization", bearer.as_str()),
        json,
    ];
    assert_eq!(
        http(addr, "POST", "/rpc", &rebound, &hello()).await.status,
        421
    );
    let other_port = format!("127.0.0.1:{}", addr.port() + 1);
    let wrong_port = [
        ("Host", other_port.as_str()),
        ("Authorization", bearer.as_str()),
        json,
    ];
    assert_eq!(
        http(addr, "POST", "/rpc", &wrong_port, &hello())
            .await
            .status,
        421
    );
    for origin in ["http://attaquant.example", "null", "https://127.0.0.1"] {
        let mut headers = ok.to_vec();
        headers.push(("Origin", origin));
        assert_eq!(
            http(addr, "POST", "/rpc", &headers, &hello()).await.status,
            403,
            "{origin}"
        );
    }

    // Sans jeton, jeton faux, cookie seul (jamais pour `/rpc`) : 401.
    let no_token = [("Host", host.as_str()), json];
    let answer = http(addr, "POST", "/rpc", &no_token, &hello()).await;
    assert_eq!(answer.status, 401);
    assert_eq!(answer.header("WWW-Authenticate").as_deref(), Some("Bearer"));
    let fake = format!("Bearer auro_{}", "A".repeat(43));
    let faked = [
        ("Host", host.as_str()),
        ("Authorization", fake.as_str()),
        json,
    ];
    assert_eq!(
        http(addr, "POST", "/rpc", &faked, &hello()).await.status,
        401
    );
    let cookie = format!("{COOKIE}={token}");
    let by_cookie = [("Host", host.as_str()), ("Cookie", cookie.as_str()), json];
    assert_eq!(
        http(addr, "POST", "/rpc", &by_cookie, &hello())
            .await
            .status,
        401
    );

    // Type de contenu, méthode, chemin.
    let as_form = [
        ("Host", host.as_str()),
        ("Authorization", bearer.as_str()),
        ("Content-Type", "application/x-www-form-urlencoded"),
    ];
    assert_eq!(
        http(addr, "POST", "/rpc", &as_form, &hello()).await.status,
        415
    );
    assert_eq!(http(addr, "GET", "/rpc", &ok, "").await.status, 405);
    assert_eq!(http(addr, "GET", "/ailleurs", &ok, "").await.status, 404);

    // Portée `control` : rien d'`admin`, même par `POST /rpc`.
    let shutdown = json!({"jsonrpc": "2.0", "id": 2, "method": "engine.shutdown"}).to_string();
    let answer = http(addr, "POST", "/rpc", &ok, &shutdown).await;
    let reply: Value = serde_json::from_str(&answer.body).unwrap();
    assert_eq!(reply["error"]["data"]["code"], "UNAUTHORIZED", "{reply}");
    // Une notification du client n'a pas de réponse.
    let quiet = json!({"jsonrpc": "2.0", "method": "engine.hello"}).to_string();
    assert_eq!(http(addr, "POST", "/rpc", &ok, &quiet).await.status, 204);
    crate::testing::stop(&engine).await;
}

/// Au-delà de [`MAX_FAILURES`] jetons refusés en une minute, l'adresse est refusée, même
/// avec un bon jeton.
#[tokio::test]
async fn repeated_failures_lock_the_address_out() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let (_, token) = engine.create_token("script", "read").await.unwrap();
    let addr = listening(&engine).await;
    let host = format!("127.0.0.1:{}", addr.port());
    let json = ("Content-Type", "application/json");
    let fake = format!("Bearer auro_{}", "B".repeat(43));
    for _ in 0..=MAX_FAILURES {
        let headers = [
            ("Host", host.as_str()),
            ("Authorization", fake.as_str()),
            json,
        ];
        assert_eq!(
            http(addr, "POST", "/rpc", &headers, &hello()).await.status,
            401
        );
    }
    let bearer = format!("Bearer {token}");
    let headers = [
        ("Host", host.as_str()),
        ("Authorization", bearer.as_str()),
        json,
    ];
    assert_eq!(
        http(addr, "POST", "/rpc", &headers, &hello()).await.status,
        429
    );
    crate::testing::stop(&engine).await;
}

async fn websocket(
    addr: SocketAddr,
    headers: &[(&str, &str)],
) -> Result<tokio_tungstenite::WebSocketStream<TcpStream>, tokio_tungstenite::tungstenite::Error> {
    let mut request = format!("ws://127.0.0.1:{}/ws", addr.port())
        .into_client_request()
        .unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    let stream = TcpStream::connect(addr).await.unwrap();
    tokio_tungstenite::client_async(request, stream)
        .await
        .map(|(socket, _)| socket)
}

fn refused_with(result: Result<impl Sized, tokio_tungstenite::tungstenite::Error>) -> u16 {
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        Err(e) => panic!("refus HTTP attendu : {e}"),
        Ok(_) => panic!("refus attendu"),
    }
}

async fn next_text<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Option<Value>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match timeout(WAIT, socket.next()).await.expect("rien reçu") {
            Some(Ok(Message::Text(text))) => return Some(serde_json::from_str(&text).unwrap()),
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
            Some(Ok(_)) => {}
        }
    }
}

/// Le navigateur : jeton échangé contre un cookie `HttpOnly` et `SameSite=Strict`, qui
/// n'ouvre le WebSocket que depuis la même origine ; fermé par `/logout` ; le jeton
/// révoqué ferme le WebSocket.
#[tokio::test]
async fn a_browser_session_opens_the_websocket_from_its_own_origin_only() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let (row, token) = engine.create_token("téléphone", "control").await.unwrap();
    let addr = listening(&engine).await;
    let host = format!("127.0.0.1:{}", addr.port());
    let origin = format!("http://{host}");
    let json = ("Content-Type", "application/json");
    let body = json!({"token": token}).to_string();

    // `/login` : seulement depuis la page (même origine), avec un vrai jeton.
    let no_origin = [("Host", host.as_str()), json];
    assert_eq!(
        http(addr, "POST", "/login", &no_origin, &body).await.status,
        403
    );
    let page = [("Host", host.as_str()), ("Origin", origin.as_str()), json];
    let wrong = json!({"token": format!("auro_{}", "C".repeat(43))}).to_string();
    assert_eq!(
        http(addr, "POST", "/login", &page, &wrong).await.status,
        401
    );
    // Un corps démesuré est refusé avant d'être lu.
    let huge = format!("{{\"token\": \"{}\"}}", "x".repeat(8192));
    assert_eq!(http(addr, "POST", "/login", &page, &huge).await.status, 413);
    let answer = http(addr, "POST", "/login", &page, &body).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let set_cookie = answer.header("Set-Cookie").unwrap();
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"));
    assert!(
        !set_cookie.contains(&token),
        "le jeton ne sert pas de cookie"
    );
    let session = set_cookie.split(';').next().unwrap().trim().to_owned();
    assert!(session.starts_with(&format!("{COOKIE}=auro_")), "{session}");

    // Cookie sans `Origin`, ou d'une autre origine : refusé (CSWSH).
    let status = refused_with(websocket(addr, &[("Cookie", session.as_str())]).await);
    assert_eq!(status, 403);
    let other = [
        ("Cookie", session.as_str()),
        ("Origin", "http://attaquant.example"),
    ];
    assert_eq!(refused_with(websocket(addr, &other).await), 403);

    // Depuis la page : le protocole entier, avec la portée du jeton.
    let mut socket = websocket(
        addr,
        &[("Cookie", session.as_str()), ("Origin", origin.as_str())],
    )
    .await
    .unwrap();
    socket.send(Message::text(hello())).await.unwrap();
    let reply = next_text(&mut socket).await.unwrap();
    assert_eq!(reply["result"]["scope"], "control", "{reply}");
    // Un message binaire n'est pas du protocole : la connexion se ferme.
    let mut binary = websocket(
        addr,
        &[("Cookie", session.as_str()), ("Origin", origin.as_str())],
    )
    .await
    .unwrap();
    binary
        .send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .unwrap();
    assert!(next_text(&mut binary).await.is_none());

    // Un script, par le jeton, sans `Origin`.
    let bearer = format!("Bearer {token}");
    let mut script = websocket(addr, &[("Authorization", bearer.as_str())])
        .await
        .unwrap();
    script.send(Message::text(hello())).await.unwrap();
    assert!(next_text(&mut script).await.is_some());

    // `/logout` : la session ne vaut plus.
    let out = [
        ("Host", host.as_str()),
        ("Origin", origin.as_str()),
        ("Cookie", session.as_str()),
    ];
    let answer = http(addr, "POST", "/logout", &out, "").await;
    assert_eq!(answer.status, 204);
    assert!(answer.header("Set-Cookie").unwrap().contains("Max-Age=0"));
    let again = [("Cookie", session.as_str()), ("Origin", origin.as_str())];
    assert_eq!(refused_with(websocket(addr, &again).await), 401);

    // Révocation : les WebSocket du jeton se ferment.
    assert!(engine.revoke_token(&row.id).await.unwrap());
    assert!(next_text(&mut socket).await.is_none());
    assert!(next_text(&mut script).await.is_none());
    crate::testing::stop(&engine).await;
}

async fn state_until(engine: &Engine, pred: impl Fn(&RemoteState) -> bool) -> RemoteState {
    for _ in 0..200 {
        let state = engine.remote_state();
        if pred(&state) {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("état attendu non atteint : {:?}", engine.remote_state());
}

/// `remote::run` suit les réglages : fermé par défaut, ouvert, déplacé, refermé ; hors de
/// la boucle locale, refusé sans TLS.
#[tokio::test]
async fn the_listener_follows_its_settings() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    tokio::spawn(run(engine.clone()));
    let state = state_until(&engine, |s| s.listen.is_some()).await;
    assert!(!state.enabled && !state.listening, "{state:?}");

    let free = || {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap()
    };
    let first = free();
    engine.set_remote_listen(Some(first)).await.unwrap();
    engine.set_flag(Flag::RemoteEnabled, true).await.unwrap();
    state_until(&engine, |s| {
        s.listening && s.listen == Some(first.to_string())
    })
    .await;
    assert!(TcpStream::connect(first).await.is_ok());

    let second = free();
    engine.set_remote_listen(Some(second)).await.unwrap();
    state_until(&engine, |s| {
        s.listening && s.listen == Some(second.to_string())
    })
    .await;
    assert!(TcpStream::connect(second).await.is_ok());
    assert!(
        TcpStream::connect(first).await.is_err(),
        "l'ancienne écoute est fermée"
    );

    // Hors de la boucle locale : en TLS ; une adresse qui n'est pas à la machine ne s'ouvre
    // pas (le certificat n'est fabriqué qu'une fois le port ouvert).
    let outside: SocketAddr = "192.0.2.10:7380".parse().unwrap();
    engine.set_remote_listen(Some(outside)).await.unwrap();
    let state = state_until(&engine, |s| s.error.is_some()).await;
    assert!(state.tls && !state.listening, "{state:?}");
    assert!(state.error.unwrap().contains("écoute impossible"));
    assert!(!dir.path().join(super::tls::CERT_FILE).exists());

    engine.set_remote_listen(Some(second)).await.unwrap();
    state_until(&engine, |s| s.listening).await;
    engine.set_flag(Flag::RemoteEnabled, false).await.unwrap();
    state_until(&engine, |s| !s.listening && !s.enabled).await;
    assert!(TcpStream::connect(second).await.is_err());
    assert_eq!(engine.remote_listen().await.unwrap(), second);
    engine.set_remote_listen(None).await.unwrap();
    assert_eq!(
        engine.remote_listen().await.unwrap().to_string(),
        crate::store::settings::REMOTE_DEFAULT_LISTEN
    );
    crate::testing::stop(&engine).await;
}

/// Client TLS qui n'accepte que le certificat de cette empreinte : ce que fait
/// l'utilisateur qui la compare dans son navigateur.
#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: std::sync::Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if super::tls::fingerprint(end_entity) == self.fingerprint {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("empreinte différente".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

type Tls = tokio_rustls::client::TlsStream<TcpStream>;

async fn tls_connect(addr: SocketAddr, fingerprint: &str) -> std::io::Result<Tls> {
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = Pinned {
        fingerprint: fingerprint.to_owned(),
        provider: provider.clone(),
    };
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(verifier))
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
    let tcp = TcpStream::connect(addr).await?;
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    connector.connect(name, tcp).await
}

async fn https(
    addr: SocketAddr,
    fingerprint: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    let stream = tls_connect(addr, fingerprint).await.unwrap();
    exchange(stream, method, path, headers, body).await
}

fn free_port() -> SocketAddr {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    probe.local_addr().unwrap()
}

/// L'écoute en TLS (forcée ici sur la boucle locale, comme hors d'elle) : certificat
/// fabriqué à côté de la base, son empreinte dans l'état ; requêtes, session `Secure` et
/// WebSocket chiffrés ; refus d'un client en clair ou qui attend un autre certificat ;
/// certificat refait quand un nom manque, repris sinon ; certificat fourni.
#[tokio::test]
async fn the_network_listener_serves_tls_with_its_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let (_, token) = engine.create_token("téléphone", "control").await.unwrap();
    tokio::spawn(super::supervise(engine.clone(), true));
    let addr = free_port();
    engine.set_remote_listen(Some(addr)).await.unwrap();
    engine.set_flag(Flag::RemoteEnabled, true).await.unwrap();
    let state = state_until(&engine, |s| s.listening).await;
    assert!(state.tls, "{state:?}");
    assert_eq!(state.urls, [format!("https://{addr}")]);
    let cert = state.certificate.expect("certificat");
    assert!(cert.generated);
    assert!(cert.names.contains(&"127.0.0.1".to_owned()), "{cert:?}");
    assert!(dir.path().join(super::tls::CERT_FILE).exists());
    assert!(dir.path().join(super::tls::KEY_FILE).exists());
    let fp = cert.fingerprint.clone();

    let host = format!("127.0.0.1:{}", addr.port());
    let bearer = format!("Bearer {token}");
    let ok = [
        ("Host", host.as_str()),
        ("Authorization", bearer.as_str()),
        ("Content-Type", "application/json"),
    ];
    let answer = https(addr, &fp, "POST", "/rpc", &ok, &hello()).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    // L'origine de la page est en `https` : celle en `http` est étrangère.
    let mut from_http = ok.to_vec();
    let http_origin = format!("http://{host}");
    from_http.push(("Origin", &http_origin));
    assert_eq!(
        https(addr, &fp, "POST", "/rpc", &from_http, &hello())
            .await
            .status,
        403
    );
    // En clair sur le port TLS : aucune réponse HTTP.
    assert_eq!(http(addr, "POST", "/rpc", &ok, &hello()).await.status, 0);
    // Un client qui attend un autre certificat (attaquant au milieu) : refusé.
    assert!(tls_connect(addr, "00:11").await.is_err());

    // Session du navigateur : cookie `Secure`.
    let origin = format!("https://{host}");
    let login = [
        ("Host", host.as_str()),
        ("Origin", origin.as_str()),
        ("Content-Type", "application/json"),
    ];
    let body = json!({"token": token}).to_string();
    let answer = https(addr, &fp, "POST", "/login", &login, &body).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let set_cookie = answer.header("Set-Cookie").unwrap();
    assert!(set_cookie.contains("; Secure"), "{set_cookie}");
    let session = set_cookie.split(';').next().unwrap().to_owned();

    // WebSocket chiffré, ouvert par le cookie depuis la page.
    let mut request = format!("wss://127.0.0.1:{}/ws", addr.port())
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", HeaderValue::from_str(&origin).unwrap());
    request
        .headers_mut()
        .insert("Cookie", HeaderValue::from_str(&session).unwrap());
    let stream = tls_connect(addr, &fp).await.unwrap();
    let (mut socket, _) = tokio_tungstenite::client_async(request, stream)
        .await
        .unwrap();
    socket.send(Message::text(hello())).await.unwrap();
    let reply = next_text(&mut socket).await.unwrap();
    assert_eq!(reply["result"]["scope"], "control", "{reply}");
    drop(socket);

    // Un nom ajouté : le certificat est refait, et `Host` l'accepte.
    engine
        .set_remote_hosts(vec!["NAS.maison".to_owned()])
        .await
        .unwrap();
    let state = state_until(&engine, |s| {
        s.listening
            && s.certificate
                .as_ref()
                .is_some_and(|c| c.names.contains(&"nas.maison".to_owned()))
    })
    .await;
    let second = state.certificate.unwrap().fingerprint;
    assert_ne!(second, fp);
    assert!(
        state
            .urls
            .contains(&format!("https://nas.maison:{}", addr.port()))
    );
    let nas = format!("nas.maison:{}", addr.port());
    let by_name = [
        ("Host", nas.as_str()),
        ("Authorization", bearer.as_str()),
        ("Content-Type", "application/json"),
    ];
    assert_eq!(
        https(addr, &second, "POST", "/rpc", &by_name, &hello())
            .await
            .status,
        200
    );
    // Retiré : le certificat, qui le porte encore, est repris tel quel.
    engine.set_remote_hosts(Vec::new()).await.unwrap();
    let state = state_until(&engine, |s| s.listening && s.urls.len() == 1).await;
    assert_eq!(state.certificate.unwrap().fingerprint, second);
    assert_eq!(
        https(addr, &second, "POST", "/rpc", &by_name, &hello())
            .await
            .status,
        421
    );

    // Certificat fourni ; un fichier absent coupe l'écoute, avec la raison.
    let other = tempfile::tempdir().unwrap();
    let names = super::tls::wanted_names(addr, &[]);
    let mine = super::tls::generated(other.path(), &names, crate::store::now_ms()).unwrap();
    let path = |name: &str| other.path().join(name).to_string_lossy().into_owned();
    engine
        .set_remote_tls(Some((
            path(super::tls::CERT_FILE),
            path(super::tls::KEY_FILE),
        )))
        .await
        .unwrap();
    let state = state_until(&engine, |s| {
        s.listening && s.certificate.as_ref().is_some_and(|c| !c.generated)
    })
    .await;
    assert_eq!(state.certificate.unwrap().fingerprint, mine.fingerprint);
    assert_eq!(
        https(addr, &mine.fingerprint, "POST", "/rpc", &ok, &hello())
            .await
            .status,
        200
    );
    engine
        .set_remote_tls(Some((path("absent.pem"), path(super::tls::KEY_FILE))))
        .await
        .unwrap();
    let state = state_until(&engine, |s| s.error.is_some()).await;
    assert!(!state.listening);
    assert!(state.error.unwrap().contains("absent.pem"));
    // Retour au certificat fabriqué, toujours le même.
    engine.set_remote_tls(None).await.unwrap();
    let state = state_until(&engine, |s| s.listening).await;
    assert_eq!(state.certificate.unwrap().fingerprint, second);
    // Chemin relatif : refusé.
    assert!(
        engine
            .set_remote_tls(Some(("c.pem".to_owned(), "k.pem".to_owned())))
            .await
            .is_err()
    );
    crate::testing::stop(&engine).await;
}

/// Valeurs de `Host` acceptées (SPEC §7.19) : adresses IP en chiffres et noms connus, avec
/// le bon port ; sans port, seulement sur celui par défaut du protocole.
#[test]
fn host_values_name_this_server_only() {
    let rule = server::HostRule::new(7380, true, vec!["localhost".into(), "nas.local".into()]);
    for good in [
        "127.0.0.1:7380",
        "192.168.1.20:7380",
        "[::1]:7380",
        "[fe80::1]:7380",
        "localhost:7380",
        "NAS.local:7380",
        "nas.local.:7380",
    ] {
        assert!(rule.accepts(good), "{good}");
    }
    for bad in [
        "attaquant.example:7380",
        "192.168.1.20:7381",
        "192.168.1.20",
        "nas.local",
        "[::1]",
        "::1:7380",
        "[::1]x:7380",
        "192.168.1.20:+7380",
        "192.168.1.20: 7380",
        "localhost:7380:7380",
        "192.168.1.20.example:7380",
        "",
    ] {
        assert!(!rule.accepts(bad), "{bad}");
    }
    let https = server::HostRule::new(443, true, vec!["nas.local".into()]);
    assert!(https.accepts("nas.local") && https.accepts("10.0.0.2") && https.accepts("[::1]"));
    assert!(https.accepts("nas.local:443") && !https.accepts("nas.local:80"));
    let http = server::HostRule::new(80, false, vec![]);
    assert!(http.accepts("127.0.0.1") && !http.accepts("localhost"));
}

/// Le certificat fabriqué arrive à 30 jours de son expiration pendant que l'écoute tourne :
/// il est refait à cette échéance, sans changement de réglage, et l'écoute repart avec lui.
#[tokio::test]
async fn the_generated_certificate_is_renewed_when_due() {
    use super::tls::{BACKDATE_S, RENEW_BEFORE_MS, VALIDITY_S};
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let addr = free_port();
    // Un certificat dont l'échéance tombe dans cinq secondes.
    let due = crate::store::now_ms() + 5000;
    let made_at = ((due + RENEW_BEFORE_MS) / 1000 + BACKDATE_S - VALIDITY_S) * 1000;
    let names = super::tls::wanted_names(addr, &[]);
    let old = super::tls::generated(dir.path(), &names, made_at).unwrap();
    tokio::spawn(super::supervise(engine.clone(), true));
    engine.set_remote_listen(Some(addr)).await.unwrap();
    engine.set_flag(Flag::RemoteEnabled, true).await.unwrap();
    let state = state_until(&engine, |s| s.listening).await;
    assert_eq!(
        state.certificate.unwrap().fingerprint,
        old.fingerprint,
        "repris tant que l'échéance n'est pas atteinte"
    );
    let mut renewed = None;
    for i in 0..200 {
        // D'autres événements du moteur, sans cesse (comme l'avancement des tâches) : ils
        // ne repoussent pas l'échéance.
        engine
            .set_flag(Flag::TorrentUpnp, i % 2 == 0)
            .await
            .unwrap();
        let state = engine.remote_state();
        if let Some(cert) = state
            .certificate
            .filter(|c| c.fingerprint != old.fingerprint)
        {
            renewed = Some(cert);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let renewed = renewed.expect("certificat refait à l'échéance");
    assert!(renewed.expires_at - crate::store::now_ms() > 800 * 24 * 3600 * 1000);
    assert_eq!(
        https(
            addr,
            &renewed.fingerprint,
            "POST",
            "/rpc",
            &[("Host", &format!("127.0.0.1:{}", addr.port()))],
            ""
        )
        .await
        .status,
        401
    );
    crate::testing::stop(&engine).await;
}
