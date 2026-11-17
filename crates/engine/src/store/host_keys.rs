//! Clés des serveurs SSH acceptées par l'utilisateur (SFTP, F-08, SPEC §7.22) : une par
//! `hôte:port`, par son empreinte SHA-256.

use rusqlite::{Connection, Row, params};

use super::{StoreError, now_ms};

/// Une clé acceptée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyRow {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    pub accepted_at: i64,
}

fn row(r: &Row<'_>) -> rusqlite::Result<HostKeyRow> {
    Ok(HostKeyRow {
        host: r.get(0)?,
        port: r.get(1)?,
        algorithm: r.get(2)?,
        fingerprint: r.get(3)?,
        accepted_at: r.get(4)?,
    })
}

/// Toutes les clés acceptées, par hôte.
pub fn list(conn: &Connection) -> Result<Vec<HostKeyRow>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT host, port, algorithm, fingerprint, accepted_at FROM ssh_host_keys
         ORDER BY host, port",
    )?;
    let rows = stmt.query_map([], row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Accepte une clé pour `host:port`, à la place de celle d'avant.
pub fn accept(
    conn: &Connection,
    host: &str,
    port: u16,
    algorithm: &str,
    fingerprint: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO ssh_host_keys (host, port, algorithm, fingerprint, accepted_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (host, port) DO UPDATE SET algorithm = ?3, fingerprint = ?4,
           accepted_at = ?5",
        params![host, port, algorithm, fingerprint, now_ms()],
    )?;
    Ok(())
}

/// Oublie la clé de `host:port` ; `false` s'il n'y en avait pas.
pub fn remove(conn: &Connection, host: &str, port: u16) -> Result<bool, StoreError> {
    Ok(conn.execute(
        "DELETE FROM ssh_host_keys WHERE host = ?1 AND port = ?2",
        params![host, port],
    )? > 0)
}

/// Tâches arrêtées par une clé SSH inconnue ou changée, et les adresses de leurs sources.
pub fn blocked_tasks(conn: &Connection) -> Result<Vec<(String, String)>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, s.url FROM tasks t JOIN sources s ON s.task_id = t.id
         WHERE t.state = 'failed' AND t.parent_id IS NULL
           AND t.error_code IN ('HOST_KEY_UNKNOWN', 'HOST_KEY_CHANGED')",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_in_memory;

    #[test]
    fn one_key_per_server_replaced_and_removed() {
        let conn = open_in_memory().unwrap();
        accept(&conn, "h", 22, "ssh-ed25519", "SHA256:a").unwrap();
        accept(&conn, "h", 2222, "ssh-ed25519", "SHA256:b").unwrap();
        accept(&conn, "h", 22, "ssh-rsa", "SHA256:c").unwrap();
        let keys = list(&conn).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(
            (keys[0].algorithm.as_str(), keys[0].fingerprint.as_str()),
            ("ssh-rsa", "SHA256:c")
        );
        assert!(remove(&conn, "h", 22).unwrap());
        assert!(!remove(&conn, "h", 22).unwrap());
        assert_eq!(list(&conn).unwrap().len(), 1);
    }
}
