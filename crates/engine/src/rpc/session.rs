//! Une connexion d'un client : lecture des messages ligne par ligne, réponses dans
//! l'ordre, notifications des événements auxquels il s'est abonné (SPEC §6.5, §6.10).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use super::{
    MAX_MESSAGE, RpcError, Scope, checksum_json, end_action_json, methods, notification,
    queue_json, response, schedule_json, task_json,
};
use crate::linkscan::crawl;
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

/// Avancement d'une exploration annoncé au plus une fois par cet intervalle (SPEC
/// §7.18 bis).
const CRAWL_NOTES: Duration = Duration::from_millis(500);

/// État d'une connexion, pour les méthodes.
pub(super) struct Ctx {
    pub engine: Engine,
    out: mpsc::Sender<String>,
    events: Option<JoinHandle<()>>,
    /// Exploration d'un site de cette connexion (SPEC §7.18 bis), et l'annonce de son
    /// avancement : la fermeture de la connexion les arrête.
    crawl: Option<crawl::Handle>,
    crawl_notes: Option<JoinHandle<()>>,
    /// Une seule requête, puis la connexion se ferme (`POST /rpc`).
    pub one_shot: bool,
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

    /// L'exploration de cette connexion, s'il y en a une.
    pub fn crawl(&self) -> Option<&crawl::Handle> {
        self.crawl.as_ref()
    }

    /// Garde l'exploration lancée, et annonce son avancement à cette connexion seule.
    pub fn keep_crawl(&mut self, handle: crawl::Handle) {
        self.forget_crawl();
        let notes = crawl_notes(
            self.engine.clone(),
            self.out.clone(),
            Arc::clone(handle.shared()),
            handle.id.clone(),
        );
        self.crawl_notes = Some(tokio::spawn(notes));
        self.crawl = Some(handle);
    }

    /// Arrête et oublie l'exploration de cette connexion.
    pub fn forget_crawl(&mut self) {
        if let Some(task) = self.crawl_notes.take() {
            task.abort();
        }
        self.crawl = None;
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        self.stop_events();
        self.forget_crawl();
    }
}

/// Annonce l'avancement d'une exploration (`linkscan.progress`) : tout de suite, puis à
/// chaque changement, au plus une fois par [`CRAWL_NOTES`], et une dernière fois à la fin.
async fn crawl_notes(
    engine: Engine,
    out: mpsc::Sender<String>,
    shared: Arc<crawl::Shared>,
    id: String,
) {
    let mut changed = shared.watch();
    loop {
        let progress = shared.progress();
        let mut params = methods::crawl_progress_json(&id, &progress);
        params["rev"] = json!(engine.rev());
        let note = notification("linkscan.progress", params);
        if out.send(note.to_string()).await.is_err() || progress.state != crawl::State::Running {
            return;
        }
        tokio::time::sleep(CRAWL_NOTES).await;
        if changed.changed().await.is_err() {
            return;
        }
    }
}

/// Qui est à l'autre bout d'une connexion.
#[derive(Debug, Clone)]
pub struct Peer {
    /// Portée de départ ; le client peut la réduire à l'ouverture (`engine.hello`).
    pub scope: Scope,
    /// Jeton de l'interface distante (SPEC §7.19) ; `None` : canal local.
    pub token_id: Option<String>,
    /// Une seule requête, puis la connexion se ferme (`POST /rpc`).
    pub one_shot: bool,
}

impl Peer {
    /// Canal local, réservé à l'utilisateur par le système : tous les droits, sauf ceux
    /// que le client abandonne à l'ouverture.
    pub fn local() -> Peer {
        Peer {
            scope: Scope::Admin,
            token_id: None,
            one_shot: false,
        }
    }

    /// Connexion par jeton : sa portée, jamais `admin`.
    pub fn token(token_id: &str, scope: Scope) -> Peer {
        Peer {
            scope: if scope == Scope::Admin {
                Scope::Control
            } else {
                scope
            },
            token_id: Some(token_id.to_owned()),
            one_shot: false,
        }
    }

    /// La même, pour une seule requête (`POST /rpc`) : rien ne doit lui survivre.
    pub fn one_shot(mut self) -> Peer {
        self.one_shot = true;
        self
    }
}

/// Un message reçu d'un transport.
#[derive(Debug)]
pub enum Incoming {
    Message(Vec<u8>),
    /// Plus long que [`MAX_MESSAGE`] : la connexion se ferme après une erreur.
    TooLong,
}

/// Sert un client du canal local sur un flux d'octets, jusqu'à ce qu'il ferme la
/// connexion.
pub async fn serve<R, W>(engine: Engine, reader: R, writer: W)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    serve_as(engine, Peer::local(), reader, writer).await;
}

/// Comme [`serve`], pour un client donné : un message JSON par ligne.
pub async fn serve_as<R, W>(engine: Engine, peer: Peer, reader: R, writer: W)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out, outgoing) = mpsc::channel(OUTGOING);
    let writing = tokio::spawn(write_all(writer, outgoing));
    let (incoming, messages) = mpsc::channel(1);
    let reading = tokio::spawn(read_lines(reader, incoming));
    serve_messages(engine, peer, messages, out).await;
    reading.abort();
    let _ = writing.await;
}

async fn read_lines<R: AsyncRead + Unpin>(reader: R, incoming: mpsc::Sender<Incoming>) {
    let mut reader = BufReader::new(reader);
    loop {
        let mut line = Vec::new();
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
        // La fin de ligne ne compte pas : un message de `MAX_MESSAGE` octets passe (audit
        // R22).
        let content = line.len() - usize::from(line.last() == Some(&b'\n'));
        let message = if content > MAX_MESSAGE {
            Incoming::TooLong
        } else {
            Incoming::Message(line)
        };
        let last = matches!(message, Incoming::TooLong);
        if incoming.send(message).await.is_err() || last {
            break;
        }
    }
}

/// Sert un client dont les messages arrivent un par un, quel que soit le transport
/// (lignes du canal local, trames du WebSocket) ; les réponses et les notifications
/// partent par `out`. Rend la main quand le client a fini, quand la connexion doit se
/// fermer, ou quand le jeton de la connexion est révoqué.
pub async fn serve_messages(
    engine: Engine,
    peer: Peer,
    mut messages: mpsc::Receiver<Incoming>,
    out: mpsc::Sender<String>,
) {
    // Connexion par jeton : la poignée restreinte (SPEC §7.19), et la révocation suivie.
    let (engine, mut revoked) = match &peer.token_id {
        None => (engine, None),
        Some(id) => {
            let allow_private = engine
                .flag(crate::store::settings::Flag::RemoteAllowPrivate)
                .await
                .unwrap_or(false);
            let Ok(restricted) = engine.restricted(allow_private) else {
                return;
            };
            let events = engine.subscribe();
            // Abonné d'abord, puis le jeton relu : une révocation arrivée entre sa
            // vérification et cet abonnement n'est pas perdue (audit D2).
            match engine.tokens().await {
                Ok(tokens) if tokens.iter().any(|t| &t.id == id) => {}
                _ => return,
            }
            let watch = Box::pin(revocation(engine.clone(), events, id.clone()));
            (restricted, Some(watch))
        }
    };
    let mut ctx = Ctx {
        engine,
        out: out.clone(),
        events: None,
        close_after: false,
        shutdown_after: false,
        scope: peer.scope,
        crawl: None,
        crawl_notes: None,
        one_shot: peer.one_shot,
    };
    loop {
        let message = tokio::select! {
            message = messages.recv() => message,
            () = async { revoked.as_mut().expect("suivi").await }, if revoked.is_some() => break,
        };
        let line = match message {
            None => break,
            Some(Incoming::TooLong) => {
                let e = RpcError::invalid_request("message trop long");
                let _ = out.send(response(&Value::Null, Err(e)).to_string()).await;
                break;
            }
            Some(Incoming::Message(line)) => line,
        };
        // La révocation d'un jeton coupe aussi un appel en cours (audit R13).
        let reply = tokio::select! {
            reply = handle(&mut ctx, &line) => reply,
            () = async { revoked.as_mut().expect("suivi").await }, if revoked.is_some() => break,
        };
        if let Some(reply) = reply
            && out.send(reply.to_string()).await.is_err()
        {
            break;
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
}

/// Se termine quand le jeton `token_id` est révoqué, ou quand le moteur s'arrête.
async fn revocation(engine: Engine, mut events: broadcast::Receiver<Event>, token_id: String) {
    loop {
        match events.recv().await {
            Ok(Event::TokenRevoked { token_id: id, .. }) if id == token_id => return,
            Ok(_) => {}
            // Des événements perdus ont pu contenir la révocation : le jeton est relu.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let still = engine
                    .tokens()
                    .await
                    .map(|rows| rows.iter().any(|t| t.id == token_id));
                if !still.unwrap_or(false) {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
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
    // UTF-8 invalide : refusé comme illisible, jamais corrigé en silence (audit R11).
    let Ok(text) = std::str::from_utf8(line) else {
        return Some(response(
            &Value::Null,
            Err(RpcError::parse("UTF-8 invalide")),
        ));
    };
    if text.trim().is_empty() {
        return None;
    }
    let message: Value = match serde_json::from_str(text) {
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
        // Requête invalide : toujours une réponse, `id` à `null` s'il manque ou ne se lit
        // pas (JSON-RPC 2.0, §5 ; audit R8 : sans `id`, elle restait sans réponse).
        _ => {
            let id = if valid_id { reply_id } else { Value::Null };
            return Some(response(
                &id,
                Err(RpcError::invalid_request(
                    "jsonrpc « 2.0 », method et id valides attendus",
                )),
            ));
        }
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
                Ok(event) => {
                    // Le dernier avancement d'une tâche part avant son changement d'état,
                    // sinon une pause gardait l'avancement d'avant, ou une fin le perdait
                    // (audit R4). Révision juste antérieure : la page ne l'écarte pas.
                    if let Event::StateChanged { rev, task_id, .. } = &event
                        && topics.progress
                        && progress.has(task_id)
                        && let Some(early) = progress.flush(rev.saturating_sub(1))
                        && out.send(early.to_string()).await.is_err()
                    {
                        break;
                    }
                    match describe(&engine, event, topics, &mut progress).await {
                    Some(message) => message,
                    None => continue,
                    }
                }
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
        Event::Added { rev, task } if topics.tasks => Some(notification(
            "task.added",
            json!({"rev": rev, "task": task_json(&task)}),
        )),
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
            if let Some(ratio) = update.ratio_limit {
                params["ratioLimit"] = json!(ratio.map(|r| r.0));
            }
            Some(notification("task.updated", params))
        }
        Event::Prepared {
            rev,
            task_id,
            dest_dir,
            file_name,
            display_name,
            total_bytes,
            found_checksum,
        } if topics.tasks => {
            // Le nom affiché depuis la préparation : celui du fichier, ou le titre d'une
            // vidéo. Le dossier aussi : une règle le choisit à la préparation (§7.21).
            let mut params = json!({
                "rev": rev,
                "taskId": task_id,
                "destDir": dest_dir,
                "fileName": file_name,
                "displayName": display_name,
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
        // Réglages de l'écoute : canal local seulement (audit R1). Une connexion à jeton
        // reçoit la révision sans la valeur, pour ne pas y voir un trou (SPEC §6.10).
        Event::SettingChanged { rev, key, value } if topics.queues => {
            let values = if engine.is_restricted() && super::methods::is_remote_setting(key) {
                json!({})
            } else {
                json!({key: value})
            };
            Some(notification(
                "settings.updated",
                json!({"rev": rev, "values": values}),
            ))
        }
        Event::FlagChanged { rev, flag, on } if topics.queues => {
            let key = flag.key();
            let values = if engine.is_restricted() && super::methods::is_remote_setting(key) {
                json!({})
            } else {
                json!({key: on})
            };
            Some(notification(
                "settings.updated",
                json!({"rev": rev, "values": values}),
            ))
        }
        Event::NetworkChanged {
            rev,
            usable,
            metered,
        } if topics.queues => Some(notification(
            "engine.network",
            json!({"rev": rev, "usable": usable, "metered": metered}),
        )),
        // Avertissements du moteur (mise à jour de yt-dlp, SPEC §7.17).
        Event::Warning {
            rev,
            code,
            message,
            task_id,
        } if topics.queues => {
            let mut params = json!({"rev": rev, "code": code, "message": message});
            if let Some(id) = task_id {
                params["taskId"] = json!(id);
            }
            Some(notification("engine.warning", params))
        }
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
        // Projets (SPEC §7.18 quater) : avec les planifications ; `project` nul, supprimé.
        Event::ProjectChanged {
            rev,
            project_id,
            project,
        } if topics.queues => Some(notification(
            "project.updated",
            json!({
                "rev": rev,
                "projectId": project_id,
                "project": project.as_deref().map(crate::rpc::project_json),
            }),
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
    /// Un avancement de cette tâche attend l'envoi.
    fn has(&self, task_id: &str) -> bool {
        self.latest.contains_key(task_id)
    }

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
            // Copie d'un site (SPEC §7.18 ter, depuis le 2026-11-26) : ses éléments.
            if let Some(c) = p.site {
                item["site"] = json!({
                    "pages": c.pages,
                    "resources": c.resources,
                    "files": c.files,
                    "failed": c.failed,
                    "ignored": c.ignored,
                    "unchanged": c.unchanged,
                    "queued": c.queued,
                    "mode": if c.files_only { "files" } else { "copy" },
                });
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
            merging: false,
            site: None,
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

    /// `engine.warning` porte `taskId` quand l'avertissement vise une tâche (sous-titres
    /// manquants, SPEC §7.17), et pas sinon.
    #[tokio::test]
    async fn a_warning_names_its_task_when_it_has_one() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::start(crate::scheduler::Config {
            db_path: dir.path().join("auro.db"),
            client: crate::net::Client::new().unwrap(),
            transfer: crate::transfer::Options::default(),
            disk_recheck: crate::scheduler::DISK_RECHECK,
            planning: crate::scheduler::Planning::default(),
        })
        .unwrap();
        let topics = Topics {
            tasks: false,
            queues: true,
            progress: false,
        };
        let mut batch = ProgressBatch::default();
        for (task_id, expected) in [(Some("t1".to_owned()), json!("t1")), (None, Value::Null)] {
            let event = Event::Warning {
                rev: 7,
                code: "SUBTITLES_MISSING",
                message: "m".into(),
                task_id,
            };
            let message = describe(&engine, event, topics, &mut batch).await.unwrap();
            assert_eq!(message["method"], "engine.warning");
            assert_eq!(message["params"]["code"], "SUBTITLES_MISSING");
            assert_eq!(message["params"]["taskId"], expected);
        }
        engine.shutdown().await;
    }
}
