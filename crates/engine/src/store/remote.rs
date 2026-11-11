//! Jetons et sessions de l'interface distante (F-39, SPEC §7.19).
//!
//! Ni jeton ni valeur de cookie en base : seulement leur empreinte SHA-256, calculée par
//! le module `remote`. Une empreinte suffit à reconnaître une valeur de 32 octets
//! aléatoires, et ne permet pas de la retrouver.

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{StoreError, new_id, now_ms};

/// Un jeton, tel que la base le garde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    pub id: String,
    pub name: String,
    /// `read`, `add` ou `control`.
    pub scope: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

/// Longueur d'un nom de jeton, en caractères.
pub const MAX_NAME: usize = 64;

/// Dernier usage noté au plus une fois par minute : pas une écriture par requête.
const TOUCH_EVERY_MS: i64 = 60_000;

const COLUMNS: &str = "t.id, t.name, t.scope, t.created_at, t.last_used_at";

fn row(r: &Row<'_>) -> rusqlite::Result<TokenRow> {
    Ok(TokenRow {
        id: r.get(0)?,
        name: r.get(1)?,
        scope: r.get(2)?,
        created_at: r.get(3)?,
        last_used_at: r.get(4)?,
    })
}

/// Enregistre un jeton dont `digest` est l'empreinte.
pub fn create_token(
    conn: &Connection,
    name: &str,
    scope: &str,
    digest: &[u8],
) -> Result<TokenRow, StoreError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(StoreError::BadValue(format!(
            "nom de jeton : 1 à {MAX_NAME} caractères, sans caractère de contrôle"
        )));
    }
    if !matches!(scope, "read" | "add" | "control") {
        return Err(StoreError::BadValue(format!(
            "portée d'un jeton : read, add ou control, pas {scope}"
        )));
    }
    let (id, now) = (new_id(), now_ms());
    conn.execute(
        "INSERT INTO remote_tokens (id, name, scope, digest, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, name, scope, digest, now],
    )?;
    Ok(TokenRow {
        id,
        name: name.to_owned(),
        scope: scope.to_owned(),
        created_at: now,
        last_used_at: None,
    })
}

/// Tous les jetons, du plus ancien au plus récent.
pub fn list_tokens(conn: &Connection) -> Result<Vec<TokenRow>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM remote_tokens t ORDER BY t.created_at, t.id"
    ))?;
    let rows = stmt.query_map([], row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Efface un jeton et ses sessions ; faux s'il n'existait pas.
pub fn revoke_token(conn: &Connection, id: &str) -> Result<bool, StoreError> {
    Ok(conn.execute("DELETE FROM remote_tokens WHERE id = ?1", [id])? > 0)
}

/// Le jeton dont c'est l'empreinte, dont l'usage est noté.
pub fn use_token(conn: &Connection, digest: &[u8]) -> Result<Option<TokenRow>, StoreError> {
    let found = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM remote_tokens t WHERE t.digest = ?1"),
            [digest],
            row,
        )
        .optional()?;
    if let Some(token) = &found {
        touch(conn, token)?;
    }
    Ok(found)
}

fn touch(conn: &Connection, token: &TokenRow) -> Result<(), StoreError> {
    let now = now_ms();
    if token
        .last_used_at
        .is_none_or(|last| now - last >= TOUCH_EVERY_MS)
    {
        conn.execute(
            "UPDATE remote_tokens SET last_used_at = ?2 WHERE id = ?1",
            params![token.id, now],
        )?;
    }
    Ok(())
}

/// Ouvre une session de `token_id`, valable jusqu'à `expires_at` (millisecondes Unix).
pub fn create_session(
    conn: &Connection,
    digest: &[u8],
    token_id: &str,
    expires_at: i64,
) -> Result<(), StoreError> {
    let now = now_ms();
    // Les sessions échues partent à chaque ouverture : la table ne grossit pas.
    conn.execute("DELETE FROM remote_sessions WHERE expires_at <= ?1", [now])?;
    conn.execute(
        "INSERT INTO remote_sessions (digest, token_id, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![digest, token_id, now, expires_at],
    )?;
    Ok(())
}

/// Le jeton d'une session encore valable, dont l'usage est noté.
pub fn use_session(conn: &Connection, digest: &[u8]) -> Result<Option<TokenRow>, StoreError> {
    let found = conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM remote_sessions s
                 JOIN remote_tokens t ON t.id = s.token_id
                 WHERE s.digest = ?1 AND s.expires_at > ?2"
            ),
            params![digest, now_ms()],
            row,
        )
        .optional()?;
    if let Some(token) = &found {
        touch(conn, token)?;
    }
    Ok(found)
}

/// Ferme une session (`POST /logout`).
pub fn delete_session(conn: &Connection, digest: &[u8]) -> Result<(), StoreError> {
    conn.execute("DELETE FROM remote_sessions WHERE digest = ?1", [digest])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_in_memory;

    #[test]
    fn tokens_and_sessions_are_found_by_digest_and_revoked_together() {
        let conn = open_in_memory().unwrap();
        let token = create_token(&conn, "  téléphone ", "control", &[1; 32]).unwrap();
        assert_eq!(token.name, "téléphone");
        assert!(create_token(&conn, "autre", "control", &[1; 32]).is_err());
        for (name, scope) in [("", "read"), ("x", "admin"), ("x", ""), ("a\nb", "read")] {
            assert!(
                create_token(&conn, name, scope, &[9; 32]).is_err(),
                "{name} {scope}"
            );
        }
        let long = "n".repeat(MAX_NAME + 1);
        assert!(create_token(&conn, &long, "read", &[8; 32]).is_err());

        let used = use_token(&conn, &[1; 32]).unwrap().unwrap();
        assert_eq!(used.id, token.id);
        assert!(use_token(&conn, &[2; 32]).unwrap().is_none());
        assert!(list_tokens(&conn).unwrap()[0].last_used_at.is_some());

        let later = now_ms() + 60_000;
        create_session(&conn, &[3; 32], &token.id, later).unwrap();
        create_session(&conn, &[4; 32], &token.id, now_ms() - 1).unwrap();
        assert_eq!(use_session(&conn, &[3; 32]).unwrap().unwrap().id, token.id);
        assert!(use_session(&conn, &[4; 32]).unwrap().is_none(), "échue");
        delete_session(&conn, &[3; 32]).unwrap();
        assert!(use_session(&conn, &[3; 32]).unwrap().is_none());

        create_session(&conn, &[5; 32], &token.id, later).unwrap();
        assert!(revoke_token(&conn, &token.id).unwrap());
        assert!(!revoke_token(&conn, &token.id).unwrap());
        assert!(use_session(&conn, &[5; 32]).unwrap().is_none());
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM remote_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }
}
