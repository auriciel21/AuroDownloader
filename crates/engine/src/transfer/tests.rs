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

/// Quatre connexions imposées : les tests restent déterministes.
fn opts() -> Options {
    Options {
        connections: Some(4),
        min_segment: 16 * 1024,
        buffer: 4096,
        checkpoint_every: Duration::from_millis(20),
        ..Options::default()
    }
}

/// Nombre de connexions automatique, mesuré souvent.
fn auto_opts() -> Options {
    Options {
        connections: None,
        measure_every: Duration::from_millis(100),
        ..opts()
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
    new_case_in(url, tempfile::tempdir().unwrap())
}

/// Tâche dont la destination est `dir`.
fn new_case_in(url: &str, dir: tempfile::TempDir) -> Case {
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
            speed_limit_bps: None,
            idempotency_key: None,
            requested_name: None,
            checksum: None,
            headers: &[],
            parent_id: None,
        },
    )
    .unwrap();
    Case { conn, task_id, dir }
}

impl Case {
    async fn run_with(&mut self, stop: watch::Receiver<bool>) -> Outcome {
        self.run_opts(&opts(), stop).await
    }

    async fn run_opts(&mut self, opts: &Options, stop: watch::Receiver<bool>) -> Outcome {
        run(
            &mut self.conn,
            &client(),
            &self.task_id,
            opts,
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
    assert_eq!(
        case.count("sources"),
        1,
        "adresse gardée (retéléchargement)"
    );
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

mod auto {
    use super::*;
    use crate::testing::{Pace, serve_file_paced};

    #[test]
    fn doubles_while_the_rate_improves_then_stops() {
        let mut a = AutoConnections::new(2, 16);
        assert_eq!(a.on_rate(0.0), 2, "débit nul : rien à conclure");
        assert_eq!(a.on_rate(100.0), 4, "première mesure : un essai");
        assert_eq!(a.on_rate(111.0), 8, "+11 % : on double encore");
        assert_eq!(a.on_rate(115.0), 8, "+3,6 % : on s'arrête");
        assert!(a.is_frozen());
        assert_eq!(
            a.on_rate(1000.0),
            8,
            "plus d'augmentation pour cette exécution"
        );
    }

    #[test]
    fn reaches_the_cap_in_two_windows_from_the_default_start() {
        let mut a = AutoConnections::new(AUTO_START_CONNECTIONS, AUTO_MAX_CONNECTIONS);
        assert_eq!(a.on_rate(100.0), 8);
        assert_eq!(a.on_rate(180.0), 16);
        assert_eq!(a.on_rate(400.0), 16, "au plafond, sans gel");
        assert!(!a.is_frozen());
    }

    #[test]
    fn never_exceeds_its_cap() {
        let mut a = AutoConnections::new(4, 6);
        let mut rate = 100.0;
        for _ in 0..10 {
            rate *= 2.0;
            a.on_rate(rate);
        }
        assert_eq!(a.width(), 6);
        assert_eq!(
            AutoConnections::new(4, 2).width(),
            2,
            "départ sous le plafond"
        );
    }

    #[test]
    fn overload_keeps_the_connections_still_alive() {
        let mut a = AutoConnections::new(8, 16);
        a.on_rate(100.0);
        assert_eq!(a.width(), 16);
        // Salve de refus : 15, puis 14… encore actives, jusqu'aux 8 que le serveur garde.
        for alive in (8..=15).rev() {
            assert_eq!(a.on_overload(alive), alive);
        }
        assert_eq!(a.width(), 8, "ce que le serveur accepte, pas 1");
        assert!(a.is_frozen());
        // Jamais au-dessus de la cible déjà retenue, jamais moins d'une connexion.
        assert_eq!(a.on_overload(12), 8);
        assert_eq!(a.on_overload(0), 1);
        assert_eq!(a.width(), 1);
    }

    #[test]
    fn host_cap_lowers_the_automatic_cap() {
        let client = client();
        let opts = Options::default();
        assert_eq!(auto_cap(&client, "http://lent.example/f", &opts), 16);
        client.limit_host("lent.example", 3);
        client.limit_host("lent.example", 7);
        assert_eq!(auto_cap(&client, "http://lent.example/f", &opts), 3);
        assert_eq!(auto_cap(&client, "http://autre.example/f", &opts), 16);
    }

    /// Serveur bridé par connexion : plus de connexions, plus de débit.
    fn paced_site(len: u64) -> (Arc<Mutex<FileSite>>, Server) {
        let site = Arc::new(Mutex::new(FileSite::new(len)));
        let pace = Pace {
            chunk: 16 * 1024,
            delay: Duration::from_millis(20),
        };
        let server = serve_file_paced(Arc::clone(&site), Some(pace));
        (site, server)
    }

    async fn run_auto(case: &mut Case, client: &Client) -> Outcome {
        let (_, stop) = watch::channel(false);
        run(
            &mut case.conn,
            client,
            &case.task_id,
            &auto_opts(),
            stop,
            &mut |_| {},
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn automatic_mode_adds_connections_when_it_pays() {
        let len = 4 * 1024 * 1024;
        let (_, server) = paced_site(len);
        let mut case = new_case(&server.url("/gros.bin"));
        let outcome = run_auto(&mut case, &client()).await;
        assert_completed(&case, &outcome, "gros.bin", &content(len as usize));
        assert!(
            server.peak_connections() > 4,
            "au plus {} connexions simultanées",
            server.peak_connections()
        );
    }

    /// SPEC §7.3 : sur un 503, moitié moins de connexions et un plafond pour l'hôte ; le
    /// téléchargement continue au lieu d'échouer.
    #[tokio::test]
    async fn busy_server_halves_connections_and_the_download_goes_on() {
        let len = 2 * 1024 * 1024;
        let (site, server) = paced_site(len);
        site.lock().unwrap().busy_responses = 1;
        let mut case = new_case(&server.url("/gros.bin"));
        let client = client();
        let outcome = run_auto(&mut case, &client).await;
        assert_completed(&case, &outcome, "gros.bin", &content(len as usize));
        assert_eq!(
            client.host_cap("127.0.0.1"),
            Some(3),
            "4 atteintes, moins une"
        );
    }

    /// Serveur qui plafonne les connexions par client (6 ici) : la montée automatique
    /// (4, 8, 16) se heurte à une salve de refus. Le moteur garde les connexions que le
    /// serveur accepte, au lieu de diviser par deux à chaque refus de la salve (16, 8, 4,
    /// 2, 1 : la fin du téléchargement sur une seule connexion, constaté le 2026-11-07).
    #[tokio::test]
    async fn a_burst_of_refusals_keeps_the_connections_the_server_accepts() {
        let len = 8 * 1024 * 1024;
        let (site, server) = paced_site(len);
        site.lock().unwrap().max_concurrent = Some(6);
        let mut case = new_case(&server.url("/plafond.bin"));
        let client = client();
        let outcome = run_auto(&mut case, &client).await;
        assert_completed(&case, &outcome, "plafond.bin", &content(len as usize));
        let cap = client.host_cap("127.0.0.1").expect("refus constatés");
        assert!(
            (5..=6).contains(&cap),
            "plafond retenu : {cap} connexions, le serveur en accepte 6"
        );
    }

    #[tokio::test]
    async fn imposed_connections_do_not_adapt() {
        let len = 2 * 1024 * 1024;
        let (site, server) = paced_site(len);
        site.lock().unwrap().busy_responses = 1;
        let mut case = new_case(&server.url("/gros.bin"));
        let outcome = case.run().await;
        assert!(
            matches!(
                outcome,
                Outcome::RetryLater {
                    code: "HTTP_STATUS",
                    ..
                }
            ),
            "{outcome:?}"
        );
    }
}

/// Limites de débit (SPEC §7.6) et espace disque (F-18).
mod limits_and_space {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::limit::{Bucket, Throttle};
    use crate::testing::{Pace, same_as_served, serve_file_paced};

    fn no_stop() -> watch::Receiver<bool> {
        watch::channel(false).1
    }

    #[tokio::test]
    async fn a_speed_limit_holds_the_rate() {
        let len = 1_500_000;
        let (_, server) = site(content(len));
        let mut case = new_case(&server.url("/lent.bin"));
        let opts = Options {
            throttle: Throttle::new([Bucket::new(Some(500_000))]),
            ..opts()
        };
        let start = std::time::Instant::now();
        let outcome = case.run_opts(&opts, no_stop()).await;
        let elapsed = start.elapsed();
        assert_completed(&case, &outcome, "lent.bin", &content(len));
        // Trois secondes à ce débit, moins ce que chaque connexion lit d'avance.
        assert!(elapsed >= Duration::from_secs(2), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(8), "{elapsed:?}");
    }

    /// Une connexion qui attend son tour de débit ne lit pas : cette attente ne compte pas
    /// dans le délai de lecture. Avant le 2026-11-07, le délai de reqwest courait aussi
    /// entre deux lectures ; sous une limite basse, les tâches finissaient en
    /// `NET_TIMEOUT` sur un serveur qui répondait pourtant aussitôt.
    #[tokio::test]
    async fn waiting_for_the_speed_limit_is_not_a_read_timeout() {
        let len = 400_000;
        let (_, server) = site(content(len));
        let mut case = new_case(&server.url("/attente.bin"));
        let opts = Options {
            // 4 s à ce débit ; chaque morceau reçu fait attendre bien plus que le délai.
            throttle: Throttle::new([Bucket::new(Some(100_000))]),
            ..opts()
        };
        let impatient =
            Client::with_timeouts(Duration::from_secs(5), Duration::from_millis(200)).unwrap();
        let outcome = run(
            &mut case.conn,
            &impatient,
            &case.task_id,
            &opts,
            no_stop(),
            &mut |_| {},
        )
        .await
        .unwrap();
        assert_completed(&case, &outcome, "attente.bin", &content(len));
    }

    #[tokio::test]
    async fn missing_space_pauses_the_task_before_it_starts() {
        fn almost_full(_: &Path) -> std::io::Result<u64> {
            Ok(1024)
        }
        let (_, server) = site(content(SIZE));
        let mut case = new_case(&server.url("/plein.bin"));
        let opts = Options {
            free_space: almost_full,
            ..opts()
        };
        assert_eq!(case.run_opts(&opts, no_stop()).await, Outcome::DiskFull);
        let task = case.task();
        assert_eq!(task.state, TaskState::Paused);
        assert_eq!(task.pause_reason.as_deref(), Some("disk_full"));
        assert_eq!(task.error_code.as_deref(), Some("DISK_FULL"));
        assert!(
            task.error_detail
                .as_deref()
                .is_some_and(|d| d.contains("espace disque insuffisant")),
            "{:?}",
            task.error_detail
        );
        // Aucun segment demandé : seule la préparation a eu lieu.
        assert!(
            server
                .requests()
                .iter()
                .all(|r| r.header("range").is_none_or(|v| v == "bytes=0-0")),
            "segments demandés malgré le manque de place"
        );

        // La place revient : la tâche repart et finit.
        requeue(&mut case.conn, &case.task_id).unwrap();
        let outcome = case.run().await;
        assert_completed(&case, &outcome, "plein.bin", &content(SIZE));
    }

    #[tokio::test]
    async fn a_disk_filling_up_stops_the_running_task_cleanly() {
        static FREE: AtomicU64 = AtomicU64::new(u64::MAX);
        fn free(_: &Path) -> std::io::Result<u64> {
            Ok(FREE.load(Ordering::SeqCst))
        }
        let len: usize = 4 * 1024 * 1024;
        let pace = Pace {
            chunk: 16 * 1024,
            delay: Duration::from_millis(20),
        };
        let site = Arc::new(Mutex::new(FileSite::new(len as u64)));
        let server = serve_file_paced(site, Some(pace));
        let mut case = new_case(&server.url("/rempli.bin"));
        let opts = Options {
            free_space: free,
            keep_free: 1000,
            ..opts()
        };
        let (outcome, ()) = tokio::join!(case.run_opts(&opts, no_stop()), async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            FREE.store(0, Ordering::SeqCst);
        });
        assert_eq!(outcome, Outcome::DiskFull);
        assert_eq!(case.task().pause_reason.as_deref(), Some("disk_full"));

        // Rien de perdu : chaque octet compté en base est sur disque, exact.
        let part = case.part("rempli.bin");
        let segments = crate::store::segments::load_segments(&case.conn, &case.task_id).unwrap();
        let written: u64 = segments.iter().map(|s| s.written).sum();
        assert!(written > 0 && written < len as u64, "{written} octets");
        for s in &segments {
            same_as_served(&part, s.start, s.written).unwrap();
        }

        FREE.store(u64::MAX, Ordering::SeqCst);
        requeue(&mut case.conn, &case.task_id).unwrap();
        let outcome = case.run_opts(&opts, no_stop()).await;
        assert_completed(&case, &outcome, "rempli.bin", &content(len));
    }

    /// Les deux essais du disque vraiment plein partagent le même petit volume, que chacun
    /// remplit : ils passent l'un après l'autre.
    static FULL_DISK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Disque vraiment plein : l'écriture échoue (`ENOSPC`), sans contrôle préalable de
    /// la place. Demande un petit volume dédié, désigné par `AURO_FULL_DISK_DIR` ; sous
    /// Linux, un `tmpfs` de 8 Mio monté sans droits d'administrateur :
    /// `unshare -rm sh -c 'mount -t tmpfs -o size=8m tmpfs "$D" && AURO_FULL_DISK_DIR="$D"
    /// cargo test -p auro-engine --lib -- --ignored really_full'`.
    #[tokio::test]
    #[ignore = "demande un petit volume dédié (AURO_FULL_DISK_DIR)"]
    async fn a_really_full_disk_loses_nothing() {
        let _alone = FULL_DISK.lock().await;
        fn plenty(_: &Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        let volume = PathBuf::from(
            std::env::var("AURO_FULL_DISK_DIR").expect("AURO_FULL_DISK_DIR : petit volume dédié"),
        );
        // Un fichier de remplissage, que l'on supprimera pour libérer la place ; le
        // téléchargement dépasse d'un mébioctet la place qu'il laisse.
        let filler = volume.join("remplissage");
        std::fs::write(&filler, vec![0u8; 2 * 1024 * 1024]).unwrap();
        let free = crate::io::free_space(&volume).unwrap();
        let len = usize::try_from(free).unwrap() + 1024 * 1024;
        let (_, server) = site(content(len));
        let mut case = new_case_in(
            &server.url("/trop.bin"),
            tempfile::tempdir_in(&volume).unwrap(),
        );
        // Sans réservation (F-10) : l'échec vient de l'écriture.
        let opts = Options {
            free_space: plenty,
            keep_free: 0,
            preallocate: false,
            ..opts()
        };
        assert_eq!(case.run_opts(&opts, no_stop()).await, Outcome::DiskFull);
        let task = case.task();
        assert_eq!(task.pause_reason.as_deref(), Some("disk_full"));
        assert_eq!(task.error_code.as_deref(), Some("DISK_FULL"));
        let part = case.part("trop.bin");
        let segments = crate::store::segments::load_segments(&case.conn, &case.task_id).unwrap();
        for s in &segments {
            same_as_served(&part, s.start, s.written).unwrap();
        }
        let written: u64 = segments.iter().map(|s| s.written).sum();
        eprintln!("disque plein après {written} octets enregistrés sur {len}");

        std::fs::remove_file(&filler).unwrap();
        requeue(&mut case.conn, &case.task_id).unwrap();
        let outcome = case.run_opts(&opts, no_stop()).await;
        assert_completed(&case, &outcome, "trop.bin", &content(len));
    }

    /// Même disque vraiment plein, avec la réservation de la place (F-10) : le manque se
    /// voit avant le premier octet reçu. Même volume dédié que le test précédent.
    #[tokio::test]
    #[ignore = "demande un petit volume dédié (AURO_FULL_DISK_DIR)"]
    async fn a_really_full_disk_is_seen_before_downloading() {
        let _alone = FULL_DISK.lock().await;
        fn plenty(_: &Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        let volume = PathBuf::from(
            std::env::var("AURO_FULL_DISK_DIR").expect("AURO_FULL_DISK_DIR : petit volume dédié"),
        );
        let filler = volume.join("remplissage-reserve");
        std::fs::write(&filler, vec![0u8; 2 * 1024 * 1024]).unwrap();
        let free = crate::io::free_space(&volume).unwrap();
        let len = usize::try_from(free).unwrap() + 1024 * 1024;
        let (_, server) = site(content(len));
        let mut case = new_case_in(
            &server.url("/trop.bin"),
            tempfile::tempdir_in(&volume).unwrap(),
        );
        let opts = Options {
            free_space: plenty,
            keep_free: 0,
            ..opts()
        };
        assert_eq!(case.run_opts(&opts, no_stop()).await, Outcome::DiskFull);
        assert_eq!(case.task().error_code.as_deref(), Some("DISK_FULL"));
        let segments = crate::store::segments::load_segments(&case.conn, &case.task_id).unwrap();
        assert_eq!(segments.iter().map(|s| s.written).sum::<u64>(), 0);
        assert!(
            server
                .requests()
                .iter()
                .all(|r| r.method != "GET" || r.header("range").is_none_or(|v| v == "bytes=0-0")),
            "segments demandés malgré le disque plein"
        );

        std::fs::remove_file(&filler).unwrap();
        requeue(&mut case.conn, &case.task_id).unwrap();
        let outcome = case.run_opts(&opts, no_stop()).await;
        assert_completed(&case, &outcome, "trop.bin", &content(len));
    }

    /// Réservation de la place (F-10) : le `.part` a sa taille finale avant le premier
    /// octet reçu ; sans réservation, rien n'est pris d'avance.
    #[tokio::test]
    async fn the_part_file_is_reserved_before_the_first_byte() {
        let (_, server) = site(content(SIZE));
        for preallocate in [true, false] {
            let mut case = new_case(&server.url("/reserve.bin"));
            let opts = Options {
                preallocate,
                ..opts()
            };
            // Arrêt demandé dès le départ : la préparation et la réservation ont lieu,
            // aucun segment n'est reçu.
            let (stop_tx, stop) = watch::channel(true);
            assert_eq!(case.run_opts(&opts, stop).await, Outcome::Paused);
            drop(stop_tx);
            let len = std::fs::metadata(case.part("reserve.bin")).unwrap().len();
            let expected = if preallocate { SIZE as u64 } else { 0 };
            assert_eq!(len, expected, "réservation {preallocate}");
            requeue(&mut case.conn, &case.task_id).unwrap();
            let outcome = case.run_opts(&opts, no_stop()).await;
            assert_completed(&case, &outcome, "reserve.bin", &content(SIZE));
        }
    }
}

/// Points de contrôle par volume (SPEC §7.5) : un téléchargement rapide enregistre ce
/// qu'il reçoit sans attendre l'intervalle.
mod checkpoints {
    use super::*;
    use crate::testing::{Pace, same_as_served, serve_file_paced};

    /// Arrêt brutal, à 600 ms, d'un téléchargement qui en dure plus de 1 300 (4 Mio, 4
    /// connexions à 800 Kio/s), points de contrôle par intervalle hors d'atteinte. Renvoie
    /// les octets enregistrés, après avoir vérifié que chacun est sur disque, exact.
    async fn saved_before_a_crash(checkpoint_bytes: u64) -> u64 {
        let len: u64 = 4 * 1024 * 1024;
        let pace = Pace {
            chunk: 16 * 1024,
            delay: Duration::from_millis(20),
        };
        let server = serve_file_paced(Arc::new(Mutex::new(FileSite::new(len))), Some(pace));
        let mut case = new_case(&server.url("/rapide.bin"));
        let opts = Options {
            checkpoint_every: Duration::from_secs(3600),
            checkpoint_bytes,
            ..opts()
        };
        let (_, stop) = watch::channel(false);
        let run = case.run_opts(&opts, stop);
        let crashed = tokio::time::timeout(Duration::from_millis(600), run).await;
        assert!(crashed.is_err(), "téléchargement fini avant l'arrêt");

        let part = case.part("rapide.bin");
        let segments = crate::store::segments::load_segments(&case.conn, &case.task_id).unwrap();
        for s in &segments {
            same_as_served(&part, s.start, s.written).unwrap();
        }
        segments.iter().map(|s| s.written).sum()
    }

    #[tokio::test]
    async fn a_fast_download_checkpoints_by_volume() {
        let saved = saved_before_a_crash(256 * 1024).await;
        assert!(saved >= 256 * 1024, "{saved} octets enregistrés");
    }

    #[tokio::test]
    async fn without_a_volume_threshold_only_the_interval_checkpoints() {
        assert_eq!(saved_before_a_crash(u64::MAX).await, 0);
    }
}

/// Deux téléchargements du même nom ne partagent jamais un `.part` (SPEC §7.1, étape 3).
mod names {
    use super::*;
    use crate::store::open;

    fn add_task(conn: &mut Connection, url: &str, dir: &Path) -> String {
        let queue = ensure_queue(conn, "default").unwrap();
        let (task_id, _) = create_task(
            conn,
            &NewTask {
                queue_id: &queue,
                url,
                display_name: &display_name_for(url),
                dest_dir: dir.to_str().unwrap(),
                max_connections: None,
                speed_limit_bps: None,
                idempotency_key: None,
                requested_name: None,
                checksum: None,
                headers: &[],
                parent_id: None,
            },
        )
        .unwrap();
        task_id
    }

    async fn run_task(conn: &mut Connection, task_id: &str) -> Outcome {
        let (_, stop) = watch::channel(false);
        run(conn, &client(), task_id, &opts(), stop, &mut |_| {})
            .await
            .unwrap()
    }

    /// Une tâche préparée dont le `.part` manque (perdu, ou arrêt avant sa création)
    /// garde son nom : une nouvelle tâche du même nom en prend un autre, et chacune finit
    /// avec son propre contenu.
    #[tokio::test]
    async fn a_name_recorded_without_its_part_file_stays_reserved() {
        let (first, second) = (content(150_000), content(90_000));
        let (_, server_a) = site(first.clone());
        let (_, server_b) = site(second.clone());
        let mut case = new_case(&server_a.url("/a.bin"));
        let task_b = add_task(&mut case.conn, &server_b.url("/a.bin"), case.dir.path());
        let source = primary_source(&case.conn, &task_b).unwrap();
        let prep = Preparation {
            source_id: &source.id,
            accepts_ranges: true,
            etag: None,
            last_modified: None,
            total_bytes: Some(second.len() as u64),
            file_name: "a.bin",
        };
        let single = initial_split(second.len() as u64, 1, 1);
        save_preparation(&mut case.conn, &task_b, &prep, &single).unwrap();

        let path_a = case.dir.path().join("a (1).bin");
        assert_eq!(case.run().await, Outcome::Completed(path_a.clone()));
        assert_eq!(std::fs::read(&path_a).unwrap(), first);

        let path_b = case.dir.path().join("a.bin");
        let outcome = run_task(&mut case.conn, &task_b).await;
        assert_eq!(outcome, Outcome::Completed(path_b.clone()));
        assert_eq!(std::fs::read(&path_b).unwrap(), second);
    }

    /// Seize tâches du même nom, de contenus différents, préparées en même temps sur des
    /// fils différents : chacune obtient son nom et finit avec son propre contenu.
    #[test]
    fn simultaneous_downloads_of_the_same_name_keep_their_own_files() {
        const N: usize = 16;
        let base = tempfile::tempdir().unwrap();
        let dl = tempfile::tempdir().unwrap();
        let db = base.path().join("auro.db");
        let bodies: Vec<Vec<u8>> = (0..N).map(|i| content(40_000 + 7_000 * i)).collect();
        let servers: Vec<_> = bodies.iter().map(|b| site(b.clone())).collect();
        let mut conn = open(&db).unwrap();
        let ids: Vec<String> = servers
            .iter()
            .map(|(_, s)| add_task(&mut conn, &s.url("/c.bin"), dl.path()))
            .collect();
        // Un fil et un environnement d'exécution par tâche, comme dans le moteur où
        // plusieurs tâches se préparent en parallèle.
        let start = std::sync::Barrier::new(N);
        let outcomes: Vec<Outcome> = std::thread::scope(|s| {
            let handles: Vec<_> = ids
                .iter()
                .map(|id| {
                    let (db, start) = (&db, &start);
                    s.spawn(move || {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap();
                        let mut conn = open(db).unwrap();
                        start.wait();
                        runtime.block_on(run_task(&mut conn, id))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut paths = HashSet::new();
        for (i, outcome) in outcomes.into_iter().enumerate() {
            let Outcome::Completed(path) = outcome else {
                panic!("tâche {i} non terminée : {outcome:?}");
            };
            assert_eq!(
                std::fs::read(&path).unwrap(),
                bodies[i],
                "contenu de {path:?}"
            );
            assert!(paths.insert(path.clone()), "{path:?} partagé");
        }
    }
}

/// Un secret qui n'est plus dans le coffre (coffre en mémoire, moteur redémarré) : la
/// tâche part sans cet en-tête, avec les autres.
#[tokio::test]
async fn a_secret_missing_from_the_vault_is_left_out() {
    let (_, server) = site(content(SIZE));
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_in_memory().unwrap();
    let queue = ensure_queue(&conn, "default").unwrap();
    let url = server.url("/s.bin");
    let stored = [
        ("Cookie".to_owned(), Stored::Secret("disparu".into())),
        (
            "Referer".to_owned(),
            Stored::Plain("https://exemple.org/".into()),
        ),
    ];
    let (task_id, _) = create_task(
        &mut conn,
        &NewTask {
            queue_id: &queue,
            url: &url,
            display_name: "s.bin",
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
    let mut case = Case { conn, task_id, dir };
    assert!(matches!(case.run().await, Outcome::Completed(_)));
    for r in server.requests() {
        assert_eq!(r.header("cookie"), None);
        assert_eq!(r.header("referer"), Some("https://exemple.org/"));
    }
}

/// Coffre qui ne répond pas (trousseau verrouillé) : échec temporaire, la tâche réessaiera.
#[tokio::test]
async fn an_unreachable_vault_is_a_temporary_failure() {
    struct Locked;
    impl crate::secrets::Backend for Locked {
        fn kind(&self) -> &'static str {
            "verrouillé"
        }
        fn put(&self, _: &str, _: &str, _: &[u8]) -> Result<(), VaultError> {
            Err(VaultError("verrouillé".into()))
        }
        fn get(&self, _: &str, _: &str) -> Result<Option<Vec<u8>>, VaultError> {
            Err(VaultError("verrouillé".into()))
        }
        fn delete(&self, _: &str, _: &str) -> Result<(), VaultError> {
            Err(VaultError("verrouillé".into()))
        }
        fn ids(&self, _: &str) -> Result<Vec<String>, VaultError> {
            Err(VaultError("verrouillé".into()))
        }
    }
    let (_, server) = site(content(SIZE));
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_in_memory().unwrap();
    let queue = ensure_queue(&conn, "default").unwrap();
    let url = server.url("/v.bin");
    let stored = [("Cookie".to_owned(), Stored::Secret("x".into()))];
    let (task_id, _) = create_task(
        &mut conn,
        &NewTask {
            queue_id: &queue,
            url: &url,
            display_name: "v.bin",
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
    let mut case = Case { conn, task_id, dir };
    let opts = Options {
        vault: Vault::with_backend(Arc::new(Locked)),
        ..opts()
    };
    let (_, stop) = watch::channel(false);
    match case.run_opts(&opts, stop).await {
        Outcome::RetryLater { code, .. } => assert_eq!(code, "SECRET_UNAVAILABLE"),
        other => panic!("{other:?}"),
    }
    assert!(
        server.requests().is_empty(),
        "rien n'est demandé sans le secret"
    );
}

/// Empreinte du fichier complet (F-11, SPEC §7.9) : vérifiée en `verifying` avant le
/// renommage, interruptible, reprise après un arrêt brutal.
mod checksums {
    use super::*;
    use crate::io::checksum::{Algo, Checksum};
    use crate::store::tasks::set_checksum;
    use sha2::{Digest, Sha256};

    fn sha256(data: &[u8]) -> Checksum {
        let hex: String = Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Checksum::new(Algo::Sha256, &hex).unwrap()
    }

    fn wrong() -> Checksum {
        Checksum::new(Algo::Md5, "00000000000000000000000000000000").unwrap()
    }

    fn gets(server: &Server) -> usize {
        server
            .requests()
            .iter()
            .filter(|r| r.method == "GET")
            .count()
    }

    /// Exécution qui relève l'avancement de la vérification, et demande l'arrêt au début
    /// de celle-ci si `stop_on_verify`.
    async fn run_watching(case: &mut Case, stop_on_verify: bool) -> (Outcome, Vec<u64>) {
        let (stop_tx, stop) = watch::channel(false);
        let mut verified = Vec::new();
        let outcome = run(
            &mut case.conn,
            &client(),
            &case.task_id,
            &opts(),
            stop,
            &mut |p| {
                if let Some(n) = p.verified {
                    verified.push(n);
                    if stop_on_verify {
                        let _ = stop_tx.send(true);
                    }
                }
            },
        )
        .await
        .unwrap();
        (outcome, verified)
    }

    #[tokio::test]
    async fn a_matching_checksum_completes_the_task() {
        let data = content(SIZE);
        let (_, server) = site(data.clone());
        let mut case = new_case(&server.url("/a.bin"));
        set_checksum(&case.conn, &case.task_id, Some(&sha256(&data)), None).unwrap();
        let (outcome, verified) = run_watching(&mut case, false).await;
        assert_completed(&case, &outcome, "a.bin", &data);
        assert_eq!(verified.first(), Some(&0));
        assert_eq!(verified.last(), Some(&(SIZE as u64)), "tout le fichier lu");
        assert_eq!(
            case.task().checksum,
            Some(sha256(&data)),
            "empreinte gardée"
        );
    }

    #[tokio::test]
    async fn a_wrong_checksum_fails_and_keeps_the_part_until_it_is_corrected() {
        let data = content(SIZE);
        let (_, server) = site(data.clone());
        let mut case = new_case(&server.url("/a.bin"));
        set_checksum(&case.conn, &case.task_id, Some(&wrong()), None).unwrap();
        let outcome = case.run().await;
        assert!(
            matches!(&outcome, Outcome::Failed { code: "CHECKSUM_MISMATCH", detail }
                if detail.contains("MD5") && detail.contains(&"0".repeat(32))),
            "{outcome:?}"
        );
        let task = case.task();
        assert_eq!(task.state, TaskState::Failed);
        assert_eq!(task.error_code.as_deref(), Some("CHECKSUM_MISMATCH"));
        // Le fichier fautif ne prend jamais son nom ; le `.part` complet reste.
        assert!(!case.dir.path().join("a.bin").exists());
        assert_eq!(std::fs::read(case.part("a.bin")).unwrap(), data);

        // Empreinte corrigée, tâche relancée : le `.part` se vérifie, sans réseau.
        let before = gets(&server);
        set_checksum(&case.conn, &case.task_id, Some(&sha256(&data)), None).unwrap();
        change_state(
            &mut case.conn,
            &case.task_id,
            &Change::to(TaskState::Queued),
        )
        .unwrap();
        let outcome = case.run().await;
        assert_completed(&case, &outcome, "a.bin", &data);
        assert_eq!(gets(&server), before, "rien de retéléchargé");
    }

    #[tokio::test]
    async fn a_stream_of_unknown_size_is_verified_too() {
        let data = content(SIZE);
        let (site, server) = site(data.clone());
        {
            let mut s = site.lock().unwrap();
            s.ranges = false;
            s.send_length = false;
        }
        let mut case = new_case(&server.url("/flux"));
        set_checksum(&case.conn, &case.task_id, Some(&sha256(&data)), None).unwrap();
        let outcome = case.run().await;
        assert_completed(&case, &outcome, "flux", &data);

        let mut case = new_case(&server.url("/flux"));
        set_checksum(&case.conn, &case.task_id, Some(&wrong()), None).unwrap();
        let outcome = case.run().await;
        assert!(matches!(
            outcome,
            Outcome::Failed {
                code: "CHECKSUM_MISMATCH",
                ..
            }
        ));
        assert_eq!(std::fs::read(case.part("flux")).unwrap(), data);
    }

    /// Arrêt demandé pendant le calcul : pause ; à la reprise, le `.part` complet se
    /// vérifie de nouveau, sans rien retélécharger, même d'un serveur sans plages.
    #[tokio::test]
    async fn a_stop_during_the_check_pauses_and_the_resume_downloads_nothing() {
        // Plusieurs lectures de calcul : l'arrêt tombe avant la fin.
        let data = content(3 * 1024 * 1024 + 5);
        let (site, server) = site(data.clone());
        site.lock().unwrap().ranges = false;
        let mut case = new_case(&server.url("/a.bin"));
        set_checksum(&case.conn, &case.task_id, Some(&sha256(&data)), None).unwrap();
        let (outcome, verified) = run_watching(&mut case, true).await;
        assert_eq!(outcome, Outcome::Paused);
        assert!(
            verified.iter().all(|&n| n < data.len() as u64),
            "{verified:?}"
        );
        let task = case.task();
        assert_eq!(task.state, TaskState::Paused);
        assert_eq!(task.pause_reason.as_deref(), Some("user"));
        assert!(!case.dir.path().join("a.bin").exists());
        let before = gets(&server);

        requeue(&mut case.conn, &case.task_id).unwrap();
        let (outcome, verified) = run_watching(&mut case, false).await;
        assert_completed(&case, &outcome, "a.bin", &data);
        assert_eq!(verified.last(), Some(&(data.len() as u64)));
        assert_eq!(gets(&server), before, "le flux n'est pas redemandé");
    }

    /// Tâche préparée dont le `.part` complet est écrit et enregistré, laissée en
    /// `verifying` comme par un arrêt brutal du moteur pendant le calcul. Son adresse est
    /// un port fermé : toute requête échouerait.
    fn left_verifying(data: &[u8], checksum: &Checksum) -> Case {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut case = new_case(&format!("http://127.0.0.1:{port}/f.bin"));
        let len = data.len() as u64;
        let source = primary_source(&case.conn, &case.task_id).unwrap();
        let segments = initial_split(len, 2, 1);
        let prep = Preparation {
            source_id: &source.id,
            accepts_ranges: true,
            etag: Some("\"v1\""),
            last_modified: None,
            total_bytes: Some(len),
            file_name: "f.bin",
        };
        save_preparation(&mut case.conn, &case.task_id, &prep, &segments).unwrap();
        set_checksum(&case.conn, &case.task_id, Some(checksum), None).unwrap();
        for to in [TaskState::Active, TaskState::Verifying] {
            change_state(&mut case.conn, &case.task_id, &Change::to(to)).unwrap();
        }
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
        part.seal().unwrap();
        case
    }

    #[tokio::test]
    async fn a_crash_during_the_check_resumes_it_without_network() {
        let data = content(5000);
        let mut case = left_verifying(&data, &sha256(&data));
        let (outcome, verified) = run_watching(&mut case, false).await;
        assert_completed(&case, &outcome, "f.bin", &data);
        assert_eq!(verified.last(), Some(&5000));

        // Empreinte fausse : échec, `.part` gardé.
        let mut case = left_verifying(&data, &wrong());
        let outcome = case.run().await;
        assert!(matches!(
            outcome,
            Outcome::Failed {
                code: "CHECKSUM_MISMATCH",
                ..
            }
        ));
        assert!(case.part("f.bin").exists());
    }

    /// Arrêt brutal entre le renommage, qui ne suit qu'une empreinte juste, et
    /// l'enregistrement de la fin : la tâche se termine.
    #[tokio::test]
    async fn a_crash_after_the_rename_of_a_checked_file_completes_it() {
        let data = content(5000);
        let mut case = left_verifying(&data, &sha256(&data));
        SealedPart::existing(&case.dir.path().join("f.bin"))
            .finish()
            .unwrap();
        let outcome = case.run().await;
        assert_completed(&case, &outcome, "f.bin", &data);

        // Ni `.part` ni fichier : échec, rien d'inventé.
        let mut case = left_verifying(&data, &sha256(&data));
        std::fs::remove_file(case.part("f.bin")).unwrap();
        let outcome = case.run().await;
        assert!(
            matches!(
                outcome,
                Outcome::Failed {
                    code: "DISK_IO",
                    ..
                }
            ),
            "{outcome:?}"
        );
    }
}

/// Empreinte lue dans un fichier voisin (F-11, SPEC §7.9) : cherchée à la préparation
/// quand la tâche n'en a pas, puis vérifiée comme celle de l'utilisateur.
mod neighbor_checksums {
    use super::*;
    use crate::io::checksum::{Algo, Checksum};
    use crate::store::tasks::{restart_task, set_checksum};
    use crate::testing::content_sha256;

    const ZEROS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    async fn run_neighbors(case: &mut Case, on: bool) -> Outcome {
        let opts = Options {
            neighbor_checksum: on,
            ..opts()
        };
        // Émetteur abandonné : aucun arrêt ne sera demandé.
        let (_, stop) = watch::channel(false);
        case.run_opts(&opts, stop).await
    }

    fn asked(site: &Arc<Mutex<FileSite>>) -> Vec<String> {
        site.lock().unwrap().neighbor_requests.clone()
    }

    #[tokio::test]
    async fn a_list_next_to_the_file_gives_its_checksum_which_is_then_checked() {
        let data = content(SIZE);
        let (site, server) = site(data.clone());
        let hex = content_sha256(SIZE as u64);
        let list = format!("{ZEROS}  autre.bin\n{hex}  a.bin\n");
        site.lock()
            .unwrap()
            .neighbors
            .push(("/pub/SHA256SUMS".into(), list.into_bytes()));
        let mut case = new_case(&server.url("/pub/a.bin?jeton=1"));
        let outcome = run_neighbors(&mut case, true).await;
        assert_completed(&case, &outcome, "a.bin", &data);
        let task = case.task();
        assert_eq!(
            task.checksum.as_ref().map(Checksum::value),
            Some(hex.as_str())
        );
        assert_eq!(task.checksum_source.as_deref(), Some("SHA256SUMS"));
        // Les six fichiers voisins demandés, à côté du fichier, sans la requête.
        let mut paths = asked(&site);
        paths.sort();
        assert_eq!(
            paths,
            [
                "/pub/MD5SUMS",
                "/pub/SHA1SUMS",
                "/pub/SHA256SUMS",
                "/pub/a.bin.md5",
                "/pub/a.bin.sha1",
                "/pub/a.bin.sha256",
            ]
        );
        // L'historique garde l'empreinte vérifiée.
        let kept: Option<String> = case
            .conn
            .query_row("SELECT sha256 FROM history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept.as_deref(), Some(hex.as_str()));

        // Un retéléchargement l'oublie : le fichier a pu changer depuis.
        restart_task(&mut case.conn, &case.task_id).unwrap();
        let task = case.task();
        assert_eq!(task.checksum, None);
        assert_eq!(task.checksum_source, None);
    }

    #[tokio::test]
    async fn sha256_is_preferred_and_a_wrong_value_keeps_the_part() {
        let data = content(SIZE);
        let (site, server) = site(data.clone());
        {
            let mut site = site.lock().unwrap();
            // SHA-256 faux dans le compagnon, MD5 de la liste juste ou non : SHA-256 gagne.
            site.neighbors
                .push(("/a.bin.sha256".into(), format!("{ZEROS}\n").into_bytes()));
            site.neighbors.push((
                "/MD5SUMS".into(),
                format!("{}  a.bin\n", &ZEROS[..32]).into_bytes(),
            ));
        }
        let mut case = new_case(&server.url("/a.bin"));
        let outcome = run_neighbors(&mut case, true).await;
        assert!(
            matches!(&outcome, Outcome::Failed { code: "CHECKSUM_MISMATCH", detail }
                if detail.contains("SHA-256")),
            "{outcome:?}"
        );
        let task = case.task();
        assert_eq!(task.checksum_source.as_deref(), Some("a.bin.sha256"));
        assert!(!case.dir.path().join("a.bin").exists());
        assert_eq!(std::fs::read(case.part("a.bin")).unwrap(), data);

        // L'empreinte de l'utilisateur remplace celle du voisin, et un retéléchargement
        // la garde.
        let mine = Checksum::new(Algo::Sha256, &content_sha256(SIZE as u64)).unwrap();
        set_checksum(&case.conn, &case.task_id, Some(&mine), None).unwrap();
        restart_task(&mut case.conn, &case.task_id).unwrap();
        let task = case.task();
        assert_eq!(task.checksum, Some(mine));
        assert_eq!(task.checksum_source, None);
    }

    #[tokio::test]
    async fn without_neighbors_the_file_completes_without_a_checksum() {
        let data = content(SIZE);
        let (site, server) = site(data.clone());
        let mut case = new_case(&server.url("/a.bin"));
        let outcome = run_neighbors(&mut case, true).await;
        assert_completed(&case, &outcome, "a.bin", &data);
        assert_eq!(case.task().checksum, None);
        assert_eq!(asked(&site).len(), 6, "{:?}", asked(&site));
    }

    #[tokio::test]
    async fn nothing_is_asked_when_off_or_when_the_user_gave_a_checksum() {
        let data = content(SIZE);
        let (site, server) = site(data.clone());
        site.lock()
            .unwrap()
            .neighbors
            .push(("/a.bin.sha256".into(), format!("{ZEROS}\n").into_bytes()));

        // Réglage coupé.
        let mut case = new_case(&server.url("/a.bin"));
        let outcome = run_neighbors(&mut case, false).await;
        assert_completed(&case, &outcome, "a.bin", &data);
        assert_eq!(case.task().checksum, None);
        assert!(asked(&site).is_empty());

        // Empreinte de l'utilisateur : la sienne compte, aucun voisin n'est lu.
        let mut case = new_case(&server.url("/a.bin"));
        let mine = Checksum::new(Algo::Sha256, &content_sha256(SIZE as u64)).unwrap();
        set_checksum(&case.conn, &case.task_id, Some(&mine), None).unwrap();
        let outcome = run_neighbors(&mut case, true).await;
        assert_completed(&case, &outcome, "a.bin", &data);
        assert!(asked(&site).is_empty());
    }
}
