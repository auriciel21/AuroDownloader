//! Tâches vidéo (SPEC §7.17) : la ligne `media_jobs` (page, format, conteneur), les flux
//! retenus (`media_streams`, migration 6), ses sous-titres (même table, avec leur langue,
//! migration 7) et leurs tâches HTTP internes (`tasks.parent_id`).

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{StoreError, new_id, now_ms};

/// Une tâche vidéo à créer.
#[derive(Debug, Clone)]
pub struct NewMedia<'a> {
    pub queue_id: &'a str,
    pub page_url: &'a str,
    pub display_name: &'a str,
    pub dest_dir: &'a str,
    pub speed_limit_bps: Option<u64>,
    pub idempotency_key: Option<&'a str>,
    /// Sélecteur de formats de yt-dlp (`bv*+ba/b` par défaut).
    pub format: &'a str,
    /// Conteneur demandé (`mp4`, `mkv`, `webm`) ; `None` : automatique.
    pub container: Option<&'a str>,
    /// Langues des sous-titres demandés.
    pub subtitles: &'a [String],
    /// Ajoutée par une connexion à jeton : réseau public seulement (SPEC §7.19), flux
    /// internes compris.
    pub public_only: bool,
}

/// Crée une tâche vidéo `queued`, sa source (l'adresse de la page) et sa ligne
/// `media_jobs` ; renvoie l'identifiant de la tâche.
pub fn create_media_task(
    conn: &mut Connection,
    new: &NewMedia<'_>,
    start: super::tasks::Start,
) -> Result<String, StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (task_id, now) = (new_id(), now_ms());
    let limit = new
        .speed_limit_bps
        .map(|l| super::to_i64(l, "limite de débit"))
        .transpose()?;
    tx.execute(
        "INSERT INTO tasks (id, queue_id, kind, state, position, display_name, dest_dir,
                            speed_limit_bps, idempotency_key, created_at, updated_at, public_only)
         VALUES (?1, ?2, 'media', 'queued',
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE queue_id = ?2),
                 ?3, ?4, ?5, ?6, ?7, ?7, ?8)",
        params![
            task_id,
            new.queue_id,
            new.display_name,
            new.dest_dir,
            limit,
            new.idempotency_key,
            now,
            new.public_only
        ],
    )?;
    tx.execute(
        "INSERT INTO sources (id, task_id, url) VALUES (?1, ?2, ?3)",
        params![new_id(), task_id, new.page_url],
    )?;
    let subtitles = serde_json::Value::from(new.subtitles.to_vec()).to_string();
    tx.execute(
        "INSERT INTO media_jobs (task_id, page_url, format_selector, merge_container,
                                 subtitles_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id, new.page_url, new.format, new.container, subtitles],
    )?;
    super::tasks::apply_start(&tx, &task_id, start)?;
    tx.commit()?;
    Ok(task_id)
}

/// Ce que l'utilisateur a demandé pour une vidéo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaJob {
    pub page_url: String,
    pub format: String,
    /// Conteneur demandé, ou retenu à la résolution.
    pub container: Option<String>,
    pub subtitles: Vec<String>,
}

pub fn load_job(conn: &Connection, task_id: &str) -> Result<MediaJob, StoreError> {
    let row = conn
        .query_row(
            "SELECT page_url, format_selector, merge_container, subtitles_json
             FROM media_jobs WHERE task_id = ?1",
            [task_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::BadValue(format!("tâche {task_id} : pas une vidéo")))?;
    let subtitles = row
        .3
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    Ok(MediaJob {
        page_url: row.0,
        format: row.1,
        container: row.2,
        subtitles,
    })
}

/// Un flux retenu d'une vidéo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRow {
    pub position: u32,
    pub format_id: String,
    pub protocol: String,
    pub ext: String,
    pub has_video: bool,
    pub has_audio: bool,
    /// Nom du fichier du flux, dans le dossier de la tâche.
    pub file_name: String,
    /// Taille, si elle est connue.
    pub size: Option<u64>,
    /// Sa tâche HTTP interne (flux direct).
    pub child_id: Option<String>,
    /// Fichier complet et vidé sur disque.
    pub done: bool,
    /// Langue d'un sous-titre (F-24) ; `None` : flux vidéo ou audio, à fusionner.
    pub language: Option<String>,
}

impl StreamRow {
    /// Un sous-titre, gardé à côté de la vidéo, jamais fusionné.
    pub fn is_subtitle(&self) -> bool {
        self.language.is_some()
    }
}

pub fn load_streams(conn: &Connection, task_id: &str) -> Result<Vec<StreamRow>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT position, format_id, protocol, ext, has_video, has_audio, file_name, size,
                child_id, done, language
         FROM media_streams WHERE task_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([task_id], |r| {
        Ok(StreamRow {
            position: r.get(0)?,
            format_id: r.get(1)?,
            protocol: r.get(2)?,
            ext: r.get(3)?,
            has_video: r.get(4)?,
            has_audio: r.get(5)?,
            file_name: r.get(6)?,
            size: r.get::<_, Option<i64>>(7)?.map(|s| s.unsigned_abs()),
            child_id: r.get(8)?,
            done: r.get(9)?,
            language: r.get(10)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Première résolution d'une vidéo : son titre (nom affiché), son nom final, le conteneur
/// retenu et ses flux, enregistrés ensemble.
pub fn save_resolution(
    conn: &mut Connection,
    task_id: &str,
    title: &str,
    final_name: &str,
    container: &str,
    streams: &[StreamRow],
    placed: &super::rules::Placed<'_>,
) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Rangement des règles (SPEC §7.21), avec le nom pris dans ce dossier.
    super::rules::save_placement(&tx, task_id, placed)?;
    // Les sous-titres, petits et de taille souvent inconnue, ne comptent pas.
    let total: Option<u64> = streams
        .iter()
        .filter(|s| !s.is_subtitle())
        .map(|s| s.size)
        .sum();
    let total = total.map(|t| super::to_i64(t, "taille")).transpose()?;
    tx.execute(
        "UPDATE tasks SET display_name = ?2, file_name = ?3, total_bytes = ?4, updated_at = ?5
         WHERE id = ?1",
        params![task_id, title, final_name, total, now_ms()],
    )?;
    tx.execute(
        "UPDATE media_jobs SET merge_container = ?2 WHERE task_id = ?1",
        params![task_id, container],
    )?;
    for s in streams {
        tx.execute(
            "INSERT INTO media_streams (task_id, position, format_id, protocol, ext, has_video,
                                        has_audio, file_name, size, language)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task_id,
                s.position,
                s.format_id,
                s.protocol,
                s.ext,
                s.has_video,
                s.has_audio,
                s.file_name,
                s.size.map(|v| v as i64),
                s.language
            ],
        )?;
    }
    // Le nom final est enregistré : sa réservation part avec lui (SPEC §5.6, règle 7).
    super::claims::release(&tx, task_id)?;
    tx.commit()?;
    Ok(())
}

/// Rattache à un flux sa tâche HTTP interne.
pub fn set_child(
    conn: &Connection,
    task_id: &str,
    position: u32,
    child_id: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE media_streams SET child_id = ?3 WHERE task_id = ?1 AND position = ?2",
        params![task_id, position, child_id],
    )?;
    Ok(())
}

/// La tâche interne déjà créée pour ce nom de flux (arrêt entre sa création et son
/// rattachement), s'il y en a une.
pub fn child_named(
    conn: &Connection,
    parent_id: &str,
    requested_name: &str,
) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT id FROM tasks WHERE parent_id = ?1 AND requested_name = ?2",
            params![parent_id, requested_name],
            |r| r.get(0),
        )
        .optional()?)
}

/// Un flux est complet et vidé sur disque, sous `file_name` (nom final de sa tâche
/// interne, ou celui donné à yt-dlp), avec sa taille.
pub fn stream_done(
    conn: &Connection,
    task_id: &str,
    position: u32,
    file_name: &str,
    size: u64,
) -> Result<(), StoreError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE media_streams SET done = 1, file_name = ?3, size = ?4
         WHERE task_id = ?1 AND position = ?2",
        params![task_id, position, file_name, size as i64],
    )?;
    // Sous-titre écrit par le moteur : sa réservation part avec son enregistrement
    // (SPEC §5.6, règle 7).
    super::claims::release(&tx, task_id)?;
    tx.commit()?;
    Ok(())
}

/// Octets reçus (une indication, comme pour un torrent).
pub fn save_progress(conn: &Connection, task_id: &str, done_bytes: u64) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE tasks SET done_bytes = ?2, updated_at = ?3 WHERE id = ?1",
        params![task_id, done_bytes as i64, now_ms()],
    )?;
    Ok(())
}

/// Note la durée annoncée par la page (secondes), à chaque résolution.
pub fn set_duration(
    conn: &Connection,
    task_id: &str,
    duration_s: Option<f64>,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE media_jobs SET duration_s = ?2 WHERE task_id = ?1",
        params![task_id, duration_s],
    )?;
    Ok(())
}

/// Durée de la vidéo notée à sa dernière résolution.
pub fn duration(conn: &Connection, task_id: &str) -> Result<Option<f64>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT duration_s FROM media_jobs WHERE task_id = ?1",
            [task_id],
            |r| r.get::<_, Option<f64>>(0),
        )
        .optional()?
        .flatten())
}

/// Les tâches internes d'une vidéo.
pub fn children(conn: &Connection, parent_id: &str) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT id FROM tasks WHERE parent_id = ?1 ORDER BY id")?;
    let ids = stmt
        .query_map([parent_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

/// Oublie les flux d'une vidéo et ses tâches internes (retéléchargement).
pub fn forget_streams(conn: &mut Connection, task_id: &str) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM media_streams WHERE task_id = ?1", [task_id])?;
    tx.execute("DELETE FROM tasks WHERE parent_id = ?1", [task_id])?;
    tx.commit()?;
    Ok(())
}

/// Langues demandées dont le sous-titre n'est pas sur le disque (F-24) : absentes de la
/// page, ou refusées par le site. Dans l'ordre de la demande.
pub fn missing_subtitles(conn: &Connection, task_id: &str) -> Result<Vec<String>, StoreError> {
    let job = load_job(conn, task_id)?;
    let streams = load_streams(conn, task_id)?;
    Ok(job
        .subtitles
        .into_iter()
        .filter(|lang| {
            !streams
                .iter()
                .any(|s| s.done && s.language.as_deref() == Some(lang.as_str()))
        })
        .collect())
}
