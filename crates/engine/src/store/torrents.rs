//! Torrents en base (SPEC §5.4 et §7.16) : la ligne `torrents` d'une tâche et ses
//! fichiers (`task_files`).
//!
//! Une tâche torrent a pour source son lien magnet, donné ou déduit du fichier `.torrent` :
//! l'historique et le retéléchargement s'en servent comme d'une adresse. Ses métadonnées
//! (le fichier `.torrent`) vont dans `torrents.metainfo` dès qu'elles sont connues.
//!
//! L'avancement enregistré n'est qu'une indication : la reprise revérifie les morceaux sur
//! disque (SPEC §7.16), elle ne s'en sert jamais.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{StoreError, new_id, now_ms};

/// Tâche torrent à créer.
#[derive(Debug, Clone, Copy)]
pub struct NewTorrent<'a> {
    pub queue_id: &'a str,
    pub display_name: &'a str,
    pub dest_dir: &'a str,
    /// Lien magnet : la source de la tâche.
    pub magnet: &'a str,
    /// Empreinte d'information (SHA-1, 40 chiffres hexadécimaux).
    pub info_hash: &'a str,
    /// Contenu du fichier `.torrent`, s'il a été donné.
    pub metainfo: Option<&'a [u8]>,
    pub speed_limit_bps: Option<u64>,
    pub idempotency_key: Option<&'a str>,
    /// Nom choisi pour le premier élément (fichier ou dossier), déjà vérifié.
    pub requested_name: Option<&'a str>,
}

/// Crée une tâche torrent `queued`, sa source (le lien magnet) et sa ligne `torrents` ;
/// renvoie l'identifiant de la tâche.
pub fn create_torrent_task(
    conn: &mut Connection,
    new: &NewTorrent<'_>,
) -> Result<String, StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (task_id, now) = (new_id(), now_ms());
    let limit = new
        .speed_limit_bps
        .map(|l| i64::try_from(l).map_err(|_| StoreError::BadValue("limite de débit".into())))
        .transpose()?;
    tx.execute(
        "INSERT INTO tasks (id, queue_id, kind, state, position, display_name, dest_dir,
                            speed_limit_bps, idempotency_key, requested_name, created_at,
                            updated_at)
         VALUES (?1, ?2, 'torrent', 'queued',
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE queue_id = ?2),
                 ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            task_id,
            new.queue_id,
            new.display_name,
            new.dest_dir,
            limit,
            new.idempotency_key,
            new.requested_name,
            now
        ],
    )?;
    tx.execute(
        "INSERT INTO sources (id, task_id, url) VALUES (?1, ?2, ?3)",
        params![new_id(), task_id, new.magnet],
    )?;
    tx.execute(
        "INSERT INTO torrents (task_id, info_hash, magnet, metainfo) VALUES (?1, ?2, ?3, ?4)",
        params![task_id, new.info_hash, new.magnet, new.metainfo],
    )?;
    tx.commit()?;
    Ok(task_id)
}

/// Une tâche non terminée pour ce torrent, s'il y en a une : deux tâches ne téléchargent
/// pas le même torrent en même temps.
pub fn unfinished_with_hash(
    conn: &Connection,
    info_hash: &str,
) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT t.task_id FROM torrents t JOIN tasks k ON k.id = t.task_id
             WHERE t.info_hash = ?1 AND k.state NOT IN ('completed', 'failed', 'cancelled')
             LIMIT 1",
            [info_hash],
            |r| r.get(0),
        )
        .optional()?)
}

/// La ligne `torrents` d'une tâche.
#[derive(Debug, Clone, PartialEq)]
pub struct TorrentRow {
    pub info_hash: String,
    pub magnet: Option<String>,
    /// Le fichier `.torrent` ; `None` tant que les métadonnées d'un lien magnet ne sont
    /// pas venues.
    pub metainfo: Option<Vec<u8>>,
    pub uploaded_bytes: u64,
    pub ratio_limit: Option<f64>,
    pub seed_time_limit_s: Option<u64>,
}

pub fn load_torrent(conn: &Connection, task_id: &str) -> Result<TorrentRow, StoreError> {
    let row = conn
        .query_row(
            "SELECT info_hash, magnet, metainfo, uploaded_bytes, ratio_limit, seed_time_limit_s
             FROM torrents WHERE task_id = ?1",
            [task_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<Vec<u8>>>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<f64>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::TaskNotFound {
            task_id: task_id.to_owned(),
        })?;
    let bad = |what: &str| StoreError::BadValue(format!("torrent {task_id} : {what}"));
    Ok(TorrentRow {
        info_hash: row.0,
        magnet: row.1,
        metainfo: row.2,
        uploaded_bytes: u64::try_from(row.3).map_err(|_| bad("octets envoyés négatifs"))?,
        ratio_limit: row.4,
        seed_time_limit_s: row
            .5
            .map(|s| u64::try_from(s).map_err(|_| bad("durée de partage négative")))
            .transpose()?,
    })
}

/// Enregistre les métadonnées venues d'un lien magnet.
pub fn save_metainfo(conn: &Connection, task_id: &str, metainfo: &[u8]) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE torrents SET metainfo = ?2 WHERE task_id = ?1",
        params![task_id, metainfo],
    )?;
    Ok(())
}

/// Un fichier d'un torrent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentFile {
    /// Rang dans le torrent.
    pub index: u32,
    /// Chemin dans le torrent, composants nettoyés et séparés par `/` (SPEC §8.1) ; pour
    /// un torrent à un fichier, son nom.
    pub path: String,
    pub length: u64,
    pub selected: bool,
    /// Indication seulement (SPEC §7.16).
    pub done_bytes: u64,
}

/// Enregistre la préparation d'un torrent (métadonnées connues) : nom du premier élément,
/// nom affiché, taille des fichiers choisis, et la liste des fichiers.
pub fn save_preparation(
    conn: &mut Connection,
    task_id: &str,
    file_name: &str,
    display_name: &str,
    total_bytes: u64,
    files: &[TorrentFile],
) -> Result<(), StoreError> {
    let to_i64 = |v: u64| i64::try_from(v).map_err(|_| StoreError::BadValue("taille".into()));
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "UPDATE tasks SET file_name = ?2, display_name = ?3, total_bytes = ?4, updated_at = ?5
         WHERE id = ?1",
        params![
            task_id,
            file_name,
            display_name,
            to_i64(total_bytes)?,
            now_ms()
        ],
    )?;
    tx.execute("DELETE FROM task_files WHERE task_id = ?1", [task_id])?;
    for f in files {
        tx.execute(
            "INSERT INTO task_files (task_id, file_index, path, length, selected, done_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                task_id,
                f.index,
                f.path,
                to_i64(f.length)?,
                f.selected,
                to_i64(f.done_bytes)?
            ],
        )?;
    }
    // Le nom est enregistré : sa réservation part avec lui (SPEC §5.6, règle 7).
    super::claims::release(&tx, task_id)?;
    tx.commit()?;
    Ok(())
}

/// Les fichiers d'un torrent, par rang ; vide avant la préparation.
pub fn load_files(conn: &Connection, task_id: &str) -> Result<Vec<TorrentFile>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT file_index, path, length, selected, done_bytes FROM task_files
         WHERE task_id = ?1 ORDER BY file_index",
    )?;
    let rows = stmt.query_map([task_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, bool>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let bad = |what: &str| StoreError::BadValue(format!("fichier du torrent {task_id} : {what}"));
    let mut files = Vec::new();
    for row in rows {
        let (index, path, length, selected, done) = row?;
        files.push(TorrentFile {
            index: u32::try_from(index).map_err(|_| bad("rang"))?,
            path,
            length: u64::try_from(length).map_err(|_| bad("taille"))?,
            selected,
            done_bytes: u64::try_from(done).map_err(|_| bad("octets"))?,
        });
    }
    Ok(files)
}

/// Octets envoyés aux autres pairs, cumulés d'une session à l'autre (SPEC §7.16).
pub fn save_uploaded(conn: &Connection, task_id: &str, uploaded: u64) -> Result<(), StoreError> {
    let uploaded = i64::try_from(uploaded).map_err(|_| StoreError::BadValue("octets".into()))?;
    conn.execute(
        "UPDATE torrents SET uploaded_bytes = ?2 WHERE task_id = ?1",
        params![task_id, uploaded],
    )?;
    Ok(())
}

/// Choisit les fichiers à télécharger (`task.setFiles`) : ceux de `selected`, par rang,
/// les autres non. La taille de la tâche devient celle des fichiers choisis ; renvoyée.
/// Au moins un fichier, et seulement des rangs connus.
pub fn set_selected(
    conn: &mut Connection,
    task_id: &str,
    selected: &[u32],
) -> Result<u64, StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let files = load_files(&tx, task_id)?;
    if selected.is_empty() {
        return Err(StoreError::BadValue("aucun fichier choisi".into()));
    }
    if let Some(unknown) = selected
        .iter()
        .find(|i| !files.iter().any(|f| f.index == **i))
    {
        return Err(StoreError::BadValue(format!("fichier inconnu : {unknown}")));
    }
    let mut total = 0u64;
    for f in &files {
        let on = selected.contains(&f.index);
        if on {
            total = total.saturating_add(f.length);
        }
        tx.execute(
            "UPDATE task_files SET selected = ?3 WHERE task_id = ?1 AND file_index = ?2",
            params![task_id, f.index, on],
        )?;
    }
    let total_sql = i64::try_from(total).map_err(|_| StoreError::BadValue("taille".into()))?;
    tx.execute(
        "UPDATE tasks SET total_bytes = ?2, updated_at = ?3 WHERE id = ?1",
        params![task_id, total_sql, now_ms()],
    )?;
    tx.commit()?;
    Ok(total)
}

/// Avancement indicatif de chaque fichier (rang, octets).
pub fn save_file_progress(
    conn: &mut Connection,
    task_id: &str,
    done: &[(u32, u64)],
) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    for (index, bytes) in done {
        let bytes = i64::try_from(*bytes).map_err(|_| StoreError::BadValue("octets".into()))?;
        tx.execute(
            "UPDATE task_files SET done_bytes = ?3 WHERE task_id = ?1 AND file_index = ?2",
            params![task_id, index, bytes],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Fichiers choisis reçus en entier et sous leur nom final : leur avancement indicatif
/// devient leur taille (le dernier relevé pouvait la manquer de peu).
pub fn files_complete(conn: &Connection, task_id: &str) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE task_files SET done_bytes = length WHERE task_id = ?1 AND selected = 1",
        [task_id],
    )?;
    Ok(())
}

/// Avancement indicatif d'un torrent (SPEC §7.16) : octets des morceaux vérifiés.
pub fn save_progress(conn: &Connection, task_id: &str, done_bytes: u64) -> Result<(), StoreError> {
    let done = i64::try_from(done_bytes).map_err(|_| StoreError::BadValue("octets".into()))?;
    conn.execute(
        "UPDATE tasks SET done_bytes = ?2, updated_at = ?3 WHERE id = ?1",
        params![task_id, done, now_ms()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tasks::{TaskKind, TaskState, ensure_queue, load_task, primary_source};
    use crate::store::{MIGRATIONS, migrate, open_in_memory};

    fn db() -> Connection {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn, MIGRATIONS).unwrap();
        conn
    }

    fn new<'a>(queue: &'a str, metainfo: Option<&'a [u8]>) -> NewTorrent<'a> {
        NewTorrent {
            queue_id: queue,
            display_name: "debian.iso",
            dest_dir: "/tmp",
            magnet: "magnet:?xt=urn:btih:7acf8fb590b2060dd9c3146ef770169d593433b0",
            info_hash: "7acf8fb590b2060dd9c3146ef770169d593433b0",
            metainfo,
            speed_limit_bps: Some(1000),
            idempotency_key: None,
            requested_name: None,
        }
    }

    #[test]
    fn a_torrent_task_has_its_magnet_as_source_and_a_torrents_row() {
        let mut conn = db();
        let queue = ensure_queue(&conn, "principale").unwrap();
        let id = create_torrent_task(&mut conn, &new(&queue, None)).unwrap();
        let task = load_task(&conn, &id).unwrap();
        assert_eq!(task.kind, TaskKind::Torrent);
        assert_eq!(task.state, TaskState::Queued);
        assert_eq!(task.speed_limit_bps, Some(1000));
        assert!(
            primary_source(&conn, &id)
                .unwrap()
                .url
                .starts_with("magnet:")
        );
        let row = load_torrent(&conn, &id).unwrap();
        assert_eq!(row.info_hash, "7acf8fb590b2060dd9c3146ef770169d593433b0");
        assert_eq!(row.metainfo, None);
        save_metainfo(&conn, &id, b"d4:infod...ee").unwrap();
        assert_eq!(
            load_torrent(&conn, &id).unwrap().metainfo.as_deref(),
            Some(&b"d4:infod...ee"[..])
        );
    }

    #[test]
    fn preparation_records_the_name_size_and_files() {
        let mut conn = db();
        let queue = ensure_queue(&conn, "principale").unwrap();
        let id = create_torrent_task(&mut conn, &new(&queue, Some(b"x"))).unwrap();
        let files = [
            TorrentFile {
                index: 0,
                path: "a/b.txt".into(),
                length: 10,
                selected: true,
                done_bytes: 0,
            },
            TorrentFile {
                index: 1,
                path: "c.txt".into(),
                length: 5,
                selected: false,
                done_bytes: 0,
            },
        ];
        save_preparation(&mut conn, &id, "Dossier", "Dossier", 10, &files).unwrap();
        let task = load_task(&conn, &id).unwrap();
        assert_eq!(task.file_name.as_deref(), Some("Dossier"));
        assert_eq!(task.total_bytes, Some(10));
        assert_eq!(load_files(&conn, &id).unwrap(), files);
        // Une seconde préparation remplace la liste.
        save_preparation(&mut conn, &id, "Dossier", "Dossier", 10, &files[..1]).unwrap();
        assert_eq!(load_files(&conn, &id).unwrap().len(), 1);
        save_progress(&conn, &id, 7).unwrap();
        assert_eq!(load_task(&conn, &id).unwrap().done_bytes, 7);
    }
}
