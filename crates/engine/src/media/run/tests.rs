//! Tests de l'exécution d'une vidéo.
//!
//! L'essai avec les vrais outils (ignoré par défaut) fabrique une vidéo de test avec ffmpeg,
//! la sert en local sous plusieurs formes (fichier seul, HLS à flux séparés, DASH en
//! fichiers uniques) et la fait télécharger au moteur par yt-dlp : sans Internet, une fois
//! les outils installés.

use std::path::{Path, PathBuf};

use std::time::Duration;

use tokio::sync::broadcast;
use tokio::time::timeout;

use crate::net::Client;
use crate::scheduler::{AddTask, Config, DISK_RECHECK, Engine, Event, MediaRequest, Planning};
use crate::store::tasks::TaskState;
use crate::testing::{reply, serve};

const WAIT: Duration = Duration::from_secs(120);

/// Attend que `task_id` passe à `to` ; rend l'état atteint si c'est un échec.
async fn wait_end(events: &mut broadcast::Receiver<Event>, task_id: &str) -> TaskState {
    timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(Event::StateChanged { task_id: t, to, .. })
                    if t == task_id
                        && matches!(
                            to,
                            TaskState::Completed | TaskState::Failed | TaskState::Paused
                        ) =>
                {
                    return to;
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("moteur arrêté"),
            }
        }
    })
    .await
    .expect("la vidéo ne finit pas")
}

/// Les états annoncés pour `task_id` jusqu'à sa fin.
async fn states_until_end(
    events: &mut broadcast::Receiver<Event>,
    task_id: &str,
) -> Vec<TaskState> {
    let mut seen = Vec::new();
    timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(Event::StateChanged { task_id: t, to, .. }) if t == task_id => {
                    seen.push(to);
                    if matches!(
                        to,
                        TaskState::Completed | TaskState::Failed | TaskState::Paused
                    ) {
                        return;
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("moteur arrêté"),
            }
        }
    })
    .await
    .expect("la vidéo ne finit pas");
    seen
}

/// Sert les fichiers de `dir` (sans plages), avec le bon type pour les listes.
fn serve_dir(dir: PathBuf) -> crate::testing::Server {
    serve(move |request| {
        let name = request.path.trim_start_matches('/');
        match std::fs::read(dir.join(name)) {
            Ok(body) => {
                let kind = match Path::new(name).extension().and_then(|e| e.to_str()) {
                    Some("m3u8") => "application/vnd.apple.mpegurl",
                    Some("mpd") => "application/dash+xml",
                    Some("mp4") => "video/mp4",
                    Some("m4a") => "audio/mp4",
                    Some("html") => "text/html",
                    _ => "application/octet-stream",
                };
                reply("200 OK", &[("Content-Type", kind)], &body)
            }
            Err(_) => reply("404 Not Found", &[], b""),
        }
    })
}

/// ffmpeg réel : lance-le dans `dir`.
fn ffmpeg(tool: &Path, dir: &Path, args: &[&str]) {
    let out = std::process::Command::new(tool)
        .current_dir(dir)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Essai réel, sans Internet : les vrais yt-dlp et ffmpeg de `AURO_TOOLS_DIR` (installés
/// s'ils manquent) ; vidéo de test fabriquée et servie en local.
/// `AURO_TOOLS_DIR=<dossier> cargo test -p auro-engine --lib -- --ignored real_local --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "vrais yt-dlp et ffmpeg (AURO_TOOLS_DIR)"]
async fn real_local_videos_are_downloaded_and_merged() {
    let tools_dir = PathBuf::from(std::env::var_os("AURO_TOOLS_DIR").expect("AURO_TOOLS_DIR"));
    let sources = crate::media::tools::Sources {
        system_ffmpeg: false,
        ..Default::default()
    };
    let tools = crate::media::tools::Tools::open(
        tools_dir.clone(),
        sources.clone(),
        Client::new().unwrap(),
    );
    if !tools.status().ready() {
        assert!(tools.begin_install());
        tools.install(false).await.unwrap();
    }
    let status = tools.status();
    let real_ffmpeg = status.ffmpeg.clone().unwrap().path;
    let real_ffprobe = status.ffprobe.clone().unwrap();

    // Une vidéo de 6 s : mire et son.
    let site = tempfile::tempdir().unwrap();
    let s = site.path();
    let src = [
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440",
        "-t",
        "6",
    ];
    let mut film = src.to_vec();
    film.extend([
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "film.mp4",
    ]);
    ffmpeg(&real_ffmpeg, s, &film);
    // HLS : image et son en flux séparés, liste maîtresse.
    let mut hls = src.to_vec();
    hls.extend([
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-f",
        "hls",
        "-hls_time",
        "2",
        "-hls_playlist_type",
        "vod",
        "-master_pl_name",
        "master.m3u8",
        "-var_stream_map",
        "v:0,agroup:son a:0,agroup:son",
        "-hls_segment_filename",
        "hls_%v_%03d.ts",
        "hls_%v.m3u8",
    ]);
    ffmpeg(&real_ffmpeg, s, &hls);
    // DASH à fichiers directs (un par flux, `BaseURL` seule), comme sur bien des sites :
    // image seule et son seul, téléchargés par le moteur, puis fusionnés.
    let mut video = src.to_vec();
    video.extend([
        "-an",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-movflags",
        "+faststart",
        "video.mp4",
    ]);
    ffmpeg(&real_ffmpeg, s, &video);
    let mut audio = src.to_vec();
    audio.extend(["-vn", "-c:a", "aac", "-movflags", "+faststart", "audio.m4a"]);
    ffmpeg(&real_ffmpeg, s, &audio);
    std::fs::write(
        s.join("manifest.mpd"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT6S"
     minBufferTime="PT2S" profiles="urn:mpeg:dash:profile:isoff-on-demand:2011">
  <Period>
    <AdaptationSet mimeType="video/mp4" contentType="video">
      <Representation id="v1" bandwidth="300000" width="320" height="240" codecs="avc1.64000d">
        <BaseURL>video.mp4</BaseURL>
      </Representation>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" contentType="audio" lang="und">
      <Representation id="a1" bandwidth="128000" codecs="mp4a.40.2" audioSamplingRate="44100">
        <BaseURL>audio.m4a</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>
"#,
    )
    .unwrap();
    let server = serve_dir(s.to_path_buf());

    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(Config {
        db_path: dir.path().join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(10)).unwrap(),
        transfer: crate::transfer::Options {
            media: crate::media::Options {
                sources,
                dir: Some(tools_dir),
            },
            ..crate::transfer::Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap();
    let dest = dir.path().join("videos");
    std::fs::create_dir_all(&dest).unwrap();
    for (page, title) in [
        ("/film.mp4", "fichier seul"),
        ("/master.m3u8", "HLS, flux séparés"),
        ("/manifest.mpd", "DASH, fichiers directs"),
    ] {
        let probe = engine.probe_media(&server.url(page)).await.unwrap();
        let protocols: Vec<String> = probe
            .formats
            .iter()
            .map(|f| {
                format!(
                    "{} {} v={:?} a={:?}",
                    f.format_id, f.protocol, f.vcodec, f.acodec
                )
            })
            .collect();
        eprintln!("{title} : {protocols:?}");
        let mut events = engine.subscribe();
        let id = engine
            .add(AddTask {
                dest_dir: Some(dest.to_string_lossy().into_owned()),
                media: Some(MediaRequest {
                    page_url: server.url(page),
                    title: Some(title.into()),
                    ..MediaRequest::default()
                }),
                ..AddTask::default()
            })
            .await
            .unwrap();
        let end = wait_end(&mut events, &id).await;
        let task = engine.get(&id).await.unwrap();
        assert_eq!(end, TaskState::Completed, "{title} : {task:?}");
        let path = dest.join(task.file_name.as_deref().unwrap());
        let out = std::process::Command::new(&real_ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type",
                "-of",
                "csv=p=0",
            ])
            .arg(&path)
            .output()
            .unwrap();
        let kinds = String::from_utf8_lossy(&out.stdout).replace(['\r', '\n'], " ");
        eprintln!(
            "{title} : {} ({} octets) : {kinds}",
            path.display(),
            task.total_bytes.unwrap_or(0)
        );
        assert!(
            kinds.contains("video") && kinds.contains("audio"),
            "{title} : {kinds}"
        );
    }
    // Rien d'autre que les vidéos finales dans le dossier : ni flux, ni `.part`.
    let mut left: Vec<String> = std::fs::read_dir(&dest)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    eprintln!("dossier : {left:?}");
    assert_eq!(left.len(), 3, "{left:?}");
    // Les tâches internes ne se voient pas ; l'historique n'a que les vidéos.
    assert_eq!(engine.list(Default::default()).await.unwrap().0.len(), 3);
    engine.shutdown().await;
}

// Faux outils (`testing::tools`) : yt-dlp rend une résolution, ffmpeg met ses entrées bout
// à bout, ffprobe rend une durée. Sans Internet ni vrais outils.

/// Contenu servi : image, puis son.
fn video_bytes() -> Vec<u8> {
    crate::testing::content_range(0, 300 * 1024)
}

fn audio_bytes() -> Vec<u8> {
    crate::testing::content_range(7_000_000, 70 * 1024)
}

/// Un site : `/v.mp4`, `/a.m4a`, et `/old/…` refusé (adresse expirée) ; bridé si `pace`.
fn site(pace: Option<crate::testing::Pace>) -> crate::testing::Server {
    crate::testing::serve_paced(
        |request| match request.path.as_str() {
            "/v.mp4" => reply("200 OK", &[("Content-Type", "video/mp4")], &video_bytes()),
            "/a.m4a" => reply("200 OK", &[("Content-Type", "audio/mp4")], &audio_bytes()),
            p if p.starts_with("/old/") => reply("403 Forbidden", &[], b""),
            _ => reply("404 Not Found", &[], b""),
        },
        pace,
    )
}

/// Réponse de `yt-dlp -J -f …` : image et son directs, sous `prefix`.
fn resolution(server: &crate::testing::Server, prefix: &str, drm: bool) -> String {
    serde_json::json!({
        "title": "Essai : vidéo", "duration": 6.0,
        "requested_formats": [
            {"format_id": "v1", "ext": "mp4", "vcodec": "avc1", "acodec": "none",
             "protocol": "http", "url": server.url(&format!("{prefix}/v.mp4")),
             "has_drm": drm,
             "http_headers": {"User-Agent": "essai-ua", "Accept-Encoding": "gzip"}},
            {"format_id": "a1", "ext": "m4a", "vcodec": "none", "acodec": "mp4a",
             "protocol": "http", "url": server.url(&format!("{prefix}/a.m4a"))}
        ]
    })
    .to_string()
}

struct Setup {
    engine: Engine,
    dest: PathBuf,
    _dir: tempfile::TempDir,
}

fn setup(answers: &[&str], tools: bool) -> Setup {
    setup_with(|fake| {
        if tools {
            crate::media::tools::Sources {
                local_ytdlp: Some(crate::testing::tools::fake_ytdlp_answers(fake, answers)),
                local_ffmpeg: Some(crate::testing::tools::fake_ffmpeg(fake, "6.0")),
                system_ffmpeg: false,
                ..Default::default()
            }
        } else {
            crate::media::tools::Sources {
                system_ffmpeg: false,
                ..Default::default()
            }
        }
    })
}

/// Un moteur dont les outils sont ceux que `tools` prépare dans un dossier de faux outils.
fn setup_with(tools: impl FnOnce(&Path) -> crate::media::tools::Sources) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("faux");
    let sources = tools(&fake);
    let engine = Engine::start(Config {
        db_path: dir.path().join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: crate::transfer::Options {
            media: crate::media::Options { sources, dir: None },
            connections: Some(2),
            min_segment: 16 * 1024,
            ..crate::transfer::Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap();
    let dest = dir.path().join("videos");
    std::fs::create_dir_all(&dest).unwrap();
    Setup {
        engine,
        dest,
        _dir: dir,
    }
}

fn add_video(setup: &Setup, page: &str) -> AddTask {
    AddTask {
        dest_dir: Some(setup.dest.to_string_lossy().into_owned()),
        media: Some(MediaRequest {
            page_url: page.into(),
            ..MediaRequest::default()
        }),
        ..AddTask::default()
    }
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Attend un avancement de `task_id` au-delà de `bytes`.
async fn wait_received(events: &mut broadcast::Receiver<Event>, task_id: &str, bytes: u64) {
    timeout(WAIT, async {
        loop {
            if let Ok(Event::Progress {
                task_id: t,
                progress,
                ..
            }) = events.recv().await
                && t == task_id
                && progress.received > bytes
            {
                return;
            }
        }
    })
    .await
    .expect("aucun avancement");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_direct_streams_become_one_video() {
    let server = site(None);
    let answer = resolution(&server, "", false);
    let s = setup(&[&answer], true);
    let mut events = s.engine.subscribe();
    let id = s
        .engine
        .add(add_video(&s, "https://exemple.org/page"))
        .await
        .unwrap();
    let states = states_until_end(&mut events, &id).await;
    assert_eq!(
        states,
        [
            TaskState::Active,
            TaskState::PostProcessing,
            TaskState::Completed
        ],
        "fusion annoncée"
    );

    let task = s.engine.get(&id).await.unwrap();
    assert_eq!(task.display_name, "Essai : vidéo");
    let name = task.file_name.clone().unwrap();
    assert!(name.ends_with(".mp4"), "{name}");
    let mut expected = video_bytes();
    expected.extend(audio_bytes());
    assert_eq!(
        std::fs::read(s.dest.join(&name)).unwrap(),
        expected,
        "flux bout à bout"
    );
    assert_eq!(
        files_in(&s.dest),
        std::slice::from_ref(&name),
        "ni flux ni .part"
    );
    // Les en-têtes de yt-dlp suivent le flux, sauf ceux que le moteur fixe lui-même.
    let asked: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.path == "/v.mp4")
        .collect();
    assert!(!asked.is_empty());
    assert!(
        asked
            .iter()
            .all(|r| r.header("user-agent") == Some("essai-ua"))
    );
    assert!(
        asked
            .iter()
            .all(|r| r.header("accept-encoding") != Some("gzip"))
    );
    // Une seule tâche visible, une seule ligne d'historique.
    let (listed, _) = s.engine.list(Default::default()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(s.engine.history(Default::default()).await.unwrap().len(), 1);
    s.engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paused_video_resumes_its_streams_without_loss() {
    let server = site(Some(crate::testing::Pace {
        chunk: 8 * 1024,
        delay: Duration::from_millis(20),
    }));
    let answer = resolution(&server, "", false);
    let s = setup(&[&answer], true);
    let mut events = s.engine.subscribe();
    let id = s
        .engine
        .add(add_video(&s, "https://exemple.org/page"))
        .await
        .unwrap();
    // En plein premier flux.
    wait_received(&mut events, &id, 64 * 1024).await;
    s.engine.pause(&id).await.unwrap();
    assert_eq!(wait_end(&mut events, &id).await, TaskState::Paused);
    s.engine.resume(&id).await.unwrap();
    assert_eq!(wait_end(&mut events, &id).await, TaskState::Completed);
    let task = s.engine.get(&id).await.unwrap();
    let mut expected = video_bytes();
    expected.extend(audio_bytes());
    assert_eq!(
        std::fs::read(s.dest.join(task.file_name.unwrap())).unwrap(),
        expected
    );
    s.engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_stream_address_is_resolved_again() {
    let server = site(None);
    let first = resolution(&server, "/old", false);
    let second = resolution(&server, "", false);
    let s = setup(&[&first, &second], true);
    let mut events = s.engine.subscribe();
    let id = s
        .engine
        .add(add_video(&s, "https://exemple.org/page"))
        .await
        .unwrap();
    assert_eq!(wait_end(&mut events, &id).await, TaskState::Completed);
    assert!(server.requests().iter().any(|r| r.path == "/old/v.mp4"));
    let task = s.engine.get(&id).await.unwrap();
    assert_eq!(
        std::fs::metadata(s.dest.join(task.file_name.unwrap()))
            .unwrap()
            .len(),
        (video_bytes().len() + audio_bytes().len()) as u64
    );
    s.engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_are_needed_and_drm_is_refused() {
    // Sans outils, l'ajout est refusé.
    let s = setup(&[], false);
    let err = s
        .engine
        .add(add_video(&s, "https://exemple.org/page"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, crate::scheduler::EngineError::ToolsMissing(_)),
        "{err}"
    );
    s.engine.shutdown().await;

    // Un flux protégé : échec, rien de téléchargé.
    let server = site(None);
    let answer = resolution(&server, "", true);
    let s = setup(&[&answer], true);
    let mut events = s.engine.subscribe();
    let id = s
        .engine
        .add(add_video(&s, "https://exemple.org/page"))
        .await
        .unwrap();
    assert_eq!(wait_end(&mut events, &id).await, TaskState::Failed);
    let task = s.engine.get(&id).await.unwrap();
    assert_eq!(task.error_code.as_deref(), Some("EXTRACTOR_FAILED"));
    assert!(server.requests().is_empty());
    s.engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_video_deletes_its_streams() {
    let server = site(Some(crate::testing::Pace {
        chunk: 8 * 1024,
        delay: Duration::from_millis(20),
    }));
    let answer = resolution(&server, "", false);
    let s = setup(&[&answer], true);
    let mut events = s.engine.subscribe();
    let id = s
        .engine
        .add(add_video(&s, "https://exemple.org/page"))
        .await
        .unwrap();
    wait_received(&mut events, &id, 32 * 1024).await;
    s.engine.cancel(&id, true).await.unwrap();
    // Le temps que l'exécution s'arrête et que les fichiers partent.
    timeout(WAIT, async {
        while !files_in(&s.dest).is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("restent : {:?}", files_in(&s.dest)));
    s.engine.shutdown().await;
}

/// Un flux fragmenté passe par yt-dlp (`-o` vers le fichier du flux), avec la plus petite
/// limite de débit (`--limit-rate`) ; puis fusion avec le flux direct.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fragmented_stream_goes_through_ytdlp_within_the_limit() {
    let server = site(None);
    let fragment = crate::testing::content_range(3_000_000, 100 * 1024);
    let answer = serde_json::json!({
        "title": "Fragments", "duration": 6.0,
        "requested_formats": [
            {"format_id": "hls-1", "ext": "mp4", "vcodec": "avc1", "acodec": "none",
             "protocol": "m3u8_native", "url": server.url("/v.m3u8")},
            {"format_id": "a1", "ext": "m4a", "vcodec": "none", "acodec": "mp4a",
             "protocol": "http", "url": server.url("/a.m4a")}
        ]
    })
    .to_string();
    let s = setup_with(|fake| crate::media::tools::Sources {
        local_ytdlp: Some(crate::testing::tools::fake_ytdlp_media(
            fake, &answer, &fragment, 0,
        )),
        local_ffmpeg: Some(crate::testing::tools::fake_ffmpeg(fake, "6.0")),
        system_ffmpeg: false,
        ..Default::default()
    });
    s.engine.set_download_limit(Some(2_000_000)).await.unwrap();
    let mut events = s.engine.subscribe();
    let mut add = add_video(&s, "https://exemple.org/page");
    add.speed_limit_bps = Some(1_500_000);
    let id = s.engine.add(add).await.unwrap();
    let end = wait_end(&mut events, &id).await;
    let task = s.engine.get(&id).await.unwrap();
    assert_eq!(end, TaskState::Completed, "{task:?}");
    let mut expected = fragment.clone();
    expected.extend(audio_bytes());
    assert_eq!(
        std::fs::read(s.dest.join(task.file_name.as_deref().unwrap())).unwrap(),
        expected
    );
    let downloads =
        std::fs::read_to_string(s._dir.path().join("faux").join("downloads.txt")).unwrap();
    assert!(downloads.contains("--limit-rate"), "{downloads}");
    assert!(
        downloads.contains("1500000"),
        "la plus petite limite : {downloads}"
    );
    assert!(downloads.contains("hls-1"), "{downloads}");
    assert_eq!(files_in(&s.dest).len(), 1, "{:?}", files_in(&s.dest));
    s.engine.shutdown().await;
}
