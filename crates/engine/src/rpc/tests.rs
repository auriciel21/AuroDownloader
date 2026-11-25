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

use crate::net::Client;
use crate::scheduler::{Config, DISK_RECHECK, Engine, Planning};
use crate::testing::{FileSite, Pace, Server, serve_file, serve_file_paced};
use crate::transfer::Options;

const WAIT: Duration = Duration::from_secs(30);

fn engine(dir: &Path) -> Engine {
    Engine::start(engine_config(dir)).unwrap()
}

fn engine_config(dir: &Path) -> Config {
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
        Self::connect_as(engine, super::Peer::local())
    }

    /// Connexion d'un client donné (connexion par jeton, SPEC §7.19).
    fn connect_as(engine: &Engine, peer: super::Peer) -> Self {
        let (ours, theirs) = duplex(1 << 20);
        let (read, write) = split(theirs);
        tokio::spawn(super::serve_as(engine.clone(), peer, read, write));
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
        let message: Value = serde_json::from_str(&line).unwrap();
        // Contrat du protocole (SPEC §9.1) : toute notification suit son schéma.
        if message.get("method").is_some() && message.get("id").is_none() {
            super::contract::check_notification(&message);
        }
        Some(message)
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
        // Contrat du protocole (SPEC §9.1) : tout résultat, toute erreur suit son schéma.
        super::contract::check_reply(method, &reply);
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    // Audit R17 : une page vide n'a pas de sens.
    assert_eq!(
        c.error_code("task.list", json!({"limit": 0})).await,
        "INVALID_PARAMS"
    );
    // Audit R6 : un dossier refusé ne laisse pas les autres réglages changés.
    let before =
        c.result("settings.get", json!({})).await["values"]["downloads.preallocate"].clone();
    assert!(
        !c.error_code(
            "settings.set",
            json!({"values": {"downloads.preallocate": before == false, "downloads.dir": "relatif"}}),
        )
        .await
        .is_empty()
    );
    let after =
        c.result("settings.get", json!({})).await["values"]["downloads.preallocate"].clone();
    assert_eq!(after, before, "réglage changé malgré le refus");
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

    // Audit R8 : une requête invalide, même sans `id`, reçoit -32600 avec `id` à null.
    c.send_raw(r#"{"method": "task.list"}"#).await;
    let reply = c.reply().await;
    assert_eq!(reply["error"]["code"], -32600, "{reply}");
    assert_eq!(reply["id"], Value::Null);
    c.send_raw(r#"{"jsonrpc": "2.0", "id": {"objet": 1}, "method": "task.list"}"#)
        .await;
    let reply = c.reply().await;
    assert_eq!(reply["error"]["code"], -32600, "{reply}");
    assert_eq!(
        reply["id"],
        Value::Null,
        "un id illisible n'est pas renvoyé"
    );

    // Audit R11 : de l'UTF-8 invalide est illisible (-32700), jamais corrigé en silence.
    c.writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"task.get\",\"params\":{\"taskId\":\"\xff\"}}\n")
        .await
        .unwrap();
    let reply = c.reply().await;
    assert_eq!(reply["error"]["code"], -32700, "{reply}");
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
}

/// Audit R4 : le dernier avancement d'une tâche part juste avant son changement d'état,
/// au lieu d'être perdu (fin) ou de suivre l'état (pause). Avancement envoyé toutes les
/// 10 min ici : seul ce départ anticipé peut le faire arriver.
#[tokio::test]
async fn the_last_progress_comes_before_the_state_change() {
    let dir = tempfile::tempdir().unwrap();
    let server = site(512 * 1024);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result(
        "events.subscribe",
        json!({"topics": ["tasks", "progress"], "progressIntervalMs": 600_000}),
    )
    .await;
    let id = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/f.bin")}], "destDir": dir_text(dir.path())}),
        )
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut previous = Value::Null;
    loop {
        let message = c.read().await.expect("connexion ouverte");
        if message["method"] == "task.stateChanged"
            && message["params"]["taskId"] == id.as_str()
            && message["params"]["to"] == "completed"
        {
            assert_eq!(previous["method"], "task.progress", "{previous}");
            let item = &previous["params"]["items"][0];
            assert_eq!(item["taskId"], id.as_str());
            assert_eq!(item["doneBytes"], 512 * 1024);
            assert!(previous["params"]["rev"].as_u64() < message["params"]["rev"].as_u64());
            break;
        }
        previous = message;
    }
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    // Les deux se préparent en même temps : leurs annonces arrivent dans un ordre
    // quelconque (`notification` écarte celles qui ne conviennent pas).
    let mut names = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    while seen.len() < ids.len() {
        let note = c
            .notification("task.updated", |p| {
                ids.iter().any(|id| p["taskId"] == id.as_str())
                    && !seen.iter().any(|s| p["taskId"] == s.as_str())
            })
            .await;
        let id = note["taskId"].as_str().unwrap().to_owned();
        seen.push(id.clone());
        assert_eq!(note["totalBytes"], 256 * 1024);
        assert_eq!(note["displayName"], note["fileName"]);
        let task = c.result("task.get", json!({"taskId": id})).await;
        assert_eq!(note["fileName"], task["fileName"], "le nom enregistré");
        names.push(note["fileName"].as_str().unwrap().to_owned());
    }
    names.sort();
    assert_eq!(names, ["meme (1).bin", "meme.bin"]);
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
        // Plusieurs sources : des miroirs (SPEC §7.7) depuis la phase 6, tous des adresses
        // HTTP(S).
        (
            json!({"sources": [{"url": "http://a/f"}, {"url": "ftp://b/f"}]}),
            "URL_INVALID",
        ),
        (json!({"sources": [{"url": "gopher://h/f"}]}), "URL_INVALID"),
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
        // Débit d'envoi (torrents) : pris en charge depuis le 2026-11-09 ; 0 refusé.
        (json!({"uploadBps": 0}), "INVALID_PARAMS"),
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&other).await;

    crate::testing::stop(&engine).await;
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
    let sources = running["sources"].as_array().unwrap();
    for s in segments {
        let size = s["end"].as_u64().unwrap() - s["start"].as_u64().unwrap() + 1;
        assert!(s["doneBytes"].as_u64().unwrap() <= size, "{s}");
        // Un segment en cours de réception dit d'où il vient (depuis le 2026-11-11).
        if s["receiving"] == true {
            assert_eq!(s["sourceId"], sources[0]["sourceId"], "{s}");
        } else {
            assert!(s.get("sourceId").is_none(), "{s}");
        }
    }
    assert_eq!(sources.len(), 1, "{running}");
    assert_eq!(sources[0]["url"], json!(url));
    assert_eq!(sources[0]["acceptsRanges"], true);
    assert_eq!(sources[0]["rank"], 0);
    assert_eq!(sources[0]["state"], "ok");
    assert_eq!(sources[0]["failCount"], 0);

    // Sans `include`, ni segments ni sources.
    let plain = c.result("task.get", json!({"taskId": id})).await;
    assert!(plain.get("segments").is_none() && plain.get("sources").is_none());
    // Fichiers d'un torrent (depuis le 2026-11-09) : aucun pour une tâche HTTP.
    let files = c
        .result("task.get", json!({"taskId": id, "include": ["files"]}))
        .await;
    assert_eq!(files["files"], json!([]), "{files}");
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
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
    crate::testing::stop(&engine).await;
}

/// Réseau utilisable ou non (SPEC §7.15) : dit par l'application, annoncé, dans
/// l'instantané.
#[tokio::test]
async fn the_network_state_is_set_announced_and_in_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    assert_eq!(
        c.result("sync.snapshot", json!({})).await["networkUsable"],
        true
    );
    c.result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    let set = c
        .result("engine.setNetwork", json!({"usable": false}))
        .await;
    assert_eq!(set, json!({"usable": false, "metered": false}));
    let note = c.notification("engine.network", |_| true).await;
    assert_eq!(note["usable"], false);
    assert!(note["rev"].as_u64().is_some());
    assert_eq!(
        c.result("sync.snapshot", json!({})).await["networkUsable"],
        false
    );
    assert_eq!(
        c.error_code("engine.setNetwork", json!({"usable": "non"}))
            .await,
        "INVALID_PARAMS"
    );
    crate::testing::stop(&engine).await;
}

/// Torrent (SPEC §7.16) : `sources[0].torrent`, un fichier `.torrent` en base64, fait une
/// tâche `torrent` ; le moteur annonce la capacité.
#[tokio::test]
async fn a_torrent_file_in_base64_makes_a_torrent_task() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let hello = c.result("engine.hello", hello()).await;
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("torrent"))
    );

    let mut metainfo =
        b"d4:infod6:lengthi3e4:name5:x.bin12:piece lengthi16384e6:pieces20:".to_vec();
    metainfo.extend([7u8; 20]);
    metainfo.extend_from_slice(b"ee");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&metainfo);
    let dest = dir_text(dir.path());
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"torrent": encoded}], "destDir": dest, "startPaused": true}),
        )
        .await;
    let task = c
        .result("task.get", json!({"taskId": added["taskId"]}))
        .await;
    assert_eq!(task["kind"], "torrent", "{task}");
    assert_eq!(task["displayName"], "x.bin", "{task}");

    for bad in [
        json!({"sources": [{"torrent": "%%%"}], "destDir": dest}),
        json!({"sources": [{"torrent": encoded, "url": "https://a.b/c"}], "destDir": dest}),
        json!({"sources": [{"torrent": base64::engine::general_purpose::STANDARD.encode(b"non")}], "destDir": dest}),
        json!({"sources": [{"url": "magnet:?dn=sans-empreinte"}], "destDir": dest}),
    ] {
        assert_eq!(
            c.error_code("task.add", bad.clone()).await,
            "INVALID_PARAMS",
            "{bad}"
        );
    }
    crate::testing::stop(&engine).await;
}

/// Contrat du protocole (SPEC §9.1) : ce que les autres tests n'atteignent pas par le
/// protocole est appelé ici, et validé par son schéma : choix des fichiers d'un torrent,
/// interfaces réseau, fin d'abonnement, annonce et exécution d'une action de fin.
#[tokio::test]
async fn the_rest_of_the_protocol_follows_its_contract() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(Config {
        planning: Planning {
            check_every: Duration::from_millis(20),
            end_delay: Duration::from_millis(200),
            ..Planning::default()
        },
        ..engine_config(dir.path())
    })
    .unwrap();
    let server = site(64 * 1024);
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks", "queues"]}))
        .await;

    // Torrent de deux fichiers, en pause : ses fichiers, puis un seul choisi.
    let mut metainfo = b"d4:infod5:filesld6:lengthi3e4:pathl5:a.binee\
        d6:lengthi4e4:pathl5:b.bineee4:name3:lot12:piece lengthi16384e6:pieces20:"
        .to_vec();
    metainfo.extend([7u8; 20]);
    metainfo.extend_from_slice(b"ee");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&metainfo);
    let dest = dir_text(dir.path());
    let id = c
        .result(
            "task.add",
            json!({"sources": [{"torrent": encoded}], "destDir": dest, "startPaused": true}),
        )
        .await["taskId"]
        .clone();
    // Fichiers pas encore connus (métadonnées attendues) : refusé, selon le contrat des
    // erreurs ; le résultat de `task.setFiles` est validé par le test des torrents.
    c.result("task.get", json!({"taskId": id, "include": ["files"]}))
        .await;
    assert_eq!(
        c.error_code("task.setFiles", json!({"taskId": id, "selected": [1]}))
            .await,
        "INVALID_PARAMS"
    );
    c.result("task.remove", json!({"taskIds": [id], "deleteFiles": true}))
        .await;
    c.result("torrent.interfaces", json!({})).await;

    // Action de fin : une planification ouverte à toute heure, la veille à la fin (sans
    // effet dans les tests) ; un téléchargement fini l'annonce, puis elle s'exécute.
    let rules = json!({
        "windows": [{"days": [1, 2, 3, 4, 5, 6, 7], "start": "00:00", "end": "00:00"}],
        "onDone": "sleep",
    });
    let schedule = c
        .result("schedule.set", json!({"name": "Toujours", "rules": rules}))
        .await;
    let queue = c.result("queue.list", json!({})).await["items"][0]["queueId"].clone();
    c.result(
        "queue.update",
        json!({"queueId": queue, "scheduleId": schedule["scheduleId"]}),
    )
    .await;
    c.result(
        "task.add",
        json!({"sources": [{"url": server.url("/fin.bin")}], "destDir": dest}),
    )
    .await;
    let announced = c
        .notification("engine.endAction", |p| !p["pending"].is_null())
        .await;
    assert_eq!(announced["pending"]["action"], "sleep");
    c.notification("engine.endAction", |p| p["pending"].is_null())
        .await;
    c.result("events.unsubscribe", json!({})).await;
    crate::testing::stop(&engine).await;
}

/// Proxy (F-07) : `proxy.set` le change pour les téléchargements suivants ; le mot de
/// passe va au coffre, jamais en base ni dans une réponse ; des réglages refusés ne
/// changent rien ; réservé au canal local.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downloads_follow_the_proxy_and_its_password_stays_in_the_vault() {
    use crate::testing::proxies::http_proxy;
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let server = site(8 * 1024);
    let relay = http_proxy();
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    assert_eq!(
        c.result("proxy.get", json!({})).await,
        json!({"mode": "system", "url": null, "exceptions": [], "username": null, "hasPassword": false})
    );
    let set = c
        .result(
            "proxy.set",
            json!({"mode": "manual", "url": relay.url("http"), "username": "moi", "password": "mot-de-passe-du-proxy"}),
        )
        .await;
    assert_eq!(set["hasPassword"], true);
    assert!(!set.to_string().contains("mot-de-passe-du-proxy"));

    // Un téléchargement passe par le proxy, avec l'identifiant.
    let add = |name: &str| json!({"sources": [{"url": server.url(name)}], "destDir": dir_text(dir.path())});
    let id = c.result("task.add", add("/par-proxy.bin")).await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id.as_str() && p["to"] == "completed"
    })
    .await;
    let seen = relay.seen();
    assert!(
        seen.iter().any(|s| s.target.ends_with("/par-proxy.bin")),
        "{seen:?}"
    );
    assert!(seen.iter().all(|s| s.credentials.is_some()));

    // Ni en base, ni ailleurs que dans le coffre.
    let db = std::fs::read(dir.path().join("auro.db")).unwrap();
    assert!(!db.windows(21).any(|w| w == b"mot-de-passe-du-proxy"));
    let wal = std::fs::read(dir.path().join("auro.db-wal")).unwrap_or_default();
    assert!(!wal.windows(21).any(|w| w == b"mot-de-passe-du-proxy"));

    // Mot de passe gardé quand il n'est pas redonné ; refus sans rien changer.
    let kept = c
        .result(
            "proxy.set",
            json!({"mode": "manual", "url": relay.url("http"), "username": "moi", "exceptions": ["exemple.lan"]}),
        )
        .await;
    assert_eq!(kept["hasPassword"], true);
    for bad in [
        json!({"mode": "manual"}),
        json!({"mode": "manual", "url": "ftp://p.lan:21"}),
        json!({"mode": "partout"}),
        json!({"mode": "manual", "url": relay.url("http"), "password": 3}),
    ] {
        assert_eq!(
            c.error_code("proxy.set", bad.clone()).await,
            "INVALID_PARAMS",
            "{bad}"
        );
    }
    assert_eq!(c.result("proxy.get", json!({})).await, kept);

    // Aucun : le téléchargement suivant va en direct.
    c.result("proxy.set", json!({"mode": "none"})).await;
    let none = c.result("proxy.get", json!({})).await;
    assert_eq!(
        none["hasPassword"], false,
        "sans identifiant, plus de mot de passe"
    );
    let before = relay.seen().len();
    let id = c.result("task.add", add("/en-direct.bin")).await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id.as_str() && p["to"] == "completed"
    })
    .await;
    assert_eq!(relay.seen().len(), before);

    // Réservé au canal local.
    let mut remote = TestClient::connect(&engine);
    let mut params = hello();
    params["scope"] = json!("control");
    remote.result("engine.hello", params).await;
    assert_eq!(
        remote.error_code("proxy.get", json!({})).await,
        "UNAUTHORIZED"
    );
    crate::testing::stop(&engine).await;
}

/// Règles de rangement (F-32, SPEC §7.21) : une tâche ajoutée sans dossier naît dans le
/// dossier et sous le nom de la première règle qui la prend ; un dossier choisi l'emporte ;
/// un jeton lit les règles sans leurs commandes et ne les écrit pas.
#[tokio::test]
async fn rules_place_tasks_added_without_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let server = site(4096);
    let downloads = dir.path().join("dl");
    let elsewhere = dir.path().join("ailleurs").join("binaires");
    std::fs::create_dir(&downloads).unwrap();
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    c.result(
        "settings.set",
        json!({"values": {"downloads.dir": dir_text(&downloads)}}),
    )
    .await;
    assert_eq!(
        c.result("rules.list", json!({})).await,
        json!({"rules": []})
    );
    #[cfg(windows)]
    let program = r"C:\Outils\analyse.exe";
    #[cfg(not(windows))]
    let program = "/usr/bin/analyse";
    let set = c
        .result(
            "rules.set",
            json!({"rules": [
                {"name": "Éteinte", "enabled": false, "actions": {"folder": "Jamais"}},
                {"name": "Images", "match": {"categories": ["images"], "urlRegex": "^http://127\\.0\\.0\\.1:"},
                 "actions": {"folder": "Images/Sites", "rename": "{hôte}-{nom}.{ext}"}},
                {"name": "Binaires", "match": {"extensions": ["bin"], "urlRegex": "/b/"},
                 "actions": {"folder": dir_text(&elsewhere),
                             "command": {"program": program, "args": ["{chemin}"]}}}
            ]}),
        )
        .await;
    let rules = set["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 3);
    assert!(
        rules
            .iter()
            .all(|r| r["ruleId"].as_str().is_some_and(|id| !id.is_empty()))
    );
    assert_eq!(rules[2]["actions"]["command"]["timeoutS"], 600);
    assert_eq!(c.result("rules.list", json!({})).await, set);

    // Ajoute, attend la fin (terminée, ou en échec), rend l'emplacement du fichier et la
    // tâche. La préparation annonce le dossier de la règle avec le nom : l'interface montre
    // l'emplacement réel sans relire la tâche.
    async fn placed(c: &mut TestClient, params: Value) -> (std::path::PathBuf, Value) {
        let id = c.result("task.add", params).await["taskId"]
            .as_str()
            .unwrap()
            .to_owned();
        let prepared = c
            .notification("task.updated", |p| {
                p["taskId"] == id.as_str() && p.get("fileName").is_some_and(Value::is_string)
            })
            .await;
        c.notification("task.stateChanged", |p| {
            p["taskId"] == id.as_str() && (p["to"] == "completed" || p["to"] == "failed")
        })
        .await;
        let task = c.result("task.get", json!({"taskId": id})).await;
        assert_eq!(prepared["destDir"], task["destDir"], "{prepared}");
        assert_eq!(prepared["fileName"], task["fileName"], "{prepared}");
        let path =
            Path::new(task["destDir"].as_str().unwrap()).join(task["fileName"].as_str().unwrap());
        (path, task)
    }
    async fn place(c: &mut TestClient, params: Value) -> std::path::PathBuf {
        let (path, task) = placed(c, params).await;
        assert_eq!(task["state"], "completed", "{task}");
        path
    }
    let auto = |path: &str| json!({"sources": [{"url": server.url(path)}]});
    let image = place(&mut c, auto("/photo.png")).await;
    assert_eq!(
        image,
        downloads
            .join("Images")
            .join("Sites")
            .join("127.0.0.1-photo.png")
    );
    assert!(image.is_file());
    // Un retéléchargement est rangé de la même façon (pas de dossier emboîté).
    let id = c.result("task.list", json!({})).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["fileName"] == "127.0.0.1-photo.png")
        .unwrap()["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.result(
        "task.restart",
        json!({"taskIds": [id], "deleteFiles": true}),
    )
    .await;
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id.as_str() && p["to"] == "completed"
    })
    .await;
    let again = c.result("task.get", json!({"taskId": id})).await;
    assert_eq!(
        Path::new(again["destDir"].as_str().unwrap()).join(again["fileName"].as_str().unwrap()),
        image
    );
    assert!(image.is_file());
    // Sa commande de fin (F-34) vise un programme absent : la tâche échoue, son fichier
    // reste en place.
    let (binary, task) = placed(&mut c, auto("/b/outil.bin")).await;
    assert_eq!(binary, elsewhere.join("outil.bin"));
    assert!(binary.is_file());
    assert_eq!(task["state"], "failed", "{task}");
    assert_eq!(task["errorCode"], "POSTPROCESS_FAILED", "{task}");
    // Aucune règle ne la prend : le dossier des téléchargements.
    assert_eq!(
        place(&mut c, auto("/a/outil.bin")).await,
        downloads.join("outil.bin")
    );
    // Un dossier choisi l'emporte, et le nom du serveur reste.
    let chosen =
        json!({"sources": [{"url": server.url("/choisie.png")}], "destDir": dir_text(&downloads)});
    assert_eq!(place(&mut c, chosen).await, downloads.join("choisie.png"));
    // Un nom choisi l'emporte sur le modèle, pas sur le dossier.
    let named = json!({"sources": [{"url": server.url("/x.png")}], "fileName": "mien.png"});
    assert_eq!(
        place(&mut c, named).await,
        downloads.join("Images").join("Sites").join("mien.png")
    );

    // Une liste refusée ne change rien.
    for bad in [
        json!({}),
        json!({"rules": {}}),
        json!({"rules": [{"match": {"urlRegex": "("}}]}),
        json!({"rules": [{"actions": {"folder": "../haut"}}]}),
        json!({"rules": [{"ruleId": "a"}, {"ruleId": "a"}]}),
    ] {
        assert_eq!(
            c.error_code("rules.set", bad.clone()).await,
            "INVALID_PARAMS",
            "{bad}"
        );
    }
    assert_eq!(c.result("rules.list", json!({})).await, set);

    // Un jeton : les règles sans commandes, et pas d'écriture.
    let token = c
        .result("token.create", json!({"name": "tél", "scope": "control"}))
        .await;
    let peer = super::Peer::token(token["id"].as_str().unwrap(), super::Scope::Control);
    let mut remote = TestClient::connect_as(&engine, peer);
    remote.result("engine.hello", hello()).await;
    let seen = remote.result("rules.list", json!({})).await;
    assert!(!seen.to_string().contains("analyse"), "{seen}");
    assert_eq!(seen["rules"][2]["actions"]["hasCommand"], true);
    assert_eq!(seen["rules"][1]["actions"]["hasCommand"], false);
    assert_eq!(
        remote.error_code("rules.set", json!({"rules": []})).await,
        "UNAUTHORIZED"
    );
    assert_eq!(c.result("rules.list", json!({})).await, set);
    crate::testing::stop(&engine).await;
}

/// Journaux et diagnostic (N-12) : le niveau se lit et se change ; le diagnostic ne porte ni
/// adresse, ni hôte, ni nom de fichier, ni chemin, même quand le journal en contient.
#[tokio::test]
async fn the_diagnostic_reveals_no_address_name_or_path() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let server = site(4096);
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    assert_eq!(
        c.result("settings.get", json!({})).await["values"]["logs.level"],
        "info"
    );
    let set = c
        .result("settings.set", json!({"values": {"logs.level": "debug"}}))
        .await;
    assert_eq!(set["values"]["logs.level"], "debug");
    let note = c
        .notification("settings.updated", |p| {
            p["values"]["logs.level"].is_string()
        })
        .await;
    assert_eq!(note["values"]["logs.level"], "debug");
    for bad in [json!("trace"), json!(3)] {
        assert_eq!(
            c.error_code("settings.set", json!({"values": {"logs.level": bad}}))
                .await,
            "INVALID_PARAMS"
        );
    }

    // Une tâche, un journal qui nomme son adresse, son hôte et un chemin.
    let url = server.url("/secret-rapport.pdf");
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"url": url}], "destDir": dir_text(dir.path()), "startPaused": true}),
        )
        .await;
    let logs = dir.path().join(crate::logs::DIR);
    std::fs::create_dir_all(&logs).unwrap();
    let line = json!({"level": "WARN", "fields": {
        "message": format!("échec de {url}"),
        "host": "127.0.0.1",
        "chemin": dir.path().join("secret-rapport.pdf").to_string_lossy(),
    }});
    std::fs::write(logs.join(crate::logs::FILE), format!("{line}\n")).unwrap();

    let diagnostic = c.result("engine.diagnostic", json!({})).await;
    let text = diagnostic.to_string();
    let id = added["taskId"].as_str().unwrap();
    for leak in [
        "secret-rapport",
        "127.0.0.1",
        id,
        &dir.path().to_string_lossy().replace('\\', "\\\\"),
    ] {
        assert!(!text.contains(leak), "{leak} dans {text}");
    }
    assert_eq!(diagnostic["report"]["tasks"]["byState"]["paused"], 1);
    assert_eq!(diagnostic["report"]["settings"]["logs.level"], "debug");
    let logged = diagnostic["logs"].as_array().unwrap();
    assert_eq!(logged.len(), 1, "{logged:?}");
    let logged_line = logged[0].as_str().unwrap();
    for kept in ["http://…", "<hôte>", "<données>"] {
        assert!(logged_line.contains(kept), "{kept} absent de {logged_line}");
    }
    // Réservé au canal local (admin).
    let mut reader = TestClient::connect(&engine);
    let mut params = hello();
    params["scope"] = json!("control");
    reader.result("engine.hello", params).await;
    assert_eq!(
        reader.error_code("engine.diagnostic", json!({})).await,
        "UNAUTHORIZED"
    );
    crate::testing::stop(&engine).await;
}

/// Contrat du protocole (SPEC §9.1) : « un client de la version précédente doit toujours
/// fonctionner ». Les requêtes réelles de chaque client (`docs/protocol/corpus`) suivent
/// leur schéma de paramètres, et sont rejouées contre le moteur : résultat ou erreur
/// attendus, validés par leur schéma.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_clients_corpus_still_works() {
    use base64::Engine as _;
    use std::collections::HashMap;

    let corpus_dir = super::contract::dir().join("corpus");
    let mut files: Vec<_> = std::fs::read_dir(&corpus_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(files.len() >= 5, "{files:?}");
    for file in files {
        let corpus: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let client = corpus["client"].as_str().unwrap().to_owned();
        let dir = tempfile::tempdir().unwrap();
        let site = Arc::new(Mutex::new(FileSite::new(64 * 1024)));
        let file_handler = crate::testing::file_handler(site);
        let server = crate::testing::serve(move |r| match r.path.as_str() {
            "/page.html" | "/video.html" => crate::testing::reply(
                "200 OK",
                &[("Content-Type", "text/html")],
                b"<html><body><a href=\"d.bin\">d</a> <a href=\"e.zip\">e</a></body></html>",
            ),
            "/lot.torrent" => crate::testing::reply(
                "200 OK",
                &[("Content-Type", "application/x-bittorrent")],
                b"d4:infod4:name3:loteee",
            ),
            "/lot.meta4" => {
                let xml = "<?xml version=\"1.0\"?><metalink xmlns=\"urn:ietf:params:xml:ns:metalink\">\
                    <file name=\"m.bin\"><size>65536</size><url>http://HOTE/m.bin</url></file></metalink>"
                    .replace("HOTE", r.header("host").unwrap_or_default());
                crate::testing::reply(
                    "200 OK",
                    &[("Content-Type", "application/metalink4+xml")],
                    xml.as_bytes(),
                )
            }
            _ => file_handler(r),
        });
        let engine = engine(dir.path());
        engine
            .set_download_dir(Some(dir_text(dir.path())))
            .await
            .unwrap();
        let mut vars: HashMap<String, Value> = HashMap::new();
        let b64 = |bytes: &[u8]| json!(base64::engine::general_purpose::STANDARD.encode(bytes));
        vars.insert("dir".into(), json!(dir_text(dir.path())));
        let mut metainfo = b"d4:infod5:filesld6:lengthi3e4:pathl5:a.binee\
            d6:lengthi4e4:pathl5:b.bineee4:name3:lot12:piece lengthi16384e6:pieces20:"
            .to_vec();
        metainfo.extend([7u8; 20]);
        metainfo.extend_from_slice(b"ee");
        vars.insert("torrent".into(), b64(&metainfo));
        let meta = format!(
            "<?xml version=\"1.0\"?><metalink xmlns=\"urn:ietf:params:xml:ns:metalink\">\
             <file name=\"m.bin\"><size>65536</size><url>{}</url></file></metalink>",
            server.url("/m.bin")
        );
        vars.insert("metalink".into(), b64(meta.as_bytes()));
        let out = format!(
            r#"{{"links": [{{"url": "{}", "name": "p.bin"}}]}}"#,
            server.url("/p.bin")
        );
        let plugin = crate::testing::plugin_files::fixed(&out);
        vars.insert(
            "pluginDigest".into(),
            json!(crate::plugins::digest_of(&plugin)),
        );
        vars.insert("plugin".into(), b64(&plugin));

        let mut c = TestClient::connect(&engine);
        for (i, step) in corpus["steps"].as_array().unwrap().iter().enumerate() {
            let method = step["method"].as_str().unwrap();
            let params = substitute(&step["params"], &vars, &server);
            let at = format!("{client} n° {i} : {method} {params}");
            // Ce que le client envoie suit le contrat.
            super::contract::check_params(method, &params);
            if step["replay"] == json!(false) {
                continue;
            }
            let expect: Vec<String> = match step.get("expect") {
                None => vec!["ok".into()],
                Some(list) => list
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect(),
            };
            let reply = c.call(method, params.clone()).await;
            let outcome = match reply.get("error") {
                None => "ok".to_owned(),
                Some(e) => e["data"]["code"].as_str().unwrap_or_default().to_owned(),
            };
            assert!(expect.contains(&outcome), "{at} → {reply}");
            if let Some(save) = step.get("save").and_then(Value::as_object) {
                for (name, pointer) in save {
                    let value = reply["result"]
                        .pointer(pointer.as_str().unwrap())
                        .unwrap_or_else(|| panic!("{at} : {pointer} absent de {reply}"));
                    vars.insert(name.clone(), value.clone());
                }
            }
        }
        crate::testing::stop(&engine).await;
    }

    /// `${nom}` seul : la valeur ; dans un texte, `${url:/chemin}` et `${dir}` remplacés.
    fn substitute(value: &Value, vars: &HashMap<String, Value>, server: &Server) -> Value {
        match value {
            Value::String(s) => {
                if let Some(name) = s.strip_prefix("${").and_then(|r| r.strip_suffix('}'))
                    && !name.contains(['$', ':'])
                {
                    return vars
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| panic!("variable inconnue : {name}"));
                }
                let mut text = s.clone();
                while let Some(start) = text.find("${url:") {
                    let end = start + text[start..].find('}').expect("} attendu");
                    let path = text[start + 6..end].to_owned();
                    text.replace_range(start..=end, &server.url(&path));
                }
                Value::String(text.replace("${dir}", vars["dir"].as_str().unwrap()))
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|v| substitute(v, vars, server)).collect())
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), substitute(v, vars, server)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// Un gros `.torrent` (16 Mio au plus côté clients, SPEC §6.3) passe en un seul message :
/// ici un fichier de près de 1 Gio en morceaux de 16 Kio, 1,2 Mio d'empreintes, 1,6 Mio
/// une fois en base64. Au-delà de la limite, la connexion se ferme.
#[tokio::test]
async fn a_large_torrent_file_fits_in_one_message() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    const PIECES: usize = 60_000;
    let mut metainfo = format!(
        "d4:infod6:lengthi{}e4:name8:gros.iso12:piece lengthi16384e6:pieces{}:",
        PIECES * 16384,
        PIECES * 20
    )
    .into_bytes();
    metainfo.extend((0..PIECES * 20).map(|i| (i % 251) as u8));
    metainfo.extend_from_slice(b"ee");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&metainfo);
    assert!(encoded.len() > 1 << 20);
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"torrent": encoded}], "destDir": dir_text(dir.path()), "startPaused": true}),
        )
        .await;
    let task = c
        .result("task.get", json!({"taskId": added["taskId"]}))
        .await;
    assert_eq!(task["displayName"], "gros.iso", "{task}");

    // À la limite : exactement `MAX_MESSAGE` octets, puis la fin de ligne, qui ne compte
    // pas (audit R22).
    let mut limit = br#"{"jsonrpc": "2.0", "id": 98, "method": "task.list", "pad": ""#.to_vec();
    limit.resize(super::MAX_MESSAGE - 2, b'x');
    limit.extend_from_slice(br#""}"#);
    assert_eq!(limit.len(), super::MAX_MESSAGE);
    limit.push(b'\n');
    c.writer.write_all(&limit).await.unwrap();
    let reply = c.read().await.expect("réponse attendue");
    assert_eq!(reply["id"], 98, "{reply}");
    assert!(reply["result"].is_object(), "{reply}");

    // Trop long : refusé, puis la connexion se ferme. Exactement un octet de trop, sans fin
    // de ligne : le moteur lit tout ce qui est écrit avant de fermer.
    let mut long = br#"{"jsonrpc": "2.0", "id": 99, "method": "task.list", "pad": ""#.to_vec();
    long.resize(super::MAX_MESSAGE - 1, b'x');
    long.extend_from_slice(br#""}"#);
    assert_eq!(long.len(), super::MAX_MESSAGE + 1);
    c.writer.write_all(&long).await.unwrap();
    let reply = c.read().await.expect("refus attendu");
    assert_eq!(reply["error"]["code"], -32600, "{reply}");
    assert!(c.read().await.is_none());
    crate::testing::stop(&engine).await;
}

/// Outils de la vidéo (SPEC §7.17) : état, installation sur demande seulement (portée
/// `admin`), annoncée par `media.tools` ; réglage `media.autoUpdate`, vrai par défaut.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn media_tools_are_installed_only_on_request() {
    let release = crate::testing::tools::FakeRelease::new();
    release.publish_ytdlp("2026.11.01", b"faux yt-dlp", None);
    release.publish_default_ffmpeg();
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(Config {
        db_path: dir.path().join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options {
            media: crate::media::Options {
                sources: release.sources(),
                dir: None,
            },
            ..Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap();
    let mut c = TestClient::connect(&engine);
    c.result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    let values = c.result("settings.get", json!({})).await["values"].clone();
    assert_eq!(values["media.autoUpdate"], true, "{values}");

    let tools = c.result("media.tools", json!({})).await;
    assert_eq!(tools["ready"], false, "{tools}");
    assert_eq!(tools["ytdlp"], Value::Null);
    assert_eq!(tools["installing"], false);
    // Rien n'a été demandé au serveur : aucune installation d'elle-même.
    assert_eq!(release.requested(), Vec::<String>::new());
    assert!(
        !dir.path()
            .join("tools")
            .join(crate::media::tools::exe("yt-dlp"))
            .exists()
    );

    // Une connexion `control` ne peut pas installer de programmes.
    let mut limited = TestClient::connect(&engine);
    let mut params = hello();
    params["scope"] = json!("control");
    limited.result("engine.hello", params).await;
    assert_eq!(
        limited.error_code("media.installTools", json!({})).await,
        "UNAUTHORIZED"
    );
    assert_eq!(
        c.error_code("media.installTools", json!({"force": "oui"}))
            .await,
        "INVALID_PARAMS"
    );

    let started = c.result("media.installTools", json!({})).await;
    assert_eq!(started["ready"], false, "{started}");
    let done = c
        .notification("media.tools", |p| {
            p["installing"] == false && p["ready"] == true
        })
        .await;
    assert_eq!(done["ytdlp"]["version"], "2026.11.01", "{done}");
    assert_eq!(done["ytdlp"]["origin"], "download");
    assert_eq!(done["error"], Value::Null);
    let tools = c.result("media.tools", json!({})).await;
    assert_eq!(tools["ready"], true);
    assert_eq!(
        std::fs::read(
            dir.path()
                .join("tools")
                .join(crate::media::tools::exe("yt-dlp"))
        )
        .unwrap(),
        b"faux yt-dlp"
    );

    c.result(
        "settings.set",
        json!({"values": {"media.autoUpdate": false}}),
    )
    .await;
    let values = c.result("settings.get", json!({})).await["values"].clone();
    assert_eq!(values["media.autoUpdate"], false);
    crate::testing::stop(&engine).await;
}

/// Un moteur dont les outils de la vidéo viennent de `sources`, dans `dir/tools`.
fn media_engine(dir: &Path, sources: crate::media::tools::Sources) -> Engine {
    Engine::start(Config {
        db_path: dir.join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options {
            media: crate::media::Options { sources, dir: None },
            ..Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap()
}

/// `media.probe` (SPEC §7.17), portée `add` : yt-dlp décrit la page ; ses erreurs ne
/// citent pas l'adresse ; sans yt-dlp, `TOOLS_MISSING`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn media_probe_describes_a_page_through_ytdlp() {
    let dir = tempfile::tempdir().unwrap();
    // Sans yt-dlp.
    let engine = media_engine(dir.path(), crate::media::tools::Sources::default());
    let mut c = TestClient::connect(&engine);
    assert_eq!(
        c.error_code("media.probe", json!({"pageUrl": "https://exemple.org/v"}))
            .await,
        "TOOLS_MISSING"
    );
    crate::testing::stop(&engine).await;

    let answer = json!({
        "title": "Un film libre", "duration": 12.5, "extractor_key": "Generic",
        "subtitles": {"fr": [{"ext": "vtt", "url": "https://exemple.org/fr.vtt"}]},
        "formats": [
            {"format_id": "a", "ext": "m4a", "vcodec": "none", "acodec": "mp4a",
             "filesize": 1000, "protocol": "https"},
            {"format_id": "v", "ext": "mp4", "vcodec": "avc1", "acodec": "none",
             "width": 640, "height": 360, "protocol": "m3u8_native", "has_drm": false}
        ]
    });
    let fake =
        crate::testing::tools::fake_ytdlp(&dir.path().join("fake"), &answer.to_string(), "", 0, 0);
    let sources = crate::media::tools::Sources {
        local_ytdlp: Some(fake),
        ..crate::media::tools::Sources::default()
    };
    let engine = media_engine(dir.path(), sources);
    // Une connexion `add` (l'extension) peut décrire une page.
    let mut c = TestClient::connect(&engine);
    let mut params = hello();
    params["scope"] = json!("add");
    c.result("engine.hello", params).await;
    let probe = c
        .result("media.probe", json!({"pageUrl": "https://exemple.org/v"}))
        .await;
    assert_eq!(probe["title"], "Un film libre", "{probe}");
    assert_eq!(probe["durationS"], 12.5);
    assert_eq!(probe["extractor"], "Generic");
    assert_eq!(probe["subtitles"], json!(["fr"]));
    assert_eq!(probe["formats"][0]["formatId"], "a");
    assert_eq!(probe["formats"][0]["vcodec"], Value::Null);
    assert_eq!(probe["formats"][0]["filesize"], 1000);
    assert_eq!(probe["formats"][1]["protocol"], "m3u8_native");
    assert_eq!(probe["formats"][1]["drm"], false);
    for bad in [
        json!({}),
        json!({"pageUrl": "ftp://exemple.org/v"}),
        json!({"pageUrl": "pas une adresse"}),
    ] {
        let code = c.error_code("media.probe", bad.clone()).await;
        assert!(
            code == "INVALID_PARAMS" || code == "URL_INVALID",
            "{bad} : {code}"
        );
    }
    crate::testing::stop(&engine).await;

    // yt-dlp en échec : son message, sans l'adresse.
    let fake = crate::testing::tools::fake_ytdlp(
        &dir.path().join("fake-err"),
        "",
        "ERROR: [generic] Unsupported URL: https://exemple.org/secret?jeton=1\n",
        1,
        0,
    );
    let sources = crate::media::tools::Sources {
        local_ytdlp: Some(fake),
        ..crate::media::tools::Sources::default()
    };
    let engine = media_engine(dir.path(), sources);
    let mut c = TestClient::connect(&engine);
    let reply = c
        .call(
            "media.probe",
            json!({"pageUrl": "https://exemple.org/secret?jeton=1"}),
        )
        .await;
    assert_eq!(
        reply["error"]["data"]["code"], "EXTRACTOR_FAILED",
        "{reply}"
    );
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(
        !message.contains("secret") && !message.contains("jeton"),
        "{message}"
    );
    crate::testing::stop(&engine).await;
}

/// `task.add` avec `media` (SPEC §7.17) : vérifié avant toute création ; la tâche porte
/// `kind: media` ; capacité `media` annoncée sous Windows et Linux.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_video_is_added_through_task_add() {
    let dir = tempfile::tempdir().unwrap();
    // Sans outils : refusé.
    let engine = media_engine(dir.path(), crate::media::tools::Sources::default());
    let mut c = TestClient::connect(&engine);
    let hello = c.result("engine.hello", hello()).await;
    assert_eq!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("media")),
        crate::media::SUPPORTED
    );
    let dest = dir_text(dir.path());
    assert_eq!(
        c.error_code(
            "task.add",
            json!({"media": {"pageUrl": "https://exemple.org/v"}, "destDir": dest})
        )
        .await,
        "TOOLS_MISSING"
    );
    crate::testing::stop(&engine).await;

    let fake = dir.path().join("faux");
    let sources = crate::media::tools::Sources {
        local_ytdlp: Some(crate::testing::tools::fake_ytdlp(&fake, "{}", "", 0, 0)),
        local_ffmpeg: Some(crate::testing::tools::fake_ffmpeg(&fake, "1.0")),
        system_ffmpeg: false,
        ..crate::media::tools::Sources::default()
    };
    let engine = media_engine(dir.path(), sources);
    let mut c = TestClient::connect(&engine);
    for bad in [
        json!({"media": {"pageUrl": "https://exemple.org/v"}, "sources": [{"url": "https://a/b"}]}),
        json!({"media": "https://exemple.org/v"}),
        json!({"media": {}}),
        json!({"media": {"pageUrl": "https://exemple.org/v", "container": "avi"}}),
        json!({"media": {"pageUrl": "https://exemple.org/v", "subtitles": ["fr fr"]}}),
        json!({"media": {"pageUrl": "https://exemple.org/v", "format": ""}}),
        json!({"media": {"pageUrl": "ftp://exemple.org/v"}}),
    ] {
        let code = c.error_code("task.add", bad.clone()).await;
        assert!(
            code == "INVALID_PARAMS" || code == "URL_INVALID",
            "{bad} : {code}"
        );
    }
    let added = c
        .result(
            "task.add",
            json!({"media": {"pageUrl": "https://exemple.org/v", "title": "Un film",
                             "container": "mkv", "subtitles": ["fr"]},
                   "destDir": dest, "startPaused": true}),
        )
        .await;
    let task = c
        .result("task.get", json!({"taskId": added["taskId"]}))
        .await;
    assert_eq!(task["kind"], "media", "{task}");
    assert_eq!(task["displayName"], "Un film");
    assert_eq!(task["state"], "paused");
    crate::testing::stop(&engine).await;
}

/// La mise à jour de yt-dlp part d'un `media.probe` quand c'est le moment, et s'annonce
/// par `engine.warning` ; `media.autoUpdate` à faux l'empêche.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ytdlp_updates_itself_on_a_probe_and_says_so() {
    let release = crate::testing::tools::FakeRelease::new();
    release.publish_ytdlp("2026.11.01", b"version 1", None);
    release.publish_default_ffmpeg();
    let dir = tempfile::tempdir().unwrap();
    let tools_dir = dir.path().join("tools");
    {
        let tools = crate::media::tools::Tools::open(
            tools_dir.clone(),
            release.sources(),
            Client::new().unwrap(),
        );
        assert!(tools.begin_install());
        tools.install(false).await.unwrap();
    }
    // La dernière recherche date d'il y a plus d'un jour.
    let state = tools_dir.join("tools.json");
    let mut json: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    json["lastCheckMs"] = json!(0);
    std::fs::write(&state, json.to_string()).unwrap();
    release.publish_ytdlp("2026.11.05", b"version 2", None);

    // Réglage à faux : pas de mise à jour.
    let engine = media_engine(dir.path(), release.sources());
    engine
        .set_flag(crate::store::settings::Flag::MediaAutoUpdate, false)
        .await
        .unwrap();
    let mut c = TestClient::connect(&engine);
    // yt-dlp installé n'est qu'un faux : l'extraction échoue, la mise à jour non.
    let _ = c
        .call("media.probe", json!({"pageUrl": "https://exemple.org/v"}))
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        engine.media_tools().unwrap().ytdlp.unwrap().version,
        "2026.11.01"
    );
    engine
        .set_flag(crate::store::settings::Flag::MediaAutoUpdate, true)
        .await
        .unwrap();

    c.result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    let _ = c
        .call("media.probe", json!({"pageUrl": "https://exemple.org/v"}))
        .await;
    let warning = c
        .notification("engine.warning", |p| p["code"] == "YTDLP_UPDATED")
        .await;
    assert!(
        warning["message"].as_str().unwrap().contains("2026.11.05"),
        "{warning}"
    );
    // Avertissement du moteur entier : sans tâche.
    assert!(warning.get("taskId").is_none(), "{warning}");
    let tools = c
        .notification("media.tools", |p| p["ytdlp"]["version"] == "2026.11.05")
        .await;
    assert_eq!(tools["ready"], true);
    assert_eq!(
        std::fs::read(tools_dir.join(crate::media::tools::exe("yt-dlp"))).unwrap(),
        b"version 2"
    );
    crate::testing::stop(&engine).await;
}

/// Interface réseau des torrents (SPEC §7.16) : `torrent.interface`, son état dans
/// l'instantané et par `engine.torrentInterface`.
#[tokio::test]
async fn the_torrent_interface_goes_through_the_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    let values = c.result("settings.get", json!({})).await["values"].clone();
    assert_eq!(values["torrent.interface"], Value::Null, "{values}");
    let snapshot = c.result("sync.snapshot", json!({})).await;
    assert_eq!(snapshot["torrentInterfaceUp"], true);
    // Les interfaces de la machine, pour choisir : jamais la boucle locale, sans doublon,
    // triées ; chacune est reconnue par le réglage.
    let names: Vec<String> = c.result("torrent.interfaces", json!({})).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect();
    assert!(
        !names.iter().any(|n| n == "lo" || n.contains("Loopback")),
        "{names:?}"
    );
    assert!(names.windows(2).all(|w| w[0] < w[1]), "{names:?}");
    for name in &names {
        assert!(crate::torrent::system_interface(name).is_some(), "{name}");
    }

    // Une interface qui n'existe pas : les torrents l'attendent.
    let absent = "auro-essai-interface-absente";
    let set = c
        .result(
            "settings.set",
            json!({"values": {"torrent.interface": absent}}),
        )
        .await;
    assert_eq!(set["values"]["torrent.interface"], absent);
    let note = c.notification("engine.torrentInterface", |_| true).await;
    assert_eq!(note["up"], false);
    let note = c
        .notification("settings.updated", |p| {
            p["values"].get("torrent.interface").is_some()
        })
        .await;
    assert_eq!(note["values"]["torrent.interface"], absent);
    let snapshot = c.result("sync.snapshot", json!({})).await;
    assert_eq!(snapshot["torrentInterfaceUp"], false);
    assert_eq!(snapshot["settings"]["torrent.interface"], absent);

    for bad in [
        json!(""),
        json!("  "),
        json!("a\nb"),
        json!(5),
        json!("x".repeat(257)),
    ] {
        assert_eq!(
            c.error_code(
                "settings.set",
                json!({"values": {"torrent.interface": bad}})
            )
            .await,
            "INVALID_PARAMS",
            "{bad}"
        );
    }

    c.result(
        "settings.set",
        json!({"values": {"torrent.interface": null}}),
    )
    .await;
    let note = c.notification("engine.torrentInterface", |_| true).await;
    assert_eq!(note["up"], true);
    let snapshot = c.result("sync.snapshot", json!({})).await;
    assert_eq!(snapshot["torrentInterfaceUp"], true);
    assert_eq!(snapshot["settings"]["torrent.interface"], Value::Null);
    crate::testing::stop(&engine).await;
}

/// Réglages et limites des torrents (SPEC §7.16) : débit d'envoi, ratio de partage, UPnP
/// (faux par défaut), réseau facturé ; `task.setFiles` seulement pour un torrent.
#[tokio::test]
async fn torrent_settings_limits_and_files_go_through_the_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let server = site(1024);
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("events.subscribe", json!({"topics": ["queues"]}))
        .await;

    let values = c.result("settings.get", json!({})).await["values"].clone();
    assert_eq!(values["torrent.ratioLimit"], json!(1.0), "{values}");
    assert_eq!(values["torrent.upnp"], json!(false), "{values}");

    let limits = c.result("limits.set", json!({"uploadBps": 4096})).await;
    assert_eq!(limits["uploadBps"], 4096);
    let note = c.notification("limits.updated", |_| true).await;
    assert_eq!(note["uploadBps"], 4096);
    assert_eq!(c.result("limits.get", json!({})).await["uploadBps"], 4096);

    let set = c
        .result(
            "settings.set",
            json!({"values": {"torrent.ratioLimit": 2.5}}),
        )
        .await;
    assert_eq!(set["values"]["torrent.ratioLimit"], json!(2.5));
    let note = c
        .notification("settings.updated", |p| {
            p["values"].get("torrent.ratioLimit").is_some()
        })
        .await;
    assert_eq!(note["values"]["torrent.ratioLimit"], json!(2.5));
    let set = c
        .result(
            "settings.set",
            json!({"values": {"torrent.ratioLimit": null}}),
        )
        .await;
    assert_eq!(set["values"]["torrent.ratioLimit"], Value::Null);
    for bad in [json!(-1), json!("2"), json!(true)] {
        assert_eq!(
            c.error_code(
                "settings.set",
                json!({"values": {"torrent.ratioLimit": bad}})
            )
            .await,
            "INVALID_PARAMS"
        );
    }

    let network = c
        .result(
            "engine.setNetwork",
            json!({"usable": true, "metered": true}),
        )
        .await;
    assert_eq!(network["metered"], true);
    let snapshot = c.result("sync.snapshot", json!({})).await;
    assert_eq!(snapshot["networkMetered"], true);
    assert_eq!(snapshot["limits"]["uploadBps"], 4096);
    assert_eq!(snapshot["settings"]["torrent.ratioLimit"], Value::Null);

    // `task.setFiles` : refusé pour une tâche HTTP.
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/f")}], "destDir": dir_text(dir.path()), "startPaused": true}),
        )
        .await;
    assert_eq!(
        c.error_code(
            "task.setFiles",
            json!({"taskId": added["taskId"], "selected": [0]})
        )
        .await,
        "INVALID_PARAMS"
    );
    assert_eq!(
        c.error_code(
            "task.setFiles",
            json!({"taskId": added["taskId"], "selected": "0"})
        )
        .await,
        "INVALID_PARAMS"
    );
    let task = c
        .result(
            "task.get",
            json!({"taskId": added["taskId"], "include": ["files"]}),
        )
        .await;
    assert_eq!(task["files"], json!([]), "{task}");
    crate::testing::stop(&engine).await;
}

/// Miroirs (SPEC §7.7, phase 6, étape 1a) : `task.add` avec plusieurs sources, validées à la
/// première exécution face à la référence (taille et échantillons) ; un autre fichier de
/// même taille est refusé, avec la raison.
#[tokio::test]
async fn mirrors_are_validated_against_the_reference() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    const LEN: u64 = 1024 * 1024;
    // Référence lente : les miroirs se valident pendant le téléchargement, et le bon y
    // prend part (étape 1b).
    let reference = serve_file_paced(
        Arc::new(Mutex::new(FileSite::new(LEN))),
        Some(Pace {
            chunk: 16 * 1024,
            delay: Duration::from_millis(30),
        }),
    );
    let good = site(LEN);
    let other = serve_file(Arc::new(Mutex::new(FileSite {
        shift: 1,
        ..FileSite::new(LEN)
    })));
    let dest = dir_text(dir.path());
    let added = c
        .result(
            "task.add",
            json!({"sources": [
                {"url": reference.url("/f.bin")},
                {"url": good.url("/miroir/f.bin")},
                {"url": other.url("/f.bin")},
            ], "destDir": dest}),
        )
        .await;
    let id = added["taskId"].as_str().unwrap().to_owned();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id.as_str() && p["to"] == "completed"
    })
    .await;
    let task = c
        .result("task.get", json!({"taskId": id, "include": ["sources"]}))
        .await;
    let sources = task["sources"].as_array().unwrap();
    let states: Vec<_> = sources
        .iter()
        .map(|s| (s["rank"].clone(), s["state"].clone()))
        .collect();
    assert_eq!(
        states,
        [
            (json!(0), json!("ok")),
            (json!(1), json!("ok")),
            (json!(2), json!("failed"))
        ],
        "{task}"
    );
    assert!(
        sources[2]["lastError"]
            .as_str()
            .is_some_and(|e| e.contains("contenu différent")),
        "{task}"
    );
    assert!(sources[0]["sourceId"].is_string());
    // Le bon miroir a servi des segments, au-delà des échantillons de sa validation ;
    // l'autre, seulement ses échantillons.
    let ranged = |server: &Server| {
        server
            .requests()
            .iter()
            .filter(|r| r.header("range").is_some())
            .count()
    };
    assert!(ranged(&good) > crate::mirrors::SAMPLES, "{}", ranged(&good));
    assert!(
        ranged(&other) <= crate::mirrors::SAMPLES,
        "{}",
        ranged(&other)
    );
    assert_eq!(
        std::fs::read(dir.path().join("f.bin")).unwrap(),
        crate::testing::content(LEN as usize)
    );

    // Refus : la même adresse deux fois, un miroir pour un torrent.
    for bad in [
        json!({"sources": [{"url": reference.url("/g.bin")}, {"url": reference.url("/g.bin")}], "destDir": dest}),
        json!({"sources": [{"url": "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"}, {"url": good.url("/g.bin")}], "destDir": dest}),
        json!({"sources": [{"url": reference.url("/g.bin")}, {"torrent": "ZA=="}], "destDir": dest}),
    ] {
        assert_eq!(
            c.error_code("task.add", bad.clone()).await,
            "INVALID_PARAMS",
            "{bad}"
        );
    }
    // Une tâche terminée ne prend plus de miroir.
    assert_eq!(
        c.error_code(
            "task.addSource",
            json!({"taskId": id, "url": good.url("/autre")})
        )
        .await,
        "INVALID_STATE"
    );
    crate::testing::stop(&engine).await;
}

/// `task.addSource` (SPEC §7.7) : sur une tâche préparée, le miroir est validé tout de
/// suite ; un autre fichier est refusé à l'appel ; sur une tâche pas encore préparée, il
/// attend la première exécution.
#[tokio::test]
async fn links_of_a_page_are_found_sized_and_added_together() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let hello = c.result("engine.hello", hello()).await;
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("linkscan"))
    );
    const LEN: u64 = 100_000;
    let files = site(LEN);
    let html = format!(
        r#"<html><body><a href="{}">a</a> <a href="{}">b</a> <a href="autre.html">page</a>
<p>En clair : {}</p></body></html>"#,
        files.url("/pub/a.iso"),
        files.url("/pub/b.zip"),
        files.url("/pub/c.7z"),
    );
    let pages = crate::testing::serve(move |r| match r.path.as_str() {
        "/liste" => crate::testing::reply(
            "302 Found",
            &[("Location", "/vrai/liste.html"), ("Content-Length", "0")],
            b"",
        ),
        // Un fichier sans extension, décrit comme du texte : un fichier, pas une page.
        "/SHA256SUMS" => crate::testing::reply(
            "200 OK",
            &[
                ("Content-Type", "text/plain; charset=utf-8"),
                ("Content-Length", "5"),
            ],
            b"abcde",
        ),
        "/vrai/liste.html" => {
            crate::testing::reply("200 OK", &[("Content-Type", "text/html")], html.as_bytes())
        }
        _ => crate::testing::reply("404 Not Found", &[("Content-Length", "0")], b""),
    });

    // Une page : ses liens, résolus par rapport à son adresse après redirection.
    let scan = c
        .result("linkscan.run", json!({"pageUrl": pages.url("/liste")}))
        .await;
    assert_eq!(
        scan["pageUrl"],
        json!(pages.url("/vrai/liste.html")),
        "{scan}"
    );
    let links = scan["links"].as_array().unwrap();
    let found: Vec<(&str, &str)> = links
        .iter()
        .map(|l| (l["name"].as_str().unwrap(), l["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(
        found,
        [
            ("a.iso", "file"),
            ("b.zip", "file"),
            ("autre.html", "page"),
            ("c.7z", "file")
        ],
        "{scan}"
    );
    assert_eq!(links[2]["url"], json!(pages.url("/vrai/autre.html")));
    assert_eq!(links[0]["ext"], "iso");
    // Un texte : ses adresses.
    let text = c
        .result(
            "linkscan.run",
            json!({"text": format!("x {} y", files.url("/d.bin"))}),
        )
        .await;
    assert_eq!(text["links"][0]["name"], "d.bin");
    assert!(text.get("pageUrl").is_none());

    // Tailles : chacune pour elle-même.
    let probed = c
        .result(
            "linkscan.probe",
            json!({"urls": [files.url("/pub/a.iso"), pages.url("/absent.iso"), "ftp://x/y"]}),
        )
        .await;
    let items = probed["items"].as_array().unwrap();
    assert_eq!(items[0]["size"], LEN, "{probed}");
    assert_eq!(items[0]["name"], "a.iso");
    assert_eq!(items[1]["error"]["code"], "SOURCE_UNREACHABLE", "{probed}");
    assert_eq!(items[2]["error"]["code"], "URL_INVALID", "{probed}");
    // Ce que dit le serveur : un fichier (sans extension, en text/plain), une page (HTML), ou
    // rien (pas de Content-Type).
    let kinds = c
        .result(
            "linkscan.probe",
            json!({"urls": [pages.url("/SHA256SUMS"), pages.url("/vrai/liste.html"), files.url("/x")]}),
        )
        .await;
    let kinds: Vec<&Value> = kinds["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| &i["kind"])
        .collect();
    assert_eq!(kinds, [&json!("file"), &json!("page"), &Value::Null]);

    // Ajout groupé : le refus d'un élément n'empêche pas les autres.
    let dest = dir_text(dir.path());
    let batch = c
        .result(
            "task.addBatch",
            json!({"items": [
                {"sources": [{"url": files.url("/pub/a.iso")}], "destDir": dest},
                {"sources": [{"url": "gopher://x/y.iso"}], "destDir": dest},
                {"sources": [{"url": files.url("/pub/b.zip")}], "destDir": dest},
                "pas un objet",
            ]}),
        )
        .await;
    let ids: Vec<String> = batch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["taskId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 2, "{batch}");
    assert_eq!(batch["tasks"][1]["index"], 2);
    assert_eq!(batch["errors"][0]["index"], 1);
    assert_eq!(batch["errors"][0]["code"], "URL_INVALID", "{batch}");
    assert_eq!(batch["errors"][1]["index"], 3);
    assert_eq!(batch["errors"][1]["code"], "INVALID_PARAMS", "{batch}");
    timeout(WAIT, async {
        loop {
            let mut done = 0;
            for id in &ids {
                let task = c.result("task.get", json!({"taskId": id})).await;
                done += usize::from(task["state"] == "completed");
            }
            if done == ids.len() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("les deux tâches finissent");
    assert_eq!(
        std::fs::read(dir.path().join("b.zip")).unwrap().len() as u64,
        LEN
    );

    for (method, bad) in [
        ("linkscan.run", json!({})),
        (
            "linkscan.run",
            json!({"text": "a", "pageUrl": pages.url("/liste")}),
        ),
        (
            "linkscan.run",
            json!({"pageUrl": pages.url("/absent.html")}),
        ),
        ("linkscan.probe", json!({"urls": []})),
        ("linkscan.probe", json!({"urls": vec![files.url("/f"); 51]})),
        ("task.addBatch", json!({"items": []})),
        ("task.addBatch", json!({})),
    ] {
        assert_eq!(
            c.error_code(method, bad.clone()).await,
            "INVALID_PARAMS",
            "{method} {bad}"
        );
    }
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn a_metalink_gives_one_task_per_file_with_its_mirrors_and_checksum() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    const LEN: u64 = 300_000;
    let reference = site(LEN);
    let mirror = site(LEN);
    let sum = crate::testing::content_sha256(LEN);
    // Empreintes MD5 de morceaux de 100 000 octets (F-12) pour le second fichier.
    let pieces: String = (0..3u64)
        .map(|i| {
            use md5::Digest as _;
            let digest = md5::Md5::digest(crate::testing::content_range(i * 100_000, 100_000));
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            format!("<hash>{hex}</hash>")
        })
        .collect();
    // Deux fichiers (le second dans un sous-dossier), un troisième écarté (il remonte).
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="a.bin">
    <size>{LEN}</size>
    <hash type="sha-256">{sum}</hash>
    <url priority="2">{}</url>
    <url priority="1">{}</url>
  </file>
  <file name="sous/b.bin">
    <pieces type="md5" length="100000">{pieces}</pieces>
    <url>{}</url>
  </file>
  <file name="../c.bin"><url>{}</url></file>
</metalink>"#,
        mirror.url("/miroir/a.bin"),
        reference.url("/a.bin"),
        reference.url("/b.bin"),
        reference.url("/c.bin"),
    );
    let served = doc.clone();
    let host = crate::testing::serve(move |r| match r.path.as_str() {
        "/f.meta4" => crate::testing::reply(
            "200 OK",
            &[("Content-Type", "application/metalink4+xml")],
            served.as_bytes(),
        ),
        "/page.html" => crate::testing::reply("200 OK", &[], b"<html></html>"),
        _ => crate::testing::reply("404 Not Found", &[("Content-Length", "0")], b""),
    });

    // Lu à une adresse : les fichiers, ceux écartés, et le contenu lu.
    let read = c
        .result("metalink.read", json!({"url": host.url("/f.meta4")}))
        .await;
    assert_eq!(read["format"], "4", "{read}");
    let files = read["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "{read}");
    assert_eq!(files[0]["path"], "a.bin");
    assert_eq!(files[0]["size"], LEN);
    assert_eq!(
        files[0]["checksum"],
        json!({"algo": "sha256", "value": sum})
    );
    assert_eq!(
        files[0]["urls"],
        json!([reference.url("/a.bin"), mirror.url("/miroir/a.bin")])
    );
    assert_eq!(files[1]["path"], "sous/b.bin");
    assert_eq!(files[1]["name"], "b.bin");
    assert_eq!(read["ignored"][0]["name"], "../c.bin", "{read}");
    let data = read["data"].as_str().unwrap().to_owned();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&data)
            .unwrap(),
        doc.as_bytes()
    );
    // Lu depuis son contenu : rien à rendre.
    let local = c.result("metalink.read", json!({"data": data})).await;
    assert!(local.get("data").is_none() && local["files"] == read["files"]);

    // Ajouté : une tâche par fichier, la première avec son miroir et son empreinte.
    let dest = dir_text(dir.path());
    let add = json!({"data": data, "destDir": dest, "idempotencyKey": "meta-1"});
    let added = c.result("metalink.add", add.clone()).await;
    assert_eq!(added["errors"], json!([]), "{added}");
    let ids: Vec<String> = added["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["taskId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 2, "{added}");
    // Les deux finissent dans un ordre quelconque : leur état est relu.
    timeout(WAIT, async {
        loop {
            let mut done = 0;
            for id in &ids {
                let task = c.result("task.get", json!({"taskId": id})).await;
                assert_ne!(task["state"], "failed", "{task}");
                done += usize::from(task["state"] == "completed");
            }
            if done == ids.len() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("les deux tâches finissent");
    let content = crate::testing::content(LEN as usize);
    assert_eq!(std::fs::read(dir.path().join("a.bin")).unwrap(), content);
    assert_eq!(
        std::fs::read(dir.path().join("sous").join("b.bin")).unwrap(),
        content
    );
    let first = c
        .result(
            "task.get",
            json!({"taskId": ids[0], "include": ["sources"]}),
        )
        .await;
    assert_eq!(first["checksum"]["value"], json!(sum), "{first}");
    let urls: Vec<&Value> = first["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["url"])
        .collect();
    assert_eq!(
        urls,
        [
            &json!(reference.url("/a.bin")),
            &json!(mirror.url("/miroir/a.bin"))
        ]
    );
    assert!(first.get("pieces").is_none(), "{first}");
    assert_eq!(first["sources"][1]["badPieces"], 0, "{first}");
    // Le second fichier garde ses empreintes de morceaux, toutes justes (F-12).
    let second = c.result("task.get", json!({"taskId": ids[1]})).await;
    assert_eq!(
        second["pieces"],
        json!({"count": 3, "length": 100_000, "algo": "md5", "redone": 0, "rounds": 0}),
        "{second}"
    );
    // Même clé : les mêmes tâches, rien de nouveau.
    let again = c.result("metalink.add", add).await;
    assert_eq!(again["tasks"], added["tasks"]);

    // Un seul fichier choisi ; un rang inconnu, les deux sources ou aucune, une adresse
    // sans Metalink : refusés avant tout ajout.
    let one = c
        .result(
            "metalink.add",
            json!({"data": data, "select": [1], "destDir": dest, "startPaused": true}),
        )
        .await;
    assert_eq!(one["tasks"].as_array().unwrap().len(), 1, "{one}");
    assert_eq!(one["tasks"][0]["index"], 1);
    for (method, bad, code) in [
        (
            "metalink.add",
            json!({"data": data, "select": [2]}),
            "INVALID_PARAMS",
        ),
        (
            "metalink.add",
            json!({"data": data, "select": []}),
            "INVALID_PARAMS",
        ),
        (
            "metalink.read",
            json!({"data": data, "url": host.url("/f.meta4")}),
            "INVALID_PARAMS",
        ),
        ("metalink.read", json!({}), "INVALID_PARAMS"),
        (
            "metalink.read",
            json!({"data": "pas du base64 !"}),
            "INVALID_PARAMS",
        ),
        (
            "metalink.read",
            json!({"url": host.url("/absent.meta4")}),
            "INVALID_PARAMS",
        ),
        (
            "metalink.read",
            json!({"url": host.url("/page.html")}),
            "INVALID_PARAMS",
        ),
        (
            "metalink.read",
            json!({"url": "ftp://exemple/f.meta4"}),
            "URL_INVALID",
        ),
    ] {
        assert_eq!(
            c.error_code(method, bad.clone()).await,
            code,
            "{method} {bad}"
        );
    }
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn a_mirror_added_to_a_prepared_task_is_checked_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    const LEN: u64 = 4 * 1024 * 1024;
    // Lent : la tâche se met en pause avant la fin.
    let reference = serve_file_paced(
        Arc::new(Mutex::new(FileSite::new(LEN))),
        Some(Pace {
            chunk: 16 * 1024,
            delay: Duration::from_millis(20),
        }),
    );
    let good = site(LEN);
    let other = serve_file(Arc::new(Mutex::new(FileSite {
        shift: 7,
        ..FileSite::new(LEN)
    })));
    let dest = dir_text(dir.path());
    let id = c
        .result(
            "task.add",
            json!({"sources": [{"url": reference.url("/f.bin")}], "destDir": dest}),
        )
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.notification("task.updated", |p| {
        p["taskId"] == id.as_str() && p.get("fileName").is_some()
    })
    .await;
    c.result("task.pause", json!({"taskIds": [id]})).await;
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id.as_str() && p["to"] == "paused"
    })
    .await;

    let added = c
        .result(
            "task.addSource",
            json!({"taskId": id, "url": good.url("/f.bin")}),
        )
        .await;
    assert!(added["sourceId"].is_string());
    assert_eq!(
        c.error_code(
            "task.addSource",
            json!({"taskId": id, "url": other.url("/f.bin")})
        )
        .await,
        "SOURCE_MISMATCH"
    );
    assert_eq!(
        c.error_code(
            "task.addSource",
            json!({"taskId": id, "url": good.url("/f.bin")})
        )
        .await,
        "INVALID_PARAMS",
        "déjà là"
    );
    assert_eq!(
        c.error_code(
            "task.addSource",
            json!({"taskId": id, "url": good.url("/x"), "rank": 1})
        )
        .await,
        "CAPABILITY_MISSING"
    );
    let task = c
        .result("task.get", json!({"taskId": id, "include": ["sources"]}))
        .await;
    let states: Vec<_> = task["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["state"].clone())
        .collect();
    assert_eq!(states, [json!("ok"), json!("ok")], "{task}");

    // Pas encore préparée : le miroir attend sa validation.
    let fresh = c
        .result(
            "task.add",
            json!({"sources": [{"url": reference.url("/g.bin")}], "destDir": dest, "startPaused": true}),
        )
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.result(
        "task.addSource",
        json!({"taskId": fresh, "url": other.url("/g.bin")}),
    )
    .await;
    let task = c
        .result("task.get", json!({"taskId": fresh, "include": ["sources"]}))
        .await;
    assert_eq!(task["sources"][1]["state"], "unknown", "{task}");
    crate::testing::stop(&engine).await;
}

/// Interface distante (SPEC §7.19) : un jeton n'a jamais `admin` ; ses ajouts et ses
/// changements ne joignent que le réseau public et n'écrivent que sous le dossier des
/// téléchargements, pour toute la vie de la tâche ; sa révocation ferme ses connexions.
#[tokio::test]
async fn a_token_connection_reaches_only_the_public_network_and_the_download_folder() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("dl");
    std::fs::create_dir_all(downloads.join("sous")).unwrap();
    let elsewhere = dir.path().join("ailleurs");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let server = site(64 * 1024);
    let engine = engine(dir.path());
    let mut local = TestClient::connect(&engine);
    local.result("engine.hello", hello()).await;
    local
        .result(
            "settings.set",
            json!({"values": {"downloads.dir": dir_text(&downloads)}}),
        )
        .await;

    // Jetons : par le canal local seulement ; la valeur n'est rendue qu'à la création.
    assert_eq!(
        local
            .error_code("token.create", json!({"name": "tél", "scope": "admin"}))
            .await,
        "INVALID_PARAMS"
    );
    let created = local
        .result(
            "token.create",
            json!({"name": "téléphone", "scope": "control"}),
        )
        .await;
    let token_id = created["id"].as_str().unwrap().to_owned();
    assert!(created["token"].as_str().unwrap().starts_with("auro_"));
    let listed = local.result("token.list", json!({})).await;
    assert_eq!(listed["tokens"][0]["name"], "téléphone");
    assert!(listed["tokens"][0].get("token").is_none(), "{listed}");

    let peer = super::Peer::token(&token_id, super::Scope::Control);
    let mut remote = TestClient::connect_as(&engine, peer);
    assert_eq!(
        remote.result("engine.hello", hello()).await["scope"],
        "control"
    );
    for (method, params) in [
        ("token.list", json!({})),
        ("token.create", json!({"name": "x"})),
        (
            "settings.set",
            json!({"values": {"remote.allowPrivate": true}}),
        ),
        ("engine.shutdown", json!({})),
        ("login.list", json!({})),
        ("media.installTools", json!({})),
    ] {
        assert_eq!(
            remote.error_code(method, params).await,
            "UNAUTHORIZED",
            "{method}"
        );
    }

    // Adresses du réseau interne : refusées dès l'ajout, sans requête au serveur.
    let inside = server.url("/f.bin");
    let by_name = inside.replace("127.0.0.1", "localhost");
    for url in [&inside, &by_name] {
        assert_eq!(
            remote
                .error_code("task.add", json!({"sources": [{"url": url}]}))
                .await,
            "ADDRESS_FORBIDDEN"
        );
    }
    let batch = remote
        .result(
            "task.addBatch",
            json!({"items": [{"sources": [{"url": inside}]}]}),
        )
        .await;
    assert_eq!(batch["errors"][0]["code"], "ADDRESS_FORBIDDEN", "{batch}");
    assert_eq!(
        remote
            .error_code("linkscan.run", json!({"pageUrl": inside}))
            .await,
        "ADDRESS_FORBIDDEN"
    );
    assert_eq!(
        remote
            .error_code("metalink.read", json!({"url": inside}))
            .await,
        "ADDRESS_FORBIDDEN"
    );
    // Audit C16 : un `.torrent` lu par le moteur suit la même limite.
    assert_eq!(
        remote
            .error_code("torrent.fetch", json!({"url": inside}))
            .await,
        "ADDRESS_FORBIDDEN"
    );
    let probed = remote
        .result("linkscan.probe", json!({"urls": [by_name]}))
        .await;
    assert_eq!(
        probed["items"][0]["error"]["code"], "ADDRESS_FORBIDDEN",
        "{probed}"
    );
    assert!(server.requests().is_empty(), "{:?}", server.requests());

    // Dossier : sous celui des téléchargements seulement.
    let public = "https://fichiers.invalid/f.iso";
    assert_eq!(
        remote
            .error_code(
                "task.add",
                json!({"sources": [{"url": public}], "destDir": dir_text(&elsewhere)})
            )
            .await,
        "DEST_FORBIDDEN"
    );
    let added = remote
        .result(
            "task.add",
            json!({"sources": [{"url": public}], "destDir": dir_text(&downloads.join("sous")),
                   "startPaused": true}),
        )
        .await;
    let remote_task = added["taskId"].as_str().unwrap().to_owned();
    let got = local
        .result("task.get", json!({"taskId": remote_task}))
        .await;
    assert_eq!(got["publicOnly"], true, "{got}");
    assert_eq!(
        remote
            .error_code(
                "task.update",
                json!({"taskId": remote_task, "destDir": dir_text(&elsewhere)})
            )
            .await,
        "DEST_FORBIDDEN"
    );

    // Une tâche locale : un miroir donné par le jeton la restreint pour toujours, et son
    // exécution ne joint plus le serveur local.
    let mine = local
        .result(
            "task.add",
            json!({"sources": [{"url": inside}], "startPaused": true}),
        )
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        remote
            .error_code(
                "task.addSource",
                json!({"taskId": mine, "url": "http://192.168.1.1/f.bin"})
            )
            .await,
        "ADDRESS_FORBIDDEN"
    );
    let got = local.result("task.get", json!({"taskId": mine})).await;
    assert_eq!(got["publicOnly"], false);
    remote
        .result(
            "task.addSource",
            json!({"taskId": mine, "url": "https://miroir.invalid/f.bin"}),
        )
        .await;
    let got = local.result("task.get", json!({"taskId": mine})).await;
    assert_eq!(got["publicOnly"], true);
    local
        .result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    local
        .result("task.resume", json!({"taskIds": [mine]}))
        .await;
    let failed = local
        .notification("task.stateChanged", |p| {
            p["taskId"] == mine.as_str() && p["to"] == "failed"
        })
        .await;
    assert_eq!(failed["errorCode"], "ADDRESS_FORBIDDEN", "{failed}");
    assert!(server.requests().is_empty());

    // Révocation : la connexion du jeton se ferme.
    let revoked = local
        .result("token.revoke", json!({"tokenId": token_id}))
        .await;
    assert_eq!(revoked["revoked"], true);
    loop {
        match remote.read().await {
            None => break,
            Some(message) => assert!(message.get("method").is_some(), "{message}"),
        }
    }
    assert!(
        local.result("token.list", json!({})).await["tokens"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Audit D2 : une connexion vérifiée juste avant la révocation, servie juste après,
    // se ferme aussitôt au lieu de rester ouverte pour toujours.
    let mut late = TestClient::connect_as(
        &engine,
        super::Peer::token(&token_id, super::Scope::Control),
    );
    late.send_raw(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "engine.hello", "params": hello()})
            .to_string(),
    )
    .await;
    assert!(late.read().await.is_none(), "jeton révoqué servi");
    crate::testing::stop(&engine).await;
}

/// Audit R13 : la révocation d'un jeton coupe aussi un appel en cours (ici une analyse de
/// page vers un serveur qui ne répond qu'au bout de 20 s).
#[tokio::test]
async fn a_revoked_token_cuts_a_call_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let slow = crate::testing::serve(|_| {
        std::thread::sleep(Duration::from_secs(20));
        crate::testing::reply(
            "200 OK",
            &[("Content-Type", "text/html"), ("Content-Length", "0")],
            b"",
        )
    });
    let mut local = TestClient::connect(&engine);
    local.result("engine.hello", hello()).await;
    local
        .result(
            "settings.set",
            json!({"values": {"remote.allowPrivate": true}}),
        )
        .await;
    let token = local
        .result("token.create", json!({"name": "tél", "scope": "control"}))
        .await;
    let token_id = token["id"].as_str().unwrap().to_owned();
    let mut remote = TestClient::connect_as(
        &engine,
        super::Peer::token(&token_id, super::Scope::Control),
    );
    remote.result("engine.hello", hello()).await;
    remote
        .send_raw(
            &json!({"jsonrpc": "2.0", "id": 7, "method": "linkscan.run",
                    "params": {"pageUrl": slow.url("/page.html")}})
            .to_string(),
        )
        .await;
    // L'appel est parti vers le serveur lent.
    timeout(WAIT, async {
        while slow.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("analyse lancée");
    let asked = std::time::Instant::now();
    local
        .result("token.revoke", json!({"tokenId": token_id}))
        .await;
    while let Some(message) = remote.read().await {
        assert!(
            message.get("method").is_some(),
            "réponse servie : {message}"
        );
    }
    assert!(
        asked.elapsed() < Duration::from_secs(10),
        "connexion fermée après l'appel seulement"
    );
    crate::testing::stop(&engine).await;
}

/// `remote.allowPrivate` (SPEC §7.19) : réglé par le canal local, il ouvre le réseau
/// interne aux connexions à jeton suivantes, sans lever la limite du dossier.
#[tokio::test]
async fn allow_private_opens_the_internal_network_but_not_other_folders() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("dl");
    std::fs::create_dir_all(&downloads).unwrap();
    let server = site(32 * 1024);
    let engine = engine(dir.path());
    let mut local = TestClient::connect(&engine);
    local.result("engine.hello", hello()).await;
    let values = local.result("settings.get", json!({})).await["values"].clone();
    assert_eq!(values["remote.allowPrivate"], false, "{values}");
    assert_eq!(values["remote.enabled"], false, "{values}");
    local
        .result(
            "settings.set",
            json!({"values": {"downloads.dir": dir_text(&downloads), "remote.allowPrivate": true}}),
        )
        .await;
    let token = local
        .result("token.create", json!({"name": "nas", "scope": "add"}))
        .await;
    let peer = super::Peer::token(token["id"].as_str().unwrap(), super::Scope::Add);
    let mut remote = TestClient::connect_as(&engine, peer);
    assert_eq!(remote.result("engine.hello", hello()).await["scope"], "add");
    assert_eq!(
        remote
            .error_code(
                "task.add",
                json!({"sources": [{"url": server.url("/a.bin")}], "destDir": dir_text(dir.path())})
            )
            .await,
        "DEST_FORBIDDEN"
    );
    let id = remote
        .result(
            "task.add",
            json!({"sources": [{"url": server.url("/a.bin")}]}),
        )
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Portée `add` : ni lecture ni contrôle.
    assert_eq!(
        remote.error_code("task.get", json!({"taskId": id})).await,
        "UNAUTHORIZED"
    );
    local
        .result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let task = local.result("task.get", json!({"taskId": id})).await;
    assert_eq!(task["publicOnly"], false, "{task}");
    if task["state"] != "completed" {
        local
            .notification("task.stateChanged", |p| {
                p["taskId"] == id.as_str() && p["to"] == "completed"
            })
            .await;
    }
    assert!(downloads.join("a.bin").exists());
    crate::testing::stop(&engine).await;
}

/// `subDir` (étape 3c) : la page distante range un ajout dans un sous-dossier, créé sous le
/// dossier des téléchargements, pour tout téléchargement ; jamais au-dessus. Une connexion à
/// jeton ne voit pas les réglages de l'écoute.
#[tokio::test]
async fn a_sub_folder_is_created_under_the_destination_and_never_escapes_it() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("dl");
    std::fs::create_dir_all(&downloads).unwrap();
    let elsewhere = dir.path().join("ailleurs");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let engine = engine(dir.path());
    let mut local = TestClient::connect(&engine);
    local.result("engine.hello", hello()).await;
    local
        .result(
            "settings.set",
            json!({"values": {"downloads.dir": dir_text(&downloads)}}),
        )
        .await;
    let token = local
        .result("token.create", json!({"name": "tél", "scope": "control"}))
        .await;
    let peer = super::Peer::token(token["id"].as_str().unwrap(), super::Scope::Control);
    let mut remote = TestClient::connect_as(&engine, peer);
    remote.result("engine.hello", hello()).await;

    let add = |sub: &str| {
        json!({"sources": [{"url": "https://exemple.org/a.iso"}], "subDir": sub,
               "startPaused": true})
    };
    let id = remote.result("task.add", add("ISO/Debian/")).await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = remote.result("task.get", json!({"taskId": id})).await;
    let wanted = downloads.join("ISO").join("Debian");
    assert_eq!(task["destDir"], json!(dir_text(&wanted)), "{task}");
    assert!(wanted.is_dir());
    for bad in [
        "..",
        "../ailleurs",
        "a/../../ailleurs",
        "a/./b",
        "1/2/3/4/5/6/7/8/9",
    ] {
        assert_eq!(
            remote.error_code("task.add", add(bad)).await,
            "INVALID_PARAMS",
            "{bad}"
        );
    }
    // Un nom que le système refuserait : refusé tel quel, jamais corrigé (« \ » partout,
    // « : » sous Windows seulement).
    let refused: &[&str] = if cfg!(windows) {
        &["a\\b", "a:b"]
    } else {
        &["a\\b"]
    };
    for bad in refused {
        assert_eq!(
            remote.error_code("task.add", add(bad)).await,
            "INVALID_PARAMS",
            "{bad}"
        );
    }
    // Ailleurs, même avec un sous-dossier : refusé.
    let outside = json!({"sources": [{"url": "https://exemple.org/b.iso"}],
                         "destDir": dir_text(&elsewhere), "subDir": "x", "startPaused": true});
    assert_eq!(
        remote.error_code("task.add", outside).await,
        "DEST_FORBIDDEN"
    );
    assert!(!elsewhere.join("x").exists());
    // Un sous-dossier déjà là qui mène ailleurs (lien) : refusé.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&elsewhere, downloads.join("lien")).unwrap();
        assert_eq!(
            remote.error_code("task.add", add("lien")).await,
            "DEST_FORBIDDEN"
        );
    }
    // Par lot aussi.
    let batch = remote
        .result(
            "task.addBatch",
            json!({"items": [add("Lot"), add("../Lot")]}),
        )
        .await;
    assert_eq!(batch["tasks"].as_array().unwrap().len(), 1, "{batch}");
    assert_eq!(batch["errors"][0]["code"], "INVALID_PARAMS", "{batch}");
    assert!(downloads.join("Lot").is_dir());

    // Un torrent (lien magnet) aussi, depuis le canal local.
    let magnet = json!({"sources": [{"url": "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=essai"}],
                        "destDir": dir_text(&downloads), "subDir": "Torrents", "startPaused": true});
    let id = local.result("task.add", magnet).await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = local.result("task.get", json!({"taskId": id})).await;
    assert_eq!(
        task["destDir"],
        json!(dir_text(&downloads.join("Torrents"))),
        "{task}"
    );

    // Réglages : la connexion à jeton ne voit pas ceux de l'écoute.
    let seen = remote.result("settings.get", json!({})).await["values"].clone();
    assert!(seen.get("downloads.dir").is_some(), "{seen}");
    for key in [
        "remote.enabled",
        "remote.allowPrivate",
        "remote.listen",
        "remote.hosts",
        "remote.tlsCert",
        "remote.tlsKey",
    ] {
        assert!(seen.get(key).is_none(), "{key} : {seen}");
    }
    let all = local.result("settings.get", json!({})).await["values"].clone();
    assert!(all.get("remote.listen").is_some(), "{all}");
    // Ni dans l'instantané, ni dans les annonces (audit R1) : la révision arrive, sans la
    // valeur ; les autres réglages restent annoncés.
    let snapshot = remote.result("sync.snapshot", json!({})).await["settings"].clone();
    assert!(snapshot.get("downloads.dir").is_some(), "{snapshot}");
    assert!(
        snapshot
            .as_object()
            .unwrap()
            .keys()
            .all(|k| !k.starts_with("remote.")),
        "{snapshot}"
    );
    remote
        .result("events.subscribe", json!({"topics": ["queues"]}))
        .await;
    local
        .result(
            "settings.set",
            json!({"values": {"remote.hosts": ["nas.maison"], "remote.allowPrivate": true}}),
        )
        .await;
    local
        .result(
            "settings.set",
            json!({"values": {"downloads.preallocate": false}}),
        )
        .await;
    // Chaque message reçu jusqu'à l'annonce du réglage ordinaire est relu.
    let mut hidden = 0;
    loop {
        let message = match remote.pending.pop_front() {
            Some(m) => m,
            None => remote.read().await.expect("connexion fermée"),
        };
        let text = message.to_string();
        assert!(
            !text.contains("nas.maison") && !text.contains("remote."),
            "annonce vue par le jeton : {text}"
        );
        if message["method"] == "settings.updated" {
            let values = &message["params"]["values"];
            if values.get("downloads.preallocate").is_some() {
                assert_eq!(values["downloads.preallocate"], false);
                break;
            }
            assert_eq!(values, &json!({}), "{text}");
            hidden += 1;
        }
    }
    assert!(
        hidden >= 2,
        "révisions des réglages cachés annoncées : {hidden}"
    );
    crate::testing::stop(&engine).await;
}

/// Audit D1 : sur une tâche locale hors du dossier des téléchargements, une connexion à
/// jeton ne change ni l'adresse, ni le nom, ne retélécharge pas et n'ajoute pas de miroir :
/// sinon, elle ferait écrire son contenu dans ce dossier. Sur une tâche du dossier, si.
#[tokio::test]
async fn a_token_cannot_rewrite_a_task_outside_the_download_folder() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("dl");
    std::fs::create_dir_all(&downloads).unwrap();
    let elsewhere = dir.path().join("Outils");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let engine = engine(dir.path());
    let mut local = TestClient::connect(&engine);
    local.result("engine.hello", hello()).await;
    local
        .result(
            "settings.set",
            json!({"values": {"downloads.dir": dir_text(&downloads)}}),
        )
        .await;
    let add = |dest: &Path, name: &str| {
        json!({"sources": [{"url": format!("https://exemple.org/{name}")}],
               "destDir": dir_text(dest), "startPaused": true})
    };
    let outside = local.result("task.add", add(&elsewhere, "outil.exe")).await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let inside = local.result("task.add", add(&downloads, "a.iso")).await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = local
        .result("token.create", json!({"name": "tél", "scope": "control"}))
        .await;
    let peer = super::Peer::token(token["id"].as_str().unwrap(), super::Scope::Control);
    let mut remote = TestClient::connect_as(&engine, peer);
    remote.result("engine.hello", hello()).await;

    let attempts = [
        (
            "task.update",
            json!({"taskId": outside, "url": "https://attaquant.example/outil.exe"}),
        ),
        (
            "task.update",
            json!({"taskId": outside, "fileName": "autre.exe"}),
        ),
        (
            "task.addSource",
            json!({"taskId": outside, "url": "https://attaquant.example/outil.exe"}),
        ),
    ];
    for (method, params) in attempts {
        assert_eq!(
            remote.error_code(method, params.clone()).await,
            "DEST_FORBIDDEN",
            "{method} {params}"
        );
    }
    // Un retéléchargement rend ses refus tâche par tâche.
    let restarted = remote
        .result(
            "task.restart",
            json!({"taskIds": [outside], "deleteFiles": true}),
        )
        .await;
    assert_eq!(restarted["taskIds"], json!([]), "{restarted}");
    assert_eq!(
        restarted["errors"][0]["code"], "DEST_FORBIDDEN",
        "{restarted}"
    );
    let task = local.result("task.get", json!({"taskId": outside})).await;
    assert_eq!(task["displayName"], "outil.exe", "{task}");

    // Sur une tâche du dossier des téléchargements : permis.
    remote
        .result(
            "task.update",
            json!({"taskId": inside, "url": "https://exemple.org/b.iso"}),
        )
        .await;
    crate::testing::stop(&engine).await;
}

/// `remote.status` donne les adresses où l'écoute peut s'ouvrir (étape 3d) : jamais la
/// boucle locale ni une adresse locale au lien, chacune avec le nom de son interface.
#[tokio::test]
async fn remote_status_lists_the_addresses_where_listening_can_open() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut local = TestClient::connect(&engine);
    local.result("engine.hello", hello()).await;
    let status = local.result("remote.status", json!({})).await;
    let addresses = status["addresses"].as_array().expect("addresses");
    for choice in addresses {
        let ip: std::net::IpAddr = choice["address"].as_str().unwrap().parse().unwrap();
        assert!(!ip.is_loopback() && !ip.is_unspecified(), "{choice}");
        if let std::net::IpAddr::V4(v4) = ip {
            assert!(!v4.is_link_local(), "{choice}");
        }
        assert!(
            choice["interface"].as_str().is_some_and(|n| !n.is_empty()),
            "{choice}"
        );
    }
    crate::testing::stop(&engine).await;
}

/// Plugins (SPEC §7.20) : montrés, installés sur l'empreinte acceptée, servis à l'ajout par
/// une connexion à jeton `add` (sans jamais lui montrer les cookies), désactivés, retirés
/// avec leur fichier ; gestion réservée au canal local.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugins_are_installed_on_consent_used_to_add_and_removed() {
    use crate::testing::plugin_files::{MANIFEST, fetch_call, packed, plugin};
    use base64::Engine as _;

    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("dl");
    std::fs::create_dir_all(&downloads).unwrap();
    let server = crate::testing::serve(|request| {
        if request.path == "/page" {
            crate::testing::reply(
                "200 OK",
                &[("Set-Cookie", "session=secret-du-site; Path=/")],
                b"page",
            )
        } else {
            crate::testing::reply("404 Not Found", &[], b"")
        }
    });
    let engine = engine(dir.path());
    let addr: std::net::SocketAddr = server.base.trim_start_matches("http://").parse().unwrap();
    engine.plugins_resolve_to("fichiers.exemple", addr);
    let mut local = TestClient::connect(&engine);
    local.result("engine.hello", hello()).await;
    local
        .result(
            "settings.set",
            json!({"values": {"downloads.dir": dir_text(&downloads)}}),
        )
        .await;

    let page = r#"{"url": "http://fichiers.exemple/page"}"#;
    let links = r#"{"links": [{"url": "http://fichiers.exemple/dl/f.iso", "name": "f.iso", "size": 5},
        {"url": "http://fichiers.exemple/dl/g.iso", "name": "g.iso"}]}"#;
    let resolve = format!("(drop {}) {}", fetch_call(1, page), packed(0, links));
    let bytes = plugin(MANIFEST, &[links, page], &resolve, "");
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let shown = local.result("plugin.inspect", json!({"data": data})).await;
    assert_eq!(shown["pluginId"], "org.essai.fichiers", "{shown}");
    assert_eq!(shown["sites"], json!(["fichiers.exemple"]));
    let digest = shown["digest"].as_str().unwrap().to_owned();
    // Rien d'installé tant que l'empreinte acceptée n'est pas celle du fichier.
    assert_eq!(
        local
            .error_code(
                "plugin.install",
                json!({"data": data, "digest": "0".repeat(64)})
            )
            .await,
        "INVALID_PARAMS"
    );
    assert_eq!(
        local.result("plugin.list", json!({})).await["plugins"],
        json!([])
    );
    let installed = local
        .result("plugin.install", json!({"data": data, "digest": digest}))
        .await;
    assert_eq!(installed["enabled"], true, "{installed}");
    let file = dir.path().join("plugins").join(format!("{digest}.wasm"));
    assert!(file.is_file());
    let found = local
        .result(
            "plugin.match",
            json!({"url": "http://www.fichiers.exemple/f/1"}),
        )
        .await;
    assert_eq!(found["plugin"]["pluginId"], "org.essai.fichiers");
    assert_eq!(
        local
            .result("plugin.match", json!({"url": "https://autre.exemple/"}))
            .await["plugin"],
        Value::Null
    );

    // Une connexion à jeton `add` s'en sert, sans pouvoir en installer.
    let token = local
        .result("token.create", json!({"name": "tél", "scope": "add"}))
        .await;
    let peer = super::Peer::token(token["id"].as_str().unwrap(), super::Scope::Add);
    let mut remote = TestClient::connect_as(&engine, peer);
    remote.result("engine.hello", hello()).await;
    for method in [
        "plugin.install",
        "plugin.list",
        "plugin.remove",
        "plugin.enable",
    ] {
        assert_eq!(
            remote
                .error_code(
                    method,
                    json!({"data": data, "digest": digest, "pluginId": "x", "enabled": false})
                )
                .await,
            "UNAUTHORIZED",
            "{method}"
        );
    }
    let resolved = remote
        .result(
            "plugin.resolve",
            json!({"url": "http://fichiers.exemple/page"}),
        )
        .await;
    assert_eq!(resolved["links"].as_array().unwrap().len(), 2, "{resolved}");
    assert_eq!(resolved["links"][0]["size"], 5);
    assert!(
        !resolved.to_string().contains("secret-du-site"),
        "{resolved}"
    );
    let resolution = resolved["resolutionId"].as_str().unwrap().to_owned();
    for bad in [json!([]), json!([2]), json!([1, 1])] {
        assert_eq!(
            remote
                .error_code(
                    "plugin.add",
                    json!({"resolutionId": resolution, "links": bad})
                )
                .await,
            "INVALID_PARAMS",
            "{bad}"
        );
    }
    assert_eq!(
        remote
            .error_code(
                "plugin.add",
                json!({"resolutionId": "inconnue", "links": [0]})
            )
            .await,
        "PLUGIN_NOT_FOUND"
    );
    let added = remote
        .result(
            "plugin.add",
            json!({"resolutionId": resolution, "links": [1], "startPaused": true, "subDir": "Hébergeur"}),
        )
        .await;
    assert_eq!(added["errors"], json!([]), "{added}");
    let id = added["tasks"][0]["taskId"].as_str().unwrap().to_owned();
    let task = local.result("task.get", json!({"taskId": id})).await;
    assert_eq!(task["displayName"], "g.iso", "{task}");
    assert_eq!(task["publicOnly"], true, "{task}");
    assert_eq!(
        task["destDir"],
        json!(dir_text(&downloads.join("Hébergeur")))
    );
    assert!(!task.to_string().contains("secret-du-site"), "{task}");
    // En base : le Referer (la page) en clair, le cookie du site par le coffre seulement,
    // et la page d'origine pour un lien neuf.
    {
        let db = rusqlite::Connection::open(dir.path().join("auro.db")).unwrap();
        let mut stmt = db
            .prepare(
                "SELECT name, value, secret_ref FROM task_headers WHERE task_id = ?1 ORDER BY name",
            )
            .unwrap();
        let headers: Vec<(String, Option<String>, Option<String>)> = stmt
            .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(headers.len(), 2, "{headers:?}");
        assert_eq!(headers[0].0.to_ascii_lowercase(), "cookie");
        assert!(
            headers[0].1.is_none() && headers[0].2.is_some(),
            "{headers:?}"
        );
        assert_eq!(headers[1].0.to_ascii_lowercase(), "referer");
        assert_eq!(
            headers[1].1.as_deref(),
            Some("http://fichiers.exemple/page")
        );
        let origin: (String, String) = db
            .query_row(
                "SELECT plugin_page, plugin_id FROM tasks WHERE id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            origin,
            (
                "http://fichiers.exemple/page".into(),
                "org.essai.fichiers".into()
            )
        );
    }

    // Désactivé : plus de plugin pour ce site.
    let off = local
        .result(
            "plugin.enable",
            json!({"pluginId": "org.essai.fichiers", "enabled": false}),
        )
        .await;
    assert_eq!(off["enabled"], false);
    assert_eq!(
        remote
            .error_code(
                "plugin.resolve",
                json!({"url": "http://fichiers.exemple/page"})
            )
            .await,
        "PLUGIN_NOT_FOUND"
    );
    // Retiré, avec son fichier.
    local
        .result("plugin.remove", json!({"pluginId": "org.essai.fichiers"}))
        .await;
    assert!(!file.exists());
    assert_eq!(
        local
            .error_code("plugin.remove", json!({"pluginId": "org.essai.fichiers"}))
            .await,
        "PLUGIN_NOT_FOUND"
    );
    crate::testing::stop(&engine).await;
}

/// Plugins (SPEC §7.20) : `task.add` d'une page garde son lien s'il est seul, refuse s'il
/// y en a plusieurs (`PLUGIN_CHOICE`), ajoute l'adresse telle quelle avec `plugin: false` ;
/// un lien expiré (403) est redemandé au plugin, une fois, puis la tâche reprend et finit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hoster_page_keeps_its_single_link_and_an_expired_link_is_renewed() {
    use crate::testing::plugin_files::{fetch_call, packed, plugin};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const LEN: u64 = 300_000;
    let dir = tempfile::tempdir().unwrap();
    let page_hits = Arc::new(AtomicUsize::new(0));
    let site = Arc::new(Mutex::new(FileSite::new(LEN)));
    let file = crate::testing::file_handler(site);
    let hits = Arc::clone(&page_hits);
    let server = crate::testing::serve(move |request| match request.path.as_str() {
        "/page" => {
            hits.fetch_add(1, Ordering::SeqCst);
            crate::testing::reply("200 OK", &[], b"page")
        }
        // Le lien ne vaut qu'après la seconde lecture de la page.
        "/dl/f.bin" if hits.load(Ordering::SeqCst) < 2 => {
            crate::testing::reply("403 Forbidden", &[("Content-Length", "0")], b"")
        }
        "/dl/f.bin" => file(request),
        _ => crate::testing::reply("404 Not Found", &[("Content-Length", "0")], b""),
    });
    let engine = engine(dir.path());
    let addr: std::net::SocketAddr = server.base.trim_start_matches("http://").parse().unwrap();
    engine.plugins_resolve_to("fichiers.exemple", addr);
    engine.plugins_resolve_to("multi.exemple", addr);
    engine.plugins_allow_private();

    let manifest = |id: &str, site: &str| {
        format!(
            r#"{{"abi": 1, "id": "{id}", "name": "{id}", "version": "1", "sites": ["{site}"], "fetch": ["{site}"]}}"#
        )
    };
    let page = r#"{"url": "http://fichiers.exemple/page"}"#;
    let one = format!(
        r#"{{"links": [{{"url": "{}", "name": "f.bin"}}]}}"#,
        server.url("/dl/f.bin")
    );
    let single = plugin(
        &manifest("org.essai.un", "fichiers.exemple"),
        &[&one, page],
        &format!("(drop {}) {}", fetch_call(1, page), packed(0, &one)),
        "",
    );
    let two = r#"{"links": [{"url": "https://a.exemple/1"}, {"url": "https://a.exemple/2"}]}"#;
    let several = plugin(
        &manifest("org.essai.deux", "multi.exemple"),
        &[two],
        &packed(0, two),
        "",
    );
    for bytes in [single, several] {
        let digest = crate::plugins::digest_of(&bytes);
        engine.install_plugin(bytes, &digest).await.unwrap();
    }

    let mut local = TestClient::connect(&engine);
    local.result("engine.hello", hello()).await;
    local
        .result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let add = |url: &str| json!({"sources": [{"url": url}], "destDir": dir_text(dir.path())});
    assert_eq!(
        local
            .error_code("task.add", add("https://multi.exemple/dossier"))
            .await,
        "PLUGIN_CHOICE"
    );
    // `plugin: false` : l'adresse telle quelle, sans lancer le plugin.
    let mut raw = add("https://multi.exemple/dossier");
    raw["plugin"] = json!(false);
    raw["startPaused"] = json!(true);
    let plain = local.result("task.add", raw).await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let plain_task = local.result("task.get", json!({"taskId": plain})).await;
    assert_eq!(plain_task["displayName"], "dossier");
    assert!(plain_task.get("plugin").is_none());

    // Lien seul : ajouté ; refusé (403) ; redemandé au plugin ; repris jusqu'au bout.
    let id = local
        .result("task.add", add("http://fichiers.exemple/page"))
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Sa page et son plugin, dans `task.get`.
    let origin = &local.result("task.get", json!({"taskId": id})).await["plugin"];
    assert_eq!(origin["page"], "http://fichiers.exemple/page");
    assert_eq!(origin["pluginId"], "org.essai.un");
    assert!(origin["name"].is_string());
    local
        .notification("task.stateChanged", |p| {
            p["taskId"] == id.as_str() && p["to"] == "failed"
        })
        .await;
    local
        .notification("task.stateChanged", |p| {
            p["taskId"] == id.as_str() && p["to"] == "completed"
        })
        .await;
    assert_eq!(page_hits.load(Ordering::SeqCst), 2);
    crate::testing::same_as_served(&dir.path().join("f.bin"), 0, LEN).unwrap();
    crate::testing::stop(&engine).await;
}

/// Revue de sécurité (étape 5) : un lien neuf refusé (ici injoignable) ne laisse pas ses
/// cookies à la tâche, qui garde son ancienne adresse : une reprise à la main ne les
/// envoie pas à l'ancien hôte.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_fresh_link_leaves_no_cookie_on_the_old_address() {
    use crate::testing::plugin_files::{fetch_call, packed, plugin};

    const LEN: u64 = 100_000;
    let dir = tempfile::tempdir().unwrap();
    let site = Arc::new(Mutex::new(FileSite::new(LEN)));
    let file = crate::testing::file_handler(site);
    let server = crate::testing::serve(move |request| match request.path.as_str() {
        "/page" => crate::testing::reply(
            "200 OK",
            &[("Set-Cookie", "session=secret-du-site; Path=/")],
            b"page",
        ),
        // Préparation possible, téléchargement refusé : un lien expiré.
        "/dl/f.bin" if request.method == "HEAD" => file(request),
        "/dl/f.bin" => crate::testing::reply("403 Forbidden", &[("Content-Length", "0")], b""),
        _ => crate::testing::reply("404 Not Found", &[("Content-Length", "0")], b""),
    });
    let engine = engine(dir.path());
    let addr: std::net::SocketAddr = server.base.trim_start_matches("http://").parse().unwrap();
    engine.plugins_resolve_to("fichiers.exemple", addr);
    engine.plugins_resolve_to("autre.exemple", addr);
    engine.plugins_allow_private();
    let manifest = r#"{"abi": 1, "id": "org.essai.renouvele", "name": "Renouvelé", "version": "1",
        "sites": ["fichiers.exemple"], "fetch": ["fichiers.exemple", "autre.exemple"]}"#;
    let version = |page: &str, link: &str| {
        let out = format!(r#"{{"links": [{{"url": "{link}", "name": "f.bin"}}]}}"#);
        let page = format!(r#"{{"url": "{page}"}}"#);
        plugin(
            manifest,
            &[&out, &page],
            &format!("(drop {}) {}", fetch_call(1, &page), packed(0, &out)),
            "",
        )
    };
    let install = |bytes: Vec<u8>| {
        let engine = engine.clone();
        async move {
            let digest = crate::plugins::digest_of(&bytes);
            engine.install_plugin(bytes, &digest).await.unwrap();
        }
    };
    // Version 1 : le lien de la tâche, sur ce serveur, sans cookie pour lui.
    install(version(
        "http://fichiers.exemple/page",
        &server.url("/dl/f.bin"),
    ))
    .await;

    let mut local = TestClient::connect(&engine);
    local.result("engine.hello", hello()).await;
    local
        .result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let id = local
        .result(
            "task.add",
            json!({"sources": [{"url": "http://fichiers.exemple/page"}],
                   "destDir": dir_text(dir.path())}),
        )
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Refusé (403), renouvelé une fois par le moteur (même lien), refusé de nouveau ; le
    // moteur ne renouvelle plus avant 10 minutes.
    for _ in 0..2 {
        local
            .notification("task.stateChanged", |p| {
                p["taskId"] == id.as_str() && p["to"] == "failed"
            })
            .await;
    }

    // Version 2 : un lien sur un autre hôte, avec ses cookies, que le moteur ne joint pas.
    install(version(
        "http://autre.exemple/page",
        &format!("http://autre.exemple:{}/dl/f.bin", addr.port()),
    ))
    .await;
    assert!(engine.refresh_plugin_link(&id).await.is_err());
    assert!(
        server.requests().iter().any(|r| r.path == "/page"
            && r.header("host")
                .is_some_and(|h| h.starts_with("autre.exemple"))),
        "la version 2 a lu sa page (et reçu son cookie)"
    );

    // Reprise à la main : l'ancienne adresse ne reçoit aucun cookie.
    let before = server.requests().len();
    local.result("task.resume", json!({"taskIds": [id]})).await;
    local
        .notification("task.stateChanged", |p| {
            p["taskId"] == id.as_str() && p["to"] == "failed"
        })
        .await;
    let after: Vec<_> = server.requests()[before..]
        .iter()
        .filter(|r| r.path == "/dl/f.bin")
        .cloned()
        .collect();
    assert!(!after.is_empty());
    assert!(
        after.iter().all(|r| r.header("cookie").is_none()),
        "{:?}",
        after.iter().map(|r| r.header("cookie")).collect::<Vec<_>>()
    );
    crate::testing::stop(&engine).await;
}

/// Audit F1 : une adresse qui ne porte que le nom (`sftp://moi@hôte/f`) prend le mot de
/// passe de l'identifiant du site pour ce nom ; avant le correctif, un en-tête au mot de
/// passe vide le masquait et la tâche échouait en `AUTH_FAILED`.
#[tokio::test]
async fn an_sftp_address_with_a_user_only_takes_the_site_password() {
    use crate::testing::sftp::{SftpOptions, SftpSite};
    const LEN: usize = 100_000;
    let dir = tempfile::tempdir().unwrap();
    let site = SftpSite::start(SftpOptions {
        password: Some(("moi", "s3cr3t-ssh")),
        ..SftpOptions::default()
    })
    .await;
    site.put("pub/f.bin", &crate::testing::content(LEN));
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    c.result(
        "login.set",
        json!({"site": format!("sftp://{}", site.addr), "username": "moi", "password": "s3cr3t-ssh"}),
    )
    .await;
    let url = format!("sftp://moi@{}/pub/f.bin", site.addr);
    let id = c
        .result(
            "task.add",
            json!({"sources": [{"url": url}], "destDir": dir_text(dir.path())}),
        )
        .await["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id.as_str() && p["to"] == "failed"
    })
    .await;
    let keys = c.result("hostkey.list", json!({})).await;
    let pending = &keys["pending"][0];
    c.result(
        "hostkey.accept",
        json!({"host": pending["host"], "port": pending["port"], "fingerprint": site.fingerprint()}),
    )
    .await;
    let end = c
        .notification("task.stateChanged", |p| {
            p["taskId"] == id.as_str() && matches!(p["to"].as_str(), Some("completed" | "failed"))
        })
        .await;
    let task = c.result("task.get", json!({"taskId": id})).await;
    assert_eq!(end["to"], "completed", "{task}");
    assert_eq!(
        std::fs::read(dir.path().join("f.bin")).unwrap(),
        crate::testing::content(LEN)
    );
    crate::testing::stop(&engine).await;
}

/// SFTP (F-08, SPEC §7.22) : une clé de serveur inconnue arrête la tâche ; `hostkey.list`
/// la montre, `hostkey.accept` l'enregistre et relance la tâche, qui finit ; une clé qui
/// change ensuite bloque de nouveau. Les identifiants de l'adresse vont au coffre.
#[tokio::test]
async fn an_sftp_host_key_is_accepted_then_the_task_resumes() {
    use crate::testing::sftp::{SftpOptions, SftpSite};
    const LEN: usize = 600_000;
    let dir = tempfile::tempdir().unwrap();
    let site = SftpSite::start(SftpOptions {
        password: Some(("moi", "s3cr3t-ssh")),
        ..SftpOptions::default()
    })
    .await;
    site.put("pub/f.bin", &crate::testing::content(LEN));
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let url = format!("sftp://moi:s3cr3t-ssh@{}/pub/f.bin", site.addr);
    let added = c
        .result(
            "task.add",
            json!({"sources": [{"url": url}], "destDir": dir_text(dir.path())}),
        )
        .await;
    let id = added["taskId"].as_str().unwrap().to_owned();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id.as_str() && p["to"] == "failed"
    })
    .await;
    let task = c.result("task.get", json!({"taskId": id})).await;
    assert_eq!(task["errorCode"], "HOST_KEY_UNKNOWN", "{task}");
    assert!(!task.to_string().contains("s3cr3t"), "{task}");

    let keys = c.result("hostkey.list", json!({})).await;
    assert_eq!(keys["items"], json!([]));
    let pending = &keys["pending"][0];
    assert_eq!(pending["fingerprint"], site.fingerprint().as_str());
    assert_eq!(pending["changed"], false);
    let (host, port) = (pending["host"].clone(), pending["port"].clone());
    assert_eq!(
        c.error_code(
            "hostkey.accept",
            json!({"host": host, "port": port, "fingerprint": "SHA256:autre"}),
        )
        .await,
        "INVALID_PARAMS"
    );
    let accepted = c
        .result(
            "hostkey.accept",
            json!({"host": host, "port": port, "fingerprint": site.fingerprint()}),
        )
        .await;
    assert_eq!(accepted["resumed"], json!([id]));
    c.notification("task.stateChanged", |p| {
        p["taskId"] == id.as_str() && p["to"] == "completed"
    })
    .await;
    assert_eq!(
        std::fs::read(dir.path().join("f.bin")).unwrap(),
        crate::testing::content(LEN)
    );
    let keys = c.result("hostkey.list", json!({})).await;
    assert_eq!(keys["items"][0]["fingerprint"], site.fingerprint().as_str());
    assert_eq!(keys["pending"], json!([]));

    // Clé changée, après un redémarrage (la clé acceptée est relue de la base ; une
    // connexion déjà ouverte, elle, a été vérifiée à son ouverture) : la tâche suivante est
    // bloquée, la nouvelle empreinte en attente.
    crate::testing::stop(&engine).await;
    site.replace_host_key();
    let engine = self::engine(dir.path());
    let mut c = TestClient::connect(&engine);
    c.result("engine.hello", hello()).await;
    c.result("events.subscribe", json!({"topics": ["tasks"]}))
        .await;
    let again = c
        .result(
            "task.add",
            json!({"sources": [{"url": url}], "destDir": dir_text(dir.path()), "fileName": "g.bin"}),
        )
        .await;
    let again = again["taskId"].as_str().unwrap().to_owned();
    c.notification("task.stateChanged", |p| {
        p["taskId"] == again.as_str() && p["to"] == "failed"
    })
    .await;
    let task = c.result("task.get", json!({"taskId": again})).await;
    assert_eq!(task["errorCode"], "HOST_KEY_CHANGED", "{task}");
    let keys = c.result("hostkey.list", json!({})).await;
    assert_eq!(keys["pending"][0]["changed"], true);
    let removed = c
        .result("hostkey.remove", json!({"host": host, "port": port}))
        .await;
    assert_eq!(removed["removed"], true);
    assert_eq!(
        c.result("hostkey.list", json!({})).await["items"],
        json!([])
    );
    crate::testing::stop(&engine).await;
}

/// Audit C16 : un fichier `.torrent` lu à son adresse par le moteur, rendu en base64 ; une
/// page qui n'en est pas un est refusée ; réseau dit inutilisable : rien n'est lu.
#[tokio::test]
async fn a_torrent_file_is_fetched_by_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let mut c = TestClient::connect(&engine);
    let metainfo = b"d4:infod6:lengthi3e4:name5:a.bin12:piece lengthi16384e6:pieces0:ee".to_vec();
    let served = metainfo.clone();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    let host = crate::testing::serve(move |r| {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match r.path.as_str() {
            "/a.torrent" => crate::testing::reply(
                "200 OK",
                &[("Content-Type", "application/x-bittorrent")],
                &served,
            ),
            "/page.html" => crate::testing::reply("200 OK", &[], b"<html></html>"),
            _ => crate::testing::reply("404 Not Found", &[("Content-Length", "0")], b""),
        }
    });
    let fetched = c
        .result("torrent.fetch", json!({"url": host.url("/a.torrent")}))
        .await;
    let data = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        fetched["data"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(data, metainfo);
    for (url, code) in [
        (host.url("/page.html"), "INVALID_PARAMS"),
        (host.url("/absent.torrent"), "INVALID_PARAMS"),
        ("ftp://exemple.org/a.torrent".to_owned(), "URL_INVALID"),
    ] {
        assert_eq!(
            c.error_code("torrent.fetch", json!({"url": url})).await,
            code,
            "{url}"
        );
    }
    assert_eq!(
        c.error_code("torrent.fetch", json!({})).await,
        "INVALID_PARAMS"
    );
    // Réseau inutilisable (Android, « Wi-Fi seulement ») : refusé sans requête.
    c.result("engine.setNetwork", json!({"usable": false}))
        .await;
    let before = hits.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        c.error_code("torrent.fetch", json!({"url": host.url("/a.torrent")}))
            .await,
        "SOURCE_UNREACHABLE"
    );
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), before);
    crate::testing::stop(&engine).await;
}

/// Audit R12, mesure (ignorée par défaut, relevé dans `docs/MESURES.md`) : un lot de 1 000
/// ajouts suivi par 4 abonnés ; chaque abonné doit recevoir ses 1 000 `task.added`, sans
/// `EVENTS_LOST`.
/// `cargo test --release -p auro-engine --lib -- --ignored r12_ --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn r12_a_batch_of_additions_reaches_every_subscriber() {
    const TASKS: usize = 1000;
    const SUBSCRIBERS: usize = 4;
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let started = std::time::Instant::now();
    let mut readers = Vec::new();
    for _ in 0..SUBSCRIBERS {
        let mut c = TestClient::connect(&engine);
        c.result("engine.hello", hello()).await;
        c.result("events.subscribe", json!({"topics": ["tasks"]}))
            .await;
        readers.push(tokio::spawn(async move {
            let (mut added, mut lost) = (0, 0);
            let mut last = Duration::ZERO;
            while added < TASKS {
                let Ok(Ok(Some(line))) =
                    timeout(Duration::from_secs(20), c.lines.next_line()).await
                else {
                    break;
                };
                if line.contains("\"task.added\"") {
                    added += 1;
                    last = started.elapsed();
                } else if line.contains("EVENTS_LOST") {
                    lost += 1;
                }
            }
            (added, lost, last)
        }));
    }
    let mut adder = TestClient::connect(&engine);
    adder.result("engine.hello", hello()).await;
    let items: Vec<Value> = (0..TASKS)
        .map(|i| {
            json!({
                "sources": [{"url": format!("http://127.0.0.1:9/f{i}.bin")}],
                "destDir": dir_text(dir.path()),
                "startPaused": true,
            })
        })
        .collect();
    let sent = started.elapsed();
    let batch = adder.result("task.addBatch", json!({"items": items})).await;
    let accepted = started.elapsed() - sent;
    assert_eq!(batch["tasks"].as_array().unwrap().len(), TASKS, "{batch}");
    let mut complete = true;
    for reader in readers {
        let (added, lost, last) = reader.await.unwrap();
        println!(
            "abonné : {added} task.added, {lost} EVENTS_LOST, dernier {:?} après l'envoi",
            last.saturating_sub(sent)
        );
        complete &= added == TASKS && lost == 0;
    }
    println!("lot de {TASKS} accepté en {accepted:?}");
    crate::testing::stop(&engine).await;
    assert!(complete, "un abonné n'a pas tout reçu");
}
