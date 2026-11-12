//! Plugins installés (F-41, SPEC §7.20) : manifeste, empreinte du fichier, permissions
//! acceptées et activation. Le fichier lui-même est sous `plugins/` à côté de la base
//! (module `plugins`) ; ce module ne touche qu'à la base.

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{StoreError, now_ms};

/// Un plugin, tel que la base le garde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    /// SHA-256 du fichier `.wasm`, en hexadécimal minuscule.
    pub digest: String,
    /// Domaines des pages prises en charge.
    pub sites: Vec<String>,
    /// Domaines que le plugin peut faire demander au moteur.
    pub fetch: Vec<String>,
    pub enabled: bool,
    pub installed_at: i64,
}

const COLUMNS: &str =
    "id, name, version, author, description, digest, sites, fetch, enabled, installed_at";

fn list_of(text: String) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(&text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn row(r: &Row<'_>) -> rusqlite::Result<PluginRow> {
    Ok(PluginRow {
        id: r.get(0)?,
        name: r.get(1)?,
        version: r.get(2)?,
        author: r.get(3)?,
        description: r.get(4)?,
        digest: r.get(5)?,
        sites: list_of(r.get(6)?)?,
        fetch: list_of(r.get(7)?)?,
        enabled: r.get(8)?,
        installed_at: r.get(9)?,
    })
}

/// Tous les plugins, par nom.
pub fn list(conn: &Connection) -> Result<Vec<PluginRow>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM plugins ORDER BY name COLLATE NOCASE, id"
    ))?;
    let rows = stmt.query_map([], row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn load(conn: &Connection, id: &str) -> Result<Option<PluginRow>, StoreError> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM plugins WHERE id = ?1"),
            [id],
            row,
        )
        .optional()?)
}

/// Installe un plugin, ou remplace la version installée sous le même identifiant (activé
/// de nouveau). Rend la ligne enregistrée et l'empreinte du fichier remplacé, s'il y en
/// avait un autre.
pub fn install(
    conn: &Connection,
    plugin: &PluginRow,
) -> Result<(PluginRow, Option<String>), StoreError> {
    let previous = load(conn, &plugin.id)?;
    let sites = serde_json::to_string(&plugin.sites).unwrap_or_default();
    let fetch = serde_json::to_string(&plugin.fetch).unwrap_or_default();
    conn.execute(
        "INSERT INTO plugins (id, name, version, author, description, digest, sites, fetch,
                              enabled, installed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)
         ON CONFLICT (id) DO UPDATE SET
           name = excluded.name, version = excluded.version, author = excluded.author,
           description = excluded.description, digest = excluded.digest,
           sites = excluded.sites, fetch = excluded.fetch, enabled = 1,
           installed_at = excluded.installed_at",
        params![
            plugin.id,
            plugin.name,
            plugin.version,
            plugin.author,
            plugin.description,
            plugin.digest,
            sites,
            fetch,
            now_ms()
        ],
    )?;
    let saved = load(conn, &plugin.id)?
        .ok_or_else(|| StoreError::BadValue("plugin introuvable après son écriture".into()))?;
    let replaced = previous
        .map(|p| p.digest)
        .filter(|digest| *digest != saved.digest);
    Ok((saved, replaced))
}

/// Active ou désactive un plugin ; faux s'il n'existe pas.
pub fn set_enabled(conn: &Connection, id: &str, enabled: bool) -> Result<bool, StoreError> {
    Ok(conn.execute(
        "UPDATE plugins SET enabled = ?2 WHERE id = ?1",
        params![id, enabled],
    )? > 0)
}

/// Retire un plugin ; rend l'empreinte de son fichier, à effacer ensuite.
pub fn remove(conn: &Connection, id: &str) -> Result<Option<String>, StoreError> {
    let Some(found) = load(conn, id)? else {
        return Ok(None);
    };
    conn.execute("DELETE FROM plugins WHERE id = ?1", [id])?;
    Ok(Some(found.digest))
}

/// Un autre plugin utilise-t-il encore ce fichier ?
pub fn digest_in_use(conn: &Connection, digest: &str) -> Result<bool, StoreError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM plugins WHERE digest = ?1 LIMIT 1",
            [digest],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, digest: &str) -> PluginRow {
        PluginRow {
            id: id.into(),
            name: "Fichiers".into(),
            version: "1.0".into(),
            author: Some("moi".into()),
            description: None,
            digest: digest.into(),
            sites: vec!["fichiers.exemple".into()],
            fetch: vec!["fichiers.exemple".into(), "cdn.exemple".into()],
            enabled: true,
            installed_at: 0,
        }
    }

    #[test]
    fn plugins_are_installed_replaced_disabled_and_removed() {
        let conn = crate::store::open_in_memory().unwrap();
        let (saved, replaced) = install(&conn, &sample("org.fichiers", "aa")).unwrap();
        assert_eq!(saved.fetch, ["fichiers.exemple", "cdn.exemple"]);
        assert!(saved.enabled);
        assert_eq!(replaced, None);
        assert!(set_enabled(&conn, "org.fichiers", false).unwrap());
        assert!(!load(&conn, "org.fichiers").unwrap().unwrap().enabled);
        // Nouvelle version : réactivée, ancien fichier rendu.
        let (saved, replaced) = install(&conn, &sample("org.fichiers", "bb")).unwrap();
        assert!(saved.enabled);
        assert_eq!(replaced.as_deref(), Some("aa"));
        // Même fichier réinstallé : rien à effacer.
        assert_eq!(
            install(&conn, &sample("org.fichiers", "bb")).unwrap().1,
            None
        );
        assert!(digest_in_use(&conn, "bb").unwrap());
        assert_eq!(
            remove(&conn, "org.fichiers").unwrap().as_deref(),
            Some("bb")
        );
        assert!(!digest_in_use(&conn, "bb").unwrap());
        assert_eq!(remove(&conn, "org.fichiers").unwrap(), None);
        assert!(!set_enabled(&conn, "org.fichiers", true).unwrap());
        assert!(list(&conn).unwrap().is_empty());
    }
}
