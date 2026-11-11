//! Sources d'une tâche HTTP : la référence et ses miroirs (SPEC §5.3 et §7.7, phase 6).
//! Ajout d'un miroir, résultat de sa validation, échecs.

use rusqlite::{Connection, ErrorCode, OptionalExtension, params};

use super::tasks::SourceState;
use super::{StoreError, new_id};

/// Plus de sources par tâche (SPEC §7.7) : la référence et 15 miroirs.
pub const MAX_SOURCES: usize = 16;

/// Insère une source à valider (`unknown`) au rang donné ; renvoie son identifiant. Une
/// adresse déjà présente dans la tâche est refusée.
pub(super) fn insert(
    conn: &Connection,
    task_id: &str,
    url: &str,
    rank: u32,
) -> Result<String, StoreError> {
    let id = new_id();
    match conn.execute(
        "INSERT INTO sources (id, task_id, url, rank) VALUES (?1, ?2, ?3, ?4)",
        params![id, task_id, url, rank],
    ) {
        Ok(_) => Ok(id),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::ConstraintViolation => {
            Err(StoreError::BadValue(
                "adresse déjà présente parmi les sources de la tâche".into(),
            ))
        }
        Err(e) => Err(e.into()),
    }
}

/// Ajoute un miroir à la fin des sources d'une tâche ; renvoie son identifiant.
pub fn add_source(conn: &mut Connection, task_id: &str, url: &str) -> Result<String, StoreError> {
    let tx = conn.transaction()?;
    let (count, last): (i64, Option<i64>) = tx.query_row(
        "SELECT COUNT(*), MAX(rank) FROM sources WHERE task_id = ?1",
        [task_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count == 0 {
        return Err(StoreError::NoSource {
            task_id: task_id.to_owned(),
        });
    }
    if count as usize >= MAX_SOURCES {
        return Err(StoreError::BadValue(format!(
            "{MAX_SOURCES} sources au plus par tâche"
        )));
    }
    let rank = u32::try_from(last.unwrap_or(0) + 1).unwrap_or(u32::MAX);
    let id = insert(&tx, task_id, url, rank)?;
    tx.commit()?;
    Ok(id)
}

/// Ce qu'une source a montré à sa validation, ou à la préparation pour la référence.
#[derive(Debug, Clone, Copy)]
pub struct Checked<'a> {
    pub accepts_ranges: bool,
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
    pub content_length: Option<u64>,
}

/// Source vérifiée et utilisable : ses validateurs, pour la reprise sur elle.
pub fn set_checked(
    conn: &Connection,
    source_id: &str,
    checked: &Checked<'_>,
) -> Result<(), StoreError> {
    let length = checked
        .content_length
        .map(|l| i64::try_from(l).map_err(|_| StoreError::BadValue("taille".into())))
        .transpose()?;
    conn.execute(
        "UPDATE sources SET state = 'ok', accepts_ranges = ?2, etag = ?3, last_modified = ?4,
                            content_length = ?5, fail_count = 0, last_error = NULL
         WHERE id = ?1",
        params![
            source_id,
            checked.accepts_ranges,
            checked.etag,
            checked.last_modified,
            length
        ],
    )?;
    Ok(())
}

/// Source en échec, ou miroir refusé, avec la raison ; ses échecs consécutifs comptent un
/// de plus.
pub fn set_failed(conn: &Connection, source_id: &str, reason: &str) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE sources SET state = 'failed', last_error = ?2, fail_count = fail_count + 1
         WHERE id = ?1",
        params![source_id, reason],
    )?;
    Ok(())
}

/// État et débit d'une source à la fin d'une exécution, ou quand ils changent (SPEC §7.7) :
/// une source écartée garde la raison, et ses échecs se comptent ; une source bonne les
/// remet à zéro.
pub fn save_run(
    conn: &Connection,
    source_id: &str,
    state: SourceState,
    avg_bps: Option<u64>,
    reason: Option<&str>,
) -> Result<(), StoreError> {
    let bps = avg_bps.map(|b| i64::try_from(b).unwrap_or(i64::MAX));
    match state {
        SourceState::Failed => conn.execute(
            "UPDATE sources SET state = 'failed', avg_bps = COALESCE(?2, avg_bps),
                    last_error = COALESCE(?3, last_error), fail_count = fail_count + 1
             WHERE id = ?1",
            params![source_id, bps, reason],
        )?,
        other => conn.execute(
            "UPDATE sources SET state = ?2, avg_bps = COALESCE(?3, avg_bps), fail_count = 0
             WHERE id = ?1",
            params![source_id, other.as_sql(), bps],
        )?,
    };
    Ok(())
}

/// Débit lissé d'une source, pour l'affichage et l'exécution suivante.
pub fn save_speed(conn: &Connection, source_id: &str, avg_bps: u64) -> Result<(), StoreError> {
    let bps = i64::try_from(avg_bps).unwrap_or(i64::MAX);
    conn.execute(
        "UPDATE sources SET avg_bps = ?2 WHERE id = ?1",
        params![source_id, bps],
    )?;
    Ok(())
}

/// Miroir injoignable à sa validation : il reste à valider (`unknown`), la raison et ses
/// échecs notés.
pub fn set_unreachable(conn: &Connection, source_id: &str, reason: &str) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE sources SET last_error = ?2, fail_count = fail_count + 1 WHERE id = ?1",
        params![source_id, reason],
    )?;
    Ok(())
}

/// Tâche interne d'une vidéo (flux direct, SPEC §7.17) : jamais de miroir pour elle.
pub fn is_internal(conn: &Connection, task_id: &str) -> Result<bool, StoreError> {
    Ok(conn
        .query_row(
            "SELECT parent_id IS NOT NULL FROM tasks WHERE id = ?1",
            [task_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false))
}

/// État d'une source ; `None` si elle n'existe pas.
pub fn state_of(conn: &Connection, source_id: &str) -> Result<Option<SourceState>, StoreError> {
    let text: Option<String> = conn
        .query_row(
            "SELECT state FROM sources WHERE id = ?1",
            [source_id],
            |r| r.get(0),
        )
        .optional()?;
    text.map(|t| SourceState::from_sql(&t)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open;
    use crate::store::tasks::{NewTask, create_task_with_mirrors, ensure_queue, load_sources};

    fn task(conn: &mut Connection, mirrors: &[String]) -> String {
        let queue = ensure_queue(conn, "default").unwrap();
        let new = NewTask {
            queue_id: &queue,
            url: "https://a.exemple/f.iso",
            display_name: "f.iso",
            dest_dir: "/tmp",
            max_connections: None,
            speed_limit_bps: None,
            idempotency_key: None,
            requested_name: None,
            checksum: None,
            headers: &[],
            parent_id: None,
            public_only: false,
        };
        create_task_with_mirrors(conn, &new, mirrors).unwrap().0
    }

    #[test]
    fn mirrors_are_ranked_after_the_reference_and_never_twice() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(&dir.path().join("auro.db")).unwrap();
        let id = task(&mut conn, &["https://b.exemple/f.iso".into()]);
        let added = add_source(&mut conn, &id, "https://c.exemple/f.iso").unwrap();
        let sources = load_sources(&conn, &id).unwrap();
        let ranks: Vec<_> = sources.iter().map(|s| (s.rank, s.url.as_str())).collect();
        assert_eq!(
            ranks,
            [
                (0, "https://a.exemple/f.iso"),
                (1, "https://b.exemple/f.iso"),
                (2, "https://c.exemple/f.iso")
            ]
        );
        assert!(sources.iter().all(|s| s.state == SourceState::Unknown));
        // Une adresse déjà là, la référence comprise : refusée.
        for url in ["https://c.exemple/f.iso", "https://a.exemple/f.iso"] {
            assert!(matches!(
                add_source(&mut conn, &id, url),
                Err(StoreError::BadValue(_))
            ));
        }
        // Validé, puis refusé : la raison reste, les échecs se comptent.
        let checked = Checked {
            accepts_ranges: true,
            etag: Some("\"e\""),
            last_modified: None,
            content_length: Some(10),
        };
        set_checked(&conn, &added, &checked).unwrap();
        assert_eq!(state_of(&conn, &added).unwrap(), Some(SourceState::Ok));
        set_failed(&conn, &added, "contenu différent").unwrap();
        let mirror = load_sources(&conn, &id).unwrap().pop().unwrap();
        assert_eq!(mirror.state, SourceState::Failed);
        assert_eq!(mirror.fail_count, 1);
        assert_eq!(mirror.last_error.as_deref(), Some("contenu différent"));
        assert_eq!(mirror.content_length, Some(10));
    }

    #[test]
    fn sixteen_sources_at_most() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(&dir.path().join("auro.db")).unwrap();
        let mirrors: Vec<String> = (1..MAX_SOURCES)
            .map(|i| format!("https://m{i}.exemple/f.iso"))
            .collect();
        let id = task(&mut conn, &mirrors);
        assert!(matches!(
            add_source(&mut conn, &id, "https://trop.exemple/f.iso"),
            Err(StoreError::BadValue(_))
        ));
        let too_many: Vec<String> = (0..MAX_SOURCES)
            .map(|i| format!("https://n{i}.e/f"))
            .collect();
        let queue = ensure_queue(&conn, "default").unwrap();
        let new = NewTask {
            queue_id: &queue,
            url: "https://a.exemple/g.iso",
            display_name: "g.iso",
            dest_dir: "/tmp",
            max_connections: None,
            speed_limit_bps: None,
            idempotency_key: None,
            requested_name: None,
            checksum: None,
            headers: &[],
            parent_id: None,
            public_only: false,
        };
        assert!(create_task_with_mirrors(&mut conn, &new, &too_many).is_err());
    }
}
