//! Tests de l'extraction : la réponse de yt-dlp lue, ses erreurs nettoyées, et un faux
//! yt-dlp (`testing::tools::fake_ytdlp`) pour le lancement.

use serde_json::json;

use super::*;
use crate::testing::tools::fake_ytdlp;

/// Réponse de `yt-dlp -J` réduite à ce que le moteur lit, sur le modèle d'une vraie page
/// (vidéo et audio séparés, HLS, DRM, sous-titres).
fn sample() -> Value {
    json!({
        "_type": "video",
        "id": "abc",
        "title": "Un film libre",
        "duration": 596.5,
        "extractor": "peertube",
        "extractor_key": "PeerTube",
        "thumbnail": "https://exemple.org/vignette.jpg",
        "subtitles": {
            "fr": [{"ext": "vtt", "url": "https://exemple.org/fr.vtt"}],
            "en": [{"ext": "vtt", "url": "https://exemple.org/en.vtt"}],
            // Seulement en HLS : le moteur ne sait pas le télécharger, il n'est pas proposé.
            "es": [{"ext": "vtt", "url": "https://exemple.org/es.m3u8",
                    "protocol": "m3u8_native"}]
        },
        "automatic_captions": {"de": [{"ext": "vtt", "url": "https://exemple.org/de.vtt"}]},
        "formats": [
            {"format_id": "a1", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2",
             "tbr": 128.0, "filesize": 9_000_000, "protocol": "https",
             "url": "https://exemple.org/a1"},
            {"format_id": "v720", "ext": "mp4", "vcodec": "avc1.64001f", "acodec": "none",
             "width": 1280, "height": 720, "fps": 25.0, "tbr": 2500.0,
             "filesize_approx": 180_000_000.0, "protocol": "https", "format_note": "720p"},
            {"format_id": "hls-1080", "ext": "mp4", "vcodec": "avc1", "acodec": "mp4a.40.2",
             "width": 1920, "height": 1080, "protocol": "m3u8_native"},
            {"format_id": "drm", "ext": "mp4", "vcodec": "avc1", "acodec": "mp4a",
             "protocol": "https", "has_drm": "maybe"},
            {"ext": "mp4", "protocol": "https"}
        ]
    })
}

#[test]
fn a_ytdlp_answer_is_read() {
    let probe = parse_probe(&sample()).unwrap();
    assert_eq!(probe.title, "Un film libre");
    assert_eq!(probe.duration_s, Some(596.5));
    assert_eq!(probe.extractor.as_deref(), Some("PeerTube"));
    assert_eq!(
        probe.subtitles,
        ["en", "fr"],
        "sans les sous-titres automatiques"
    );
    assert_eq!(
        probe.formats.len(),
        4,
        "un format sans identifiant est écarté"
    );
    let audio = &probe.formats[0];
    assert_eq!(audio.vcodec, None);
    assert_eq!(audio.acodec.as_deref(), Some("mp4a.40.2"));
    assert_eq!(
        (audio.filesize, audio.filesize_approx),
        (Some(9_000_000), false)
    );
    let video = &probe.formats[1];
    assert_eq!((video.width, video.height), (Some(1280), Some(720)));
    assert_eq!(video.acodec, None);
    assert_eq!(
        (video.filesize, video.filesize_approx),
        (Some(180_000_000), true)
    );
    assert_eq!(video.note.as_deref(), Some("720p"));
    assert_eq!(probe.formats[2].protocol, "m3u8_native");
    assert!(!probe.formats[2].drm);
    assert!(probe.formats[3].drm, "« maybe » compte comme protégé");
}

/// Sous-titres (F-24) : `.srt` préféré, puis WebVTT ; formats propres à un site, HLS et
/// codes de langue étranges écartés ; texte donné en ligne accepté ; en-têtes du format,
/// sinon de la page.
#[test]
fn subtitles_are_picked_by_format_and_protocol() {
    let answer = json!({
        "title": "t", "format_id": "0", "ext": "mp4", "protocol": "https",
        "url": "https://exemple.org/v.mp4",
        "http_headers": {"User-Agent": "ua-page"},
        "subtitles": {
            "fr": [{"ext": "json3", "url": "https://exemple.org/fr.json3"},
                   {"ext": "vtt", "url": "https://exemple.org/fr.vtt"},
                   {"ext": "srt", "url": "https://exemple.org/fr.srt",
                    "http_headers": {"User-Agent": "ua-sous-titre"}}],
            "en": [{"ext": "vtt", "url": "https://exemple.org/en.vtt"}],
            "de": [{"ext": "srt", "data": "1\n00:00:00,000 --> 00:00:01,000\nHallo\n"}],
            "es": [{"ext": "vtt", "url": "https://exemple.org/es.m3u8",
                    "protocol": "m3u8_native"}],
            "it": [{"ext": "srv3", "url": "https://exemple.org/it.srv3"}],
            "pt": [{"ext": "vtt", "url": "ftp://exemple.org/pt.vtt"}],
            "../x": [{"ext": "vtt", "url": "https://exemple.org/x.vtt"}]
        }
    });
    let resolution = parse_resolution(&answer).unwrap();
    let mut got: Vec<(String, String)> = resolution
        .subtitles
        .iter()
        .map(|s| (s.language.clone(), s.ext.clone()))
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            ("de".to_owned(), "srt".to_owned()),
            ("en".into(), "vtt".into()),
            ("fr".into(), "srt".into()),
        ]
    );
    let by = |lang: &str| {
        resolution
            .subtitles
            .iter()
            .find(|s| s.language == lang)
            .unwrap()
            .source
            .clone()
    };
    assert_eq!(
        by("fr"),
        SubtitleSource::Url {
            url: "https://exemple.org/fr.srt".into(),
            headers: vec![("User-Agent".into(), "ua-sous-titre".into())],
        }
    );
    assert_eq!(
        by("en"),
        SubtitleSource::Url {
            url: "https://exemple.org/en.vtt".into(),
            headers: vec![("User-Agent".into(), "ua-page".into())],
        },
        "en-têtes de la page"
    );
    assert!(matches!(by("de"), SubtitleSource::Data(t) if t.contains("Hallo")));
    // `media.probe` ne propose que les mêmes langues.
    assert_eq!(parse_probe(&answer).unwrap().subtitles, ["de", "en", "fr"]);
}

#[test]
fn a_single_format_video_and_a_playlist() {
    // Vidéo à format unique : décrite à la racine.
    let single = json!({"title": "t", "format_id": "0", "ext": "webm",
                        "vcodec": "vp9", "acodec": "opus", "protocol": "https"});
    let probe = parse_probe(&single).unwrap();
    assert_eq!(probe.formats.len(), 1);
    assert_eq!(probe.formats[0].ext, "webm");

    let playlist = json!({"_type": "playlist", "entries": []});
    assert!(matches!(
        parse_probe(&playlist),
        Err(ExtractError::Failed(_))
    ));
    assert!(matches!(
        parse_probe(&json!({"title": "rien"})),
        Err(ExtractError::Failed(_))
    ));
}

#[test]
fn error_messages_never_cite_an_address() {
    let stderr = b"WARNING: [generic] something\n\
        ERROR: [generic] Unable to download webpage: HTTP Error 404: Not Found \
        (caused by <HTTPError 404: https://secret.example/page?token=abc>)\n";
    let message = error_message(stderr);
    assert!(!message.contains("secret.example"), "{message}");
    assert!(!message.contains("token"), "{message}");
    assert!(message.contains("HTTP Error 404"), "{message}");
    assert!(message.contains("<adresse>"), "{message}");
    assert!(!message.starts_with("ERROR"), "{message}");
    // Sans ligne `ERROR:`, la dernière ligne ; long, raccourci.
    assert_eq!(error_message(b"premiere\nderniere\n"), "derniere");
    assert_eq!(error_message(b""), "yt-dlp a échoué sans message");
    assert!(error_message("x".repeat(1000).as_bytes()).chars().count() <= 301);
}

#[tokio::test]
async fn ytdlp_is_run_with_the_address_after_the_options() {
    let dir = tempfile::tempdir().unwrap();
    let ytdlp = fake_ytdlp(dir.path(), &sample().to_string(), "", 0, 0);
    let probe = probe(
        &ytdlp,
        &crate::net::Client::new().unwrap(),
        "https://exemple.org/video/1",
        PROBE_TIMEOUT,
    )
    .await
    .unwrap();
    assert_eq!(probe.title, "Un film libre");
    let args = std::fs::read_to_string(dir.path().join("args.txt")).unwrap();
    for wanted in [
        "--ignore-config",
        "--no-cache-dir",
        "-J",
        "--no-playlist",
        "--write-subs",
    ] {
        assert!(args.contains(wanted), "{wanted} absent : {args}");
    }
    let end = args.trim_end();
    assert!(
        end.ends_with("-- https://exemple.org/video/1")
            || end.ends_with("--\nhttps://exemple.org/video/1"),
        "l'adresse vient après `--` : {args}"
    );
}

#[tokio::test]
async fn a_failing_ytdlp_gives_its_message_without_the_address() {
    let dir = tempfile::tempdir().unwrap();
    let ytdlp = fake_ytdlp(
        dir.path(),
        "",
        "ERROR: [generic] Unsupported URL: https://exemple.org/rien\n",
        1,
        0,
    );
    let err = probe(
        &ytdlp,
        &crate::net::Client::new().unwrap(),
        "https://exemple.org/rien",
        PROBE_TIMEOUT,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err,
        ExtractError::Failed("[generic] Unsupported URL: <adresse>".into())
    );
}

#[tokio::test]
async fn a_slow_ytdlp_is_stopped_at_its_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let ytdlp = fake_ytdlp(dir.path(), "", "", 0, 20);
    let started = std::time::Instant::now();
    let err = probe(
        &ytdlp,
        &crate::net::Client::new().unwrap(),
        "https://exemple.org/lent",
        Duration::from_millis(500),
    )
    .await
    .unwrap_err();
    assert_eq!(err, ExtractError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn garbage_output_and_a_missing_program_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let ytdlp = fake_ytdlp(dir.path(), "pas du JSON", "", 0, 0);
    assert!(matches!(
        probe(
            &ytdlp,
            &crate::net::Client::new().unwrap(),
            "https://exemple.org/x",
            PROBE_TIMEOUT
        )
        .await,
        Err(ExtractError::Failed(_))
    ));
    assert!(matches!(
        probe(
            &dir.path().join("absent"),
            &crate::net::Client::new().unwrap(),
            "https://exemple.org/x",
            PROBE_TIMEOUT
        )
        .await,
        Err(ExtractError::Launch(_))
    ));
}

/// Pages de vidéos sous licence libre pour les essais réels (décision de l'utilisateur du
/// 2026-11-09 : PeerTube, Wikimedia Commons, Internet Archive ; jamais YouTube).
pub(crate) const FREE_PAGES: [&str; 4] = [
    // Blender Foundation, PeerTube, CC BY-SA, 75 s.
    "https://video.blender.org/w/55bb3q2JuqERvT88aLEYCb",
    // Big Buck Bunny (Blender Foundation, CC BY), Wikimedia Commons ; 31 sous-titres WebVTT.
    "https://commons.wikimedia.org/wiki/File:Big_Buck_Bunny_4K.webm",
    // Big Buck Bunny, Internet Archive.
    "https://archive.org/details/BigBuckBunny_124",
    // « Bruce Schneier introduces FSF EncryptFest » (Free Software Foundation, CC BY,
    // 119 s), PeerTube de Framasoft : HLS, son seul à part, sous-titre anglais.
    FRAMATUBE_PAGE,
];

/// La page de [`FREE_PAGES`] servie en HLS.
pub(crate) const FRAMATUBE_PAGE: &str = "https://framatube.org/w/4Tg9KUhgBQSYB383W7VUKV";

/// Essai réel (ignoré par défaut) : le vrai yt-dlp décrit des pages de vidéos sous licence
/// libre. Outils installés dans `AURO_TOOLS_DIR` s'il est donné (gardés d'un essai à
/// l'autre), sinon dans un dossier temporaire.
/// `cargo test -p auro-engine --lib -- --ignored real_probe --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Internet : télécharge et lance yt-dlp sur des pages réelles"]
async fn real_probe_of_free_videos() {
    use crate::media::tools::{Sources, Tools};
    let temp = tempfile::tempdir().unwrap();
    let dir = std::env::var_os("AURO_TOOLS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| temp.path().to_path_buf());
    let tools = Tools::open(dir, Sources::default(), crate::net::Client::new().unwrap());
    if tools.status().ytdlp.is_none() {
        assert!(tools.begin_install());
        tools.install(false).await.unwrap();
    }
    let (ytdlp, _running) = tools.ytdlp().await.unwrap();
    for page in FREE_PAGES {
        let started = std::time::Instant::now();
        let probe = probe(
            &ytdlp,
            &crate::net::Client::new().unwrap(),
            page,
            PROBE_TIMEOUT,
        )
        .await
        .unwrap_or_else(|e| panic!("{page} : {e}"));
        let mut protocols: Vec<&str> = probe.formats.iter().map(|f| f.protocol.as_str()).collect();
        protocols.sort();
        protocols.dedup();
        let best = probe.formats.iter().filter_map(|f| f.height).max();
        eprintln!(
            "{} : « {} », {:?} s, {} formats ({}), hauteur max {:?}, sous-titres {:?}, DRM {}, en {:?}",
            probe.extractor.as_deref().unwrap_or("?"),
            probe.title,
            probe.duration_s,
            probe.formats.len(),
            protocols.join(", "),
            best,
            probe.subtitles,
            probe.formats.iter().any(|f| f.drm),
            started.elapsed()
        );
        assert!(!probe.formats.is_empty());
    }
}
