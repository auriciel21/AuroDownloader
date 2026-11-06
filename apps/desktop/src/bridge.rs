//! Liaison avec le moteur (SPEC §6) : connexion au point d'accès local, démarrage d'`aurod`
//! s'il ne répond pas (SPEC §4.6), ouverture de session et abonnement, appels de la page,
//! relais des notifications, reconnexion après une perte.
//!
//! La liaison ne connaît pas Tauri : elle envoie tout à un [`Sink`], la fenêtre de
//! l'application ou un test.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auro_engine::rpc::local::{self, Halves};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::sync::{mpsc, oneshot};

/// Avancement regroupé toutes les 500 ms (SPEC §6.5).
const PROGRESS_INTERVAL_MS: u64 = 500;
/// Attente d'`aurod` après l'avoir lancé.
const START_WAIT: Duration = Duration::from_secs(10);

/// Erreur d'un appel, telle que la page la reçoit : `code` est le `data.code` stable du
/// moteur (SPEC §6.7), ou `ENGINE_DOWN` quand le moteur n'est pas joignable.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CallError {
    pub code: String,
    pub message: String,
}

impl CallError {
    fn engine_down() -> Self {
        CallError {
            code: "ENGINE_DOWN".into(),
            message: "moteur injoignable".into(),
        }
    }

    fn from_reply(error: &Value) -> Self {
        CallError {
            code: error["data"]["code"]
                .as_str()
                .unwrap_or("INTERNAL_ERROR")
                .to_owned(),
            message: error["message"]
                .as_str()
                .unwrap_or("erreur du moteur")
                .to_owned(),
        }
    }
}

/// État de la connexion au moteur.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    /// Première tentative, ou reprise après une connexion perdue.
    Connecting,
    /// Session ouverte et abonnement actif ; `engine` : réponse d'`engine.hello`.
    Up { engine: Value },
    /// Injoignable ; nouvel essai sous peu.
    Down { message: String },
}

/// Ce que la liaison envoie à la page. Un seul canal, dans l'ordre : l'annonce d'une
/// connexion précède toujours ses notifications.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Outgoing {
    Status(Status),
    /// Notification du moteur, telle quelle.
    Message {
        message: Value,
    },
    /// De l'avancement a été omis pendant que la page était cachée : recharger l'état.
    Resync,
}

/// Destination de ce que relaie la liaison.
pub trait Sink: Send + Sync + 'static {
    fn send(&self, out: Outgoing);
    /// La page n'est pas affichée : l'avancement ne lui est pas transmis.
    fn hidden(&self) -> bool {
        false
    }
}

/// Ouvre une connexion au moteur.
pub type Connect =
    Box<dyn Fn() -> Pin<Box<dyn Future<Output = io::Result<Halves>> + Send>> + Send + Sync>;

/// Délais entre deux tentatives de connexion : doublés à chaque échec, jusqu'au plafond.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    pub first: Duration,
    pub max: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Retry {
            first: Duration::from_millis(500),
            max: Duration::from_secs(10),
        }
    }
}

/// Connexion au point d'accès local ; si personne ne répond et que `autostart` est vrai,
/// lance `aurod` (avec `db` s'il est donné) et l'attend.
pub fn local_connect(endpoint: String, db: Option<PathBuf>, autostart: bool) -> Connect {
    Box::new(move || {
        let (endpoint, db) = (endpoint.clone(), db.clone());
        Box::pin(async move {
            match local::connect(&endpoint).await {
                Ok(halves) => return Ok(halves),
                Err(e) if !autostart => return Err(e),
                Err(_) => {}
            }
            auro_engine::launch::start_daemon(&endpoint, db.as_deref(), None)?;
            let deadline = tokio::time::Instant::now() + START_WAIT;
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                match local::connect(&endpoint).await {
                    Ok(halves) => return Ok(halves),
                    Err(e) if tokio::time::Instant::now() >= deadline => {
                        return Err(io::Error::new(
                            e.kind(),
                            format!("aurod lancé, mais ne répond pas : {e}"),
                        ));
                    }
                    Err(_) => {}
                }
            }
        })
    })
}

type Reply = Result<Value, CallError>;

/// Liaison avec le moteur, partagée entre la boucle de connexion et les appels de la page.
pub struct Bridge {
    /// Messages à écrire sur la connexion en cours ; `None` sans connexion.
    out: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// Appels en attente de réponse, par identifiant.
    waiting: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    next_id: AtomicU64,
    status: Mutex<Status>,
    /// De l'avancement n'a pas été transmis pendant que la page était cachée.
    missed: AtomicBool,
}

impl Bridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Bridge {
            out: Mutex::new(None),
            waiting: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            status: Mutex::new(Status::Connecting),
            missed: AtomicBool::new(false),
        })
    }

    /// État courant de la connexion, pour une page qui vient de s'abonner.
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Vrai une seule fois après de l'avancement omis : la page doit recharger l'état.
    pub fn take_missed(&self) -> bool {
        self.missed.swap(false, Ordering::SeqCst)
    }

    /// Appelle `method` sur la connexion en cours.
    pub async fn call(&self, method: &str, params: Value) -> Reply {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        // Inscrit avant de prendre la connexion : une connexion qui se ferme ensuite
        // vide `waiting`, et l'appel échoue au lieu d'attendre sans fin.
        self.waiting.lock().unwrap().insert(id, tx);
        let out = self.out.lock().unwrap().clone();
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let sent = out.is_some_and(|out| out.send(request.to_string()).is_ok());
        if !sent {
            self.waiting.lock().unwrap().remove(&id);
            return Err(CallError::engine_down());
        }
        rx.await.unwrap_or_else(|_| Err(CallError::engine_down()))
    }

    /// Boucle de connexion : ne rend jamais la main.
    pub async fn run(self: Arc<Self>, connect: Connect, sink: Arc<dyn Sink>, retry: Retry) {
        let mut delay = retry.first;
        loop {
            if !matches!(self.status(), Status::Down { .. }) {
                self.set_status(&*sink, Status::Connecting);
            }
            let message = match connect().await {
                Ok(halves) => {
                    delay = retry.first;
                    match self.session(halves, &*sink).await {
                        Ok(()) => "connexion au moteur perdue".to_owned(),
                        Err(e) => e.to_string(),
                    }
                }
                Err(e) => e.to_string(),
            };
            self.set_status(&*sink, Status::Down { message });
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(retry.max);
        }
    }

    fn set_status(&self, sink: &dyn Sink, status: Status) {
        *self.status.lock().unwrap() = status.clone();
        sink.send(Outgoing::Status(status));
    }

    /// Une connexion : session, abonnement, puis relais jusqu'à sa fermeture.
    async fn session(&self, (read, write): Halves, sink: &dyn Sink) -> io::Result<()> {
        let mut lines = BufReader::new(read).lines();
        let mut write = write;
        // Notifications arrivées pendant l'ouverture : relayées après l'annonce.
        let mut early = Vec::new();
        let hello = json!({
            "client": {"name": "auro-desktop", "version": env!("CARGO_PKG_VERSION")},
            "protocol": {"min": "1.0", "max": "1.0"},
        });
        let engine = self
            .exchange(&mut lines, &mut write, "engine.hello", hello, &mut early)
            .await?
            .map_err(|e| io::Error::other(format!("{} [{}]", e.message, e.code)))?;
        let topics = json!({
            "topics": ["tasks", "queues", "progress"],
            "progressIntervalMs": PROGRESS_INTERVAL_MS,
        });
        self.exchange(
            &mut lines,
            &mut write,
            "events.subscribe",
            topics,
            &mut early,
        )
        .await?
        .map_err(|e| io::Error::other(format!("{} [{}]", e.message, e.code)))?;

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let writer = tokio::spawn(async move {
            while let Some(mut line) = rx.recv().await {
                line.push('\n');
                if write.write_all(line.as_bytes()).await.is_err() || write.flush().await.is_err() {
                    break;
                }
            }
        });
        *self.out.lock().unwrap() = Some(tx);
        self.set_status(sink, Status::Up { engine });
        for message in early {
            self.relay(sink, message);
        }
        let result = loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    match message.get("id").and_then(Value::as_u64) {
                        Some(id) => self.answer(id, &message),
                        None => self.relay(sink, message),
                    }
                }
                Ok(None) => break Ok(()),
                Err(e) => break Err(e),
            }
        };
        *self.out.lock().unwrap() = None;
        // Les appels en attente reçoivent `ENGINE_DOWN`.
        self.waiting.lock().unwrap().clear();
        writer.abort();
        result
    }

    /// Requête de l'ouverture de session, avant que la page ne puisse appeler.
    async fn exchange<R, W>(
        &self,
        lines: &mut Lines<BufReader<R>>,
        write: &mut W,
        method: &str,
        params: Value,
        early: &mut Vec<Value>,
    ) -> io::Result<Reply>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut bytes = request.to_string().into_bytes();
        bytes.push(b'\n');
        write.write_all(&bytes).await?;
        write.flush().await?;
        loop {
            let line = lines.next_line().await?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "le moteur a fermé la connexion",
                )
            })?;
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                if message.get("method").is_some() {
                    early.push(message);
                }
                continue;
            }
            return Ok(match message.get("error") {
                Some(error) => Err(CallError::from_reply(error)),
                None => Ok(message["result"].clone()),
            });
        }
    }

    fn answer(&self, id: u64, message: &Value) {
        let Some(tx) = self.waiting.lock().unwrap().remove(&id) else {
            return;
        };
        let reply = match message.get("error") {
            Some(error) => Err(CallError::from_reply(error)),
            None => Ok(message["result"].clone()),
        };
        let _ = tx.send(reply);
    }

    fn relay(&self, sink: &dyn Sink, message: Value) {
        if message["method"] == "task.progress" && sink.hidden() {
            self.missed.store(true, Ordering::SeqCst);
            return;
        }
        sink.send(Outgoing::Message { message });
    }
}

#[cfg(test)]
mod tests;
