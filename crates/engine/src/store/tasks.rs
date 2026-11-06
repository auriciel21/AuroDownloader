//! Tâches et sources dans la base : création, préparation, changements d'état (SPEC §5.3
//! et §5.5) et fin de vie (SPEC §5.6, règle 4).
//!
//! Chaque changement d'état est vérifié contre la table de la SPEC §5.5, dans la même
//! transaction que l'écriture : une transition interdite n'est jamais enregistrée.

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use super::segments::set_size_and_segments;
use super::{StoreError, new_id, now_ms};
use crate::segmenter::Segment;

/// État d'une tâche (SPEC §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Queued,
    Active,
    Paused,
    RetryWait,
    Verifying,
    PostProcessing,
    Seeding,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn as_sql(self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Active => "active",
            TaskState::Paused => "paused",
            TaskState::RetryWait => "retry_wait",
            TaskState::Verifying => "verifying",
            TaskState::PostProcessing => "post_processing",
            TaskState::Seeding => "seeding",
            TaskState::Completed => "completed",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }

    pub fn from_sql(text: &str) -> Option<Self> {
        Some(match text {
            "queued" => TaskState::Queued,
            "active" => TaskState::Active,
            "paused" => TaskState::Paused,
            "retry_wait" => TaskState::RetryWait,
            "verifying" => TaskState::Verifying,
            "post_processing" => TaskState::PostProcessing,
            "seeding" => TaskState::Seeding,
            "completed" => TaskState::Completed,
            "failed" => TaskState::Failed,
            "cancelled" => TaskState::Cancelled,
            _ => return None,
        })
    }

    /// Transitions permises, colonne « Passe à » de la SPEC §5.5.
    pub fn can_become(self, next: TaskState) -> bool {
        use TaskState::*;
        matches!(
            (self, next),
            (Queued, Active | Paused | Cancelled)
                | (
                    Active,
                    Paused | RetryWait | Verifying | Seeding | Failed | Cancelled
                )
                | (Paused, Queued | Cancelled)
                | (RetryWait, Active | Paused | Failed | Cancelled)
                | (Verifying, PostProcessing | Completed | Failed)
                | (PostProcessing, Completed | Failed)
                | (Seeding, Completed | Cancelled)
                | (Failed, Queued)
        )
    }
}

/// Cause d'une pause, colonne `tasks.pause_reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    User,
    Schedule,
    DiskFull,
    Network,
    FileChanged,
}

impl PauseReason {
    pub fn as_sql(self) -> &'static str {
        match self {
            PauseReason::User => "user",
            PauseReason::Schedule => "schedule",
            PauseReason::DiskFull => "disk_full",
            PauseReason::Network => "network",
            PauseReason::FileChanged => "file_changed",
        }
    }
}

/// Une tâche telle que la base la décrit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    pub queue_id: String,
    pub state: TaskState,
    /// La plus grande part la première (F-14).
    pub priority: i64,
    pub display_name: String,
    pub dest_dir: String,
    /// Nom du fichier final ; `None` tant que la tâche n'est pas préparée.
    pub file_name: Option<String>,
    pub total_bytes: Option<u64>,
    pub done_bytes: u64,
    pub retry_count: u32,
    /// Connexions imposées ; `None` : nombre automatique (SPEC §7.3).
    pub max_connections: Option<u32>,
    /// Limite de débit de la tâche, en octets par seconde (F-15).
    pub speed_limit_bps: Option<u64>,
    pub pause_reason: Option<String>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
}

/// Une source de téléchargement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRow {
    pub id: String,
    pub url: String,
    pub accepts_ranges: Option<bool>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: Option<u64>,
}

fn to_u64(value: Option<i64>, what: &str) -> Result<Option<u64>, StoreError> {
    value
        .map(|v| u64::try_from(v).map_err(|_| StoreError::BadValue(format!("{what} négatif"))))
        .transpose()
}

fn to_i64(value: u64, what: &str) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::BadValue(format!("{what} {value} trop grand pour la base")))
}

/// Limite de débit à enregistrer : strictement positive, ou absente.
fn limit_to_sql(limit: Option<u64>) -> Result<Option<i64>, StoreError> {
    match limit {
        Some(0) => Err(StoreError::BadValue("limite de débit nulle".into())),
        limit => limit.map(|l| to_i64(l, "limite de débit")).transpose(),
    }
}

/// Change la limite de débit d'une tâche ; `None` la retire.
pub fn set_task_speed_limit(
    conn: &Connection,
    task_id: &str,
    limit: Option<u64>,
) -> Result<(), StoreError> {
    let updated = conn.execute(
        "UPDATE tasks SET speed_limit_bps = ?2, updated_at = ?3 WHERE id = ?1",
        params![task_id, limit_to_sql(limit)?, now_ms()],
    )?;
    if updated == 0 {
        return Err(StoreError::TaskNotFound {
            task_id: task_id.to_owned(),
        });
    }
    Ok(())
}

/// File nommée `name`, créée si elle n'existe pas ; renvoie son identifiant.
pub fn ensure_queue(conn: &Connection, name: &str) -> Result<String, StoreError> {
    let found: Option<String> = conn
        .query_row("SELECT id FROM queues WHERE name = ?1", [name], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(id) = found {
        return Ok(id);
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO queues (id, name, position, created_at)
         VALUES (?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM queues), ?3)",
        params![id, name, now_ms()],
    )?;
    Ok(id)
}

/// Tâche HTTP(S) à créer, avec sa source unique.
#[derive(Debug, Clone, Copy)]
pub struct NewTask<'a> {
    pub queue_id: &'a str,
    pub url: &'a str,
    pub display_name: &'a str,
    pub dest_dir: &'a str,
    /// `None` : nombre de connexions automatique.
    pub max_connections: Option<u32>,
    /// `None` : pas de limite propre à la tâche.
    pub speed_limit_bps: Option<u64>,
}

/// Crée une tâche `queued` et sa source ; renvoie leurs identifiants.
pub fn create_task(
    conn: &mut Connection,
    new: &NewTask<'_>,
) -> Result<(String, String), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (task_id, source_id, now) = (new_id(), new_id(), now_ms());
    let limit = limit_to_sql(new.speed_limit_bps)?;
    tx.execute(
        "INSERT INTO tasks (id, queue_id, kind, state, position, display_name, dest_dir,
                            max_connections, speed_limit_bps, created_at, updated_at)
         VALUES (?1, ?2, 'http', 'queued',
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE queue_id = ?2),
                 ?3, ?4, ?5, ?7, ?6, ?6)",
        params![
            task_id,
            new.queue_id,
            new.display_name,
            new.dest_dir,
            new.max_connections,
            now,
            limit
        ],
    )?;
    tx.execute(
        "INSERT INTO sources (id, task_id, url) VALUES (?1, ?2, ?3)",
        params![source_id, task_id, new.url],
    )?;
    tx.commit()?;
    Ok((task_id, source_id))
}

const TASK_COLUMNS: &str = "id, state, display_name, dest_dir, file_name, total_bytes, \
     done_bytes, retry_count, pause_reason, error_code, error_detail, max_connections, \
     queue_id, priority, speed_limit_bps";

/// Ligne `tasks` telle que SQLite la rend, avant vérification des valeurs.
struct RawTask {
    id: String,
    state: String,
    display_name: String,
    dest_dir: String,
    file_name: Option<String>,
    total_bytes: Option<i64>,
    done_bytes: i64,
    retry_count: i64,
    pause_reason: Option<String>,
    error_code: Option<String>,
    error_detail: Option<String>,
    max_connections: Option<i64>,
    queue_id: String,
    priority: i64,
    speed_limit_bps: Option<i64>,
}

fn raw_task(row: &Row<'_>) -> rusqlite::Result<RawTask> {
    Ok(RawTask {
        id: row.get(0)?,
        state: row.get(1)?,
        display_name: row.get(2)?,
        dest_dir: row.get(3)?,
        file_name: row.get(4)?,
        total_bytes: row.get(5)?,
        done_bytes: row.get(6)?,
        retry_count: row.get(7)?,
        pause_reason: row.get(8)?,
        error_code: row.get(9)?,
        error_detail: row.get(10)?,
        max_connections: row.get(11)?,
        queue_id: row.get(12)?,
        priority: row.get(13)?,
        speed_limit_bps: row.get(14)?,
    })
}

impl TryFrom<RawTask> for TaskRow {
    type Error = StoreError;

    fn try_from(raw: RawTask) -> Result<Self, StoreError> {
        let bad = |what: &str| StoreError::BadValue(format!("tâche {} : {what}", raw.id));
        Ok(TaskRow {
            state: TaskState::from_sql(&raw.state).ok_or_else(|| bad("état inconnu"))?,
            total_bytes: to_u64(raw.total_bytes, "taille")?,
            done_bytes: u64::try_from(raw.done_bytes).map_err(|_| bad("octets écrits négatifs"))?,
            retry_count: u32::try_from(raw.retry_count)
                .map_err(|_| bad("nombre d'essais invalide"))?,
            max_connections: raw
                .max_connections
                .map(|c| u32::try_from(c).map_err(|_| bad("nombre de connexions invalide")))
                .transpose()?,
            speed_limit_bps: to_u64(raw.speed_limit_bps, "limite de débit")?,
            id: raw.id,
            queue_id: raw.queue_id,
            priority: raw.priority,
            display_name: raw.display_name,
            dest_dir: raw.dest_dir,
            file_name: raw.file_name,
            pause_reason: raw.pause_reason,
            error_code: raw.error_code,
            error_detail: raw.error_detail,
        })
    }
}

/// Lit une tâche.
pub fn load_task(conn: &Connection, task_id: &str) -> Result<TaskRow, StoreError> {
    let raw = conn
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
            [task_id],
            raw_task,
        )
        .optional()?
        .ok_or_else(|| StoreError::TaskNotFound {
            task_id: task_id.to_owned(),
        })?;
    raw.try_into()
}

/// Toutes les tâches, dans l'ordre de création.
pub fn list_tasks(conn: &Connection) -> Result<Vec<TaskRow>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {TASK_COLUMNS} FROM tasks ORDER BY created_at, id"
    ))?;
    let rows = stmt.query_map([], raw_task)?;
    rows.map(|r| r?.try_into()).collect()
}

/// Source de rang le plus faible (la préférée) d'une tâche.
pub fn primary_source(conn: &Connection, task_id: &str) -> Result<SourceRow, StoreError> {
    let raw = conn
        .query_row(
            "SELECT id, url, accepts_ranges, etag, last_modified, content_length
             FROM sources WHERE task_id = ?1 ORDER BY rank, id LIMIT 1",
            [task_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::NoSource {
            task_id: task_id.to_owned(),
        })?;
    let (id, url, ranges, etag, last_modified, length) = raw;
    Ok(SourceRow {
        id,
        url,
        accepts_ranges: ranges.map(|r| r != 0),
        etag,
        last_modified,
        content_length: to_u64(length, "taille de la source")?,
    })
}

/// Noms de fichier enregistrés par les autres tâches du dossier `dest_dir` qui peuvent
/// encore écrire leur `.part` : toutes, sauf les terminées et les annulées. Un nouveau
/// téléchargement ne doit prendre aucun de ces noms, même si leur `.part` manque sur le
/// disque (SPEC §7.1, étape 3).
pub fn reserved_names(
    conn: &Connection,
    dest_dir: &str,
    except_task: &str,
) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT file_name FROM tasks
         WHERE dest_dir = ?1 AND id <> ?2 AND file_name IS NOT NULL
           AND state NOT IN ('completed', 'cancelled')",
    )?;
    let names = stmt
        .query_map(params![dest_dir, except_task], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(names)
}

/// Résultat de la préparation (SPEC §7.1) à enregistrer.
#[derive(Debug, Clone, Copy)]
pub struct Preparation<'a> {
    pub source_id: &'a str,
    pub accepts_ranges: bool,
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
    pub total_bytes: Option<u64>,
    pub file_name: &'a str,
}

/// Enregistre la préparation : validateurs de la source, nom du fichier, taille et
/// segments initiaux, dans une seule transaction (SPEC §7.1, étapes 3 et 5, et §5.6,
/// règle 2).
pub fn save_preparation(
    conn: &mut Connection,
    task_id: &str,
    prep: &Preparation<'_>,
    segments: &[Segment],
) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let length = prep.total_bytes.map(|t| to_i64(t, "taille")).transpose()?;
    let updated = tx.execute(
        "UPDATE sources SET accepts_ranges = ?3, etag = ?4, last_modified = ?5,
                            content_length = ?6, state = 'ok'
         WHERE id = ?1 AND task_id = ?2",
        params![
            prep.source_id,
            task_id,
            prep.accepts_ranges,
            prep.etag,
            prep.last_modified,
            length
        ],
    )?;
    if updated == 0 {
        return Err(StoreError::NoSource {
            task_id: task_id.to_owned(),
        });
    }
    tx.execute(
        "UPDATE tasks SET file_name = ?2, display_name = ?2 WHERE id = ?1",
        params![task_id, prep.file_name],
    )?;
    set_size_and_segments(&tx, task_id, prep.total_bytes, segments)?;
    tx.commit()?;
    Ok(())
}

/// Changement d'état demandé.
#[derive(Debug, Clone, Copy)]
pub struct Change<'a> {
    pub to: TaskState,
    /// Obligatoire pour `paused`, interdit sinon.
    pub pause_reason: Option<PauseReason>,
    /// Code (SPEC §6.8) et détail ; effacés si `None`.
    pub error: Option<(&'a str, &'a str)>,
    /// Pour `retry_wait` : date de la nouvelle tentative, en millisecondes Unix.
    pub next_retry_at: Option<i64>,
}

impl Change<'_> {
    /// Changement simple, sans cause ni erreur.
    pub fn to(state: TaskState) -> Self {
        Change {
            to: state,
            pause_reason: None,
            error: None,
            next_retry_at: None,
        }
    }
}

/// Lit l'état et vérifie que `to` lui est permis.
fn check_transition(
    conn: &Connection,
    task_id: &str,
    to: TaskState,
) -> Result<TaskState, StoreError> {
    let state: String = conn
        .query_row("SELECT state FROM tasks WHERE id = ?1", [task_id], |r| {
            r.get(0)
        })
        .optional()?
        .ok_or_else(|| StoreError::TaskNotFound {
            task_id: task_id.to_owned(),
        })?;
    let from = TaskState::from_sql(&state)
        .ok_or_else(|| StoreError::BadValue(format!("tâche {task_id} : état inconnu")))?;
    if !from.can_become(to) {
        return Err(StoreError::BadTransition {
            task_id: task_id.to_owned(),
            from: from.as_sql(),
            to: to.as_sql(),
        });
    }
    Ok(from)
}

/// Change l'état d'une tâche si la SPEC §5.5 le permet ; renvoie l'état quitté.
///
/// `retry_count` compte les passages consécutifs en `retry_wait` ; il repart de zéro à
/// une relance manuelle (`queued`).
pub fn change_state(
    conn: &mut Connection,
    task_id: &str,
    change: &Change<'_>,
) -> Result<TaskState, StoreError> {
    if (change.to == TaskState::Paused) != change.pause_reason.is_some() {
        return Err(StoreError::BadValue(
            "une cause de pause accompagne l'état paused, et lui seul".into(),
        ));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let from = check_transition(&tx, task_id, change.to)?;
    let to = change.to;
    let (code, detail) = change.error.unzip();
    tx.execute(
        "UPDATE tasks SET
           state = ?2, pause_reason = ?3, error_code = ?4, error_detail = ?5,
           next_retry_at = ?6,
           retry_count = CASE ?2 WHEN 'retry_wait' THEN retry_count + 1
                                 WHEN 'queued' THEN 0 ELSE retry_count END,
           started_at = CASE ?2 WHEN 'active' THEN COALESCE(started_at, ?7) ELSE started_at END,
           finished_at = CASE WHEN ?2 IN ('failed', 'cancelled') THEN ?7 ELSE NULL END,
           updated_at = ?7
         WHERE id = ?1",
        params![
            task_id,
            to.as_sql(),
            change.pause_reason.map(PauseReason::as_sql),
            code,
            detail,
            change.next_retry_at.filter(|_| to == TaskState::RetryWait),
            now_ms()
        ],
    )?;
    tx.commit()?;
    Ok(from)
}

/// Fin d'une tâche réussie (SPEC §5.6, règle 4) : depuis `active` (ou `verifying`), passe
/// par `verifying` puis `completed`, écrit la ligne `history`, puis purge les segments et
/// les sources, dans une seule transaction. À appeler après le renommage du fichier.
///
/// `verifying` n'est pas encore un état durable : sans empreinte à contrôler (F-11, à
/// venir), il est traversé dans cette transaction.
pub fn complete_task(
    conn: &mut Connection,
    task_id: &str,
    final_path: &str,
    total_bytes: u64,
) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let from = match check_transition(&tx, task_id, TaskState::Verifying) {
        Ok(from) => from,
        Err(StoreError::BadTransition {
            from: "verifying", ..
        }) => TaskState::Verifying,
        Err(e) => return Err(e),
    };
    debug_assert!(matches!(from, TaskState::Active | TaskState::Verifying));
    let total = to_i64(total_bytes, "taille")?;
    let now = now_ms();

    let url: Option<String> = tx
        .query_row(
            "SELECT url FROM sources WHERE task_id = ?1 ORDER BY rank, id LIMIT 1",
            [task_id],
            |r| r.get(0),
        )
        .optional()?;
    // Le nom d'hôte seulement : l'adresse complète peut porter un jeton (SPEC §8.2).
    let host = url
        .as_deref()
        .and_then(|u| reqwest::Url::parse(u).ok())
        .and_then(|u| u.host_str().map(str::to_owned));
    tx.execute(
        "INSERT INTO history (id, task_id, display_name, final_path, total_bytes, source_host,
                              final_state, finished_at)
         SELECT ?2, id, display_name, ?3, ?4, ?5, 'completed', ?6 FROM tasks WHERE id = ?1",
        params![task_id, new_id(), final_path, total, host, now],
    )?;
    tx.execute(
        "UPDATE tasks SET state = 'completed', total_bytes = ?2, done_bytes = ?2,
           pause_reason = NULL, error_code = NULL, error_detail = NULL, next_retry_at = NULL,
           retry_count = 0, finished_at = ?3, updated_at = ?3
         WHERE id = ?1",
        params![task_id, total, now],
    )?;
    tx.execute("DELETE FROM segments WHERE task_id = ?1", [task_id])?;
    tx.execute("DELETE FROM sources WHERE task_id = ?1", [task_id])?;
    tx.commit()?;
    Ok(())
}

/// Retire une tâche finie (`completed`, `failed` ou `cancelled`) : sa ligne et, en
/// cascade, ses sources, segments et en-têtes. La ligne `history` survit (SPEC §5.3) ;
/// une tâche en échec ou annulée, qui n'en a pas encore, reçoit la sienne ici, dans la
/// même transaction. Les fichiers ne sont pas touchés.
pub fn remove_task(conn: &mut Connection, task_id: &str) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let task = load_task(&tx, task_id)?;
    if !matches!(
        task.state,
        TaskState::Completed | TaskState::Failed | TaskState::Cancelled
    ) {
        return Err(StoreError::BadTransition {
            task_id: task_id.to_owned(),
            from: task.state.as_sql(),
            to: "retirée",
        });
    }
    let recorded: bool = tx
        .query_row(
            "SELECT 1 FROM history WHERE task_id = ?1",
            [task_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !recorded {
        let url: Option<String> = tx
            .query_row(
                "SELECT url FROM sources WHERE task_id = ?1 ORDER BY rank, id LIMIT 1",
                [task_id],
                |r| r.get(0),
            )
            .optional()?;
        // Le nom d'hôte seulement : l'adresse complète peut porter un jeton (SPEC §8.2).
        let host = url
            .as_deref()
            .and_then(|u| reqwest::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(str::to_owned));
        tx.execute(
            "INSERT INTO history (id, task_id, display_name, final_path, total_bytes, source_host,
                                  final_state, finished_at)
             SELECT ?2, id, display_name, NULL, total_bytes, ?3, state,
                    COALESCE(finished_at, ?4)
             FROM tasks WHERE id = ?1",
            params![task_id, new_id(), host, now_ms()],
        )?;
    }
    tx.execute("DELETE FROM tasks WHERE id = ?1", [task_id])?;
    tx.commit()?;
    Ok(())
}

/// La file `queue_id` existe-t-elle ?
pub fn queue_exists(conn: &Connection, queue_id: &str) -> Result<bool, StoreError> {
    Ok(conn
        .query_row("SELECT 1 FROM queues WHERE id = ?1", [queue_id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Tâche qui peut partir, avec la limite de sa file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub task_id: String,
    pub queue_id: String,
    /// Tâches simultanées permises dans la file (F-14).
    pub max_active: u32,
    pub state: TaskState,
}

/// Tâches qui peuvent partir à `now` (millisecondes Unix), dans l'ordre où le planificateur
/// les démarre : files dans leur ordre ; dans une file, d'abord les tâches `active`
/// (interrompues par un arrêt du moteur, SPEC §7.4), puis par priorité décroissante et
/// position. Une tâche `retry_wait` part quand sa date est passée (SPEC §7.8).
pub fn runnable_tasks(conn: &Connection, now: i64) -> Result<Vec<Candidate>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.queue_id, q.max_active, t.state
         FROM tasks t JOIN queues q ON q.id = t.queue_id
         WHERE t.state IN ('queued', 'active')
            OR (t.state = 'retry_wait' AND COALESCE(t.next_retry_at, 0) <= ?1)
         ORDER BY q.position, t.state = 'active' DESC, t.priority DESC, t.position, t.id",
    )?;
    let rows = stmt.query_map([now], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (task_id, queue_id, max_active, state) = row?;
        let bad = |what: &str| StoreError::BadValue(format!("tâche {task_id} : {what}"));
        Ok(Candidate {
            max_active: u32::try_from(max_active).map_err(|_| bad("limite de file invalide"))?,
            state: TaskState::from_sql(&state).ok_or_else(|| bad("état inconnu"))?,
            task_id,
            queue_id,
        })
    })
    .collect()
}

/// Filtre de [`find_tasks`] ; les champs vides ne filtrent rien.
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub queue_id: Option<String>,
    pub states: Vec<TaskState>,
    /// Tâches d'identifiant strictement plus grand (pagination ; les identifiants sont
    /// des UUID v7, triés par date de création).
    pub after: Option<String>,
    pub limit: Option<u32>,
}

/// Tâches selon `filter`, par identifiant croissant (ordre de création).
pub fn find_tasks(conn: &Connection, filter: &TaskFilter) -> Result<Vec<TaskRow>, StoreError> {
    let states: Vec<&str> = filter.states.iter().map(|s| s.as_sql()).collect();
    // Les états sont des constantes de ce module : aucun texte venu de l'extérieur.
    let state_clause = if states.is_empty() {
        String::new()
    } else {
        format!(" AND state IN ('{}')", states.join("','"))
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {TASK_COLUMNS} FROM tasks
         WHERE (?1 IS NULL OR queue_id = ?1) AND (?2 IS NULL OR id > ?2){state_clause}
         ORDER BY id LIMIT ?3"
    ))?;
    let limit = filter.limit.map_or(-1, i64::from);
    let rows = stmt.query_map(params![filter.queue_id, filter.after, limit], raw_task)?;
    rows.map(|r| r?.try_into()).collect()
}

/// Une file de téléchargement (SPEC §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRow {
    pub id: String,
    pub name: String,
    pub position: i64,
    pub max_active: u32,
    pub speed_limit_bps: Option<u64>,
}

/// Toutes les files, dans leur ordre.
pub fn list_queues(conn: &Connection) -> Result<Vec<QueueRow>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, position, max_active, speed_limit_bps FROM queues
         ORDER BY position, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Option<i64>>(4)?,
        ))
    })?;
    rows.map(|row| {
        let (id, name, position, max_active, limit) = row?;
        Ok(QueueRow {
            max_active: u32::try_from(max_active)
                .map_err(|_| StoreError::BadValue(format!("file {id} : limite invalide")))?,
            speed_limit_bps: to_u64(limit, "limite de débit")?,
            id,
            name,
            position,
        })
    })
    .collect()
}

/// Lit une file.
pub fn load_queue(conn: &Connection, queue_id: &str) -> Result<Option<QueueRow>, StoreError> {
    Ok(list_queues(conn)?.into_iter().find(|q| q.id == queue_id))
}

/// Crée une file en dernière position ; renvoie son identifiant. Échoue si le nom est
/// déjà pris (`UNIQUE`).
pub fn create_queue(
    conn: &Connection,
    name: &str,
    max_active: u32,
    speed_limit_bps: Option<u64>,
) -> Result<String, StoreError> {
    let id = new_id();
    conn.execute(
        "INSERT INTO queues (id, name, position, max_active, speed_limit_bps, created_at)
         VALUES (?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM queues), ?3, ?4, ?5)",
        params![
            id,
            name,
            max_active,
            limit_to_sql(speed_limit_bps)?,
            now_ms()
        ],
    )?;
    Ok(id)
}

/// Change le nom, le nombre de tâches simultanées ou la limite de débit d'une file
/// (`Some(None)` retire la limite) ; `false` si elle n'existe pas.
pub fn update_queue(
    conn: &Connection,
    queue_id: &str,
    name: Option<&str>,
    max_active: Option<u32>,
    speed_limit_bps: Option<Option<u64>>,
) -> Result<bool, StoreError> {
    let limit = speed_limit_bps.map(limit_to_sql).transpose()?;
    let updated = conn.execute(
        "UPDATE queues SET name = COALESCE(?2, name), max_active = COALESCE(?3, max_active),
           speed_limit_bps = CASE WHEN ?4 THEN ?5 ELSE speed_limit_bps END
         WHERE id = ?1",
        params![queue_id, name, max_active, limit.is_some(), limit.flatten()],
    )?;
    Ok(updated > 0)
}

/// Date de la prochaine nouvelle tentative (millisecondes Unix), s'il y en a une.
pub fn next_retry_at(conn: &Connection) -> Result<Option<i64>, StoreError> {
    Ok(conn.query_row(
        "SELECT MIN(next_retry_at) FROM tasks WHERE state = 'retry_wait'",
        [],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segmenter::initial_split;
    use crate::store::open_in_memory;
    use crate::store::segments::load_segments;

    fn new_http_task(conn: &mut Connection) -> (String, String) {
        let queue = ensure_queue(conn, "default").unwrap();
        create_task(
            conn,
            &NewTask {
                queue_id: &queue,
                url: "https://exemple.org/f.iso?token=secret",
                display_name: "f.iso",
                dest_dir: "/dl",
                max_connections: None,
                speed_limit_bps: None,
            },
        )
        .unwrap()
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn speed_limits_are_kept_changed_and_removed() {
        let mut conn = open_in_memory().unwrap();
        let (task, _) = new_http_task(&mut conn);
        assert_eq!(load_task(&conn, &task).unwrap().speed_limit_bps, None);
        set_task_speed_limit(&conn, &task, Some(500_000)).unwrap();
        assert_eq!(
            load_task(&conn, &task).unwrap().speed_limit_bps,
            Some(500_000)
        );
        assert!(matches!(
            set_task_speed_limit(&conn, &task, Some(0)),
            Err(StoreError::BadValue(_))
        ));
        assert!(matches!(
            set_task_speed_limit(&conn, "inconnue", None),
            Err(StoreError::TaskNotFound { .. })
        ));

        let queue = create_queue(&conn, "lente", 2, Some(1_000)).unwrap();
        let limit = |conn: &Connection| load_queue(conn, &queue).unwrap().unwrap().speed_limit_bps;
        assert_eq!(limit(&conn), Some(1_000));
        // Une modification sans limite la garde ; `Some(None)` la retire.
        assert!(update_queue(&conn, &queue, Some("lente 2"), None, None).unwrap());
        assert_eq!(limit(&conn), Some(1_000));
        assert!(update_queue(&conn, &queue, None, None, Some(Some(2_000))).unwrap());
        assert_eq!(limit(&conn), Some(2_000));
        assert!(update_queue(&conn, &queue, None, None, Some(None)).unwrap());
        assert_eq!(limit(&conn), None);
        assert!(!update_queue(&conn, "inconnue", None, None, Some(None)).unwrap());
    }

    #[test]
    fn transitions_follow_the_table_of_the_spec() {
        use TaskState::*;
        let all = [
            Queued,
            Active,
            Paused,
            RetryWait,
            Verifying,
            PostProcessing,
            Seeding,
            Completed,
            Failed,
            Cancelled,
        ];
        for s in all {
            assert_eq!(TaskState::from_sql(s.as_sql()), Some(s));
            assert!(!s.can_become(s), "{s:?} vers lui-même");
        }
        assert!(Queued.can_become(Active));
        assert!(Active.can_become(RetryWait));
        assert!(RetryWait.can_become(Active));
        assert!(Failed.can_become(Queued));
        assert!(!Paused.can_become(Active), "une pause repasse par la file");
        assert!(!Active.can_become(Completed), "via verifying");
        for terminal in [Completed, Cancelled] {
            assert!(all.iter().all(|&s| !terminal.can_become(s)));
        }
    }

    #[test]
    fn queues_are_created_once() {
        let conn = open_in_memory().unwrap();
        let a = ensure_queue(&conn, "default").unwrap();
        assert_eq!(ensure_queue(&conn, "default").unwrap(), a);
        assert_ne!(ensure_queue(&conn, "autre").unwrap(), a);
    }

    #[test]
    fn new_task_is_queued_with_its_source() {
        let mut conn = open_in_memory().unwrap();
        let (task_id, source_id) = new_http_task(&mut conn);
        let task = load_task(&conn, &task_id).unwrap();
        assert_eq!(task.state, TaskState::Queued);
        assert_eq!((task.file_name, task.total_bytes), (None, None));
        let source = primary_source(&conn, &task_id).unwrap();
        assert_eq!(source.id, source_id);
        assert_eq!(source.accepts_ranges, None);
        assert_eq!(list_tasks(&conn).unwrap().len(), 1);
    }

    #[test]
    fn preparation_is_saved_in_one_transaction() {
        let mut conn = open_in_memory().unwrap();
        let (task_id, source_id) = new_http_task(&mut conn);
        let prep = Preparation {
            source_id: &source_id,
            accepts_ranges: true,
            etag: Some("\"v1\""),
            last_modified: None,
            total_bytes: Some(100),
            file_name: "f.iso",
        };
        // Segments qui ne couvrent pas la taille : rien n'est enregistré.
        let bad = initial_split(90, 2, 1);
        assert!(save_preparation(&mut conn, &task_id, &prep, &bad).is_err());
        assert_eq!(primary_source(&conn, &task_id).unwrap().etag, None);
        assert_eq!(load_task(&conn, &task_id).unwrap().file_name, None);

        let segments = initial_split(100, 2, 1);
        save_preparation(&mut conn, &task_id, &prep, &segments).unwrap();
        let source = primary_source(&conn, &task_id).unwrap();
        assert_eq!(source.accepts_ranges, Some(true));
        assert_eq!(source.etag.as_deref(), Some("\"v1\""));
        assert_eq!(source.content_length, Some(100));
        let task = load_task(&conn, &task_id).unwrap();
        assert_eq!(task.file_name.as_deref(), Some("f.iso"));
        assert_eq!(task.total_bytes, Some(100));
        assert_eq!(load_segments(&conn, &task_id).unwrap(), segments);
    }

    #[test]
    fn unknown_size_is_saved_without_segments() {
        let mut conn = open_in_memory().unwrap();
        let (task_id, source_id) = new_http_task(&mut conn);
        let prep = Preparation {
            source_id: &source_id,
            accepts_ranges: false,
            etag: None,
            last_modified: None,
            total_bytes: None,
            file_name: "page",
        };
        assert!(matches!(
            save_preparation(&mut conn, &task_id, &prep, &initial_split(10, 1, 1)),
            Err(StoreError::SegmentsWithoutSize { .. })
        ));
        save_preparation(&mut conn, &task_id, &prep, &[]).unwrap();
        assert_eq!(load_task(&conn, &task_id).unwrap().total_bytes, None);
    }

    #[test]
    fn state_changes_are_checked_and_recorded() {
        let mut conn = open_in_memory().unwrap();
        let (id, _) = new_http_task(&mut conn);
        assert!(matches!(
            change_state(&mut conn, &id, &Change::to(TaskState::Completed)),
            Err(StoreError::BadTransition {
                from: "queued",
                to: "completed",
                ..
            })
        ));
        assert!(matches!(
            change_state(&mut conn, &id, &Change::to(TaskState::Paused)),
            Err(StoreError::BadValue(_))
        ));
        change_state(&mut conn, &id, &Change::to(TaskState::Active)).unwrap();
        let retry = Change {
            error: Some(("NET_TIMEOUT", "délai dépassé")),
            next_retry_at: Some(42),
            ..Change::to(TaskState::RetryWait)
        };
        change_state(&mut conn, &id, &retry).unwrap();
        let task = load_task(&conn, &id).unwrap();
        assert_eq!(task.state, TaskState::RetryWait);
        assert_eq!(task.retry_count, 1);
        assert_eq!(task.error_code.as_deref(), Some("NET_TIMEOUT"));

        change_state(&mut conn, &id, &Change::to(TaskState::Active)).unwrap();
        let pause = Change {
            pause_reason: Some(PauseReason::FileChanged),
            ..Change::to(TaskState::Paused)
        };
        assert_eq!(
            change_state(&mut conn, &id, &pause).unwrap(),
            TaskState::Active
        );
        let task = load_task(&conn, &id).unwrap();
        assert_eq!(task.pause_reason.as_deref(), Some("file_changed"));
        assert_eq!(task.error_code, None);
        assert_eq!(task.retry_count, 1, "compté jusqu'à la relance manuelle");

        change_state(&mut conn, &id, &Change::to(TaskState::Queued)).unwrap();
        let task = load_task(&conn, &id).unwrap();
        assert_eq!((task.retry_count, task.pause_reason), (0, None));
    }

    #[test]
    fn completion_writes_history_then_purges_segments_and_sources() {
        let mut conn = open_in_memory().unwrap();
        let (id, source_id) = new_http_task(&mut conn);
        let prep = Preparation {
            source_id: &source_id,
            accepts_ranges: true,
            etag: None,
            last_modified: None,
            total_bytes: Some(100),
            file_name: "f.iso",
        };
        save_preparation(&mut conn, &id, &prep, &initial_split(100, 2, 1)).unwrap();
        assert!(matches!(
            complete_task(&mut conn, &id, "/dl/f.iso", 100),
            Err(StoreError::BadTransition { from: "queued", .. })
        ));
        change_state(&mut conn, &id, &Change::to(TaskState::Active)).unwrap();
        complete_task(&mut conn, &id, "/dl/f.iso", 100).unwrap();

        let task = load_task(&conn, &id).unwrap();
        assert_eq!(task.state, TaskState::Completed);
        assert_eq!(task.done_bytes, 100);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM segments"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM sources"), 0);
        let (path, host, state): (String, String, String) = conn
            .query_row(
                "SELECT final_path, source_host, final_state FROM history WHERE task_id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (path.as_str(), host.as_str(), state.as_str()),
            ("/dl/f.iso", "exemple.org", "completed")
        );
    }

    #[test]
    fn only_finished_tasks_are_removed_and_their_history_stays() {
        let mut conn = open_in_memory().unwrap();
        let history = |conn: &Connection, id: &str| -> Vec<(String, Option<String>, String)> {
            let mut stmt = conn
                .prepare(
                    "SELECT final_state, final_path, source_host FROM history WHERE task_id = ?1",
                )
                .unwrap();
            stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };

        // En file, en cours, en pause : refusées, rien ne change.
        let (busy, _) = new_http_task(&mut conn);
        for change in [
            None,
            Some(Change::to(TaskState::Active)),
            Some(Change {
                pause_reason: Some(PauseReason::User),
                ..Change::to(TaskState::Paused)
            }),
        ] {
            if let Some(change) = change {
                change_state(&mut conn, &busy, &change).unwrap();
            }
            assert!(matches!(
                remove_task(&mut conn, &busy),
                Err(StoreError::BadTransition { .. })
            ));
            assert!(load_task(&conn, &busy).is_ok());
        }
        assert!(matches!(
            remove_task(&mut conn, "inconnue"),
            Err(StoreError::TaskNotFound { .. })
        ));

        // Terminée : sa ligne d'historique existe déjà, elle n'est pas doublée.
        let (done, source_id) = new_http_task(&mut conn);
        let prep = Preparation {
            source_id: &source_id,
            accepts_ranges: true,
            etag: None,
            last_modified: None,
            total_bytes: Some(10),
            file_name: "f.iso",
        };
        save_preparation(&mut conn, &done, &prep, &initial_split(10, 1, 1)).unwrap();
        change_state(&mut conn, &done, &Change::to(TaskState::Active)).unwrap();
        complete_task(&mut conn, &done, "/dl/f.iso", 10).unwrap();
        remove_task(&mut conn, &done).unwrap();
        assert!(matches!(
            load_task(&conn, &done),
            Err(StoreError::TaskNotFound { .. })
        ));
        assert_eq!(
            history(&conn, &done),
            [(
                "completed".into(),
                Some("/dl/f.iso".into()),
                "exemple.org".into()
            )]
        );

        // En échec, puis annulée : chacune reçoit sa ligne au retrait ; sources et
        // segments partent avec la tâche.
        for (to, error) in [
            (TaskState::Failed, Some(("HTTP_STATUS", "404"))),
            (TaskState::Cancelled, None),
        ] {
            let (id, source_id) = new_http_task(&mut conn);
            let prep = Preparation {
                source_id: &source_id,
                accepts_ranges: true,
                etag: None,
                last_modified: None,
                total_bytes: Some(10),
                file_name: "g.iso",
            };
            save_preparation(&mut conn, &id, &prep, &initial_split(10, 2, 1)).unwrap();
            change_state(&mut conn, &id, &Change::to(TaskState::Active)).unwrap();
            change_state(
                &mut conn,
                &id,
                &Change {
                    error,
                    ..Change::to(to)
                },
            )
            .unwrap();
            remove_task(&mut conn, &id).unwrap();
            assert_eq!(
                history(&conn, &id),
                [(to.as_sql().into(), None, "exemple.org".into())]
            );
            let left = |table: &str| {
                count(
                    &conn,
                    &format!("SELECT COUNT(*) FROM {table} WHERE task_id = '{id}'"),
                )
            };
            assert_eq!((left("sources"), left("segments")), (0, 0));
        }
        assert!(load_task(&conn, &busy).is_ok(), "les autres restent");
    }
}
