//! Protocole sur un flux en mémoire : un client de test écrit des lignes JSON et lit les
//! réponses et les notifications, comme sur le canal local.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf};
use tokio::io::{WriteHalf, duplex, split};
use tokio::time::timeout;

use super::serve;
use crate::net::Client;
use crate::scheduler::{Config, DISK_RECHECK, Engine};
use crate::testing::{FileSite, Server, serve_file};
use crate::transfer::Options;

const WAIT: Duration = Duration::from_secs(30);

fn engine(dir: &Path) -> Engine {
    Engine::start(Config {
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
    })
    .unwrap()
}

fn site(len: u64) -> Server {
    serve_file(Arc::new(Mutex::new(FileSite::new(len))))
}

struct TestClient {
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    writer: WriteHalf<DuplexStream>,
    /// Notifications reçues en attendant une réponse.
    pending: VecDeque<Value>,
    next_id: u64,
}

impl TestClient {
    fn connect(engine: &Engine) -> Self {
        let (ours, theirs) = duplex(1 << 20);
        let (read, write) = split(theirs);
        tokio::spawn(serve(engine.clone(), read, write));
        let (read, writer) = split(ours);
        TestClient {
            lines: BufReader::new(read).lines(),
            writer,
            pending: VecDeque::new(),
            next_id: 1,
        }
    }

    async fn send_raw(&mut self, text: &str) {
        self.writer.write_all(text.as_bytes()).await.unwrap();
        self.writer.write_all(b"\n").await.unwrap();
    }

    /// Prochain message, `None` à la fermeture.
    async fn read(&mut self) -> Option<Value> {
        let line = timeout(WAIT, self.lines.next_line())
            .await
            .expect("pas de message")
            .unwrap()?;
        Some(serde_json::from_str(&line).unwrap())
    }

    /// Prochaine réponse (avec `id`, ou lot) ; met les notifications de côté.
    async fn reply(&mut self) -> Value {
        loop {
            let message = self.read().await.expect("connexion fermée");
            if message.is_array() || message.get("id").is_some() {
                return message;
            }
            self.pending.push_back(message);
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send_raw(&request.to_string()).await;
        let reply = self.reply().await;
        assert_eq!(reply["id"], json!(id));
        reply
    }

    async fn result(&mut self, method: &str, params: Value) -> Value {
        let reply = self.call(method, params).await;
        assert!(reply.get("error").is_none(), "{method} : {reply}");
        reply["result"].clone()
    }

    async fn error_code(&mut self, method: &str, params: Value) -> String {
        let reply = self.call(method, params).await;
        reply["error"]["data"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("{method} : erreur attendue, reçu {reply}"))
            .to_owned()
    }

    /// Prochaine notification `method` qui satisfait `pred`.
    async fn notification(&mut self, method: &str, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            let message = match self.pending.pop_front() {
                Some(m) => m,
                None => self.read().await.expect("connexion fermée"),
            };
            if message["method"] == method && pred(&message["params"]) {
                return message["params"].clone();
            }
        }
    }
}

fn hello() -> Value {
    json!({"client": {"name": "test", "version": "0"}, "protocol": {"min": "1.0", "max": "1.2"}})
}

fn dir_text(dir: &Path) -> String {
    dir.to_str().unwrap().to_owned()
}

#[tokio::test]
async fn hello_negotiates_the_protocol_version() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let result = c.result("engine.hello", hello()).await;
    assert_eq!(result["protocol"], "1.0");
    assert_eq!(result["engine"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(result["rev"].is_u64());

    let mut old = TestClient::connect(&engine);
    let too_new = json!({"client": {"name": "t"}, "protocol": {"min": "2.0", "max": "2.3"}});
    let reply = old.call("engine.hello", too_new).await;
    assert_eq!(reply["error"]["code"], -32002);
    assert_eq!(reply["error"]["data"]["code"], "PROTOCOL_UNSUPPORTED");
    assert!(
        old.read().await.is_none(),
        "connexion fermée après le refus"
    );
    engine.shutdown().await;
}

#[tokio::test]
async fn malformed_messages_get_standard_errors() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);

    c.send_raw("{pas du json").await;
    let reply = c.reply().await;
    assert_eq!(reply["error"]["code"], -32700);
    assert_eq!(reply["id"], Value::Null);

    c.send_raw(r#"{"id": 9, "method": "engine.hello"}"#).await;
    assert_eq!(c.reply().await["error"]["code"], -32600);

    assert_eq!(
        c.error_code("task.explode", json!({})).await,
        "METHOD_NOT_FOUND"
    );
    assert_eq!(
        c.error_code("task.get", json!(["x"])).await,
        "INVALID_PARAMS"
    );
    assert_eq!(c.error_code("task.get", json!({})).await, "INVALID_PARAMS");
    assert_eq!(
        c.error_code("task.get", json!({"taskId": "inconnue"}))
            .await,
        "TASK_NOT_FOUND"
    );

    // Une notification du client (sans `id`) ne reçoit aucune réponse, même en erreur.
    c.send_raw(r#"{"jsonrpc": "2.0", "method": "task.explode"}"#)
        .await;
    let reply = c.call("sync.snapshot", json!({})).await;
    assert!(reply["result"]["rev"].is_u64());
    engine.shutdown().await;
}

#[tokio::test]
async fn batches_are_answered_together() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.send_raw(
        r#"[{"jsonrpc":"2.0","id":1,"method":"engine.hello","params":{"protocol":{"min":"1.0","max":"1.0"}}},
            {"jsonrpc":"2.0","method":"events.unsubscribe"},
            {"jsonrpc":"2.0","id":2,"method":"nope"}]"#
            .replace('\n', " ")
            .as_str(),
    )
    .await;
    let replies = c.reply().await;
    let replies = replies.as_array().unwrap();
    assert_eq!(replies.len(), 2, "pas de réponse à la notification");
    assert_eq!(replies[0]["result"]["protocol"], "1.0");
    assert_eq!(replies[1]["error"]["code"], -32601);

    c.send_raw("[]").await;
    assert_eq!(c.reply().await["error"]["code"], -32600);
    engine.shutdown().await;
}

#[tokio::test]
async fn an_added_task_is_followed_to_completion() {
    let dir = tempfile::tempdir().unwrap();
    let server = site(512 * 1024);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    let sub = c
        .result(
            "events.subscribe",
            json!({"topics": ["tasks", "progress"], "progressIntervalMs": 50}),
        )
        .await;
    let start = sub["rev"].as_u64().unwrap();
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/f.bin")}], "destDir": dir_text(dir.path())}),
        )
        .await;
    let id = added["taskId"].as_str().unwrap().to_owned();

    let note = c.notification("task.added", |_| true).await;
    assert_eq!(note["task"]["taskId"], id.as_str());
    assert_eq!(note["rev"], start + 1);
    let done = c
        .notification("task.stateChanged", |p| p["to"] == "completed")
        .await;
    assert_eq!(done["taskId"], id.as_str());
    assert_eq!(done["from"], "active");

    let task = c.result("task.get", json!({"taskId": id})).await;
    assert_eq!(task["state"], "completed");
    assert_eq!(task["doneBytes"], 512 * 1024);
    assert_eq!(
        std::fs::metadata(dir.path().join("f.bin")).unwrap().len(),
        512 * 1024
    );
    engine.shutdown().await;
}

#[tokio::test]
async fn the_file_name_chosen_at_preparation_is_announced() {
    // Deux tâches du même nom dans le même dossier : chacune annonce le sien, celui qu'un
    // client doit ouvrir une fois la tâche terminée.
    let dir = tempfile::tempdir().unwrap();
    let server = site(256 * 1024);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let mut ids = Vec::new();
    for _ in 0..2 {
        let added = c
            .result(
                "task.add",
                json!({"sources": [{"url": server.url("/meme.bin")}], "destDir": dir_text(dir.path())}),
            )
            .await;
        ids.push(added["taskId"].as_str().unwrap().to_owned());
    }
    let mut names = Vec::new();
    for id in &ids {
        let note = c
            .notification("task.updated", |p| p["taskId"] == id.as_str())
            .await;
        assert_eq!(note["totalBytes"], 256 * 1024);
        assert_eq!(note["displayName"], note["fileName"]);
        let task = c.result("task.get", json!({"taskId": id})).await;
        assert_eq!(note["fileName"], task["fileName"], "le nom enregistré");
        names.push(note["fileName"].as_str().unwrap().to_owned());
    }
    names.sort();
    assert_eq!(names, ["meme (1).bin", "meme.bin"]);
    engine.shutdown().await;
}

#[tokio::test]
async fn progress_is_batched_with_the_current_revision() {
    let dir = tempfile::tempdir().unwrap();
    let pace = crate::testing::Pace {
        chunk: 16 * 1024,
        delay: Duration::from_millis(10),
    };
    let server =
        crate::testing::serve_file_paced(Arc::new(Mutex::new(FileSite::new(4 << 20))), Some(pace));
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result(
        "events.subscribe",
        json!({"topics": ["progress"], "progressIntervalMs": 100}),
    )
    .await;
    c.result(
        "task.add",
        json!({"sources": [{"url": server.url("/slow.bin")}], "destDir": dir_text(dir.path())}),
    )
    .await;
    let p = c
        .notification("task.progress", |p| {
            p["items"][0]["doneBytes"].as_u64().unwrap_or(0) > 0
        })
        .await;
    let item = &p["items"][0];
    assert_eq!(item["totalBytes"], 4 << 20);
    assert!(item["connections"].is_u64());
    assert!(p["rev"].is_u64());
    // Abonné à l'avancement seulement : aucun changement d'état n'arrive.
    assert!(c.pending.iter().all(|m| m["method"] != "task.stateChanged"));
    engine.shutdown().await;
}

#[tokio::test]
async fn tasks_are_listed_page_by_page() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let mut ids = Vec::new();
    for i in 0..3 {
        let added = c
            .result(
                "task.add",
                json!({
                    "sources": [{"url": format!("http://127.0.0.1:1/{i}")}],
                    "destDir": dir_text(dir.path()),
                    "startPaused": true,
                }),
            )
            .await;
        ids.push(added["taskId"].as_str().unwrap().to_owned());
    }
    let first = c
        .result("task.list", json!({"limit": 2, "states": ["paused"]}))
        .await;
    let page: Vec<&str> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["taskId"].as_str().unwrap())
        .collect();
    assert_eq!(page, [ids[0].as_str(), ids[1].as_str()]);
    let cursor = first["nextCursor"].clone();
    assert_eq!(cursor, ids[1].as_str());
    let second = c
        .result("task.list", json!({"limit": 2, "cursor": cursor}))
        .await;
    assert_eq!(second["items"][0]["taskId"], ids[2].as_str());
    assert_eq!(second["nextCursor"], Value::Null);
    assert_eq!(
        c.error_code("task.list", json!({"states": ["perdu"]}))
            .await,
        "INVALID_PARAMS"
    );
    engine.shutdown().await;
}

#[tokio::test]
async fn actions_report_refused_tasks_separately() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let added = c
        .result(
            "task.add",
            json!({
                "sources": [{"url": "http://127.0.0.1:1/x"}],
                "destDir": dir_text(dir.path()),
                "startPaused": true,
            }),
        )
        .await;
    let id = added["taskId"].as_str().unwrap();
    let result = c
        .result(
            "task.cancel",
            json!({"taskIds": [id, "inconnue"], "deleteFiles": true}),
        )
        .await;
    assert_eq!(result["taskIds"], json!([id]));
    assert_eq!(result["errors"][0]["taskId"], "inconnue");
    assert_eq!(result["errors"][0]["code"], "TASK_NOT_FOUND");
    let again = c.result("task.resume", json!({"taskIds": [id]})).await;
    assert_eq!(again["errors"][0]["code"], "INVALID_STATE");
    assert_eq!(
        c.error_code("task.pause", json!({"taskIds": []})).await,
        "INVALID_PARAMS"
    );
    engine.shutdown().await;
}

#[tokio::test]
async fn unsupported_or_invalid_additions_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let base = |extra: Value| {
        let mut p = json!({"sources": [{"url": "http://h/f"}], "destDir": dir_text(dir.path())});
        p.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        p
    };
    let cases = [
        (
            json!({"checksum": {"algo": "sha256", "value": "00"}}),
            "CAPABILITY_MISSING",
        ),
        (
            json!({"sources": [{"url": "http://a/f"}, {"url": "http://b/f"}]}),
            "CAPABILITY_MISSING",
        ),
        (json!({"sources": [{"url": "ftp://h/f"}]}), "URL_INVALID"),
        (json!({"destDir": "relatif"}), "DEST_NOT_WRITABLE"),
        (json!({"queueId": "inconnue"}), "QUEUE_NOT_FOUND"),
        (json!({"maxConnections": 0}), "INVALID_PARAMS"),
        (json!({"speedLimitBps": 0}), "INVALID_PARAMS"),
        (json!({"sources": []}), "INVALID_PARAMS"),
    ];
    for (extra, code) in cases {
        let params = base(extra.clone());
        assert_eq!(c.error_code("task.add", params).await, code, "{extra}");
    }
    engine.shutdown().await;
}

#[tokio::test]
async fn queues_are_managed_and_announced() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    let created = c
        .result("queue.create", json!({"name": "nuit", "maxActive": 1}))
        .await;
    assert_eq!(created["maxActive"], 1);
    let id = created["queueId"].as_str().unwrap().to_owned();
    let note = c.notification("queue.updated", |_| true).await;
    assert_eq!(note["queue"]["name"], "nuit");

    let updated = c
        .result("queue.update", json!({"queueId": id, "maxActive": 4}))
        .await;
    assert_eq!(updated["maxActive"], 4);
    let list = c.result("queue.list", json!({})).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        c.error_code("queue.create", json!({"name": "nuit"})).await,
        "INVALID_PARAMS"
    );
    assert_eq!(
        c.error_code("queue.update", json!({"queueId": "x", "maxActive": 2}))
            .await,
        "QUEUE_NOT_FOUND"
    );
    assert_eq!(
        c.error_code("events.subscribe", json!({"topics": ["meteo"]}))
            .await,
        "INVALID_PARAMS"
    );
    engine.shutdown().await;
}

#[tokio::test]
async fn speed_limits_are_read_changed_and_announced() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let none = json!({"downloadBps": null, "uploadBps": null});
    assert_eq!(c.result("limits.get", json!({})).await, none);
    let set = c
        .result("limits.set", json!({"downloadBps": 1_000_000}))
        .await;
    assert_eq!(set["downloadBps"], 1_000_000);
    // Paramètre absent : limite inchangée ; `null` : retirée.
    let same = c.result("limits.set", json!({})).await;
    assert_eq!(same["downloadBps"], 1_000_000);
    assert_eq!(
        c.result("limits.set", json!({"downloadBps": null})).await,
        none
    );
    for (params, code) in [
        (json!({"uploadBps": 5}), "CAPABILITY_MISSING"),
        (json!({"downloadBps": 0}), "INVALID_PARAMS"),
        (json!({"downloadBps": "vite"}), "INVALID_PARAMS"),
    ] {
        assert_eq!(
            c.error_code("limits.set", params.clone()).await,
            code,
            "{params}"
        );
    }

    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"url": "http://127.0.0.1:1/f"}],
                   "destDir": dir_text(dir.path()), "startPaused": true,
                   "speedLimitBps": 2000}),
        )
        .await;
    let id = added["taskId"].as_str().unwrap().to_owned();
    let task = c.result("task.get", json!({"taskId": id})).await;
    assert_eq!(task["speedLimitBps"], 2000);
    let updated = c
        .result("task.update", json!({"taskId": id, "speedLimitBps": 4000}))
        .await;
    assert_eq!(updated["speedLimitBps"], 4000);
    let note = c.notification("task.updated", |_| true).await;
    assert_eq!(note["taskId"], json!(id));
    assert_eq!(note["speedLimitBps"], 4000);
    let lifted = c
        .result("task.update", json!({"taskId": id, "speedLimitBps": null}))
        .await;
    assert!(lifted["speedLimitBps"].is_null());
    let note = c.notification("task.updated", |_| true).await;
    assert!(note.as_object().unwrap().contains_key("speedLimitBps"));
    assert_eq!(
        c.error_code("task.update", json!({"taskId": id, "maxConnections": 2}))
            .await,
        "CAPABILITY_MISSING"
    );
    assert_eq!(
        c.error_code("task.update", json!({"taskId": "x", "speedLimitBps": 1}))
            .await,
        "TASK_NOT_FOUND"
    );

    let queue = c
        .result(
            "queue.create",
            json!({"name": "nuit", "speedLimitBps": 3000}),
        )
        .await;
    assert_eq!(queue["speedLimitBps"], 3000);
    let queue_id = queue["queueId"].clone();
    let changed = c
        .result("queue.update", json!({"queueId": queue_id, "maxActive": 1}))
        .await;
    assert_eq!(changed["speedLimitBps"], 3000, "inchangée si absente");
    let lifted = c
        .result(
            "queue.update",
            json!({"queueId": queue_id, "speedLimitBps": null}),
        )
        .await;
    assert!(lifted["speedLimitBps"].is_null());
    engine.shutdown().await;
}

#[tokio::test]
async fn shutdown_stops_the_engine_after_answering() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let reply = c.call("engine.shutdown", json!({"graceful": true})).await;
    assert_eq!(reply["result"], json!({}));
    timeout(WAIT, engine.stopped())
        .await
        .expect("moteur arrêté");
}

#[tokio::test]
async fn the_local_endpoint_serves_one_engine_at_a_time() {
    use super::local;

    let dir = tempfile::tempdir().unwrap();
    let endpoint = if cfg!(windows) {
        format!(r"\\.\pipe\aurodownloader-test-{}", crate::store::new_id())
    } else {
        dir.path()
            .join("run")
            .join("aurod.sock")
            .to_string_lossy()
            .into_owned()
    };
    let engine = engine(dir.path());
    let mut listening = tokio::spawn({
        let (engine, endpoint) = (engine.clone(), endpoint.clone());
        async move { local::listen(engine, &endpoint).await }
    });

    // Le serveur peut ne pas être encore prêt.
    let (read, mut write) = timeout(WAIT, async {
        loop {
            match local::connect(&endpoint).await {
                Ok(halves) => return halves,
                Err(e) => {
                    if listening.is_finished() {
                        let server = (&mut listening).await;
                        panic!("le serveur s'est arrêté : {server:?} ; client : {e}");
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
            }
        }
    })
    .await
    .expect("connexion au point d'accès");
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "engine.hello", "params": hello()});
    write
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let mut lines = BufReader::new(read).lines();
    let line = timeout(WAIT, lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["result"]["protocol"], "1.0");

    // Un second moteur ne peut pas prendre le même point d'accès.
    let other = engine_at(dir.path(), "autre.db");
    let err = local::listen(other.clone(), &endpoint).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse, "{err}");
    other.shutdown().await;

    engine.shutdown().await;
    timeout(WAIT, listening)
        .await
        .expect("le serveur s'arrête avec le moteur")
        .unwrap()
        .unwrap();
}

fn engine_at(dir: &Path, db: &str) -> Engine {
    Engine::start(Config {
        db_path: dir.join(db),
        client: Client::new().unwrap(),
        transfer: Options::default(),
        disk_recheck: DISK_RECHECK,
    })
    .unwrap()
}
