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
    tokio::spawn(server::serve(engine.clone(), listener));
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
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut request = format!("{method} {path} HTTP/1.1\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    timeout(WAIT, stream.read_to_end(&mut raw))
        .await
        .expect("pas de réponse")
        .unwrap();
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

    let outside: SocketAddr = "192.0.2.10:7380".parse().unwrap();
    engine.set_remote_listen(Some(outside)).await.unwrap();
    let state = state_until(&engine, |s| s.error.is_some()).await;
    assert!(!state.listening && state.error.unwrap().contains("TLS"));

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
}
