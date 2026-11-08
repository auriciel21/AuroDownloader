//! Réglages du moteur (table `settings`, SPEC §5.2) : une valeur JSON par clé.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::{StoreError, now_ms};

/// Limite de débit descendant globale, en octets par seconde ; `null` : aucune (F-15).
pub const DOWNLOAD_LIMIT: &str = "limits.downloadBps";

/// Dossier des téléchargements ajoutés sans `destDir` ; `null` : le dossier
/// « Téléchargements » du système.
pub const DOWNLOAD_DIR: &str = "downloads.dir";

/// Réserver la place de chaque fichier au départ (F-10, SPEC §7.6 bis) ; vrai par défaut.
pub const PREALLOCATE: &str = "downloads.preallocate";

/// Chercher l'empreinte d'un fichier dans ses fichiers voisins (`.sha256`, `SHA256SUMS`…) quand
/// l'utilisateur n'en a pas donné (F-11, SPEC §7.9) ; vrai par défaut.
pub const NEIGHBOR_CHECKSUM: &str = "downloads.neighborChecksum";

/// Espace de noms des secrets de cette base dans le coffre du système (`secrets`), tiré
/// au hasard au premier démarrage. Interne : `settings.get` ne le montre pas.
pub const VAULT_NAMESPACE: &str = "vault.namespace";

/// Espace de noms des secrets de cette base, créé s'il n'existe pas encore.
pub fn vault_namespace(conn: &Connection) -> Result<String, StoreError> {
    match get(conn, VAULT_NAMESPACE)? {
        Some(Value::String(ns)) if !ns.is_empty() => Ok(ns),
        None => {
            let ns = uuid::Uuid::now_v7().simple().to_string();
            set(conn, VAULT_NAMESPACE, &Value::from(ns.as_str()))?;
            Ok(ns)
        }
        Some(v) => Err(StoreError::BadValue(format!(
            "réglage {VAULT_NAMESPACE} : {v} n'est pas un espace de noms"
        ))),
    }
}

/// Dossier des téléchargements enregistré ; `None` : aucun (celui du système).
pub fn download_dir(conn: &Connection) -> Result<Option<String>, StoreError> {
    match get(conn, DOWNLOAD_DIR)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(dir)) => Ok(Some(dir)),
        Some(v) => Err(StoreError::BadValue(format!(
            "réglage {DOWNLOAD_DIR} : {v} n'est pas un dossier"
        ))),
    }
}

/// Enregistre le dossier des téléchargements ; `None` : celui du système.
pub fn set_download_dir(conn: &Connection, dir: Option<&str>) -> Result<(), StoreError> {
    set(conn, DOWNLOAD_DIR, &dir.map_or(Value::Null, Value::from))
}

/// Réglages oui ou non du moteur, vrais tant que rien n'est enregistré.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flag {
    /// Réserver la place de chaque fichier au départ (F-10, SPEC §7.6 bis).
    Preallocate,
    /// Chercher l'empreinte d'un fichier dans ses fichiers voisins (F-11, SPEC §7.9).
    NeighborChecksum,
}

impl Flag {
    pub const ALL: [Flag; 2] = [Flag::Preallocate, Flag::NeighborChecksum];

    /// Clé du réglage, dans la base et dans le protocole.
    pub fn key(self) -> &'static str {
        match self {
            Flag::Preallocate => PREALLOCATE,
            Flag::NeighborChecksum => NEIGHBOR_CHECKSUM,
        }
    }

    pub fn from_key(key: &str) -> Option<Flag> {
        Flag::ALL.into_iter().find(|f| f.key() == key)
    }
}

/// Valeur d'un réglage oui ou non ; vraie tant que rien n'est enregistré.
pub fn flag(conn: &Connection, flag: Flag) -> Result<bool, StoreError> {
    let key = flag.key();
    match get(conn, key)? {
        None | Some(Value::Null) => Ok(true),
        Some(Value::Bool(on)) => Ok(on),
        Some(v) => Err(StoreError::BadValue(format!(
            "réglage {key} : {v} n'est pas un booléen"
        ))),
    }
}

pub fn set_flag(conn: &Connection, flag: Flag, on: bool) -> Result<(), StoreError> {
    set(conn, flag.key(), &Value::Bool(on))
}
/// Valeur d'un réglage ; `None` s'il n'a jamais été enregistré.
pub fn get(conn: &Connection, key: &str) -> Result<Option<Value>, StoreError> {
    let text: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    text.map(|t| {
        serde_json::from_str(&t)
            .map_err(|e| StoreError::BadValue(format!("réglage {key} illisible : {e}")))
    })
    .transpose()
}

/// Enregistre un réglage.
pub fn set(conn: &Connection, key: &str, value: &Value) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO settings (key, value_json, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (key) DO UPDATE SET value_json = ?2, updated_at = ?3",
        params![key, value.to_string(), now_ms()],
    )?;
    Ok(())
}

/// Limite de débit globale ; `None` : aucune.
pub fn download_limit(conn: &Connection) -> Result<Option<u64>, StoreError> {
    match get(conn, DOWNLOAD_LIMIT)? {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n > 0 => Ok(Some(n)),
            _ => Err(StoreError::BadValue(format!(
                "réglage {DOWNLOAD_LIMIT} : {v} n'est pas un débit"
            ))),
        },
    }
}

/// Enregistre la limite de débit globale ; `None` : aucune.
pub fn set_download_limit(conn: &Connection, limit: Option<u64>) -> Result<(), StoreError> {
    set(
        conn,
        DOWNLOAD_LIMIT,
        &limit.map_or(Value::Null, Value::from),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_in_memory;

    #[test]
    fn the_download_limit_is_kept_and_removed() {
        let conn = open_in_memory().unwrap();
        assert_eq!(download_limit(&conn).unwrap(), None);
        set_download_limit(&conn, Some(2_000_000)).unwrap();
        assert_eq!(download_limit(&conn).unwrap(), Some(2_000_000));
        set_download_limit(&conn, None).unwrap();
        assert_eq!(download_limit(&conn).unwrap(), None);
        assert_eq!(get(&conn, DOWNLOAD_LIMIT).unwrap(), Some(Value::Null));
    }

    #[test]
    fn the_download_dir_is_kept_and_reset() {
        let conn = open_in_memory().unwrap();
        assert_eq!(download_dir(&conn).unwrap(), None);
        set_download_dir(&conn, Some("/srv/dl")).unwrap();
        assert_eq!(download_dir(&conn).unwrap().as_deref(), Some("/srv/dl"));
        set_download_dir(&conn, None).unwrap();
        assert_eq!(download_dir(&conn).unwrap(), None);
        set(&conn, DOWNLOAD_DIR, &Value::from(3)).unwrap();
        assert!(download_dir(&conn).is_err());
    }

    #[test]
    fn a_damaged_setting_is_reported() {
        let conn = open_in_memory().unwrap();
        set(&conn, DOWNLOAD_LIMIT, &Value::from("vite")).unwrap();
        assert!(matches!(
            download_limit(&conn),
            Err(StoreError::BadValue(_))
        ));
    }
}
