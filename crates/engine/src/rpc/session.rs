//! Une connexion d'un client : lecture des messages ligne par ligne, réponses dans
//! l'ordre, notifications des événements auxquels il s'est abonné (SPEC §6.5, §6.10).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use super::{
    MAX_MESSAGE, RpcError, Scope, checksum_json, end_action_json, methods, notification,
    queue_json, response, schedule_json, task_json,
};
use crate::scheduler::{Engine, Event};
use crate::store::settings::DOWNLOAD_DIR;
use crate::store::tasks::TaskState;
use crate::transfer::Progress;

/// Messages en attente d'écriture vers le client ; un client lent ralentit l'envoi des
/// notifications, puis en perd (signalé par `engine.warning`).
const OUTGOING: usize = 256;

/// Sujets d'un abonnement (`events.subscribe`).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Topics {
    /// `task.added`, `task.stateChanged`, `task.updated`, `task.removed`.
    pub tasks: bool,
    /// `queue.updated`, `limits.updated`, `settings.updated`, `schedule.updated`,
    /// `schedule.removed`, `engine.endAction`.
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
    /// Ce que la connexion peut demander (SPEC §6.9).
    pub scope: Scope,
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
        // Canal local, réservé à l'utilisateur par le système : tous les droits, sauf
        // ceux que le client abandonne à l'ouverture.
        scope: Scope::Admin,
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
        Event::TaskUpdated {
            rev,
            task_id,
            update,
        } if topics.tasks => {
            // Seulement les champs modifiés (SPEC §6.5).
            let mut params = json!({"rev": rev, "taskId": task_id});
            if let Some(limit) = update.speed_limit_bps {
                params["speedLimitBps"] = json!(limit);
            }
            if let Some(checksum) = &update.checksum {
                params["checksum"] = checksum_json(checksum.as_ref(), None);
            }
            Some(notification("task.updated", params))
        }
        Event::Prepared {
            rev,
            task_id,
            file_name,
            total_bytes,
            found_checksum,
        } if topics.tasks => {
            // Le nom du fichier sert aussi de nom affiché depuis la préparation.
            let mut params = json!({
                "rev": rev,
                "taskId": task_id,
                "fileName": file_name,
                "displayName": file_name,
                "totalBytes": total_bytes,
            });
            if let Some((checksum, source)) = &found_checksum {
                params["checksum"] = checksum_json(Some(checksum), Some(source));
            }
            Some(notification("task.updated", params))
        }
        Event::Restarted {
            rev,
            task_id,
            checksum,
        } => {
            progress.forget(&task_id);
            topics.tasks.then(|| {
                notification(
                    "task.updated",
                    json!({
                        "rev": rev,
                        "taskId": task_id,
                        "fileName": null,
                        "totalBytes": null,
                        "doneBytes": 0,
                        "errorCode": null,
                        "errorDetail": null,
                        "checksum": checksum_json(checksum.as_ref(), None),
                    }),
                )
            })
        }
        Event::Relocated {
            rev,
            task_id,
            dest_dir,
            file_name,
            display_name,
            moving,
            error,
        } if topics.tasks => {
            let mut params = json!({
                "rev": rev,
                "taskId": task_id,
                "destDir": dest_dir,
                "fileName": file_name,
                "displayName": display_name,
                "moving": moving,
            });
            // Échec d'une copie vers un autre volume : la tâche est restée à sa place.
            if let Some(error) = error {
                params["moveError"] = json!(error);
            }
            Some(notification("task.updated", params))
        }
        Event::SourceChanged {
            rev,
            task_id,
            display_name,
        } if topics.tasks => Some(notification(
            "task.updated",
            json!({"rev": rev, "taskId": task_id, "displayName": display_name}),
        )),
        Event::Removed { rev, task_id } if topics.tasks => Some(notification(
            "task.removed",
            json!({"rev": rev, "taskId": task_id}),
        )),
        Event::LimitsChanged {
            rev,
            download_bps,
            upload_bps,
        } if topics.queues => Some(notification(
            "limits.updated",
            json!({"rev": rev, "downloadBps": download_bps, "uploadBps": upload_bps}),
        )),
        Event::SettingChanged { rev, key, value } if topics.queues => Some(notification(
            "settings.updated",
            json!({"rev": rev, "values": {key: value}}),
        )),
        Event::FlagChanged { rev, flag, on } if topics.queues => Some(notification(
            "settings.updated",
            json!({"rev": rev, "values": {flag.key(): on}}),
        )),
        Event::NetworkChanged {
            rev,
            usable,
            metered,
        } if topics.queues => Some(notification(
            "engine.network",
            json!({"rev": rev, "usable": usable, "metered": metered}),
        )),
        // Outils de la vidéo (SPEC §7.17) : installation, mise à jour.
        Event::ToolsChanged { rev, status } if topics.queues => {
            let mut params = super::methods::tools_json(&status);
            params["rev"] = json!(rev);
            Some(notification("media.tools", params))
        }
        // Interface réseau des torrents (SPEC §7.16) là ou non.
        Event::TorrentInterfaceChanged { rev, up } if topics.queues => Some(notification(
            "engine.torrentInterface",
            json!({"rev": rev, "up": up}),
        )),
        Event::DownloadDirChanged { rev, download_dir } if topics.queues => Some(notification(
            "settings.updated",
            json!({"rev": rev, "values": {DOWNLOAD_DIR: download_dir}}),
        )),
        Event::QueueUpdated { rev, queue_id } if topics.queues => {
            let queues = engine.queues().await.ok()?;
            let queue = queues.iter().find(|q| q.id == queue_id)?;
            Some(notification(
                "queue.updated",
                json!({"rev": rev, "queue": queue_json(queue)}),
            ))
        }
        Event::ScheduleChanged { rev, view } if topics.queues => Some(notification(
            "schedule.updated",
            json!({"rev": rev, "schedule": schedule_json(&view)}),
        )),
        Event::ScheduleRemoved { rev, schedule_id } if topics.queues => Some(notification(
            "schedule.removed",
            json!({"rev": rev, "scheduleId": schedule_id}),
        )),
        Event::EndAction {
            rev,
            pending,
            error,
        } if topics.queues => Some(notification(
            "engine.endAction",
            json!({"rev": rev, "pending": end_action_json(pending.as_ref()), "error": error}),
        )),
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

/// Durée sur laquelle se mesure le débit annoncé. Les octets arrivent par à-coups (lectures
/// de plusieurs centaines de Kio, seaux des limites de débit) : mesuré d'un envoi à
/// l'autre, le débit alternait entre zéro et le double du vrai.
const RATE_WINDOW: Duration = Duration::from_secs(6);

/// Débit d'une tâche, mesuré entre les instants où ses octets ont augmenté : sur des
/// arrivées par blocs réguliers, la mesure tombe juste, quel que soit le rapport entre les
/// blocs et la fenêtre.
#[derive(Default)]
struct Rate {
    /// Le relevé de départ, puis chaque relevé où les octets ont augmenté, sur
    /// `RATE_WINDOW` au plus (le plus ancien est gardé comme point de départ).
    changes: VecDeque<(Instant, u64)>,
}

impl Rate {
    fn measure(&mut self, now: Instant, received: u64) -> u64 {
        match self.changes.back() {
            // Moins d'octets qu'avant : la tâche repart (fichier distant changé).
            Some(&(_, last)) if received < last => {
                self.changes.clear();
                self.changes.push_back((now, received));
            }
            Some(&(_, last)) if received == last => {}
            _ => self.changes.push_back((now, received)),
        }
        while self.changes.len() > 2 && now.duration_since(self.changes[1].0) >= RATE_WINDOW {
            self.changes.pop_front();
        }
        let (Some(&(first_at, first)), Some(&(last_at, last))) =
            (self.changes.front(), self.changes.back())
        else {
            return 0;
        };
        let intervals = self.changes.len() - 1;
        // Rien de nouveau depuis toute une fenêtre : plus de débit.
        if intervals == 0 || now.duration_since(last_at) >= RATE_WINDOW {
            return 0;
        }
        // Tant que le prochain bloc n'est pas en retard sur l'écart moyen, la mesure va
        // du premier au dernier bloc ; ensuite, elle décroît avec l'attente.
        let span = last_at.duration_since(first_at);
        let gap = span / intervals as u32;
        let waited = now.duration_since(first_at).saturating_sub(gap);
        let elapsed = span.max(waited).as_secs_f64();
        if elapsed <= 0.0 {
            return 0;
        }
        ((last - first) as f64 / elapsed) as u64
    }
}

/// Avancement regroupé par tâche entre deux envois (SPEC §6.5), et débit de chacune.
#[derive(Default)]
struct ProgressBatch {
    latest: HashMap<String, Progress>,
    sent: HashMap<String, Rate>,
}

impl ProgressBatch {
    fn forget(&mut self, task_id: &str) {
        self.latest.remove(task_id);
        self.sent.remove(task_id);
    }

    fn flush(&mut self, rev: u64) -> Option<Value> {
        self.flush_at(rev, Instant::now())
    }

    fn flush_at(&mut self, rev: u64, now: Instant) -> Option<Value> {
        if self.latest.is_empty() {
            return None;
        }
        let mut items: Vec<Value> = Vec::with_capacity(self.latest.len());
        let mut latest: Vec<(String, Progress)> = self.latest.drain().collect();
        latest.sort_by(|a, b| a.0.cmp(&b.0));
        for (task_id, p) in latest {
            let bps = self
                .sent
                .entry(task_id.clone())
                .or_default()
                .measure(now, p.received);
            let eta = match (p.total, bps) {
                (Some(total), bps) if bps > 0 => Some(total.saturating_sub(p.received) / bps),
                _ => None,
            };
            let mut item = json!({
                "taskId": task_id,
                "doneBytes": p.received,
                "totalBytes": p.total,
                "bps": bps,
                "connections": p.connections,
                "etaS": eta,
            });
            // Calcul de l'empreinte en cours (`verifying`, F-11) : octets du fichier lus.
            if let Some(verified) = p.verified {
                item["verifiedBytes"] = json!(verified);
            }
            // Torrent (SPEC §7.16) : octets envoyés aux autres pairs, et le débit d'envoi.
            if let Some(uploaded) = p.uploaded {
                item["uploadedBytes"] = json!(uploaded);
                item["uploadBps"] = json!(p.upload_bps.unwrap_or(0));
            }
            items.push(item);
        }
        Some(notification(
            "task.progress",
            json!({"rev": rev, "items": items}),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(received: u64) -> Progress {
        Progress {
            received,
            total: Some(100 << 20),
            connections: 1,
            segments: std::sync::Arc::from([]),
            verified: None,
            uploaded: None,
            upload_bps: None,
            seeding: false,
        }
    }

    /// Débit annoncé pour `a` au relevé fait `ms` millisecondes après `t0`.
    fn rate(batch: &mut ProgressBatch, t0: Instant, ms: u64, received: u64) -> u64 {
        batch.latest.insert("a".into(), progress(received));
        let message = batch.flush_at(1, t0 + Duration::from_millis(ms)).unwrap();
        message["params"]["items"][0]["bps"].as_u64().unwrap()
    }

    /// Blocs de `chunk` octets arrivant toutes les `period_ms`, relevés toutes les 500 ms ;
    /// renvoie le débit annoncé à chaque relevé.
    fn bursts(chunk: u64, period_ms: u64, steps: u64) -> Vec<u64> {
        let t0 = Instant::now();
        let mut batch = ProgressBatch::default();
        (0..=steps)
            .map(|step| {
                let ms = step * 500;
                rate(&mut batch, t0, ms, (ms / period_ms) * chunk)
            })
            .collect()
    }

    fn assert_near(rates: &[u64], truth: f64, tolerance: f64) {
        for &bps in rates {
            let gap = (bps as f64 - truth).abs() / truth;
            assert!(gap < tolerance, "{bps} o/s pour {truth} dans {rates:?}");
        }
    }

    #[test]
    fn bursty_arrivals_give_a_steady_rate() {
        // 512 Kio par seconde, arrivés d'un bloc une fois sur deux envois.
        let rates = bursts(512 << 10, 1000, 30);
        assert_eq!(rates[0], 0, "un seul relevé : pas encore de débit");
        assert_near(&rates[2..], 524_288.0, 0.02);
        // Blocs qui ne tombent pas sur les envois : 512 Kio toutes les 1,7 s (une limite
        // de 300 Kio/s). Jamais zéro, jamais le double : à quelques pour cent du vrai.
        let rates = bursts(512 << 10, 1700, 40);
        assert_near(&rates[8..], 524_288.0 / 1.7, 0.08);
    }

    #[test]
    fn the_rate_decays_after_a_stop_and_restarts_with_the_task() {
        let t0 = Instant::now();
        let mut batch = ProgressBatch::default();
        let mut received = 0;
        for step in 0..=10u64 {
            received += 100_000;
            rate(&mut batch, t0, step * 500, received);
        }
        // Arrêt net à 5 s : le débit décroît, puis tombe à zéro une fois la fenêtre passée.
        let after: Vec<u64> = (11..=24u64)
            .map(|step| rate(&mut batch, t0, step * 500, received))
            .collect();
        assert!(after.windows(2).all(|w| w[1] <= w[0]), "{after:?}");
        assert_eq!(*after.last().unwrap(), 0, "{after:?}");
        // Moins d'octets qu'avant : mesure recommencée, sans débit négatif ni énorme.
        assert_eq!(rate(&mut batch, t0, 12_500, 1_000), 0);
        assert_eq!(rate(&mut batch, t0, 13_000, 51_000), 100_000);
        batch.forget("a");
        assert!(batch.sent.is_empty());
    }

    #[test]
    fn the_check_of_the_checksum_reports_the_bytes_read() {
        let mut batch = ProgressBatch::default();
        batch.latest.insert("a".into(), progress(10));
        let message = batch.flush_at(1, Instant::now()).unwrap();
        let item = &message["params"]["items"][0];
        assert!(item.get("verifiedBytes").is_none(), "{item}");
        let checking = Progress {
            verified: Some(4 << 20),
            ..progress(100 << 20)
        };
        batch.latest.insert("a".into(), checking);
        let message = batch.flush_at(2, Instant::now()).unwrap();
        let item = &message["params"]["items"][0];
        assert_eq!(item["verifiedBytes"], 4 << 20);
        assert_eq!(item["doneBytes"], 100 << 20);
    }
}
