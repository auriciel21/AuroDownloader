//! Téléchargement réparti sur plusieurs sources (SPEC §7.7, phase 6, étape 1b) : partage,
//! bascule quand une source change ou casse, source lente.

use std::sync::Mutex;

use super::*;
use crate::store::open_in_memory;
use crate::store::sources::{self, Checked};
use crate::store::tasks::{NewTask, create_task, ensure_queue};
use crate::testing::{FileSite, Pace, Server, content, serve_file, serve_file_paced};

struct Case {
    conn: Connection,
    task_id: String,
    dir: tempfile::TempDir,
}

fn opts() -> Options {
    Options {
        connections: Some(4),
        min_segment: 16 * 1024,
        buffer: 4096,
        checkpoint_every: Duration::from_millis(20),
        ..Options::default()
    }
}

fn client() -> Client {
    Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap()
}

fn server(len: u64, pace: Option<Pace>) -> (Arc<Mutex<FileSite>>, Server) {
    let site = Arc::new(Mutex::new(FileSite::new(len)));
    let server = serve_file_paced(Arc::clone(&site), pace);
    (site, server)
}

impl Case {
    fn new(url: &str) -> Case {
        let mut conn = open_in_memory().unwrap();
        let queue = ensure_queue(&conn, "default").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (task_id, _) = create_task(
            &mut conn,
            &NewTask {
                queue_id: &queue,
                url,
                display_name: &display_name_for(url),
                dest_dir: dir.path().to_str().unwrap(),
                max_connections: None,
                speed_limit_bps: None,
                idempotency_key: None,
                requested_name: None,
                checksum: None,
                headers: &[],
                parent_id: None,
                public_only: false,
            },
        )
        .unwrap();
        Case { conn, task_id, dir }
    }

    /// Un miroir déjà validé, avec l'`ETag` que son serveur avait alors.
    fn mirror(&mut self, url: &str, len: u64, etag: &str) -> String {
        let id = sources::add_source(&mut self.conn, &self.task_id, url).unwrap();
        let checked = Checked {
            accepts_ranges: true,
            etag: Some(etag),
            last_modified: None,
            content_length: Some(len),
        };
        sources::set_checked(&self.conn, &id, &checked).unwrap();
        id
    }

    async fn run(&mut self) -> Outcome {
        self.run_seen().await.0
    }

    /// Exécute la tâche ; rend aussi les sources vues dans l'avancement, segment par segment.
    async fn run_seen(&mut self) -> (Outcome, std::collections::HashSet<String>) {
        let (_, stop) = watch::channel(false);
        let mut seen = std::collections::HashSet::new();
        let outcome = run(
            &mut self.conn,
            &client(),
            &self.task_id,
            &opts(),
            stop,
            &mut |p| {
                seen.extend(p.segments.iter().map(|s| s.source_id.to_string()));
            },
        )
        .await
        .unwrap();
        (outcome, seen)
    }

    fn sources(&self) -> Vec<SourceRow> {
        load_sources(&self.conn, &self.task_id).unwrap()
    }

    fn file(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.path().join(name)).unwrap()
    }
}

/// Requêtes par plage reçues par un serveur.
fn ranged(server: &Server) -> usize {
    server
        .requests()
        .iter()
        .filter(|r| r.header("range").is_some())
        .count()
}

#[tokio::test]
async fn two_sources_share_the_download() {
    const LEN: u64 = 2 * 1024 * 1024;
    let pace = Some(Pace {
        chunk: 16 * 1024,
        delay: Duration::from_millis(100),
    });
    let (_r, reference) = server(LEN, pace);
    let (_m, mirror) = server(LEN, pace);
    let mut case = Case::new(&reference.url("/f.bin"));
    let mirror_id = case.mirror(&mirror.url("/miroir/f.bin"), LEN, "\"v1\"");
    let (outcome, seen) = case.run_seen().await;
    assert_eq!(outcome, Outcome::Completed(case.dir.path().join("f.bin")));
    assert_eq!(case.file("f.bin"), content(LEN as usize));
    assert!(
        ranged(&reference) > 0 && ranged(&mirror) > 0,
        "les deux servent"
    );
    let sources = case.sources();
    // L'avancement dit quelle source envoie chaque segment (affichage, SPEC §7.7).
    let expected: std::collections::HashSet<String> =
        sources.iter().map(|s| s.id.clone()).collect();
    assert_eq!(
        seen, expected,
        "segments de chaque source dans l'avancement"
    );
    assert!(
        sources.iter().all(|s| s.state == SourceState::Ok),
        "{sources:?}"
    );
    let mirror = sources.iter().find(|s| s.id == mirror_id).unwrap();
    assert!(
        mirror.avg_bps.is_some_and(|b| b > 0),
        "débit gardé : {mirror:?}"
    );
}

#[tokio::test]
async fn a_changed_mirror_is_dropped_alone() {
    const LEN: u64 = 600_000;
    let (_r, reference) = server(LEN, None);
    let (mirror_site, mirror) = server(LEN, None);
    let mut case = Case::new(&reference.url("/f.bin"));
    let mirror_id = case.mirror(&mirror.url("/f.bin"), LEN, "\"v1\"");
    // Le fichier du miroir a changé depuis sa validation.
    mirror_site.lock().unwrap().etag = Some("\"v2\"".into());
    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::Completed(case.dir.path().join("f.bin")));
    assert_eq!(case.file("f.bin"), content(LEN as usize));
    let sources = case.sources();
    let mirror = sources.iter().find(|s| s.id == mirror_id).unwrap();
    assert_eq!(mirror.state, SourceState::Failed, "{mirror:?}");
    assert!(mirror.last_error.is_some(), "{mirror:?}");
    assert_eq!(sources[0].state, SourceState::Ok);
}

#[tokio::test]
async fn a_changed_reference_lets_the_mirror_finish() {
    const LEN: u64 = 600_000;
    let (reference_site, reference) = server(LEN, None);
    let (_m, mirror) = server(LEN, None);
    let mut case = Case::new(&reference.url("/f.bin"));
    // Préparée sur la référence, avec un miroir validé ; puis la référence change.
    case.mirror(&mirror.url("/f.bin"), LEN, "\"v1\"");
    reference_site.lock().unwrap().etag = Some("\"v2\"".into());
    // Une préparation lirait la nouvelle étiquette : la tâche est préparée à la main, avec
    // l'ancienne, comme une tâche commencée avant le changement.
    let source = primary_source(&case.conn, &case.task_id).unwrap();
    let segments = initial_split(LEN, 4, 16 * 1024);
    save_preparation(
        &mut case.conn,
        &case.task_id,
        &Preparation {
            source_id: &source.id,
            accepts_ranges: true,
            etag: Some("\"v1\""),
            last_modified: None,
            total_bytes: Some(LEN),
            file_name: "f.bin",
        },
        &segments,
    )
    .unwrap();
    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::Completed(case.dir.path().join("f.bin")));
    assert_eq!(case.file("f.bin"), content(LEN as usize));
    let sources = case.sources();
    assert_eq!(sources[0].state, SourceState::Failed, "{sources:?}");
    assert_eq!(sources[1].state, SourceState::Ok, "{sources:?}");
}

#[tokio::test]
async fn when_every_source_changed_the_task_pauses() {
    const LEN: u64 = 600_000;
    let (reference_site, reference) = server(LEN, None);
    let (mirror_site, mirror) = server(LEN, None);
    let mut case = Case::new(&reference.url("/f.bin"));
    case.mirror(&mirror.url("/f.bin"), LEN, "\"v1\"");
    let source = primary_source(&case.conn, &case.task_id).unwrap();
    save_preparation(
        &mut case.conn,
        &case.task_id,
        &Preparation {
            source_id: &source.id,
            accepts_ranges: true,
            etag: Some("\"v1\""),
            last_modified: None,
            total_bytes: Some(LEN),
            file_name: "f.bin",
        },
        &initial_split(LEN, 4, 16 * 1024),
    )
    .unwrap();
    reference_site.lock().unwrap().etag = Some("\"v2\"".into());
    mirror_site.lock().unwrap().etag = Some("\"v2\"".into());
    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::FileChanged);
    let task = load_task(&case.conn, &case.task_id).unwrap();
    assert_eq!(task.pause_reason.as_deref(), Some("file_changed"));
}

#[tokio::test]
async fn a_mirror_that_keeps_cutting_rests_and_the_reference_finishes() {
    const LEN: u64 = 600_000;
    let (_r, reference) = server(LEN, None);
    let mirror = serve_file(Arc::new(Mutex::new(FileSite {
        // Chaque réponse s'arrête net après 1 000 octets.
        cut_responses: usize::MAX,
        cut_after: 1000,
        ..FileSite::new(LEN)
    })));
    let mut case = Case::new(&reference.url("/f.bin"));
    let mirror_id = case.mirror(&mirror.url("/f.bin"), LEN, "\"v1\"");
    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::Completed(case.dir.path().join("f.bin")));
    assert_eq!(case.file("f.bin"), content(LEN as usize));
    let sources = case.sources();
    let mirror = sources.iter().find(|s| s.id == mirror_id).unwrap();
    assert_eq!(mirror.state, SourceState::Failed, "au repos : {mirror:?}");
    assert!(mirror.fail_count >= 1, "{mirror:?}");
}

#[tokio::test]
async fn a_slow_mirror_gets_no_new_segments() {
    const LEN: u64 = 12 * 1024 * 1024;
    let (_r, reference) = server(
        LEN,
        Some(Pace {
            chunk: 16 * 1024,
            delay: Duration::from_millis(8),
        }),
    );
    let (_m, mirror) = server(
        LEN,
        Some(Pace {
            chunk: 1024,
            delay: Duration::from_millis(40),
        }),
    );
    let mut case = Case::new(&reference.url("/f.bin"));
    let mirror_id = case.mirror(&mirror.url("/f.bin"), LEN, "\"v1\"");
    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::Completed(case.dir.path().join("f.bin")));
    assert_eq!(case.file("f.bin"), content(LEN as usize));
    let sources = case.sources();
    let slow = sources.iter().find(|s| s.id == mirror_id).unwrap();
    assert_eq!(slow.state, SourceState::Slow, "{slow:?}");
    let fast = &sources[0];
    assert!(
        fast.avg_bps.unwrap_or(0) > 4 * slow.avg_bps.unwrap_or(0),
        "{fast:?} / {slow:?}"
    );
}

#[test]
fn the_best_rate_per_connection_gets_the_next_one() {
    let target = |id: &str| {
        Arc::new(Target::new(
            id.into(),
            String::new(),
            Vec::new(),
            None,
            None,
        ))
    };
    let mut slots = vec![Slot::new(target("a")), Slot::new(target("b"))];
    // Rien de mesuré : la référence d'abord, puis la moins chargée.
    assert_eq!(pick_slot(&slots, 4), Some(0));
    slots[0].connections = 1;
    assert_eq!(pick_slot(&slots, 4), Some(1));
    // Mesurées : le meilleur débit par connexion.
    slots[0].measures = 1;
    slots[0].rate = 10_000.0;
    slots[1].measures = 1;
    slots[1].rate = 1_000.0;
    slots[1].connections = 0;
    assert_eq!(pick_slot(&slots, 4), Some(0));
    // Plein : plus de connexion pour elle.
    slots[0].connections = 4;
    assert_eq!(pick_slot(&slots, 4), Some(1));
    slots[1].state = SlotState::Slow;
    assert_eq!(pick_slot(&slots, 4), None);
}

#[test]
fn a_source_rests_after_three_failures_then_longer_each_time() {
    let target = Arc::new(Target::new(
        "a".into(),
        String::new(),
        Vec::new(),
        None,
        None,
    ));
    let mut slot = Slot::new(target);
    let now = Instant::now();
    assert_eq!(slot.on_failure(false, now), None);
    assert_eq!(slot.on_failure(false, now), None);
    assert_eq!(
        slot.on_failure(false, now),
        Some(SlotState::Resting(now + SOURCE_REST))
    );
    slot.state = SlotState::Usable;
    for _ in 0..3 {
        slot.on_failure(false, now);
    }
    assert_eq!(slot.state, SlotState::Resting(now + SOURCE_REST * 2));
    for _ in 0..30 {
        slot.on_failure(false, now);
    }
    assert!(matches!(slot.state, SlotState::Resting(t) if t <= now + SOURCE_REST_MAX));
    // Un échec définitif l'écarte tout de suite.
    assert_eq!(slot.on_failure(true, now), Some(SlotState::Dropped));
}
