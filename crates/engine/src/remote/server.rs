//! Écoute HTTP et WebSocket de l'interface distante (SPEC §7.19).
//!
//! Routes : `GET /ws` (WebSocket, un message JSON-RPC par trame texte), `POST /rpc` (un
//! message, une réponse, sans notifications), `POST /login` (jeton échangé contre un cookie
//! de session), `POST /logout`. La page elle-même viendra à l'étape 3c.
//!
//! Avant toute route : `Host` doit nommer ce serveur (contre le DNS rebinding) ; une
//! requête avec `Origin` doit venir de la même origine (contre les pages d'autres sites) ;
//! aucun en-tête CORS n'est jamais envoyé. Puis l'authentification : `Authorization:
//! Bearer <jeton>` partout, le cookie de session seulement pour le WebSocket et
//! `/logout`, et alors avec un `Origin` de même origine ; jamais pour `POST /rpc`
//! (décision de l'utilisateur : contre le CSRF). Au-delà de [`MAX_FAILURES`] échecs en une
//! minute, une adresse est refusée (`429`) pendant une minute.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, Limited};
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Message, Role, WebSocketConfig};

use crate::rpc::{Incoming, MAX_MESSAGE, Peer, serve_messages};
use crate::scheduler::Engine;
use crate::store::remote::TokenRow;

/// Nom du cookie de session.
pub const COOKIE: &str = "auro_session";
/// Échecs d'authentification tolérés par adresse et par minute.
pub const MAX_FAILURES: u32 = 10;
/// Durée d'un refus après trop d'échecs, et fenêtre où ils se comptent.
const PENALTY: Duration = Duration::from_secs(60);
/// Connexions servies à la fois ; les suivantes sont fermées aussitôt.
const MAX_CONNECTIONS: usize = 64;
/// En-têtes d'une requête reçus dans ce délai, sinon la connexion se ferme.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// Corps de `POST /login`, au plus.
const MAX_LOGIN: usize = 4096;
/// Corps de `POST /rpc` reçu en entier dans ce délai (24 Mio au plus).
const RPC_BODY_TIMEOUT: Duration = Duration::from_secs(120);

type Reply = Response<Full<Bytes>>;
/// Une réponse de refus, rendue comme erreur (en boîte : elle est grosse).
type Refusal = Box<Reply>;

/// Ce que toutes les connexions d'une écoute partagent.
struct Shared {
    engine: Engine,
    /// Valeurs de `Host` acceptées, en minuscules.
    hosts: Vec<String>,
    /// `https` (étape 3b) ou `http`.
    scheme: &'static str,
    failures: Mutex<HashMap<IpAddr, Failures>>,
    /// Change (se ferme) quand l'écoute s'arrête : les WebSocket en cours se ferment avec
    /// elle.
    closed: watch::Receiver<()>,
}

#[derive(Debug, Clone, Copy)]
struct Failures {
    since: Instant,
    count: u32,
    blocked_until: Option<Instant>,
}

/// Valeurs de `Host` qui désignent ce serveur : son adresse d'écoute, et pour la boucle
/// locale `localhost`, `127.0.0.1` et `[::1]` ; toujours avec le port (un navigateur
/// l'envoie dès qu'il n'est pas celui par défaut), et aussi sans lui sur le port par
/// défaut du protocole.
pub fn host_names(addr: SocketAddr, tls: bool) -> Vec<String> {
    let port = addr.port();
    let mut names: Vec<String> = Vec::new();
    let ip = match addr.ip() {
        IpAddr::V6(v6) => format!("[{v6}]"),
        IpAddr::V4(v4) => v4.to_string(),
    };
    names.push(ip);
    if addr.ip().is_loopback() || addr.ip().is_unspecified() {
        names.extend(["localhost", "127.0.0.1", "[::1]"].map(str::to_owned));
    }
    names.sort();
    names.dedup();
    let default_port = if tls { 443 } else { 80 };
    let mut hosts: Vec<String> = names.iter().map(|n| format!("{n}:{port}")).collect();
    if port == default_port {
        hosts.extend(names);
    }
    hosts
}

/// Sert l'interface distante sur `listener` jusqu'à ce que la tâche soit arrêtée ; les
/// connexions en cours s'arrêtent avec elle.
pub async fn serve(engine: Engine, listener: TcpListener) -> std::io::Result<()> {
    let addr = listener.local_addr()?;
    // Gardé jusqu'à l'arrêt de l'écoute : sa disparition ferme les WebSocket.
    let (_closing, closed) = watch::channel(());
    let shared = Arc::new(Shared {
        engine,
        hosts: host_names(addr, false),
        scheme: "http",
        failures: Mutex::default(),
        closed,
    });
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(pair) => pair,
                    // Trop de fichiers ouverts, connexion avortée : on continue.
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
                    continue;
                };
                let shared = Arc::clone(&shared);
                connections.spawn(async move {
                    let _permit = permit;
                    serve_connection(shared, stream, peer).await;
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn serve_connection(shared: Arc<Shared>, stream: TcpStream, peer: SocketAddr) {
    let _ = stream.set_nodelay(true);
    let service = hyper::service::service_fn(move |request| {
        let shared = Arc::clone(&shared);
        async move { Ok::<_, Infallible>(route(shared, peer.ip(), request).await) }
    });
    let mut http = hyper::server::conn::http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .max_buf_size(64 * 1024);
    let _ = http
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await;
}

async fn route(shared: Arc<Shared>, ip: IpAddr, request: Request<hyper::body::Incoming>) -> Reply {
    let headers = request.headers();
    let Some(host) = checked_host(&shared, headers) else {
        return plain(StatusCode::MISDIRECTED_REQUEST, "hôte inconnu");
    };
    let origin = match checked_origin(&shared, headers, &host) {
        Ok(origin) => origin,
        Err(()) => return plain(StatusCode::FORBIDDEN, "origine refusée"),
    };
    match (request.method(), request.uri().path()) {
        (&Method::GET, "/ws") => websocket(shared, ip, origin, request).await,
        (&Method::POST, "/rpc") => rpc_post(shared, ip, request).await,
        (&Method::POST, "/login") => login(shared, ip, origin, request).await,
        (&Method::POST, "/logout") => logout(shared, origin, request).await,
        (_, "/ws" | "/rpc" | "/login" | "/logout") => plain(
            StatusCode::METHOD_NOT_ALLOWED,
            "méthode non prise en charge",
        ),
        _ => plain(StatusCode::NOT_FOUND, "introuvable"),
    }
}

/// `Host` de la requête s'il désigne ce serveur, en minuscules.
fn checked_host(shared: &Shared, headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get(header::HOST)?
        .to_str()
        .ok()?
        .to_ascii_lowercase();
    shared.hosts.contains(&host).then_some(host)
}

/// `Ok(true)` : `Origin` présent et de même origine ; `Ok(false)` : absent ; `Err` : une
/// autre origine (ou `null`, ou plusieurs).
fn checked_origin(shared: &Shared, headers: &HeaderMap, host: &str) -> Result<bool, ()> {
    let mut values = headers.get_all(header::ORIGIN).iter();
    let Some(origin) = values.next() else {
        return Ok(false);
    };
    if values.next().is_some() {
        return Err(());
    }
    let origin = origin.to_str().map_err(|_| ())?.to_ascii_lowercase();
    if origin == format!("{}://{host}", shared.scheme) {
        Ok(true)
    } else {
        Err(())
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.trim())
}

impl Shared {
    fn blocked(&self, ip: IpAddr) -> bool {
        let failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        failures
            .get(&ip)
            .and_then(|f| f.blocked_until)
            .is_some_and(|until| Instant::now() < until)
    }

    fn failed(&self, ip: IpAddr) {
        let now = Instant::now();
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        // Les adresses oubliées depuis longtemps partent : la table ne grossit pas.
        if failures.len() > 4096 {
            failures.retain(|_, f| now.duration_since(f.since) < PENALTY * 2);
        }
        let entry = failures.entry(ip).or_insert(Failures {
            since: now,
            count: 0,
            blocked_until: None,
        });
        if now.duration_since(entry.since) >= PENALTY {
            *entry = Failures {
                since: now,
                count: 0,
                blocked_until: None,
            };
        }
        entry.count += 1;
        if entry.count > MAX_FAILURES {
            entry.blocked_until = Some(now + PENALTY);
        }
    }
}

/// Comment une requête s'est authentifiée.
enum Credential {
    Bearer,
    Cookie,
}

/// Le jeton de la requête : `Authorization: Bearer`, ou le cookie si `cookie_ok`.
async fn authenticate(
    shared: &Shared,
    ip: IpAddr,
    headers: &HeaderMap,
    cookie_ok: bool,
) -> Result<(TokenRow, Credential), Refusal> {
    if shared.blocked(ip) {
        return Err(Box::new(plain(
            StatusCode::TOO_MANY_REQUESTS,
            "trop d'essais refusés : réessayer dans une minute",
        )));
    }
    let found = if let Some(token) = bearer(headers) {
        shared
            .engine
            .authenticate(token)
            .await
            .map(|t| t.map(|t| (t, Credential::Bearer)))
    } else if let Some(cookie) = session_cookie(headers).filter(|_| cookie_ok) {
        shared
            .engine
            .session(cookie)
            .await
            .map(|t| t.map(|t| (t, Credential::Cookie)))
    } else {
        return Err(Box::new(unauthorized()));
    };
    match found {
        Ok(Some(found)) => Ok(found),
        Ok(None) => {
            shared.failed(ip);
            Err(Box::new(unauthorized()))
        }
        Err(_) => Err(Box::new(plain(
            StatusCode::SERVICE_UNAVAILABLE,
            "moteur indisponible",
        ))),
    }
}

fn peer_of(token: &TokenRow) -> Option<Peer> {
    super::token_scope(&token.scope).map(|scope| Peer::token(&token.id, scope))
}

async fn websocket(
    shared: Arc<Shared>,
    ip: IpAddr,
    same_origin: bool,
    request: Request<hyper::body::Incoming>,
) -> Reply {
    let headers = request.headers();
    let (token, credential) = match authenticate(&shared, ip, headers, true).await {
        Ok(found) => found,
        Err(reply) => return *reply,
    };
    // Un cookie part avec toute requête du navigateur : seulement depuis notre page.
    if matches!(credential, Credential::Cookie) && !same_origin {
        return plain(StatusCode::FORBIDDEN, "origine absente");
    }
    let upgrade = headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let connection = headers
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
        });
    let version = headers
        .get(header::SEC_WEBSOCKET_VERSION)
        .is_some_and(|v| v == "13");
    let Some(key) = headers.get(header::SEC_WEBSOCKET_KEY).cloned() else {
        return plain(StatusCode::BAD_REQUEST, "WebSocket attendu");
    };
    if !(upgrade && connection && version) {
        return plain(StatusCode::BAD_REQUEST, "WebSocket attendu");
    }
    let Some(peer) = peer_of(&token) else {
        return plain(StatusCode::FORBIDDEN, "portée du jeton invalide");
    };
    let accept = derive_accept_key(key.as_bytes());
    let engine = shared.engine.clone();
    let closed = shared.closed.clone();
    tokio::spawn(async move {
        let Ok(upgraded) = hyper::upgrade::on(request).await else {
            return;
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let socket =
            WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, Some(config))
                .await;
        run_websocket(engine, peer, socket, closed).await;
    });
    let mut reply = Response::new(Full::new(Bytes::new()));
    *reply.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let h = reply.headers_mut();
    h.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    h.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    if let Ok(accept) = HeaderValue::from_str(&accept) {
        h.insert(header::SEC_WEBSOCKET_ACCEPT, accept);
    }
    reply
}

async fn run_websocket<S>(
    engine: Engine,
    peer: Peer,
    socket: WebSocketStream<S>,
    mut closed: watch::Receiver<()>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut frames) = socket.split();
    let (incoming, messages) = mpsc::channel(1);
    let (out, mut outgoing) = mpsc::channel::<String>(256);
    let mut serving = tokio::spawn(serve_messages(engine, peer, messages, out));
    let writing = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if sink.send(Message::text(message)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let mut served = false;
    loop {
        tokio::select! {
            frame = frames.next() => {
                let message = match frame {
                    Some(Ok(Message::Text(text))) => Incoming::Message(text.as_bytes().to_vec()),
                    // Ping, pong : tungstenite répond seul.
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                    Some(Err(tokio_tungstenite::tungstenite::Error::Capacity(_))) => {
                        Incoming::TooLong
                    }
                    // Binaire (le protocole est en texte), fermeture, erreur.
                    _ => break,
                };
                if incoming.send(message).await.is_err() {
                    break;
                }
            }
            _ = &mut serving => {
                served = true;
                break;
            }
            // L'écoute s'arrête (interface distante coupée).
            _ = closed.changed() => break,
        }
    }
    drop(incoming);
    if !served {
        let _ = serving.await;
    }
    let _ = writing.await;
}

/// `POST /rpc` : un message JSON-RPC (requête ou lot), sa réponse ; `Authorization`
/// obligatoire.
async fn rpc_post(
    shared: Arc<Shared>,
    ip: IpAddr,
    request: Request<hyper::body::Incoming>,
) -> Reply {
    let headers = request.headers();
    let (token, _) = match authenticate(&shared, ip, headers, false).await {
        Ok(found) => found,
        Err(reply) => return *reply,
    };
    if !is_json(headers) {
        return plain(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type: application/json attendu",
        );
    }
    let Some(peer) = peer_of(&token) else {
        return plain(StatusCode::FORBIDDEN, "portée du jeton invalide");
    };
    let body = match read_body(request, MAX_MESSAGE, RPC_BODY_TIMEOUT).await {
        Ok(body) => body,
        Err(reply) => return *reply,
    };
    let (incoming, messages) = mpsc::channel(1);
    let (out, mut outgoing) = mpsc::channel::<String>(256);
    let _ = incoming.send(Incoming::Message(body.to_vec())).await;
    drop(incoming);
    let serving = tokio::spawn(serve_messages(shared.engine.clone(), peer, messages, out));
    let mut replies = Vec::new();
    while let Some(message) = outgoing.recv().await {
        // Les notifications (abonnement) n'ont pas leur place ici.
        if let Ok(value) = serde_json::from_str::<Value>(&message)
            && (value.is_array() || value.get("id").is_some())
        {
            replies.push(message);
        }
    }
    let _ = serving.await;
    match replies.into_iter().next() {
        Some(reply) => json_reply(StatusCode::OK, reply),
        None => empty(StatusCode::NO_CONTENT),
    }
}

/// `POST /login` : `{"token": "…"}` depuis la page, échangé contre un cookie de session.
async fn login(
    shared: Arc<Shared>,
    ip: IpAddr,
    same_origin: bool,
    request: Request<hyper::body::Incoming>,
) -> Reply {
    if !same_origin {
        return plain(StatusCode::FORBIDDEN, "origine absente");
    }
    if !is_json(request.headers()) {
        return plain(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type: application/json attendu",
        );
    }
    if shared.blocked(ip) {
        return plain(
            StatusCode::TOO_MANY_REQUESTS,
            "trop d'essais refusés : réessayer dans une minute",
        );
    }
    let body = match read_body(request, MAX_LOGIN, HEADER_TIMEOUT).await {
        Ok(body) => body,
        Err(reply) => return *reply,
    };
    let token = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.get("token").and_then(Value::as_str).map(str::to_owned));
    let Some(token) = token else {
        return plain(StatusCode::BAD_REQUEST, "{\"token\": \"…\"} attendu");
    };
    let row = match shared.engine.authenticate(&token).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            shared.failed(ip);
            return unauthorized();
        }
        Err(_) => return plain(StatusCode::SERVICE_UNAVAILABLE, "moteur indisponible"),
    };
    let Ok(cookie) = shared.engine.open_session(&row.id).await else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "moteur indisponible");
    };
    let mut reply = json_reply(
        StatusCode::OK,
        json!({"name": row.name, "scope": row.scope}).to_string(),
    );
    let secure = if shared.scheme == "https" {
        "; Secure"
    } else {
        ""
    };
    let max_age = super::SESSION_TTL_MS / 1000;
    let value =
        format!("{COOKIE}={cookie}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}");
    if let Ok(value) = HeaderValue::from_str(&value) {
        reply.headers_mut().insert(header::SET_COOKIE, value);
    }
    reply
}

/// `POST /logout` : la session du cookie est fermée, le cookie effacé.
async fn logout(
    shared: Arc<Shared>,
    same_origin: bool,
    request: Request<hyper::body::Incoming>,
) -> Reply {
    if !same_origin {
        return plain(StatusCode::FORBIDDEN, "origine absente");
    }
    if let Some(cookie) = session_cookie(request.headers()) {
        let _ = shared.engine.close_session(cookie).await;
    }
    let mut reply = empty(StatusCode::NO_CONTENT);
    let value = format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
    if let Ok(value) = HeaderValue::from_str(&value) {
        reply.headers_mut().insert(header::SET_COOKIE, value);
    }
    reply
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
}

/// Le corps d'une requête, `limit` octets au plus, reçu en entier dans le délai voulu
/// (sans quoi un client lent garderait une place de connexion).
async fn read_body(
    request: Request<hyper::body::Incoming>,
    limit: usize,
    within: Duration,
) -> Result<Bytes, Refusal> {
    let too_long = || plain(StatusCode::PAYLOAD_TOO_LARGE, "message trop long");
    let announced = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if announced.is_some_and(|n| n > limit as u64) {
        return Err(Box::new(too_long()));
    }
    match tokio::time::timeout(within, Limited::new(request.into_body(), limit).collect()).await {
        Ok(Ok(collected)) => Ok(collected.to_bytes()),
        Ok(Err(_)) => Err(Box::new(too_long())),
        Err(_) => Err(Box::new(plain(
            StatusCode::REQUEST_TIMEOUT,
            "message reçu trop lentement",
        ))),
    }
}

fn unauthorized() -> Reply {
    let mut reply = plain(StatusCode::UNAUTHORIZED, "jeton absent ou refusé");
    reply
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    reply
}

fn plain(status: StatusCode, text: &str) -> Reply {
    let mut reply = Response::new(Full::new(Bytes::from(format!("{text}\n"))));
    *reply.status_mut() = status;
    reply.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    hardened(reply)
}

fn json_reply(status: StatusCode, body: String) -> Reply {
    let mut reply = Response::new(Full::new(Bytes::from(body)));
    *reply.status_mut() = status;
    reply.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    hardened(reply)
}

fn empty(status: StatusCode) -> Reply {
    let mut reply = Response::new(Full::new(Bytes::new()));
    *reply.status_mut() = status;
    hardened(reply)
}

/// En-têtes de toute réponse (SPEC §7.19) : rien à afficher dans un cadre, rien à deviner,
/// rien à garder en cache, rien à révéler par `Referer`.
fn hardened(mut reply: Reply) -> Reply {
    let h = reply.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    reply
}
