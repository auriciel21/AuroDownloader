//! Tests des outils : un faux serveur de publications (`testing::tools`), sans Internet.

use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::testing::tools::FakeRelease;

/// Les outils d'un moteur dont le dossier est `dir`, servis par `release`.
fn tools_from(release: &FakeRelease, dir: &Path) -> Tools {
    Tools::open(dir.to_path_buf(), release.sources(), Client::new().unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installing_fetches_checks_and_keeps_both_tools() {
    let release = FakeRelease::new();
    release.publish_ytdlp("2026.11.01", b"faux yt-dlp", None);
    release.publish_default_ffmpeg();
    let dir = tempfile::tempdir().unwrap();
    let tools = tools_from(&release, dir.path());
    assert!(!tools.status().ready());

    assert!(tools.begin_install());
    let status = tools.install(false).await.unwrap();
    assert!(status.ready(), "{status:?}");
    assert!(!status.installing);
    let ytdlp = status.ytdlp.unwrap();
    assert_eq!(ytdlp.version, "2026.11.01");
    assert_eq!(ytdlp.origin, Origin::Download);
    assert_eq!(std::fs::read(&ytdlp.path).unwrap(), b"faux yt-dlp");
    let ffmpeg = status.ffmpeg.unwrap();
    assert_eq!(std::fs::read(&ffmpeg.path).unwrap(), b"faux ffmpeg");
    assert_eq!(
        std::fs::read(status.ffprobe.unwrap()).unwrap(),
        b"faux ffprobe"
    );
    // Seuls les outils, leur état et rien d'autre (ni archive, ni fichier provisoire, ni
    // ffplay).
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut expected = vec![
        exe("ffmpeg"),
        exe("ffprobe"),
        exe("yt-dlp"),
        STATE_FILE.into(),
    ];
    expected.sort();
    assert_eq!(names, expected);

    // Un autre moteur sur le même dossier les retrouve.
    let again = tools_from(&release, dir.path());
    assert!(again.status().ready());
    assert_eq!(again.status().ytdlp.unwrap().version, "2026.11.01");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_list_signed_by_another_key_installs_nothing() {
    let release = FakeRelease::new();
    let stranger = FakeRelease::new();
    release.publish_ytdlp("2026.11.01", b"faux yt-dlp", Some(&stranger));
    release.publish_default_ffmpeg();
    let dir = tempfile::tempdir().unwrap();
    let tools = tools_from(&release, dir.path());
    assert!(tools.begin_install());
    let err = tools.install(false).await.unwrap_err();
    assert!(
        matches!(err, ToolsError::Signature(PgpError::WrongKey)),
        "{err}"
    );
    let status = tools.status();
    assert!(status.ytdlp.is_none());
    assert!(status.error.is_some());
    assert!(!status.installing);
    assert!(!dir.path().join(exe("yt-dlp")).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_that_differs_from_its_published_sum_is_refused() {
    let release = FakeRelease::new();
    release.publish_ytdlp("2026.11.01", b"faux yt-dlp", None);
    // Le fichier servi n'est plus celui de la liste signée.
    release.put(
        &format!("/dl/2026.11.01/{}", ytdlp_asset().unwrap()),
        b"yt-dlp piege".to_vec(),
    );
    let dir = tempfile::tempdir().unwrap();
    let tools = tools_from(&release, dir.path());
    assert!(tools.begin_install());
    let err = tools.install(false).await.unwrap_err();
    assert!(err.to_string().contains("empreinte différente"), "{err}");
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(left.is_empty(), "aucun fichier laissé : {left:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ffmpeg_archive_that_differs_or_lacks_ffprobe_is_refused() {
    let release = FakeRelease::new();
    release.publish_ytdlp("2026.11.01", b"faux yt-dlp", None);
    let dir = tempfile::tempdir().unwrap();
    let tools = tools_from(&release, dir.path());

    // Sans ffprobe.
    let only = format!("x/bin/{}", exe("ffmpeg"));
    release.publish_ffmpeg(&[(only.as_str(), b"faux ffmpeg")]);
    assert!(tools.begin_install());
    let err = tools.install(false).await.unwrap_err();
    assert!(err.to_string().contains("sans ffmpeg ou ffprobe"), "{err}");
    assert!(tools.status().ffmpeg.is_none());

    // Archive changée après la publication de son empreinte.
    release.publish_default_ffmpeg();
    let asset = ffmpeg_asset().unwrap();
    let mut archive = release.files.lock().unwrap()[&format!("/ffmpeg/{asset}")].clone();
    archive.push(0);
    release.put(&format!("/ffmpeg/{asset}"), archive);
    assert!(tools.begin_install());
    let err = tools.install(false).await.unwrap_err();
    assert!(err.to_string().contains("empreinte différente"), "{err}");
    assert!(tools.status().ffmpeg.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ytdlp_updates_at_most_once_a_day_to_a_newer_release() {
    let release = FakeRelease::new();
    release.publish_ytdlp("2026.11.01", b"version 1", None);
    release.publish_default_ffmpeg();
    let dir = tempfile::tempdir().unwrap();
    let tools = tools_from(&release, dir.path());
    assert!(tools.begin_install());
    tools.install(false).await.unwrap();
    let installed_at = crate::store::now_ms();

    release.publish_ytdlp("2026.11.05", b"version 2", None);
    // L'installation vient de chercher : pas avant 24 heures.
    assert_eq!(
        tools
            .update_ytdlp_if_due(installed_at + 1000)
            .await
            .unwrap(),
        None
    );
    let later = installed_at + UPDATE_EVERY_MS + 1;
    assert_eq!(
        tools.update_ytdlp_if_due(later).await.unwrap().as_deref(),
        Some("2026.11.05")
    );
    let ytdlp = tools.status().ytdlp.unwrap();
    assert_eq!(ytdlp.version, "2026.11.05");
    assert_eq!(std::fs::read(&ytdlp.path).unwrap(), b"version 2");
    // Rien de plus le même jour, même si une autre version paraît.
    release.publish_ytdlp("2026.11.06", b"version 3", None);
    assert_eq!(tools.update_ytdlp_if_due(later + 1000).await.unwrap(), None);
    // Le moment de la dernière recherche survit au redémarrage.
    let again = tools_from(&release, dir.path());
    assert_eq!(again.update_ytdlp_if_due(later + 1000).await.unwrap(), None);

    // Une mise à jour dont la signature est fausse garde l'ancienne version.
    let stranger = FakeRelease::new();
    release.publish_ytdlp("2026.11.07", b"version 4", Some(&stranger));
    let much_later = later + 2 * UPDATE_EVERY_MS;
    assert!(tools.update_ytdlp_if_due(much_later).await.is_err());
    let ytdlp = tools.status().ytdlp.unwrap();
    assert_eq!(ytdlp.version, "2026.11.05");
    assert_eq!(std::fs::read(&ytdlp.path).unwrap(), b"version 2");

    // Audit V10 (décision 11) : une publication plus ancienne, même bien signée, n'est
    // jamais installée.
    release.publish_ytdlp("2026.10.01", b"ancienne", None);
    let even_later = much_later + 2 * UPDATE_EVERY_MS;
    assert_eq!(tools.update_ytdlp_if_due(even_later).await.unwrap(), None);
    let ytdlp = tools.status().ytdlp.unwrap();
    assert_eq!(ytdlp.version, "2026.11.05");
    assert_eq!(std::fs::read(&ytdlp.path).unwrap(), b"version 2");
}

/// Audit V10 : comparaison des étiquettes de yt-dlp.
#[test]
fn only_a_newer_ytdlp_tag_is_newer() {
    assert!(is_newer("2026.11.05", "2026.11.01"));
    assert!(is_newer("2026.11.05.1", "2026.11.05"));
    assert!(is_newer("2026.12.01", "2026.11.30"));
    assert!(!is_newer("2026.11.01", "2026.11.05"));
    assert!(!is_newer("2026.11.05", "2026.11.05"));
    assert!(!is_newer("nightly", "2026.11.05"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_waits_until_no_ytdlp_runs() {
    let release = FakeRelease::new();
    release.publish_ytdlp("2026.11.01", b"version 1", None);
    release.publish_default_ffmpeg();
    let dir = tempfile::tempdir().unwrap();
    let tools = Arc::new(tools_from(&release, dir.path()));
    assert!(tools.begin_install());
    tools.install(false).await.unwrap();
    release.publish_ytdlp("2026.11.05", b"version 2", None);

    let (path, running) = tools.ytdlp().await.unwrap();
    let updating = tokio::spawn({
        let tools = Arc::clone(&tools);
        async move {
            tools
                .update_ytdlp_if_due(crate::store::now_ms() + 2 * UPDATE_EVERY_MS)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !updating.is_finished(),
        "remplacé pendant qu'un yt-dlp tourne"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"version 1");
    drop(running);
    let updated = tokio::time::timeout(Duration::from_secs(10), updating)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(updated.as_deref(), Some("2026.11.05"));
    assert_eq!(std::fs::read(&path).unwrap(), b"version 2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_one_installation_at_a_time() {
    let release = FakeRelease::new();
    let dir = tempfile::tempdir().unwrap();
    let tools = tools_from(&release, dir.path());
    assert!(tools.begin_install());
    assert!(!tools.begin_install());
    assert!(tools.status().installing);
    // Rien de publié : l'installation échoue, et une autre peut commencer.
    assert!(tools.install(false).await.is_err());
    assert!(!tools.status().installing);
    assert!(tools.begin_install());
}

/// Outils installés par un faux serveur dans `dir` (yt-dlp, ffmpeg et ffprobe).
async fn installed(release: &FakeRelease, dir: &Path) {
    release.publish_ytdlp("2026.11.01", b"faux yt-dlp", None);
    release.publish_default_ffmpeg();
    let tools = tools_from(release, dir);
    assert!(tools.begin_install());
    assert!(tools.install(false).await.unwrap().ready());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn damaged_tools_are_forgotten_before_their_first_use() {
    let release = FakeRelease::new();
    let dir = tempfile::tempdir().unwrap();
    installed(&release, dir.path()).await;

    // Intacts : un nouveau processus les garde, sans rien réécrire.
    let state = dir.path().join(STATE_FILE);
    let before = std::fs::read(&state).unwrap();
    let tools = tools_from(&release, dir.path());
    tools.verify_once().await;
    assert!(tools.status().ready());
    assert!(tools.status().error.is_none());
    assert_eq!(std::fs::read(&state).unwrap(), before);

    // yt-dlp abîmé : oublié, et l'erreur le dit ; ffmpeg reste.
    std::fs::write(dir.path().join(exe("yt-dlp")), b"autre yt-dlp").unwrap();
    // Une seule vérification par processus.
    tools.verify_once().await;
    assert!(tools.status().ytdlp.is_some());
    let tools = tools_from(&release, dir.path());
    tools.verify_once().await;
    let status = tools.status();
    assert!(status.ytdlp.is_none());
    assert!(status.ffmpeg.is_some());
    assert!(status.error.unwrap().contains("yt-dlp"));
    // Oublié aussi pour un autre processus.
    assert!(tools_from(&release, dir.path()).status().ytdlp.is_none());

    // ffprobe remplacé : ffmpeg oublié avec lui.
    std::fs::write(dir.path().join(exe("ffprobe")), b"autre ffprobe").unwrap();
    let tools = tools_from(&release, dir.path());
    tools.verify_once().await;
    let status = tools.status();
    assert!(status.ffmpeg.is_none());
    assert!(status.error.unwrap().contains("ffmpeg"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_state_gets_the_sums_of_ffmpeg_and_ffprobe() {
    let release = FakeRelease::new();
    let dir = tempfile::tempdir().unwrap();
    installed(&release, dir.path()).await;
    // `tools.json` d'avant le 2026-11-24 : sans les empreintes des fichiers de ffmpeg.
    let state = dir.path().join(STATE_FILE);
    let mut json: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    assert!(json["ffmpeg"]["files"]["ffprobe"].is_string());
    json["ffmpeg"].as_object_mut().unwrap().remove("files");
    std::fs::write(&state, json.to_string()).unwrap();

    let tools = tools_from(&release, dir.path());
    tools.verify_once().await;
    assert!(tools.status().ready());
    let json: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    assert_eq!(
        json["ffmpeg"]["files"]["ffmpeg"].as_str(),
        sha256_of(&dir.path().join(exe("ffmpeg"))).as_deref()
    );
    // Relevées, elles servent : ffmpeg changé ensuite est oublié.
    std::fs::write(dir.path().join(exe("ffmpeg")), b"autre ffmpeg").unwrap();
    let tools = tools_from(&release, dir.path());
    tools.verify_once().await;
    assert!(tools.status().ffmpeg.is_none());
}

#[test]
fn sums_are_read_like_sha256sum() {
    let a = "a".repeat(64);
    let b = "B".repeat(64);
    let list = format!("{a}  yt-dlp\n{b} *yt-dlp.exe\r\nnimporte quoi\n");
    assert_eq!(find_sum(list.as_bytes(), "yt-dlp").unwrap(), a);
    assert_eq!(
        find_sum(list.as_bytes(), "yt-dlp.exe").unwrap(),
        b.to_lowercase()
    );
    assert!(find_sum(list.as_bytes(), "yt-dlp_linux").is_err());
    assert!(find_sum(b"court  yt-dlp", "yt-dlp").is_err());
}

#[test]
fn the_embedded_ytdlp_key_is_the_projects() {
    assert_eq!(ytdlp_key().fingerprint, YTDLP_FINGERPRINT);
    assert_eq!(Sources::default().ytdlp_key, ytdlp_key());
}

/// Essai réel (ignoré par défaut) : installation depuis les publications officielles
/// (GitHub de yt-dlp, builds BtbN), signature et empreintes comprises, dans un dossier
/// temporaire ; rien n'est exécuté. `cargo test -p auro-engine --lib -- --ignored real_tools`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Internet : télécharge yt-dlp et ffmpeg officiels"]
async fn real_tools_install_from_the_official_releases() {
    let dir = tempfile::tempdir().unwrap();
    let sources = Sources {
        system_ffmpeg: false,
        ..Sources::default()
    };
    let tools = Tools::open(dir.path().to_path_buf(), sources, Client::new().unwrap());
    assert!(tools.begin_install());
    let started = std::time::Instant::now();
    let status = tools.install(false).await.unwrap();
    assert!(status.ready(), "{status:?}");
    let ytdlp = status.ytdlp.unwrap();
    let ffmpeg = status.ffmpeg.unwrap();
    let size = |p: &Path| std::fs::metadata(p).unwrap().len();
    eprintln!(
        "yt-dlp {} ({} octets), ffmpeg {} ({} octets), ffprobe ({} octets), en {:?}",
        ytdlp.version,
        size(&ytdlp.path),
        ffmpeg.version,
        size(&ffmpeg.path),
        size(status.ffprobe.as_ref().unwrap()),
        started.elapsed()
    );
}
