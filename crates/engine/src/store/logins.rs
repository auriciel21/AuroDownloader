//! Identifiants de sites (F-06, SPEC §7.14) : nom d'utilisateur et référence au coffre du
//! mot de passe, par site. Le mot de passe lui-même n'est jamais en base (SPEC §5.1).
//!
//! Ordre avec le coffre, comme pour les en-têtes : le secret est écrit avant sa référence,
//! la référence effacée avant le secret. Ce module ne touche qu'à la base.

use reqwest::Url;
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{StoreError, new_id, now_ms};
use crate::net::auth::Site;

/// Identifiants d'un site, tels que la base les garde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginRow {
    pub id: String,
    /// Forme normalisée du site ([`Site::text`]).
    pub site: String,
    pub username: String,
    pub secret_ref: String,
}

const COLUMNS: &str = "id, site, username, secret_ref";

fn row(r: &Row<'_>) -> rusqlite::Result<LoginRow> {
    Ok(LoginRow {
        id: r.get(0)?,
        site: r.get(1)?,
        username: r.get(2)?,
        secret_ref: r.get(3)?,
    })
}

/// Tous les identifiants, par site.
pub fn list(conn: &Connection) -> Result<Vec<LoginRow>, StoreError> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM site_logins ORDER BY site"))?;
    let rows = stmt.query_map([], row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn load(conn: &Connection, id: &str) -> Result<Option<LoginRow>, StoreError> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM site_logins WHERE id = ?1"),
            [id],
            row,
        )
        .optional()?)
}

/// Identifiants enregistrés pour ce site exactement (forme normalisée).
pub fn by_site(conn: &Connection, site: &str) -> Result<Option<LoginRow>, StoreError> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM site_logins WHERE site = ?1"),
            [site],
            row,
        )
        .optional()?)
}

/// Identifiants du site le plus précis qui couvre `url` ([`Site::matches`]).
pub fn best_for(conn: &Connection, url: &str) -> Result<Option<LoginRow>, StoreError> {
    let Ok(url) = Url::parse(url) else {
        return Ok(None);
    };
    Ok(list(conn)?
        .into_iter()
        .filter_map(|login| {
            let score = Site::parse(&login.site).ok()?.matches(&url)?;
            Some((score, login))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, login)| login))
}

/// Enregistre de nouveaux identifiants ; le mot de passe est déjà au coffre (`secret_ref`).
pub fn insert(
    conn: &Connection,
    site: &str,
    username: &str,
    secret_ref: &str,
) -> Result<String, StoreError> {
    let (id, now) = (new_id(), now_ms());
    conn.execute(
        "INSERT INTO site_logins (id, site, username, secret_ref, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, site, username, secret_ref, now],
    )?;
    Ok(id)
}

/// Change le site, le nom d'utilisateur et, si `secret_ref` est donné, la référence du mot
/// de passe. Renvoie l'ancienne référence remplacée, à effacer du coffre ensuite.
pub fn update(
    conn: &Connection,
    id: &str,
    site: &str,
    username: &str,
    secret_ref: Option<&str>,
) -> Result<Option<String>, StoreError> {
    let old = load(conn, id)?
        .ok_or_else(|| StoreError::BadValue(format!("identifiants {id} inconnus")))?;
    conn.execute(
        "UPDATE site_logins SET site = ?2, username = ?3,
           secret_ref = COALESCE(?4, secret_ref), updated_at = ?5
         WHERE id = ?1",
        params![id, site, username, secret_ref, now_ms()],
    )?;
    Ok(secret_ref
        .filter(|new| *new != old.secret_ref)
        .map(|_| old.secret_ref))
}

/// Retire des identifiants ; renvoie la référence de leur mot de passe, à effacer du coffre
/// ensuite.
pub fn delete(conn: &Connection, id: &str) -> Result<Option<String>, StoreError> {
    let Some(old) = load(conn, id)? else {
        return Ok(None);
    };
    conn.execute("DELETE FROM site_logins WHERE id = ?1", [id])?;
    Ok(Some(old.secret_ref))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::headers::all_secret_refs;
    use crate::store::open_in_memory;

    #[test]
    fn the_most_precise_site_wins() {
        let conn = open_in_memory().unwrap();
        insert(&conn, "exemple.org", "large", "r1").unwrap();
        insert(&conn, "https://dl.exemple.org/prive", "precis", "r2").unwrap();
        let pick = |url: &str| best_for(&conn, url).unwrap().map(|l| l.username);
        assert_eq!(
            pick("https://dl.exemple.org/prive/f.iso").as_deref(),
            Some("precis")
        );
        assert_eq!(
            pick("https://dl.exemple.org/public/f.iso").as_deref(),
            Some("large")
        );
        assert_eq!(pick("https://ailleurs.net/f.iso"), None);
        assert_eq!(pick("pas une adresse"), None);
    }

    #[test]
    fn a_replaced_or_removed_password_is_handed_back_for_the_vault() {
        let conn = open_in_memory().unwrap();
        let id = insert(&conn, "exemple.org", "moi", "r1").unwrap();
        assert!(
            insert(&conn, "exemple.org", "autre", "r9").is_err(),
            "un site, une fois"
        );
        // Les mots de passe restent hors du ramassage.
        assert!(all_secret_refs(&conn).unwrap().contains("r1"));
        // Sans nouveau mot de passe, la référence reste.
        assert_eq!(
            update(&conn, &id, "exemple.org", "moi2", None).unwrap(),
            None
        );
        assert_eq!(load(&conn, &id).unwrap().unwrap().secret_ref, "r1");
        assert_eq!(
            update(&conn, &id, "exemple.org", "moi2", Some("r2")).unwrap(),
            Some("r1".into())
        );
        assert_eq!(
            by_site(&conn, "exemple.org").unwrap().unwrap().username,
            "moi2"
        );
        assert_eq!(delete(&conn, &id).unwrap(), Some("r2".into()));
        assert_eq!(delete(&conn, &id).unwrap(), None);
        assert!(list(&conn).unwrap().is_empty());
    }
}
