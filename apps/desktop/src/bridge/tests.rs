//! La liaison face au vrai protocole du moteur, servi en mémoire (`rpc::serve`) : les
//! tests coupent la connexion à volonté, comme un `aurod` qui s'arrête.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auro_engine::net::Client;
use auro_engine::scheduler::{Config, DISK_RECHECK, Engine, Planning};
use auro_engine::testing::{FileSite, Pace, serve_file_paced};
use auro_engine::transfer::Options;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex, split};
use tokio::task::JoinHandle;

use super::*;

const WAIT: Duration = Duration::from_secs(30);

fn fast() -> Retry {
    Retry {
        first: Duration::from_millis(50),
        max: Duration::from_millis(200),
    }
}

fn engine(dir: &std::path::Path) -> Engine {
    Engine::start(Config {
        db_path: dir.join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options {
            connections: Some(1),
            min_segment: 16 * 1024,
            buffer: 4096,
            checkpoint_every: Duration::from_millis(50),
            ..Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap()
}

#[derive(Default)]
struct TestSink {
    log: Mutex<Vec<Outgoing>>,
    hidden: AtomicBool,
}

impl Sink for TestSink {
    fn send(&self, out: Outgoing) {
        self.log.lock().unwrap().push(out);
    }
    fn hidden(&self) -> bool {
        self.hidden.load(Ordering::SeqCst)
    }
}

impl TestSink {
    fn log(&self) -> Vec<Outgoing> {
        self.log.lock().unwrap().clone()
    }

    /// Attend que le journal vérifie `ready` ; renvoie la position où il l'a vérifié.
    async fn wait(&self, what: &str, ready: impl Fn(&[Outgoing]) -> Option<usize>) -> usize {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            if let Some(i) = ready(&self.log()) {
                return i;
            }
            assert!(tokio::time::Instant::now() < deadline, "attendu : {what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Attend la `n`-ième annonce `Up` (à partir de 1).
    async fn wait_up(&self, n: usize) -> usize {
        self.wait("connexion établie", |log| {
            nth(log, n, |o| matches!(o, Outgoing::Status(Status::Up { .. })))
        })
        .await
    }

    async fn wait_down(&self, n: usize) -> usize {
        self.wait("connexion perdue", |log| {
            nth(log, n, |o| {
                matches!(o, Outgoing::Status(Status::Down { .. }))
            })
        })
        .await
    }

    fn count(&self, method: &str) -> usize {
        self.log().iter().filter(|o| is_message(o, method)).count()
    }
}

fn nth(log: &[Outgoing], n: usize, pred: impl Fn(&Outgoing) -> bool) -> Option<usize> {
    log.iter()
        .enumerate()
        .filter(|(_, o)| pred(o))
        .nth(n - 1)
        .map(|(i, _)| i)
}

fn is_message(o: &Outgoing, method: &str) -> bool {
    matches!(o, Outgoing::Message { message } if message["method"] == method)
}

/// Moteur servi en mémoire : chaque connexion est une session `rpc::serve`.
struct Served {
    engine: Engine,
    sessions: Mutex<Vec<JoinHandle<()>>>,
    /// Nombre de connexions à refuser avant d'accepter.
    refuse: Mutex<usize>,
}

impl Served {
    fn new(engine: Engine) -> Arc<Self> {
        Arc::new(Served {
            engine,
            sessions: Mutex::new(Vec::new()),
            refuse: Mutex::new(0),
        })
    }

    fn connect(self: &Arc<Self>) -> Connect {
        let served = Arc::clone(self);
        Box::new(move || {
            let served = Arc::clone(&served);
            Box::pin(async move {
                let refused = {
                    let mut left = served.refuse.lock().unwrap();
                    let refuse = *left > 0;
                    *left = left.saturating_sub(1);
                    refuse
                };
                if refused {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        "connexion refusée par le test",
                    ));
                }
                let (ours, theirs) = duplex(1 << 20);
                let (read, write) = split(theirs);
                let session =
                    tokio::spawn(auro_engine::rpc::serve(served.engine.clone(), read, write));
                served.sessions.lock().unwrap().push(session);
                let (read, write) = split(ours);
                Ok((Box::new(read) as _, Box::new(write) as _))
            })
        })
    }

    /// Coupe les connexions ouvertes, comme un moteur qui s'arrête.
    fn cut(&self) {
        for session in self.sessions.lock().unwrap().drain(..) {
            session.abort();
        }
    }
}

fn start(connect: Connect, sink: &Arc<TestSink>) -> Arc<Bridge> {
    let bridge = Bridge::new();
    let sink: Arc<dyn Sink> = sink.clone();
    tokio::spawn(Arc::clone(&bridge).run(connect, sink, fast()));
    bridge
}

fn add_params(dir: &std::path::Path, url: &str, paused: bool) -> Value {
    json!({
        "sources": [{"url": url}],
        "destDir": dir.to_str().unwrap(),
        "startPaused": paused,
    })
}

#[tokio::test]
async fn up_is_announced_before_the_notifications_it_carries() {
    let dir = tempfile::tempdir().unwrap();
    let served = Served::new(engine(dir.path()));
    let sink = Arc::new(TestSink::default());
    let bridge = start(served.connect(), &sink);
    let up = sink.wait_up(1).await;
    assert_eq!(bridge.status(), sink.log()[up].clone().status().unwrap());

    let added = bridge
        .call(
            "task.add",
            add_params(dir.path(), "http://127.0.0.1:9/a.bin", true),
        )
        .await
        .unwrap();
    let id = added["taskId"].as_str().unwrap().to_owned();
    let at = sink
        .wait("task.added", |log| {
            log.iter().position(|o| {
                matches!(o, Outgoing::Message { message }
                    if message["method"] == "task.added" && message["params"]["task"]["taskId"] == id.as_str())
            })
        })
        .await;
    assert!(
        up < at,
        "l'annonce de la connexion précède ses notifications"
    );

    // Erreur du moteur : son code stable, pas ENGINE_DOWN.
    let refused = bridge.call("task.nothing", json!({})).await.unwrap_err();
    assert_eq!(refused.code, "METHOD_NOT_FOUND");
}

#[tokio::test]
async fn a_lost_engine_is_reported_then_reached_again() {
    let dir = tempfile::tempdir().unwrap();
    let served = Served::new(engine(dir.path()));
    let sink = Arc::new(TestSink::default());
    let bridge = start(served.connect(), &sink);
    sink.wait_up(1).await;

    // Les trois prochaines tentatives échouent : la perte reste visible un moment.
    *served.refuse.lock().unwrap() = 3;
    served.cut();
    sink.wait_down(1).await;
    let refused = bridge.call("task.list", json!({})).await.unwrap_err();
    assert_eq!(refused.code, "ENGINE_DOWN");

    sink.wait_up(2).await;
    let listed = bridge.call("task.list", json!({})).await.unwrap();
    assert!(listed["items"].is_array());
    let Outgoing::Status(Status::Down { message }) = sink.log()[sink.wait_down(2).await].clone()
    else {
        unreachable!()
    };
    assert!(message.contains("refusée par le test"), "{message}");
}

#[tokio::test]
async fn a_call_pending_when_the_connection_drops_fails() {
    // Faux moteur : ouvre la session, puis ferme la connexion à la réception de « hang ».
    let connect: Connect = Box::new(|| {
        Box::pin(async {
            let (ours, theirs) = duplex(1 << 16);
            tokio::spawn(async move {
                let (read, mut write) = split(theirs);
                let mut lines = BufReader::new(read).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let request: Value = serde_json::from_str(&line).unwrap();
                    if request["method"] == "hang" {
                        return;
                    }
                    let reply = json!({"jsonrpc": "2.0", "id": request["id"], "result": {}});
                    write
                        .write_all(format!("{reply}\n").as_bytes())
                        .await
                        .unwrap();
                }
            });
            let (read, write) = split(ours);
            Ok((Box::new(read) as _, Box::new(write) as _))
        })
    });
    let sink = Arc::new(TestSink::default());
    let bridge = start(connect, &sink);
    sink.wait_up(1).await;
    let pending = tokio::time::timeout(WAIT, bridge.call("hang", json!({})))
        .await
        .expect("l'appel ne doit pas attendre sans fin");
    assert_eq!(pending.unwrap_err().code, "ENGINE_DOWN");
}

#[tokio::test]
async fn a_hidden_page_gets_no_progress_and_is_told_to_reload() {
    let dir = tempfile::tempdir().unwrap();
    let site = Arc::new(Mutex::new(FileSite::new(64 << 20)));
    let pace = Pace {
        chunk: 4096,
        delay: Duration::from_millis(10),
    };
    let server = serve_file_paced(site, Some(pace));
    let served = Served::new(engine(dir.path()));
    let sink = Arc::new(TestSink::default());
    let bridge = start(served.connect(), &sink);
    sink.wait_up(1).await;
    bridge
        .call(
            "task.add",
            add_params(dir.path(), &server.url("/gros.bin"), false),
        )
        .await
        .unwrap();
    sink.wait("avancement", |log| {
        log.iter().position(|o| is_message(o, "task.progress"))
    })
    .await;

    sink.hidden.store(true, Ordering::SeqCst);
    let before = sink.count("task.progress");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(sink.count("task.progress"), before, "page cachée : rien");
    assert!(bridge.take_missed(), "avancement omis");
    assert!(!bridge.take_missed(), "signalé une seule fois");

    sink.hidden.store(false, Ordering::SeqCst);
    sink.wait("avancement après réapparition", |log| {
        (log.iter()
            .filter(|o| is_message(o, "task.progress"))
            .count()
            > before)
            .then_some(0)
    })
    .await;
}

impl Outgoing {
    fn status(self) -> Option<Status> {
        match self {
            Outgoing::Status(s) => Some(s),
            _ => None,
        }
    }
}
