//! Projets enregistrés et programmés (SPEC §7.18 quater, F-44, migration 20) : une
//! exploration gardée sous un nom (réglages, filtres, copie ou fichiers seuls, dossier,
//! file, programme), et sa tâche de copie, reprise à chaque lancement.

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::site::{Filters, Settings};
use super::{StoreError, new_id, now_ms, to_i64};

/// Programme d'un projet : à `minute` (du jour, heure locale du moteur), les jours de
/// `days` (bit 0 : lundi … bit 6 : dimanche ; 0 : une seule fois).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub minute: u16,
    pub days: u8,
}

/// Ce que l'on enregistre d'un projet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSpec {
    pub name: String,
    pub page_url: String,
    pub settings: Settings,
    pub filters: Filters,
    pub copy_site: bool,
    /// Fichiers seuls : sous `<hôte>/<chemin>` plutôt qu'à plat.
    pub keep_tree: bool,
    /// `None` : le dossier des téléchargements du moteur.
    pub dest_dir: Option<String>,
    /// `None` : la file par défaut.
    pub queue_id: Option<String>,
    pub schedule: Option<Schedule>,
}

/// Un projet enregistré.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub spec: ProjectSpec,
    /// Prochain lancement prévu (millisecondes Unix) ; `None` sans programme.
    pub next_run_at: Option<i64>,
    /// Sa tâche de copie ; `None` avant son premier lancement, ou retirée.
    pub task_id: Option<String>,
    pub last_run_at: Option<i64>,
    /// Dernière occurrence sautée : sa tâche tournait encore.
    pub last_skipped_at: Option<i64>,
    /// Créé par une connexion à jeton : réseau public seulement (SPEC §7.19).
    pub public_only: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

const COLUMNS: &str = "id, name, page_url, depth, max_pages, max_bytes, exclude, same_folder, \
                       ignore_robots, filter_ext, filter_regex, filter_min, filter_max, \
                       copy_site, keep_tree, dest_dir, queue_id, schedule_time, schedule_days, \
                       next_run_at, task_id, last_run_at, last_skipped_at, public_only, \
                       created_at, updated_at";

fn bad(what: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Integer,
        format!("projet : {what}").into(),
    )
}

fn read(r: &Row<'_>) -> rusqlite::Result<Project> {
    let size = |v: Option<i64>| v.map(|v| u64::try_from(v).unwrap_or(0));
    let time: Option<String> = r.get(17)?;
    let schedule = match time {
        Some(text) => Some(Schedule {
            minute: crate::schedule::parse_hhmm(&text).ok_or_else(|| bad("heure"))?,
            days: u8::try_from(r.get::<_, i64>(18)?).map_err(|_| bad("jours"))?,
        }),
        None => None,
    };
    Ok(Project {
        id: r.get(0)?,
        spec: ProjectSpec {
            name: r.get(1)?,
            page_url: r.get(2)?,
            settings: Settings {
                depth: u8::try_from(r.get::<_, i64>(3)?).map_err(|_| bad("profondeur"))?,
                max_pages: u32::try_from(r.get::<_, i64>(4)?).map_err(|_| bad("pages"))?,
                max_bytes: u64::try_from(r.get::<_, i64>(5)?).map_err(|_| bad("volume"))?,
                exclude: r.get(6)?,
                same_folder: r.get(7)?,
                ignore_robots: r.get(8)?,
            },
            filters: Filters {
                extensions: Filters::extensions_of(&r.get::<_, String>(9)?),
                regex: r.get(10)?,
                min_size: size(r.get(11)?),
                max_size: size(r.get(12)?),
            },
            copy_site: r.get(13)?,
            keep_tree: r.get(14)?,
            dest_dir: r.get(15)?,
            queue_id: r.get(16)?,
            schedule,
        },
        next_run_at: r.get(19)?,
        task_id: r.get(20)?,
        last_run_at: r.get(21)?,
        last_skipped_at: r.get(22)?,
        public_only: r.get(23)?,
        created_at: r.get(24)?,
        updated_at: r.get(25)?,
    })
}

/// Tous les projets, par nom.
pub fn list(conn: &Connection) -> Result<Vec<Project>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM projects ORDER BY name COLLATE NOCASE, id"
    ))?;
    let rows = stmt.query_map([], read)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Project>, StoreError> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM projects WHERE id = ?1"),
            [id],
            read,
        )
        .optional()?)
}

/// Un autre projet porte-t-il déjà ce nom (sans tenir compte de la casse) ?
pub fn name_taken(conn: &Connection, name: &str, except: Option<&str>) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM projects
                        WHERE name = ?1 COLLATE NOCASE AND id IS NOT ?2)",
        params![name, except],
        |r| r.get(0),
    )?)
}

fn spec_params(spec: &ProjectSpec) -> Result<[rusqlite::types::Value; 17], StoreError> {
    use rusqlite::types::Value;
    let int = |v: Option<u64>, what| -> Result<Value, StoreError> {
        Ok(match v {
            Some(v) => Value::Integer(to_i64(v, what)?),
            None => Value::Null,
        })
    };
    let text = |v: Option<&String>| v.map_or(Value::Null, |t| Value::Text(t.clone()));
    let s = &spec.settings;
    let f = &spec.filters;
    Ok([
        Value::Text(spec.name.clone()),
        Value::Text(spec.page_url.clone()),
        Value::Integer(i64::from(s.depth)),
        Value::Integer(i64::from(s.max_pages)),
        Value::Integer(to_i64(s.max_bytes, "volume")?),
        text(s.exclude.as_ref()),
        Value::Integer(i64::from(s.same_folder)),
        Value::Integer(i64::from(s.ignore_robots)),
        Value::Text(f.extensions_sql()),
        text(f.regex.as_ref()),
        int(f.min_size, "taille mini")?,
        int(f.max_size, "taille maxi")?,
        Value::Integer(i64::from(spec.copy_site)),
        Value::Integer(i64::from(spec.keep_tree)),
        text(spec.dest_dir.as_ref()),
        text(spec.queue_id.as_ref()),
        spec.schedule.map_or(Value::Null, |s| {
            Value::Text(crate::schedule::hhmm(s.minute))
        }),
    ])
}

/// Enregistre un nouveau projet ; son prochain lancement est calculé par l'appelant.
pub fn create(
    conn: &Connection,
    spec: &ProjectSpec,
    next_run_at: Option<i64>,
    public_only: bool,
) -> Result<String, StoreError> {
    let (id, now) = (new_id(), now_ms());
    let p = spec_params(spec)?;
    conn.execute(
        &format!(
            "INSERT INTO projects ({COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18, ?19, ?20, NULL, NULL, NULL, ?21, ?22, ?22)"
        ),
        params![
            id,
            p[0],
            p[1],
            p[2],
            p[3],
            p[4],
            p[5],
            p[6],
            p[7],
            p[8],
            p[9],
            p[10],
            p[11],
            p[12],
            p[13],
            p[14],
            p[15],
            p[16],
            spec.schedule.map_or(0, |s| i64::from(s.days)),
            next_run_at,
            public_only,
            now
        ],
    )?;
    Ok(id)
}

/// Remplace les réglages d'un projet ; sa tâche, ses lancements et son origine restent.
/// Rend faux si le projet n'existe pas.
pub fn replace(
    conn: &Connection,
    id: &str,
    spec: &ProjectSpec,
    next_run_at: Option<i64>,
) -> Result<bool, StoreError> {
    let p = spec_params(spec)?;
    let changed = conn.execute(
        "UPDATE projects SET name = ?2, page_url = ?3, depth = ?4, max_pages = ?5, max_bytes = ?6,
                             exclude = ?7, same_folder = ?8, ignore_robots = ?9,
                             filter_ext = ?10, filter_regex = ?11, filter_min = ?12,
                             filter_max = ?13, copy_site = ?14, keep_tree = ?15, dest_dir = ?16,
                             queue_id = ?17, schedule_time = ?18, schedule_days = ?19,
                             next_run_at = ?20, updated_at = ?21
         WHERE id = ?1",
        params![
            id,
            p[0],
            p[1],
            p[2],
            p[3],
            p[4],
            p[5],
            p[6],
            p[7],
            p[8],
            p[9],
            p[10],
            p[11],
            p[12],
            p[13],
            p[14],
            p[15],
            p[16],
            spec.schedule.map_or(0, |s| i64::from(s.days)),
            next_run_at,
            now_ms()
        ],
    )?;
    Ok(changed == 1)
}

/// Supprime un projet (sa tâche et ses fichiers restent) ; faux s'il n'existait pas.
pub fn delete(conn: &Connection, id: &str) -> Result<bool, StoreError> {
    Ok(conn.execute("DELETE FROM projects WHERE id = ?1", [id])? == 1)
}

/// Un lancement : sa tâche, son heure.
pub fn mark_run(conn: &Connection, id: &str, task_id: &str, at: i64) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE projects SET task_id = ?2, last_run_at = ?3 WHERE id = ?1",
        params![id, task_id, at],
    )?;
    Ok(())
}

/// Une occurrence sautée : la tâche du projet tournait encore.
pub fn mark_skipped(conn: &Connection, id: &str, at: i64) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE projects SET last_skipped_at = ?2 WHERE id = ?1",
        params![id, at],
    )?;
    Ok(())
}

/// Le prochain lancement prévu ; sans lui (programme d'une seule fois, passé), le
/// programme s'efface.
pub fn set_next_run(conn: &Connection, id: &str, next: Option<i64>) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE projects SET next_run_at = ?2,
                            schedule_time = CASE WHEN ?2 IS NULL THEN NULL ELSE schedule_time END,
                            schedule_days = CASE WHEN ?2 IS NULL THEN 0 ELSE schedule_days END
         WHERE id = ?1",
        params![id, next],
    )?;
    Ok(())
}

/// Projets dus à l'instant `now` (millisecondes Unix), du plus ancien prévu au plus récent.
pub fn due(conn: &Connection, now: i64) -> Result<Vec<Project>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM projects
         WHERE next_run_at IS NOT NULL AND next_run_at <= ?1 ORDER BY next_run_at, id"
    ))?;
    let rows = stmt
        .query_map([now], read)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Le plus proche lancement prévu, s'il y en a un.
pub fn next_due(conn: &Connection) -> Result<Option<i64>, StoreError> {
    Ok(conn.query_row(
        "SELECT MIN(next_run_at) FROM projects WHERE next_run_at IS NOT NULL",
        [],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests;
