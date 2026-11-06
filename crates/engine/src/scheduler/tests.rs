//! Planificateur face au serveur de test : files, pause, reprise, annulation, nouvelles
//! tentatives, reprise au démarrage et arrêt du moteur.

use std::sync::{Arc, Mutex};

use rusqlite::params;
use tokio::time::timeout;

use super::*;
use crate::testing::{FileSite, Pace, Server, content, serve_file, serve_file_paced};

const SIZE: usize = 256 * 1024;
const WAIT: Duration = Duration::from_secs(30);

fn config(dir: &Path) -> Config {
    Config {
        db_path: dir.join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options {
            connections: Some(2),
            min_segment: 16 * 1024,
            buffer: 4096,
            checkpoint_every: Duration::from_millis(20),
            ..Options::default()
        },
    }
}

/// Serveur lent : 16 Kio toutes les 10 ms par connexion, le temps d'agir sur une tâche
/// en cours.
fn slow_site(len: usize) -> Server {
    let pace = Pace {
        chunk: 16 * 1024,
        delay: Duration::from_millis(10),
    };
    serve_file_paced(Arc::new(Mutex::new(FileSite::new(len as u64))), Some(pace))
}

fn fast_site(len: usize) -> (Arc<Mutex<FileSite>>, Server) {
    let site = Arc::new(Mutex::new(FileSite::new(len as u64)));
    let server = serve_file(Arc::clone(&site));
    (site, server)
}

fn add(url: &str, dir: &Path) -> AddTask {
    AddTask {
        url: url.to_owned(),
        dest_dir: dir.to_str().unwrap().to_owned(),
        ..AddTask::default()
    }
}

/// Attend que `task_id` passe à l'état `to` ; renvoie les événements vus jusque-là.
async fn wait_state(events: &mut broadcast::Receiver<Event>, task_id: &str, to: TaskState) {
    let found = timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(Event::StateChanged {
                    task_id: t, to: s, ..
                }) if t == task_id && s == to => {
                    return;
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("moteur arrêté"),
            }
        }
    })
    .await;
    assert!(found.is_ok(), "{task_id} n'est jamais passée à {to:?}");
}

fn task(dir: &Path, task_id: &str) -> TaskRow {
    load_task(&store::open(&dir.join("auro.db")).unwrap(), task_id).unwrap()
}

fn assert_file(dir: &Path, name: &str, len: usize) {
    let got = std::fs::read(dir.join(name)).unwrap();
    assert!(got == content(len), "{name} : contenu faux");
}

#[tokio::test]
async fn added_tasks_run_and_complete() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let mut ids = Vec::new();
    for name in ["a.bin", "b.bin", "c.bin"] {
        ids.push(
            engine
                .add(add(&server.url(&format!("/{name}")), dir.path()))
                .await
                .unwrap(),
        );
    }
    // Les fins arrivent dans n'importe quel ordre.
    let mut left: HashSet<String> = ids.iter().cloned().collect();
    timeout(WAIT, async {
        while !left.is_empty() {
            if let Ok(Event::StateChanged { task_id, to, .. }) = events.recv().await {
                if to == TaskState::Completed {
                    left.remove(&task_id);
                }
            }
        }
    })
    .await
    .expect("les trois tâches finissent");
    for name in ["a.bin", "b.bin", "c.bin"] {
        assert_file(dir.path(), name, SIZE);
    }
    engine.shutdown().await;
}

#[tokio::test]
async fn a_queue_never_runs_more_tasks_than_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let server = slow_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    store::open(&dir.path().join("auro.db"))
        .unwrap()
        .execute("UPDATE queues SET max_active = 1", [])
        .unwrap();
    let mut events = engine.subscribe();
    let mut ids = Vec::new();
    for name in ["a.bin", "b.bin", "c.bin"] {
        ids.push(
            engine
                .add(add(&server.url(&format!("/{name}")), dir.path()))
                .await
                .unwrap(),
        );
    }
    let (mut active, mut peak, mut done) = (0i32, 0i32, 0);
    timeout(WAIT, async {
        while done < ids.len() {
            if let Ok(Event::StateChanged { from, to, .. }) = events.recv().await {
                if to == TaskState::Active {
                    active += 1;
                } else if from == TaskState::Active {
                    active -= 1;
                }
                peak = peak.max(active);
                done += usize::from(to == TaskState::Completed);
            }
        }
    })
    .await
    .expect("les trois tâches finissent");
    assert_eq!(peak, 1);
    engine.shutdown().await;
}

#[tokio::test]
async fn higher_priority_starts_first() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    // Tâches créées moteur arrêté, pour fixer les priorités avant le premier départ.
    let engine = Engine::start(config(dir.path())).unwrap();
    let queue = engine.default_queue().to_owned();
    engine.shutdown().await;
    let mut conn = store::open(&dir.path().join("auro.db")).unwrap();
    conn.execute("UPDATE queues SET max_active = 1", [])
        .unwrap();
    let mut ids = Vec::new();
    for name in ["low.bin", "high.bin"] {
        let url = server.url(&format!("/{name}"));
        let new = NewTask {
            queue_id: &queue,
            url: &url,
            display_name: name,
            dest_dir: dir.path().to_str().unwrap(),
            max_connections: None,
        };
        ids.push(tasks::create_task(&mut conn, &new).unwrap().0);
    }
    conn.execute(
        "UPDATE tasks SET priority = 5 WHERE id = ?1",
        params![ids[1]],
    )
    .unwrap();

    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    if task(dir.path(), &ids[0]).state != TaskState::Completed {
        wait_state(&mut events, &ids[0], TaskState::Completed).await;
    }
    engine.shutdown().await;
    // La file n'accepte qu'une tâche à la fois : la plus prioritaire, créée en second,
    // est partie la première et a fini avant le départ de l'autre.
    let times = |id: &str| -> (i64, i64) {
        conn.query_row(
            "SELECT started_at, finished_at FROM tasks WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    };
    let (low_start, _) = times(&ids[0]);
    let (high_start, high_end) = times(&ids[1]);
    assert!(
        high_start <= low_start,
        "la tâche prioritaire part la première"
    );
    assert!(
        high_end <= low_start,
        "une seule tâche à la fois dans la file"
    );
}

#[tokio::test]
async fn pause_stops_a_running_task_and_resume_finishes_it() {
    let dir = tempfile::tempdir().unwrap();
    let server = slow_site(4 * SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/p.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    engine.pause(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Paused).await;
    let paused = task(dir.path(), &id);
    assert_eq!(paused.pause_reason.as_deref(), Some("user"));
    assert!(paused.done_bytes < 4 * SIZE as u64, "arrêtée avant la fin");

    engine.resume(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_file(dir.path(), "p.bin", 4 * SIZE);
    engine.shutdown().await;
}

#[tokio::test]
async fn resume_during_a_pause_restarts_the_task() {
    let dir = tempfile::tempdir().unwrap();
    let server = slow_site(4 * SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/r.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    // Reprise demandée avant que la pause ne soit enregistrée.
    engine.pause(&id).await.unwrap();
    engine.resume(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_file(dir.path(), "r.bin", 4 * SIZE);
    engine.shutdown().await;
}

#[tokio::test]
async fn cancel_stops_the_task_and_deletes_its_part_file() {
    let dir = tempfile::tempdir().unwrap();
    let server = slow_site(4 * SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/x.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    // Laisse le temps de créer le `.part`.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(dir.path().join("x.bin.part").exists());
    engine.cancel(&id, true).await.unwrap();
    wait_state(&mut events, &id, TaskState::Cancelled).await;
    assert!(!dir.path().join("x.bin.part").exists());
    assert!(!dir.path().join("x.bin").exists());
    // Une tâche annulée ne repart plus.
    assert!(matches!(
        engine.resume(&id).await,
        Err(EngineError::InvalidState { .. })
    ));
    engine.shutdown().await;
}

#[tokio::test]
async fn a_task_added_paused_waits_for_resume() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let task_add = AddTask {
        start_paused: true,
        ..add(&server.url("/s.bin"), dir.path())
    };
    let id = engine.add(task_add).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(task(dir.path(), &id).state, TaskState::Paused);
    assert!(
        server.requests().is_empty(),
        "aucune requête avant la reprise"
    );
    engine.resume(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    engine.shutdown().await;
}

#[tokio::test]
async fn a_busy_server_is_retried_automatically() {
    let dir = tempfile::tempdir().unwrap();
    let (site, server) = fast_site(SIZE);
    site.lock().unwrap().busy_responses = 1;
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/busy.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::RetryWait).await;
    assert_eq!(
        task(dir.path(), &id).error_code.as_deref(),
        Some("HTTP_STATUS")
    );
    // Nouvelle tentative après 2 à 3 s (SPEC §7.8), sans intervention.
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_file(dir.path(), "busy.bin", SIZE);
    engine.shutdown().await;
}

#[tokio::test]
async fn tasks_interrupted_by_a_crash_resume_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let queue = engine.default_queue().to_owned();
    engine.shutdown().await;
    // Tâche restée `active`, comme après un arrêt brutal du moteur.
    let mut conn = store::open(&dir.path().join("auro.db")).unwrap();
    let url = server.url("/crash.bin");
    let new = NewTask {
        queue_id: &queue,
        url: &url,
        display_name: "crash.bin",
        dest_dir: dir.path().to_str().unwrap(),
        max_connections: None,
    };
    let (id, _) = tasks::create_task(&mut conn, &new).unwrap();
    change_state(&mut conn, &id, &Change::to(TaskState::Active)).unwrap();

    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    if task(dir.path(), &id).state != TaskState::Completed {
        wait_state(&mut events, &id, TaskState::Completed).await;
    }
    assert_file(dir.path(), "crash.bin", SIZE);
    engine.shutdown().await;
}

#[tokio::test]
async fn shutdown_keeps_running_tasks_for_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let server = slow_site(4 * SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/k.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    engine.shutdown().await;
    assert_eq!(task(dir.path(), &id).state, TaskState::Queued);
    assert!(matches!(engine.pause(&id).await, Err(EngineError::Stopped)));

    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    if task(dir.path(), &id).state != TaskState::Completed {
        wait_state(&mut events, &id, TaskState::Completed).await;
    }
    assert_file(dir.path(), "k.bin", 4 * SIZE);
    engine.shutdown().await;
}

#[tokio::test]
async fn bad_requests_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(config(dir.path())).unwrap();
    let ftp = add("ftp://exemple.org/f", dir.path());
    assert!(matches!(engine.add(ftp).await, Err(EngineError::Url(_))));
    let elsewhere = AddTask {
        queue_id: Some("inconnue".into()),
        ..add("http://127.0.0.1:1/f", dir.path())
    };
    assert!(matches!(
        engine.add(elsewhere).await,
        Err(EngineError::QueueNotFound(_))
    ));
    assert!(matches!(
        engine.pause("pas-une-tache").await,
        Err(EngineError::Store(StoreError::TaskNotFound { .. }))
    ));
    engine.shutdown().await;
}

#[tokio::test]
async fn each_change_raises_the_revision_by_one() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let start = engine.rev();
    let id = engine
        .add(add(&server.url("/rev.bin"), dir.path()))
        .await
        .unwrap();
    let mut revs = Vec::new();
    timeout(WAIT, async {
        loop {
            match events.recv().await.unwrap() {
                Event::Progress { rev, .. } => {
                    // L'avancement porte la révision courante, sans l'augmenter.
                    assert!(rev <= start + revs.len() as u64);
                }
                Event::Added { rev, .. } | Event::QueueUpdated { rev, .. } => revs.push(rev),
                Event::StateChanged { rev, to, .. } => {
                    revs.push(rev);
                    if to == TaskState::Completed {
                        return;
                    }
                }
            }
        }
    })
    .await
    .unwrap();
    let expected: Vec<u64> = (start + 1..=start + revs.len() as u64).collect();
    assert_eq!(revs, expected, "aucun trou, aucun doublon");

    let snapshot = engine.snapshot().await.unwrap();
    assert_eq!(snapshot.rev, *revs.last().unwrap());
    let done = snapshot.tasks.iter().find(|t| t.id == id).unwrap();
    assert_eq!(done.state, TaskState::Completed);
    assert_eq!(snapshot.queues.len(), 1);
    engine.shutdown().await;
}

#[tokio::test]
async fn state_changes_carry_the_pause_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let paused = AddTask {
        start_paused: true,
        ..add(&server.url("/q.bin"), dir.path())
    };
    let id = engine.add(paused).await.unwrap();
    let reason = timeout(WAIT, async {
        loop {
            if let Ok(Event::StateChanged {
                task_id,
                to: TaskState::Paused,
                pause_reason,
                ..
            }) = events.recv().await
            {
                if task_id == id {
                    return pause_reason;
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(reason.as_deref(), Some("user"));
    engine.shutdown().await;
}

#[tokio::test]
async fn queues_are_created_listed_and_updated() {
    let dir = tempfile::tempdir().unwrap();
    let server = slow_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let slow = engine.create_queue("lente", 1).await.unwrap();
    assert!(matches!(
        engine.create_queue("lente", 1).await,
        Err(EngineError::BadRequest(_))
    ));
    assert!(matches!(
        engine.create_queue("vide", 0).await,
        Err(EngineError::BadRequest(_))
    ));
    let a = AddTask {
        queue_id: Some(slow.clone()),
        ..add(&server.url("/qa.bin"), dir.path())
    };
    let b = AddTask {
        queue_id: Some(slow.clone()),
        ..add(&server.url("/qb.bin"), dir.path())
    };
    let (a, b) = (engine.add(a).await.unwrap(), engine.add(b).await.unwrap());
    // Une seule à la fois dans la file lente ; relever la limite laisse partir l'autre.
    wait_state(&mut events, &a, TaskState::Active).await;
    let (queued, _) = engine
        .list(TaskFilter {
            queue_id: Some(slow.clone()),
            states: vec![TaskState::Queued],
            ..TaskFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(queued.iter().map(|t| &t.id).collect::<Vec<_>>(), [&b]);
    let updated = engine
        .update_queue(&slow, Some("rapide".into()), Some(2))
        .await
        .unwrap();
    assert_eq!((updated.name.as_str(), updated.max_active), ("rapide", 2));
    wait_state(&mut events, &b, TaskState::Active).await;
    assert!(matches!(
        engine.update_queue("inconnue", None, Some(2)).await,
        Err(EngineError::QueueNotFound(_))
    ));
    let names: Vec<String> = engine
        .snapshot()
        .await
        .unwrap()
        .queues
        .into_iter()
        .map(|q| q.name)
        .collect();
    assert_eq!(names, [DEFAULT_QUEUE, "rapide"]);
    engine.shutdown().await;
}

/// Une annonce et un instantané de même révision décrivent le même état (SPEC §6.10) :
/// quand la boucle annonce une tâche `active`, la base le dit déjà, avant même que
/// l'exécution n'ait commencé. Sans cela, un client qui suit le protocole (instantané,
/// puis notifications plus récentes) voyait la tâche rester « en file ».
#[tokio::test]
async fn an_announced_state_is_already_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let conn = store::open(&config.db_path).unwrap();
    let queue = tasks::ensure_queue(&conn, DEFAULT_QUEUE).unwrap();
    let (events, mut announced) = broadcast::channel(16);
    let mut scheduler = Scheduler::new(conn, config, events, Arc::default(), queue);
    let id = scheduler
        .add(add("http://127.0.0.1:1/f", dir.path()))
        .unwrap();

    // Démarre la tâche sans rendre la main : l'exécution lancée n'a pas encore tourné.
    scheduler.start_runnable();
    let mut active_announced = false;
    while let Ok(event) = announced.try_recv() {
        if let Event::StateChanged {
            to: TaskState::Active,
            ..
        } = event
        {
            active_announced = true;
        }
    }
    assert!(active_announced);
    assert_eq!(
        load_task(&scheduler.conn, &id).unwrap().state,
        TaskState::Active,
        "annoncée active, encore en file en base"
    );
    scheduler.begin_shutdown(None);
}
