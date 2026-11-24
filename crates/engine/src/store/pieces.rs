//! Empreintes de morceaux d'une tâche (F-12, SPEC §7.9, table `task_pieces`, migration
//! 12) et tours de reprise des morceaux faux.
//!
//! Un tour s'enregistre en une seule transaction ([`start_round`]) : compte des tours et
//! des morceaux, sources mises en cause (écartées à [`SET_ASIDE_AFTER`] morceaux faux),
//! segments rouverts sur les morceaux faux, et passage de `verifying` à `active`. Après un
//! arrêt, la tâche est soit encore en `verifying` (la vérification se refait), soit en
//! `active` avec ses morceaux à télécharger.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::StoreError;
use super::tasks::{Change, TaskState, change_state_in};
use crate::io::checksum::Algo;
use crate::pieces::{Pieces, SET_ASIDE_AFTER};

/// Empreintes de morceaux d'une tâche et ses tours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPieces {
    pub pieces: Pieces,
    /// Tours faits depuis la dernière relance.
    pub rounds: u32,
    /// Morceaux retéléchargés en tout.
    pub redone: u64,
}

/// Enregistre les empreintes de morceaux d'une tâche, dans la transaction de l'appelant.
pub(super) fn save_in(conn: &Connection, task_id: &str, pieces: &Pieces) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO task_pieces (task_id, algo, length, hashes) VALUES (?1, ?2, ?3, ?4)",
        params![
            task_id,
            pieces.algo().as_sql(),
            super::to_i64(pieces.length(), "longueur de morceau")?,
            pieces.to_sql()
        ],
    )?;
    Ok(())
}

/// Empreintes de morceaux d'une tâche, si elle en a.
pub fn load(conn: &Connection, task_id: &str) -> Result<Option<TaskPieces>, StoreError> {
    let row = conn
        .query_row(
            "SELECT algo, length, hashes, rounds, redone FROM task_pieces WHERE task_id = ?1",
            [task_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((algo, length, hashes, rounds, redone)) = row else {
        return Ok(None);
    };
    let bad = |what: &str| StoreError::BadValue(format!("tâche {task_id} : {what}"));
    let algo = Algo::from_sql(&algo).ok_or_else(|| bad("algorithme de morceaux inconnu"))?;
    let length = u64::try_from(length).map_err(|_| bad("longueur de morceau négative"))?;
    let pieces = Pieces::from_sql(algo, length, &hashes).map_err(|e| bad(&e))?;
    Ok(Some(TaskPieces {
        pieces,
        rounds: u32::try_from(rounds).unwrap_or(u32::MAX),
        redone: u64::try_from(redone).unwrap_or(0),
    }))
}

/// Un tour de reprise des morceaux faux (F-12) : `bad`, les plages de chaque morceau faux
/// (une par morceau, pour mettre en cause leurs sources), `merged`, les mêmes plages
/// réunies (segments rouverts). Passe la tâche de `verifying` à `active`. Renvoie les
/// sources écartées par ce tour.
pub fn start_round(
    conn: &mut Connection,
    task_id: &str,
    bad: &[(u64, u64)],
    merged: &[(u64, u64)],
) -> Result<Vec<String>, StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let updated = tx.execute(
        "UPDATE task_pieces SET rounds = rounds + 1, redone = redone + ?2 WHERE task_id = ?1",
        params![task_id, bad.len() as i64],
    )?;
    if updated == 0 {
        return Err(StoreError::BadValue(format!(
            "tâche {task_id} : pas d'empreintes de morceaux"
        )));
    }
    // Un morceau faux compte contre la source qui l'a envoyé seule. Venu de plusieurs, il
    // ne dit pas laquelle s'est trompée : aucune n'est mise en cause.
    let mut blamed: Vec<String> = Vec::new();
    for &(start, end) in bad {
        let mut senders = super::segments::sources_over(&tx, task_id, start, end)?;
        if senders.len() == 1 {
            blamed.append(&mut senders);
        }
    }
    blamed.sort_unstable();
    // Morceaux faux de ce tour, par source.
    let mut tally: Vec<(String, i64)> = Vec::new();
    for source_id in blamed {
        match tally.last_mut() {
            Some((last, n)) if *last == source_id => *n += 1,
            _ => tally.push((source_id, 1)),
        }
    }
    let mut set_aside = Vec::new();
    for (source_id, added) in &tally {
        tx.execute(
            "UPDATE sources SET bad_pieces = bad_pieces + ?3 WHERE id = ?1 AND task_id = ?2",
            params![source_id, task_id, added],
        )?;
        let count: i64 = tx.query_row(
            "SELECT bad_pieces FROM sources WHERE id = ?1",
            [source_id],
            |r| r.get(0),
        )?;
        // Écartée quand ce tour lui fait franchir le seuil, une seule fois.
        let limit = i64::from(SET_ASIDE_AFTER);
        if count >= limit && count - added < limit {
            tx.execute(
                "UPDATE sources SET state = 'failed', last_error = ?2 WHERE id = ?1",
                params![
                    source_id,
                    format!("{SET_ASIDE_AFTER} morceaux faux envoyés")
                ],
            )?;
            set_aside.push(source_id.clone());
        }
    }
    super::segments::reopen_in(&tx, task_id, merged)?;
    change_state_in(&tx, task_id, &Change::to(TaskState::Active))?;
    tx.commit()?;
    Ok(set_aside)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segmenter::{Segment, SegmentState};
    use crate::store::open;
    use crate::store::segments::{load_segments, save_initial_segments, set_source};
    use crate::store::tasks::{
        NewTask, change_state, create_task_with_pieces, ensure_queue, load_sources, load_task,
        restart_task,
    };

    fn hex(c: char) -> String {
        std::iter::repeat_n(c, 32).collect()
    }

    /// Tâche de 30 octets en morceaux de 10, deux sources, deux segments terminés
    /// (0-14 par la référence, 15-29 par le miroir), en `verifying`.
    fn verifying_task(conn: &mut Connection) -> (String, Vec<String>) {
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
        let pieces = Pieces::new(Algo::Md5, 10, vec![hex('0'), hex('1'), hex('2')]).unwrap();
        let (id, _) = create_task_with_pieces(
            conn,
            &new,
            &["https://b.exemple/f.iso".into()],
            Some(&pieces),
            Default::default(),
        )
        .unwrap();
        let done = |idx, start, end: u64| Segment {
            idx,
            start,
            end,
            written: end - start + 1,
            state: SegmentState::Done,
        };
        save_initial_segments(conn, &id, 30, &[done(0, 0, 14), done(1, 15, 29)]).unwrap();
        let sources: Vec<String> = load_sources(conn, &id)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        set_source(conn, &id, 0, &sources[0]).unwrap();
        set_source(conn, &id, 1, &sources[1]).unwrap();
        for state in [TaskState::Active, TaskState::Verifying] {
            change_state(conn, &id, &Change::to(state)).unwrap();
        }
        (id, sources)
    }

    #[test]
    fn a_round_reopens_bad_pieces_and_blames_their_sources() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(&dir.path().join("auro.db")).unwrap();
        let (id, sources) = verifying_task(&mut conn);
        let loaded = load(&conn, &id).unwrap().unwrap();
        assert_eq!(
            (loaded.pieces.count(), loaded.rounds, loaded.redone),
            (3, 0, 0)
        );

        // Morceau 1 (octets 10 à 19) : à cheval sur les deux sources, qui ne sont pas mises
        // en cause ; il est rouvert sur les deux segments.
        let set_aside = start_round(&mut conn, &id, &[(10, 19)], &[(10, 19)]).unwrap();
        assert!(set_aside.is_empty());
        let task = load_task(&conn, &id).unwrap();
        assert_eq!((task.state, task.done_bytes), (TaskState::Active, 20));
        let mut pending: Vec<(u64, u64)> = load_segments(&conn, &id)
            .unwrap()
            .iter()
            .filter(|s| s.state == SegmentState::Pending)
            .map(|s| (s.start, s.end))
            .collect();
        pending.sort_unstable();
        assert_eq!(pending, [(10, 14), (15, 19)]);
        let counts: Vec<u32> = load_sources(&conn, &id)
            .unwrap()
            .iter()
            .map(|s| s.bad_pieces)
            .collect();
        assert_eq!(counts, [0, 0]);
        let loaded = load(&conn, &id).unwrap().unwrap();
        assert_eq!((loaded.rounds, loaded.redone), (1, 1));

        // Les morceaux rouverts téléchargés de nouveau, par le miroir seul ; il envoie faux
        // les deux derniers morceaux : écarté à 2, d'un seul tour.
        let mut finished = load_segments(&conn, &id).unwrap();
        for s in &mut finished {
            s.written = s.size();
            s.state = SegmentState::Done;
        }
        crate::store::segments::clear(&conn, &id).unwrap();
        conn.execute("UPDATE tasks SET total_bytes = NULL WHERE id = ?1", [&id])
            .unwrap();
        save_initial_segments(&mut conn, &id, 30, &finished).unwrap();
        for s in &finished {
            set_source(&conn, &id, s.idx, &sources[1]).unwrap();
        }
        change_state(&mut conn, &id, &Change::to(TaskState::Verifying)).unwrap();
        let set_aside = start_round(&mut conn, &id, &[(10, 19), (20, 29)], &[(10, 29)]).unwrap();
        assert_eq!(set_aside, [sources[1].clone()]);
        let mirror = &load_sources(&conn, &id).unwrap()[1];
        assert_eq!(mirror.bad_pieces, 2);
        assert_eq!(mirror.state, crate::store::tasks::SourceState::Failed);

        // Une relance manuelle redonne les tours ; un retéléchargement efface tout.
        let conn2 = &mut conn;
        change_state(
            conn2,
            &id,
            &Change {
                to: TaskState::Paused,
                pause_reason: Some(crate::store::tasks::PauseReason::User),
                error: None,
                next_retry_at: None,
            },
        )
        .unwrap();
        // Remise en file automatique (arrêt du moteur, plage) : les tours restent (audit
        // S4) ; une relance de l'utilisateur les redonne.
        change_state(conn2, &id, &Change::to(TaskState::Queued)).unwrap();
        let loaded = load(conn2, &id).unwrap().unwrap();
        assert_eq!(
            (loaded.rounds, loaded.redone),
            (2, 3),
            "les deux tours restent"
        );
        let pause = Change {
            to: TaskState::Paused,
            pause_reason: Some(crate::store::tasks::PauseReason::User),
            error: None,
            next_retry_at: None,
        };
        change_state(conn2, &id, &pause).unwrap();
        crate::store::tasks::change_state_by_user(conn2, &id).unwrap();
        let loaded = load(conn2, &id).unwrap().unwrap();
        assert_eq!((loaded.rounds, loaded.redone), (0, 3));
        let pause = Change {
            to: TaskState::Paused,
            pause_reason: Some(crate::store::tasks::PauseReason::User),
            error: None,
            next_retry_at: None,
        };
        change_state(conn2, &id, &pause).unwrap();
        restart_task(conn2, &id).unwrap();
        let loaded = load(conn2, &id).unwrap().unwrap();
        assert_eq!((loaded.rounds, loaded.redone), (0, 0));
        assert!(
            load_sources(conn2, &id)
                .unwrap()
                .iter()
                .all(|s| s.bad_pieces == 0)
        );
    }

    #[test]
    fn a_round_needs_a_verifying_task_and_changes_nothing_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(&dir.path().join("auro.db")).unwrap();
        let (id, _) = verifying_task(&mut conn);
        // Déjà active : la transition est refusée, et le tour annulé en entier.
        start_round(&mut conn, &id, &[(0, 9)], &[(0, 9)]).unwrap();
        assert!(start_round(&mut conn, &id, &[(0, 9)], &[(0, 9)]).is_err());
        let loaded = load(&conn, &id).unwrap().unwrap();
        assert_eq!((loaded.rounds, loaded.redone), (1, 1));
        assert!(
            load_sources(&conn, &id)
                .unwrap()
                .iter()
                .all(|s| s.bad_pieces <= 1)
        );
    }
}
