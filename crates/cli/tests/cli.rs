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
    auro_engine::testing::stop(&engine).await;
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
    auro_engine::testing::stop(&engine).await;
}

/// Miroirs (SPEC §7.7, phase 6) : `auro get --mirror` les donne au moteur, qui les vérifie ;
/// `auro mirror` montre les sources et en ajoute une.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metalink_files_are_shown_added_and_followed() {
    use std::sync::{Arc, Mutex};

    use auro_engine::testing::{FileSite, content, content_sha256, serve_file};
    const LEN: u64 = 200_000;
    let site = || serve_file(Arc::new(Mutex::new(FileSite::new(LEN))));
    let (reference, mirror) = (site(), site());
    let dir = tempfile::tempdir().unwrap();
    let (engine, ep) = serve(dir.path(), "metalink", Options::default()).await;
    let dest = dir.path().join("dl");
    std::fs::create_dir_all(&dest).unwrap();
    let dest = dest.to_str().unwrap();
    let doc = format!(
        r#"<?xml version="1.0"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="a.bin"><size>{LEN}</size><hash type="sha-256">{}</hash>
    <url>{}</url><url>{}</url></file>
  <file name="lot/b.bin"><url>{}</url></file>
  <file name="../c.bin"><url>{}</url></file>
</metalink>"#,
        content_sha256(LEN),
        reference.url("/a.bin"),
        mirror.url("/a.bin"),
        reference.url("/b.bin"),
        reference.url("/c.bin"),
    );
    let file = dir.path().join("lot.meta4");
    std::fs::write(&file, doc).unwrap();
    let file = file.to_str().unwrap();

    let shown = ok(&ep, &["metalink", file]).await;
    let lines: Vec<&str> = shown.lines().collect();
    assert_eq!(lines[0], "Metalink 4", "{shown}");
    assert!(
        lines[1].contains("0  a.bin") && lines[1].contains("sha256, 2 sources"),
        "{shown}"
    );
    assert!(
        lines[2].contains("1  lot/b.bin") && lines[2].contains("taille inconnue"),
        "{shown}"
    );
    assert!(
        lines[3].starts_with("écarté : ../c.bin  nom refusé"),
        "{shown}"
    );
    // Les deux fichiers, suivis ensemble jusqu'au bout.
    let got = ok(&ep, &["get", file, "-o", dest]).await;
    assert!(got.contains("a.bin") && got.contains("lot/b.bin"), "{got}");
    for path in ["a.bin", "lot/b.bin"] {
        assert_eq!(
            std::fs::read(Path::new(dest).join(path)).unwrap(),
            content(LEN as usize),
            "{path}"
        );
    }
    // Un seul fichier choisi, en pause ; une empreinte ou un rang hors Metalink : refusés.
    let added = ok(&ep, &["add", file, "-o", dest, "--select", "1", "--paused"]).await;
    assert_eq!(added.lines().count(), 1, "{added}");
    assert!(added.trim_end().ends_with("lot/b.bin"), "{added}");
    for args in [
        vec![
            "add",
            file,
            "-o",
            dest,
            "--checksum",
            "md5:098f6bcd4621d373cade4e832627b4f6",
        ],
        vec!["add", &reference.url("/x.bin"), "-o", dest, "--select", "0"],
        vec!["add", file, "-o", dest, "--select", "7"],
    ] {
        let refused = auro(&ep, &args).await;
        assert!(!refused.status.success(), "{args:?}");
    }
    auro_engine::testing::stop(&engine).await;
}

#[tokio::test]
async fn links_of_a_page_or_a_list_are_filtered_and_added() {
    use std::sync::{Arc, Mutex};

    use auro_engine::testing::{FileSite, reply, serve as serve_http, serve_file};
    let small = serve_file(Arc::new(Mutex::new(FileSite::new(1000))));
    let big = serve_file(Arc::new(Mutex::new(FileSite::new(300_000))));
    let html = format!(
        r#"<a href="{}">a</a> <a href="{}">b</a> <a href="{}">c</a> <a href="suite.html">page</a> <a href="SHA256SUMS">sommes</a>"#,
        small.url("/a.iso"),
        big.url("/b.iso"),
        big.url("/c.zip"),
    );
    // SHA256SUMS : sans extension, mais servi en texte : un fichier.
    let page = serve_http(move |r| match r.path.as_str() {
        "/SHA256SUMS" => reply(
            "200 OK",
            &[("Content-Type", "text/plain"), ("Content-Length", "4")],
            b"abcd",
        ),
        _ => reply("200 OK", &[("Content-Type", "text/html")], html.as_bytes()),
    });
    let dir = tempfile::tempdir().unwrap();
    let (engine, ep) = serve(dir.path(), "liens", Options::default()).await;
    let dest = dir.path().join("dl");
    std::fs::create_dir_all(&dest).unwrap();
    let dest = dest.to_str().unwrap();

    // Toute la page, pages masquées : quatre fichiers avec leur taille, dont SHA256SUMS,
    // reconnu comme un fichier par son type.
    let shown = ok(&ep, &["links", &page.url("/liste.html")]).await;
    let lines: Vec<&str> = shown.lines().collect();
    assert_eq!(lines.len(), 5, "{shown}");
    assert!(
        lines[3].contains("4 o") && lines[3].contains("SHA256SUMS"),
        "{shown}"
    );
    assert!(
        lines[0].contains("1000 o") && lines[0].contains("a.iso"),
        "{shown}"
    );
    assert!(
        lines[4].contains("4 lien(s) gardé(s), 1 écarté(s)"),
        "{shown}"
    );
    // Extension, taille, expression.
    let iso = ok(
        &ep,
        &["links", &page.url("/"), "-e", ".ISO", "--min-size", "100K"],
    )
    .await;
    assert!(
        iso.lines().next().unwrap().contains("b.iso") && iso.lines().count() == 2,
        "{iso}"
    );
    let regex = ok(
        &ep,
        &["links", &page.url("/"), "--regex", r"/c\.", "--no-sizes"],
    )
    .await;
    assert!(regex.starts_with("   0           ?  c.zip"), "{regex}");
    let all = ok(&ep, &["links", &page.url("/"), "--all", "--no-sizes"]).await;
    assert!(all.contains("suite.html"), "{all}");

    // Une liste CSV importée, ajoutée.
    let list = dir.path().join("liste.csv");
    std::fs::write(
        &list,
        format!(
            "url;nom\n{};a\n\"{}\";c\n",
            small.url("/a.iso"),
            big.url("/c.zip")
        ),
    )
    .unwrap();
    let added = ok(
        &ep,
        &[
            "links",
            list.to_str().unwrap(),
            "--add",
            "-o",
            dest,
            "--paused",
        ],
    )
    .await;
    // Les lignes d'ajout (« tâche  adresse ») suivent la liste, qui commence par des espaces.
    let ids: Vec<&str> = added
        .lines()
        .filter(|l| !l.starts_with(' ') && l.contains("://"))
        .collect();
    assert_eq!(ids.len(), 2, "{added}");
    let listed = ok(&ep, &["list"]).await;
    assert_eq!(
        listed
            .lines()
            .filter(|l| l.contains("En pause") || l.contains("paused"))
            .count(),
        2,
        "{listed}"
    );
    // Ni adresse ni fichier ; expression illisible ; filtre de taille sans tailles.
    for args in [
        vec!["links", "absent.txt"],
        vec!["links", list.to_str().unwrap(), "--regex", "("],
        vec![
            "links",
            list.to_str().unwrap(),
            "--no-sizes",
            "--min-size",
            "1K",
        ],
    ] {
        assert!(!auro(&ep, &args).await.status.success(), "{args:?}");
    }
    auro_engine::testing::stop(&engine).await;
}

#[tokio::test]
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
    auro_engine::testing::stop(&engine).await;
}

/// Jetons de l'interface distante (SPEC §7.19) : créés avec leur portée, montrés une seule
/// fois, listés sans leur valeur, révoqués par nom ou identifiant ; `admin` refusé.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokens_are_created_listed_and_revoked() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, ep) = serve(dir.path(), "jetons", Options::default()).await;
    assert!(ok(&ep, &["token", "list"]).await.contains("Aucun jeton."));
    let created = ok(&ep, &["token", "create", "téléphone"]).await;
    assert!(created.contains("portée control"), "{created}");
    let secret = created
        .lines()
        .find(|l| l.starts_with("auro_"))
        .expect("jeton affiché")
        .to_owned();
    ok(&ep, &["token", "create", "tableau", "--scope", "read"]).await;
    let refused = auro(&ep, &["token", "create", "x", "--scope", "admin"]).await;
    assert!(!refused.status.success());
    let listed = ok(&ep, &["token", "list"]).await;
    assert!(
        listed.contains("téléphone") && listed.contains("tableau"),
        "{listed}"
    );
    assert!(listed.contains("jamais"), "{listed}");
    assert!(!listed.contains(&secret), "{listed}");
    assert!(engine.authenticate(&secret).await.unwrap().is_some());

    let revoked = ok(&ep, &["token", "revoke", "téléphone"]).await;
    assert!(revoked.contains("révoqué"), "{revoked}");
    assert!(engine.authenticate(&secret).await.unwrap().is_none());
    let unknown = auro(&ep, &["token", "revoke", "téléphone"]).await;
    assert!(!unknown.status.success());
    let id = engine.tokens().await.unwrap()[0].id.clone();
    ok(&ep, &["token", "revoke", &id]).await;
    assert!(ok(&ep, &["token", "list"]).await.contains("Aucun jeton."));
    auro_engine::testing::stop(&engine).await;
}

/// `auro remote` (SPEC §7.19) : l'écoute suit les réglages ; hors de la boucle locale,
/// elle reste fermée faute de TLS.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_remote_interface_is_opened_moved_and_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, ep) = serve(dir.path(), "distante", Options::default()).await;
    tokio::spawn(auro_engine::remote::run(engine.clone()));
    let status = ok(&ep, &["remote"]).await;
    assert!(
        status.contains("coupée") && status.contains("127.0.0.1:7380"),
        "{status}"
    );
    assert!(status.contains("refusé"), "{status}");

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let listen = format!("127.0.0.1:{port}");
    let opened = ok(&ep, &["remote", "enable", "--listen", &listen]).await;
    assert!(
        opened.contains(&format!("ouverte sur {listen}"))
            && opened.contains(&format!("http://{listen}")),
        "{opened}"
    );
    assert!(tokio::net::TcpStream::connect(&listen).await.is_ok());

    // Noms ajoutés : remplacés, vérifiés, effacés.
    let named = ok(&ep, &["remote", "hosts", "NAS.maison", "192.168.1.9"]).await;
    assert!(
        named.contains("Noms ajoutés : nas.maison, 192.168.1.9")
            && named.contains(&format!("http://nas.maison:{port}")),
        "{named}"
    );
    assert!(
        !auro(&ep, &["remote", "hosts", "pas un nom"])
            .await
            .status
            .success()
    );
    let cleared = ok(&ep, &["remote", "hosts", "--clear"]).await;
    assert!(!cleared.contains("Noms ajoutés"), "{cleared}");
    // Certificat fourni : chemins rendus absolus ; puis retour à celui du moteur.
    let provided = ok(
        &ep,
        &["remote", "cert", "--file", "c.pem", "--key", "k.pem"],
    )
    .await;
    let cert = std::path::absolute("c.pem").unwrap();
    assert!(
        provided.contains(&format!("Certificat fourni : {}", cert.display())),
        "{provided}"
    );
    let generated = ok(&ep, &["remote", "cert", "--generated"]).await;
    assert!(!generated.contains("Certificat fourni"), "{generated}");
    assert!(
        !auro(&ep, &["remote", "cert", "--file", "c.pem"])
            .await
            .status
            .success()
    );

    let private = ok(&ep, &["remote", "allow-private", "on"]).await;
    assert!(private.contains("permis"), "{private}");
    // Hors de la boucle locale : en TLS ; cette adresse n'est pas à la machine.
    let refused = ok(&ep, &["remote", "enable", "--listen", "192.0.2.10:7380"]).await;
    assert!(
        refused.contains("fermée") && refused.contains("écoute impossible"),
        "{refused}"
    );
    let closed = ok(&ep, &["remote", "disable"]).await;
    assert!(closed.contains("coupée"), "{closed}");
    let bad = auro(&ep, &["remote", "enable", "--listen", "pas-une-adresse"]).await;
    assert!(!bad.status.success());
    auro_engine::testing::stop(&engine).await;
}
