//! Le binaire `auro` face à un moteur servi dans le processus du test, sur un point d'accès
//! d'essai (`--endpoint`, `--no-start` : jamais le moteur de l'utilisateur) : torrents,
//! réglages, limite d'envoi et vidéo (faux yt-dlp et ffmpeg de `testing::tools`).

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use auro_engine::net::Client;
use auro_engine::rpc::local;
use auro_engine::scheduler::{Config, DISK_RECHECK, Engine, Planning};
use auro_engine::transfer::Options;

/// Point d'accès propre à ce test.
fn endpoint(dir: &Path, name: &str) -> String {
    if cfg!(windows) {
        format!(r"\\.\pipe\auro-cli-test-{name}-{}", std::process::id())
    } else {
        dir.join(format!("{name}.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

/// Un moteur servi sur un point d'accès d'essai.
async fn serve(dir: &Path, name: &str, transfer: Options) -> (Engine, String) {
    let engine = Engine::start(Config {
        db_path: dir.join(format!("{name}.db")),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer,
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap();
    let endpoint = endpoint(dir, name);
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
    (engine, endpoint)
}

/// `auro --endpoint <essai> --no-start <args>`.
async fn auro(endpoint: &str, args: &[&str]) -> Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_auro"));
    command
        .args(["--endpoint", endpoint, "--no-start"])
        .args(args);
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Comme [`auro`], qui doit réussir ; sa sortie.
async fn ok(endpoint: &str, args: &[&str]) -> String {
    let out = auro(endpoint, args).await;
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "auro {args:?} : {stdout}\n{stderr}");
    stdout
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn torrents_settings_and_upload_limit() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, ep) = serve(dir.path(), "torrent", Options::default()).await;
    let dest = dir.path().join("dl");
    std::fs::create_dir_all(&dest).unwrap();
    let dest = dest.to_str().unwrap();

    // Un fichier .torrent local : envoyé au moteur, qui en fait une tâche torrent.
    let torrent = auro_engine::testing::swarm::make_torrent(
        "essai",
        &[("a.bin", vec![1; 40_000]), ("b.bin", vec![2; 20_000])],
        false,
    );
    let file = dir.path().join("essai.torrent");
    std::fs::write(&file, torrent).unwrap();
    let added = ok(
        &ep,
        &["add", file.to_str().unwrap(), "-o", dest, "--paused"],
    )
    .await;
    let id = added.split_whitespace().next().unwrap().to_owned();
    let task = engine.get(&id).await.unwrap();
    assert_eq!(task.kind.as_sql(), "torrent");
    let listed = ok(&ep, &["list"]).await;
    assert!(listed.contains("[torrent]"), "{listed}");
    // Fichiers inconnus tant que le torrent n'a pas été préparé.
    let files = ok(&ep, &["files", &id]).await;
    assert!(files.contains("pas encore connus"), "{files}");
    // Rangs mal écrits : refusés par la CLI.
    let bad = auro(&ep, &["files", &id, "--select", "3-1"]).await;
    assert!(!bad.status.success());

    // Réglages : lus, changés, refusés s'ils sont inconnus.
    let all = ok(&ep, &["config"]).await;
    for key in [
        "torrent.ratioLimit",
        "torrent.interface",
        "torrent.upnp",
        "media.autoUpdate",
    ] {
        assert!(all.contains(key), "{key} : {all}");
    }
    let ratio = ok(&ep, &["config", "torrent.ratioLimit", "1.5"]).await;
    assert_eq!(ratio.trim(), "torrent.ratioLimit = 1.5");
    let update = ok(&ep, &["config", "media.autoUpdate", "non"]).await;
    assert_eq!(update.trim(), "media.autoUpdate = false");
    let none = ok(&ep, &["config", "torrent.ratioLimit", "none"]).await;
    assert_eq!(none.trim(), "torrent.ratioLimit = aucun");
    let unknown = auro(&ep, &["config", "inconnu", "1"]).await;
    assert!(!unknown.status.success());
    assert!(
        text(&unknown).1.contains("réglage inconnu"),
        "{:?}",
        text(&unknown)
    );

    // Limite d'envoi des torrents.
    let upload = ok(&ep, &["limit", "--upload", "500K"]).await;
    assert!(upload.contains("d'envoi des torrents"), "{upload}");
    assert!(upload.contains("500"), "{upload}");
    let shown = ok(&ep, &["limit", "--upload"]).await;
    assert_eq!(shown, upload);

    // Vidéo sans outils : l'état le dit, l'ajout est refusé avec la commande à lancer.
    let tools = ok(&ep, &["video", "tools"]).await;
    assert!(tools.contains("absent"), "{tools}");
    let refused = auro(&ep, &["video", "add", "https://exemple.org/v", "-o", dest]).await;
    assert!(!refused.status.success());
    assert!(
        text(&refused).1.contains("auro video tools --install"),
        "{:?}",
        text(&refused)
    );
    engine.shutdown().await;
}

/// Une vidéo suivie par `auro video get` jusqu'au bout, fusion comprise ; un sous-titre
/// manquant est signalé. `auro video info` décrit la page.
#[cfg(any(windows, target_os = "linux"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_video_is_described_then_followed_through_its_merge() {
    use auro_engine::testing::{content_range, reply, serve as serve_http, tools};
    let video = content_range(0, 200 * 1024);
    let audio = content_range(5_000_000, 50 * 1024);
    let site = serve_http({
        let (video, audio) = (video.clone(), audio.clone());
        move |request| match request.path.as_str() {
            "/v.mp4" => reply("200 OK", &[("Content-Type", "video/mp4")], &video),
            "/a.m4a" => reply("200 OK", &[("Content-Type", "audio/mp4")], &audio),
            "/fr.vtt" => reply("200 OK", &[], b"WEBVTT\n"),
            _ => reply("404 Not Found", &[], b""),
        }
    });
    let v = serde_json::json!({"format_id": "v1", "ext": "mp4", "vcodec": "avc1",
        "acodec": "none", "protocol": "http", "url": site.url("/v.mp4"), "height": 360,
        "filesize": video.len()});
    let a = serde_json::json!({"format_id": "a1", "ext": "m4a", "vcodec": "none",
        "acodec": "mp4a", "protocol": "http", "url": site.url("/a.m4a")});
    let answer = serde_json::json!({
        "title": "Essai CLI", "duration": 6.0, "extractor_key": "Essai",
        "formats": [v, a], "requested_formats": [v, a],
        "subtitles": {"fr": [{"ext": "vtt", "url": site.url("/fr.vtt")}]}
    })
    .to_string();
    let dir = tempfile::tempdir().unwrap();
    let fake: PathBuf = dir.path().join("faux");
    let sources = auro_engine::media::tools::Sources {
        local_ytdlp: Some(tools::fake_ytdlp_answers(&fake, &[&answer])),
        local_ffmpeg: Some(tools::fake_ffmpeg(&fake, "6.0")),
        system_ffmpeg: false,
        ..Default::default()
    };
    let transfer = Options {
        media: auro_engine::media::Options { sources, dir: None },
        ..Options::default()
    };
    let (engine, ep) = serve(dir.path(), "video", transfer).await;
    let dest = dir.path().join("videos");
    std::fs::create_dir_all(&dest).unwrap();

    let info = ok(&ep, &["video", "info", "https://exemple.org/page"]).await;
    for wanted in [
        "Essai CLI",
        "Sous-titres : fr",
        "v1",
        "image",
        "a1",
        "son",
        "360p",
    ] {
        assert!(info.contains(wanted), "{wanted} : {info}");
    }

    let out = auro(
        &ep,
        &[
            "video",
            "get",
            "https://exemple.org/page",
            "-o",
            dest.to_str().unwrap(),
            "--subs",
            "fr,xx",
        ],
    )
    .await;
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("Terminé : "), "{stdout}");
    assert!(stdout.contains("Essai CLI.mp4"), "{stdout}");
    assert!(
        stderr.contains("Attention") && stderr.contains("xx"),
        "sous-titre manquant signalé : {stderr}"
    );
    let mut expected = video.clone();
    expected.extend(&audio);
    assert_eq!(std::fs::read(dest.join("Essai CLI.mp4")).unwrap(), expected);
    assert!(dest.join("Essai CLI.fr.vtt").exists());
    let listed = ok(&ep, &["list"]).await;
    assert!(listed.contains("[vidéo] Essai CLI"), "{listed}");
    engine.shutdown().await;
}

/// Miroirs (SPEC §7.7, phase 6) : `auro get --mirror` les donne au moteur, qui les vérifie ;
/// `auro mirror` montre les sources et en ajoute une.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mirrors_are_given_checked_and_shown() {
    use std::sync::{Arc, Mutex};

    use auro_engine::testing::{FileSite, Pace, serve_file_paced};
    const LEN: u64 = 1024 * 1024;
    let site = |shift: u64, pace: Option<Pace>| {
        serve_file_paced(
            Arc::new(Mutex::new(FileSite {
                shift,
                ..FileSite::new(LEN)
            })),
            pace,
        )
    };
    // Référence lente : les miroirs se valident pendant le téléchargement (étape 1b).
    let slow = Pace {
        chunk: 16 * 1024,
        delay: Duration::from_millis(30),
    };
    let (reference, good, other) = (site(0, Some(slow)), site(0, None), site(3, None));
    let dir = tempfile::tempdir().unwrap();
    let (engine, ep) = serve(dir.path(), "miroirs", Options::default()).await;
    let dest = dir.path().join("dl");
    std::fs::create_dir_all(&dest).unwrap();
    let dest = dest.to_str().unwrap();

    let (r, g, o) = (
        reference.url("/f.bin"),
        good.url("/f.bin"),
        other.url("/f.bin"),
    );
    ok(&ep, &["get", &r, "-o", dest, "--mirror", &g, "-m", &o]).await;
    assert_eq!(
        std::fs::read(Path::new(dest).join("f.bin")).unwrap(),
        auro_engine::testing::content(LEN as usize)
    );
    let listed = ok(&ep, &["list"]).await;
    let id = listed.split_whitespace().next().unwrap().to_owned();
    let shown = ok(&ep, &["mirror", &id]).await;
    let lines: Vec<&str> = shown.lines().collect();
    assert_eq!(lines.len(), 3, "{shown}");
    assert!(
        lines[0].contains("référence") && lines[0].contains("bonne"),
        "{shown}"
    );
    assert!(
        lines[1].contains("miroir") && lines[1].contains("bonne"),
        "{shown}"
    );
    assert!(
        lines[2].contains("écartée") && lines[2].contains("contenu différent"),
        "{shown}"
    );
    // Plusieurs adresses et des miroirs : refusé par la CLI.
    let both = auro(&ep, &["add", &r, &g, "-o", dest, "--mirror", &o]).await;
    assert!(!both.status.success());
    // Une tâche en pause, pas encore préparée : le miroir attend sa vérification.
    let added = ok(
        &ep,
        &["add", &reference.url("/g.bin"), "-o", dest, "--paused"],
    )
    .await;
    let paused = added.split_whitespace().next().unwrap().to_owned();
    ok(&ep, &["mirror", &paused, &good.url("/g.bin")]).await;
    let shown = ok(&ep, &["mirror", &paused]).await;
    assert!(
        shown
            .lines()
            .nth(1)
            .is_some_and(|l| l.contains("à vérifier")),
        "{shown}"
    );
    engine.shutdown().await;
}
