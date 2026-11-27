//! Projets (SPEC §7.18 quater) par le planificateur, sur un petit site local.

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use rusqlite::Connection;
use tokio::time::timeout;

use crate::net::Client;
use crate::schedule::{LocalNow, Zone};
use crate::scheduler::{Config, DISK_RECHECK, Engine, EngineError, Planning};
use crate::store::projects::{ProjectSpec, Schedule};
use crate::store::site::{Filters, Settings};
use crate::store::tasks::TaskState;
use crate::testing::{Pace, Server, reply, serve_paced};
use crate::transfer::Options;

const WAIT: Duration = Duration::from_secs(60);
/// Lundi 2026-11-02, minuit, en heure locale (décalage nul).
const MONDAY: i64 = 20_759 * 86_400_000;

fn at(day: i64, h: i64, m: i64) -> i64 {
    MONDAY + day * 86_400_000 + h * 3_600_000 + m * 60_000
}

/// Une horloge par test : ils tournent en même temps.
static CLOCK_A: AtomicI64 = AtomicI64::new(0);
static CLOCK_B: AtomicI64 = AtomicI64::new(0);

fn fixed(ms: i64) -> LocalNow {
    LocalNow {
        utc_ms: ms,
        offset_ms: 0,
        zone: Zone::Fixed,
    }
}

fn clock_a(_: &Connection) -> LocalNow {
    fixed(CLOCK_A.load(Ordering::SeqCst))
}

fn clock_b(_: &Connection) -> LocalNow {
    fixed(CLOCK_B.load(Ordering::SeqCst))
}

fn engine(dir: &Path, clock: fn(&Connection) -> LocalNow) -> Engine {
    Engine::start(Config {
        db_path: dir.join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options {
            site_spacing: Duration::from_millis(5),
            ..Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning {
            clock,
            check_every: Duration::from_millis(20),
            ..Planning::default()
        },
    })
    .unwrap()
}

/// `/` lie un PDF et une archive ; servi lentement avec `slow` (une copie qui dure).
fn site(slow: bool) -> Server {
    let pace = slow.then_some(Pace {
        chunk: 512,
        delay: Duration::from_millis(100),
    });
    serve_paced(
        |r| {
            let (kind, body): (&str, Vec<u8>) = match r.path.as_str() {
                "/" => (
                    "text/html",
                    br#"<a href="/doc.pdf">pdf</a> <a href="/archive.zip">zip</a>"#.to_vec(),
                ),
                "/doc.pdf" => ("application/pdf", vec![b'%'; 4096]),
                "/archive.zip" => ("application/zip", b"PK".to_vec()),
                _ => return reply("404 Not Found", &[("Content-Length", "0")], b""),
            };
            reply(
                "200 OK",
                &[
                    ("Content-Type", kind),
                    ("Content-Length", &body.len().to_string()),
                ],
                &body,
            )
        },
        pace,
    )
}

fn spec(name: &str, server: &Server, dest: &Path) -> ProjectSpec {
    ProjectSpec {
        name: name.into(),
        page_url: server.url("/"),
        settings: Settings {
            depth: 1,
            max_pages: 200,
            max_bytes: 1 << 30,
            exclude: None,
            same_folder: false,
            ignore_robots: false,
        },
        filters: Filters {
            extensions: vec!["pdf".into()],
            ..Filters::default()
        },
        copy_site: false,
        keep_tree: false,
        dest_dir: Some(dest.to_string_lossy().into_owned()),
        queue_id: None,
        schedule: None,
    }
}

async fn finished(engine: &Engine, id: &str) -> crate::store::tasks::TaskRow {
    timeout(WAIT, async {
        loop {
            let task = engine.get(id).await.unwrap();
            if matches!(task.state, TaskState::Completed | TaskState::Failed) {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("tâche finie")
}

/// Le projet tel que le moteur le garde.
async fn project(engine: &Engine, id: &str) -> crate::store::projects::Project {
    engine
        .projects()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == id)
        .expect("projet")
}

#[tokio::test]
async fn a_project_is_saved_run_and_updated_in_place() {
    CLOCK_A.store(at(0, 12, 0), Ordering::SeqCst);
    let server = site(false);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), clock_a);
    let id = engine
        .set_project(None, spec("Docs", &server, dir.path()), false)
        .await
        .unwrap();
    // Un nom déjà pris, sans tenir compte de la casse ; un nom vide.
    let taken = engine
        .set_project(None, spec(" docs ", &server, dir.path()), false)
        .await;
    assert!(
        matches!(taken, Err(EngineError::ProjectExists(_))),
        "{taken:?}"
    );
    let empty = engine
        .set_project(None, spec("  ", &server, dir.path()), false)
        .await;
    assert!(
        matches!(empty, Err(EngineError::BadRequest(_))),
        "{empty:?}"
    );
    // Une copie invalide est refusée à l'enregistrement.
    let mut bad = spec("Autre", &server, dir.path());
    bad.settings.depth = 9;
    assert!(engine.set_project(None, bad, false).await.is_err());

    let task_id = engine.run_project(&id).await.unwrap();
    // Sa copie tourne : un second lancement est refusé.
    assert!(matches!(
        engine.run_project(&id).await,
        Err(EngineError::InvalidState { .. })
    ));
    let task = finished(&engine, &task_id).await;
    assert_eq!(task.state, TaskState::Completed, "{:?}", task.error_detail);
    let root = dir.path().join(task.file_name.as_deref().unwrap());
    assert_eq!(
        std::fs::read(root.join("doc.pdf")).unwrap(),
        vec![b'%'; 4096]
    );
    assert!(!root.join("archive.zip").exists());
    let saved = project(&engine, &id).await;
    assert_eq!(saved.task_id.as_deref(), Some(task_id.as_str()));
    assert_eq!(saved.last_run_at, Some(at(0, 12, 0)));

    // Relancé : la même tâche, mise à jour sur place.
    CLOCK_A.store(at(0, 13, 0), Ordering::SeqCst);
    assert_eq!(engine.run_project(&id).await.unwrap(), task_id);
    let again = finished(&engine, &task_id).await;
    assert_eq!(again.state, TaskState::Completed);
    assert_eq!(again.file_name, task.file_name, "même dossier");
    assert_eq!(project(&engine, &id).await.last_run_at, Some(at(0, 13, 0)));

    // Modifié (une autre page) : une nouvelle copie au lancement suivant.
    let mut moved = spec("Docs", &server, dir.path());
    moved.page_url = server.url("/?autre");
    engine
        .set_project(Some(id.clone()), moved, false)
        .await
        .unwrap();
    let other = engine.run_project(&id).await.unwrap();
    assert_ne!(other, task_id);
    finished(&engine, &other).await;

    engine.delete_project(&id).await.unwrap();
    assert!(engine.projects().await.unwrap().is_empty());
    assert!(matches!(
        engine.delete_project(&id).await,
        Err(EngineError::ProjectNotFound(_))
    ));
    assert!(matches!(
        engine.run_project(&id).await,
        Err(EngineError::ProjectNotFound(_))
    ));
    // Sa tâche reste.
    assert_eq!(
        engine.get(&task_id).await.unwrap().state,
        TaskState::Completed
    );
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn a_missed_run_happens_once_and_a_busy_project_skips_its_turn() {
    CLOCK_B.store(at(0, 9, 0), Ordering::SeqCst);
    let server = site(true);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path(), clock_b);
    // Une seule fois, à 09:30.
    let once = engine
        .set_project(
            None,
            ProjectSpec {
                schedule: Some(Schedule {
                    minute: 9 * 60 + 30,
                    days: 0,
                }),
                ..spec("Une fois", &server, dir.path())
            },
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        project(&engine, &once).await.next_run_at,
        Some(at(0, 9, 30))
    );

    // Le moteur n'a pas vu l'heure passer : le lendemain, le lancement manqué se fait une
    // fois, et le programme d'une seule fois s'efface.
    CLOCK_B.store(at(1, 10, 0), Ordering::SeqCst);
    let saved = timeout(WAIT, async {
        loop {
            let p = project(&engine, &once).await;
            if p.task_id.is_some() {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("lancement manqué fait");
    assert_eq!(saved.last_run_at, Some(at(1, 10, 0)));
    assert_eq!((saved.next_run_at, saved.spec.schedule), (None, None));

    // Tous les jours à 10:30 : à l'heure dite, sa copie tourne encore (site lent), et
    // l'occurrence est sautée, puis la suivante prévue.
    let daily = engine
        .set_project(
            None,
            ProjectSpec {
                schedule: Some(Schedule {
                    minute: 10 * 60 + 30,
                    days: 0x7F,
                }),
                ..spec("Chaque jour", &server, dir.path())
            },
            false,
        )
        .await
        .unwrap();
    let task_id = engine.run_project(&daily).await.unwrap();
    CLOCK_B.store(at(1, 10, 31), Ordering::SeqCst);
    let skipped = timeout(WAIT, async {
        loop {
            let p = project(&engine, &daily).await;
            if p.last_skipped_at.is_some() {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("occurrence sautée");
    assert_eq!(skipped.last_skipped_at, Some(at(1, 10, 31)));
    assert_eq!(skipped.next_run_at, Some(at(2, 10, 30)));
    assert_eq!(
        skipped.last_run_at,
        Some(at(1, 10, 0)),
        "le lancement à la main"
    );
    assert_eq!(skipped.task_id.as_deref(), Some(task_id.as_str()));
    finished(&engine, &task_id).await;
    finished(&engine, saved.task_id.as_deref().unwrap()).await;
    crate::testing::stop(&engine).await;
}
