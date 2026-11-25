//! Tâches et sources dans la base : création, préparation, changements d'état (SPEC §5.3
//! et §5.5) et fin de vie (SPEC §5.6, règle 4).
//!
//! Chaque changement d'état est vérifié contre la table de la SPEC §5.5, dans la même
//! transaction que l'écriture : une transition interdite n'est jamais enregistrée.

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use super::headers::{self, StoredHeader};
use super::segments::set_size_and_segments;
use super::{StoreError, new_id, now_ms};
use crate::io::checksum::Checksum;
use crate::segmenter::Segment;

/// Type d'une tâche (colonne `kind`, SPEC §5.3) : ceux que ce moteur crée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Http,
    /// Torrent (SPEC §7.16) : lien magnet ou fichier `.torrent`.
    Torrent,
    /// Vidéo (SPEC §7.17) : page lue par yt-dlp, flux téléchargés puis fusionnés.
    Media,
}

impl TaskKind {
    pub fn as_sql(self) -> &'static str {
        match self {
            TaskKind::Http => "http",
            TaskKind::Torrent => "torrent",
            TaskKind::Media => "media",
        }
    }

    pub fn from_sql(s: &str) -> Option<Self> {
        match s {
            "http" => Some(TaskKind::Http),
            "torrent" => Some(TaskKind::Torrent),
            "media" => Some(TaskKind::Media),
            _ => None,
        }
    }
}

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
                    Paused
                        | RetryWait
                        | Verifying
                        | Seeding
                        // Fusion des flux d'une vidéo (SPEC §7.17, depuis le 2026-11-09).
                        | PostProcessing
                        | Failed
                        | Cancelled
                )
                | (Paused, Queued | Cancelled)
                | (RetryWait, Active | Paused | Failed | Cancelled)
                // `paused` : arrêt pendant le calcul de l'empreinte (depuis le 2026-11-08).
                // `active` : morceaux faux à télécharger de nouveau (F-12, 2026-11-17).
                | (Verifying, Active | Paused | PostProcessing | Completed | Failed)
                // `seeding` : torrent dont la commande de fin a réussi (SPEC §7.21).
                | (PostProcessing, Completed | Failed | Seeding)
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
    pub kind: TaskKind,
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
    /// Création de la tâche, en millisecondes Unix.
    pub created_at: i64,
    /// Fin (terminée, en échec ou annulée), en millisecondes Unix ; `None` avant.
    pub finished_at: Option<i64>,
    /// Nom choisi par l'utilisateur (« Déplacer/Renommer », `fileName` de `task.add`) ;
    /// `None` : celui du serveur.
    pub requested_name: Option<String>,
    /// Son fichier est en cours de déplacement (SPEC §7.12) : elle ne démarre pas, et ne
    /// se retire, ne s'annule ni ne se retélécharge avant la fin.
    pub moving: bool,
    /// Empreinte attendue du fichier (F-11), vérifiée avant le renommage du `.part`.
    pub checksum: Option<Checksum>,
    /// Fichier voisin où le moteur a lu cette empreinte (« SHA256SUMS »…) ; `None` : donnée
    /// par l'utilisateur (SPEC §7.9).
    pub checksum_source: Option<String>,
    /// Ajoutée par une connexion à jeton : réseau public seulement, pour toute sa vie
    /// (SPEC §7.19).
    pub public_only: bool,
    /// Sa préparation a suivi une redirection de HTTPS vers HTTP : le fichier arrive sans
    /// chiffrement (SPEC §8.1, migration 15).
    pub insecure_redirect: bool,
}

/// Une source de téléchargement : la référence (rang 0) ou un miroir (SPEC §7.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRow {
    pub id: String,
    pub url: String,
    pub accepts_ranges: Option<bool>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: Option<u64>,
    /// Ordre de préférence ; 0 pour la référence.
    pub rank: u32,
    pub state: SourceState,
    /// Débit moyen lissé, octets par seconde (étape 1b).
    pub avg_bps: Option<u64>,
    /// Échecs consécutifs.
    pub fail_count: u32,
    /// Raison du dernier échec, ou du refus d'un miroir.
    pub last_error: Option<String>,
    /// Morceaux faux envoyés (F-12, SPEC §7.9) : à 2, la source est écartée pour sa tâche.
    pub bad_pieces: u32,
}

/// État d'une source (SPEC §5.3 et §7.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceState {
    /// Pas encore vérifiée : un miroir attend sa validation.
    Unknown,
    /// Vérifiée, utilisable.
    Ok,
    /// Trop lente : plus de nouveaux segments, sans être coupée.
    Slow,
    /// En échec, ou miroir refusé : jamais utilisée tant qu'elle l'est.
    Failed,
}

impl SourceState {
    pub fn as_sql(self) -> &'static str {
        match self {
            SourceState::Unknown => "unknown",
            SourceState::Ok => "ok",
            SourceState::Slow => "slow",
            SourceState::Failed => "failed",
        }
    }

    pub fn from_sql(text: &str) -> Result<SourceState, StoreError> {
        Ok(match text {
            "unknown" => SourceState::Unknown,
            "ok" => SourceState::Ok,
            "slow" => SourceState::Slow,
            "failed" => SourceState::Failed,
            other => return Err(StoreError::BadValue(format!("état de source {other}"))),
        })
    }
}

fn to_u64(value: Option<i64>, what: &str) -> Result<Option<u64>, StoreError> {
    value
        .map(|v| u64::try_from(v).map_err(|_| StoreError::BadValue(format!("{what} négatif"))))
        .transpose()
}

/// Limite de débit à enregistrer : strictement positive, ou absente.
fn limit_to_sql(limit: Option<u64>) -> Result<Option<i64>, StoreError> {
    match limit {
        Some(0) => Err(StoreError::BadValue("limite de débit nulle".into())),
        limit => limit
            .map(|l| super::to_i64(l, "limite de débit"))
            .transpose(),
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

/// Change l'empreinte attendue d'une tâche (F-11) ; `None` la retire. `source` : le fichier
/// voisin où le moteur l'a lue, `None` si elle vient de l'utilisateur. L'appelant vérifie
/// l'état : l'empreinte se lit à la fin du téléchargement.
pub fn set_checksum(
    conn: &Connection,
    task_id: &str,
    checksum: Option<&Checksum>,
    source: Option<&str>,
) -> Result<(), StoreError> {
    let updated = conn.execute(
        "UPDATE tasks SET checksum_algo = ?2, checksum_value = ?3, checksum_source = ?4,
           updated_at = ?5
         WHERE id = ?1",
        params![
            task_id,
            checksum.map(|c| c.algo().as_sql()),
            checksum.map(Checksum::value),
            checksum.and(source),
            now_ms()
        ],
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
    /// Clé d'idempotence de `task.add` ; unique parmi les tâches présentes.
    pub idempotency_key: Option<&'a str>,
    /// Nom choisi par l'utilisateur (`fileName` de `task.add`), déjà vérifié ; `None` :
    /// celui du serveur, appris à la préparation.
    pub requested_name: Option<&'a str>,
    /// Empreinte attendue du fichier (F-11).
    pub checksum: Option<&'a Checksum>,
    /// En-têtes HTTP de la tâche ; les secrets sont déjà dans le coffre.
    pub headers: &'a [StoredHeader],
    /// Tâche interne d'une vidéo (flux direct, SPEC §7.17) : sa tâche vidéo. Jamais lancée
    /// par le planificateur, absente des listes et de l'historique.
    pub parent_id: Option<&'a str>,
    /// Ajoutée par une connexion à jeton : réseau public seulement (SPEC §7.19).
    pub public_only: bool,
}

/// Départ d'une tâche créée, enregistré dans la transaction de sa création (audit P14 :
/// un arrêt entre les deux la laissait partir sans pause, ou hors des règles).
#[derive(Debug, Clone, Copy, Default)]
pub struct Start {
    /// `task.add` avec `paused` : en pause (cause `user`) dès sa création.
    pub paused: bool,
    /// Ajoutée sans dossier choisi : les règles la rangeront (`rule_base`, SPEC §7.21).
    pub ruled: bool,
}

/// Applique `start` à une tâche que la transaction `tx` vient de créer.
pub(crate) fn apply_start(tx: &Connection, task_id: &str, start: Start) -> Result<(), StoreError> {
    if start.ruled {
        tx.execute(
            "UPDATE tasks SET rule_base = dest_dir WHERE id = ?1",
            [task_id],
        )?;
    }
    if start.paused {
        // Créée en pause : comme `queued` suivi de la pause de l'utilisateur (SPEC §5.5),
        // en une seule écriture.
        tx.execute(
            "UPDATE tasks SET state = 'paused', pause_reason = 'user' WHERE id = ?1",
            [task_id],
        )?;
    }
    Ok(())
}

/// Crée une tâche `queued` et sa source ; renvoie leurs identifiants.
pub fn create_task(
    conn: &mut Connection,
    new: &NewTask<'_>,
) -> Result<(String, String), StoreError> {
    create_task_with_pieces(conn, new, &[], None, Start::default())
}

/// Crée une tâche `queued`, sa source de référence et ses miroirs (rangs 1 et plus, à
/// valider, SPEC §7.7), dans une seule transaction ; renvoie les identifiants de la tâche et
/// de la référence. Une adresse en double est refusée. Tests seulement : l'ajout passe par
/// [`create_task_with_pieces`] (audit S16).
#[cfg(test)]
pub fn create_task_with_mirrors(
    conn: &mut Connection,
    new: &NewTask<'_>,
    mirrors: &[String],
) -> Result<(String, String), StoreError> {
    create_task_with_pieces(conn, new, mirrors, None, Start::default())
}

/// Crée une tâche `queued`, sa source de référence et ses miroirs (SPEC §7.7), avec les
/// empreintes de morceaux d'un Metalink (F-12, SPEC §7.9) enregistrées dans la même
/// transaction ; renvoie les identifiants de la tâche et de la référence.
pub fn create_task_with_pieces(
    conn: &mut Connection,
    new: &NewTask<'_>,
    mirrors: &[String],
    pieces: Option<&crate::pieces::Pieces>,
    start: Start,
) -> Result<(String, String), StoreError> {
    if 1 + mirrors.len() > super::sources::MAX_SOURCES {
        return Err(StoreError::BadValue(format!(
            "{} sources au plus par tâche",
            super::sources::MAX_SOURCES
        )));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (task_id, source_id, now) = (new_id(), new_id(), now_ms());
    let limit = limit_to_sql(new.speed_limit_bps)?;
    tx.execute(
        "INSERT INTO tasks (id, queue_id, kind, state, position, display_name, dest_dir,
                            max_connections, speed_limit_bps, idempotency_key, requested_name,
                            checksum_algo, checksum_value, created_at, updated_at, parent_id,
                            public_only)
         VALUES (?1, ?2, 'http', 'queued',
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE queue_id = ?2),
                 ?3, ?4, ?5, ?7, ?8, ?9, ?10, ?11, ?6, ?6, ?12, ?13)",
        params![
            task_id,
            new.queue_id,
            new.display_name,
            new.dest_dir,
            new.max_connections,
            now,
            limit,
            new.idempotency_key,
            new.requested_name,
            new.checksum.map(|c| c.algo().as_sql()),
            new.checksum.map(Checksum::value),
            new.parent_id,
            new.public_only
        ],
    )?;
    tx.execute(
        "INSERT INTO sources (id, task_id, url) VALUES (?1, ?2, ?3)",
        params![source_id, task_id, new.url],
    )?;
    for (rank, url) in mirrors.iter().enumerate() {
        super::sources::insert(&tx, &task_id, url, rank as u32 + 1)?;
    }
    headers::insert(&tx, &task_id, new.headers)?;
    if let Some(pieces) = pieces {
        super::pieces::save_in(&tx, &task_id, pieces)?;
    }
    apply_start(&tx, &task_id, start)?;
    tx.commit()?;
    Ok((task_id, source_id))
}

const TASK_COLUMNS: &str = "id, state, display_name, dest_dir, file_name, total_bytes, \
     done_bytes, retry_count, pause_reason, error_code, error_detail, max_connections, \
     queue_id, priority, speed_limit_bps, created_at, finished_at, requested_name, \
     EXISTS (SELECT 1 FROM file_moves m WHERE m.task_id = tasks.id), checksum_algo, \
     checksum_value, checksum_source, kind, public_only, insecure_redirect";

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
    created_at: i64,
    finished_at: Option<i64>,
    requested_name: Option<String>,
    moving: bool,
    checksum_algo: Option<String>,
    checksum_value: Option<String>,
    checksum_source: Option<String>,
    kind: String,
    public_only: bool,
    insecure_redirect: bool,
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
        created_at: row.get(15)?,
        finished_at: row.get(16)?,
        requested_name: row.get(17)?,
        moving: row.get(18)?,
        checksum_algo: row.get(19)?,
        checksum_value: row.get(20)?,
        checksum_source: row.get(21)?,
        kind: row.get(22)?,
        public_only: row.get(23)?,
        insecure_redirect: row.get(24)?,
    })
}

impl TryFrom<RawTask> for TaskRow {
    type Error = StoreError;

    fn try_from(raw: RawTask) -> Result<Self, StoreError> {
        let bad = |what: &str| StoreError::BadValue(format!("tâche {} : {what}", raw.id));
        let checksum = match (&raw.checksum_algo, &raw.checksum_value) {
            (Some(algo), Some(value)) => {
                Some(Checksum::parse(algo, value).map_err(|_| bad("empreinte invalide"))?)
            }
            (None, None) => None,
            _ => return Err(bad("empreinte incomplète")),
        };
        Ok(TaskRow {
            checksum_source: raw.checksum_source.filter(|_| checksum.is_some()),
            public_only: raw.public_only,
            insecure_redirect: raw.insecure_redirect,
            checksum,
            kind: TaskKind::from_sql(&raw.kind).ok_or_else(|| bad("type inconnu"))?,
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
            created_at: raw.created_at,
            finished_at: raw.finished_at,
            requested_name: raw.requested_name,
            moving: raw.moving,
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
        "SELECT {TASK_COLUMNS} FROM tasks WHERE parent_id IS NULL ORDER BY created_at, id"
    ))?;
    let rows = stmt.query_map([], raw_task)?;
    rows.map(|r| r?.try_into()).collect()
}

const SOURCE_COLUMNS: &str = "id, url, accepts_ranges, etag, last_modified, content_length, \
     rank, state, avg_bps, fail_count, last_error, bad_pieces";

fn raw_source(r: &Row<'_>) -> rusqlite::Result<RawSource> {
    Ok(RawSource {
        id: r.get(0)?,
        url: r.get(1)?,
        ranges: r.get(2)?,
        etag: r.get(3)?,
        last_modified: r.get(4)?,
        length: r.get(5)?,
        rank: r.get(6)?,
        state: r.get(7)?,
        avg_bps: r.get(8)?,
        fail_count: r.get(9)?,
        last_error: r.get(10)?,
        bad_pieces: r.get(11)?,
    })
}

/// Une ligne de `sources` telle que SQLite la rend, avant contrôle des valeurs.
struct RawSource {
    id: String,
    url: String,
    ranges: Option<i64>,
    etag: Option<String>,
    last_modified: Option<String>,
    length: Option<i64>,
    rank: i64,
    state: String,
    avg_bps: Option<i64>,
    fail_count: i64,
    last_error: Option<String>,
    bad_pieces: i64,
}

fn source_row(raw: RawSource) -> Result<SourceRow, StoreError> {
    Ok(SourceRow {
        id: raw.id,
        url: raw.url,
        accepts_ranges: raw.ranges.map(|r| r != 0),
        etag: raw.etag,
        last_modified: raw.last_modified,
        content_length: to_u64(raw.length, "taille de la source")?,
        rank: u32::try_from(raw.rank)
            .map_err(|_| StoreError::BadValue(format!("rang de source {}", raw.rank)))?,
        state: SourceState::from_sql(&raw.state)?,
        avg_bps: to_u64(raw.avg_bps, "débit de la source")?,
        fail_count: u32::try_from(raw.fail_count).unwrap_or(u32::MAX),
        last_error: raw.last_error,
        bad_pieces: u32::try_from(raw.bad_pieces).unwrap_or(u32::MAX),
    })
}

/// Hôte de la source préférée d'une tâche, pour l'historique : le nom d'hôte seulement,
/// l'adresse complète pouvant porter un jeton (SPEC §8.2 ; une seule lecture, audit S12).
fn reference_host(conn: &Connection, task_id: &str) -> Result<Option<String>, StoreError> {
    let url: Option<String> = conn
        .query_row(
            "SELECT url FROM sources WHERE task_id = ?1 ORDER BY rank, id LIMIT 1",
            [task_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(url.as_deref().and_then(crate::net::host_of))
}

/// Source de rang le plus faible (la préférée) d'une tâche.
pub fn primary_source(conn: &Connection, task_id: &str) -> Result<SourceRow, StoreError> {
    let raw = conn
        .query_row(
            &format!(
                "SELECT {SOURCE_COLUMNS} FROM sources WHERE task_id = ?1 ORDER BY rank, id LIMIT 1"
            ),
            [task_id],
            raw_source,
        )
        .optional()?
        .ok_or_else(|| StoreError::NoSource {
            task_id: task_id.to_owned(),
        })?;
    source_row(raw)
}

/// Sources d'une tâche, de la préférée à la dernière. Elles restent après sa fin, jusqu'à
/// son retrait (section 5.6, règle 4) ; une tâche terminée avant le 2026-11-07 n'en a plus.
pub fn load_sources(conn: &Connection, task_id: &str) -> Result<Vec<SourceRow>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {SOURCE_COLUMNS} FROM sources WHERE task_id = ?1 ORDER BY rank, id"
    ))?;
    let rows = stmt.query_map([task_id], raw_source)?;
    rows.map(|r| source_row(r?)).collect()
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
#[cfg(test)]
pub fn save_preparation(
    conn: &mut Connection,
    task_id: &str,
    prep: &Preparation<'_>,
    segments: &[Segment],
) -> Result<(), StoreError> {
    save_preparation_placed(conn, task_id, prep, segments, &Default::default())
}

/// [`save_preparation`], avec le rangement des règles (SPEC §7.21) dans la même
/// transaction : le dossier où le nom a été pris ne s'enregistre qu'avec lui.
pub fn save_preparation_placed(
    conn: &mut Connection,
    task_id: &str,
    prep: &Preparation<'_>,
    segments: &[Segment],
    placed: &super::rules::Placed<'_>,
) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    super::rules::save_placement(&tx, task_id, placed)?;
    let length = prep
        .total_bytes
        .map(|t| super::to_i64(t, "taille"))
        .transpose()?;
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
    // Le nom est enregistré : sa réservation part avec lui (SPEC §5.6, règle 7).
    super::claims::release(&tx, task_id)?;
    tx.commit()?;
    Ok(())
}

/// Ce que le moteur a appris de la nouvelle adresse d'une tâche préparée (« Actualiser
/// l'adresse », SPEC §7.13).
#[derive(Debug, Clone, Copy)]
pub struct Revalidation<'a> {
    pub accepts_ranges: bool,
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
    pub total_bytes: Option<u64>,
}

/// La tâche ne joindra plus que le réseau public (SPEC §7.19) : un jeton y a donné une
/// adresse ou un miroir. Jamais l'inverse.
pub fn restrict_to_public(conn: &Connection, task_id: &str) -> Result<(), StoreError> {
    let changed = conn.execute("UPDATE tasks SET public_only = 1 WHERE id = ?1", [task_id])?;
    if changed == 0 {
        return Err(StoreError::TaskNotFound {
            task_id: task_id.to_owned(),
        });
    }
    Ok(())
}

/// Note la page et le plugin d'où vient le lien d'une tâche (SPEC §7.20).
pub fn set_plugin_origin(
    conn: &Connection,
    task_id: &str,
    page: &str,
    plugin_id: &str,
) -> Result<(), StoreError> {
    let changed = conn.execute(
        "UPDATE tasks SET plugin_page = ?2, plugin_id = ?3 WHERE id = ?1",
        params![task_id, page, plugin_id],
    )?;
    if changed == 0 {
        return Err(StoreError::TaskNotFound {
            task_id: task_id.to_owned(),
        });
    }
    Ok(())
}

/// La page et le plugin d'où vient le lien d'une tâche, s'il vient d'un plugin.
pub fn plugin_origin(
    conn: &Connection,
    task_id: &str,
) -> Result<Option<(String, String)>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT plugin_page, plugin_id FROM tasks
             WHERE id = ?1 AND plugin_page IS NOT NULL AND plugin_id IS NOT NULL",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Remplace l'adresse de la source préférée d'une tâche, et oublie ses échecs. Avec
/// `check`, les validateurs de la source deviennent ceux de la nouvelle adresse, que les
/// requêtes suivantes vérifieront (`If-Range`) ; sans (tâche pas encore préparée), la
/// préparation les apprendra. `display_name` : nouveau nom affiché, s'il change.
pub fn replace_url(
    conn: &mut Connection,
    task_id: &str,
    url: &str,
    check: Option<&Revalidation<'_>>,
    display_name: Option<&str>,
) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let source_id: String = tx
        .query_row(
            "SELECT id FROM sources WHERE task_id = ?1 ORDER BY rank, id LIMIT 1",
            [task_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| StoreError::NoSource {
            task_id: task_id.to_owned(),
        })?;
    tx.execute(
        "UPDATE sources SET url = ?2, fail_count = 0, last_error = NULL WHERE id = ?1",
        params![source_id, url],
    )?;
    if let Some(check) = check {
        let length = check
            .total_bytes
            .map(|t| super::to_i64(t, "taille"))
            .transpose()?;
        tx.execute(
            "UPDATE sources SET accepts_ranges = ?2, etag = ?3, last_modified = ?4,
                                content_length = ?5, state = 'ok'
             WHERE id = ?1",
            params![
                source_id,
                check.accepts_ranges,
                check.etag,
                check.last_modified,
                length
            ],
        )?;
    }
    if let Some(name) = display_name {
        tx.execute(
            "UPDATE tasks SET display_name = ?2 WHERE id = ?1",
            params![task_id, name],
        )?;
    }
    tx.execute(
        "UPDATE tasks SET updated_at = ?2 WHERE id = ?1",
        params![task_id, now_ms()],
    )?;
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

/// Relance par l'utilisateur d'une tâche `paused` ou `failed` (SPEC §5.5) : passe en
/// `queued` et redonne ses tentatives à la tâche (SPEC §7.8) et ses tours aux morceaux
/// faux (F-12, SPEC §7.9), dans une seule transaction. Les remises en file automatiques
/// passent par [`change_state`], qui les garde.
pub fn change_state_by_user(conn: &mut Connection, task_id: &str) -> Result<TaskState, StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let from = change_state_in(&tx, task_id, &Change::to(TaskState::Queued))?;
    tx.execute("UPDATE tasks SET retry_count = 0 WHERE id = ?1", [task_id])?;
    tx.execute(
        "UPDATE task_pieces SET rounds = 0 WHERE task_id = ?1",
        [task_id],
    )?;
    tx.commit()?;
    Ok(from)
}

/// Change l'état d'une tâche si la SPEC §5.5 le permet ; renvoie l'état quitté.
///
/// `retry_count` compte les passages en `retry_wait` depuis la dernière relance par
/// l'utilisateur ([`change_state_by_user`]).
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
    let from = change_state_in(&tx, task_id, change)?;
    tx.commit()?;
    Ok(from)
}

/// Corps de [`change_state`], dans la transaction de l'appelant (cause de pause déjà
/// contrôlée).
pub(super) fn change_state_in(
    tx: &Connection,
    task_id: &str,
    change: &Change<'_>,
) -> Result<TaskState, StoreError> {
    let from = check_transition(tx, task_id, change.to)?;
    let to = change.to;
    let (code, detail) = change.error.unzip();
    // Les remises en file automatiques (arrêt du moteur, plage, place revenue, reprise
    // après un plantage) gardent les tentatives et les tours des morceaux faux : seule une
    // relance de l'utilisateur les redonne ([`change_state_by_user`], audit S4).
    tx.execute(
        "UPDATE tasks SET
           state = ?2, pause_reason = ?3, error_code = ?4, error_detail = ?5,
           next_retry_at = ?6,
           retry_count = CASE ?2 WHEN 'retry_wait' THEN retry_count + 1 ELSE retry_count END,
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
    Ok(from)
}

/// Fin d'une tâche réussie (SPEC §5.6, règle 4) : depuis `active` (ou `verifying`), passe
/// par `verifying` puis `completed`, écrit la ligne `history`, puis purge les segments, dans
/// une seule transaction ; les sources (l'adresse) restent jusqu'au retrait. À appeler
/// après le renommage du fichier.
///
/// Sans empreinte à contrôler, `verifying` est traversé dans cette transaction ; avec une
/// empreinte (F-11), la tâche y est déjà, pendant le calcul.
/// Taille d'une tâche dont le fichier est complet (commande de fin en attente,
/// SPEC §7.21) : tous ses octets sont là.
pub fn set_finished_size(conn: &Connection, task_id: &str, total: u64) -> Result<(), StoreError> {
    let total = super::to_i64(total, "taille")?;
    conn.execute(
        "UPDATE tasks SET total_bytes = ?2, done_bytes = ?2, updated_at = ?3 WHERE id = ?1",
        params![task_id, total, now_ms()],
    )?;
    Ok(())
}

pub fn complete_task(
    conn: &mut Connection,
    task_id: &str,
    final_path: &str,
    total_bytes: u64,
) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Un torrent finit son partage (`seeding` → `completed`, SPEC §7.16).
    let seeding = load_task(&tx, task_id)?.state == TaskState::Seeding;
    let from = match check_transition(&tx, task_id, TaskState::Verifying) {
        Ok(from) => from,
        Err(StoreError::BadTransition {
            from: "verifying", ..
        }) => TaskState::Verifying,
        // Vidéo dont les flux sont fusionnés (SPEC §7.17).
        Err(StoreError::BadTransition {
            from: "post_processing",
            ..
        }) => TaskState::PostProcessing,
        Err(_) if seeding => TaskState::Seeding,
        Err(e) => return Err(e),
    };
    debug_assert!(matches!(
        from,
        TaskState::Active | TaskState::Verifying | TaskState::Seeding | TaskState::PostProcessing
    ));
    let total = super::to_i64(total_bytes, "taille")?;
    let now = now_ms();

    let host = reference_host(&tx, task_id)?;
    tx.execute(
        "INSERT INTO history (id, task_id, display_name, final_path, total_bytes, source_host,
                              final_state, sha256, finished_at)
         SELECT ?2, id, display_name, ?3, ?4, ?5, 'completed',
                -- Empreinte vérifiée (F-11) : seule SHA-256 a sa colonne.
                CASE WHEN checksum_algo = 'sha256' THEN checksum_value END, ?6
         FROM tasks WHERE id = ?1
           -- Une tâche interne d'une vidéo n'a pas d'historique : sa vidéo en a un.
           AND parent_id IS NULL",
        params![task_id, new_id(), final_path, total, host, now],
    )?;
    tx.execute(
        "UPDATE tasks SET state = 'completed', total_bytes = ?2, done_bytes = ?2,
           pause_reason = NULL, error_code = NULL, error_detail = NULL, next_retry_at = NULL,
           retry_count = 0, finished_at = ?3, updated_at = ?3
         WHERE id = ?1",
        params![task_id, total, now],
    )?;
    // Les sources restent (depuis le 2026-11-07) : « Retélécharger » en a besoin.
    super::segments::clear(&tx, task_id)?;
    tx.commit()?;
    Ok(())
}

/// Retéléchargement (SPEC §5.5, depuis le 2026-11-07) : une tâche terminée, en échec,
/// annulée, en pause ou en attente d'un nouvel essai repart de zéro, en `queued`. Sa
/// préparation est oubliée (nom, taille, segments, validateurs de ses sources), comme
/// ses erreurs et ses tentatives, et une empreinte lue dans un fichier voisin (F-11) ; son
/// adresse, sa file, ses réglages et l'empreinte donnée par l'utilisateur restent. Renvoie
/// l'état quitté. Les fichiers ne sont pas touchés : le moteur supprime avant le `.part`,
/// et le fichier final si on le lui demande.
///
/// Hors de la table des transitions, qui décrit le cours d'un téléchargement : c'en est un
/// nouveau, pour la même tâche.
pub fn restart_task(conn: &mut Connection, task_id: &str) -> Result<TaskState, StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let task = load_task(&tx, task_id)?;
    if !matches!(
        task.state,
        TaskState::Completed
            | TaskState::Failed
            | TaskState::Cancelled
            | TaskState::Paused
            | TaskState::RetryWait
    ) {
        return Err(StoreError::BadTransition {
            task_id: task_id.to_owned(),
            from: task.state.as_sql(),
            to: "retéléchargée",
        });
    }
    let sources: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sources WHERE task_id = ?1",
        [task_id],
        |r| r.get(0),
    )?;
    if sources == 0 {
        return Err(StoreError::NoSource {
            task_id: task_id.to_owned(),
        });
    }
    super::segments::clear(&tx, task_id)?;
    // Fichiers d'un torrent : réappris à la préparation (SPEC §7.16) ; ses métadonnées
    // restent.
    tx.execute("DELETE FROM task_files WHERE task_id = ?1", [task_id])?;
    tx.execute(
        "UPDATE sources SET state = 'unknown', accepts_ranges = NULL, etag = NULL,
           last_modified = NULL, content_length = NULL, fail_count = 0, last_error = NULL,
           bad_pieces = 0
         WHERE task_id = ?1",
        [task_id],
    )?;
    // Morceaux faux (F-12) : un nouveau téléchargement, ses tours et son compte repartent.
    tx.execute(
        "UPDATE task_pieces SET rounds = 0, redone = 0 WHERE task_id = ?1",
        [task_id],
    )?;
    tx.execute(
        "UPDATE tasks SET state = 'queued', file_name = NULL, total_bytes = NULL,
           -- Rangée par une règle : de nouveau sous la base des règles, que la préparation
           -- range encore ; sans règle qui s'applique, la base elle-même, pas le dossier
           -- de l'ancienne règle (SPEC §7.21, audit S7).
           dest_dir = COALESCE(rule_base, dest_dir), insecure_redirect = 0,
           done_bytes = 0, pause_reason = NULL, error_code = NULL, error_detail = NULL,
           retry_count = 0, next_retry_at = NULL, started_at = NULL, finished_at = NULL,
           -- Une empreinte lue dans un fichier voisin a pu changer avec le fichier (F-11) ;
           -- celle de l'utilisateur reste.
           checksum_algo = CASE WHEN checksum_source IS NULL THEN checksum_algo END,
           checksum_value = CASE WHEN checksum_source IS NULL THEN checksum_value END,
           checksum_source = NULL,
           updated_at = ?2
         WHERE id = ?1",
        params![task_id, now_ms()],
    )?;
    tx.commit()?;
    Ok(task.state)
}

/// Tâche interne d'une vidéo (`parent_id`, SPEC §7.17) ? Une tâche absente ne l'est pas :
/// l'appelant dira qu'elle n'existe pas.
pub fn is_internal(conn: &Connection, task_id: &str) -> Result<bool, StoreError> {
    Ok(conn
        .query_row(
            "SELECT parent_id IS NOT NULL FROM tasks WHERE id = ?1",
            [task_id],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

/// Note si la préparation de la tâche a suivi une redirection de HTTPS vers HTTP (SPEC §8.1).
pub fn set_insecure_redirect(
    conn: &Connection,
    task_id: &str,
    insecure: bool,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE tasks SET insecure_redirect = ?2 WHERE id = ?1",
        params![task_id, insecure],
    )?;
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
    // Une fin `completed` a écrit sa ligne (`complete_task`) ; un échec ou une annulation,
    // jamais : une ligne plus ancienne est celle d'une fin d'avant un retéléchargement, et
    // celle-ci manquait à l'historique (audit S5).
    let recorded: bool = task.state == TaskState::Completed
        && tx
            .query_row(
                "SELECT 1 FROM history WHERE task_id = ?1",
                [task_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
    if !recorded {
        let host = reference_host(&tx, task_id)?;
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

/// Tâche créée avec cette clé d'idempotence, si elle existe encore.
pub fn task_by_idempotency_key(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT id FROM tasks WHERE idempotency_key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?)
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
    /// Torrent ou HTTP : un torrent attend son interface réseau (SPEC §7.16).
    pub kind: TaskKind,
}

/// Réseau revenu (SPEC §7.15) : les tâches qui attendent un nouvel essai après une erreur de
/// réseau (`NET_UNREACHABLE`, `NET_TIMEOUT`) peuvent repartir tout de suite, au lieu
/// d'attendre leur délai (jusqu'à 5 minutes). Un serveur qui a demandé d'attendre garde le
/// sien. Rend le nombre de tâches concernées ; leur état ne change pas.
pub fn retry_network_waits(conn: &Connection, now: i64) -> Result<usize, StoreError> {
    Ok(conn.execute(
        "UPDATE tasks SET next_retry_at = ?1
         WHERE state = 'retry_wait' AND error_code IN ('NET_UNREACHABLE', 'NET_TIMEOUT')
           AND COALESCE(next_retry_at, 0) > ?1",
        [now],
    )?)
}

/// Tâches qui peuvent partir à `now` (millisecondes Unix), dans l'ordre où le planificateur
/// les démarre : files dans leur ordre ; dans une file, d'abord les tâches `active`
/// (interrompues par un arrêt du moteur, SPEC §7.4), puis par priorité décroissante et
/// position. Une tâche `retry_wait` part quand sa date est passée (SPEC §7.8).
pub fn runnable_tasks(conn: &Connection, now: i64) -> Result<Vec<Candidate>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.queue_id, q.max_active, t.state, t.kind
         FROM tasks t JOIN queues q ON q.id = t.queue_id
         -- `verifying` : arrêt brutal pendant le calcul de l'empreinte, qui reprend.
         -- `seeding` : partage d'un torrent interrompu (arrêt du moteur, réseau), qui
         -- reprend (SPEC §7.16).
         -- `post_processing` : arrêt pendant la fusion d'une vidéo, qui la recommence (SPEC
         -- §7.17). Les tâches internes d'une vidéo ne partent qu'avec elle.
         WHERE t.parent_id IS NULL
           AND (t.state IN ('queued', 'active', 'verifying', 'seeding', 'post_processing')
                OR (t.state = 'retry_wait' AND COALESCE(t.next_retry_at, 0) <= ?1))
           -- Pendant le déplacement de son fichier, une tâche attend (SPEC §7.12).
           AND NOT EXISTS (SELECT 1 FROM file_moves m WHERE m.task_id = t.id)
         ORDER BY q.position, t.state = 'active' DESC, t.priority DESC, t.position, t.id",
    )?;
    let rows = stmt.query_map([now], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    rows.map(|row| {
        let (task_id, queue_id, max_active, state, kind) = row?;
        let bad = |what: &str| StoreError::BadValue(format!("tâche {task_id} : {what}"));
        Ok(Candidate {
            max_active: u32::try_from(max_active).map_err(|_| bad("limite de file invalide"))?,
            state: TaskState::from_sql(&state).ok_or_else(|| bad("état inconnu"))?,
            kind: TaskKind::from_sql(&kind).ok_or_else(|| bad("type inconnu"))?,
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
         WHERE parent_id IS NULL AND (?1 IS NULL OR queue_id = ?1) AND (?2 IS NULL OR id > ?2){state_clause}
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
    /// Planification de la file (F-16) ; `None` : elle télécharge à toute heure.
    pub schedule_id: Option<String>,
}

/// Toutes les files, dans leur ordre.
pub fn list_queues(conn: &Connection) -> Result<Vec<QueueRow>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, position, max_active, speed_limit_bps, schedule_id FROM queues
         ORDER BY position, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    rows.map(|row| {
        let (id, name, position, max_active, limit, schedule_id) = row?;
        Ok(QueueRow {
            max_active: u32::try_from(max_active)
                .map_err(|_| StoreError::BadValue(format!("file {id} : limite invalide")))?,
            speed_limit_bps: to_u64(limit, "limite de débit")?,
            id,
            name,
            position,
            schedule_id,
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

/// Attache une planification à une file, ou l'en détache (`None`) ; `false` si la file
/// n'existe pas. La planification doit exister (clé étrangère).
pub fn set_queue_schedule(
    conn: &Connection,
    queue_id: &str,
    schedule_id: Option<&str>,
) -> Result<bool, StoreError> {
    let updated = conn.execute(
        "UPDATE queues SET schedule_id = ?2 WHERE id = ?1",
        params![queue_id, schedule_id],
    )?;
    Ok(updated > 0)
}

/// Tâches d'une file qui ont encore à télécharger : en file, en cours, en attente d'une
/// nouvelle tentative, en vérification ou en finition. Les pauses n'en sont pas.
pub fn count_pending(conn: &Connection, queue_id: &str) -> Result<u64, StoreError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE queue_id = ?1 AND parent_id IS NULL AND state IN
           ('queued', 'active', 'retry_wait', 'verifying', 'post_processing')",
        [queue_id],
        |r| r.get(0),
    )?;
    Ok(n.unsigned_abs())
}

/// Date de la prochaine nouvelle tentative (millisecondes Unix) postérieure à `after`, s'il
/// y en a une : celles d'avant ont déjà été essayées par le planificateur.
pub fn next_retry_at(conn: &Connection, after: i64) -> Result<Option<i64>, StoreError> {
    Ok(conn.query_row(
        "SELECT MIN(next_retry_at) FROM tasks
         WHERE state = 'retry_wait' AND parent_id IS NULL AND next_retry_at > ?1",
        [after],
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
                idempotency_key: None,
                requested_name: None,
                checksum: None,
                headers: &[],
                parent_id: None,
                public_only: false,
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

        // Remise en file automatique (arrêt du moteur, plage, place revenue) : les
        // tentatives restent comptées (audit S4).
        change_state(&mut conn, &id, &Change::to(TaskState::Queued)).unwrap();
        let task = load_task(&conn, &id).unwrap();
        assert_eq!((task.retry_count, task.pause_reason), (1, None));

        // Relance de l'utilisateur : elles repartent de zéro.
        change_state(&mut conn, &id, &Change::to(TaskState::Active)).unwrap();
        change_state(&mut conn, &id, &pause).unwrap();
        change_state_by_user(&mut conn, &id).unwrap();
        let task = load_task(&conn, &id).unwrap();
        assert_eq!(task.state, TaskState::Queued);
        assert_eq!((task.retry_count, task.pause_reason), (0, None));
    }

    #[test]
    fn completion_writes_history_then_purges_segments_but_keeps_sources() {
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
        // L'adresse reste, pour « Retélécharger ».
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM sources"), 1);
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

        // Audit S5 : terminée, retéléchargée puis annulée : l'annulation a sa ligne à côté
        // de la fin d'avant.
        let (again, source_id) = new_http_task(&mut conn);
        let prep = Preparation {
            source_id: &source_id,
            accepts_ranges: true,
            etag: None,
            last_modified: None,
            total_bytes: Some(10),
            file_name: "h.iso",
        };
        save_preparation(&mut conn, &again, &prep, &initial_split(10, 1, 1)).unwrap();
        change_state(&mut conn, &again, &Change::to(TaskState::Active)).unwrap();
        complete_task(&mut conn, &again, "/dl/h.iso", 10).unwrap();
        restart_task(&mut conn, &again).unwrap();
        change_state(&mut conn, &again, &Change::to(TaskState::Cancelled)).unwrap();
        remove_task(&mut conn, &again).unwrap();
        let states: Vec<String> = history(&conn, &again).into_iter().map(|h| h.0).collect();
        assert_eq!(states, ["completed", "cancelled"]);
        assert!(load_task(&conn, &busy).is_ok(), "les autres restent");
    }

    #[test]
    fn a_finished_task_restarts_from_zero_with_its_address() {
        let mut conn = open_in_memory().unwrap();
        let (id, source_id) = new_http_task(&mut conn);
        let prep = Preparation {
            source_id: &source_id,
            accepts_ranges: true,
            etag: Some("\"v1\""),
            last_modified: None,
            total_bytes: Some(100),
            file_name: "f.iso",
        };
        save_preparation(&mut conn, &id, &prep, &initial_split(100, 2, 1)).unwrap();
        // En file ou en cours : rien à retélécharger.
        assert!(matches!(
            restart_task(&mut conn, &id),
            Err(StoreError::BadTransition { from: "queued", .. })
        ));
        change_state(&mut conn, &id, &Change::to(TaskState::Active)).unwrap();
        assert!(restart_task(&mut conn, &id).is_err());
        complete_task(&mut conn, &id, "/dl/f.iso", 100).unwrap();

        assert_eq!(restart_task(&mut conn, &id).unwrap(), TaskState::Completed);
        let task = load_task(&conn, &id).unwrap();
        assert_eq!(task.state, TaskState::Queued);
        assert_eq!((task.file_name, task.total_bytes), (None, None));
        assert_eq!((task.done_bytes, task.finished_at), (0, None));
        let source = primary_source(&conn, &id).unwrap();
        assert_eq!(source.url, "https://exemple.org/f.iso?token=secret");
        assert_eq!((source.etag, source.accepts_ranges), (None, None));
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM segments"), 0);
        // La fin précédente reste dans l'historique.
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM history"), 1);

        // Une tâche en échec repart aussi, erreur et tentatives effacées.
        change_state(&mut conn, &id, &Change::to(TaskState::Active)).unwrap();
        let failed = Change {
            error: Some(("HTTP_STATUS", "404")),
            ..Change::to(TaskState::Failed)
        };
        change_state(&mut conn, &id, &failed).unwrap();
        restart_task(&mut conn, &id).unwrap();
        let task = load_task(&conn, &id).unwrap();
        assert_eq!((task.state, task.error_code), (TaskState::Queued, None));

        // Audit S7 : rangée par une règle, elle revient sous la base des règles (la
        // préparation la range de nouveau, ou l'y laisse si plus aucune règle ne la prend).
        conn.execute(
            "UPDATE tasks SET rule_base = '/dl', dest_dir = '/dl/Rangé' WHERE id = ?1",
            [&id],
        )
        .unwrap();
        change_state(&mut conn, &id, &Change::to(TaskState::Cancelled)).unwrap();
        restart_task(&mut conn, &id).unwrap();
        assert_eq!(load_task(&conn, &id).unwrap().dest_dir, "/dl");
        // Ajoutée avec un dossier choisi : il reste.
        conn.execute(
            "UPDATE tasks SET rule_base = NULL, dest_dir = '/choisi' WHERE id = ?1",
            [&id],
        )
        .unwrap();
        change_state(&mut conn, &id, &Change::to(TaskState::Cancelled)).unwrap();
        restart_task(&mut conn, &id).unwrap();
        assert_eq!(load_task(&conn, &id).unwrap().dest_dir, "/choisi");

        // Sans adresse (tâche terminée avant qu'elle ne soit gardée) : refusé.
        conn.execute("DELETE FROM sources WHERE task_id = ?1", [&id])
            .unwrap();
        change_state(&mut conn, &id, &Change::to(TaskState::Cancelled)).unwrap();
        assert!(matches!(
            restart_task(&mut conn, &id),
            Err(StoreError::NoSource { .. })
        ));
        assert_eq!(load_task(&conn, &id).unwrap().state, TaskState::Cancelled);
    }

    /// Réseau revenu (SPEC §7.15) : une erreur de réseau repart tout de suite, un serveur qui a
    /// demandé d'attendre garde son délai.
    #[test]
    fn network_waits_are_retried_now_when_the_network_returns() {
        let mut conn = open_in_memory().unwrap();
        let mut waiting = Vec::new();
        for code in ["NET_UNREACHABLE", "NET_TIMEOUT", "HTTP_STATUS"] {
            let (id, _) = new_http_task(&mut conn);
            change_state(&mut conn, &id, &Change::to(TaskState::Active)).unwrap();
            let retry = Change {
                error: Some((code, "détail")),
                next_retry_at: Some(1_000_000),
                ..Change::to(TaskState::RetryWait)
            };
            change_state(&mut conn, &id, &retry).unwrap();
            waiting.push(id);
        }
        let runnable = |conn: &Connection| -> Vec<String> {
            runnable_tasks(conn, 500)
                .unwrap()
                .into_iter()
                .map(|c| c.task_id)
                .collect()
        };
        assert!(runnable(&conn).is_empty(), "délais pas encore passés");
        assert_eq!(retry_network_waits(&conn, 500).unwrap(), 2);
        let now = runnable(&conn);
        assert!(
            now.contains(&waiting[0]) && now.contains(&waiting[1]),
            "{now:?}"
        );
        assert!(
            !now.contains(&waiting[2]),
            "le serveur a demandé d'attendre"
        );
        for id in &waiting {
            assert_eq!(load_task(&conn, id).unwrap().state, TaskState::RetryWait);
        }
        // Déjà dues : rien de plus.
        assert_eq!(retry_network_waits(&conn, 500).unwrap(), 0);
    }
}
