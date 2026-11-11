//! Noms réservés sur le disque, pas encore enregistrés dans leur tâche (table
//! `name_claims`, SPEC §5.6, règle 7). Voir le module `claims` du moteur.

use rusqlite::{Connection, params};

use super::{StoreError, now_ms};

/// Ce qui a été créé sur le disque pour réserver le nom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimKind {
    /// `<nom>.part`, vide tant que le nom n'est pas enregistré.
    Part,
    /// Le dossier `<nom>` (torrent à plusieurs fichiers), vide de même.
    Dir,
    /// Le fichier `<nom>` lui-même, écrit puis renommé avant l'enregistrement (sous-titre
    /// donné en ligne), ou son `.part`.
    File,
}

impl ClaimKind {
    fn as_sql(self) -> &'static str {
        match self {
            ClaimKind::Part => "part",
            ClaimKind::Dir => "dir",
            ClaimKind::File => "file",
        }
    }

    fn from_sql(text: &str) -> Option<ClaimKind> {
        match text {
            "part" => Some(ClaimKind::Part),
            "dir" => Some(ClaimKind::Dir),
            "file" => Some(ClaimKind::File),
            _ => None,
        }
    }
}

/// Une réservation notée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub dir: String,
    pub name: String,
    pub kind: ClaimKind,
    pub task_id: String,
}

/// Note une réservation, avant de créer quoi que ce soit sur le disque ; faux si ce nom
/// est déjà réservé dans ce dossier.
pub fn insert(
    conn: &Connection,
    dir: &str,
    name: &str,
    kind: ClaimKind,
    task_id: &str,
) -> Result<bool, StoreError> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO name_claims (dir, name, kind, task_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![dir, name, kind.as_sql(), task_id, now_ms()],
    )?;
    Ok(inserted == 1)
}

/// Oublie une réservation (le nom n'a pas pu être pris, ou ce qui l'occupait est effacé).
pub fn delete(conn: &Connection, dir: &str, name: &str) -> Result<(), StoreError> {
    conn.execute(
        "DELETE FROM name_claims WHERE dir = ?1 AND name = ?2",
        params![dir, name],
    )?;
    Ok(())
}

/// Efface les réservations d'une tâche : à faire dans la transaction qui enregistre son
/// nom.
pub fn release(conn: &Connection, task_id: &str) -> Result<(), StoreError> {
    conn.execute("DELETE FROM name_claims WHERE task_id = ?1", [task_id])?;
    Ok(())
}

/// Toutes les réservations restantes.
pub fn all(conn: &Connection) -> Result<Vec<Claim>, StoreError> {
    let mut stmt =
        conn.prepare("SELECT dir, name, kind, task_id FROM name_claims ORDER BY created_at")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut claims = Vec::new();
    for row in rows {
        let (dir, name, kind, task_id) = row?;
        let kind = ClaimKind::from_sql(&kind)
            .ok_or_else(|| StoreError::BadValue(format!("réservation : sorte {kind}")))?;
        claims.push(Claim {
            dir,
            name,
            kind,
            task_id,
        });
    }
    Ok(claims)
}
