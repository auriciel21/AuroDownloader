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
    crate::testing::stop(&engine).await;
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
            "/fr.vtt" => reply("200 OK", &[("Content-Type", "text/vtt")], FR_VTT.as_bytes()),
            p if p.starts_with("/old/") => reply("403 Forbidden", &[], b""),
            _ => reply("404 Not Found", &[], b""),
        },
        pace,
    )
}

const FR_VTT: &str = "WEBVTT\n\n00:00.000 --> 00:01.000\nBonjour\n";
const DE_SRT: &str = "1\n00:00:00,000 --> 00:00:01,000\nHallo\n";

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

/// Fin d'une vidéo interrompue (porte de la phase 5) : un flux effacé à côté du fichier
/// final veut dire que la fusion est faite ; sans fichier final, ou avec tous les flux,
/// elle est (re)faite.
#[test]
fn a_merge_interrupted_after_the_rename_is_recognised() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("v.f1.mp4");
    let audio = dir.path().join("v.f2.m4a");
    let finale = dir.path().join("v.mkv");
    let inputs = [video.clone(), audio.clone()];
    std::fs::write(&video, b"v").unwrap();
    std::fs::write(&audio, b"a").unwrap();
    // Flux complets, pas encore de fichier final : à fusionner.
    assert!(!super::already_merged(&inputs, &finale));
    // Renommé, flux encore là (arrêt avant leur effacement) : refaite, sans perte.
    std::fs::write(&finale, b"va").unwrap();
    assert!(!super::already_merged(&inputs, &finale));
    // Un flux déjà effacé : faite.
    std::fs::remove_file(&video).unwrap();
    assert!(super::already_merged(&inputs, &finale));
    // Tous effacés (arrêt juste avant l'enregistrement de la fin) : faite.
    std::fs::remove_file(&audio).unwrap();
    assert!(super::already_merged(&inputs, &finale));
    // Flux perdus sans fichier final : rien n'est fait, la fusion échouera.
    std::fs::remove_file(&finale).unwrap();
    assert!(!super::already_merged(&inputs, &finale));
    // Flux seul gardé tel quel, renommé en fichier final.
    let single = [dir.path().join("w.mkv.f1")];
    std::fs::write(dir.path().join("w.mkv"), b"w").unwrap();
    assert!(super::already_merged(&single, &dir.path().join("w.mkv")));
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
    let mut announced = s.engine.subscribe();
    let id = s
        .engine
        .add(add_video(&s, "https://exemple.org/page"))
        .await
        .unwrap();
    let states = states_until_end(&mut events, &id).await;
    // La préparation annoncée porte le titre de la vidéo, pas le nom de son fichier.
    let mut names = Vec::new();
    while let Ok(event) = announced.try_recv() {
        if let Event::Prepared {
            task_id,
            display_name,
            file_name,
            ..
        } = event
            && task_id == id
        {
            names.push((display_name, file_name));
        }
    }
    assert!(!names.is_empty(), "préparation annoncée");
    for (display, file) in &names {
        assert_eq!(display, "Essai : vidéo");
        assert!(
            file.as_deref().is_some_and(|f| f.ends_with(".mp4")),
            "{file:?}"
        );
    }
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
    // Ni adresse à changer (celle de la page est relue à chaque exécution), ni déplacement
    // (les sous-titres ne suivraient pas), ni empreinte.
    use crate::scheduler::{EngineError, TaskUpdate};
    let refused = |e: EngineError| matches!(e, EngineError::BadRequest(_));
    assert!(refused(
        s.engine
            .set_url(&id, "https://exemple.org/autre")
            .await
            .unwrap_err()
    ));
    assert!(refused(
        s.engine
            .relocate(&id, None, Some("autre.mp4".into()))
            .await
            .unwrap_err()
    ));
    let checksum = crate::io::checksum::Checksum::parse("sha256", &"a".repeat(64)).unwrap();
    assert!(refused(
        s.engine
            .update_task(
                &id,
                TaskUpdate {
                    checksum: Some(Some(checksum)),
                    ..TaskUpdate::default()
                }
            )
            .await
            .unwrap_err()
    ));
    crate::testing::stop(&s.engine).await;
}

/// Règles de rangement (SPEC §7.21) : une vidéo ajoutée sans dossier va dans celui de la
/// règle (domaine de la page), sous le nom du modèle ; ses flux la suivent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rule_places_and_renames_a_video() {
    let server = site(None);
    let answer = resolution(&server, "", false);
    let s = setup(&[&answer], true);
    s.engine
        .set_download_dir(Some(s.dest.to_string_lossy().into_owned()))
        .await
        .unwrap();
    let rule = crate::rules::Rule::from_json(&serde_json::json!({
        "match": {"domains": ["exemple.org"], "categories": ["video"]},
        "actions": {"folder": "Vidéos/Exemple", "rename": "{hôte} - {nom}.{ext}"}
    }))
    .unwrap();
    s.engine.set_rules(vec![rule]).await.unwrap();
    let mut events = s.engine.subscribe();
    let mut request = add_video(&s, "https://www.exemple.org/page");
    request.dest_dir = None;
    let id = s.engine.add(request).await.unwrap();
    assert_eq!(wait_end(&mut events, &id).await, TaskState::Completed);

    let task = s.engine.get(&id).await.unwrap();
    let placed = s.dest.join("Vidéos").join("Exemple");
    assert_eq!(Path::new(&task.dest_dir), placed);
    let name = task.file_name.clone().unwrap();
    assert!(name.starts_with("www.exemple.org - Essai"), "{name}");
    assert!(name.ends_with(".mp4"), "{name}");
    let mut expected = video_bytes();
    expected.extend(audio_bytes());
    assert_eq!(std::fs::read(placed.join(&name)).unwrap(), expected);
    // Rien d'autre que la vidéo dans le dossier de la règle : les flux sont partis.
    let left: Vec<_> = std::fs::read_dir(&placed)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left.len(), 1, "{left:?}");
    crate::testing::stop(&s.engine).await;
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
    crate::testing::stop(&s.engine).await;
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
    crate::testing::stop(&s.engine).await;
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
    crate::testing::stop(&s.engine).await;

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
    crate::testing::stop(&s.engine).await;
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
    crate::testing::stop(&s.engine).await;
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
    crate::testing::stop(&s.engine).await;
}

/// Audit V4 : un fichier déjà présent sous le nom d'un flux (laissé par un yt-dlp lancé à
/// la main, par exemple) n'est ni pris pour le flux fait, ni fusionné, ni effacé : la
/// vidéo prend le nom suivant.
#[tokio::test]
async fn a_file_already_named_like_a_stream_is_left_alone() {
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
    let other = s.dest.join("Fragments.fhls-1.mp4");
    std::fs::write(&other, b"un autre fichier").unwrap();
    let mut events = s.engine.subscribe();
    let id = s
        .engine
        .add(add_video(&s, "https://exemple.org/page"))
        .await
        .unwrap();
    let end = wait_end(&mut events, &id).await;
    let task = s.engine.get(&id).await.unwrap();
    assert_eq!(end, TaskState::Completed, "{task:?}");
    assert_eq!(task.file_name.as_deref(), Some("Fragments (1).mp4"));
    let mut expected = fragment.clone();
    expected.extend(audio_bytes());
    assert_eq!(
        std::fs::read(s.dest.join("Fragments (1).mp4")).unwrap(),
        expected
    );
    assert_eq!(std::fs::read(&other).unwrap(), b"un autre fichier");
    assert_eq!(files_in(&s.dest).len(), 2, "{:?}", files_in(&s.dest));
    crate::testing::stop(&s.engine).await;
}

/// Attend l'avertissement `code` du moteur ; rend son message et sa tâche.
async fn wait_warning(
    events: &mut broadcast::Receiver<Event>,
    code: &str,
) -> (String, Option<String>) {
    timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(Event::Warning {
                    code: c,
                    message,
                    task_id,
                    ..
                }) if c == code => return (message, task_id),
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("moteur arrêté"),
            }
        }
    })
    .await
    .expect("pas d'avertissement")
}

/// Sous-titres (F-24) : gardés à côté de la vidéo sous `<nom>.<langue>.<ext>`, au format du
/// site ; ceux qui manquent (refusé par le site, seulement en HLS, absent) ne bloquent pas
/// la vidéo et sont signalés. Un retéléchargement garde ceux de la vidéo précédente ; un
/// retrait avec les fichiers efface ceux de la vidéo retirée.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subtitles_are_kept_beside_the_video_and_missing_ones_reported() {
    let server = site(None);
    let mut answer: serde_json::Value =
        serde_json::from_str(&resolution(&server, "", false)).unwrap();
    answer["subtitles"] = serde_json::json!({
        "fr": [{"ext": "json3", "url": server.url("/fr.json3")},
               {"ext": "vtt", "url": server.url("/fr.vtt")}],
        "en": [{"ext": "vtt", "url": server.url("/en.vtt")}],
        "de": [{"ext": "srt", "data": DE_SRT}],
        "es": [{"ext": "vtt", "url": server.url("/es.m3u8"), "protocol": "m3u8_native"}]
    });
    let answer = answer.to_string();
    let s = setup(&[&answer], true);
    let mut events = s.engine.subscribe();
    let mut add = add_video(&s, "https://exemple.org/page");
    add.media.as_mut().unwrap().subtitles =
        ["fr", "en", "de", "es", "it"].map(String::from).to_vec();
    let id = s.engine.add(add).await.unwrap();
    let end = wait_end(&mut events, &id).await;
    let task = s.engine.get(&id).await.unwrap();
    assert_eq!(end, TaskState::Completed, "{task:?}");
    let (message, warned) = wait_warning(&mut events, "SUBTITLES_MISSING").await;
    assert_eq!(warned.as_deref(), Some(id.as_str()));
    assert!(message.ends_with(": en, es, it"), "{message}");

    let name = task.file_name.clone().unwrap();
    let stem = name.strip_suffix(".mp4").unwrap().to_owned();
    let mut expected = video_bytes();
    expected.extend(audio_bytes());
    assert_eq!(
        std::fs::read(s.dest.join(&name)).unwrap(),
        expected,
        "la fusion ne prend pas les sous-titres"
    );
    let first = [
        format!("{stem}.de.srt"),
        format!("{stem}.fr.vtt"),
        name.clone(),
    ];
    assert_eq!(files_in(&s.dest), first);
    assert_eq!(
        std::fs::read_to_string(s.dest.join(&first[1])).unwrap(),
        FR_VTT
    );
    assert_eq!(
        std::fs::read_to_string(s.dest.join(&first[0])).unwrap(),
        DE_SRT
    );
    // Le site a refusé `en` : une nouvelle résolution, au plus trois fois, comme un flux.
    let refused = server
        .requests()
        .iter()
        .filter(|r| r.path == "/en.vtt")
        .count();
    assert!((1..=8).contains(&refused), "{refused} demandes de en.vtt");
    assert!(
        !server.requests().iter().any(|r| r.path.contains(".json3")
            || r.path.contains(".m3u8")
            || r.path.contains("SUMS")
            || r.path.contains(".sha")),
        "ni format écarté, ni empreinte voisine : {:?}",
        server
            .requests()
            .iter()
            .map(|r| r.path.clone())
            .collect::<Vec<_>>()
    );

    // Retéléchargement sans effacer : les fichiers de la première fois restent.
    s.engine.restart(&id, false).await.unwrap();
    assert_eq!(wait_end(&mut events, &id).await, TaskState::Completed);
    let again = s.engine.get(&id).await.unwrap().file_name.unwrap();
    assert_ne!(again, name);
    assert_eq!(files_in(&s.dest).len(), 6, "{:?}", files_in(&s.dest));
    // Retrait avec les fichiers : ceux de la seconde vidéo partent, sous-titres compris.
    s.engine.remove(&id, true).await.unwrap();
    assert_eq!(files_in(&s.dest), first);
    crate::testing::stop(&s.engine).await;
}

/// Audit V5 : un sous-titre en échec définitif pour une autre cause qu'un refus du site
/// (ici, `TLS_INVALID_CERT` : un serveur dont l'autorité n'est pas reconnue) ne met plus en
/// échec une vidéo aux flux complets ; elle se termine, le sous-titre signalé manquant.
#[tokio::test]
async fn a_subtitle_failing_for_any_reason_is_reported_missing() {
    let server = site(None);
    let tls = crate::testing::TestTls::new();
    let untrusted = crate::testing::serve_file_tls(
        std::sync::Arc::new(std::sync::Mutex::new(crate::testing::FileSite::new(100))),
        None,
        &tls,
    );
    let mut answer: serde_json::Value =
        serde_json::from_str(&resolution(&server, "", false)).unwrap();
    answer["subtitles"] =
        serde_json::json!({"en": [{"ext": "vtt", "url": untrusted.url("/en.vtt")}]});
    let answer = answer.to_string();
    let s = setup(&[&answer], true);
    let mut events = s.engine.subscribe();
    let mut add = add_video(&s, "https://exemple.org/page");
    add.media.as_mut().unwrap().subtitles = vec!["en".into()];
    let id = s.engine.add(add).await.unwrap();
    let end = wait_end(&mut events, &id).await;
    let task = s.engine.get(&id).await.unwrap();
    assert_eq!(end, TaskState::Completed, "{task:?}");
    let (message, _) = wait_warning(&mut events, "SUBTITLES_MISSING").await;
    assert!(message.ends_with(": en"), "{message}");
    crate::testing::stop(&s.engine).await;
}

/// Essai réel (Internet) : vidéos sous licence libre téléchargées jusqu'au bout par le
/// moteur, avec les vrais yt-dlp et ffmpeg de `AURO_TOOLS_DIR` (installés s'ils manquent).
/// Formats légers choisis (une centaine de Mio en tout) : flux direct seul (PeerTube de
/// Blender, Internet Archive), flux direct et sous-titres WebVTT (Commons, une langue
/// absente), HLS par yt-dlp et son seul direct fusionnés, avec un sous-titre (Framatube).
/// `AURO_TOOLS_DIR=<dossier> cargo test -p auro-engine --lib -- --ignored real_free --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Internet : vraies pages de vidéos libres, vrais yt-dlp et ffmpeg (AURO_TOOLS_DIR)"]
async fn real_free_videos_are_downloaded_to_the_end() {
    use crate::media::extract::tests::{FRAMATUBE_PAGE, FREE_PAGES};
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
    let ffprobe = tools.status().ffprobe.clone().unwrap();

    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(Config {
        db_path: dir.path().join("auro.db"),
        client: Client::new().unwrap(),
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
    // Page, format, sous-titres demandés, sous-titres attendus, flux attendus.
    type Case<'a> = (&'a str, &'a str, &'a [&'a str], &'a [&'a str], &'a str);
    let cases: [Case; 4] = [
        // Tutoriel muet : image seule.
        (FREE_PAGES[0], "bv*+ba/b", &[], &[], "video"),
        (
            FREE_PAGES[1],
            "0",
            &["fr", "en", "xx"],
            &["en", "fr"],
            "video audio",
        ),
        (FREE_PAGES[2], "1", &[], &[], "video audio"),
        (
            FRAMATUBE_PAGE,
            "3+Audio_only-0",
            &["en"],
            &["en"],
            "video audio",
        ),
    ];
    let mut expected_files = 0;
    for (page, format, asked, kept, kinds_wanted) in cases {
        let started = std::time::Instant::now();
        let mut events = engine.subscribe();
        let id = engine
            .add(AddTask {
                dest_dir: Some(dest.to_string_lossy().into_owned()),
                media: Some(MediaRequest {
                    page_url: page.into(),
                    format: Some(format.into()),
                    subtitles: asked.iter().map(|s| s.to_string()).collect(),
                    ..MediaRequest::default()
                }),
                ..AddTask::default()
            })
            .await
            .unwrap();
        let end = wait_end(&mut events, &id).await;
        let task = engine.get(&id).await.unwrap();
        assert_eq!(end, TaskState::Completed, "{page} : {task:?}");
        let name = task.file_name.clone().unwrap();
        let path = dest.join(&name);
        let out = std::process::Command::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type:format=duration",
                "-of",
                "csv=p=0",
            ])
            .arg(&path)
            .output()
            .unwrap();
        let described = String::from_utf8_lossy(&out.stdout).replace(['\r', '\n'], " ");
        for kind in kinds_wanted.split(' ') {
            assert!(described.contains(kind), "{page} : {described}");
        }
        let stem = Path::new(&name).file_stem().unwrap().to_string_lossy();
        for lang in kept {
            let sub = dest.join(format!("{stem}.{lang}.vtt"));
            let text = std::fs::read_to_string(&sub)
                .unwrap_or_else(|e| panic!("{page} : {} : {e}", sub.display()));
            assert!(text.starts_with("WEBVTT") || text.starts_with("\u{feff}WEBVTT"));
        }
        if asked.len() > kept.len() {
            let (message, _) = wait_warning(&mut events, "SUBTITLES_MISSING").await;
            assert!(message.ends_with(": xx"), "{message}");
        }
        expected_files += 1 + kept.len();
        eprintln!(
            "{page} : « {} », {} ({} octets) en {:.1?}, flux et durée : {described}, \
             sous-titres {kept:?}",
            task.display_name,
            name,
            std::fs::metadata(&path).unwrap().len(),
            started.elapsed()
        );
    }
    // Rien d'autre que les vidéos et leurs sous-titres : ni flux, ni `.part`.
    let left = files_in(&dest);
    eprintln!("dossier : {left:?}");
    assert_eq!(left.len(), expected_files, "{left:?}");
    crate::testing::stop(&engine).await;
}
