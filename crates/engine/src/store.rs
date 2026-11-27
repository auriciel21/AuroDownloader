//! État durable du moteur : base SQLite unique, dont le moteur est le seul écrivain (SPEC §5).
//!
//! Ce module ouvre la base avec les réglages de la SPEC §5.1 (mode WAL, clés étrangères,
//! `synchronous = NORMAL`) et applique les migrations numérotées (SPEC §5.6, règle 5) :
//! dans l'ordre, chacune dans sa propre transaction, après une copie de la base si elle
//! existe déjà dans une version plus ancienne. Une base plus récente que le moteur
//! empêche le démarrage au lieu d'être modifiée.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::segmenter::CoverageError;

pub mod claims;
pub mod headers;
pub mod history;
pub mod host_keys;
pub mod lock;
pub mod logins;
pub mod media;
pub mod moves;
pub mod pieces;
pub mod plugins;
pub mod remote;
pub mod rules;
pub mod schedules;
pub mod segments;
pub mod settings;
pub mod site;
pub mod sources;
pub mod tasks;
pub mod torrents;

/// Reconstruction d'une table par une migration ([`Migration::rebuild`]).
pub type Rebuild = fn(&rusqlite::Transaction<'_>) -> Result<(), StoreError>;

/// Une étape de migration du schéma.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// Numéro de version : entiers consécutifs à partir de 1.
    pub version: i64,
    /// Nom court, pour les messages et les journaux.
    pub name: &'static str,
    /// Script SQL appliqué dans une transaction.
    pub sql: &'static str,
    /// Table refaite avant le script, dans la même transaction, clés étrangères coupées
    /// (procédure en 12 étapes de SQLite) : seulement quand une contrainte doit changer,
    /// ce que `ALTER TABLE` ne sait pas faire. `PRAGMA foreign_key_check` doit être vide
    /// avant la validation.
    pub rebuild: Option<Rebuild>,
}

/// Attente maximale d'une écriture bloquée par celle d'une autre connexion.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// Toutes les migrations connues de ce moteur, dans l'ordre.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../migrations/0001_initial.sql"),
        rebuild: None,
    },
    Migration {
        version: 2,
        name: "idempotency_key",
        sql: include_str!("../migrations/0002_idempotency_key.sql"),
        rebuild: None,
    },
    Migration {
        version: 3,
        name: "move_and_rename",
        sql: include_str!("../migrations/0003_move_and_rename.sql"),
        rebuild: None,
    },
    Migration {
        version: 4,
        name: "site_logins",
        sql: include_str!("../migrations/0004_site_logins.sql"),
        rebuild: None,
    },
    Migration {
        version: 5,
        name: "checksum_source",
        sql: include_str!("../migrations/0005_checksum_source.sql"),
        rebuild: None,
    },
    Migration {
        version: 6,
        name: "media_streams",
        sql: include_str!("../migrations/0006_media_streams.sql"),
        rebuild: None,
    },
    Migration {
        version: 7,
        name: "media_subtitles",
        sql: include_str!("../migrations/0007_media_subtitles.sql"),
        rebuild: None,
    },
    Migration {
        version: 8,
        name: "remote",
        sql: include_str!("../migrations/0008_remote.sql"),
        rebuild: None,
    },
    Migration {
        version: 9,
        name: "name_claims",
        sql: include_str!("../migrations/0009_name_claims.sql"),
        rebuild: None,
    },
    Migration {
        version: 10,
        name: "plugins",
        sql: include_str!("../migrations/0010_plugins.sql"),
        rebuild: None,
    },
    Migration {
        version: 11,
        name: "rules",
        sql: include_str!("../migrations/0011_rules.sql"),
        rebuild: None,
    },
    Migration {
        version: 12,
        name: "pieces",
        sql: include_str!("../migrations/0012_pieces.sql"),
        rebuild: None,
    },
    Migration {
        version: 13,
        name: "ftp_plain",
        sql: include_str!("../migrations/0013_ftp_plain.sql"),
        rebuild: None,
    },
    Migration {
        version: 14,
        name: "sftp",
        sql: include_str!("../migrations/0014_sftp.sql"),
        rebuild: None,
    },
    Migration {
        version: 15,
        name: "insecure_redirect",
        sql: include_str!("../migrations/0015_insecure_redirect.sql"),
        rebuild: None,
    },
    Migration {
        version: 16,
        name: "media_duration",
        sql: include_str!("../migrations/0016_media_duration.sql"),
        rebuild: None,
    },
    Migration {
        version: 17,
        name: "drop_segments_state_index",
        sql: include_str!("../migrations/0017_drop_segments_state_index.sql"),
        rebuild: None,
    },
    Migration {
        version: 18,
        name: "site",
        sql: include_str!("../migrations/0018_site.sql"),
        rebuild: Some(rebuild_tasks_for_sites),
    },
    Migration {
        version: 19,
        name: "site_ignored",
        sql: include_str!("../migrations/0019_site_ignored.sql"),
        rebuild: None,
    },
    Migration {
        version: 20,
        name: "projects",
        sql: include_str!("../migrations/0020_projects.sql"),
        rebuild: None,
    },
];

/// Contrainte de `tasks.kind` avant la migration 18, et après (SPEC §7.18 ter).
const KIND_CHECK_BEFORE: &str = "CHECK (kind IN ('http','ftp','torrent','media'))";
const KIND_CHECK_AFTER: &str = "CHECK (kind IN ('http','ftp','torrent','media','site'))";

/// Migration 18 : `tasks.kind` reçoit `site`. SQLite ne change pas une contrainte : la
/// table est refaite d'après son propre texte (colonnes ajoutées comprises, dans le même
/// ordre), copiée, l'ancienne effacée, la nouvelle renommée, puis ses index et ses
/// déclencheurs refaits. Les clés étrangères sont coupées par l'appelant : effacer
/// `tasks` n'efface rien des tables qui la référencent.
fn rebuild_tasks_for_sites(tx: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    let sql: String = tx.query_row(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'tasks'",
        [],
        |r| r.get(0),
    )?;
    let head = "CREATE TABLE tasks (";
    if !sql.starts_with(head) || sql.matches(KIND_CHECK_BEFORE).count() != 1 {
        return Err(StoreError::BadValue(
            "table tasks inattendue : migration 18 impossible".into(),
        ));
    }
    let new_sql = sql.replacen(head, "CREATE TABLE tasks_new (", 1).replacen(
        KIND_CHECK_BEFORE,
        KIND_CHECK_AFTER,
        1,
    );
    let mut stmt = tx.prepare(
        "SELECT sql FROM sqlite_schema
         WHERE tbl_name = 'tasks' AND type IN ('index', 'trigger') AND sql IS NOT NULL
         ORDER BY type, name",
    )?;
    let others: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    drop(stmt);
    tx.execute_batch(&new_sql)?;
    tx.execute_batch(
        "INSERT INTO tasks_new SELECT * FROM tasks;
         DROP TABLE tasks;
         ALTER TABLE tasks_new RENAME TO tasks;",
    )?;
    for other in others {
        tx.execute_batch(&other)?;
    }
    Ok(())
}

/// Erreurs du module de stockage.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("erreur SQLite : {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("erreur d'entrée-sortie : {0}")]
    Io(#[from] std::io::Error),

    #[error(
        "la base est en version {found}, plus récente que la version {supported} \
         que ce moteur sait lire ; mettez le moteur à jour"
    )]
    DatabaseTooNew { found: i64, supported: i64 },

    #[error("liste de migrations invalide : la migration {found} suit la version {previous}")]
    BadMigrationOrder { previous: i64, found: i64 },

    #[error("tâche {task_id} introuvable")]
    TaskNotFound { task_id: String },

    #[error("segment {idx} de la tâche {task_id} introuvable")]
    SegmentNotFound { task_id: String, idx: u32 },

    #[error("la tâche {task_id} a déjà des segments")]
    SegmentsAlreadyExist { task_id: String },

    #[error("la tâche {task_id} a {count} segments mais aucune taille connue")]
    SegmentsWithoutSize { task_id: String, count: usize },

    #[error("segments de la tâche {task_id} incohérents : {error}")]
    Coverage {
        task_id: String,
        #[source]
        error: CoverageError,
    },

    #[error("valeur incohérente : {0}")]
    BadValue(String),

    #[error("la tâche {task_id} ne peut pas passer de l'état {from} à l'état {to}")]
    BadTransition {
        task_id: String,
        from: &'static str,
        to: &'static str,
    },

    #[error("la tâche {task_id} n'a aucune source")]
    NoSource { task_id: String },

    #[error("la base est déjà ouverte par un autre moteur (verrou {})", path.display())]
    Locked { path: PathBuf },
}

/// Identifiant d'une ligne : UUID v7 en texte, triable par date de création (SPEC §5.1).
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Ouvre (ou crée) la base du moteur dans `path` et la met à jour.
pub fn open(path: &Path) -> Result<Connection, StoreError> {
    open_with(path, MIGRATIONS)
}

/// Ouvre une base en mémoire, migrée : pratique pour les tests.
pub fn open_in_memory() -> Result<Connection, StoreError> {
    let mut conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&mut conn, MIGRATIONS)?;
    Ok(conn)
}

/// Comme [`open`], avec une liste de migrations donnée.
pub fn open_with(path: &Path, migrations: &[Migration]) -> Result<Connection, StoreError> {
    validate_order(migrations)?;
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    ensure_migrations_table(&conn)?;

    let current = current_version(&conn)?;
    let latest = latest_version(migrations);
    if current > latest {
        return Err(StoreError::DatabaseTooNew {
            found: current,
            supported: latest,
        });
    }
    // Une base déjà utilisée (version >= 1) qui va changer de schéma est copiée d'abord.
    if (1..latest).contains(&current) {
        backup_before_upgrade(&conn, path, current)?;
    }

    migrate(&mut conn, migrations)?;
    Ok(conn)
}

/// Réglages de connexion de la SPEC §5.1.
fn configure(conn: &Connection) -> Result<(), StoreError> {
    // WAL est conservé dans le fichier ; pour une base en mémoire SQLite répond « memory », sans erreur.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Chaque tâche en cours a sa connexion : une écriture attend que l'autre finisse,
    // au lieu d'échouer aussitôt (SQLITE_BUSY).
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(())
}

fn ensure_migrations_table(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        );",
    )?;
    Ok(())
}

/// Version actuelle du schéma : 0 pour une base vide.
pub fn current_version(conn: &Connection) -> Result<i64, StoreError> {
    ensure_migrations_table(conn)?;
    let version = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    Ok(version)
}

fn latest_version(migrations: &[Migration]) -> i64 {
    migrations.last().map_or(0, |m| m.version)
}

/// Les versions doivent être 1, 2, 3… sans trou ni doublon.
fn validate_order(migrations: &[Migration]) -> Result<(), StoreError> {
    let mut previous = 0;
    for m in migrations {
        if m.version != previous + 1 {
            return Err(StoreError::BadMigrationOrder {
                previous,
                found: m.version,
            });
        }
        previous = m.version;
    }
    Ok(())
}

/// Applique les migrations en attente, chacune dans sa transaction. Renvoie la version finale.
pub fn migrate(conn: &mut Connection, migrations: &[Migration]) -> Result<i64, StoreError> {
    validate_order(migrations)?;
    ensure_migrations_table(conn)?;

    let current = current_version(conn)?;
    let latest = latest_version(migrations);
    if current > latest {
        return Err(StoreError::DatabaseTooNew {
            found: current,
            supported: latest,
        });
    }

    for m in migrations.iter().filter(|m| m.version > current) {
        match m.rebuild {
            None => apply(conn, m)?,
            Some(rebuild) => {
                // Sans effet dans une transaction : coupées avant, rétablies après, même
                // en cas d'échec.
                let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
                conn.pragma_update(None, "foreign_keys", "OFF")?;
                let applied = apply_rebuilding(conn, m, rebuild);
                conn.pragma_update(None, "foreign_keys", fk)?;
                applied?;
            }
        }
    }
    Ok(latest)
}

fn apply(conn: &mut Connection, m: &Migration) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    tx.execute_batch(m.sql)?;
    tx.execute(
        "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
        params![m.version, now_ms()],
    )?;
    tx.commit()?;
    Ok(())
}

/// Une migration qui refait une table (procédure en 12 étapes de SQLite), clés
/// étrangères déjà coupées : aucune ligne ne doit en sortir orpheline.
fn apply_rebuilding(
    conn: &mut Connection,
    m: &Migration,
    rebuild: Rebuild,
) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    rebuild(&tx)?;
    tx.execute_batch(m.sql)?;
    let broken: i64 = tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
        r.get(0)
    })?;
    if broken > 0 {
        return Err(StoreError::BadValue(format!(
            "migration {} : {broken} lignes sans leur ligne référencée",
            m.version
        )));
    }
    tx.execute(
        "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
        params![m.version, now_ms()],
    )?;
    tx.commit()?;
    Ok(())
}

/// Copie cohérente de la base (même en mode WAL) avant une mise à jour du schéma.
fn backup_before_upgrade(
    conn: &Connection,
    path: &Path,
    from_version: i64,
) -> Result<PathBuf, StoreError> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".bak-v{from_version}"));
    let backup = path.with_file_name(name);

    let backup_str = backup.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "chemin de sauvegarde non UTF-8",
        )
    })?;
    // VACUUM INTO refuse d'écraser un fichier existant : on retire une ancienne copie.
    if backup.exists() {
        std::fs::remove_file(&backup)?;
    }
    conn.execute("VACUUM INTO ?1", [backup_str])?;
    Ok(backup)
}

/// Millisecondes Unix en UTC (SPEC §5.1).
/// Un entier à enregistrer : SQLite ne connaît que des entiers signés de 64 bits (audit
/// S11 : une seule conversion pour toute la base).
pub(crate) fn to_i64(value: u64, what: &str) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::BadValue(format!("{what} {value} trop grand pour la base")))
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_TABLES: [&str; 26] = [
        "event_log",
        "file_moves",
        "history",
        "media_jobs",
        "media_streams",
        "name_claims",
        "plugins",
        "projects",
        "queues",
        "remote_sessions",
        "remote_tokens",
        "rules",
        "schedules",
        "schema_migrations",
        "segments",
        "settings",
        "site_items",
        "site_jobs",
        "site_logins",
        "sources",
        "ssh_host_keys",
        "task_files",
        "task_headers",
        "task_pieces",
        "tasks",
        "torrents",
    ];

    fn table_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn insert_queue(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO queues (id, name, position, created_at) VALUES (?1, ?1, 0, 0)",
            [id],
        )
        .unwrap();
    }

    fn insert_task(
        conn: &Connection,
        id: &str,
        queue: &str,
        state: &str,
    ) -> rusqlite::Result<usize> {
        conn.execute(
            "INSERT INTO tasks (id, queue_id, kind, state, position, display_name, dest_dir,
                                created_at, updated_at)
             VALUES (?1, ?2, 'http', ?3, 0, 'fichier.iso', '/tmp', 0, 0)",
            params![id, queue, state],
        )
    }

    /// Version de la dernière migration connue.
    fn latest() -> i64 {
        MIGRATIONS.last().unwrap().version
    }

    #[test]
    fn migrations_create_the_26_tables_of_the_spec() {
        let conn = open_in_memory().unwrap();
        assert_eq!(table_names(&conn), EXPECTED_TABLES);
        assert_eq!(current_version(&conn).unwrap(), latest());
        // Numérotées sans trou à partir de 1 (SPEC §5.6, règle 5).
        for (i, m) in MIGRATIONS.iter().enumerate() {
            assert_eq!(m.version, i as i64 + 1, "{}", m.name);
        }
    }

    #[test]
    fn migrate_twice_changes_nothing() {
        let mut conn = open_in_memory().unwrap();
        assert_eq!(migrate(&mut conn, MIGRATIONS).unwrap(), latest());
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, latest());
    }

    /// Une base de la version 1 qui a déjà des tâches passe à la clé d'idempotence (0002) :
    /// ses tâches restent, sans clé.
    #[test]
    fn a_version_1_database_gains_the_idempotency_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auro.db");
        {
            let mut conn = Connection::open(&path).unwrap();
            migrate(&mut conn, &MIGRATIONS[..1]).unwrap();
            let queue = tasks::ensure_queue(&conn, "default").unwrap();
            insert_task(&conn, "t1", &queue, "queued").unwrap();
        }
        let conn = open(&path).unwrap();
        assert_eq!(current_version(&conn).unwrap(), latest());
        let key: Option<String> = conn
            .query_row(
                "SELECT idempotency_key FROM tasks WHERE id = 't1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(key, None);
        assert!(dir.path().join("auro.db.bak-v1").exists());
    }

    /// Lignes de chaque table qui référence `tasks`, pour la migration 18.
    fn fill_every_table_that_references_tasks(conn: &Connection, queue: &str) {
        insert_task(conn, "t1", queue, "active").unwrap();
        insert_task(conn, "t2", queue, "completed").unwrap();
        conn.execute_batch(
            "UPDATE tasks SET kind = 'media', idempotency_key = 'clé', requested_name = 'n',
                    public_only = 1, insecure_redirect = 1 WHERE id = 't2';
             INSERT INTO tasks (id, queue_id, kind, state, position, display_name, dest_dir,
                                created_at, updated_at, parent_id)
             VALUES ('t3', (SELECT queue_id FROM tasks WHERE id = 't1'), 'http', 'paused', 1,
                     'flux', '/tmp', 0, 0, 't2');
             INSERT INTO sources (id, task_id, url) VALUES ('s1', 't1', 'https://exemple.org/a');
             INSERT INTO segments (task_id, idx, start_offset, end_offset, state, source_id)
             VALUES ('t1', 0, 0, 99, 'pending', 's1');
             INSERT INTO task_headers (task_id, name, value) VALUES ('t1', 'Referer', 'x');
             INSERT INTO task_files (task_id, file_index, path, length) VALUES ('t1', 0, 'a', 1);
             INSERT INTO torrents (task_id, info_hash) VALUES ('t1', 'abc');
             INSERT INTO media_jobs (task_id, page_url, format_selector)
             VALUES ('t2', 'https://exemple.org/v', 'b');
             INSERT INTO media_streams (task_id, position, format_id, protocol, ext, has_video,
                                        has_audio, file_name, child_id)
             VALUES ('t2', 0, 'f', 'https', 'mp4', 1, 1, 'v.mp4', 't3');
             INSERT INTO file_moves (task_id, from_path, to_path, dest_dir, file_name, phase,
                                     started_at)
             VALUES ('t1', '/a', '/b', '/', 'b', 'moving', 0);
             INSERT INTO task_pieces (task_id, algo, length, hashes) VALUES ('t1', 'md5', 1, 'h');",
        )
        .unwrap();
    }

    fn rows_of_every_table(conn: &Connection) -> Vec<(String, i64)> {
        table_names(conn)
            .into_iter()
            .filter(|t| t != "schema_migrations")
            .map(|t| {
                let n = conn
                    .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                    .unwrap();
                (t, n)
            })
            .collect()
    }

    fn task_indexes(conn: &Connection) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare(
                "SELECT name, sql FROM sqlite_schema
                 WHERE type = 'index' AND tbl_name = 'tasks' AND sql IS NOT NULL ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// Migration 18 (SPEC §7.18 ter) : `tasks` refaite pour la sorte `site` sur une base
    /// remplie. Toutes ses lignes et celles qui la référencent restent, avec leurs valeurs ;
    /// ses index aussi ; les clés étrangères sont rétablies et toujours appliquées.
    #[test]
    fn migration_18_rebuilds_tasks_and_keeps_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auro.db");
        let (before_rows, before_tasks, before_indexes) = {
            let mut conn = Connection::open(&path).unwrap();
            configure(&conn).unwrap();
            migrate(&mut conn, &MIGRATIONS[..17]).unwrap();
            let queue = tasks::ensure_queue(&conn, "default").unwrap();
            fill_every_table_that_references_tasks(&conn, &queue);
            let tasks: Vec<(String, String, Option<String>, i64)> = conn
                .prepare("SELECT id, kind, parent_id, public_only FROM tasks ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            (rows_of_every_table(&conn), tasks, task_indexes(&conn))
        };
        assert_eq!(before_tasks.len(), 3);

        let conn = open(&path).unwrap();
        assert_eq!(current_version(&conn).unwrap(), latest());
        assert!(dir.path().join("auro.db.bak-v17").exists());
        let after_rows: Vec<_> = rows_of_every_table(&conn)
            .into_iter()
            .filter(|(t, _)| !matches!(t.as_str(), "site_jobs" | "site_items" | "projects"))
            .collect();
        assert_eq!(after_rows, before_rows);
        let after_tasks: Vec<(String, String, Option<String>, i64)> = conn
            .prepare("SELECT id, kind, parent_id, public_only FROM tasks ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(after_tasks, before_tasks);
        assert_eq!(task_indexes(&conn), before_indexes);
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
        let broken: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(broken, 0);

        // La nouvelle sorte est permise, une sorte inconnue toujours refusée.
        let queue: String = conn
            .query_row("SELECT queue_id FROM tasks WHERE id = 't1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        insert_task(&conn, "t4", &queue, "queued").unwrap();
        conn.execute("UPDATE tasks SET kind = 'site' WHERE id = 't4'", [])
            .unwrap();
        assert!(
            conn.execute("UPDATE tasks SET kind = 'autre' WHERE id = 't4'", [])
                .is_err()
        );
        // Les cascades suivent toujours `tasks` : retirer la vidéo retire son flux interne
        // et ses lignes.
        conn.execute("DELETE FROM tasks WHERE id = 't2'", [])
            .unwrap();
        let left: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM tasks WHERE id IN ('t2', 't3'))
                      + (SELECT COUNT(*) FROM media_jobs) + (SELECT COUNT(*) FROM media_streams)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
        conn.execute("DELETE FROM tasks WHERE id = 't1'", [])
            .unwrap();
        let left: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sources) + (SELECT COUNT(*) FROM segments)
                      + (SELECT COUNT(*) FROM task_headers) + (SELECT COUNT(*) FROM task_files)
                      + (SELECT COUNT(*) FROM torrents) + (SELECT COUNT(*) FROM file_moves)
                      + (SELECT COUNT(*) FROM task_pieces)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
        // Une clé étrangère vers une tâche absente reste refusée.
        assert!(
            conn.execute(
                "INSERT INTO sources (id, task_id, url) VALUES ('s9', 'absente', 'u')",
                []
            )
            .is_err()
        );
    }

    /// Migration 19 (SPEC §7.18 ter) : les liens d'une copie laissés en ligne par choix
    /// passent à `ignored`, les échecs techniques restent ; toutes les lignes restent.
    #[test]
    fn migration_19_sets_apart_the_links_left_online() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auro.db");
        {
            let mut conn = Connection::open(&path).unwrap();
            configure(&conn).unwrap();
            migrate(&mut conn, &MIGRATIONS[..18]).unwrap();
            let queue = tasks::ensure_queue(&conn, "default").unwrap();
            insert_task(&conn, "s1", &queue, "completed").unwrap();
            conn.execute_batch(
                "UPDATE tasks SET kind = 'site' WHERE id = 's1';
                 INSERT INTO site_jobs (task_id, page_url, depth, max_pages, max_bytes)
                 VALUES ('s1', 'https://exemple.org/', 1, 200, 1000);
                 INSERT INTO site_items (task_id, seq, url, kind, state, path, size, error)
                 VALUES ('s1', 0, 'https://exemple.org/', 'page', 'done', 'e/index.html', 10,
                         NULL),
                        ('s1', 1, 'https://exemple.org/LISEZMOI', 'page', 'failed', NULL, NULL,
                         'NOT_HTML'),
                        ('s1', 2, 'https://exemple.org/r', 'page', 'failed', NULL, NULL,
                         'REDIRECTED'),
                        ('s1', 3, 'https://exemple.org/gros.png', 'resource', 'failed', NULL,
                         NULL, 'TOO_BIG'),
                        ('s1', 4, 'https://exemple.org/manque.png', 'resource', 'failed', NULL,
                         NULL, 'HTTP_STATUS'),
                        ('s1', 5, 'https://exemple.org/p.html', 'page', 'skipped', NULL, NULL,
                         NULL);",
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        assert_eq!(current_version(&conn).unwrap(), latest());
        let states: Vec<(i64, String, Option<String>)> = conn
            .prepare("SELECT seq, state, error FROM site_items ORDER BY seq")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let states: Vec<(i64, &str, Option<&str>)> = states
            .iter()
            .map(|(s, st, e)| (*s, st.as_str(), e.as_deref()))
            .collect();
        assert_eq!(
            states,
            [
                (0, "done", None),
                (1, "ignored", Some("NOT_HTML")),
                (2, "ignored", Some("REDIRECTED")),
                (3, "ignored", Some("TOO_BIG")),
                (4, "failed", Some("HTTP_STATUS")),
                (5, "skipped", None),
            ]
        );
        let counts = site::counts(&conn, "s1").unwrap();
        assert_eq!((counts.pages, counts.failed, counts.ignored), (1, 1, 4));
        // Index refait, contrainte élargie, sans plus.
        let index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'site_items_state'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index, 1);
        assert!(
            conn.execute("UPDATE site_items SET state = 'autre' WHERE seq = 0", [])
                .is_err()
        );
        // Les cascades suivent toujours `tasks`.
        conn.execute("DELETE FROM tasks WHERE id = 's1'", [])
            .unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM site_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }

    /// Une migration qui refait une table et laisserait une ligne orpheline est annulée,
    /// et les clés étrangères sont rétablies quand même.
    #[test]
    fn a_rebuild_that_breaks_a_reference_is_rolled_back() {
        fn breaks(tx: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
            tx.execute_batch("DELETE FROM parent;")?;
            Ok(())
        }
        const V1: Migration = Migration {
            version: 1,
            name: "un",
            sql: "CREATE TABLE parent (id TEXT PRIMARY KEY);
                  CREATE TABLE child (p TEXT REFERENCES parent(id));
                  INSERT INTO parent VALUES ('a'); INSERT INTO child VALUES ('a');",
            rebuild: None,
        };
        const V2: Migration = Migration {
            version: 2,
            name: "deux",
            sql: "SELECT 1;",
            rebuild: Some(breaks),
        };
        let mut conn = Connection::open_in_memory().unwrap();
        configure(&conn).unwrap();
        assert!(matches!(
            migrate(&mut conn, &[V1, V2]),
            Err(StoreError::BadValue(_))
        ));
        assert_eq!(current_version(&conn).unwrap(), 1);
        let parents: i64 = conn
            .query_row("SELECT COUNT(*) FROM parent", [], |r| r.get(0))
            .unwrap();
        assert_eq!(parents, 1);
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn database_newer_than_engine_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auro.db");
        {
            let conn = open(&path).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (99, 0)",
                [],
            )
            .unwrap();
        }
        match open(&path) {
            Err(StoreError::DatabaseTooNew { found, supported }) => {
                assert_eq!((found, supported), (99, latest()));
            }
            other => panic!("attendu DatabaseTooNew, obtenu {other:?}"),
        }
    }

    #[test]
    fn failing_migration_is_rolled_back_and_earlier_ones_are_kept() {
        const OK: Migration = Migration {
            version: 1,
            name: "ok",
            sql: "CREATE TABLE a (x INTEGER);",
            rebuild: None,
        };
        const BAD: Migration = Migration {
            version: 2,
            name: "bad",
            sql: "CREATE TABLE b (x INTEGER); INSERT INTO table_inexistante VALUES (1);",
            rebuild: None,
        };
        let mut conn = Connection::open_in_memory().unwrap();
        assert!(migrate(&mut conn, &[OK, BAD]).is_err());
        assert_eq!(current_version(&conn).unwrap(), 1);
        let tables = table_names(&conn);
        assert!(tables.contains(&"a".to_string()));
        assert!(
            !tables.contains(&"b".to_string()),
            "la table b aurait dû être annulée"
        );
    }

    #[test]
    fn migrations_with_a_gap_are_rejected() {
        let skip = [Migration {
            version: 2,
            name: "trou",
            sql: "SELECT 1;",
            rebuild: None,
        }];
        let mut conn = Connection::open_in_memory().unwrap();
        assert!(matches!(
            migrate(&mut conn, &skip),
            Err(StoreError::BadMigrationOrder {
                previous: 0,
                found: 2
            })
        ));
    }

    #[test]
    fn file_database_uses_wal_and_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("auro.db")).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        assert_eq!(fk, 1);
    }

    #[test]
    fn upgrading_an_existing_database_makes_a_backup_first() {
        const V1: Migration = Migration {
            version: 1,
            name: "un",
            sql: "CREATE TABLE a (x INTEGER);",
            rebuild: None,
        };
        const V2: Migration = Migration {
            version: 2,
            name: "deux",
            sql: "CREATE TABLE b (x INTEGER);",
            rebuild: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auro.db");
        let backup = dir.path().join("auro.db.bak-v1");

        drop(open_with(&path, &[V1]).unwrap());
        assert!(!backup.exists(), "pas de copie à la création");

        let conn = open_with(&path, &[V1, V2]).unwrap();
        assert!(backup.exists(), "copie attendue avant la mise à jour");
        assert_eq!(current_version(&conn).unwrap(), 2);

        // la copie est une base valide, restée en version 1
        let old = Connection::open(&backup).unwrap();
        assert_eq!(current_version(&old).unwrap(), 1);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let conn = open_in_memory().unwrap();
        assert!(insert_task(&conn, "t1", "file-inconnue", "queued").is_err());
        insert_queue(&conn, "q1");
        assert!(insert_task(&conn, "t1", "q1", "queued").is_ok());
    }

    #[test]
    fn task_state_is_restricted_to_the_values_of_the_spec() {
        let conn = open_in_memory().unwrap();
        insert_queue(&conn, "q1");
        for state in [
            "queued",
            "active",
            "paused",
            "retry_wait",
            "verifying",
            "post_processing",
            "seeding",
            "completed",
            "failed",
            "cancelled",
        ] {
            insert_task(&conn, &format!("t-{state}"), "q1", state).unwrap();
        }
        assert!(insert_task(&conn, "t-x", "q1", "inconnu").is_err());
    }

    #[test]
    fn segment_constraints_reject_inconsistent_ranges() {
        let conn = open_in_memory().unwrap();
        insert_queue(&conn, "q1");
        insert_task(&conn, "t1", "q1", "active").unwrap();
        let insert = |idx: i64, start: i64, end: i64, written: i64| {
            conn.execute(
                "INSERT INTO segments (task_id, idx, start_offset, end_offset, written_bytes, state)
                 VALUES ('t1', ?1, ?2, ?3, ?4, 'pending')",
                params![idx, start, end, written],
            )
        };
        assert!(insert(0, 0, 1023, 0).is_ok());
        assert!(insert(1, 1024, 1000, 0).is_err(), "fin avant le début");
        assert!(
            insert(2, 2048, 2051, 5).is_err(),
            "plus d'octets écrits que la taille du segment"
        );
    }

    #[test]
    fn deleting_a_task_removes_its_segments_but_not_its_history() {
        let conn = open_in_memory().unwrap();
        insert_queue(&conn, "q1");
        insert_task(&conn, "t1", "q1", "completed").unwrap();
        conn.execute(
            "INSERT INTO segments (task_id, idx, start_offset, end_offset, state)
             VALUES ('t1', 0, 0, 9, 'done')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO history (id, task_id, display_name, final_state, finished_at)
             VALUES ('h1', 't1', 'fichier.iso', 'completed', 0)",
            [],
        )
        .unwrap();

        conn.execute("DELETE FROM tasks WHERE id = 't1'", [])
            .unwrap();

        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count("segments"), 0);
        assert_eq!(count("history"), 1);
    }
}
