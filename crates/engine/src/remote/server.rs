//! Écoute HTTP et WebSocket de l'interface distante (SPEC §7.19).
//!
//! Routes : `GET /ws` (WebSocket, un message JSON-RPC par trame texte), `POST /rpc` (un
//! message, une réponse, sans notifications), `POST /login` (jeton échangé contre un cookie
//! de session), `POST /logout`. La page elle-même viendra à l'étape 3c. Hors de la boucle
//! locale, en TLS (`rustls`, certificat de `remote::tls`), négociation comprise dans le
//! délai des en-têtes.
//!
//! Avant toute route : `Host` doit nommer ce serveur ([`HostRule`], contre le DNS
//! rebinding) ; une
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
use tokio::net::TcpListener;
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
/// WebSocket ouverts à la fois. Une connexion mise à niveau ne compte plus parmi les
/// connexions HTTP (hyper rend sa place à la mise à niveau) : elle a sa propre limite
/// (audit D4) ; au-delà, `503`.
pub(crate) const MAX_WEBSOCKETS: usize = 64;
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
    /// Valeurs de `Host` acceptées.
    hosts: HostRule,
    /// `https` ou `http`.
    scheme: &'static str,
    failures: Mutex<HashMap<IpAddr, Failures>>,
    /// Change (se ferme) quand l'écoute s'arrête : les WebSocket en cours se ferment avec
    /// elle.
    closed: watch::Receiver<()>,
    /// Délai de la négociation TLS, des en-têtes et du corps de `/login` ([`HEADER_TIMEOUT`],
    /// plus court dans les tests).
    timeout: Duration,
    /// Places des WebSocket ([`MAX_WEBSOCKETS`]), gardées par la tâche de chacun.
    websockets: Arc<Semaphore>,
}

#[derive(Debug, Clone, Copy)]
struct Failures {
    since: Instant,
    count: u32,
    blocked_until: Option<Instant>,
}

/// Une écoute à servir : son certificat (TLS) et les noms qui la désignent.
pub struct Listening {
    /// `None` : en clair (boucle locale seulement).
    pub tls: Option<Arc<rustls::ServerConfig>>,
    /// Noms acceptés dans `Host`, en minuscules (`remote::tls::known_names`).
    pub names: Vec<String>,
}

/// Valeurs de `Host` qui désignent ce serveur (SPEC §7.19), toujours avec son port (sans
/// lui sur le port par défaut du protocole) : toute adresse IP écrite en chiffres, que le
/// DNS rebinding ne peut pas imiter (il passe par un nom), et les noms connus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRule {
    port: u16,
    default_port: u16,
    names: Vec<String>,
}

impl HostRule {
    pub fn new(port: u16, tls: bool, names: Vec<String>) -> Self {
        HostRule {
            port,
            default_port: if tls { 443 } else { 80 },
            names: names.into_iter().map(|n| n.to_ascii_lowercase()).collect(),
        }
    }

    /// `host` (valeur de l'en-tête) désigne-t-il ce serveur ?
    pub fn accepts(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let (name, port, ip) = if let Some(rest) = host.strip_prefix('[') {
            let Some((v6, after)) = rest.split_once(']') else {
                return false;
            };
            let port = match after.strip_prefix(':') {
                Some(port) => Some(port),
                None if after.is_empty() => None,
                None => return false,
            };
            (v6, port, v6.parse::<std::net::Ipv6Addr>().is_ok())
        } else {
            let (name, port) = match host.split_once(':') {
                Some((name, port)) => (name, Some(port)),
                None => (host.as_str(), None),
            };
            (name, port, name.parse::<std::net::Ipv4Addr>().is_ok())
        };
        let port = match port {
            // Chiffres seulement : ni signe, ni espace, ni autre `:`.
            Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => p.parse().ok(),
            Some(_) => None,
            None => Some(self.default_port),
        };
        if port != Some(self.port) {
            return false;
        }
        ip || self.names.iter().any(|n| n == name.trim_end_matches('.'))
    }
}

/// Sert l'interface distante sur `listener` jusqu'à ce que la tâche soit arrêtée ; les
/// connexions en cours s'arrêtent avec elle.
pub async fn serve(
    engine: Engine,
    listener: Arc<TcpListener>,
    listening: Listening,
) -> std::io::Result<()> {
    serve_timed(engine, listener, listening, HEADER_TIMEOUT).await
}

/// [`serve`], avec un autre délai des clients lents ou muets (tests).
pub(super) async fn serve_timed(
    engine: Engine,
    listener: Arc<TcpListener>,
    listening: Listening,
    timeout: Duration,
) -> std::io::Result<()> {
    let addr = listener.local_addr()?;
    let tls = listening.tls.map(tokio_rustls::TlsAcceptor::from);
    // Gardé jusqu'à l'arrêt de l'écoute : sa disparition ferme les WebSocket.
    let (_closing, closed) = watch::channel(());
    let shared = Arc::new(Shared {
        engine,
        hosts: HostRule::new(addr.port(), tls.is_some(), listening.names),
        scheme: if tls.is_some() { "https" } else { "http" },
        failures: Mutex::default(),
        closed,
        timeout,
        websockets: Arc::new(Semaphore::new(MAX_WEBSOCKETS)),
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
                let tls = tls.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    let _ = stream.set_nodelay(true);
                    match tls {
                        // La négociation, comme les en-têtes, dans le délai : un client
                        // muet ne garde pas sa place.
                        Some(acceptor) => {
                            if let Ok(Ok(stream)) =
                                tokio::time::timeout(shared.timeout, acceptor.accept(stream)).await
                            {
                                serve_connection(shared, stream, peer).await;
                            }
                        }
                        None => serve_connection(shared, stream, peer).await,
                    }
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn serve_connection<S>(shared: Arc<Shared>, stream: S, peer: SocketAddr)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let timeout = shared.timeout;
    let service = hyper::service::service_fn(move |request| {
        let shared = Arc::clone(&shared);
        async move { Ok::<_, Infallible>(route(shared, peer.ip(), request).await) }
    });
    let mut http = hyper::server::conn::http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(timeout)
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
        (&Method::GET, "/session") => session(shared, ip, request).await,
        (_, "/ws" | "/rpc" | "/login" | "/logout" | "/session") => plain(
            StatusCode::METHOD_NOT_ALLOWED,
            "méthode non prise en charge",
        ),
        (&Method::GET, path) => match super::page::find(path) {
            Some(asset) => page_file(&shared, asset, &host),
            None => plain(StatusCode::NOT_FOUND, "introuvable"),
        },
        _ => plain(StatusCode::NOT_FOUND, "introuvable"),
    }
}

/// Un fichier de la page (étape 3c), sans authentification ; la page porte sa politique de
/// sécurité.
fn page_file(shared: &Shared, asset: &super::page::Asset, host: &str) -> Reply {
    let mut reply = Response::new(Full::new(Bytes::from_static(asset.body)));
    reply.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(asset.content_type),
    );
    let mut reply = hardened(reply);
    if asset.path == "/index.html"
        && let Ok(csp) = HeaderValue::from_str(&super::page::csp(host, shared.scheme == "https"))
    {
        reply
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    reply
}

/// `GET /session` : la page demande si sa session (cookie) est encore valable, avant
/// d'ouvrir le WebSocket ou de montrer la connexion. Un navigateur n'envoie pas `Origin`
/// pour un `GET` de même origine : le cookie `SameSite=Strict` ne part de toute façon pas
/// d'un autre site, et la réponse ne se lit pas d'ailleurs (aucun CORS).
async fn session(
    shared: Arc<Shared>,
    ip: IpAddr,
    request: Request<hyper::body::Incoming>,
) -> Reply {
    match authenticate(&shared, ip, request.headers(), true).await {
        Ok((row, _)) => json_reply(
            StatusCode::OK,
            json!({"name": row.name, "scope": row.scope}).to_string(),
        ),
        Err(reply) => *reply,
    }
}

/// `Host` de la requête s'il désigne ce serveur, en minuscules.
fn checked_host(shared: &Shared, headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get(header::HOST)?
        .to_str()
        .ok()?
        .to_ascii_lowercase();
    shared.hosts.accepts(&host).then_some(host)
}

/// `Ok(true)` : `Origin` présent et de même origine ; `Ok(false)` : absent ; `Err` : une
/// autre origine (ou `null`, ou plusieurs).
fn checked_origin(shared: &Shared, headers: &HeaderMap, host: &str) -> Result<bool, ()> {
    origin_matches(shared.scheme, headers, host)
}

/// Lecteur de `Origin` de [`checked_origin`], pour `scheme://host` (fuzzing).
pub(super) fn origin_matches(scheme: &str, headers: &HeaderMap, host: &str) -> Result<bool, ()> {
    let mut values = headers.get_all(header::ORIGIN).iter();
    let Some(origin) = values.next() else {
        return Ok(false);
    };
    if values.next().is_some() {
        return Err(());
    }
    let origin = origin.to_str().map_err(|_| ())?.to_ascii_lowercase();
    if origin == format!("{scheme}://{host}") {
        Ok(true)
    } else {
        Err(())
    }
}

pub(super) fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

pub(super) fn session_cookie(headers: &HeaderMap) -> Option<&str> {
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
    /// Attribut `Secure` du cookie en TLS : jamais renvoyé en clair.
    fn secure_flag(&self) -> &'static str {
        if self.scheme == "https" {
            "; Secure"
        } else {
            ""
        }
    }

    fn blocked(&self, ip: IpAddr) -> bool {
        let failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        is_blocked(&failures, ip, Instant::now())
    }

    fn failed(&self, ip: IpAddr) {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        note_failure(&mut failures, ip, Instant::now());
    }
}

fn is_blocked(failures: &HashMap<IpAddr, Failures>, ip: IpAddr, now: Instant) -> bool {
    failures
        .get(&network_of(ip))
        .and_then(|f| f.blocked_until)
        .is_some_and(|until| now < until)
}

fn note_failure(failures: &mut HashMap<IpAddr, Failures>, ip: IpAddr, now: Instant) {
    // La table ne grossit pas : au-delà de sa taille, les adresses oubliées depuis
    // longtemps partent, puis, s'il le faut, toutes celles qui ne sont pas bloquées. Le
    // ménage vide alors une bonne part de la table : il ne revient pas à chaque échec
    // (audit D6).
    if failures.len() > MAX_TRACKED {
        failures.retain(|_, f| now.duration_since(f.since) < PENALTY * 2);
        if failures.len() > MAX_TRACKED / 2 {
            failures.retain(|_, f| f.blocked_until.is_some_and(|until| now < until));
        }
    }
    let entry = failures.entry(network_of(ip)).or_insert(Failures {
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

/// Adresses suivies dans la table des échecs, au plus.
const MAX_TRACKED: usize = 4096;

/// Délai d'un envoi vers un WebSocket (audit D7).
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Ce que la table des échecs compte : une adresse IPv4, un réseau /64 en IPv6 (un poste y
/// change d'adresse à volonté, audit D6).
fn network_of(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
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
    let Ok(place) = Arc::clone(&shared.websockets).try_acquire_owned() else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "trop de connexions");
    };
    let accept = derive_accept_key(key.as_bytes());
    let engine = shared.engine.clone();
    let closed = shared.closed.clone();
    tokio::spawn(async move {
        // La place se rend à la fin du WebSocket, pas à la mise à niveau.
        let _place = place;
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
    // Un client qui ne lit plus ne garde pas la connexion : chaque envoi a son délai, et sa
    // fin ferme le service des messages (leur file se ferme avec l'écriture, audit D7).
    let mut writing = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            match tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::text(message))).await {
                Ok(Ok(())) => {}
                _ => break,
            }
        }
        let _ = tokio::time::timeout(WRITE_TIMEOUT, sink.close()).await;
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
    if tokio::time::timeout(WRITE_TIMEOUT, &mut writing)
        .await
        .is_err()
    {
        writing.abort();
    }
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
    let body = match read_body(request, MAX_LOGIN, shared.timeout).await {
        Ok(body) => body,
        Err(reply) => return *reply,
    };
    let Some(token) = login_token(&body) else {
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
    // Sans TLS, cookie en clair envoyé à tout port de l'hôte : session courte (D13).
    let ttl_ms = super::session_ttl_ms(shared.scheme == "https");
    let Ok(cookie) = shared.engine.open_session(&row.id, ttl_ms).await else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "moteur indisponible");
    };
    let mut reply = json_reply(
        StatusCode::OK,
        json!({"name": row.name, "scope": row.scope}).to_string(),
    );
    let max_age = ttl_ms / 1000;
    let value = format!(
        "{COOKIE}={cookie}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        shared.secure_flag()
    );
    if let Ok(value) = HeaderValue::from_str(&value) {
        reply.headers_mut().insert(header::SET_COOKIE, value);
    }
    reply
}

/// Le jeton du corps de `POST /login` (`{"token": "…"}`), sans les espaces autour.
pub(super) fn login_token(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    Some(value.get("token")?.as_str()?.trim().to_owned())
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
    let value = format!(
        "{COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
        shared.secure_flag()
    );
    if let Ok(value) = HeaderValue::from_str(&value) {
        reply.headers_mut().insert(header::SET_COOKIE, value);
    }
    reply
}

pub(super) fn is_json(headers: &HeaderMap) -> bool {
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

#[cfg(test)]
mod failure_table {
    use super::*;

    /// Audit D6 : en IPv6, un réseau /64 compte pour un, quelle que soit l'adresse prise
    /// à chaque essai ; et la table reste sous sa taille, même face à des milliers
    /// d'adresses, sans oublier une adresse bloquée.
    #[test]
    fn a_64_network_is_one_offender_and_the_table_stays_bounded() {
        let now = Instant::now();
        let mut table = HashMap::new();
        for i in 0..=MAX_FAILURES {
            let ip: IpAddr = format!("2001:db8:1:2::{:x}", i + 1).parse().unwrap();
            note_failure(&mut table, ip, now);
        }
        let other_in_64: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        assert!(is_blocked(&table, other_in_64, now), "le /64 est bloqué");
        let neighbour: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert!(!is_blocked(&table, neighbour, now));
        let mapped: IpAddr = "::ffff:192.0.2.1".parse().unwrap();
        assert_eq!(network_of(mapped), "192.0.2.1".parse::<IpAddr>().unwrap());

        for i in 0..(MAX_TRACKED as u32 * 3) {
            let ip = IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i));
            note_failure(&mut table, ip, now);
            assert!(table.len() <= MAX_TRACKED + 1, "{}", table.len());
        }
        assert!(is_blocked(&table, other_in_64, now), "le bloqué reste");
    }
}
