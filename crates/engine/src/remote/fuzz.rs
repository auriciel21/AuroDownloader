//! Lecteurs de requêtes de l'interface distante face à des entrées quelconques (SPEC §7.19,
//! étape 3e ; §9.1). Tests de propriétés : jamais de panique ; `Host` n'accepte qu'une
//! adresse écrite en chiffres ou un nom connu, au bon port (DNS rebinding) ; `Origin` que
//! la même origine ; le cookie de session et le jeton ne se lisent que sous leur nom. Puis
//! le serveur lui-même face à des requêtes abîmées (il répond ou ferme, jamais d'erreur
//! interne ni de session ouverte, et sert encore la requête suivante) et face aux clients
//! muets ou lents, en clair et en TLS : ils perdent leur place au bout du délai.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::time::{Duration, Instant};

use hyper::header::{self, HeaderMap, HeaderValue};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use super::server::{self, HostRule, bearer, is_json, login_token, origin_matches, session_cookie};
use crate::net::Client;
use crate::scheduler::{Config, DISK_RECHECK, Engine, Planning};
use crate::transfer::Options;

const PORT: u16 = 7380;
const NAME: &str = "nas.maison";

fn rule() -> HostRule {
    HostRule::new(PORT, true, vec![NAME.to_owned()])
}

/// Ce qu'une valeur de `Host` acceptée doit être, écrit autrement que `HostRule` : une
/// adresse IPv4 en chiffres, une IPv6 entre crochets ou le nom connu, puis `:` et le port.
fn names_this_server(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let Some(at) = host.rfind(':') else {
        return false;
    };
    let (name, port) = (&host[..at], &host[at + 1..]);
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if port.parse::<u32>().ok() != Some(u32::from(PORT)) {
        return false;
    }
    if let Some(v6) = name.strip_prefix('[').and_then(|n| n.strip_suffix(']')) {
        return v6.parse::<Ipv6Addr>().is_ok();
    }
    name.parse::<Ipv4Addr>().is_ok() || name.trim_end_matches('.') == NAME
}

fn header_value(bytes: &[u8]) -> Option<HeaderValue> {
    HeaderValue::from_bytes(bytes).ok()
}

/// Octets d'une valeur d'en-tête : surtout du texte, parfois n'importe quoi.
fn value_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        "[ -~]{0,80}".prop_map(String::into_bytes),
        proptest::collection::vec(any::<u8>(), 0..80),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// `Host` : jamais de panique ; accepté seulement s'il nomme ce serveur.
    #[test]
    fn host_values_never_name_another_server(host in prop_oneof![
        "[ -~]{0,60}",
        "(\\[[0-9a-fA-F:.]{0,40}\\]|[0-9.]{0,20}|[a-zA-Z0-9.-]{0,20})(:[0-9+ -]{0,7})?",
    ]) {
        if rule().accepts(&host) {
            prop_assert!(names_this_server(&host), "{host:?} accepté");
        }
    }

    /// Adresses et noms bien formés : acceptés exactement au bon port, pour une adresse ou
    /// le nom connu.
    #[test]
    fn well_formed_hosts_are_judged_by_name_and_port(
        v4 in any::<[u8; 4]>(),
        v6 in any::<[u16; 8]>(),
        name in "[a-z0-9-]{1,12}(\\.[a-z0-9-]{1,12}){0,2}",
        port in prop_oneof![Just(PORT), any::<u16>()],
    ) {
        let v4 = Ipv4Addr::from(v4);
        let v6 = Ipv6Addr::from(v6);
        let right = port == PORT;
        prop_assert_eq!(rule().accepts(&format!("{v4}:{port}")), right);
        prop_assert_eq!(rule().accepts(&format!("[{v6}]:{port}")), right);
        prop_assert_eq!(rule().accepts(&format!("{name}:{port}")), right && name == NAME);
        prop_assert_eq!(rule().accepts(&format!("{}:{port}", NAME.to_ascii_uppercase())), right);
        // Sans crochets, une IPv6 n'est jamais prise pour un nom suivi d'un port.
        let bare = v6.to_string();
        prop_assert!(!rule().accepts(&bare), "{} accepté", bare);
    }

    /// `Origin` : seule une valeur unique égale à `https://<Host>` passe ; aucune n'est
    /// acceptée si elles sont plusieurs.
    #[test]
    fn only_the_same_origin_passes(
        values in proptest::collection::vec(prop_oneof![
            value_bytes(),
            Just(b"https://nas.maison:7380".to_vec()),
            Just(b"HTTPS://NAS.MAISON:7380".to_vec()),
            Just(b"http://nas.maison:7380".to_vec()),
            Just(b"null".to_vec()),
        ], 0..3),
    ) {
        let mut headers = HeaderMap::new();
        for value in values.iter().filter_map(|v| header_value(v)) {
            headers.append(header::ORIGIN, value);
        }
        let host = "nas.maison:7380";
        let result = origin_matches("https", &headers, host);
        let present: Vec<&HeaderValue> = headers.get_all(header::ORIGIN).iter().collect();
        match result {
            Ok(false) => prop_assert!(present.is_empty()),
            Ok(true) => {
                prop_assert_eq!(present.len(), 1);
                let text = present[0].to_str().unwrap().to_ascii_lowercase();
                prop_assert_eq!(text, format!("https://{host}"));
            }
            Err(()) => prop_assert!(!present.is_empty()),
        }
    }

    /// `Authorization` : un jeton seulement après `Bearer` et un espace, sans espaces autour.
    #[test]
    fn a_bearer_token_is_read_only_after_its_scheme(value in value_bytes()) {
        let mut headers = HeaderMap::new();
        if let Some(v) = header_value(&value) {
            headers.insert(header::AUTHORIZATION, v);
        }
        if let Some(token) = bearer(&headers) {
            let text = headers[header::AUTHORIZATION].to_str().unwrap();
            prop_assert!(text[..7].eq_ignore_ascii_case("bearer "), "{text:?}");
            prop_assert_eq!(token, token.trim());
            prop_assert_eq!(token, text[7..].trim());
        }
    }

    /// Cookies : seule la valeur d'un cookie qui porte exactement le nom de la session.
    #[test]
    fn the_session_cookie_is_read_under_its_own_name_only(
        values in proptest::collection::vec(prop_oneof![
            value_bytes(),
            "[a-z_]{0,14}=[ -:<-~]{0,20}(; ?[a-z_]{0,14}=[ -:<-~]{0,20}){0,3}".prop_map(String::into_bytes),
            "(x?auro_session|auro_sessionx|AURO_SESSION)=[A-Za-z0-9_-]{0,10}".prop_map(String::into_bytes),
        ], 0..3),
    ) {
        let mut headers = HeaderMap::new();
        for value in values.iter().filter_map(|v| header_value(v)) {
            headers.append(header::COOKIE, value);
        }
        if let Some(found) = session_cookie(&headers) {
            let named = headers
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|v| v.split(';'))
                .filter_map(|pair| pair.trim().split_once('='))
                .any(|(name, value)| name == server::COOKIE && value.trim() == found);
            prop_assert!(named, "{found:?} lu sans cookie {}", server::COOKIE);
        }
    }

    /// `Content-Type` : JSON seulement pour `application/json`, paramètres permis.
    #[test]
    fn json_content_type_is_recognised_strictly(value in value_bytes()) {
        let mut headers = HeaderMap::new();
        if let Some(v) = header_value(&value) {
            headers.insert(header::CONTENT_TYPE, v);
        }
        if is_json(&headers) {
            let text = headers[header::CONTENT_TYPE].to_str().unwrap();
            let kind = text.split(';').next().unwrap().trim();
            prop_assert!(kind.eq_ignore_ascii_case("application/json"), "{text:?}");
        }
    }

    /// Corps de `/login` : jamais de panique ; un jeton seulement s'il est une chaîne.
    #[test]
    fn the_login_body_gives_a_string_token_or_nothing(
        body in prop_oneof![
            proptest::collection::vec(any::<u8>(), 0..200),
            "[ -~]{0,120}".prop_map(String::into_bytes),
        ],
        token in "\\PC{0,60}",
    ) {
        let _ = login_token(&body);
        let wrapped = serde_json::json!({"token": token, "autre": 1}).to_string();
        prop_assert_eq!(login_token(wrapped.as_bytes()), Some(token.trim().to_owned()));
        let number = serde_json::json!({"token": 12}).to_string();
        prop_assert_eq!(login_token(number.as_bytes()), None);
    }
}

// ---------- Le serveur face à des requêtes abîmées et à des clients lents ----------

/// Délai des clients lents ou muets dans ces tests (10 s en vrai).
const SHORT: Duration = Duration::from_secs(1);

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

/// Un serveur sur la boucle locale, au délai court ; en TLS avec un certificat fabriqué
/// dans `dir` si `tls`.
async fn serving(engine: &Engine, dir: &Path, tls: bool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = tls.then(|| {
        let names = super::tls::wanted_names(addr, &[]);
        super::tls::generated(dir, &names, crate::store::now_ms())
            .unwrap()
            .config
    });
    let listening = server::Listening {
        tls: config,
        names: super::tls::known_names(&[]),
    };
    tokio::spawn(server::serve_timed(
        engine.clone(),
        std::sync::Arc::new(listener),
        listening,
        SHORT,
    ));
    addr
}

/// Envoie `raw` et rend le début de la réponse : ses en-têtes, ou ce qui est arrivé avant la
/// fermeture (vide si le serveur a fermé sans répondre), ou au bout de 2 s. L'envoi n'est
/// pas fermé : hyper, sans demi-fermeture, couperait sans répondre.
async fn send_raw(addr: SocketAddr, raw: &[u8]) -> Vec<u8> {
    send_from(Ipv4Addr::LOCALHOST, addr, raw).await
}

/// [`send_raw`] depuis une autre adresse de la boucle locale (127.0.0.0/8) : le blocage
/// après 10 échecs vaut par adresse.
async fn send_from(from: Ipv4Addr, addr: SocketAddr, raw: &[u8]) -> Vec<u8> {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind(SocketAddr::new(IpAddr::V4(from), 0)).unwrap();
    let mut stream = socket.connect(addr).await.unwrap();
    // Le serveur peut fermer avant d'avoir tout lu : l'écriture échoue alors.
    let _ = stream.write_all(raw).await;
    let mut answer = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buf = [0u8; 4096];
    while !answer.windows(4).any(|w| w == b"\r\n\r\n") {
        let left = deadline.saturating_duration_since(Instant::now());
        match timeout(left, stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => answer.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    answer
}

/// Statut d'une réponse HTTP, 0 sans réponse.
fn status_of(answer: &[u8]) -> u16 {
    let text = String::from_utf8_lossy(answer);
    text.split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Le serveur sert encore : une requête correcte reçoit sa réponse (`401` sans session).
async fn still_serving(addr: SocketAddr) {
    let request = format!("GET /session HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let answer = send_raw(addr, request.as_bytes()).await;
    assert_eq!(
        status_of(&answer),
        401,
        "{}",
        String::from_utf8_lossy(&answer)
    );
}

/// Requêtes correctes, abîmées ensuite : en-têtes de la page, de la session, du
/// WebSocket, de `/rpc` et de `/login` (avec un faux jeton).
fn templates(addr: SocketAddr) -> Vec<String> {
    let fake = format!("auro_{}", "A".repeat(43));
    vec![
        format!("GET /index.html HTTP/1.1\r\nHost: {addr}\r\n\r\n"),
        format!("GET /session HTTP/1.1\r\nHost: {addr}\r\nCookie: auro_session={fake}\r\n\r\n"),
        format!(
            "GET /ws HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nCookie: auro_session={fake}\r\n\r\n"
        ),
        format!(
            "POST /rpc HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {fake}\r\n\
             Content-Type: application/json\r\nContent-Length: 17\r\n\r\n{{\"jsonrpc\":\"2.0\"}}"
        ),
        format!(
            "POST /login HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\n\
             Content-Type: application/json\r\nContent-Length: 59\r\n\r\n{{\"token\":\"{fake}\"}}"
        ),
    ]
}

/// Une abîmure : octets remplacés, insérés (dont des fins de ligne et des `:`), coupure.
#[derive(Debug, Clone)]
enum Damage {
    Replace(usize, u8),
    Insert(usize, Vec<u8>),
    Cut(usize),
    Repeat(usize, usize),
}

fn damage() -> impl Strategy<Value = Damage> {
    prop_oneof![
        (any::<usize>(), any::<u8>()).prop_map(|(at, b)| Damage::Replace(at, b)),
        (
            any::<usize>(),
            prop_oneof![
                proptest::collection::vec(any::<u8>(), 1..24),
                Just(b"\r\n".to_vec()),
                Just(b"\r\n\r\n".to_vec()),
                Just(b":".to_vec()),
                Just(b"\0".to_vec()),
                Just(b"Transfer-Encoding: chunked\r\n".to_vec()),
                Just(b"Content-Length: 99999999999999999999\r\n".to_vec()),
                Just(b"Host: evil.example\r\n".to_vec()),
            ]
        )
            .prop_map(|(at, bytes)| Damage::Insert(at, bytes)),
        any::<usize>().prop_map(Damage::Cut),
        (any::<usize>(), 1..2000usize).prop_map(|(at, n)| Damage::Repeat(at, n)),
    ]
}

fn apply(mut raw: Vec<u8>, damages: &[Damage]) -> Vec<u8> {
    for d in damages {
        let len = raw.len().max(1);
        match d {
            Damage::Replace(at, b) => {
                if !raw.is_empty() {
                    let at = at % raw.len();
                    raw[at] = *b;
                }
            }
            Damage::Insert(at, bytes) => {
                let at = at % (raw.len() + 1);
                raw.splice(at..at, bytes.iter().copied());
            }
            Damage::Cut(at) => raw.truncate(at % len),
            Damage::Repeat(at, n) => {
                if !raw.is_empty() {
                    let b = raw[at % raw.len()];
                    let at = at % raw.len();
                    raw.splice(at..at, std::iter::repeat_n(b, *n));
                }
            }
        }
    }
    raw
}

/// Requêtes abîmées : le serveur répond ou ferme, sans erreur interne, sans jamais ouvrir
/// de session ni répondre à `/rpc` sans jeton valable, et sert encore la suivante.
#[tokio::test]
async fn damaged_requests_never_break_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    // Un vrai jeton existe : un hasard ne doit jamais l'atteindre.
    engine.create_token("téléphone", "control").await.unwrap();
    let addr = serving(&engine, dir.path(), false).await;
    let templates = templates(addr);
    let mut runner = TestRunner::deterministic();
    let strategy = (
        0..templates.len(),
        proptest::collection::vec(damage(), 1..5),
    );
    let cases = std::env::var("AURO_FUZZ_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(200u32);
    let mut statuses = std::collections::BTreeMap::new();
    for case in 0..cases {
        let (which, damages) = strategy.new_tree(&mut runner).unwrap().current();
        let raw = apply(templates[which].clone().into_bytes(), &damages);
        // Une adresse par cas, ou presque : le blocage ne cache pas le reste du serveur.
        let from = Ipv4Addr::new(127, 0, 0, 2 + (case % 250) as u8);
        let answer = send_from(from, addr, &raw).await;
        let text = String::from_utf8_lossy(&answer);
        let status = status_of(&answer);
        let shown = String::from_utf8_lossy(&raw);
        assert!(
            !(500..600).contains(&status),
            "cas {case} : {status} pour {shown:?}\n{text}"
        );
        assert!(
            !text.contains("auro_session=auro_"),
            "cas {case} : session ouverte pour {shown:?}\n{text}"
        );
        assert!(
            !(status == 200 && which == 3),
            "cas {case} : /rpc servi pour {shown:?}\n{text}"
        );
        assert_ne!(status, 101, "cas {case} : WebSocket ouvert pour {shown:?}");
        *statuses.entry(status).or_insert(0u32) += 1;
    }
    // Le test ne vaut que si le serveur a répondu (une réponse jamais lue passerait tout).
    let answered: u32 = statuses
        .iter()
        .filter(|(s, _)| **s != 0)
        .map(|(_, n)| n)
        .sum();
    println!("requêtes abîmées : {cases}, statuts {statuses:?}");
    assert!(answered * 2 > cases, "trop peu de réponses : {statuses:?}");
    still_serving(addr).await;
    crate::testing::stop(&engine).await;
}

/// Une connexion muette, ou qui envoie ses en-têtes goutte à goutte, est fermée au bout du
/// délai, en clair comme en TLS (négociation comprise) ; quand toutes les places sont
/// prises par des muets, la suivante est refusée, puis servie une fois le délai passé.
#[tokio::test]
async fn silent_and_slow_clients_lose_their_place() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    for tls in [false, true] {
        let addr = serving(&engine, dir.path(), tls).await;
        // Muet.
        let start = Instant::now();
        let mut silent = TcpStream::connect(addr).await.unwrap();
        let mut buf = [0u8; 64];
        let read = timeout(Duration::from_secs(10), silent.read(&mut buf))
            .await
            .expect("connexion muette jamais fermée");
        assert!(matches!(read, Ok(0) | Err(_)), "tls {tls} : {read:?}");
        let waited = start.elapsed();
        assert!(
            waited >= SHORT / 2 && waited < SHORT * 5,
            "tls {tls} : fermée après {waited:?}"
        );
    }
    // Goutte à goutte, en clair : fermée malgré les octets qui arrivent.
    let addr = serving(&engine, dir.path(), false).await;
    let start = Instant::now();
    let mut slow = TcpStream::connect(addr).await.unwrap();
    let head = format!("GET /session HTTP/1.1\r\nHost: {addr}\r\nX-Lent: ");
    slow.write_all(head.as_bytes()).await.unwrap();
    let mut closed = false;
    while start.elapsed() < Duration::from_secs(10) {
        tokio::time::sleep(Duration::from_millis(150)).await;
        if slow.write_all(b"a").await.is_err() {
            closed = true;
            break;
        }
        let mut buf = [0u8; 256];
        if let Ok(Ok(n)) = timeout(Duration::from_millis(1), slow.read(&mut buf)).await {
            // Fermée (0), ou une réponse d'erreur avant la fermeture.
            let _ = n;
            closed = true;
            break;
        }
    }
    assert!(closed, "en-têtes goutte à goutte jamais coupés");
    assert!(
        start.elapsed() < SHORT * 5,
        "coupée après {:?}",
        start.elapsed()
    );

    // Toutes les places prises par des muets : la suivante est fermée aussitôt.
    let addr = serving(&engine, dir.path(), false).await;
    let mut holders = Vec::new();
    for _ in 0..64 {
        holders.push(TcpStream::connect(addr).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let request = format!("GET /session HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let refused = send_raw(addr, request.as_bytes()).await;
    assert_eq!(
        status_of(&refused),
        0,
        "{}",
        String::from_utf8_lossy(&refused)
    );
    // Le délai passé, les muets sont partis : la requête est servie.
    tokio::time::sleep(SHORT * 2).await;
    still_serving(addr).await;
    drop(holders);
    crate::testing::stop(&engine).await;
}

/// Une adresse IP quelconque, écrite dans `Host`, n'est jamais prise pour un nom.
#[test]
fn ip_literals_are_never_names() {
    for ip in [
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ] {
        let host = SocketAddr::new(ip, PORT).to_string();
        assert!(rule().accepts(&host), "{host}");
        assert!(names_this_server(&host), "{host}");
    }
}
