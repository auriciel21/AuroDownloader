//! Emplacement d'une tâche et journal de ses déplacements (« Déplacer/Renommer »,
//! SPEC §7.12).
//!
//! Un déplacement de fichier s'inscrit dans `file_moves` avant de toucher au disque :
//! après un arrêt brutal, le moteur sait ce qui était en cours et le termine ou l'annule
//! (`crate::moves::recover`). La tâche ne change de dossier et de nom qu'une fois le
//! fichier à sa nouvelle place ([`commit`]).

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{StoreError, now_ms};

/// Étape d'un déplacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Le fichier n'est peut-être pas encore à sa place ; la tâche porte l'ancien
    /// emplacement.
    Moving,
    /// Le fichier est à sa place et la tâche porte le nouvel emplacement ; reste l'original
    /// à effacer (copie vers un autre volume).
    Moved,
}

impl Phase {
    fn as_sql(self) -> &'static str {
        match self {
            Phase::Moving => "moving",
            Phase::Moved => "moved",
        }
    }
}

/// Un déplacement inscrit au journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMove {
    pub task_id: String,
    /// Fichier à déplacer : le fichier final d'une tâche terminée, le `.part` des autres.
    pub from_path: String,
    pub to_path: String,
    /// Copie en cours vers un autre volume, renommée en `to_path` une fois complète.
    pub temp_path: Option<String>,
    /// Dossier et nom de la tâche une fois le fichier déplacé.
    pub dest_dir: String,
    pub file_name: String,
    pub phase: Phase,
}

/// Inscrit un déplacement (phase `moving`). Échoue s'il y en a déjà un pour la tâche.
pub fn begin(conn: &Connection, m: &FileMove) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO file_moves (task_id, from_path, to_path, temp_path, dest_dir, file_name,
                                 phase, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            m.task_id,
            m.from_path,
            m.to_path,
            m.temp_path,
            m.dest_dir,
            m.file_name,
            m.phase.as_sql(),
            now_ms()
        ],
    )?;
    Ok(())
}

/// Note la copie temporaire d'un déplacement vers un autre volume, avant de la créer.
pub fn set_temp(conn: &Connection, task_id: &str, temp_path: &str) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE file_moves SET temp_path = ?2 WHERE task_id = ?1",
        params![task_id, temp_path],
    )?;
    Ok(())
}

const COLUMNS: &str = "task_id, from_path, to_path, temp_path, dest_dir, file_name, phase";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(FileMove, String)> {
    Ok((
        FileMove {
            task_id: r.get(0)?,
            from_path: r.get(1)?,
            to_path: r.get(2)?,
            temp_path: r.get(3)?,
            dest_dir: r.get(4)?,
            file_name: r.get(5)?,
            phase: Phase::Moving,
        },
        r.get(6)?,
    ))
}

fn checked((mut m, phase): (FileMove, String)) -> Result<FileMove, StoreError> {
    m.phase = match phase.as_str() {
        "moving" => Phase::Moving,
        "moved" => Phase::Moved,
        other => {
            return Err(StoreError::BadValue(format!(
                "déplacement de la tâche {} : phase inconnue {other}",
                m.task_id
            )));
        }
    };
    Ok(m)
}

/// Déplacement en cours d'une tâche.
pub fn load(conn: &Connection, task_id: &str) -> Result<Option<FileMove>, StoreError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM file_moves WHERE task_id = ?1"),
        [task_id],
        row,
    )
    .optional()?
    .map(checked)
    .transpose()
}

/// Tous les déplacements inscrits (au démarrage : ceux qu'un arrêt a interrompus).
pub fn all(conn: &Connection) -> Result<Vec<FileMove>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM file_moves ORDER BY task_id"
    ))?;
    let rows = stmt.query_map([], row)?;
    rows.map(|r| checked(r?)).collect()
}

/// Le fichier est à sa nouvelle place : la tâche prend son nouvel emplacement, dans la même
/// transaction que la fin du déplacement. `Phase::Moved` garde la ligne, l'original restant
/// à effacer ; sinon elle disparaît.
pub fn commit(conn: &mut Connection, task_id: &str, then: Phase) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let m = load(&tx, task_id)?.ok_or_else(|| {
        StoreError::BadValue(format!("tâche {task_id} : aucun déplacement en cours"))
    })?;
    set_location(&tx, task_id, &m.dest_dir, Some(&m.file_name))?;
    match then {
        Phase::Moved => {
            tx.execute(
                "UPDATE file_moves SET phase = 'moved', temp_path = NULL WHERE task_id = ?1",
                [task_id],
            )?;
        }
        Phase::Moving => {
            tx.execute("DELETE FROM file_moves WHERE task_id = ?1", [task_id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Efface la ligne d'un déplacement fini ou abandonné.
pub fn forget(conn: &Connection, task_id: &str) -> Result<(), StoreError> {
    conn.execute("DELETE FROM file_moves WHERE task_id = ?1", [task_id])?;
    Ok(())
}

/// Nouvel emplacement d'une tâche : son dossier, et son nom s'il est donné. Le nom devient
/// celui que l'utilisateur a choisi (il survit au retéléchargement) et le nom affiché ; le
/// nom du fichier ne change que s'il est déjà fixé (tâche préparée). Pour une tâche
/// terminée, l'historique suit. Ne touche pas au disque.
pub fn set_location(
    conn: &Connection,
    task_id: &str,
    dest_dir: &str,
    name: Option<&str>,
) -> Result<(), StoreError> {
    let updated = conn.execute(
        "UPDATE tasks SET dest_dir = ?2,
           requested_name = COALESCE(?3, requested_name),
           display_name = COALESCE(?3, display_name),
           file_name = CASE WHEN file_name IS NULL THEN NULL ELSE COALESCE(?3, file_name) END,
           updated_at = ?4
         WHERE id = ?1",
        params![task_id, dest_dir, name, now_ms()],
    )?;
    if updated == 0 {
        return Err(StoreError::TaskNotFound {
            task_id: task_id.to_owned(),
        });
    }
    // Chemin final de la dernière fin de la tâche, si elle est terminée.
    let (state, file_name): (String, Option<String>) = conn.query_row(
        "SELECT state, file_name FROM tasks WHERE id = ?1",
        [task_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if let ("completed", Some(file_name)) = (state.as_str(), file_name) {
        let path = std::path::Path::new(dest_dir).join(file_name);
        conn.execute(
            "UPDATE history SET final_path = ?2
             WHERE id = (SELECT id FROM history WHERE task_id = ?1 AND final_state = 'completed'
                         ORDER BY finished_at DESC, id DESC LIMIT 1)",
            params![task_id, path.to_string_lossy()],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_in_memory;
    use crate::store::tasks::{self, NewTask, load_task};

    fn task(conn: &mut Connection) -> String {
        let queue = tasks::ensure_queue(conn, "default").unwrap();
        let (id, _) = tasks::create_task(
            conn,
            &NewTask {
                queue_id: &queue,
                url: "https://exemple.org/a.iso",
                display_name: "a.iso",
                dest_dir: "/dl",
                max_connections: None,
                speed_limit_bps: None,
                idempotency_key: None,
                requested_name: None,
                checksum: None,
                headers: &[],
            },
        )
        .unwrap();
        id
    }

    fn journal(task_id: &str) -> FileMove {
        FileMove {
            task_id: task_id.to_owned(),
            from_path: "/dl/a.iso.part".into(),
            to_path: "/autre/b.iso.part".into(),
            temp_path: None,
            dest_dir: "/autre".into(),
            file_name: "b.iso".into(),
            phase: Phase::Moving,
        }
    }

    #[test]
    fn a_task_not_yet_prepared_keeps_no_file_name() {
        let mut conn = open_in_memory().unwrap();
        let id = task(&mut conn);
        set_location(&conn, &id, "/autre", Some("b.iso")).unwrap();
        let t = load_task(&conn, &id).unwrap();
        assert_eq!(t.dest_dir, "/autre");
        assert_eq!(t.file_name, None);
        assert_eq!(t.requested_name.as_deref(), Some("b.iso"));
        assert_eq!(t.display_name, "b.iso");
    }

    #[test]
    fn a_move_shows_on_the_task_until_it_is_forgotten() {
        let mut conn = open_in_memory().unwrap();
        let id = task(&mut conn);
        conn.execute("UPDATE tasks SET file_name = 'a.iso' WHERE id = ?1", [&id])
            .unwrap();
        begin(&conn, &journal(&id)).unwrap();
        assert!(load_task(&conn, &id).unwrap().moving);
        // Un seul déplacement à la fois.
        assert!(begin(&conn, &journal(&id)).is_err());
        set_temp(&conn, &id, "/autre/b.iso.part.auro-move").unwrap();
        assert_eq!(
            load(&conn, &id).unwrap().unwrap().temp_path.as_deref(),
            Some("/autre/b.iso.part.auro-move")
        );

        commit(&mut conn, &id, Phase::Moved).unwrap();
        let t = load_task(&conn, &id).unwrap();
        assert_eq!(
            (t.dest_dir.as_str(), t.file_name.as_deref()),
            ("/autre", Some("b.iso"))
        );
        assert!(t.moving, "l'original reste à effacer");
        let m = load(&conn, &id).unwrap().unwrap();
        assert_eq!((m.phase, m.temp_path), (Phase::Moved, None));

        forget(&conn, &id).unwrap();
        assert!(!load_task(&conn, &id).unwrap().moving);
        assert!(all(&conn).unwrap().is_empty());
    }

    #[test]
    fn a_direct_commit_ends_the_move() {
        let mut conn = open_in_memory().unwrap();
        let id = task(&mut conn);
        conn.execute("UPDATE tasks SET file_name = 'a.iso' WHERE id = ?1", [&id])
            .unwrap();
        begin(&conn, &journal(&id)).unwrap();
        commit(&mut conn, &id, Phase::Moving).unwrap();
        let t = load_task(&conn, &id).unwrap();
        assert_eq!(t.file_name.as_deref(), Some("b.iso"));
        assert_eq!(t.requested_name.as_deref(), Some("b.iso"));
        assert!(!t.moving);
    }

    #[test]
    fn the_history_of_a_completed_task_follows_its_file() {
        let mut conn = open_in_memory().unwrap();
        let id = task(&mut conn);
        conn.execute(
            "UPDATE tasks SET file_name = 'a.iso', state = 'completed' WHERE id = ?1",
            [&id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO history (id, task_id, display_name, final_path, final_state,
                                  finished_at)
             VALUES ('h1', ?1, 'a.iso', '/dl/a.iso', 'completed', 1)",
            [&id],
        )
        .unwrap();
        set_location(&conn, &id, "/autre", Some("b.iso")).unwrap();
        let path: String = conn
            .query_row("SELECT final_path FROM history WHERE id = 'h1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            path,
            std::path::Path::new("/autre")
                .join("b.iso")
                .to_string_lossy()
        );
    }
}
