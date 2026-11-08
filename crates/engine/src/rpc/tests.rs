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
use crate::scheduler::{Config, DISK_RECHECK, Engine, Planning};
use crate::testing::{FileSite, Pace, Server, serve_file, serve_file_paced};
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
        planning: Planning::default(),
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

/// Portées (SPEC §6.9) : une connexion qui se limite à `add` ne peut plus qu'ajouter ;
/// une portée ne s'élargit jamais ; `control` n'éteint ni ne planifie.
#[tokio::test]
async fn a_connection_keeps_to_the_scope_it_declares() {
    let dir = tempfile::tempdir().unwrap();
    let server = site(1024);
    let engine = engine(dir.path());

    // Par défaut, sur le canal local : tous les droits.
    let mut admin = TestClient::connect(&engine);
    assert_eq!(
        admin.result("engine.hello", hello()).await["scope"],
        "admin"
    );

    let mut relay = TestClient::connect(&engine);
    let mut params = hello();
    params["scope"] = json!("add");
    assert_eq!(relay.result("engine.hello", params).await["scope"], "add");
    let added = relay
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/r.bin")}], "destDir": dir_text(dir.path()), "startPaused": true}),
        )
        .await;
    let id = added["taskId"].as_str().unwrap().to_owned();
    for (method, params) in [
        ("task.list", json!({})),
        ("task.get", json!({"taskId": id})),
        ("sync.snapshot", json!({})),
        ("events.subscribe", json!({"topics": ["tasks"]})),
        ("task.remove", json!({"taskIds": [id]})),
        ("task.resume", json!({"taskIds": [id]})),
        ("limits.set", json!({"downloadBps": 1000})),
        ("engine.shutdown", json!({})),
    ] {
        assert_eq!(
            relay.error_code(method, params).await,
            "UNAUTHORIZED",
            "{method}"
        );
    }
    // Ni plus large, ni de côté.
    for scope in ["admin", "control", "read"] {
        let mut params = hello();
        params["scope"] = json!(scope);
        assert_eq!(
            relay.error_code("engine.hello", params).await,
            "UNAUTHORIZED",
            "{scope}"
        );
    }
    let mut params = hello();
    params["scope"] = json!("root");
    assert_eq!(
        relay.error_code("engine.hello", params).await,
        "INVALID_PARAMS"
    );

    // `control` : agir sur les tâches, mais ni planifier ni arrêter le moteur.
    let mut app = TestClient::connect(&engine);
    let mut params = hello();
    params["scope"] = json!("control");
    app.result("engine.hello", params).await;
    app.result("task.resume", json!({"taskIds": [id]})).await;
    app.result("engine.cancelEndAction", json!({})).await;
    let rules = json!({"windows": [{"days": [1], "start": "01:00", "end": "02:00"}]});
    let queue = app.result("queue.list", json!({})).await["items"][0]["queueId"].clone();
    for (method, params) in [
        ("schedule.set", json!({"name": "X", "rules": rules})),
        (
            "queue.update",
            json!({"queueId": queue, "scheduleId": null}),
        ),
        ("engine.shutdown", json!({})),
    ] {
        assert_eq!(
            app.error_code(method, params).await,
            "UNAUTHORIZED",
            "{method}"
        );
    }

    // L'administrateur, lui, voit la tâche ajoutée par le relais.
    let listed = admin.result("task.list", json!({})).await;
    assert_eq!(listed["items"][0]["taskId"], id.as_str());
    engine.shutdown().await;
}

/// Dossier des téléchargements du moteur (`downloads.dir`) : réglé, annoncé, et pris par
/// un ajout sans `destDir` ; `null` revient au dossier du système.
#[tokio::test]
async fn the_download_dir_is_an_engine_setting() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let server = site(1024);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    let system = c.result("settings.get", json!({})).await["values"]["downloads.dir"].clone();

    let chosen = dir_text(downloads.path());
    let set = c
        .result("settings.set", json!({"values": {"downloads.dir": chosen}}))
        .await;
    assert_eq!(set["values"]["downloads.dir"], chosen.as_str());
    let note = c.notification("settings.updated", |_| true).await;
    assert_eq!(note["values"]["downloads.dir"], chosen.as_str());
    let snapshot = c.result("sync.snapshot", json!({})).await;
    assert_eq!(snapshot["settings"]["downloads.dir"], chosen.as_str());

    // Un ajout sans dossier va dans celui du moteur.
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/d.bin")}], "startPaused": true}),
        )
        .await;
    let task = c
        .result("task.get", json!({"taskId": added["taskId"]}))
        .await;
    assert_eq!(task["destDir"], chosen.as_str());

    for (params, code) in [
        (
            json!({"values": {"downloads.dir": "relatif"}}),
            "DEST_NOT_WRITABLE",
        ),
        (json!({"values": {"downloads.dir": 3}}), "INVALID_PARAMS"),
        (json!({"values": {"inconnu": 1}}), "INVALID_PARAMS"),
        (json!({}), "INVALID_PARAMS"),
    ] {
        assert_eq!(
            c.error_code("settings.set", params.clone()).await,
            code,
            "{params}"
        );
    }
    assert_eq!(
        c.error_code(
            "task.add",
            json!({"sources": [{"url": server.url("/e.bin")}], "destDir": "relatif"}),
        )
        .await,
        "DEST_NOT_WRITABLE"
    );

    // Retiré : de nouveau le dossier du système.
    let reset = c
        .result("settings.set", json!({"values": {"downloads.dir": null}}))
        .await;
    assert_eq!(reset["values"]["downloads.dir"], system);
    engine.shutdown().await;
}

/// `idempotencyKey` (SPEC §6.1) : un ajout rejoué rend la même tâche, sans doublon ni
/// annonce ; une fois la tâche retirée, la clé resservirait.
#[tokio::test]
async fn a_replayed_add_returns_the_same_task() {
    let dir = tempfile::tempdir().unwrap();
    let server = site(1024);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let params = |key: &str| {
        json!({
            "sources": [{"url": server.url("/i.bin")}],
            "destDir": dir_text(dir.path()),
            "startPaused": true,
            "idempotencyKey": key,
        })
    };
    let first = c.result("task.add", params("clé-1")).await["taskId"].clone();
    let again = c.result("task.add", params("clé-1")).await["taskId"].clone();
    assert_eq!(first, again);
    let other = c.result("task.add", params("clé-2")).await["taskId"].clone();
    assert_ne!(first, other);
    let listed = c.result("task.list", json!({})).await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 2, "{listed}");
    // Deux ajouts annoncés, pas trois.
    let added = c.notification("task.added", |_| true).await;
    assert_eq!(added["task"]["taskId"], first);
    let added = c.notification("task.added", |_| true).await;
    assert_eq!(added["task"]["taskId"], other);

    // Retirée : la clé est libre.
    c.result("task.cancel", json!({"taskIds": [first]})).await;
    c.result("task.remove", json!({"taskIds": [first]})).await;
    let fresh = c.result("task.add", params("clé-1")).await["taskId"].clone();
    assert_ne!(fresh, first);

    for key in [String::new(), "x".repeat(257)] {
        assert_eq!(
            c.error_code("task.add", params(&key)).await,
            "INVALID_PARAMS"
        );
    }
    engine.shutdown().await;
}

/// `headers` de `task.add` : un objet de textes ; un en-tête réservé, une valeur qui n'est
/// pas un texte sont refusés ; la portée `add` suffit (relais de l'extension).
#[tokio::test]
async fn task_add_accepts_page_headers() {
    let dir = tempfile::tempdir().unwrap();
    let server = site(64 * 1024);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let mut params = hello();
    params["scope"] = json!("add");
    c.result("engine.hello", params).await;
    let add = |headers: Value| {
        json!({
            "sources": [{"url": server.url("/p.bin")}],
            "destDir": dir_text(dir.path()),
            "headers": headers,
        })
    };
    let added = c
        .result(
            "task.add",
            add(json!({"Referer": "https://exemple.org/", "Cookie": "s=1", "User-Agent": "Essai/1"})),
        )
        .await;
    assert!(added["taskId"].is_string(), "{added}");
    for bad in [
        json!({"Range": "bytes=0-"}),
        json!({"Cookie": 3}),
        json!(["Cookie", "s=1"]),
        json!({"Nom invalide": "x"}),
    ] {
        assert_eq!(
            c.error_code("task.add", add(bad.clone())).await,
            "INVALID_PARAMS",
            "{bad}"
        );
    }
    // La tâche ajoutée envoie ses en-têtes.
    let deadline = tokio::time::Instant::now() + WAIT;
    while server.requests().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "aucune requête");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let first = &server.requests()[0];
    assert_eq!(first.header("cookie"), Some("s=1"));
    assert_eq!(first.header("user-agent"), Some("Essai/1"));
    assert_eq!(first.header("referer"), Some("https://exemple.org/"));
    engine.shutdown().await;
}

/// `task.restart` : une tâche terminée repart de zéro ; `task.updated` efface son nom, sa
/// taille et ses octets avant `task.stateChanged` vers `queued`. La portée `add` ne suffit
/// pas.
#[tokio::test]
async fn task_restart_downloads_again() {
    let dir = tempfile::tempdir().unwrap();
    let server = site(64 * 1024);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let id = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/r.bin")}], "destDir": dir_text(dir.path())}),
        )
        .await["taskId"]
        .clone();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id && p["to"] == "completed"
    })
    .await;
    let done = c
        .result(
            "task.restart",
            json!({"taskIds": [id], "deleteFiles": true}),
        )
        .await;
    assert_eq!(done["taskIds"], json!([id]), "{done}");
    let reset = c
        .notification("task.updated", |p| p["taskId"] == id && p["doneBytes"] == 0)
        .await;
    assert!(
        reset["fileName"].is_null() && reset["totalBytes"].is_null(),
        "{reset}"
    );
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id && p["from"] == "completed" && p["to"] == "queued"
    })
    .await;
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id && p["to"] == "completed"
    })
    .await;
    assert!(dir.path().join("r.bin").exists());

    // Un identifiant inconnu figure dans `errors`, sans empêcher les autres.
    let mixed = c
        .result("task.restart", json!({"taskIds": ["inconnue"]}))
        .await;
    assert_eq!(mixed["errors"][0]["code"], "TASK_NOT_FOUND", "{mixed}");

    let mut relay = TestClient::connect(&engine);
    let mut params = hello();
    params["scope"] = json!("add");
    relay.result("engine.hello", params).await;
    assert_eq!(
        relay
            .error_code("task.restart", json!({"taskIds": [id]}))
            .await,
        "UNAUTHORIZED"
    );
    engine.shutdown().await;
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
    // Dates de la tâche, en millisecondes Unix : la fin suit la création.
    let created = task["createdAt"].as_i64().expect("createdAt");
    let finished = task["finishedAt"].as_i64().expect("finishedAt");
    assert!(created > 1_700_000_000_000 && finished >= created, "{task}");
    assert_eq!(
        std::fs::metadata(dir.path().join("f.bin")).unwrap().len(),
        512 * 1024
    );
    engine.shutdown().await;
}

#[tokio::test]
async fn finished_tasks_are_removed_and_announced() {
    let dir = tempfile::tempdir().unwrap();
    let server = site(64 * 1024);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let done = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/fini.bin")}], "destDir": dir_text(dir.path())}),
        )
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let waiting = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/attente.bin")}], "destDir": dir_text(dir.path()),
                   "startPaused": true}),
        )
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == done.as_str() && p["to"] == "completed"
    })
    .await;

    let result = c
        .result(
            "task.remove",
            json!({"taskIds": [done, waiting, "inconnue"], "deleteFiles": false}),
        )
        .await;
    assert_eq!(result["removed"], 1);
    assert_eq!(result["taskIds"], json!([done]));
    let codes: Vec<&str> = result["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["INVALID_STATE", "TASK_NOT_FOUND"]);
    let removed = c.notification("task.removed", |_| true).await;
    assert_eq!(removed["taskId"], done.as_str());
    assert!(removed["rev"].is_u64());
    assert!(dir.path().join("fini.bin").exists(), "fichier gardé");
    assert_eq!(
        c.error_code("task.get", json!({"taskId": done})).await,
        "TASK_NOT_FOUND"
    );
    assert_eq!(
        c.error_code("task.remove", json!({"taskIds": []})).await,
        "INVALID_PARAMS"
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
        // Empreinte (F-11) : valeur trop courte, algorithme inconnu, forme inattendue.
        (
            json!({"checksum": {"algo": "sha256", "value": "00"}}),
            "INVALID_PARAMS",
        ),
        (
            json!({"checksum": {"algo": "crc32", "value": "00000000"}}),
            "INVALID_PARAMS",
        ),
        (json!({"checksum": "sha256:00"}), "INVALID_PARAMS"),
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
    assert_eq!(c.result("sync.snapshot", json!({})).await["limits"], none);
    let sub = c
        .result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    let set = c
        .result("limits.set", json!({"downloadBps": 1_000_000}))
        .await;
    assert_eq!(set["downloadBps"], 1_000_000);
    // Annoncée, avec la révision suivante ; l'instantané la porte.
    let note = c.notification("limits.updated", |_| true).await;
    assert_eq!(note["downloadBps"], 1_000_000);
    assert_eq!(note["rev"], sub["rev"].as_u64().unwrap() + 1);
    let snapshot = c.result("sync.snapshot", json!({})).await;
    assert_eq!(snapshot["limits"]["downloadBps"], 1_000_000);
    assert_eq!(snapshot["rev"], note["rev"]);
    // Paramètre absent : limite inchangée ; `null` : retirée.
    let same = c.result("limits.set", json!({})).await;
    assert_eq!(same["downloadBps"], 1_000_000);
    assert_eq!(
        c.result("limits.set", json!({"downloadBps": null})).await,
        none
    );
    let note = c
        .notification("limits.updated", |p| p["downloadBps"].is_null())
        .await;
    assert!(note["uploadBps"].is_null());
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
        planning: Planning::default(),
    })
    .unwrap()
}

#[tokio::test]
async fn task_get_shows_segments_and_sources_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let len: u64 = 1 << 20;
    let pace = Pace {
        chunk: 16 * 1024,
        delay: Duration::from_millis(40),
    };
    let server = serve_file_paced(Arc::new(Mutex::new(FileSite::new(len))), Some(pace));
    let url = server.url("/carte.bin");
    let mut c = TestClient::connect(&engine);
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"url": url}], "destDir": dir_text(dir.path())}),
        )
        .await;
    let id = added["taskId"].as_str().unwrap().to_owned();
    let both = json!({"taskId": id, "include": ["segments", "sources"]});

    // Pendant le téléchargement : les segments couvrent le fichier, et au moins une
    // connexion reçoit le sien.
    let running = timeout(WAIT, async {
        loop {
            let task = c.result("task.get", both.clone()).await;
            let receiving = task["segments"]
                .as_array()
                .is_some_and(|s| s.iter().any(|s| s["receiving"] == true));
            if receiving {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("aucun segment en cours de réception");
    let segments = running["segments"].as_array().unwrap();
    let covered: u64 = segments
        .iter()
        .map(|s| s["end"].as_u64().unwrap() - s["start"].as_u64().unwrap() + 1)
        .sum();
    assert_eq!(covered, len, "{running}");
    for s in segments {
        let size = s["end"].as_u64().unwrap() - s["start"].as_u64().unwrap() + 1;
        assert!(s["doneBytes"].as_u64().unwrap() <= size, "{s}");
    }
    assert_eq!(
        running["sources"],
        json!([{"url": url, "acceptsRanges": true}])
    );

    // Sans `include`, ni segments ni sources.
    let plain = c.result("task.get", json!({"taskId": id})).await;
    assert!(plain.get("segments").is_none() && plain.get("sources").is_none());
    assert_eq!(
        c.error_code("task.get", json!({"taskId": id, "include": ["files"]}))
            .await,
        "CAPABILITY_MISSING"
    );
    assert_eq!(
        c.error_code("task.get", json!({"taskId": id, "include": ["tout"]}))
            .await,
        "INVALID_PARAMS"
    );

    // Terminée : les segments sont partis avec sa fin ; l'adresse reste (retéléchargement).
    let done = timeout(WAIT, async {
        loop {
            let task = c.result("task.get", both.clone()).await;
            if task["state"] == "completed" {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("jamais terminée");
    assert_eq!(done["segments"], json!([]));
    assert_eq!(done["sources"][0]["url"], json!(url));
    engine.shutdown().await;
}

#[tokio::test]
async fn schedules_are_set_attached_announced_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    let rules = json!({
        "windows": [{"days": [1, 2, 3, 4, 5], "start": "23:00", "end": "07:00"}],
        "onDone": "shutdown",
    });

    let set = c
        .result("schedule.set", json!({"name": "Nuit", "rules": rules}))
        .await;
    let id = set["scheduleId"].as_str().unwrap().to_owned();
    assert_eq!(set["name"], "Nuit");
    assert_eq!(set["enabled"], true, "activée par défaut");
    assert_eq!(set["rules"], rules);
    assert!(set["open"].is_boolean());
    assert!(set["nextChangeAt"].as_i64().is_some(), "{set}");
    let note = c.notification("schedule.updated", |_| true).await;
    assert_eq!(note["schedule"], set);

    let listed = c.result("schedule.list", json!({})).await;
    assert_eq!(listed["items"], json!([set]));

    // Attachée à la file principale : annoncée, et dans l'instantané.
    let queue = c.result("queue.list", json!({})).await["items"][0]["queueId"]
        .as_str()
        .unwrap()
        .to_owned();
    let updated = c
        .result("queue.update", json!({"queueId": queue, "scheduleId": id}))
        .await;
    assert_eq!(updated["scheduleId"], id.as_str());
    let note = c.notification("queue.updated", |_| true).await;
    assert_eq!(note["queue"]["scheduleId"], id.as_str());
    let snapshot = c.result("sync.snapshot", json!({})).await;
    assert_eq!(snapshot["schedules"][0]["scheduleId"], id.as_str());
    assert_eq!(snapshot["queues"][0]["scheduleId"], id.as_str());
    assert_eq!(snapshot["endAction"], Value::Null);

    // Désactivée : toujours ouverte, sans prochain changement.
    let off = c
        .result(
            "schedule.set",
            json!({"scheduleId": id, "name": "Nuit", "enabled": false, "rules": rules}),
        )
        .await;
    assert_eq!(off["open"], true);
    assert_eq!(off["nextChangeAt"], Value::Null);

    // Refus.
    for (method, params, code) in [
        (
            "schedule.set",
            json!({"name": "X", "rules": {"windows": []}}),
            "INVALID_PARAMS",
        ),
        ("schedule.set", json!({"name": "X"}), "INVALID_PARAMS"),
        (
            "schedule.set",
            json!({"scheduleId": "inconnue", "name": "X", "rules": rules}),
            "SCHEDULE_NOT_FOUND",
        ),
        (
            "queue.update",
            json!({"queueId": queue, "scheduleId": "inconnue"}),
            "SCHEDULE_NOT_FOUND",
        ),
        (
            "queue.update",
            json!({"queueId": queue, "scheduleId": 3}),
            "INVALID_PARAMS",
        ),
        (
            "schedule.delete",
            json!({"scheduleId": "inconnue"}),
            "SCHEDULE_NOT_FOUND",
        ),
        (
            "queue.create",
            json!({"name": "Orpheline", "scheduleId": "inconnue"}),
            "SCHEDULE_NOT_FOUND",
        ),
        (
            "queue.update",
            json!({"queueId": queue, "name": "Renommée", "scheduleId": "inconnue"}),
            "SCHEDULE_NOT_FOUND",
        ),
    ] {
        assert_eq!(
            c.error_code(method, params.clone()).await,
            code,
            "{method} {params}"
        );
    }
    // Une demande refusée ne laisse rien : ni file créée, ni nom changé.
    let queues = c.result("queue.list", json!({})).await;
    assert_eq!(queues["items"].as_array().unwrap().len(), 1, "{queues}");
    assert_ne!(queues["items"][0]["name"], "Renommée");
    assert_eq!(
        c.result("engine.cancelEndAction", json!({})).await,
        json!({"cancelled": false})
    );

    // Supprimée : annoncée, et la file la perd.
    c.result("schedule.delete", json!({"scheduleId": id})).await;
    let note = c.notification("schedule.removed", |_| true).await;
    assert_eq!(note["scheduleId"], id.as_str());
    let note = c
        .notification("queue.updated", |p| p["queue"]["scheduleId"].is_null())
        .await;
    assert_eq!(note["queue"]["queueId"], queue.as_str());
    assert_eq!(
        c.result("schedule.list", json!({})).await,
        json!({"items": []})
    );
    engine.shutdown().await;
}

/// « Déplacer/Renommer » et « Actualiser l'adresse » par `task.update` (SPEC §7.12 et
/// §7.13), et `fileName` de `task.add`.
#[tokio::test]
async fn task_update_moves_renames_and_replaces_the_address() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("ailleurs");
    std::fs::create_dir(&elsewhere).unwrap();
    let server = site(64 * 1024);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let id = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/r.bin")}], "destDir": dir_text(dir.path()),
                   "fileName": "donné.bin"}),
        )
        .await["taskId"]
        .clone();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id && p["to"] == "completed"
    })
    .await;
    assert!(dir.path().join("donné.bin").exists());

    let moved = c
        .result(
            "task.update",
            json!({"taskId": id, "destDir": dir_text(&elsewhere), "fileName": "rangé.bin"}),
        )
        .await;
    assert_eq!(moved["destDir"], json!(dir_text(&elsewhere)));
    assert_eq!(moved["fileName"], "rangé.bin");
    assert_eq!(moved["moving"], false);
    let note = c
        .notification("task.updated", |p| {
            p["taskId"] == id && p["destDir"].is_string()
        })
        .await;
    assert_eq!(note["fileName"], "rangé.bin");
    assert_eq!(note["displayName"], "rangé.bin");
    assert_eq!(note["moving"], false);
    assert!(elsewhere.join("rangé.bin").exists());

    std::fs::write(elsewhere.join("pris.bin"), b"x").unwrap();
    let refused = [
        (json!({"taskId": id, "fileName": "pris.bin"}), "FILE_EXISTS"),
        (json!({"taskId": id, "fileName": "a/b"}), "INVALID_PARAMS"),
        (
            json!({"taskId": id, "fileName": "c.bin", "url": server.url("/s.bin")}),
            "INVALID_PARAMS",
        ),
        (json!({"taskId": id, "url": "ftp://h/f"}), "URL_INVALID"),
    ];
    for (params, code) in refused {
        assert_eq!(
            c.error_code("task.update", params.clone()).await,
            code,
            "{params}"
        );
    }

    // Tâche commencée : la nouvelle adresse doit donner le même fichier.
    let size = 1024 * 1024;
    let slow = serve_file_paced(
        Arc::new(Mutex::new(FileSite::new(size))),
        Some(Pace {
            chunk: 16 * 1024,
            delay: Duration::from_millis(10),
        }),
    );
    let started = c
        .result(
            "task.add",
            json!({"sources": [{"url": slow.url("/l.bin")}], "destDir": dir_text(dir.path())}),
        )
        .await["taskId"]
        .clone();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == started && p["to"] == "active"
    })
    .await;
    c.result("task.pause", json!({"taskIds": [started]})).await;
    c.notification("task.stateChanged", |p| {
        p["taskId"] == started && p["to"] == "paused"
    })
    .await;
    let other = site(1000);
    let cases = [
        (other.url("/x"), "SOURCE_MISMATCH"),
        ("http://127.0.0.1:1/x".to_owned(), "SOURCE_UNREACHABLE"),
    ];
    for (url, code) in cases {
        let params = json!({"taskId": started, "url": url});
        assert_eq!(c.error_code("task.update", params).await, code, "{url}");
    }
    let same = site(size);
    c.result(
        "task.update",
        json!({"taskId": started, "url": same.url("/l.bin")}),
    )
    .await;
    let detail = c
        .result(
            "task.get",
            json!({"taskId": started, "include": ["sources"]}),
        )
        .await;
    assert_eq!(detail["sources"][0]["url"], json!(same.url("/l.bin")));
    c.result("task.resume", json!({"taskIds": [started]})).await;
    c.notification("task.stateChanged", |p| {
        p["taskId"] == started && p["to"] == "completed"
    })
    .await;
    engine.shutdown().await;
}

/// Identifiants de sites (SPEC §7.14) : portée `admin`, mot de passe jamais rendu.
#[tokio::test]
async fn site_logins_are_managed_by_admins_and_never_show_their_password() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    let saved = c
        .result(
            "login.set",
            json!({"site": "https://Exemple.org/", "username": "moi", "password": "s3cr3t"}),
        )
        .await;
    assert_eq!(saved["site"], "https://exemple.org");
    assert_eq!(saved["username"], "moi");
    let id = saved["loginId"].clone();
    let list = c.result("login.list", json!({})).await;
    assert_eq!(list["items"], json!([saved]));
    assert!(!list.to_string().contains("s3cr3t"));
    // Sans mot de passe : le nom change, le mot de passe reste.
    let renamed = c
        .result(
            "login.set",
            json!({"loginId": id, "site": "exemple.org", "username": "toi"}),
        )
        .await;
    assert_eq!(renamed["site"], "exemple.org");
    assert_eq!(
        c.error_code("login.set", json!({"site": "exemple.org", "username": "x"}))
            .await,
        "INVALID_PARAMS"
    );
    assert_eq!(
        c.error_code(
            "login.set",
            json!({"loginId": "inconnu", "site": "a.org", "username": "x"})
        )
        .await,
        "LOGIN_NOT_FOUND"
    );

    let mut control = TestClient::connect(&engine);
    let mut params = hello();
    params["scope"] = json!("control");
    control.result("engine.hello", params).await;
    for method in ["login.list", "login.set", "login.delete"] {
        assert_eq!(
            control.error_code(method, json!({})).await,
            "UNAUTHORIZED",
            "{method}"
        );
    }

    c.result("login.delete", json!({"loginId": id})).await;
    assert_eq!(c.result("login.list", json!({})).await["items"], json!([]));
    assert_eq!(
        c.error_code("login.delete", json!({"loginId": id})).await,
        "LOGIN_NOT_FOUND"
    );
    engine.shutdown().await;
}

/// Empreinte (F-11) : `checksum` de `task.add` et `task.update`, montrée par `task.get`,
/// annoncée par `task.updated` ; la vérification passe par `verifying` (son avancement,
/// `verifiedBytes` : test de `session`).
#[tokio::test]
async fn checksums_are_given_checked_and_announced() {
    use sha2::{Digest, Sha256};
    let len = 512 * 1024;
    let dir = tempfile::tempdir().unwrap();
    let server = site(len);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result(
        "events.subscribe",
        json!({"topics": ["tasks", "progress"], "progressIntervalMs": 50}),
    )
    .await;
    let hex: String = Sha256::digest(crate::testing::content(len as usize))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/f.bin")}], "destDir": dir_text(dir.path()),
                   "checksum": {"algo": "SHA256", "value": hex.to_uppercase()}}),
        )
        .await;
    let id = added["taskId"].as_str().unwrap().to_owned();
    let checking = c
        .notification("task.stateChanged", |p| p["to"] == "verifying")
        .await;
    assert_eq!(checking["from"], "active");
    let done = c
        .notification("task.stateChanged", |p| p["to"] == "completed")
        .await;
    assert_eq!(done["from"], "verifying");
    let task = c.result("task.get", json!({"taskId": id})).await;
    assert_eq!(
        task["checksum"],
        json!({"algo": "sha256", "value": hex, "source": null})
    );
    assert_eq!(
        std::fs::metadata(dir.path().join("f.bin")).unwrap().len(),
        len
    );

    // Tâche sans empreinte, en pause : on la donne, puis on la retire.
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/g.bin")}], "destDir": dir_text(dir.path()),
                   "startPaused": true}),
        )
        .await;
    let id = added["taskId"].as_str().unwrap().to_owned();
    assert!(c.result("task.get", json!({"taskId": id})).await["checksum"].is_null());
    let md5 = json!({"algo": "md5", "value": "d41d8cd98f00b204e9800998ecf8427e"});
    // Rendue avec son origine : `null`, donnée par l'utilisateur.
    let mut md5_out = md5.clone();
    md5_out["source"] = Value::Null;
    let updated = c
        .result("task.update", json!({"taskId": id, "checksum": md5}))
        .await;
    assert_eq!(updated["checksum"], md5_out);
    let note = c
        .notification("task.updated", |p| p["taskId"] == json!(id))
        .await;
    assert_eq!(note["checksum"], md5_out);
    let cleared = c
        .result("task.update", json!({"taskId": id, "checksum": null}))
        .await;
    assert!(cleared["checksum"].is_null());
    let note = c
        .notification("task.updated", |p| p["taskId"] == json!(id))
        .await;
    assert!(note.as_object().unwrap().contains_key("checksum"));
    assert!(note["checksum"].is_null());
    assert_eq!(
        c.error_code(
            "task.update",
            json!({"taskId": id, "checksum": md5, "speedLimitBps": 10})
        )
        .await,
        "INVALID_PARAMS"
    );
    assert_eq!(
        c.error_code(
            "task.update",
            json!({"taskId": id, "checksum": {"algo": "md5", "value": "xyz"}})
        )
        .await,
        "INVALID_PARAMS"
    );
    engine.shutdown().await;
}

/// Historique (F-35) : une tâche terminée y entre, avec son empreinte SHA-256 vérifiée ;
/// une tâche annulée y entre à son retrait ; recherche, pages et effacement ; la portée
/// `read` lit sans effacer.
#[tokio::test]
async fn history_is_listed_searched_paged_and_cleared() {
    use sha2::{Digest, Sha256};
    let len = 64 * 1024;
    let dir = tempfile::tempdir().unwrap();
    let server = site(len);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let hex: String = Sha256::digest(crate::testing::content(len as usize))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    for name in ["alpha.bin", "beta.bin"] {
        let mut params = json!({"sources": [{"url": server.url(&format!("/{name}"))}],
                                "destDir": dir_text(dir.path())});
        if name == "alpha.bin" {
            params["checksum"] = json!({"algo": "sha256", "value": hex});
        }
        let id = c.result("task.add", params).await["taskId"]
            .as_str()
            .unwrap()
            .to_owned();
        c.notification("task.stateChanged", |p| {
            p["taskId"] == json!(id) && p["to"] == "completed"
        })
        .await;
    }
    // Une tâche annulée puis retirée : elle aussi a sa ligne.
    let cancelled = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/gamma.bin")}], "destDir": dir_text(dir.path()),
                   "startPaused": true}),
        )
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.result("task.cancel", json!({"taskIds": [cancelled]}))
        .await;
    c.result("task.remove", json!({"taskIds": [cancelled]}))
        .await;

    let all = c.result("history.list", json!({})).await;
    let items = all["items"].as_array().unwrap();
    let names: Vec<&str> = items
        .iter()
        .map(|i| i["displayName"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 3, "{all}");
    assert_eq!(names[0], "gamma.bin", "les plus récentes d'abord");
    let alpha = items
        .iter()
        .find(|i| i["displayName"] == "alpha.bin")
        .unwrap();
    assert_eq!(alpha["finalState"], "completed");
    assert_eq!(alpha["sha256"], json!(hex));
    assert_eq!(alpha["totalBytes"], len);
    assert_eq!(alpha["sourceHost"], "127.0.0.1");
    assert!(alpha["finalPath"].as_str().unwrap().ends_with("alpha.bin"));
    assert!(alpha["finishedAt"].as_i64().unwrap() > 1_700_000_000_000);
    assert_eq!(items[0]["finalState"], "cancelled");
    assert!(all["nextCursor"].is_null());

    // Recherche dans le nom, sans égard à la casse.
    let found = c.result("history.list", json!({"query": "BETA"})).await;
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
    assert_eq!(found["items"][0]["displayName"], "beta.bin");

    // Pages de deux, puis d'une.
    let first = c.result("history.list", json!({"limit": 2})).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let second = c
        .result("history.list", json!({"limit": 2, "cursor": cursor}))
        .await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second["nextCursor"].is_null());
    for params in [
        json!({"cursor": "rien"}),
        json!({"limit": 0}),
        json!({"before": "hier"}),
    ] {
        assert_eq!(
            c.error_code("history.list", params.clone()).await,
            "INVALID_PARAMS",
            "{params}"
        );
    }

    // Lecture seule : lire oui, effacer non.
    let mut reader = TestClient::connect(&engine);
    let mut params = hello();
    params["scope"] = json!("read");
    reader.result("engine.hello", params).await;
    assert_eq!(
        reader.result("history.list", json!({})).await["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        reader.error_code("history.clear", json!({})).await,
        "UNAUTHORIZED"
    );

    // Effacement de ce qui a fini avant la dernière entrée, puis de tout ; les tâches et
    // les fichiers restent.
    let newest = items[0]["finishedAt"].as_i64().unwrap();
    let cleared = c.result("history.clear", json!({"before": newest})).await;
    assert_eq!(cleared["removed"], 2);
    let cleared = c.result("history.clear", json!({})).await;
    assert_eq!(cleared["removed"], 1);
    assert!(
        c.result("history.list", json!({})).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        c.result("task.list", json!({})).await["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(dir.path().join("alpha.bin").exists());
    engine.shutdown().await;
}

/// Réservation de la place (F-10) : réglage `downloads.preallocate`, vrai par défaut,
/// changé par `settings.set` et annoncé par `settings.updated`.
#[tokio::test]
async fn preallocation_is_an_engine_setting_on_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let got = c.result("settings.get", json!({})).await;
    assert_eq!(got["values"]["downloads.preallocate"], true);
    let snapshot = c.result("sync.snapshot", json!({})).await;
    assert_eq!(snapshot["settings"]["downloads.preallocate"], true);
    c.result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    let set = c
        .result(
            "settings.set",
            json!({"values": {"downloads.preallocate": false}}),
        )
        .await;
    assert_eq!(set["values"]["downloads.preallocate"], false);
    let note = c
        .notification("settings.updated", |p| {
            p["values"].get("downloads.preallocate").is_some()
        })
        .await;
    assert_eq!(note["values"]["downloads.preallocate"], false);
    // Une valeur fausse ne change rien, pas même l'autre réglage de l'appel.
    assert_eq!(
        c.error_code(
            "settings.set",
            json!({"values": {"downloads.preallocate": "oui", "downloads.dir": null}})
        )
        .await,
        "INVALID_PARAMS"
    );
    let got = c.result("settings.get", json!({})).await;
    assert_eq!(got["values"]["downloads.preallocate"], false);
    engine.shutdown().await;
}

/// Empreinte lue dans un fichier voisin (F-11) : réglage `downloads.neighborChecksum`, vrai
/// par défaut ; l'empreinte trouvée est annoncée avec le fichier qui la donne.
#[tokio::test]
async fn neighbor_checksums_follow_the_setting_and_are_announced_with_their_origin() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(Config {
        db_path: dir.path().join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options {
            neighbor_checksum: true,
            ..Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap();
    let len: u64 = 200_000;
    let hex = crate::testing::content_sha256(len);
    let site = Arc::new(Mutex::new(FileSite::new(len)));
    for name in ["n.bin", "m.bin"] {
        site.lock()
            .unwrap()
            .neighbors
            .push((format!("/{name}.sha256"), format!("{hex}\n").into_bytes()));
    }
    let server = serve_file(Arc::clone(&site));
    let mut c = TestClient::connect(&engine);
    let key = "downloads.neighborChecksum";
    assert_eq!(
        c.result("settings.get", json!({})).await["values"][key],
        true
    );
    assert_eq!(
        c.result("sync.snapshot", json!({})).await["settings"][key],
        true
    );
    c.result("events.subscribe", json!({"topics": ["tasks", "queues"]}))
        .await;

    let add = |name: &str| {
        json!({"sources": [{"url": server.url(&format!("/{name}"))}],
               "destDir": dir_text(dir.path())})
    };
    let id = c.result("task.add", add("n.bin")).await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let expected = json!({"algo": "sha256", "value": hex, "source": "n.bin.sha256"});
    let note = c
        .notification("task.updated", |p| {
            p["taskId"] == json!(id) && p.get("checksum").is_some()
        })
        .await;
    assert_eq!(note["checksum"], expected);
    c.notification("task.stateChanged", |p| {
        p["taskId"] == json!(id) && p["to"] == "completed"
    })
    .await;
    let task = c.result("task.get", json!({"taskId": id})).await;
    assert_eq!(task["checksum"], expected);

    // Réglage coupé : le fichier suivant n'a pas d'empreinte, aucun voisin n'est demandé.
    let set = c
        .result("settings.set", json!({"values": {key: false}}))
        .await;
    assert_eq!(set["values"][key], false);
    let note = c
        .notification("settings.updated", |p| p["values"].get(key).is_some())
        .await;
    assert_eq!(note["values"][key], false);
    let id = c.result("task.add", add("m.bin")).await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == json!(id) && p["to"] == "completed"
    })
    .await;
    assert!(c.result("task.get", json!({"taskId": id})).await["checksum"].is_null());
    let asked = site.lock().unwrap().neighbor_requests.clone();
    assert!(asked.iter().all(|p| !p.starts_with("/m.bin")), "{asked:?}");
    assert_eq!(asked.len(), 6, "{asked:?}");
    engine.shutdown().await;
}
