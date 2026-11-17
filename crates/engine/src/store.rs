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
pub mod sources;
pub mod tasks;
pub mod torrents;

/// Une étape de migration du schéma.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// Numéro de version : entiers consécutifs à partir de 1.
    pub version: i64,
    /// Nom court, pour les messages et les journaux.
    pub name: &'static str,
    /// Script SQL appliqué dans une transaction.
    pub sql: &'static str,
}

/// Attente maximale d'une écriture bloquée par celle d'une autre connexion.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// Toutes les migrations connues de ce moteur, dans l'ordre.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "idempotency_key",
        sql: include_str!("../migrations/0002_idempotency_key.sql"),
    },
    Migration {
        version: 3,
        name: "move_and_rename",
        sql: include_str!("../migrations/0003_move_and_rename.sql"),
    },
    Migration {
        version: 4,
        name: "site_logins",
        sql: include_str!("../migrations/0004_site_logins.sql"),
    },
    Migration {
        version: 5,
        name: "checksum_source",
        sql: include_str!("../migrations/0005_checksum_source.sql"),
    },
    Migration {
        version: 6,
        name: "media_streams",
        sql: include_str!("../migrations/0006_media_streams.sql"),
    },
    Migration {
        version: 7,
        name: "media_subtitles",
        sql: include_str!("../migrations/0007_media_subtitles.sql"),
    },
    Migration {
        version: 8,
        name: "remote",
        sql: include_str!("../migrations/0008_remote.sql"),
    },
    Migration {
        version: 9,
        name: "name_claims",
        sql: include_str!("../migrations/0009_name_claims.sql"),
    },
    Migration {
        version: 10,
        name: "plugins",
        sql: include_str!("../migrations/0010_plugins.sql"),
    },
    Migration {
        version: 11,
        name: "rules",
        sql: include_str!("../migrations/0011_rules.sql"),
    },
    Migration {
        version: 12,
        name: "pieces",
        sql: include_str!("../migrations/0012_pieces.sql"),
    },
    Migration {
        version: 13,
        name: "ftp_plain",
        sql: include_str!("../migrations/0013_ftp_plain.sql"),
    },
];

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
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![m.version, now_ms()],
        )?;
        tx.commit()?;
    }
    Ok(latest)
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
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_TABLES: [&str; 22] = [
        "event_log",
        "file_moves",
        "history",
        "media_jobs",
        "media_streams",
        "name_claims",
        "plugins",
        "queues",
        "remote_sessions",
        "remote_tokens",
        "rules",
        "schedules",
        "schema_migrations",
        "segments",
        "settings",
        "site_logins",
        "sources",
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
    fn migrations_create_the_22_tables_of_the_spec() {
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
        };
        const BAD: Migration = Migration {
            version: 2,
            name: "bad",
            sql: "CREATE TABLE b (x INTEGER); INSERT INTO table_inexistante VALUES (1);",
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
        };
        const V2: Migration = Migration {
            version: 2,
            name: "deux",
            sql: "CREATE TABLE b (x INTEGER);",
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
