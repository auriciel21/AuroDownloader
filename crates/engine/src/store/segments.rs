//! Segments d'une tâche dans la base : lecture, enregistrement du découpage initial et des
//! redécoupages, contrôle au démarrage (SPEC §5.6, règle 2, et §7.4, étape 2).
//!
//! Chaque écriture se fait dans une transaction qui relit les segments enregistrés et
//! vérifie leur couverture avant de valider : un état incohérent n'est jamais validé.
//! Les compteurs `written_bytes` ne sont pas écrits ici : ils suivent le vidage des
//! octets sur disque (règle 1).

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{StoreError, now_ms};
use crate::segmenter::{self, Segment, SegmentState, Split};

fn state_to_sql(state: SegmentState) -> &'static str {
    match state {
        SegmentState::Pending => "pending",
        SegmentState::Active => "active",
        SegmentState::Done => "done",
        SegmentState::Failed => "failed",
    }
}

fn state_from_sql(text: &str) -> Option<SegmentState> {
    match text {
        "pending" => Some(SegmentState::Pending),
        "active" => Some(SegmentState::Active),
        "done" => Some(SegmentState::Done),
        "failed" => Some(SegmentState::Failed),
        _ => None,
    }
}

/// SQLite ne connaît que des entiers signés de 64 bits.
fn to_sql_int(value: u64, what: &str) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::BadValue(format!("{what} {value} trop grand pour la base")))
}

/// Segments enregistrés d'une tâche, triés par numéro.
pub fn load_segments(conn: &Connection, task_id: &str) -> Result<Vec<Segment>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT idx, start_offset, end_offset, written_bytes, state
         FROM segments WHERE task_id = ?1 ORDER BY idx",
    )?;
    let rows = stmt.query_map([task_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    let mut segments = Vec::new();
    for row in rows {
        let (idx, start, end, written, state) = row?;
        let bad =
            |what: &str| StoreError::BadValue(format!("tâche {task_id}, segment {idx} : {what}"));
        segments.push(Segment {
            idx: u32::try_from(idx).map_err(|_| bad("numéro hors limites"))?,
            start: u64::try_from(start).map_err(|_| bad("début négatif"))?,
            end: u64::try_from(end).map_err(|_| bad("fin négative"))?,
            written: u64::try_from(written).map_err(|_| bad("octets écrits négatifs"))?,
            state: state_from_sql(&state).ok_or_else(|| bad("état inconnu"))?,
        });
    }
    Ok(segments)
}

/// Taille enregistrée d'une tâche : `None` tant qu'elle est inconnue.
fn task_size(conn: &Connection, task_id: &str) -> Result<Option<u64>, StoreError> {
    let total: Option<Option<i64>> = conn
        .query_row(
            "SELECT total_bytes FROM tasks WHERE id = ?1",
            [task_id],
            |row| row.get(0),
        )
        .optional()?;
    let total = total.ok_or_else(|| StoreError::TaskNotFound {
        task_id: task_id.to_owned(),
    })?;
    total
        .map(|t| {
            u64::try_from(t)
                .map_err(|_| StoreError::BadValue(format!("tâche {task_id} : taille négative")))
        })
        .transpose()
}

/// Vérifie l'invariant de couverture sur les segments enregistrés d'une tâche.
///
/// Taille connue : les segments couvrent exactement `[0, taille - 1]`. Taille inconnue :
/// la tâche est en flux unique et n'a aucun segment.
pub fn check_task_coverage(conn: &Connection, task_id: &str) -> Result<(), StoreError> {
    let total = task_size(conn, task_id)?;
    let segments = load_segments(conn, task_id)?;
    match total {
        Some(total) => {
            segmenter::check_coverage(total, &segments).map_err(|error| StoreError::Coverage {
                task_id: task_id.to_owned(),
                error,
            })
        }
        None if segments.is_empty() => Ok(()),
        None => Err(StoreError::SegmentsWithoutSize {
            task_id: task_id.to_owned(),
            count: segments.len(),
        }),
    }
}

fn insert_segment(conn: &Connection, task_id: &str, s: &Segment) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO segments (task_id, idx, start_offset, end_offset, written_bytes, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            task_id,
            s.idx,
            to_sql_int(s.start, "début")?,
            to_sql_int(s.end, "fin")?,
            to_sql_int(s.written, "octets écrits")?,
            state_to_sql(s.state),
        ],
    )?;
    Ok(())
}

/// Enregistre la taille d'une tâche et son découpage initial dans une seule transaction
/// (SPEC §7.1, étape 5) : la base ne connaît jamais la taille sans les segments.
pub fn save_initial_segments(
    conn: &mut Connection,
    task_id: &str,
    total: u64,
    segments: &[Segment],
) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: i64 = tx.query_row(
        "SELECT COUNT(*) FROM segments WHERE task_id = ?1",
        [task_id],
        |row| row.get(0),
    )?;
    if existing > 0 {
        return Err(StoreError::SegmentsAlreadyExist {
            task_id: task_id.to_owned(),
        });
    }
    let updated = tx.execute(
        "UPDATE tasks SET total_bytes = ?2, updated_at = ?3 WHERE id = ?1",
        params![task_id, to_sql_int(total, "taille")?, now_ms()],
    )?;
    if updated == 0 {
        return Err(StoreError::TaskNotFound {
            task_id: task_id.to_owned(),
        });
    }
    for s in segments {
        insert_segment(&tx, task_id, s)?;
    }
    check_task_coverage(&tx, task_id)?;
    tx.commit()?;
    Ok(())
}

/// Enregistre un redécoupage calculé par [`segmenter::split_largest`] (SPEC §7.2) : la
/// nouvelle fin du segment `split.from` et le segment `split.new`, pris dans `segments`.
pub fn save_split(
    conn: &mut Connection,
    task_id: &str,
    segments: &[Segment],
    split: Split,
) -> Result<(), StoreError> {
    let find = |idx: u32| {
        segments
            .iter()
            .find(|s| s.idx == idx)
            .ok_or_else(|| StoreError::SegmentNotFound {
                task_id: task_id.to_owned(),
                idx,
            })
    };
    let from = find(split.from)?;
    let new = find(split.new)?;

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let updated = tx.execute(
        "UPDATE segments SET end_offset = ?3 WHERE task_id = ?1 AND idx = ?2",
        params![task_id, from.idx, to_sql_int(from.end, "fin")?],
    )?;
    if updated == 0 {
        return Err(StoreError::SegmentNotFound {
            task_id: task_id.to_owned(),
            idx: from.idx,
        });
    }
    insert_segment(&tx, task_id, new)?;
    check_task_coverage(&tx, task_id)?;
    tx.commit()?;
    Ok(())
}

/// Contrôle de démarrage (SPEC §7.4, étape 2) : tâches HTTP et FTP non terminées dont les
/// segments enregistrés violent l'invariant de couverture, avec la cause.
///
/// Ne modifie rien : le moteur décide du sort de chaque tâche signalée. Une erreur de
/// lecture de la base interrompt le contrôle, car elle ne dit rien sur la tâche.
pub fn find_coverage_errors(conn: &Connection) -> Result<Vec<(String, StoreError)>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id FROM tasks
         WHERE kind IN ('http', 'ftp') AND state NOT IN ('completed', 'cancelled')
         ORDER BY id",
    )?;
    let ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut broken = Vec::new();
    for id in ids {
        match check_task_coverage(conn, &id) {
            Ok(()) => {}
            Err(
                error @ (StoreError::Coverage { .. }
                | StoreError::SegmentsWithoutSize { .. }
                | StoreError::BadValue(_)),
            ) => broken.push((id, error)),
            Err(error) => return Err(error),
        }
    }
    Ok(broken)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segmenter::{CoverageError, initial_split, split_largest};
    use crate::store::open_in_memory;
    use proptest::prelude::*;

    const MIB: u64 = 1024 * 1024;

    fn new_task(conn: &Connection, id: &str, kind: &str, state: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO queues (id, name, position, created_at)
             VALUES ('q', 'q', 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, queue_id, kind, state, position, display_name, dest_dir,
                                created_at, updated_at)
             VALUES (?1, 'q', ?2, ?3, 0, 'fichier.iso', '/tmp', 0, 0)",
            params![id, kind, state],
        )
        .unwrap();
    }

    fn raw_segment(conn: &Connection, task_id: &str, idx: i64, start: i64, end: i64) {
        conn.execute(
            "INSERT INTO segments (task_id, idx, start_offset, end_offset, state)
             VALUES (?1, ?2, ?3, ?4, 'pending')",
            params![task_id, idx, start, end],
        )
        .unwrap();
    }

    fn set_size(conn: &Connection, task_id: &str, total: i64) {
        conn.execute(
            "UPDATE tasks SET total_bytes = ?2 WHERE id = ?1",
            params![task_id, total],
        )
        .unwrap();
    }

    fn size(conn: &Connection, task_id: &str) -> Option<i64> {
        conn.query_row(
            "SELECT total_bytes FROM tasks WHERE id = ?1",
            [task_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn active(mut segments: Vec<Segment>) -> Vec<Segment> {
        for s in &mut segments {
            s.state = SegmentState::Active;
        }
        segments
    }

    #[test]
    fn initial_segments_are_saved_with_the_size() {
        let mut conn = open_in_memory().unwrap();
        new_task(&conn, "t1", "http", "active");
        let segments = initial_split(10 * MIB, 4, MIB);
        save_initial_segments(&mut conn, "t1", 10 * MIB, &segments).unwrap();
        assert_eq!(load_segments(&conn, "t1").unwrap(), segments);
        assert_eq!(size(&conn, "t1"), Some(10 * 1024 * 1024));
        check_task_coverage(&conn, "t1").unwrap();
    }

    #[test]
    fn invalid_initial_layout_is_rolled_back() {
        let mut conn = open_in_memory().unwrap();
        new_task(&conn, "t1", "http", "active");
        let mut segments = initial_split(10 * MIB, 4, MIB);
        segments.remove(1);
        assert!(matches!(
            save_initial_segments(&mut conn, "t1", 10 * MIB, &segments),
            Err(StoreError::Coverage {
                error: CoverageError::Gap { .. },
                ..
            })
        ));
        assert!(load_segments(&conn, "t1").unwrap().is_empty());
        assert_eq!(size(&conn, "t1"), None, "la taille doit être annulée aussi");
    }

    #[test]
    fn initial_segments_cannot_be_saved_twice() {
        let mut conn = open_in_memory().unwrap();
        new_task(&conn, "t1", "http", "active");
        let segments = initial_split(100, 2, 1);
        save_initial_segments(&mut conn, "t1", 100, &segments).unwrap();
        assert!(matches!(
            save_initial_segments(&mut conn, "t1", 100, &segments),
            Err(StoreError::SegmentsAlreadyExist { .. })
        ));
    }

    #[test]
    fn unknown_task_is_reported() {
        let mut conn = open_in_memory().unwrap();
        assert!(matches!(
            save_initial_segments(&mut conn, "absente", 100, &initial_split(100, 1, 1)),
            Err(StoreError::TaskNotFound { .. })
        ));
        assert!(matches!(
            check_task_coverage(&conn, "absente"),
            Err(StoreError::TaskNotFound { .. })
        ));
    }

    #[test]
    fn split_is_saved_and_reloaded() {
        let mut conn = open_in_memory().unwrap();
        new_task(&conn, "t1", "http", "active");
        let mut segments = active(initial_split(64 * MIB, 4, MIB));
        save_initial_segments(&mut conn, "t1", 64 * MIB, &segments).unwrap();

        let split = split_largest(&mut segments, MIB).unwrap();
        save_split(&mut conn, "t1", &segments, split).unwrap();
        assert_eq!(load_segments(&conn, "t1").unwrap(), segments);
    }

    #[test]
    fn inconsistent_split_is_rolled_back() {
        let mut conn = open_in_memory().unwrap();
        new_task(&conn, "t1", "http", "active");
        let saved = active(initial_split(64 * MIB, 4, MIB));
        save_initial_segments(&mut conn, "t1", 64 * MIB, &saved).unwrap();

        // Nouveau segment sans raccourcir le segment 0 : chevauchement.
        let mut bogus = saved.clone();
        bogus.push(Segment {
            idx: 4,
            start: saved[0].end - 9,
            end: saved[0].end,
            written: 0,
            state: SegmentState::Active,
        });
        assert!(matches!(
            save_split(&mut conn, "t1", &bogus, Split { from: 0, new: 4 }),
            Err(StoreError::Coverage {
                error: CoverageError::Overlap { .. },
                ..
            })
        ));
        assert_eq!(load_segments(&conn, "t1").unwrap(), saved);
    }

    #[test]
    fn split_of_an_unknown_segment_is_refused() {
        let mut conn = open_in_memory().unwrap();
        new_task(&conn, "t1", "http", "active");
        let mut segments = active(initial_split(64 * MIB, 4, MIB));
        save_initial_segments(&mut conn, "t1", 64 * MIB, &segments).unwrap();
        let split = split_largest(&mut segments, MIB).unwrap();

        assert!(matches!(
            save_split(&mut conn, "t1", &segments, Split { from: 99, ..split }),
            Err(StoreError::SegmentNotFound { idx: 99, .. })
        ));
        // Segment absent de la base mais présent dans la liste : rien n'est enregistré.
        segments[0].idx = 50;
        assert!(matches!(
            save_split(&mut conn, "t1", &segments, Split { from: 50, ..split }),
            Err(StoreError::SegmentNotFound { idx: 50, .. })
        ));
        assert_eq!(load_segments(&conn, "t1").unwrap().len(), 4);
    }

    #[test]
    fn task_of_unknown_size_has_no_segments() {
        let conn = open_in_memory().unwrap();
        new_task(&conn, "t1", "http", "active");
        check_task_coverage(&conn, "t1").unwrap();
        raw_segment(&conn, "t1", 0, 0, 99);
        assert!(matches!(
            check_task_coverage(&conn, "t1"),
            Err(StoreError::SegmentsWithoutSize { count: 1, .. })
        ));
    }

    #[test]
    fn negative_offsets_are_reported() {
        let conn = open_in_memory().unwrap();
        new_task(&conn, "t1", "http", "active");
        set_size(&conn, "t1", 100);
        raw_segment(&conn, "t1", 0, -5, 99);
        assert!(matches!(
            load_segments(&conn, "t1"),
            Err(StoreError::BadValue(_))
        ));
    }

    #[test]
    fn startup_check_reports_only_broken_unfinished_transfers() {
        let mut conn = open_in_memory().unwrap();
        new_task(&conn, "sain", "http", "active");
        save_initial_segments(&mut conn, "sain", 100, &initial_split(100, 2, 1)).unwrap();

        new_task(&conn, "troue", "ftp", "paused");
        set_size(&conn, "troue", 100);
        raw_segment(&conn, "troue", 0, 0, 49);

        new_task(&conn, "sans-taille", "http", "queued");

        // Segments purgés en fin de vie (règle 4) : pas d'alerte.
        new_task(&conn, "finie", "http", "completed");
        set_size(&conn, "finie", 100);
        new_task(&conn, "annulee", "http", "cancelled");
        set_size(&conn, "annulee", 100);
        // Torrent et vidéo n'utilisent pas la table segments.
        new_task(&conn, "torrent", "torrent", "active");
        set_size(&conn, "torrent", 100);

        let broken = find_coverage_errors(&conn).unwrap();
        let ids: Vec<&str> = broken.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["troue"]);
        assert!(matches!(
            broken[0].1,
            StoreError::Coverage {
                error: CoverageError::Gap { start: 50, end: 99 },
                ..
            }
        ));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Les redécoupages enregistrés un par un donnent en base exactement l'état calculé
        /// en mémoire, et chaque transaction a vérifié la couverture.
        #[test]
        fn saved_splits_match_memory(
            total in 1..(1u64 << 36),
            connections in 1..=32u32,
            min_size in prop_oneof![1..8u64, 1..=4 * MIB],
            writes in prop::collection::vec((any::<usize>(), 0..64 * MIB), 0..60),
        ) {
            let mut conn = open_in_memory().unwrap();
            new_task(&conn, "t1", "http", "active");
            let mut segments = active(initial_split(total, connections, min_size));
            save_initial_segments(&mut conn, "t1", total, &segments).unwrap();

            for (pick, bytes) in writes {
                // Octets reçus en mémoire, pas encore enregistrés : ils déplacent le milieu.
                let k = pick % segments.len();
                let s = &mut segments[k];
                s.written += bytes.min(s.remaining());
                if let Some(split) = split_largest(&mut segments, min_size) {
                    save_split(&mut conn, "t1", &segments, split).unwrap();
                }
                let ranges = |list: &[Segment]| -> Vec<(u32, u64, u64)> {
                    list.iter().map(|s| (s.idx, s.start, s.end)).collect()
                };
                prop_assert_eq!(ranges(&load_segments(&conn, "t1").unwrap()), ranges(&segments));
            }
        }
    }
}
