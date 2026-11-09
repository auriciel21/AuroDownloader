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
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
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
        dest_dir: Some(dir.to_str().unwrap().to_owned()),
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
async fn each_run_announces_the_name_and_size_it_prepared() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let first = engine
        .add(add(&server.url("/nom.bin"), dir.path()))
        .await
        .unwrap();
    let second = engine
        .add(add(&server.url("/nom.bin"), dir.path()))
        .await
        .unwrap();
    let mut prepared = HashMap::new();
    let mut completed = HashSet::new();
    timeout(WAIT, async {
        // L'annonce peut suivre la fin d'une exécution rapide : attendre les deux.
        while prepared.len() < 2 || completed.len() < 2 {
            match events.recv().await {
                Ok(Event::Prepared {
                    task_id,
                    file_name,
                    total_bytes,
                    ..
                }) => {
                    prepared.insert(task_id, (file_name, total_bytes));
                }
                Ok(Event::StateChanged {
                    task_id,
                    to: TaskState::Completed,
                    ..
                }) => {
                    completed.insert(task_id);
                }
                Ok(_) => {}
                Err(e) => panic!("{e}"),
            }
        }
    })
    .await
    .expect("deux préparations annoncées");
    for id in [&first, &second] {
        let row = task(dir.path(), id);
        assert_eq!(prepared[id], (row.file_name.clone(), Some(SIZE as u64)));
    }
    let mut names: Vec<_> = prepared.values().map(|(n, _)| n.clone().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["nom (1).bin", "nom.bin"]);
    engine.shutdown().await;
}

/// Attend l'annonce du retrait de `task_id`.
async fn wait_removed(events: &mut broadcast::Receiver<Event>, task_id: &str) {
    let found = timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(Event::Removed { task_id: t, .. }) if t == task_id => return,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("moteur arrêté"),
            }
        }
    })
    .await;
    assert!(found.is_ok(), "retrait de {task_id} jamais annoncé");
}

#[tokio::test]
async fn finished_tasks_are_removed_with_or_without_their_files() {
    let dir = tempfile::tempdir().unwrap();
    let (_, fast) = fast_site(SIZE);
    let slow = slow_site(4 * SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();

    // Terminées : l'une garde son fichier, l'autre le perd.
    let keep = engine
        .add(add(&fast.url("/garde.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &keep, TaskState::Completed).await;
    let gone = engine
        .add(add(&fast.url("/jette.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &gone, TaskState::Completed).await;
    engine.remove(&keep, false).await.unwrap();
    wait_removed(&mut events, &keep).await;
    assert_file(dir.path(), "garde.bin", SIZE);
    engine.remove(&gone, true).await.unwrap();
    wait_removed(&mut events, &gone).await;
    assert!(!dir.path().join("jette.bin").exists());
    for id in [&keep, &gone] {
        assert!(matches!(
            engine.get(id).await,
            Err(EngineError::Store(StoreError::TaskNotFound { .. }))
        ));
    }

    // En pause : refusée tant qu'elle n'est pas annulée ; annulée, son `.part` part avec.
    let paused = engine
        .add(add(&slow.url("/lente.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &paused, TaskState::Active).await;
    engine.pause(&paused).await.unwrap();
    wait_state(&mut events, &paused, TaskState::Paused).await;
    let part = dir.path().join("lente.bin.part");
    assert!(part.exists());
    assert!(matches!(
        engine.remove(&paused, true).await,
        Err(EngineError::InvalidState { .. })
    ));
    assert!(part.exists(), "rien n'est supprimé d'une tâche refusée");
    engine.cancel(&paused, false).await.unwrap();
    wait_state(&mut events, &paused, TaskState::Cancelled).await;
    assert!(part.exists());
    engine.remove(&paused, true).await.unwrap();
    wait_removed(&mut events, &paused).await;
    assert!(!part.exists());
    assert!(matches!(
        engine.remove(&paused, true).await,
        Err(EngineError::Store(StoreError::TaskNotFound { .. }))
    ));

    let snapshot = engine.snapshot().await.unwrap();
    assert!(snapshot.tasks.is_empty(), "plus aucune tâche");
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
            speed_limit_bps: None,
            idempotency_key: None,
            requested_name: None,
            checksum: None,
            headers: &[],
            parent_id: None,
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
    // Le `.part` naît à la préparation ; la machine peut être chargée (autres tests).
    let part = dir.path().join("x.bin.part");
    let created = timeout(WAIT, async {
        while !part.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(created.is_ok(), "le .part n'a jamais été créé");
    engine.cancel(&id, true).await.unwrap();
    wait_state(&mut events, &id, TaskState::Cancelled).await;
    // Le moteur annonce l'annulation, puis efface le `.part`.
    let gone = timeout(WAIT, async {
        while part.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "le .part reste");
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
        speed_limit_bps: None,
        idempotency_key: None,
        requested_name: None,
        checksum: None,
        headers: &[],
        parent_id: None,
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
                Event::Added { rev, .. }
                | Event::QueueUpdated { rev, .. }
                | Event::TaskUpdated { rev, .. }
                | Event::Prepared { rev, .. }
                | Event::Removed { rev, .. }
                | Event::Restarted { rev, .. }
                | Event::Relocated { rev, .. }
                | Event::SourceChanged { rev, .. }
                | Event::LimitsChanged { rev, .. }
                | Event::DownloadDirChanged { rev, .. }
                | Event::FlagChanged { rev, .. }
                | Event::NetworkChanged { rev, .. }
                | Event::TorrentInterfaceChanged { rev, .. }
                | Event::ToolsChanged { rev, .. }
                | Event::Warning { rev, .. }
                | Event::SettingChanged { rev, .. }
                | Event::ScheduleChanged { rev, .. }
                | Event::ScheduleRemoved { rev, .. }
                | Event::EndAction { rev, .. } => revs.push(rev),
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
    let slow = engine.create_queue("lente", 1, None).await.unwrap();
    assert!(matches!(
        engine.create_queue("lente", 1, None).await,
        Err(EngineError::BadRequest(_))
    ));
    assert!(matches!(
        engine.create_queue("vide", 0, None).await,
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
        .update_queue(
            &slow,
            QueueChange {
                name: Some("rapide".into()),
                max_active: Some(2),
                ..QueueChange::default()
            },
        )
        .await
        .unwrap();
    assert_eq!((updated.name.as_str(), updated.max_active), ("rapide", 2));
    wait_state(&mut events, &b, TaskState::Active).await;
    assert!(matches!(
        engine
            .update_queue(
                "inconnue",
                QueueChange {
                    max_active: Some(2),
                    ..QueueChange::default()
                }
            )
            .await,
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

/// Vrai si la tâche est terminée après `during`.
async fn completed_after(dir: &Path, id: &str, during: Duration) -> bool {
    tokio::time::sleep(during).await;
    task(dir, id).state == TaskState::Completed
}

#[tokio::test]
async fn the_global_limit_applies_on_the_fly_and_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(1024 * 1024);
    let engine = Engine::start(config(dir.path())).unwrap();
    engine.set_download_limit(Some(100_000)).await.unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/global.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    // 1 Mio à 100 ko/s : une dizaine de secondes.
    assert!(!completed_after(dir.path(), &id, Duration::from_secs(1)).await);
    engine.set_download_limit(None).await.unwrap();
    let start = Instant::now();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "limite levée trop tard"
    );
    assert_eq!(engine.download_limit().await.unwrap(), None);
    assert!(matches!(
        engine.set_download_limit(Some(0)).await,
        Err(EngineError::BadRequest(_))
    ));

    // Enregistrée : elle vaut encore au démarrage suivant.
    engine.set_download_limit(Some(123_456)).await.unwrap();
    engine.shutdown().await;
    let engine = Engine::start(config(dir.path())).unwrap();
    assert_eq!(engine.download_limit().await.unwrap(), Some(123_456));
    engine.shutdown().await;
}

#[tokio::test]
async fn task_and_queue_limits_change_on_the_fly() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(1024 * 1024);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let queue = engine
        .create_queue("bridée", 3, Some(100_000))
        .await
        .unwrap();
    let limited = AddTask {
        queue_id: Some(queue.clone()),
        speed_limit_bps: Some(50_000),
        ..add(&server.url("/bride.bin"), dir.path())
    };
    let id = engine.add(limited).await.unwrap();
    assert_eq!(task(dir.path(), &id).speed_limit_bps, Some(50_000));
    wait_state(&mut events, &id, TaskState::Active).await;
    assert!(!completed_after(dir.path(), &id, Duration::from_millis(500)).await);

    // Limite de la tâche retirée : celle de la file tient encore.
    let lifted = TaskUpdate {
        speed_limit_bps: Some(None),
        ..TaskUpdate::default()
    };
    let row = engine.update_task(&id, lifted.clone()).await.unwrap();
    assert_eq!(row.speed_limit_bps, None);
    let announced = timeout(WAIT, async {
        loop {
            if let Ok(Event::TaskUpdated {
                task_id, update, ..
            }) = events.recv().await
            {
                if task_id == id {
                    return update;
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(announced, lifted);
    assert!(!completed_after(dir.path(), &id, Duration::from_millis(500)).await);

    // Limite de la file retirée : plus rien ne retient la tâche.
    let free = QueueChange {
        speed_limit_bps: Some(None),
        ..QueueChange::default()
    };
    let queue_row = engine.update_queue(&queue, free).await.unwrap();
    assert_eq!(queue_row.speed_limit_bps, None);
    let start = Instant::now();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "limite levée trop tard"
    );

    let zero = TaskUpdate {
        speed_limit_bps: Some(Some(0)),
        ..TaskUpdate::default()
    };
    assert!(matches!(
        engine.update_task(&id, zero).await,
        Err(EngineError::BadRequest(_))
    ));
    assert!(matches!(
        engine.update_task("inconnue", lifted).await,
        Err(EngineError::Store(StoreError::TaskNotFound { .. }))
    ));
    engine.shutdown().await;
}

#[tokio::test]
async fn a_task_short_of_space_starts_again_when_space_returns() {
    static FREE: AtomicU64 = AtomicU64::new(0);
    fn free(_: &Path) -> std::io::Result<u64> {
        Ok(FREE.load(Ordering::SeqCst))
    }
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let mut config = config(dir.path());
    config.transfer.free_space = free;
    config.disk_recheck = Duration::from_millis(50);
    let engine = Engine::start(config).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/place.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Paused).await;
    let paused = task(dir.path(), &id);
    assert_eq!(paused.pause_reason.as_deref(), Some("disk_full"));
    assert_eq!(paused.error_code.as_deref(), Some("DISK_FULL"));
    // Plusieurs vérifications passent sans place : la tâche attend.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(task(dir.path(), &id).state, TaskState::Paused);

    FREE.store(u64::MAX, Ordering::SeqCst);
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_file(dir.path(), "place.bin", SIZE);
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
    let (prepared, _) = mpsc::unbounded_channel();
    let mut scheduler = Scheduler::new(conn, config, events, Arc::default(), queue, None, prepared);
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

/// Planification (F-16, SPEC §7.11) : plages horaires et action de fin.
mod planning {
    use std::sync::atomic::{AtomicI64, Ordering};

    use serde_json::{Value, json};

    use super::*;
    use crate::schedule::{EndAction, LocalNow, Rules};

    /// Lundi 2026-11-02, minuit, en heure locale.
    const MONDAY: i64 = 20_759 * 86_400_000;

    fn monday(h: i64, m: i64) -> i64 {
        MONDAY + h * 3_600_000 + m * 60_000
    }

    fn planning_config(
        dir: &Path,
        clock: fn(&Connection) -> LocalNow,
        power: fn(EndAction) -> std::io::Result<()>,
    ) -> Config {
        Config {
            planning: Planning {
                clock,
                check_every: Duration::from_millis(20),
                end_delay: Duration::from_millis(300),
                power,
                ..Planning::default()
            },
            ..config(dir)
        }
    }

    /// Crée une planification et l'attache à la file par défaut.
    async fn attach(engine: &Engine, rules: Value) -> ScheduleView {
        let rules = Rules::from_json(&rules).unwrap();
        let view = engine
            .set_schedule(None, "Essai".into(), true, rules)
            .await
            .unwrap();
        let change = QueueChange {
            schedule_id: Some(Some(view.schedule.id.clone())),
            ..QueueChange::default()
        };
        let queue = engine.default_queue().to_owned();
        engine.update_queue(&queue, change).await.unwrap();
        view
    }

    static WINDOW_CLOCK: AtomicI64 = AtomicI64::new(0);

    fn window_clock(_: &Connection) -> LocalNow {
        LocalNow {
            utc_ms: WINDOW_CLOCK.load(Ordering::SeqCst),
            offset_ms: 0,
        }
    }

    #[tokio::test]
    async fn a_scheduled_queue_downloads_only_inside_its_window() {
        WINDOW_CLOCK.store(monday(12, 0), Ordering::SeqCst);
        let dir = tempfile::tempdir().unwrap();
        let len = 8 << 20;
        let server = slow_site(len);
        let engine = Engine::start(planning_config(
            dir.path(),
            window_clock,
            crate::power::unplugged,
        ))
        .unwrap();
        let mut events = engine.subscribe();
        let view = attach(
            &engine,
            json!({"windows": [{"days": [1], "start": "13:00", "end": "14:00"}]}),
        )
        .await;
        assert!(!view.open);
        assert_eq!(view.next_change_at, Some(monday(13, 0)));
        let snapshot = engine.snapshot().await.unwrap();
        assert_eq!(
            snapshot.queues[0].schedule_id.as_deref(),
            Some(view.schedule.id.as_str())
        );

        // Fermée : la tâche attend.
        let id = engine
            .add(add(&server.url("/plage.bin"), dir.path()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(engine.get(&id).await.unwrap().state, TaskState::Queued);

        // La plage s'ouvre : elle part.
        WINDOW_CLOCK.store(monday(13, 0), Ordering::SeqCst);
        wait_state(&mut events, &id, TaskState::Active).await;
        tokio::time::sleep(Duration::from_millis(400)).await;

        // La plage se ferme : elle retourne en file ; ce qu'elle a reçu reste.
        WINDOW_CLOCK.store(monday(14, 0), Ordering::SeqCst);
        wait_state(&mut events, &id, TaskState::Queued).await;
        let waiting = engine.get(&id).await.unwrap();
        assert!(
            waiting.done_bytes > 0 && waiting.done_bytes < len as u64,
            "{}",
            waiting.done_bytes
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(engine.get(&id).await.unwrap().state, TaskState::Queued);
        assert!(!engine.schedules().await.unwrap()[0].open);

        // Rouverte : elle reprend où elle en était et finit.
        WINDOW_CLOCK.store(monday(13, 30), Ordering::SeqCst);
        wait_state(&mut events, &id, TaskState::Completed).await;
        assert_file(dir.path(), "plage.bin", len);
        assert!(engine.schedules().await.unwrap()[0].open);

        // Supprimée : la file ne l'a plus.
        engine.delete_schedule(&view.schedule.id).await.unwrap();
        assert_eq!(engine.snapshot().await.unwrap().queues[0].schedule_id, None);
        assert!(matches!(
            engine.delete_schedule(&view.schedule.id).await,
            Err(EngineError::ScheduleNotFound(_))
        ));
        engine.shutdown().await;
    }

    static CALLS: Mutex<Vec<EndAction>> = Mutex::new(Vec::new());

    fn record(action: EndAction) -> std::io::Result<()> {
        CALLS.lock().unwrap().push(action);
        Ok(())
    }

    /// Prochaine annonce d'action de fin.
    async fn end_action(
        events: &mut broadcast::Receiver<Event>,
    ) -> (Option<PendingEnd>, Option<String>) {
        timeout(WAIT, async {
            loop {
                match events.recv().await {
                    Ok(Event::EndAction { pending, error, .. }) => return (pending, error),
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => panic!("moteur arrêté"),
                }
            }
        })
        .await
        .expect("aucune action de fin annoncée")
    }

    #[tokio::test]
    async fn the_end_action_follows_the_last_download_and_can_be_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let (_, server) = fast_site(SIZE);
        let engine = Engine::start(planning_config(
            dir.path(),
            crate::schedule::local_now,
            record,
        ))
        .unwrap();
        let mut events = engine.subscribe();
        // Ouverte à toute heure, la veille à la fin.
        attach(
            &engine,
            json!({
                "windows": [{"days": [1, 2, 3, 4, 5, 6, 7], "start": "00:00", "end": "00:00"}],
                "onDone": "sleep",
            }),
        )
        .await;

        // La file se vide : la veille est annoncée, puis exécutée une fois.
        let before = now_ms();
        engine
            .add(add(&server.url("/a.bin"), dir.path()))
            .await
            .unwrap();
        let pending = end_action(&mut events).await.0.expect("annonce");
        assert_eq!(pending.action, EndAction::Sleep);
        assert_eq!(pending.queue_id, engine.default_queue());
        assert!(pending.at_ms >= before + 300, "{pending:?}");
        assert_eq!(
            engine.snapshot().await.unwrap().end_action,
            Some(pending.clone())
        );
        assert_eq!(end_action(&mut events).await, (None, None));
        assert_eq!(*CALLS.lock().unwrap(), [EndAction::Sleep]);

        // Annoncée de nouveau, puis annulée : rien ne s'exécute.
        engine
            .add(add(&server.url("/b.bin"), dir.path()))
            .await
            .unwrap();
        assert!(end_action(&mut events).await.0.is_some());
        assert!(engine.cancel_end_action().await.unwrap());
        assert_eq!(end_action(&mut events).await, (None, None));
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(CALLS.lock().unwrap().len(), 1);
        assert!(!engine.cancel_end_action().await.unwrap());

        // Du travail revient dans la file pendant l'annonce : elle tombe d'elle-même.
        engine
            .add(add(&server.url("/c.bin"), dir.path()))
            .await
            .unwrap();
        assert!(end_action(&mut events).await.0.is_some());
        let slow = slow_site(8 << 20);
        engine
            .add(add(&slow.url("/d.bin"), dir.path()))
            .await
            .unwrap();
        assert_eq!(end_action(&mut events).await, (None, None));
        assert_eq!(CALLS.lock().unwrap().len(), 1);
        engine.shutdown().await;
    }

    static CHANGED_CALLS: Mutex<Vec<EndAction>> = Mutex::new(Vec::new());

    fn record_changed(action: EndAction) -> std::io::Result<()> {
        CHANGED_CALLS.lock().unwrap().push(action);
        Ok(())
    }

    #[tokio::test]
    async fn an_announced_end_action_falls_when_its_schedule_no_longer_asks_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let (_, server) = fast_site(SIZE);
        let engine = Engine::start(planning_config(
            dir.path(),
            crate::schedule::local_now,
            record_changed,
        ))
        .unwrap();
        let mut events = engine.subscribe();
        let rules = json!({
            "windows": [{"days": [1, 2, 3, 4, 5, 6, 7], "start": "00:00", "end": "00:00"}],
            "onDone": "shutdown",
        });
        let view = attach(&engine, rules.clone()).await;
        engine
            .add(add(&server.url("/f.bin"), dir.path()))
            .await
            .unwrap();
        assert!(end_action(&mut events).await.0.is_some());

        // « Ne rien faire » choisi pendant le décompte : l'annonce tombe, rien ne s'exécute.
        let mut none = Rules::from_json(&rules).unwrap();
        none.on_done = EndAction::None;
        engine
            .set_schedule(Some(view.schedule.id), "Essai".into(), true, none)
            .await
            .unwrap();
        assert_eq!(end_action(&mut events).await, (None, None));
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(CHANGED_CALLS.lock().unwrap().is_empty());
        engine.shutdown().await;
    }

    static CLOSED_CLOCK: AtomicI64 = AtomicI64::new(0);

    fn closed_clock(_: &Connection) -> LocalNow {
        LocalNow {
            utc_ms: CLOSED_CLOCK.load(Ordering::SeqCst),
            offset_ms: 0,
        }
    }

    #[tokio::test]
    async fn a_task_left_active_in_a_closed_queue_goes_back_to_waiting() {
        CLOSED_CLOCK.store(monday(12, 0), Ordering::SeqCst);
        let dir = tempfile::tempdir().unwrap();
        let server = slow_site(1 << 20);
        let make = || planning_config(dir.path(), closed_clock, crate::power::unplugged);
        let engine = Engine::start(make()).unwrap();
        attach(
            &engine,
            json!({"windows": [{"days": [1], "start": "13:00", "end": "14:00"}]}),
        )
        .await;
        let id = engine
            .add(add(&server.url("/g.bin"), dir.path()))
            .await
            .unwrap();
        engine.shutdown().await;

        // Arrêt brutal pendant le téléchargement : la tâche est restée `active` en base.
        let mut conn = store::open(&dir.path().join("auro.db")).unwrap();
        change_state(&mut conn, &id, &Change::to(TaskState::Active)).unwrap();
        drop(conn);

        // Redémarré hors de la plage : elle retourne en file, sans partir.
        let engine = Engine::start(make()).unwrap();
        let task = timeout(WAIT, async {
            loop {
                let task = engine.get(&id).await.unwrap();
                if task.state != TaskState::Active {
                    return task;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(task.state, TaskState::Queued);
        assert!(server.requests().is_empty(), "rien ne doit partir");
        engine.shutdown().await;
    }

    #[tokio::test]
    async fn without_a_plugged_power_action_the_refusal_is_announced() {
        let dir = tempfile::tempdir().unwrap();
        let (_, server) = fast_site(SIZE);
        let engine = Engine::start(planning_config(
            dir.path(),
            crate::schedule::local_now,
            crate::power::unplugged,
        ))
        .unwrap();
        let mut events = engine.subscribe();
        attach(
            &engine,
            json!({
                "windows": [{"days": [1, 2, 3, 4, 5, 6, 7], "start": "00:00", "end": "00:00"}],
                "onDone": "shutdown",
            }),
        )
        .await;
        engine
            .add(add(&server.url("/e.bin"), dir.path()))
            .await
            .unwrap();
        assert!(end_action(&mut events).await.0.is_some());
        let (pending, error) = end_action(&mut events).await;
        assert!(pending.is_none());
        assert!(error.is_some_and(|e| e.contains("aucune action de fin")));
        engine.shutdown().await;
    }
}

/// En-têtes d'une tâche (SPEC §5.1) : envoyés à chaque requête ; le secret va au coffre,
/// jamais en base, et en sort quand la tâche est finie.
#[tokio::test]
async fn task_headers_are_sent_and_secrets_leave_the_vault_when_done() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let config = config(dir.path());
    let vault = config.transfer.vault.clone();
    let engine = Engine::start(config).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(AddTask {
            headers: vec![
                ("Referer".into(), "https://exemple.org/page".into()),
                ("Cookie".into(), "session=s3cr3t".into()),
            ],
            ..add(&server.url("/h.bin"), dir.path())
        })
        .await
        .unwrap();
    let conn = store::open(&dir.path().join("auro.db")).unwrap();
    let ns = settings::vault_namespace(&conn).unwrap();
    let scoped = vault.scoped(&ns);
    // En base : la référence du cookie, jamais sa valeur.
    let raw: Vec<(String, Option<String>, Option<String>)> = conn
        .prepare("SELECT name, value, secret_ref FROM task_headers ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(raw.len(), 2);
    assert_eq!(raw[0].0, "Cookie");
    assert_eq!(raw[0].1, None);
    let secret_ref = raw[0].2.clone().unwrap();
    assert_eq!(
        raw[1],
        (
            "Referer".into(),
            Some("https://exemple.org/page".into()),
            None
        )
    );
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_file(dir.path(), "h.bin", SIZE);
    let requests = server.requests();
    assert!(requests.len() >= 2, "préparation puis au moins une plage");
    for r in &requests {
        assert_eq!(
            r.header("cookie"),
            Some("session=s3cr3t"),
            "{} {}",
            r.method,
            r.path
        );
        assert_eq!(r.header("referer"), Some("https://exemple.org/page"));
    }
    // Tâche finie : la référence quitte la base, puis le secret le coffre.
    assert!(headers::secret_refs(&conn, &id).unwrap().is_empty());
    timeout(WAIT, async {
        while scoped.get(&secret_ref).unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("secret effacé du coffre");
    engine.shutdown().await;
}

/// Un en-tête refusé (réservé au moteur, en double) : rien n'est créé, rien n'est rangé.
#[tokio::test]
async fn refused_headers_create_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let vault = config.transfer.vault.clone();
    let engine = Engine::start(config).unwrap();
    for headers in [
        vec![("Range".to_owned(), "bytes=0-".to_owned())],
        vec![
            ("Cookie".to_owned(), "a=1".to_owned()),
            ("cookie".to_owned(), "b=2".to_owned()),
        ],
        vec![("Mauvais nom".to_owned(), "x".to_owned())],
        vec![("X-Ligne".to_owned(), "a\nb".to_owned())],
    ] {
        let refused = engine
            .add(AddTask {
                headers,
                ..add("https://exemple.org/f.bin", dir.path())
            })
            .await;
        assert!(
            matches!(refused, Err(EngineError::BadRequest(_))),
            "{refused:?}"
        );
    }
    let (tasks, _) = engine.list(TaskFilter::default()).await.unwrap();
    assert!(tasks.is_empty());
    let conn = store::open(&dir.path().join("auro.db")).unwrap();
    let ns = settings::vault_namespace(&conn).unwrap();
    assert!(vault.scoped(&ns).ids().unwrap().is_empty());
    engine.shutdown().await;
}

/// Au démarrage, un secret du coffre sans référence en base (arrêt brutal entre les deux)
/// est effacé ; celui d'une tâche, et ceux d'une autre base, restent.
#[tokio::test]
async fn orphan_secrets_are_collected_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let vault = config.transfer.vault.clone();
    let mut conn = store::open(&config.db_path).unwrap();
    let ns = settings::vault_namespace(&conn).unwrap();
    let scoped = vault.scoped(&ns);
    let elsewhere = vault.scoped("une-autre-base");
    let orphan = scoped.put("orphelin=1").unwrap();
    let kept = scoped.put("garde=1").unwrap();
    let foreign = elsewhere.put("autre=1").unwrap();
    let queue = tasks::ensure_queue(&conn, DEFAULT_QUEUE).unwrap();
    let stored = [("Cookie".to_owned(), Stored::Secret(kept.clone()))];
    tasks::create_task(
        &mut conn,
        &NewTask {
            queue_id: &queue,
            url: "https://exemple.org/f.bin",
            display_name: "f.bin",
            dest_dir: dir.path().to_str().unwrap(),
            max_connections: None,
            speed_limit_bps: None,
            idempotency_key: None,
            requested_name: None,
            checksum: None,
            headers: &stored,
            parent_id: None,
        },
    )
    .unwrap();
    // Qu'elle ne parte pas : seul le ramassage compte ici.
    let paused = tasks::runnable_tasks(&conn, now_ms()).unwrap();
    for c in paused {
        change_state(&mut conn, &c.task_id, &user_pause()).unwrap();
    }
    drop(conn);
    let engine = Engine::start(config).unwrap();
    timeout(WAIT, async {
        while scoped.get(&orphan).unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("orphelin effacé");
    assert_eq!(scoped.get(&kept).unwrap().as_deref(), Some("garde=1"));
    assert_eq!(elsewhere.get(&foreign).unwrap().as_deref(), Some("autre=1"));
    engine.shutdown().await;
}

/// « Retélécharger » (SPEC §5.5) : une tâche terminée repart de zéro. Avec
/// `delete_files`, sous le même nom ; sans, l'ancien fichier reste et le nouveau en prend
/// un autre. Refusé pendant le téléchargement.
#[tokio::test]
async fn a_completed_task_is_downloaded_again() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/encore.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    let first = server.requests().len();

    // Remplacé : supprimé, puis retéléchargé sous le même nom.
    engine.restart(&id, true).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert!(
        server.requests().len() > first,
        "de nouveau demandé au serveur"
    );
    assert_file(dir.path(), "encore.bin", SIZE);
    assert_eq!(
        task(dir.path(), &id).file_name.as_deref(),
        Some("encore.bin")
    );

    // Gardé : le nouveau prend un autre nom.
    engine.restart(&id, false).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_file(dir.path(), "encore.bin", SIZE);
    assert_file(dir.path(), "encore (1).bin", SIZE);
    assert_eq!(
        task(dir.path(), &id).file_name.as_deref(),
        Some("encore (1).bin")
    );
    engine.shutdown().await;
}

#[tokio::test]
async fn a_running_or_queued_task_is_not_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let server = slow_site(4 * SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/lent.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    assert!(matches!(
        engine.restart(&id, true).await,
        Err(EngineError::InvalidState { .. })
    ));
    // En pause, le `.part` est supprimé et la tâche repart de zéro.
    engine.pause(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Paused).await;
    let name = task(dir.path(), &id).file_name.unwrap();
    assert!(part_path(&dir.path().join(&name)).exists());
    engine.restart(&id, false).await.unwrap();
    timeout(WAIT, async {
        loop {
            if let Ok(Event::Restarted { task_id, .. }) = events.recv().await
                && task_id == id
            {
                return;
            }
        }
    })
    .await
    .expect("retéléchargement annoncé");
    wait_state(&mut events, &id, TaskState::Queued).await;
    engine.cancel(&id, true).await.unwrap();
    engine.shutdown().await;
}

/// Attend la fin annoncée d'un déplacement de `task_id` ; renvoie son erreur éventuelle.
async fn wait_relocated(events: &mut broadcast::Receiver<Event>, task_id: &str) -> Option<String> {
    timeout(WAIT, async {
        loop {
            if let Ok(Event::Relocated {
                task_id: t,
                moving: false,
                error,
                ..
            }) = events.recv().await
                && t == task_id
            {
                return error;
            }
        }
    })
    .await
    .expect("fin du déplacement annoncée")
}

fn dir_text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_paused_task_is_moved_and_renamed_then_finishes_there() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("ailleurs");
    std::fs::create_dir(&elsewhere).unwrap();
    let server = slow_site(4 * SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/avant.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    engine.pause(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Paused).await;
    assert!(part_path(&dir.path().join("avant.bin")).exists());

    let moved = engine
        .relocate(&id, Some(dir_text(&elsewhere)), Some("après.bin".into()))
        .await
        .unwrap();
    assert_eq!(moved.dest_dir, dir_text(&elsewhere));
    assert_eq!(moved.file_name.as_deref(), Some("après.bin"));
    assert_eq!(moved.display_name, "après.bin");
    assert!(!moved.moving, "même volume : fini au retour");
    assert!(!part_path(&dir.path().join("avant.bin")).exists());
    assert!(part_path(&elsewhere.join("après.bin")).exists());

    engine.resume(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_file(&elsewhere, "après.bin", 4 * SIZE);

    // Le nom choisi survit au retéléchargement.
    engine.restart(&id, true).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_file(&elsewhere, "après.bin", 4 * SIZE);
    assert_eq!(
        task(dir.path(), &id).file_name.as_deref(),
        Some("après.bin")
    );
    engine.shutdown().await;
}

#[tokio::test]
async fn a_completed_file_is_copied_to_another_volume() {
    let dir = tempfile::tempdir().unwrap();
    let other = dir.path().join(crate::moves::SIMULATED_VOLUME);
    std::fs::create_dir(&other).unwrap();
    let (_, server) = fast_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/fini.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;

    let started = engine
        .relocate(&id, Some(dir_text(&other)), None)
        .await
        .unwrap();
    assert!(started.moving, "copie en cours au retour");
    assert_eq!(
        started.dest_dir,
        dir_text(dir.path()),
        "pas encore déplacée"
    );
    assert!(matches!(
        engine.remove(&id, true).await,
        Err(EngineError::Moving(_))
    ));
    assert_eq!(wait_relocated(&mut events, &id).await, None);

    let t = task(dir.path(), &id);
    assert_eq!((t.dest_dir, t.moving), (dir_text(&other), false));
    assert!(!dir.path().join("fini.bin").exists(), "original effacé");
    assert_file(&other, "fini.bin", SIZE);
    assert!(!crate::moves::temp_for(&other.join("fini.bin")).exists());
    let conn = store::open(&dir.path().join("auro.db")).unwrap();
    let final_path: String = conn
        .query_row(
            "SELECT final_path FROM history WHERE task_id = ?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(final_path, other.join("fini.bin").to_string_lossy());
    engine.shutdown().await;
}

#[tokio::test]
async fn a_task_not_yet_prepared_takes_the_chosen_name() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    // Nom donné à l'ajout.
    let given = engine
        .add(AddTask {
            file_name: Some("donné.bin".into()),
            ..add(&server.url("/serveur.bin"), dir.path())
        })
        .await
        .unwrap();
    assert_eq!(task(dir.path(), &given).display_name, "donné.bin");
    wait_state(&mut events, &given, TaskState::Completed).await;
    assert_file(dir.path(), "donné.bin", SIZE);

    // Nom changé avant la préparation.
    let later = engine
        .add(AddTask {
            start_paused: true,
            ..add(&server.url("/serveur.bin"), dir.path())
        })
        .await
        .unwrap();
    let t = engine
        .relocate(&later, None, Some("choisi.bin".into()))
        .await
        .unwrap();
    assert_eq!((t.file_name, t.display_name.as_str()), (None, "choisi.bin"));
    engine.resume(&later).await.unwrap();
    wait_state(&mut events, &later, TaskState::Completed).await;
    assert_file(dir.path(), "choisi.bin", SIZE);
    assert!(!dir.path().join("serveur.bin").exists());
    engine.shutdown().await;
}

#[tokio::test]
async fn moves_that_would_lose_or_overwrite_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let server = slow_site(4 * SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/garde.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    assert!(matches!(
        engine.relocate(&id, None, Some("x.bin".into())).await,
        Err(EngineError::InvalidState { .. })
    ));
    engine.pause(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Paused).await;

    std::fs::write(dir.path().join("pris.bin"), b"autre").unwrap();
    assert!(matches!(
        engine.relocate(&id, None, Some("pris.bin".into())).await,
        Err(EngineError::FileExists(_))
    ));
    assert_eq!(
        std::fs::read(dir.path().join("pris.bin")).unwrap(),
        b"autre"
    );
    for bad in ["a/b.bin", "..", " ", "nom "] {
        assert!(
            matches!(
                engine.relocate(&id, None, Some(bad.into())).await,
                Err(EngineError::BadRequest(_))
            ),
            "{bad:?} refusé"
        );
    }
    assert!(matches!(
        engine.relocate(&id, Some("relatif".into()), None).await,
        Err(EngineError::DestDir(_))
    ));
    let t = task(dir.path(), &id);
    assert_eq!(t.file_name.as_deref(), Some("garde.bin"), "rien n'a changé");
    assert!(part_path(&dir.path().join("garde.bin")).exists());
    engine.cancel(&id, true).await.unwrap();

    // Fichier terminé disparu : rien à déplacer.
    let (_, fast) = fast_site(SIZE);
    let done = engine
        .add(add(&fast.url("/parti.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &done, TaskState::Completed).await;
    std::fs::remove_file(dir.path().join("parti.bin")).unwrap();
    assert!(matches!(
        engine.relocate(&done, None, Some("ici.bin".into())).await,
        Err(EngineError::FileMissing(_))
    ));
    engine.shutdown().await;
}

#[tokio::test]
async fn an_expired_address_is_replaced_and_the_download_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let old = slow_site(4 * SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&old.url("/lien.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    engine.pause(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Paused).await;
    let done = task(dir.path(), &id).done_bytes;
    assert!(done > 0, "commencée");

    // Pas le même fichier, ou injoignable : refusé, rien ne change.
    let (_, smaller) = fast_site(3 * SIZE);
    assert!(matches!(
        engine.set_url(&id, &smaller.url("/autre.bin")).await,
        Err(EngineError::SourceMismatch { .. })
    ));
    assert!(matches!(
        engine.set_url(&id, "http://127.0.0.1:1/mort.bin").await,
        Err(EngineError::SourceUnreachable(_))
    ));
    let conn = store::open(&dir.path().join("auro.db")).unwrap();
    assert_eq!(
        tasks::primary_source(&conn, &id).unwrap().url,
        old.url("/lien.bin")
    );

    // Le même fichier, ailleurs, avec un autre ETag : ses validateurs remplacent les
    // anciens, sinon la reprise y verrait un fichier changé.
    let (site, fresh) = fast_site(4 * SIZE);
    site.lock().unwrap().etag = Some("\"v2\"".into());
    engine.set_url(&id, &fresh.url("/neuf.bin")).await.unwrap();
    assert_eq!(
        tasks::primary_source(&conn, &id).unwrap().url,
        fresh.url("/neuf.bin")
    );
    // Octets déjà enregistrés, segment par segment, avant la reprise.
    let saved: Vec<(u64, u64)> = load_segments(&conn, &id)
        .unwrap()
        .iter()
        .filter(|s| s.written > 0)
        .map(|s| (s.start, s.start + s.written))
        .collect();
    // La vérification de la nouvelle adresse (`set_url`) en a déjà demandé le début.
    let checked = fresh.requests().len();
    engine.resume(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_file(dir.path(), "lien.bin", 4 * SIZE);
    // Reprise, pas recommencement : la nouvelle adresse n'a servi que ce qui manquait.
    // Aucune requête ne commence dans des octets déjà là ; la somme des plages ne le dit
    // pas, une coupe (SPEC §7.2) redemandant la fin d'une plage déjà demandée.
    let starts: Vec<u64> = fresh.requests()[checked..]
        .iter()
        .filter(|r| r.method == "GET")
        .filter_map(|r| {
            let (a, _) = r.header("range")?.strip_prefix("bytes=")?.split_once('-')?;
            a.parse().ok()
        })
        .collect();
    assert!(!saved.is_empty(), "rien d'enregistré ({done} octets)");
    // Rien à demander si tout était déjà reçu à la pause (machine rapide).
    for a in &starts {
        assert!(
            !saved.iter().any(|&(s, e)| (s..e).contains(a)),
            "requête depuis l'octet {a}, déjà là : {saved:?} ({done} octets)"
        );
    }
    engine.shutdown().await;
}

#[tokio::test]
async fn cookies_do_not_follow_an_address_to_another_host() {
    let dir = tempfile::tempdir().unwrap();
    let (_, server) = fast_site(SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let id = engine
        .add(AddTask {
            start_paused: true,
            headers: vec![
                ("Referer".into(), "https://exemple.org/page".into()),
                ("Cookie".into(), "session=s3cr3t".into()),
            ],
            ..add(&server.url("/c.bin"), dir.path())
        })
        .await
        .unwrap();
    let conn = store::open(&dir.path().join("auro.db")).unwrap();
    // Même hôte : le cookie reste ; le nom affiché suit l'adresse.
    let t = engine.set_url(&id, &server.url("/d.bin")).await.unwrap();
    assert_eq!(t.display_name, "d.bin");
    assert_eq!(headers::secret_refs(&conn, &id).unwrap().len(), 1);
    // Autre hôte : le cookie est effacé, le Referer reste.
    let other = server.url("/e.bin").replace("127.0.0.1", "localhost");
    assert_ne!(other, server.url("/e.bin"));
    engine.set_url(&id, &other).await.unwrap();
    assert!(headers::secret_refs(&conn, &id).unwrap().is_empty());
    assert_eq!(headers::load(&conn, &id).unwrap().len(), 1);
    engine.shutdown().await;
}

/// Un déplacement interrompu par un arrêt du moteur est fini au démarrage suivant, avant
/// que la tâche ne reparte (SPEC §7.12).
#[tokio::test]
async fn a_move_interrupted_by_a_crash_is_finished_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("ailleurs");
    std::fs::create_dir(&elsewhere).unwrap();
    let server = slow_site(4 * SIZE);
    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server.url("/arrêt.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    engine.pause(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Paused).await;
    engine.shutdown().await;

    // Arrêt simulé entre le renommage du `.part` et l'enregistrement : journal en phase
    // `moving`, fichier déjà à sa nouvelle place ; la tâche, remise en file, attend.
    let from = part_path(&dir.path().join("arrêt.bin"));
    let to = part_path(&elsewhere.join("déplacé.bin"));
    std::fs::rename(&from, &to).unwrap();
    {
        let conn = store::open(&dir.path().join("auro.db")).unwrap();
        store::moves::begin(
            &conn,
            &store::moves::FileMove {
                task_id: id.clone(),
                from_path: from.to_string_lossy().into_owned(),
                to_path: to.to_string_lossy().into_owned(),
                temp_path: None,
                dest_dir: dir_text(&elsewhere),
                file_name: "déplacé.bin".into(),
                phase: store::moves::Phase::Moving,
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET state = 'queued', pause_reason = NULL WHERE id = ?1",
            [&id],
        )
        .unwrap();
    }

    let engine = Engine::start(config(dir.path())).unwrap();
    let mut events = engine.subscribe();
    wait_state(&mut events, &id, TaskState::Completed).await;
    let t = task(dir.path(), &id);
    assert_eq!(t.dest_dir, dir_text(&elsewhere));
    assert!(!t.moving);
    assert_file(&elsewhere, "déplacé.bin", 4 * SIZE);
    assert!(!dir.path().join("arrêt.bin").exists());
    engine.shutdown().await;
}

/// Identifiants de sites (F-06, SPEC §7.14) : demandés, refusés, puis acceptés ; le mot de
/// passe reste au coffre, survit au ramassage du démarrage et part avec les identifiants.
#[tokio::test]
async fn site_logins_answer_protected_servers_and_stay_in_the_vault() {
    const PASSWORD: &str = "s3cr3t-du-site";
    let dir = tempfile::tempdir().unwrap();
    let mut protected = FileSite::new(SIZE as u64);
    protected.auth = Some(crate::testing::SiteAuth::digest("moi", PASSWORD, "SHA-256"));
    let server = serve_file(Arc::new(Mutex::new(protected)));
    let config = config(dir.path());
    let vault = config.transfer.vault.clone();
    let engine = Engine::start(config.clone()).unwrap();
    let mut events = engine.subscribe();

    // Sans identifiants : le serveur les demande, échec définitif et explicite.
    let id = engine
        .add(add(&server.url("/prive.bin"), dir.path()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Failed).await;
    assert_eq!(
        task(dir.path(), &id).error_code.as_deref(),
        Some("AUTH_REQUIRED")
    );

    let refused = [
        ("127.0.0.1", "moi", None),
        ("ftp://127.0.0.1", "moi", Some("x")),
        ("127.0.0.1", "a:b", Some("x")),
        ("127.0.0.1", " ", Some("x")),
        ("127.0.0.1", "moi", Some("")),
    ];
    for (site, username, password) in refused {
        let change = LoginChange {
            id: None,
            site: site.into(),
            username: username.into(),
            password: password.map(Into::into),
        };
        assert!(
            matches!(
                engine.set_login(change).await,
                Err(EngineError::BadRequest(_))
            ),
            "{site} {username} {password:?}"
        );
    }

    // Mauvais mot de passe : refusé par le serveur.
    let saved = engine
        .set_login(LoginChange {
            id: None,
            site: " HTTP://127.0.0.1/ ".into(),
            username: "moi".into(),
            password: Some("faux".into()),
        })
        .await
        .unwrap();
    assert_eq!(saved.site, "http://127.0.0.1");
    let duplicate = LoginChange {
        id: None,
        site: "http://127.0.0.1".into(),
        username: "autre".into(),
        password: Some("x".into()),
    };
    assert!(matches!(
        engine.set_login(duplicate).await,
        Err(EngineError::BadRequest(_))
    ));
    engine.resume(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Failed).await;
    assert_eq!(
        task(dir.path(), &id).error_code.as_deref(),
        Some("AUTH_FAILED")
    );

    // Le bon : le téléchargement aboutit.
    let fixed = engine
        .set_login(LoginChange {
            id: Some(saved.id.clone()),
            site: saved.site.clone(),
            username: "moi".into(),
            password: Some(PASSWORD.into()),
        })
        .await
        .unwrap();
    assert_eq!(fixed, saved);
    engine.resume(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_file(dir.path(), "prive.bin", SIZE);
    assert_eq!(engine.logins().await.unwrap(), vec![saved.clone()]);

    // Jamais en base : ni dans le fichier, ni dans son journal.
    for name in ["auro.db", "auro.db-wal"] {
        if let Ok(bytes) = std::fs::read(dir.path().join(name)) {
            let found = bytes
                .windows(PASSWORD.len())
                .any(|w| w == PASSWORD.as_bytes());
            assert!(!found, "mot de passe dans {name}");
        }
    }
    let conn = store::open(&dir.path().join("auro.db")).unwrap();
    let scoped = vault.scoped(&settings::vault_namespace(&conn).unwrap());
    let secret = store::logins::list(&conn).unwrap()[0].secret_ref.clone();
    assert_eq!(scoped.get(&secret).unwrap().as_deref(), Some(PASSWORD));
    // L'ancien mot de passe a quitté le coffre.
    assert_eq!(scoped.ids().unwrap(), vec![secret.clone()]);

    // Le ramassage du démarrage le garde (un orphelin, lui, part).
    engine.shutdown().await;
    let orphan = scoped.put("orphelin").unwrap();
    let engine = Engine::start(config).unwrap();
    timeout(WAIT, async {
        while scoped.get(&orphan).unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ramassage fait");
    assert_eq!(scoped.get(&secret).unwrap().as_deref(), Some(PASSWORD));

    engine.delete_login(&saved.id).await.unwrap();
    assert_eq!(scoped.get(&secret).unwrap(), None);
    assert!(matches!(
        engine.delete_login(&saved.id).await,
        Err(EngineError::LoginNotFound(_))
    ));
    engine.shutdown().await;
}

/// Empreinte (F-11) : changements d'état annoncés `active` → `verifying` → issue ;
/// empreinte modifiable tant que la tâche n'est pas finie ; tâche restée `verifying` au
/// démarrage : le calcul reprend.
mod checksums {
    use super::*;
    use crate::io::checksum::{Algo, Checksum};
    use sha2::{Digest, Sha256};

    const LEN: usize = 2 * 1024 * 1024;

    fn sha256(data: &[u8]) -> Checksum {
        let hex: String = Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Checksum::new(Algo::Sha256, &hex).unwrap()
    }

    fn wrong() -> Checksum {
        Checksum::new(Algo::Sha1, &"0".repeat(40)).unwrap()
    }

    /// Changements d'état annoncés pour `task_id`, jusqu'à un état final.
    async fn transitions(
        events: &mut broadcast::Receiver<Event>,
        task_id: &str,
    ) -> Vec<(TaskState, TaskState)> {
        let mut seen = Vec::new();
        timeout(WAIT, async {
            loop {
                if let Ok(Event::StateChanged {
                    task_id: t,
                    from,
                    to,
                    ..
                }) = events.recv().await
                    && t == task_id
                {
                    seen.push((from, to));
                    if matches!(to, TaskState::Completed | TaskState::Failed) {
                        return;
                    }
                }
            }
        })
        .await
        .expect("la tâche finit");
        seen
    }

    #[tokio::test]
    async fn the_check_is_announced_between_the_transfer_and_the_end() {
        use TaskState::*;
        let dir = tempfile::tempdir().unwrap();
        let (_, server) = fast_site(LEN);
        let engine = Engine::start(config(dir.path())).unwrap();
        let mut events = engine.subscribe();
        let good = AddTask {
            checksum: Some(sha256(&content(LEN))),
            ..add(&server.url("/bon.bin"), dir.path())
        };
        let id = engine.add(good).await.unwrap();
        assert_eq!(
            transitions(&mut events, &id).await,
            [
                (Queued, Active),
                (Active, Verifying),
                (Verifying, Completed)
            ]
        );
        assert_file(dir.path(), "bon.bin", LEN);
        // Fichier terminé : l'empreinte ne change plus.
        let late = TaskUpdate {
            checksum: Some(None),
            ..TaskUpdate::default()
        };
        assert!(matches!(
            engine.update_task(&id, late).await,
            Err(EngineError::InvalidState { .. })
        ));

        let bad = AddTask {
            checksum: Some(wrong()),
            ..add(&server.url("/faux.bin"), dir.path())
        };
        let id = engine.add(bad).await.unwrap();
        assert_eq!(
            transitions(&mut events, &id).await,
            [(Queued, Active), (Active, Verifying), (Verifying, Failed)]
        );
        let row = task(dir.path(), &id);
        assert_eq!(row.error_code.as_deref(), Some("CHECKSUM_MISMATCH"));
        assert!(!dir.path().join("faux.bin").exists());
        assert!(part_path(&dir.path().join("faux.bin")).exists());

        // Empreinte corrigée sur la tâche en échec, puis relance : elle se termine.
        let fixed = TaskUpdate {
            checksum: Some(Some(sha256(&content(LEN)))),
            ..TaskUpdate::default()
        };
        let row = engine.update_task(&id, fixed).await.unwrap();
        assert_eq!(row.checksum, Some(sha256(&content(LEN))));
        engine.resume(&id).await.unwrap();
        let seen = transitions(&mut events, &id).await;
        assert_eq!(seen.last(), Some(&(Verifying, Completed)), "{seen:?}");
        assert_file(dir.path(), "faux.bin", LEN);
        engine.shutdown().await;
    }

    #[tokio::test]
    async fn a_task_left_verifying_is_checked_again_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let (_, server) = fast_site(LEN);
        let engine = Engine::start(config(dir.path())).unwrap();
        let mut events = engine.subscribe();
        let bad = AddTask {
            checksum: Some(wrong()),
            ..add(&server.url("/f.bin"), dir.path())
        };
        let id = engine.add(bad).await.unwrap();
        wait_state(&mut events, &id, TaskState::Failed).await;
        engine.shutdown().await;

        // Comme un arrêt brutal pendant le calcul, avec la bonne empreinte.
        let conn = store::open(&dir.path().join("auro.db")).unwrap();
        let sum = sha256(&content(LEN));
        conn.execute(
            "UPDATE tasks SET state = 'verifying', error_code = NULL, error_detail = NULL,
               checksum_algo = 'sha256', checksum_value = ?2
             WHERE id = ?1",
            params![id, sum.value()],
        )
        .unwrap();
        drop(conn);

        // Lu dans la base : la fin peut précéder tout abonnement.
        let engine = Engine::start(config(dir.path())).unwrap();
        let done = timeout(WAIT, async {
            while task(dir.path(), &id).state != TaskState::Completed {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(done.is_ok(), "{:?}", task(dir.path(), &id).state);
        assert_file(dir.path(), "f.bin", LEN);
        engine.shutdown().await;
    }
}

/// Réseau qui ne convient pas (SPEC §7.15) : les tâches en cours retournent en file, rien
/// ne démarre ; au retour du réseau, tout reprend et se termine.
mod network {
    use super::*;

    #[tokio::test]
    async fn without_a_usable_network_tasks_wait_then_resume() {
        let dir = tempfile::tempdir().unwrap();
        let len = 2 * 1024 * 1024;
        let server = slow_site(len);
        let engine = Engine::start(config(dir.path())).unwrap();
        let mut events = engine.subscribe();
        let first = engine
            .add(add(&server.url("/a.bin"), dir.path()))
            .await
            .unwrap();
        wait_state(&mut events, &first, TaskState::Active).await;

        engine.set_network(false, false).await.unwrap();
        wait_state(&mut events, &first, TaskState::Queued).await;
        let snapshot = engine.snapshot().await.unwrap();
        assert!(!snapshot.network_usable);
        // Ajoutée sans réseau : elle attend.
        let second = engine
            .add(add(&server.url("/b.bin"), dir.path()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        for id in [&first, &second] {
            assert_eq!(task(dir.path(), id).state, TaskState::Queued, "{id}");
        }
        // Le même état redit : rien à annoncer.
        engine.set_network(false, false).await.unwrap();

        engine.set_network(true, false).await.unwrap();
        let mut left: HashSet<String> = [first.clone(), second.clone()].into();
        let mut announced = 0;
        timeout(WAIT, async {
            while !left.is_empty() {
                match events.recv().await {
                    Ok(Event::StateChanged {
                        task_id,
                        to: TaskState::Completed,
                        ..
                    }) => {
                        left.remove(&task_id);
                    }
                    Ok(Event::NetworkChanged { usable, .. }) => {
                        assert!(usable);
                        announced += 1;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("les deux tâches finissent au retour du réseau");
        assert_eq!(announced, 1, "le retour du réseau est annoncé une fois");
        assert_file(dir.path(), "a.bin", len);
        assert_file(dir.path(), "b.bin", len);
        engine.shutdown().await;
    }
}
