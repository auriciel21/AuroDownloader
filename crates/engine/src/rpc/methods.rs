//! Méthodes du protocole (SPEC §6.4) : lecture des paramètres nommés, appel du moteur,
//! résultat en JSON.

use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::scope::{self, Scope};
use super::session::{Ctx, Topics};
use super::{
    PROTOCOL_VERSIONS, RpcError, end_action_json, negotiate, queue_json, schedule_json, task_json,
};
use crate::schedule::Rules;
use crate::scheduler::{AddTask, EngineError, Include, QueueChange, TaskDetails, TaskUpdate};
use crate::store::tasks::{TaskFilter, TaskState};
use crate::transfer::MAX_CONNECTIONS;

/// Intervalle par défaut des notifications `task.progress` (SPEC §6.5).
const DEFAULT_PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// Longueur maximale d'une clé d'idempotence, en octets.
const MAX_IDEMPOTENCY_KEY: usize = 256;

/// Paramètres nommés d'un appel : toujours un objet (SPEC §6.1).
struct Params(Map<String, Value>);

impl Params {
    fn new(params: Value) -> Result<Self, RpcError> {
        match params {
            Value::Null => Ok(Params(Map::new())),
            Value::Object(map) => Ok(Params(map)),
            _ => Err(RpcError::invalid_params("params doit être un objet")),
        }
    }

    /// Valeur présente et non nulle.
    fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name).filter(|v| !v.is_null())
    }

    fn wrong(name: &str, what: &str) -> RpcError {
        RpcError::invalid_params(format!("{name} : {what} attendu"))
    }

    fn str(&self, name: &str) -> Result<Option<String>, RpcError> {
        self.get(name)
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| Self::wrong(name, "texte"))
            })
            .transpose()
    }

    fn required_str(&self, name: &str) -> Result<String, RpcError> {
        self.str(name)?
            .ok_or_else(|| RpcError::invalid_params(format!("{name} manquant")))
    }

    fn u32(&self, name: &str) -> Result<Option<u32>, RpcError> {
        self.get(name)
            .map(|v| {
                v.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| Self::wrong(name, "entier positif"))
            })
            .transpose()
    }

    /// Limite de débit : absente, inchangée (`None`) ; `null`, retirée (`Some(None)`) ;
    /// sinon un entier strictement positif, en octets par seconde.
    fn limit(&self, name: &str) -> Result<Option<Option<u64>>, RpcError> {
        match self.0.get(name) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(v) => match v.as_u64() {
                Some(n) if n > 0 => Ok(Some(Some(n))),
                _ => Err(Self::wrong(name, "entier strictement positif ou null")),
            },
        }
    }

    /// Texte facultatif qu'on peut effacer : absent `None`, `null` `Some(None)`.
    fn nullable_str(&self, name: &str) -> Result<Option<Option<String>>, RpcError> {
        match self.0.get(name) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(Value::String(s)) => Ok(Some(Some(s.clone()))),
            Some(_) => Err(Self::wrong(name, "texte ou null")),
        }
    }

    /// Booléen ; `default` s'il est absent.
    fn bool_or(&self, name: &str, default: bool) -> Result<bool, RpcError> {
        self.get(name)
            .map(|v| v.as_bool().ok_or_else(|| Self::wrong(name, "booléen")))
            .transpose()
            .map(|b| b.unwrap_or(default))
    }

    fn bool(&self, name: &str) -> Result<bool, RpcError> {
        self.bool_or(name, false)
    }

    fn strings(&self, name: &str) -> Result<Option<Vec<String>>, RpcError> {
        self.get(name)
            .map(|v| {
                v.as_array()
                    .and_then(|items| {
                        items
                            .iter()
                            .map(|i| i.as_str().map(str::to_owned))
                            .collect::<Option<Vec<_>>>()
                    })
                    .ok_or_else(|| Self::wrong(name, "tableau de textes"))
            })
            .transpose()
    }

    fn task_ids(&self) -> Result<Vec<String>, RpcError> {
        let ids = self
            .strings("taskIds")?
            .ok_or_else(|| RpcError::invalid_params("taskIds manquant"))?;
        if ids.is_empty() {
            return Err(RpcError::invalid_params("taskIds vide"));
        }
        Ok(ids)
    }

    /// Paramètres prévus par la SPEC mais pas encore pris en charge : refusés plutôt
    /// qu'ignorés en silence.
    fn refuse(&self, names: &[&str]) -> Result<(), RpcError> {
        match names.iter().find(|n| self.get(n).is_some()) {
            Some(name) => Err(RpcError::capability_missing(format!(
                "{name} n'est pas encore pris en charge par ce moteur"
            ))),
            None => Ok(()),
        }
    }
}

/// Appelle `method` ; les méthodes inconnues répondent « méthode inconnue ».
pub(super) async fn call(ctx: &mut Ctx, method: &str, params: Value) -> Result<Value, RpcError> {
    if let Some(need) = scope::required(method, &params)
        && !ctx.scope.allows(need)
    {
        return Err(RpcError::unauthorized(format!(
            "{method} demande la portée {}, cette connexion a la portée {}",
            need.as_str(),
            ctx.scope.as_str()
        )));
    }
    let p = Params::new(params)?;
    match method {
        "engine.hello" => hello(ctx, &p),
        "engine.shutdown" => {
            ctx.shutdown_after = true;
            Ok(json!({}))
        }
        "task.add" => task_add(ctx, &p).await,
        "task.get" => task_get(ctx, &p).await,
        "task.list" => task_list(ctx, &p).await,
        "task.update" => {
            p.refuse(&["maxConnections", "destDir", "fileName", "checksum"])?;
            let update = TaskUpdate {
                speed_limit_bps: p.limit("speedLimitBps")?,
            };
            let task = ctx
                .engine
                .update_task(&p.required_str("taskId")?, update)
                .await?;
            Ok(task_json(&task))
        }
        "task.pause" => for_each(ctx, &p, Action::Pause).await,
        "task.resume" => for_each(ctx, &p, Action::Resume).await,
        "task.cancel" => {
            let delete_files = p.bool("deleteFiles")?;
            for_each(ctx, &p, Action::Cancel { delete_files }).await
        }
        "task.remove" => {
            let delete_files = p.bool("deleteFiles")?;
            let mut result = for_each(ctx, &p, Action::Remove { delete_files }).await?;
            // SPEC §6.4 : le nombre de tâches retirées.
            result["removed"] = json!(result["taskIds"].as_array().map_or(0, Vec::len));
            Ok(result)
        }
        "queue.list" => {
            let queues = ctx.engine.queues().await?;
            Ok(json!({"items": queues.iter().map(queue_json).collect::<Vec<_>>()}))
        }
        "queue.create" => {
            let name = p.required_str("name")?;
            let max_active = p.u32("maxActive")?.unwrap_or(3);
            let limit = p.limit("speedLimitBps")?.flatten();
            let schedule_id = p.nullable_str("scheduleId")?.flatten();
            // Vérifiée avant : une planification inconnue ne laisse pas de file créée.
            if let Some(id) = &schedule_id
                && !ctx
                    .engine
                    .schedules()
                    .await?
                    .iter()
                    .any(|v| &v.schedule.id == id)
            {
                return Err(EngineError::ScheduleNotFound(id.clone()).into());
            }
            let id = ctx.engine.create_queue(&name, max_active, limit).await?;
            if let Some(schedule_id) = schedule_id {
                let change = QueueChange {
                    schedule_id: Some(Some(schedule_id)),
                    ..QueueChange::default()
                };
                ctx.engine.update_queue(&id, change).await?;
            }
            queue_by_id(ctx, &id).await
        }
        "queue.update" => {
            let id = p.required_str("queueId")?;
            let change = QueueChange {
                name: p.str("name")?,
                max_active: p.u32("maxActive")?,
                speed_limit_bps: p.limit("speedLimitBps")?,
                schedule_id: p.nullable_str("scheduleId")?,
            };
            let queue = ctx.engine.update_queue(&id, change).await?;
            Ok(queue_json(&queue))
        }
        "limits.get" => limits(ctx).await,
        "limits.set" => {
            // Débit montant : pour les torrents, pas encore pris en charge.
            p.refuse(&["uploadBps"])?;
            if let Some(limit) = p.limit("downloadBps")? {
                ctx.engine.set_download_limit(limit).await?;
            }
            limits(ctx).await
        }
        "events.subscribe" => subscribe(ctx, &p),
        "events.unsubscribe" => {
            ctx.stop_events();
            Ok(json!({}))
        }
        "sync.snapshot" => {
            let s = ctx.engine.snapshot().await?;
            Ok(json!({
                "rev": s.rev,
                "tasks": s.tasks.iter().map(task_json).collect::<Vec<_>>(),
                "queues": s.queues.iter().map(queue_json).collect::<Vec<_>>(),
                "limits": {"downloadBps": s.download_limit, "uploadBps": null},
                "schedules": s.schedules.iter().map(schedule_json).collect::<Vec<_>>(),
                "endAction": end_action_json(s.end_action.as_ref()),
            }))
        }
        "schedule.list" => {
            let items = ctx.engine.schedules().await?;
            Ok(json!({"items": items.iter().map(schedule_json).collect::<Vec<_>>()}))
        }
        "schedule.set" => {
            let rules = p
                .get("rules")
                .ok_or_else(|| RpcError::invalid_params("rules manquant"))
                .and_then(|v| Rules::from_json(v).map_err(RpcError::invalid_params))?;
            let view = ctx
                .engine
                .set_schedule(
                    p.str("scheduleId")?,
                    p.required_str("name")?,
                    p.bool_or("enabled", true)?,
                    rules,
                )
                .await?;
            Ok(schedule_json(&view))
        }
        "schedule.delete" => {
            ctx.engine
                .delete_schedule(&p.required_str("scheduleId")?)
                .await?;
            Ok(json!({}))
        }
        "engine.cancelEndAction" => {
            let cancelled = ctx.engine.cancel_end_action().await?;
            Ok(json!({"cancelled": cancelled}))
        }
        _ => Err(RpcError::method_not_found(method)),
    }
}

fn hello(ctx: &mut Ctx, p: &Params) -> Result<Value, RpcError> {
    let protocol = p
        .get("protocol")
        .and_then(Value::as_object)
        .ok_or_else(|| RpcError::invalid_params("protocol manquant"))?;
    let bound = |name: &str| {
        protocol
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params(format!("protocol.{name} manquant")))
    };
    let Some((major, minor)) = negotiate(bound("min")?, bound("max")?) else {
        // Aucune version commune : erreur, puis fermeture (SPEC §6.3).
        ctx.close_after = true;
        let ours: Vec<String> = PROTOCOL_VERSIONS
            .iter()
            .map(|(a, b)| format!("{a}.{b}"))
            .collect();
        return Err(RpcError::new(
            -32002,
            "PROTOCOL_UNSUPPORTED",
            format!("versions du protocole de ce moteur : {}", ours.join(", ")),
        ));
    };
    // Portée demandée : seulement plus étroite que celle de la connexion (SPEC §6.9).
    if let Some(text) = p.str("scope")? {
        let wanted = Scope::parse(&text).ok_or_else(|| {
            RpcError::invalid_params("scope : read, add, control ou admin attendu")
        })?;
        if !ctx.scope.allows(wanted) {
            return Err(RpcError::unauthorized(format!(
                "portée {text} refusée : cette connexion a la portée {}",
                ctx.scope.as_str()
            )));
        }
        ctx.scope = wanted;
        // Plus le droit de lire : l'abonnement en cours s'arrête.
        if !wanted.allows(Scope::Read) {
            ctx.stop_events();
        }
    }
    Ok(json!({
        "engine": {"version": env!("CARGO_PKG_VERSION"), "platform": std::env::consts::OS},
        "protocol": format!("{major}.{minor}"),
        "capabilities": [],
        "rev": ctx.engine.rev(),
        "scope": ctx.scope.as_str(),
    }))
}

/// Limites globales (`limits.get`) ; le débit montant n'est pas encore limité.
async fn limits(ctx: &Ctx) -> Result<Value, RpcError> {
    let download = ctx.engine.download_limit().await?;
    Ok(json!({"downloadBps": download, "uploadBps": null}))
}

async fn task_add(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    p.refuse(&["fileName", "checksum", "headers"])?;
    let idempotency_key = p.str("idempotencyKey")?;
    if idempotency_key
        .as_ref()
        .is_some_and(|k| k.is_empty() || k.len() > MAX_IDEMPOTENCY_KEY)
    {
        return Err(RpcError::invalid_params(format!(
            "idempotencyKey : de 1 à {MAX_IDEMPOTENCY_KEY} octets"
        )));
    }
    let sources = p
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError::invalid_params("sources manquant"))?;
    let url = match sources.as_slice() {
        [one] => one
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("sources[0].url manquant"))?,
        [] => return Err(RpcError::invalid_params("sources vide")),
        _ => {
            return Err(RpcError::capability_missing(
                "plusieurs sources par tâche : pas encore pris en charge",
            ));
        }
    };
    let dest_dir = p.required_str("destDir")?;
    let dir = Path::new(&dest_dir);
    if !dir.is_absolute() || !dir.is_dir() {
        return Err(RpcError::new(
            -32021,
            "DEST_NOT_WRITABLE",
            "destDir doit être un dossier existant, en chemin absolu",
        ));
    }
    let max_connections = p.u32("maxConnections")?;
    if max_connections.is_some_and(|c| c == 0 || c > MAX_CONNECTIONS) {
        return Err(RpcError::invalid_params(format!(
            "maxConnections : de 1 à {MAX_CONNECTIONS}"
        )));
    }
    let task_id = ctx
        .engine
        .add(AddTask {
            url: url.to_owned(),
            dest_dir,
            queue_id: p.str("queueId")?,
            max_connections,
            speed_limit_bps: p.limit("speedLimitBps")?.flatten(),
            start_paused: p.bool("startPaused")?,
            idempotency_key,
        })
        .await?;
    Ok(json!({"taskId": task_id}))
}

/// `task.get` : la tâche, et sur demande (`include`) ses segments et ses sources.
async fn task_get(ctx: &mut Ctx, p: &Params) -> Result<Value, RpcError> {
    let mut include = Include::default();
    for item in p.strings("include")?.unwrap_or_default() {
        match item.as_str() {
            "segments" => include.segments = true,
            "sources" => include.sources = true,
            // Fichiers d'un torrent (SPEC §6.4) : pas encore de torrents.
            "files" => {
                return Err(RpcError::capability_missing(
                    "include files n'est pas encore pris en charge par ce moteur",
                ));
            }
            other => {
                return Err(RpcError::invalid_params(format!(
                    "include : valeur inconnue {other}"
                )));
            }
        }
    }
    let details = ctx
        .engine
        .details(&p.required_str("taskId")?, include)
        .await?;
    Ok(details_json(&details))
}

fn details_json(d: &TaskDetails) -> Value {
    let mut task = task_json(&d.task);
    if let Some(segments) = &d.segments {
        task["segments"] = segments
            .iter()
            .map(|(s, receiving)| {
                json!({
                    "idx": s.idx,
                    "start": s.start,
                    "end": s.end,
                    // Vidés sur disque, ou reçus par la connexion en cours, le plus grand.
                    "doneBytes": receiving.map_or(s.written, |r| r.max(s.written)),
                    "receiving": receiving.is_some(),
                })
            })
            .collect();
    }
    if let Some(sources) = &d.sources {
        task["sources"] = sources
            .iter()
            .map(|s| json!({"url": s.url, "acceptsRanges": s.accepts_ranges}))
            .collect();
    }
    task
}

async fn task_list(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    let states = p
        .strings("states")?
        .unwrap_or_default()
        .iter()
        .map(|s| {
            TaskState::from_sql(s)
                .ok_or_else(|| RpcError::invalid_params(format!("état inconnu : {s}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let limit = p.u32("limit")?;
    let filter = TaskFilter {
        queue_id: p.str("queueId")?,
        states,
        after: p.str("cursor")?,
        limit,
    };
    let (tasks, rev) = ctx.engine.list(filter).await?;
    let next = match (limit, tasks.last()) {
        (Some(l), Some(last)) if tasks.len() == l as usize => Some(last.id.clone()),
        _ => None,
    };
    Ok(json!({
        "items": tasks.iter().map(task_json).collect::<Vec<_>>(),
        "nextCursor": next,
        "rev": rev,
    }))
}

#[derive(Clone, Copy)]
enum Action {
    Pause,
    Resume,
    Cancel { delete_files: bool },
    Remove { delete_files: bool },
}

/// Applique `action` à chaque tâche de `taskIds`. Les tâches refusées n'empêchent pas
/// les autres : elles figurent dans `errors`, avec leur erreur.
async fn for_each(ctx: &Ctx, p: &Params, action: Action) -> Result<Value, RpcError> {
    let (mut done, mut errors) = (Vec::new(), Vec::new());
    for id in p.task_ids()? {
        let result: Result<(), EngineError> = match action {
            Action::Pause => ctx.engine.pause(&id).await,
            Action::Resume => ctx.engine.resume(&id).await,
            Action::Cancel { delete_files } => ctx.engine.cancel(&id, delete_files).await,
            Action::Remove { delete_files } => ctx.engine.remove(&id, delete_files).await,
        };
        match result {
            Ok(()) => done.push(id),
            Err(e) => {
                let e = RpcError::from(e);
                errors.push(json!({
                    "taskId": id,
                    "code": e.data_code,
                    "message": e.message,
                }));
            }
        }
    }
    Ok(json!({"taskIds": done, "errors": errors}))
}

async fn queue_by_id(ctx: &Ctx, queue_id: &str) -> Result<Value, RpcError> {
    let queues = ctx.engine.queues().await?;
    queues
        .iter()
        .find(|q| q.id == queue_id)
        .map(queue_json)
        .ok_or_else(|| RpcError::internal("file créée introuvable"))
}

fn subscribe(ctx: &mut Ctx, p: &Params) -> Result<Value, RpcError> {
    let names = p
        .strings("topics")?
        .ok_or_else(|| RpcError::invalid_params("topics manquant"))?;
    let mut topics = Topics::default();
    for name in &names {
        match name.as_str() {
            "tasks" => topics.tasks = true,
            "queues" => topics.queues = true,
            "progress" => topics.progress = true,
            other => {
                return Err(RpcError::invalid_params(format!(
                    "sujet inconnu : {other} (tasks, queues, progress)"
                )));
            }
        }
    }
    let interval = p
        .u32("progressIntervalMs")?
        .map_or(DEFAULT_PROGRESS_INTERVAL, |ms| {
            Duration::from_millis(u64::from(ms.max(50)))
        });
    ctx.start_events(topics, interval);
    Ok(json!({
        "topics": names,
        "progressIntervalMs": interval.as_millis() as u64,
        "rev": ctx.engine.rev(),
    }))
}
