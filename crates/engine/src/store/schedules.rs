//! Planifications enregistrées (table `schedules`, SPEC §5.2 et §7.11). Les règles sont
//! gardées en JSON, au format du protocole, et vérifiées à la lecture comme à l'écriture.

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{StoreError, new_id};
use crate::schedule::{Rules, Schedule};

const COLUMNS: &str = "id, name, rules_json, enabled";

/// Ligne brute : identifiant, nom, règles en JSON, activée.
type Raw = (String, String, String, bool);

fn raw(r: &Row<'_>) -> rusqlite::Result<Raw> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
}

fn schedule((id, name, rules_json, enabled): Raw) -> Result<Schedule, StoreError> {
    let bad = |e: String| StoreError::BadValue(format!("planification {id} : {e}"));
    let value = serde_json::from_str(&rules_json).map_err(|e| bad(e.to_string()))?;
    let rules = Rules::from_json(&value).map_err(bad)?;
    Ok(Schedule {
        id,
        name,
        enabled,
        rules,
    })
}

/// Toutes les planifications, par nom.
pub fn list_schedules(conn: &Connection) -> Result<Vec<Schedule>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM schedules ORDER BY name, id"
    ))?;
    let rows = stmt.query_map([], raw)?;
    rows.map(|r| schedule(r?)).collect()
}

/// Une planification, si elle existe.
pub fn load_schedule(conn: &Connection, id: &str) -> Result<Option<Schedule>, StoreError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM schedules WHERE id = ?1"),
        [id],
        raw,
    )
    .optional()?
    .map(schedule)
    .transpose()
}

/// Crée une planification (`id` absent) ou remplace celle qui existe ; renvoie son
/// identifiant, `None` si l'identifiant donné n'existe pas.
pub fn save_schedule(
    conn: &Connection,
    id: Option<&str>,
    name: &str,
    enabled: bool,
    rules: &Rules,
) -> Result<Option<String>, StoreError> {
    let rules_json = rules.to_json().to_string();
    match id {
        None => {
            let id = new_id();
            conn.execute(
                "INSERT INTO schedules (id, name, rules_json, enabled) VALUES (?1, ?2, ?3, ?4)",
                params![id, name, rules_json, enabled],
            )?;
            Ok(Some(id))
        }
        Some(id) => {
            let updated = conn.execute(
                "UPDATE schedules SET name = ?2, rules_json = ?3, enabled = ?4 WHERE id = ?1",
                params![id, name, rules_json, enabled],
            )?;
            Ok((updated > 0).then(|| id.to_owned()))
        }
    }
}

/// Supprime une planification ; ses files la perdent (`ON DELETE SET NULL`). `false` si
/// elle n'existait pas.
pub fn delete_schedule(conn: &Connection, id: &str) -> Result<bool, StoreError> {
    Ok(conn.execute("DELETE FROM schedules WHERE id = ?1", [id])? > 0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::store::open_in_memory;
    use crate::store::tasks::{ensure_queue, list_queues, set_queue_schedule};

    #[test]
    fn schedules_are_saved_listed_attached_and_deleted() {
        let conn = open_in_memory().unwrap();
        let rules = Rules::from_json(&json!({
            "windows": [{"days": [1, 2], "start": "23:00", "end": "07:00"}],
            "onDone": "sleep",
        }))
        .unwrap();
        let id = save_schedule(&conn, None, "Nuit", true, &rules)
            .unwrap()
            .unwrap();
        let got = load_schedule(&conn, &id).unwrap().unwrap();
        assert_eq!(got.rules, rules);
        assert!(got.enabled);

        // Remplacée ; un identifiant inconnu ne crée rien.
        assert!(
            save_schedule(&conn, Some(&id), "Nuit", false, &rules)
                .unwrap()
                .is_some()
        );
        assert!(!load_schedule(&conn, &id).unwrap().unwrap().enabled);
        assert!(
            save_schedule(&conn, Some("inconnue"), "X", true, &rules)
                .unwrap()
                .is_none()
        );
        assert_eq!(list_schedules(&conn).unwrap().len(), 1);

        let queue = ensure_queue(&conn, "default").unwrap();
        assert!(set_queue_schedule(&conn, &queue, Some(&id)).unwrap());
        assert_eq!(
            list_queues(&conn).unwrap()[0].schedule_id.as_deref(),
            Some(id.as_str())
        );
        // Une planification inconnue est refusée par la clé étrangère.
        assert!(set_queue_schedule(&conn, &queue, Some("inconnue")).is_err());

        // Supprimée : la file n'a plus de planification.
        assert!(delete_schedule(&conn, &id).unwrap());
        assert!(!delete_schedule(&conn, &id).unwrap());
        assert_eq!(list_queues(&conn).unwrap()[0].schedule_id, None);
    }
}
