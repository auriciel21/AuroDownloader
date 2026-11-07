//! Relais de l'extension navigateur (SPEC §4.5) : messages de la messagerie native,
//! ajouts transmis au moteur avec la portée `add`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use auro_cli::relay::{self, MAX_INCOMING, read_message, write_message};
use auro_engine::net::Client;
use auro_engine::rpc::local;
use auro_engine::scheduler::{Config, DISK_RECHECK, Engine, Planning};
use auro_engine::testing::{FileSite, serve_file};
use auro_engine::transfer::Options;
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, duplex};

/// Point d'accès propre à ce test.
fn endpoint(dir: &std::path::Path) -> String {
    if cfg!(windows) {
        format!(r"\\.\pipe\auro-relay-test-{}", std::process::id())
    } else {
        dir.join("relay.sock").to_string_lossy().into_owned()
    }
}

#[test]
fn messages_are_length_prefixed_json() {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async {
            let (mut a, mut b) = duplex(1 << 16);
            let message = json!({"type": "ping", "id": 7, "texte": "é"});
            write_message(&mut a, &message).await.unwrap();
            assert_eq!(read_message(&mut b).await.unwrap(), Some(message));

            // Longueur hors limite : refusée sans lire la suite.
            a.write_all(&((MAX_INCOMING as u32) + 1).to_ne_bytes())
                .await
                .unwrap();
            assert!(read_message(&mut b).await.is_err());

            // Fin de l'entrée : plus de message.
            drop(a);
            assert_eq!(read_message(&mut b).await.unwrap(), None);
        });
}

#[tokio::test]
async fn the_relay_adds_downloads_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let site = serve_file(Arc::new(Mutex::new(FileSite::new(4096))));
    let engine = Engine::start(Config {
        db_path: dir.path().join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options::default(),
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap();
    engine
        .set_download_dir(Some(downloads.path().to_str().unwrap().to_owned()))
        .await
        .unwrap();
    let endpoint = endpoint(dir.path());
    tokio::spawn({
        let (engine, endpoint) = (engine.clone(), endpoint.clone());
        async move { local::listen(engine, &endpoint).await }
    });
    // À l'écoute avant le premier message : sinon le relais lancerait un vrai `aurod`.
    for _ in 0..100 {
        if local::connect(&endpoint).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Le navigateur : une entrée et une sortie du relais.
    let (mut browser, relay_side) = duplex(1 << 16);
    let (relay_in, relay_out) = tokio::io::split(relay_side);
    let serving = tokio::spawn({
        let endpoint = endpoint.clone();
        async move { relay::serve(relay_in, relay_out, &endpoint).await }
    });
    let mut ask = async |message: Value| -> Value {
        write_message(&mut browser, &message).await.unwrap();
        read_message(&mut browser).await.unwrap().unwrap()
    };

    let pong = ask(json!({"type": "ping", "id": 1})).await;
    assert_eq!(pong["ok"], true);
    assert_eq!(pong["id"], 1);

    let add = json!({
        "type": "add",
        "id": 2,
        "url": site.url("/extension.bin"),
        "requestId": "dl-42",
        "paused": true,
    });
    let first = ask(add.clone()).await;
    assert_eq!(first["ok"], true, "{first}");
    let task_id = first["taskId"].as_str().unwrap().to_owned();
    // Un nouvel essai de l'extension : la même tâche.
    let again = ask(add).await;
    assert_eq!(again["taskId"], task_id.as_str());

    // Dans le dossier du moteur, en pause comme demandé.
    let task = engine.get(&task_id).await.unwrap();
    assert_eq!(task.dest_dir, downloads.path().to_str().unwrap());
    assert_eq!(task.state.as_sql(), "paused");
    assert_eq!(engine.list(Default::default()).await.unwrap().0.len(), 1);

    for (message, code) in [
        (json!({"type": "add"}), "BAD_MESSAGE"),
        (json!({"url": "https://x"}), "BAD_MESSAGE"),
        (json!({"type": "remove", "taskId": task_id}), "UNKNOWN_TYPE"),
        (json!({"type": "add", "url": "ftp:/rien"}), "URL_INVALID"),
    ] {
        let reply = ask(message.clone()).await;
        assert_eq!(reply["ok"], false, "{message}");
        assert_eq!(reply["code"], code, "{message} : {reply}");
    }

    drop(browser);
    serving.await.unwrap().unwrap();
    engine.shutdown().await;
}
