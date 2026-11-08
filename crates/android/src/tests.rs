//! `Host` sur la machine de développement : la même connexion que dans l'application,
//! sans Android.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use auro_engine::testing::{FileSite, content, serve_file};
use serde_json::{Value, json};

use super::*;

const WAIT: Duration = Duration::from_secs(30);

/// Messages reçus du moteur, lus au fil des attentes.
struct Inbox(mpsc::Receiver<String>);

impl Inbox {
    fn start(db: &Path) -> (Host, Inbox) {
        let (tx, rx) = mpsc::channel();
        let host = Host::start(db, move |line| {
            let _ = tx.send(line);
        })
        .unwrap();
        (host, Inbox(rx))
    }

    /// Premier message qui satisfait `wanted` ; les autres sont passés.
    fn wait(&self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let end = Instant::now() + WAIT;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let line = self
                .0
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("pas de message : {what}"));
            let message: Value = serde_json::from_str(&line).expect("une ligne JSON par message");
            if wanted(&message) {
                return message;
            }
        }
    }

    fn reply(&self, id: u64) -> Value {
        let reply = self.wait(&format!("réponse {id}"), |m| m["id"] == json!(id));
        assert!(reply.get("error").is_none(), "{reply}");
        reply["result"].clone()
    }
}

fn call(host: &Host, id: u64, method: &str, params: Value) {
    host.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string());
}

#[test]
fn the_engine_answers_downloads_and_keeps_its_tasks_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("auro.db");
    let downloads = dir.path().join("dl");
    std::fs::create_dir(&downloads).unwrap();
    let len = 300_000;
    let server = serve_file(Arc::new(Mutex::new(FileSite::new(len as u64))));

    let (host, inbox) = Inbox::start(&db);
    // Un message écrit sur plusieurs lignes reste un seul message.
    host.send("{\"jsonrpc\": \"2.0\",\n \"id\": 1,\n \"method\": \"settings.get\"}");
    assert!(inbox.reply(1)["values"].is_object());

    // Le moteur de l'application démarre sans réseau : elle le dit utilisable.
    call(&host, 9, "engine.setNetwork", json!({"usable": true}));
    assert_eq!(inbox.reply(9)["usable"], true);
    call(&host, 2, "events.subscribe", json!({"topics": ["tasks"]}));
    inbox.reply(2);
    call(
        &host,
        3,
        "task.add",
        json!({"sources": [{"url": server.url("/a.bin")}],
               "destDir": downloads.to_str().unwrap()}),
    );
    let id = inbox.reply(3)["taskId"].as_str().unwrap().to_owned();
    inbox.wait("fin du téléchargement", |m| {
        m["method"] == "task.stateChanged"
            && m["params"]["taskId"] == json!(id)
            && m["params"]["to"] == "completed"
    });
    assert_eq!(
        std::fs::read(downloads.join("a.bin")).unwrap(),
        content(len)
    );
    host.stop();

    // Même base, nouveau démarrage : la tâche est toujours là, terminée.
    let (host, inbox) = Inbox::start(&db);
    call(&host, 1, "task.get", json!({"taskId": id}));
    assert_eq!(inbox.reply(1)["state"], "completed");
    host.stop();
}
