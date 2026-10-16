//! Une connexion d'un client : lecture des messages ligne par ligne, réponses dans
//! l'ordre, notifications des événements auxquels il s'est abonné (SPEC §6.5, §6.10).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use super::{MAX_MESSAGE, RpcError, methods, notification, queue_json, response, task_json};
use crate::scheduler::{Engine, Event};
use crate::store::tasks::TaskState;
use crate::transfer::Progress;

/// Messages en attente d'écriture vers le client ; un client lent ralentit l'envoi des
/// notifications, puis en perd (signalé par `engine.warning`).
const OUTGOING: usize = 256;

/// Sujets d'un abonnement (`events.subscribe`).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Topics {
    /// `task.added`, `task.stateChanged`.
    pub tasks: bool,
    /// `queue.updated`.
    pub queues: bool,
    /// `task.progress`, regroupé.
    pub progress: bool,
}

/// État d'une connexion, pour les méthodes.
pub(super) struct Ctx {
    pub engine: Engine,
    out: mpsc::Sender<String>,
    events: Option<JoinHandle<()>>,
    /// Fermer la connexion après la réponse (version du protocole refusée).
    pub close_after: bool,
    /// Arrêter le moteur après la réponse (`engine.shutdown`).
    pub shutdown_after: bool,
}

impl Ctx {
    pub fn start_events(&mut self, topics: Topics, interval: Duration) {
        self.stop_events();
        // Abonné ici, avant la réponse : aucun événement postérieur à la révision
        // renvoyée n'est perdu.
        let events = self.engine.subscribe();
        let (engine, out) = (self.engine.clone(), self.out.clone());
        self.events = Some(tokio::spawn(forward(engine, events, out, topics, interval)));
    }

    pub fn stop_events(&mut self) {
        if let Some(task) = self.events.take() {
            task.abort();
        }
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        self.stop_events();
    }
}

/// Sert un client sur un flux d'octets, jusqu'à ce qu'il ferme la connexion.
pub async fn serve<R, W>(engine: Engine, reader: R, writer: W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out, outgoing) = mpsc::channel(OUTGOING);
    let writing = tokio::spawn(write_all(writer, outgoing));
    let mut ctx = Ctx {
        engine,
        out: out.clone(),
        events: None,
        close_after: false,
        shutdown_after: false,
    };
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = match (&mut reader)
            .take(MAX_MESSAGE as u64 + 1)
            .read_until(b'\n', &mut line)
            .await
        {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break;
        }
        if line.len() > MAX_MESSAGE {
            let e = RpcError::invalid_request("message trop long");
            let _ = out.send(response(&Value::Null, Err(e)).to_string()).await;
            break;
        }
        if let Some(reply) = handle(&mut ctx, &line).await {
            if out.send(reply.to_string()).await.is_err() {
                break;
            }
        }
        if ctx.shutdown_after {
            ctx.shutdown_after = false;
            let engine = ctx.engine.clone();
            tokio::spawn(async move { engine.shutdown().await });
        }
        if ctx.close_after {
            break;
        }
    }
    drop(ctx);
    drop(out);
    let _ = writing.await;
}

async fn write_all<W: AsyncWrite + Unpin>(mut writer: W, mut outgoing: mpsc::Receiver<String>) {
    while let Some(message) = outgoing.recv().await {
        let mut bytes = message.into_bytes();
        bytes.push(b'\n');
        if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
            break;
        }
    }
    let _ = writer.shutdown().await;
}

/// Un message reçu : requête, notification ou lot (SPEC §6.1). Renvoie la réponse à
/// écrire, s'il y en a une.
async fn handle(ctx: &mut Ctx, line: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(line);
    if text.trim().is_empty() {
        return None;
    }
    let message: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return Some(response(&Value::Null, Err(RpcError::parse(e.to_string())))),
    };
    match message {
        Value::Array(batch) if batch.is_empty() => Some(response(
            &Value::Null,
            Err(RpcError::invalid_request("lot vide")),
        )),
        Value::Array(batch) => {
            let mut replies = Vec::new();
            for item in batch {
                if let Some(reply) = one(ctx, item).await {
                    replies.push(reply);
                }
                if ctx.close_after {
                    break;
                }
            }
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        single => one(ctx, single).await,
    }
}

/// Une requête (avec `id`) ou une notification du client (sans `id`, sans réponse).
async fn one(ctx: &mut Ctx, message: Value) -> Option<Value> {
    let Value::Object(mut fields) = message else {
        return Some(response(
            &Value::Null,
            Err(RpcError::invalid_request("objet attendu")),
        ));
    };
    let id = fields.remove("id");
    let reply_id = id.clone().unwrap_or(Value::Null);
    let valid_id = matches!(
        id,
        None | Some(Value::Null | Value::String(_) | Value::Number(_))
    );
    let method = fields
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let result = match (fields.get("jsonrpc").and_then(Value::as_str), method) {
        (Some("2.0"), Some(method)) if valid_id => {
            let params = fields.remove("params").unwrap_or(Value::Null);
            methods::call(ctx, &method, params).await
        }
        _ => Err(RpcError::invalid_request(
            "jsonrpc « 2.0 », method et id valides attendus",
        )),
    };
    // Une notification du client ne reçoit pas de réponse, même en erreur.
    id.is_some().then(|| response(&reply_id, result))
}

/// Envoie au client les événements de ses sujets, jusqu'à la fin de l'abonnement.
async fn forward(
    engine: Engine,
    mut events: broadcast::Receiver<Event>,
    out: mpsc::Sender<String>,
    topics: Topics,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut progress = ProgressBatch::default();
    loop {
        let message = tokio::select! {
            event = events.recv() => match event {
                Ok(event) => match describe(&engine, event, topics, &mut progress).await {
                    Some(message) => message,
                    None => continue,
                },
                Err(broadcast::error::RecvError::Lagged(lost)) => notification(
                    "engine.warning",
                    json!({
                        "rev": engine.rev(),
                        "code": "EVENTS_LOST",
                        "message": format!(
                            "{lost} événements perdus : client trop lent ; appeler sync.snapshot"
                        ),
                    }),
                ),
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = ticker.tick(), if topics.progress => match progress.flush(engine.rev()) {
                Some(message) => message,
                None => continue,
            },
        };
        if out.send(message.to_string()).await.is_err() {
            break;
        }
    }
}

/// Notification d'un événement, si le client y est abonné ; l'avancement est mis de côté
/// pour le prochain envoi groupé.
async fn describe(
    engine: &Engine,
    event: Event,
    topics: Topics,
    progress: &mut ProgressBatch,
) -> Option<Value> {
    match event {
        Event::Added { rev, task_id } if topics.tasks => {
            let task = engine.get(&task_id).await.ok()?;
            Some(notification(
                "task.added",
                json!({"rev": rev, "task": task_json(&task)}),
            ))
        }
        Event::StateChanged {
            rev,
            task_id,
            from,
            to,
            pause_reason,
            error_code,
        } => {
            if is_finished(to) {
                progress.forget(&task_id);
            }
            topics.tasks.then(|| {
                notification(
                    "task.stateChanged",
                    json!({
                        "rev": rev,
                        "taskId": task_id,
                        "from": from.as_sql(),
                        "to": to.as_sql(),
                        "pauseReason": pause_reason,
                        "errorCode": error_code,
                    }),
                )
            })
        }
        Event::QueueUpdated { rev, queue_id } if topics.queues => {
            let snapshot = engine.snapshot().await.ok()?;
            let queue = snapshot.queues.iter().find(|q| q.id == queue_id)?;
            Some(notification(
                "queue.updated",
                json!({"rev": rev, "queue": queue_json(queue)}),
            ))
        }
        Event::Progress {
            task_id,
            progress: p,
            ..
        } if topics.progress => {
            progress.latest.insert(task_id, p);
            None
        }
        _ => None,
    }
}

fn is_finished(state: TaskState) -> bool {
    !matches!(state, TaskState::Active | TaskState::Verifying)
}

/// Avancement regroupé par tâche entre deux envois (SPEC §6.5), et dernier relevé envoyé,
/// pour le débit.
#[derive(Default)]
struct ProgressBatch {
    latest: HashMap<String, Progress>,
    sent: HashMap<String, (u64, Instant)>,
}

impl ProgressBatch {
    fn forget(&mut self, task_id: &str) {
        self.latest.remove(task_id);
        self.sent.remove(task_id);
    }

    fn flush(&mut self, rev: u64) -> Option<Value> {
        if self.latest.is_empty() {
            return None;
        }
        let now = Instant::now();
        let mut items: Vec<Value> = Vec::with_capacity(self.latest.len());
        let mut latest: Vec<(String, Progress)> = self.latest.drain().collect();
        latest.sort_by(|a, b| a.0.cmp(&b.0));
        for (task_id, p) in latest {
            let bps = match self.sent.get(&task_id) {
                Some(&(before, at)) if now > at => {
                    let elapsed = now.duration_since(at).as_secs_f64();
                    (p.received.saturating_sub(before) as f64 / elapsed) as u64
                }
                _ => 0,
            };
            let eta = match (p.total, bps) {
                (Some(total), bps) if bps > 0 => Some(total.saturating_sub(p.received) / bps),
                _ => None,
            };
            items.push(json!({
                "taskId": task_id,
                "doneBytes": p.received,
                "totalBytes": p.total,
                "bps": bps,
                "connections": p.connections,
                "etaS": eta,
            }));
            self.sent.insert(task_id, (p.received, now));
        }
        Some(notification(
            "task.progress",
            json!({"rev": rev, "items": items}),
        ))
    }
}
