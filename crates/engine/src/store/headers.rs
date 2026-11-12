//! En-têtes HTTP d'une tâche (table `task_headers`, SPEC §5.3) : la valeur en clair, ou la
//! référence d'un secret rangé dans le coffre du système (`secrets`, SPEC §5.1).

use std::collections::HashSet;

use rusqlite::{Connection, Transaction, params};

use super::StoreError;

/// Valeur d'un en-tête enregistré.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stored {
    Plain(String),
    /// Référence d'un secret du coffre.
    Secret(String),
}

/// Un en-tête et sa valeur enregistrée.
pub type StoredHeader = (String, Stored);

/// Enregistre les en-têtes d'une tâche, dans la transaction qui la crée.
pub(super) fn insert(
    tx: &Transaction<'_>,
    task_id: &str,
    headers: &[StoredHeader],
) -> Result<(), StoreError> {
    let mut statement = tx.prepare(
        "INSERT INTO task_headers (task_id, name, value, secret_ref) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (name, stored) in headers {
        let (value, secret_ref) = match stored {
            Stored::Plain(v) => (Some(v.as_str()), None),
            Stored::Secret(r) => (None, Some(r.as_str())),
        };
        statement.execute(params![task_id, name, value, secret_ref])?;
    }
    Ok(())
}

/// En-têtes de la tâche, par nom.
pub fn load(conn: &Connection, task_id: &str) -> Result<Vec<StoredHeader>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT name, value, secret_ref FROM task_headers WHERE task_id = ?1 ORDER BY name",
    )?;
    let rows = statement.query_map([task_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut headers = Vec::new();
    for row in rows {
        let (name, value, secret_ref) = row?;
        let stored = match (value, secret_ref) {
            (Some(v), None) => Stored::Plain(v),
            (None, Some(r)) => Stored::Secret(r),
            _ => {
                return Err(StoreError::BadValue(format!(
                    "tâche {task_id}, en-tête {name} : ni valeur ni secret, ou les deux"
                )));
            }
        };
        headers.push((name, stored));
    }
    Ok(headers)
}

/// Références des secrets de la tâche.
pub fn secret_refs(conn: &Connection, task_id: &str) -> Result<Vec<String>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT secret_ref FROM task_headers WHERE task_id = ?1 AND secret_ref IS NOT NULL",
    )?;
    let refs = statement
        .query_map([task_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(refs)
}

/// Remplace tous les en-têtes d'une tâche (lien neuf rendu par un plugin, SPEC §7.20), en
/// une transaction ; renvoie les références des secrets remplacés, à effacer ensuite du
/// coffre (les nouveaux secrets y sont déjà, SPEC §5.1).
pub fn replace(
    conn: &mut Connection,
    task_id: &str,
    headers: &[StoredHeader],
) -> Result<Vec<String>, StoreError> {
    let tx = conn.transaction()?;
    let old: Vec<String> = {
        let mut statement = tx.prepare(
            "SELECT secret_ref FROM task_headers WHERE task_id = ?1 AND secret_ref IS NOT NULL",
        )?;
        let refs = statement.query_map([task_id], |r| r.get(0))?;
        refs.collect::<Result<_, _>>()?
    };
    tx.execute("DELETE FROM task_headers WHERE task_id = ?1", [task_id])?;
    insert(&tx, task_id, headers)?;
    tx.commit()?;
    Ok(old)
}

/// Retire de la base les en-têtes secrets de la tâche ; renvoie leurs références, à
/// effacer ensuite du coffre (SPEC §5.1 : la référence d'abord).
pub fn drop_secrets(conn: &Connection, task_id: &str) -> Result<Vec<String>, StoreError> {
    let refs = secret_refs(conn, task_id)?;
    if !refs.is_empty() {
        conn.execute(
            "DELETE FROM task_headers WHERE task_id = ?1 AND secret_ref IS NOT NULL",
            [task_id],
        )?;
    }
    Ok(refs)
}

/// Références de tous les secrets de la base : en-têtes de toutes les tâches et mots de
/// passe des identifiants de sites (`store::logins`). Le ramassage du démarrage efface du
/// coffre ce qui n'y est pas.
pub fn all_secret_refs(conn: &Connection) -> Result<HashSet<String>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT secret_ref FROM task_headers WHERE secret_ref IS NOT NULL
         UNION SELECT secret_ref FROM site_logins",
    )?;
    let mut refs: HashSet<String> = statement
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    // Mot de passe du proxy (F-07, phase 7).
    if let (_, Some(secret)) = super::settings::proxy(conn)? {
        refs.insert(secret);
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_in_memory;
    use crate::store::tasks::{
        Change, NewTask, TaskState, change_state, create_task, ensure_queue, remove_task,
    };

    #[test]
    fn headers_are_kept_and_secrets_dropped() {
        let mut conn = open_in_memory().unwrap();
        let queue = ensure_queue(&conn, "default").unwrap();
        let headers = [
            (
                "Referer".to_owned(),
                Stored::Plain("https://exemple.org/".into()),
            ),
            ("Cookie".to_owned(), Stored::Secret("ref-1".into())),
        ];
        let new = NewTask {
            queue_id: &queue,
            url: "https://exemple.org/f.bin",
            display_name: "f.bin",
            dest_dir: "/tmp",
            max_connections: None,
            speed_limit_bps: None,
            idempotency_key: None,
            requested_name: None,
            checksum: None,
            headers: &headers,
            parent_id: None,
            public_only: false,
        };
        let (task, _) = create_task(&mut conn, &new).unwrap();
        let mut loaded = load(&conn, &task).unwrap();
        loaded.sort();
        let mut expected = headers.to_vec();
        expected.sort();
        assert_eq!(loaded, expected);
        assert_eq!(secret_refs(&conn, &task).unwrap(), ["ref-1"]);
        assert_eq!(
            all_secret_refs(&conn).unwrap(),
            HashSet::from(["ref-1".to_owned()])
        );
        assert_eq!(drop_secrets(&conn, &task).unwrap(), ["ref-1"]);
        assert_eq!(
            load(&conn, &task).unwrap(),
            [(
                "Referer".to_owned(),
                Stored::Plain("https://exemple.org/".into())
            )]
        );
        assert!(all_secret_refs(&conn).unwrap().is_empty());
        change_state(&mut conn, &task, &Change::to(TaskState::Cancelled)).unwrap();
        remove_task(&mut conn, &task).unwrap();
        assert!(load(&conn, &task).unwrap().is_empty());
    }

    /// Le mot de passe du proxy (F-07) est compté parmi les secrets gardés : le ramassage du
    /// démarrage ne l'efface pas.
    #[test]
    fn the_proxy_password_is_a_kept_secret() {
        let conn = open_in_memory().unwrap();
        assert!(all_secret_refs(&conn).unwrap().is_empty());
        let settings = crate::net::proxy::Settings {
            mode: crate::net::proxy::Mode::Manual,
            url: Some("socks5://p.lan:1080".into()),
            exceptions: Vec::new(),
            username: Some("moi".into()),
        };
        crate::store::settings::set_proxy(&conn, &settings, Some("ref-du-proxy")).unwrap();
        assert!(all_secret_refs(&conn).unwrap().contains("ref-du-proxy"));
        let (read, secret) = crate::store::settings::proxy(&conn).unwrap();
        assert_eq!((read, secret.as_deref()), (settings, Some("ref-du-proxy")));
    }
}
