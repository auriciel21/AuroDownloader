//! Téléchargements complets face au serveur de fichier local : segments, flux unique,
//! taille inconnue, coupures et reprise, fichier changé, arrêt demandé, `.part` perdu,
//! arrêt brutal entre le renommage et l'enregistrement de la fin.

use std::collections::HashSet;
use std::sync::Mutex;

use super::*;
use crate::store::open_in_memory;
use crate::store::tasks::{NewTask, create_task, ensure_queue};
use crate::testing::{FileSite, Server, content, serve_file};

const SIZE: usize = 200_000;

fn opts() -> Options {
    Options {
        connections: 4,
        min_segment: 16 * 1024,
        buffer: 4096,
        checkpoint_every: Duration::from_millis(20),
    }
}

fn client() -> Client {
    Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap()
}

struct Case {
    conn: Connection,
    task_id: String,
    dir: tempfile::TempDir,
}

fn new_case(url: &str) -> Case {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_in_memory().unwrap();
    let queue = ensure_queue(&conn, "default").unwrap();
    let (task_id, _) = create_task(
        &mut conn,
        &NewTask {
            queue_id: &queue,
            url,
            display_name: &display_name_for(url),
            dest_dir: dir.path().to_str().unwrap(),
            max_connections: None,
        },
    )
    .unwrap();
    Case { conn, task_id, dir }
}

impl Case {
    async fn run_with(&mut self, stop: watch::Receiver<bool>) -> Outcome {
        run(
            &mut self.conn,
            &client(),
            &self.task_id,
            &opts(),
            stop,
            &mut |_| {},
        )
        .await
        .unwrap()
    }

    async fn run(&mut self) -> Outcome {
        // Émetteur abandonné : aucun arrêt ne sera demandé.
        let (_, stop) = watch::channel(false);
        self.run_with(stop).await
    }

    fn task(&self) -> crate::store::tasks::TaskRow {
        load_task(&self.conn, &self.task_id).unwrap()
    }

    fn count(&self, table: &str) -> i64 {
        self.conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn part(&self, name: &str) -> PathBuf {
        io::part_path(&self.dir.path().join(name))
    }
}

/// Serveur du fichier de test dont `body` est le contenu (toujours `content(n)`).
fn site(body: Vec<u8>) -> (Arc<Mutex<FileSite>>, Server) {
    assert_eq!(body, content(body.len()));
    let site = Arc::new(Mutex::new(FileSite::new(body.len() as u64)));
    let server = serve_file(Arc::clone(&site));
    (site, server)
}

fn assert_completed(case: &Case, outcome: &Outcome, name: &str, data: &[u8]) {
    let path = case.dir.path().join(name);
    assert_eq!(outcome, &Outcome::Completed(path.clone()));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        data,
        "contenu du fichier final"
    );
    assert!(!case.part(name).exists(), ".part renommé");
    let task = case.task();
    assert_eq!(task.state, TaskState::Completed);
    assert_eq!(task.done_bytes, data.len() as u64);
    assert_eq!(case.count("history"), 1);
    assert_eq!(case.count("segments"), 0, "segments purgés (règle 4)");
    assert_eq!(case.count("sources"), 0);
}

#[tokio::test]
async fn segmented_download_completes() {
    let data = content(SIZE);
    let (_, server) = site(data.clone());
    let mut case = new_case(&server.url("/dossier/image.iso"));
    let outcome = case.run().await;
    assert_completed(&case, &outcome, "image.iso", &data);

    let gets: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.method == "GET")
        .collect();
    // Une par segment initial, plus une par coupe d'une connexion finie en avance (F-02).
    assert!(gets.len() >= 4, "{} requêtes", gets.len());
    for r in &gets {
        assert!(r.header("range").is_some());
        assert_eq!(r.header("if-range"), Some("\"v1\""));
    }
}

#[tokio::test]
async fn server_without_ranges_uses_one_stream() {
    let data = content(SIZE);
    let (site, server) = site(data.clone());
    site.lock().unwrap().ranges = false;
    let mut case = new_case(&server.url("/a.bin"));
    let outcome = case.run().await;
    assert_completed(&case, &outcome, "a.bin", &data);
    let gets: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.method == "GET" && r.header("range").is_none())
        .collect();
    assert_eq!(gets.len(), 1);
}

#[tokio::test]
async fn unknown_size_is_streamed_to_the_end() {
    let data = content(SIZE);
    let (site, server) = site(data.clone());
    {
        let mut s = site.lock().unwrap();
        s.ranges = false;
        s.send_length = false;
    }
    let mut case = new_case(&server.url("/flux"));
    assert_eq!(case.task().total_bytes, None);
    let outcome = case.run().await;
    assert_completed(&case, &outcome, "flux", &data);
    assert_eq!(case.task().total_bytes, Some(SIZE as u64));
}

#[tokio::test]
async fn empty_file_completes() {
    let (_, server) = site(Vec::new());
    let mut case = new_case(&server.url("/vide.txt"));
    let outcome = case.run().await;
    assert_completed(&case, &outcome, "vide.txt", &[]);
}

#[tokio::test]
async fn existing_file_is_never_overwritten() {
    let data = content(1000);
    let (_, server) = site(data.clone());
    let mut case = new_case(&server.url("/a.bin"));
    std::fs::write(case.dir.path().join("a.bin"), b"ancien").unwrap();
    let outcome = case.run().await;
    assert_completed(&case, &outcome, "a (1).bin", &data);
    assert_eq!(
        std::fs::read(case.dir.path().join("a.bin")).unwrap(),
        b"ancien"
    );
}

/// Coupe les quatre premières connexions après 20 000 octets : la tâche attend une
/// nouvelle tentative, avec ses compteurs enregistrés.
async fn interrupted(data: &[u8]) -> (Arc<Mutex<FileSite>>, Server, Case) {
    let (site, server) = site(data.to_vec());
    {
        let mut s = site.lock().unwrap();
        s.cut_responses = 4;
        s.cut_after = 20_000;
    }
    let mut case = new_case(&server.url("/gros.bin"));
    let outcome = case.run().await;
    assert!(
        matches!(
            outcome,
            Outcome::RetryLater {
                code: "NET_UNREACHABLE",
                ..
            }
        ),
        "{outcome:?}"
    );
    let task = case.task();
    assert_eq!(task.state, TaskState::RetryWait);
    assert_eq!(task.retry_count, 1);
    (site, server, case)
}

#[tokio::test]
async fn cut_connections_resume_where_they_stopped() {
    let data = content(SIZE);
    let (_, server, mut case) = interrupted(&data).await;

    // Règle 1 : ce que la base annonce est dans le .part, à sa place.
    let segments = load_segments(&case.conn, &case.task_id).unwrap();
    let saved: u64 = segments.iter().map(|s| s.written).sum();
    assert!(
        saved > 0,
        "des octets reçus avant la coupure sont enregistrés"
    );
    assert_eq!(case.task().done_bytes, saved);
    let part = std::fs::read(case.part("gros.bin")).unwrap();
    for s in segments.iter().filter(|s| s.written > 0) {
        let (from, to) = (s.start as usize, (s.start + s.written) as usize);
        assert_eq!(part[from..to], data[from..to], "segment {}", s.idx);
    }

    let before = server.requests().len();
    let outcome = case.run().await;
    assert_completed(&case, &outcome, "gros.bin", &data);
    // La reprise ne redemande pas ce qui était enregistré.
    let resumed: Vec<String> = server.requests()[before..]
        .iter()
        .filter_map(|r| r.header("range").map(str::to_owned))
        .collect();
    for s in segments.iter().filter(|s| s.written > 0) {
        let expected = format!("bytes={}-{}", s.start + s.written, s.end);
        assert!(
            resumed.contains(&expected),
            "{expected} absent de {resumed:?}"
        );
    }
}

#[tokio::test]
async fn changed_file_pauses_the_task() {
    let data = content(SIZE);
    let (site, _server, mut case) = interrupted(&data).await;
    site.lock().unwrap().etag = Some("\"v2\"".into());
    let outcome = case.run().await;
    assert_eq!(outcome, Outcome::FileChanged);
    let task = case.task();
    assert_eq!(task.state, TaskState::Paused);
    assert_eq!(task.pause_reason.as_deref(), Some("file_changed"));
    assert_eq!(task.error_code.as_deref(), Some("FILE_CHANGED"));
}

#[tokio::test]
async fn lost_part_file_restarts_the_written_segments() {
    let data = content(SIZE);
    let (_, _server, mut case) = interrupted(&data).await;
    std::fs::remove_file(case.part("gros.bin")).unwrap();
    let outcome = case.run().await;
    assert_completed(&case, &outcome, "gros.bin", &data);
}

#[tokio::test]
async fn truncated_part_file_restarts_the_missing_segments() {
    let data = content(SIZE);
    let (_, _server, mut case) = interrupted(&data).await;
    std::fs::OpenOptions::new()
        .write(true)
        .open(case.part("gros.bin"))
        .unwrap()
        .set_len(10)
        .unwrap();
    let outcome = case.run().await;
    assert_completed(&case, &outcome, "gros.bin", &data);
}

#[tokio::test]
async fn stop_request_pauses_then_the_task_resumes() {
    let data = content(SIZE);
    let (_, server) = site(data.clone());
    let mut case = new_case(&server.url("/a.bin"));
    let (stop_tx, stop) = watch::channel(true);
    let outcome = case.run_with(stop).await;
    drop(stop_tx);
    assert_eq!(outcome, Outcome::Paused);
    let task = case.task();
    assert_eq!(task.state, TaskState::Paused);
    assert_eq!(task.pause_reason.as_deref(), Some("user"));

    // Une pause ne s'exécute pas : elle repasse d'abord par la file.
    let (_, stop) = watch::channel(false);
    assert!(matches!(
        run(
            &mut case.conn,
            &client(),
            &case.task_id,
            &opts(),
            stop,
            &mut |_| {}
        )
        .await,
        Err(TransferError::NotRunnable { .. })
    ));
    requeue(&mut case.conn, &case.task_id).unwrap();
    let outcome = case.run().await;
    assert_completed(&case, &outcome, "a.bin", &data);
}

/// Arrêt brutal après le renommage du `.part`, avant l'enregistrement de la fin : la
/// reprise constate le fichier complet et termine la tâche sans réseau.
#[tokio::test]
async fn crash_after_rename_is_completed_without_network() {
    let data = content(5000);
    // Port fermé : toute requête échouerait.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut case = new_case(&format!("http://127.0.0.1:{port}/f.bin"));
    let source = primary_source(&case.conn, &case.task_id).unwrap();
    let segments = initial_split(5000, 2, 1);
    let prep = Preparation {
        source_id: &source.id,
        accepts_ranges: true,
        etag: Some("\"v1\""),
        last_modified: None,
        total_bytes: Some(5000),
        file_name: "f.bin",
    };
    save_preparation(&mut case.conn, &case.task_id, &prep, &segments).unwrap();
    change_state(
        &mut case.conn,
        &case.task_id,
        &Change::to(TaskState::Active),
    )
    .unwrap();

    let dest = case.dir.path().join("f.bin");
    let part = PartFile::create(&dest).unwrap();
    let progress: Vec<_> = segments
        .iter()
        .map(|s| {
            let mut w = part.writer(s, 1024);
            w.write(&data[s.start as usize..=s.end as usize]).unwrap();
            w.finish().unwrap()
        })
        .collect();
    save_progress(&mut case.conn, &case.task_id, &progress).unwrap();
    part.finish().unwrap();

    let outcome = case.run().await;
    assert_completed(&case, &outcome, "f.bin", &data);
}

#[tokio::test]
async fn failures_are_retried_then_abandoned() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut case = new_case(&format!("http://127.0.0.1:{port}/f.bin"));
    let before = now_ms();
    let Outcome::RetryLater { code, at_ms, .. } = case.run().await else {
        panic!("nouvelle tentative attendue");
    };
    assert_eq!(code, "NET_UNREACHABLE");
    assert!(
        (before + 2000..=now_ms() + 3000).contains(&at_ms),
        "{at_ms}"
    );

    case.conn
        .execute(
            "UPDATE tasks SET retry_count = ?2 WHERE id = ?1",
            rusqlite::params![case.task_id, MAX_FAILURES - 1],
        )
        .unwrap();
    let outcome = case.run().await;
    assert!(
        matches!(
            outcome,
            Outcome::Failed {
                code: "NET_UNREACHABLE",
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(case.task().state, TaskState::Failed);
}

#[test]
fn retry_delays_double_up_to_the_cap() {
    let none = Duration::ZERO;
    assert_eq!(retry_delay(0, None, none), Duration::from_secs(2));
    assert_eq!(retry_delay(3, None, none), Duration::from_secs(16));
    assert_eq!(retry_delay(8, None, none), RETRY_CAP);
    assert_eq!(retry_delay(40, None, none), RETRY_CAP);
    assert_eq!(
        retry_delay(1, None, Duration::from_millis(300)),
        Duration::from_millis(4300)
    );
    let asked = Some(Duration::from_secs(90));
    assert_eq!(
        retry_delay(5, asked, Duration::from_millis(300)),
        Duration::from_secs(90)
    );
}

#[test]
fn display_names_before_preparation() {
    assert_eq!(display_name_for("https://h/a/b%C3%A9.iso?x=1"), "bé.iso");
    assert_eq!(display_name_for("https://exemple.org/"), "exemple.org");
    assert_eq!(display_name_for("pas une url"), "download");
}

/// Prépare la tâche comme si elle avait été lancée avec une seule connexion : un seul
/// segment couvre tout le fichier.
fn prepare_single_segment(case: &mut Case, len: u64) {
    let source = primary_source(&case.conn, &case.task_id).unwrap();
    let prep = Preparation {
        source_id: &source.id,
        accepts_ranges: true,
        etag: Some("\"v1\""),
        last_modified: None,
        total_bytes: Some(len),
        file_name: "gros.bin",
    };
    let single = initial_split(len, 1, 1);
    save_preparation(&mut case.conn, &case.task_id, &prep, &single).unwrap();
}

/// F-02 : les connexions libres coupent le segment en cours au lieu de rester oisives.
#[tokio::test]
async fn idle_connections_split_the_running_segment() {
    let data = content(SIZE);
    let (_, server) = site(data.clone());
    let mut case = new_case(&server.url("/gros.bin"));
    prepare_single_segment(&mut case, SIZE as u64);

    let outcome = case.run().await;
    assert_completed(&case, &outcome, "gros.bin", &data);
    let starts: HashSet<String> = server
        .requests()
        .iter()
        .filter_map(|r| r.header("range").map(str::to_owned))
        .collect();
    assert!(
        starts.len() >= 4,
        "au moins quatre plages distinctes : {starts:?}"
    );
}

/// Arrêt demandé juste après les coupes : la base garde un découpage couvrant, que la
/// reprise termine.
#[tokio::test]
async fn split_layout_survives_a_stop_and_resumes() {
    let data = content(SIZE);
    let (_, server) = site(data.clone());
    let mut case = new_case(&server.url("/gros.bin"));
    prepare_single_segment(&mut case, SIZE as u64);

    let (stop_tx, stop) = watch::channel(true);
    assert_eq!(case.run_with(stop).await, Outcome::Paused);
    drop(stop_tx);
    let segments = load_segments(&case.conn, &case.task_id).unwrap();
    assert!(
        segments.len() > 1,
        "les coupes sont enregistrées : {segments:?}"
    );
    check_task_coverage(&case.conn, &case.task_id).unwrap();

    requeue(&mut case.conn, &case.task_id).unwrap();
    let outcome = case.run().await;
    assert_completed(&case, &outcome, "gros.bin", &data);
}
