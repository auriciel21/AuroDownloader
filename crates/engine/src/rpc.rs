//! Protocole du moteur : JSON-RPC 2.0, notifications, révision `rev`. Voir SPEC §6.
//!
//! Un message JSON par ligne, dans les deux sens, quel que soit le transport ([`serve`]
//! lit et écrit un flux d'octets). Le transport local (socket Unix, canal nommé Windows)
//! est dans [`local`] ; l'authentification y repose sur les droits du système (SPEC §6.2).
//!
//! Méthodes prises en charge : la liste de la SPEC §6.5 (« Méthodes servies »), tenue à
//! jour là plutôt qu'ici. Les autres méthodes de la SPEC §6.4 répondent « méthode
//! inconnue » jusqu'à leur arrivée.

use serde_json::{Value, json};

use crate::scheduler::{EngineError, PendingEnd, ScheduleView};
use crate::store::StoreError;
use crate::store::tasks::{QueueRow, TaskRow};

pub mod local;
mod methods;
mod session;
#[cfg(test)]
mod tests;

pub use session::serve;

/// Versions du protocole que ce moteur parle, de la plus ancienne à la plus récente.
pub const PROTOCOL_VERSIONS: &[(u32, u32)] = &[(1, 0)];

/// Longueur maximale d'un message reçu : au-delà, la connexion est fermée.
pub const MAX_MESSAGE: usize = 1 << 20;

/// Erreur renvoyée au client (SPEC §6.7) : `code` numérique, `data.code` stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub data_code: &'static str,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, data_code: &'static str, message: impl Into<String>) -> Self {
        RpcError {
            code,
            data_code,
            message: message.into(),
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(-32700, "PARSE_ERROR", message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(-32600, "INVALID_REQUEST", message)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            -32601,
            "METHOD_NOT_FOUND",
            format!("méthode inconnue : {method}"),
        )
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, "INVALID_PARAMS", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(-32603, "INTERNAL_ERROR", message)
    }

    pub fn capability_missing(message: impl Into<String>) -> Self {
        Self::new(-32030, "CAPABILITY_MISSING", message)
    }

    fn to_json(&self) -> Value {
        json!({"code": self.code, "message": self.message, "data": {"code": self.data_code}})
    }
}

impl From<EngineError> for RpcError {
    fn from(e: EngineError) -> Self {
        let message = e.to_string();
        match e {
            EngineError::Store(StoreError::TaskNotFound { .. }) => {
                Self::new(-32010, "TASK_NOT_FOUND", message)
            }
            EngineError::InvalidState { .. } | EngineError::FileChanged => {
                Self::new(-32011, "INVALID_STATE", message)
            }
            EngineError::QueueNotFound(_) => Self::new(-32012, "QUEUE_NOT_FOUND", message),
            EngineError::ScheduleNotFound(_) => Self::new(-32013, "SCHEDULE_NOT_FOUND", message),
            EngineError::Url(_) => Self::new(-32020, "URL_INVALID", message),
            EngineError::BadRequest(_) => Self::invalid_params(message),
            EngineError::Store(_) | EngineError::DeleteFile { .. } | EngineError::Stopped => {
                Self::internal(message)
            }
        }
    }
}

/// Réponse JSON-RPC à la requête `id`.
fn response(id: &Value, result: Result<Value, RpcError>) -> Value {
    match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e.to_json()}),
    }
}

/// Notification du moteur (sans `id`).
fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// Une tâche telle que le protocole la décrit (noms en casse mixte, SPEC §6.1).
pub fn task_json(t: &TaskRow) -> Value {
    json!({
        "taskId": t.id,
        "queueId": t.queue_id,
        "state": t.state.as_sql(),
        "priority": t.priority,
        "displayName": t.display_name,
        "destDir": t.dest_dir,
        "fileName": t.file_name,
        "totalBytes": t.total_bytes,
        "doneBytes": t.done_bytes,
        "maxConnections": t.max_connections,
        "speedLimitBps": t.speed_limit_bps,
        "pauseReason": t.pause_reason,
        "errorCode": t.error_code,
        "errorDetail": t.error_detail,
        "retryCount": t.retry_count,
        "createdAt": t.created_at,
        "finishedAt": t.finished_at,
    })
}

/// Une file telle que le protocole la décrit.
pub fn queue_json(q: &QueueRow) -> Value {
    json!({
        "queueId": q.id,
        "name": q.name,
        "position": q.position,
        "maxActive": q.max_active,
        "speedLimitBps": q.speed_limit_bps,
        "scheduleId": q.schedule_id,
    })
}

/// Une planification telle que le protocole la décrit, avec son ouverture (SPEC §7.11).
pub fn schedule_json(v: &ScheduleView) -> Value {
    json!({
        "scheduleId": v.schedule.id,
        "name": v.schedule.name,
        "enabled": v.schedule.enabled,
        "rules": v.schedule.rules.to_json(),
        "open": v.open,
        "nextChangeAt": v.next_change_at,
    })
}

/// Action de fin annoncée, ou `null`.
pub fn end_action_json(pending: Option<&PendingEnd>) -> Value {
    match pending {
        Some(p) => json!({"queueId": p.queue_id, "action": p.action.as_str(), "at": p.at_ms}),
        None => Value::Null,
    }
}

/// Version « majeure.mineure ».
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let (major, minor) = text.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Version la plus récente commune à `[min, max]` et à ce moteur (SPEC §6.3).
pub fn negotiate(min: &str, max: &str) -> Option<(u32, u32)> {
    let (min, max) = (parse_version(min)?, parse_version(max)?);
    PROTOCOL_VERSIONS
        .iter()
        .rev()
        .find(|v| min <= **v && **v <= max)
        .copied()
}
