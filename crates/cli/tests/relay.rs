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

    // Avec les en-têtes de la page : envoyés au serveur.
    let with_page = ask(json!({
        "type": "add",
        "url": site.url("/connecte.bin"),
        "requestId": "dl-43",
        "headers": {"Cookie": "session=1", "Referer": "https://exemple.org/", "User-Agent": "Navigateur/1"},
    }))
    .await;
    assert_eq!(with_page["ok"], true, "{with_page}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let sent = loop {
        if let Some(r) = site
            .requests()
            .into_iter()
            .find(|r| r.path == "/connecte.bin")
        {
            break r;
        }
        assert!(tokio::time::Instant::now() < deadline, "aucune requête");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(sent.header("cookie"), Some("session=1"));
    assert_eq!(sent.header("referer"), Some("https://exemple.org/"));
    assert_eq!(sent.header("user-agent"), Some("Navigateur/1"));

    for (message, code) in [
        (json!({"type": "add"}), "BAD_MESSAGE"),
        (
            json!({"type": "add", "url": "https://x/a", "headers": "Cookie: a"}),
            "BAD_MESSAGE",
        ),
        (
            json!({"type": "add", "url": "https://x/a", "headers": {"Host": "y"}}),
            "INVALID_PARAMS",
        ),
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

/// Étape 5d (SPEC §4.5) : liens magnet et `.torrent` en base64 (sans en-têtes de page :
/// le moteur les refuse pour un torrent), puis vidéo d'une page décrite et ajoutée.
#[cfg(any(windows, target_os = "linux"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_relay_adds_torrents_and_page_videos() {
    use auro_engine::testing::tools;
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let answer = json!({
        "title": "Essai du relais", "duration": 6.0, "extractor_key": "Essai",
        "formats": [
            {"format_id": "v1", "ext": "mp4", "vcodec": "avc1", "acodec": "none",
             "protocol": "http", "url": "http://127.0.0.1:9/v.mp4", "height": 720,
             "filesize": 1000},
            {"format_id": "a1", "ext": "m4a", "vcodec": "none", "acodec": "mp4a",
             "protocol": "http", "url": "http://127.0.0.1:9/a.m4a"}
        ],
        "subtitles": {"fr": [{"ext": "vtt", "url": "http://127.0.0.1:9/fr.vtt"}]}
    })
    .to_string();
    let fake = dir.path().join("faux");
    let sources = auro_engine::media::tools::Sources {
        local_ytdlp: Some(tools::fake_ytdlp_answers(&fake, &[&answer])),
        local_ffmpeg: Some(tools::fake_ffmpeg(&fake, "6.0")),
        system_ffmpeg: false,
        ..Default::default()
    };
    let engine = Engine::start(Config {
        db_path: dir.path().join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options {
            media: auro_engine::media::Options { sources, dir: None },
            ..Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap();
    engine
        .set_download_dir(Some(downloads.path().to_str().unwrap().to_owned()))
        .await
        .unwrap();
    let endpoint = format!("{}-5d", endpoint(dir.path()));
    tokio::spawn({
        let (engine, endpoint) = (engine.clone(), endpoint.clone());
        async move { local::listen(engine, &endpoint).await }
    });
    for _ in 0..100 {
        if local::connect(&endpoint).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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

    // Lien magnet cliqué dans une page.
    let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=essai.iso";
    let added =
        ask(json!({"type": "add", "url": magnet, "requestId": "magnet-1", "paused": true})).await;
    assert_eq!(added["ok"], true, "{added}");
    let task = engine.get(added["taskId"].as_str().unwrap()).await.unwrap();
    assert_eq!(task.display_name, "essai.iso");

    // `.torrent` lu par l'extension.
    let mut metainfo =
        b"d4:infod6:lengthi3e4:name5:x.bin12:piece lengthi16384e6:pieces20:".to_vec();
    metainfo.extend([7u8; 20]);
    metainfo.extend_from_slice(b"ee");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&metainfo);
    let added =
        ask(json!({"type": "add", "torrent": encoded, "requestId": "t-1", "paused": true})).await;
    assert_eq!(added["ok"], true, "{added}");
    let task = engine.get(added["taskId"].as_str().unwrap()).await.unwrap();
    assert_eq!(task.display_name, "x.bin");

    for (message, code) in [
        (
            json!({"type": "add", "url": magnet, "torrent": encoded}),
            "BAD_MESSAGE",
        ),
        (
            json!({"type": "add", "url": "magnet:?xt=urn:btih:1123456789abcdef0123456789abcdef01234567",
                   "headers": {"Referer": "https://exemple.org/"}}),
            "INVALID_PARAMS",
        ),
        (json!({"type": "probe"}), "BAD_MESSAGE"),
        (
            json!({"type": "video", "pageUrl": "https://exemple.org/v", "format": 3}),
            "BAD_MESSAGE",
        ),
        (
            json!({"type": "video", "pageUrl": "https://exemple.org/v", "subtitles": "fr"}),
            "BAD_MESSAGE",
        ),
    ] {
        let reply = ask(message.clone()).await;
        assert_eq!(reply["ok"], false, "{message}");
        assert_eq!(reply["code"], code, "{message} : {reply}");
    }

    // Vidéo de la page : décrite (formats réduits), puis ajoutée.
    let probe = ask(json!({"type": "probe", "id": 5, "pageUrl": "https://exemple.org/v"})).await;
    assert_eq!(probe["ok"], true, "{probe}");
    assert_eq!(probe["id"], 5);
    assert_eq!(probe["title"], "Essai du relais");
    assert_eq!(probe["subtitles"], json!(["fr"]));
    let formats = probe["formats"].as_array().unwrap();
    assert_eq!(formats.len(), 2);
    assert_eq!(formats[0]["height"], 720);
    assert_eq!(formats[0]["filesize"], 1000);
    assert!(formats[0].get("url").is_none() && formats[0].get("protocol").is_none());
    let added = ask(json!({
        "type": "video", "pageUrl": "https://exemple.org/v", "requestId": "v-1",
        "format": "bv*[height<=720]+ba/b[height<=720]", "subtitles": ["fr"],
        "title": "Essai du relais",
    }))
    .await;
    assert_eq!(added["ok"], true, "{added}");
    let task = engine.get(added["taskId"].as_str().unwrap()).await.unwrap();
    assert_eq!(task.kind.as_sql(), "media");
    assert_eq!(task.dest_dir, downloads.path().to_str().unwrap());

    drop(browser);
    serving.await.unwrap().unwrap();
    engine.shutdown().await;
}
