//! Tests du module torrent (SPEC §7.16) : un essaim local, sans Internet. Un pair librqbit
//! (`testing::swarm`) partage des fichiers depuis un dossier temporaire ; le moteur les
//! télécharge par ce seul pair, sans DHT ni connexion entrante.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use librqbit::{AddTorrent, Session, SessionOptions};
use tokio::sync::broadcast;
use tokio::time::timeout;

use super::{Binding, Layout, Listen, Options, inspect_metainfo, system_interface};
use crate::io::part_path;
use crate::net::Client;
use crate::net::filename::{NameRules, sanitize};
use crate::scheduler::{AddTask, Config, DISK_RECHECK, Engine, Event, Planning};
use crate::store::tasks::{TaskKind, TaskState};
use crate::testing::content_range;
use crate::testing::swarm::Seeder;
use crate::transfer::Options as TransferOptions;

const WAIT: Duration = Duration::from_secs(60);

/// Un pair qui partage un torrent.
struct Seed {
    addr: SocketAddr,
    metainfo: Vec<u8>,
    _seeder: Seeder,
    _dir: tempfile::TempDir,
}

/// Contenu d'un fichier partagé : différent d'un fichier à l'autre.
fn data(index: usize, len: usize) -> Vec<u8> {
    content_range(index as u64 * 7919, len)
}

/// Un pair qui partage `files` : un seul fichier si son chemin est vide (`name` est alors
/// son nom), sinon un dossier `name` qui les contient. `upload_bps` le ralentit.
async fn seeder(name: &str, files: &[(&str, usize)], upload_bps: Option<u32>) -> Seed {
    let dir = tempfile::tempdir().unwrap();
    let seeder = Seeder::start(&dir.path().join("seed"), upload_bps).await;
    let contents: Vec<(&str, Vec<u8>)> = files
        .iter()
        .enumerate()
        .map(|(i, (rel, len))| (*rel, data(i, *len)))
        .collect();
    let metainfo = seeder.share(name, &contents).await;
    Seed {
        addr: seeder.addr,
        metainfo,
        _seeder: seeder,
        _dir: dir,
    }
}

fn config(dir: &Path, torrent: Options) -> Config {
    Config {
        db_path: dir.join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: TransferOptions {
            torrent,
            ..TransferOptions::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    }
}

/// Un moteur qui télécharge par ces seuls pairs, sans partager ensuite (ratio 0) : les
/// tests du partage règlent le leur.
async fn start(dir: &Path, peers: Vec<SocketAddr>) -> Engine {
    let engine = Engine::start(config(dir, Options::local(peers))).unwrap();
    engine.set_torrent_ratio(Some(0.0)).await.unwrap();
    engine
}

/// Un moteur qui écoute à `listen` (un pair local vient y chercher ce qu'il partage), au
/// ratio par défaut (1).
fn sharing_engine(dir: &Path, peers: Vec<SocketAddr>, listen: SocketAddr) -> Engine {
    let torrent = Options {
        listen: Listen::Addr(listen),
        ..Options::local(peers)
    };
    Engine::start(config(dir, torrent)).unwrap()
}

/// Une adresse locale libre (le moteur l'ouvrira ensuite).
fn free_addr() -> SocketAddr {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
}

/// Un pair qui vient télécharger le torrent depuis `peer` seulement.
async fn leecher(metainfo: &[u8], peer: SocketAddr) -> (Arc<Session>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new_with_opts(
        dir.path().to_path_buf(),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    session
        .add_torrent(
            AddTorrent::from_bytes(metainfo.to_vec()),
            Some(librqbit::AddTorrentOptions {
                initial_peers: Some(vec![peer]),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    (session, dir)
}

/// Octets reçus par le pair qui télécharge.
fn leeched(session: &Session) -> u64 {
    session.with_torrents(|ts| ts.map(|(_, t)| t.stats().progress_bytes).sum())
}

fn add(dest: &Path, metainfo: &[u8]) -> AddTask {
    AddTask {
        torrent: Some(metainfo.to_vec()),
        dest_dir: Some(dest.to_str().unwrap().to_owned()),
        ..AddTask::default()
    }
}

/// Attend que `task_id` passe à l'état `to`.
async fn wait_state(events: &mut broadcast::Receiver<Event>, task_id: &str, to: TaskState) {
    let found = timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(Event::StateChanged {
                    task_id: t, to: s, ..
                }) if t == task_id && s == to => return,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("moteur arrêté"),
            }
        }
    })
    .await;
    assert!(found.is_ok(), "{task_id} n'est jamais passée à {to:?}");
}

/// Attend qu'un avancement de `task_id` dépasse `bytes`.
async fn wait_progress(events: &mut broadcast::Receiver<Event>, task_id: &str, bytes: u64) {
    let found = timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(Event::Progress {
                    task_id: t,
                    progress,
                    ..
                }) if t == task_id && progress.received >= bytes => return,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("moteur arrêté"),
            }
        }
    })
    .await;
    assert!(found.is_ok(), "{task_id} n'a jamais reçu {bytes} octets");
}

/// Aucun `.part` sous `dir`.
fn no_part_left(dir: &Path) {
    for entry in walk(dir) {
        assert!(
            !entry.to_string_lossy().ends_with(".part"),
            "{entry:?} reste"
        );
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        }
        out.push(path);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_single_file_torrent_ends_under_its_own_name() {
    let seed = seeder("image.iso", &[("", 300 * 1024 + 17)], None).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;

    let task = engine.get(&id).await.unwrap();
    assert_eq!(task.kind, TaskKind::Torrent);
    assert_eq!(task.file_name.as_deref(), Some("image.iso"));
    assert_eq!(task.total_bytes, Some(300 * 1024 + 17));
    assert_eq!(
        std::fs::read(dir.path().join("image.iso")).unwrap(),
        data(0, 300 * 1024 + 17)
    );
    no_part_left(dir.path());
    crate::testing::stop(&engine).await;
}

/// Règles de rangement (SPEC §7.21) : un torrent ajouté sans dossier va dans celui de la
/// règle, sous son propre nom (jamais renommé).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rule_places_a_torrent_folder_without_renaming_it() {
    let seed = seeder(
        "Album",
        &[("piste.flac", 40 * 1024), ("pochette.jpg", 9)],
        None,
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("dl");
    std::fs::create_dir(&downloads).unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    engine
        .set_download_dir(Some(downloads.to_str().unwrap().to_owned()))
        .await
        .unwrap();
    let rule = crate::rules::Rule::from_json(&serde_json::json!({
        "match": {"nameRegex": "^Album$"},
        "actions": {"folder": "Torrents/Musique", "rename": "{date}-{nom}"}
    }))
    .unwrap();
    engine.set_rules(vec![rule]).await.unwrap();
    let mut events = engine.subscribe();
    let id = engine
        .add(AddTask {
            torrent: Some(seed.metainfo.clone()),
            ..AddTask::default()
        })
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;

    let task = engine.get(&id).await.unwrap();
    let placed = downloads.join("Torrents").join("Musique");
    assert_eq!(Path::new(&task.dest_dir), placed);
    assert_eq!(task.file_name.as_deref(), Some("Album"));
    assert_eq!(
        std::fs::read(placed.join("Album").join("piste.flac")).unwrap(),
        data(0, 40 * 1024)
    );
    no_part_left(dir.path());
    crate::testing::stop(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_multi_file_torrent_keeps_its_folder_and_paths() {
    let files = [
        ("a.txt", 40 * 1024 + 3),
        ("sous/b.bin", 70 * 1024),
        ("sous/plus/c.dat", 5),
        ("vide.txt", 0),
    ];
    let seed = seeder("Dossier", &files, None).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;

    let top = dir.path().join("Dossier");
    for (i, (rel, len)) in files.iter().enumerate() {
        assert_eq!(
            std::fs::read(top.join(rel)).unwrap(),
            data(i, *len),
            "{rel}"
        );
    }
    no_part_left(dir.path());
    crate::testing::stop(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_magnet_link_gets_its_metadata_from_the_peers() {
    let seed = seeder("magnet.bin", &[("", 100 * 1024)], None).await;
    let magnet = inspect_metainfo(&seed.metainfo).unwrap().magnet;
    assert!(magnet.starts_with("magnet:?xt=urn:btih:"), "{magnet}");
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    let mut events = engine.subscribe();
    let id = engine
        .add(AddTask {
            url: magnet,
            dest_dir: Some(dir.path().to_str().unwrap().to_owned()),
            ..AddTask::default()
        })
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(
        std::fs::read(dir.path().join("magnet.bin")).unwrap(),
        data(0, 100 * 1024)
    );
    crate::testing::stop(&engine).await;
    // Les métadonnées venues des pairs sont gardées : librqbit les réécrit autour du
    // dictionnaire `info` reçu, d'où la même empreinte d'information.
    let conn = crate::store::open(&dir.path().join("auro.db")).unwrap();
    let row = crate::store::torrents::load_torrent(&conn, &id).unwrap();
    let saved = inspect_metainfo(&row.metainfo.expect("métadonnées gardées")).unwrap();
    assert_eq!(
        saved.info_hash,
        inspect_metainfo(&seed.metainfo).unwrap().info_hash
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_name_already_taken_gets_a_number() {
    let seed = seeder("pris.bin", &[("", 20 * 1024)], None).await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pris.bin"), b"autre").unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    let task = engine.get(&id).await.unwrap();
    assert_eq!(task.file_name.as_deref(), Some("pris (1).bin"));
    assert_eq!(
        std::fs::read(dir.path().join("pris.bin")).unwrap(),
        b"autre"
    );
    assert_eq!(
        std::fs::read(dir.path().join("pris (1).bin")).unwrap(),
        data(0, 20 * 1024)
    );
    crate::testing::stop(&engine).await;
}

/// Un torrent mis en pause, dont le `.part` est abîmé pendant la pause (écritures perdues,
/// SPEC §7.16) : la reprise revérifie tout, retélécharge ce qui manque, et le fichier final
/// est juste.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_damaged_part_is_found_and_fetched_again_on_resume() {
    const LEN: usize = 1024 * 1024;
    let seed = seeder("lent.bin", &[("", LEN)], Some(256 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_progress(&mut events, &id, LEN as u64 / 4).await;
    engine.pause(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Paused).await;

    // Tout ce qui est reçu est effacé : rien ne doit en être gardé.
    let part = part_path(&dir.path().join("lent.bin"));
    assert!(part.exists());
    std::fs::write(&part, vec![0u8; LEN]).unwrap();

    engine.resume(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(
        std::fs::read(dir.path().join("lent.bin")).unwrap(),
        data(0, LEN)
    );
    crate::testing::stop(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupted_torrent_resumes_after_an_engine_restart() {
    const LEN: usize = 1024 * 1024;
    let files = [("un.bin", LEN / 2), ("deux.bin", LEN / 2)];
    let seed = seeder("Reprise", &files, Some(256 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_progress(&mut events, &id, LEN as u64 / 4).await;
    crate::testing::stop(&engine).await;

    let engine = start(dir.path(), vec![seed.addr]).await;
    let mut events = engine.subscribe();
    wait_state(&mut events, &id, TaskState::Completed).await;
    let top = dir.path().join("Reprise");
    for (i, (rel, len)) in files.iter().enumerate() {
        assert_eq!(
            std::fs::read(top.join(rel)).unwrap(),
            data(i, *len),
            "{rel}"
        );
    }
    no_part_left(dir.path());
    crate::testing::stop(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_with_delete_files_leaves_nothing_behind() {
    const LEN: usize = 512 * 1024;
    let files = [("un.bin", LEN), ("deux/trois.bin", LEN)];
    let seed = seeder("Annule", &files, Some(128 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_progress(&mut events, &id, 64 * 1024).await;
    engine.cancel(&id, true).await.unwrap();
    wait_state(&mut events, &id, TaskState::Cancelled).await;
    // Le moteur annonce l'annulation, puis efface les fichiers (comme en HTTP).
    let gone = timeout(Duration::from_secs(10), async {
        while dir.path().join("Annule").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "{:?}", walk(dir.path()));
    crate::testing::stop(&engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_torrent_twice_is_refused() {
    let seed = seeder("double.bin", &[("", 64 * 1024)], Some(16 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    let again = engine.add(add(dir.path(), &seed.metainfo)).await;
    assert!(again.is_err(), "{again:?}");
    // Par son lien magnet aussi.
    let magnet = inspect_metainfo(&seed.metainfo).unwrap().magnet;
    let again = engine
        .add(AddTask {
            url: magnet,
            dest_dir: Some(dir.path().to_str().unwrap().to_owned()),
            ..AddTask::default()
        })
        .await;
    assert!(again.is_err(), "{again:?}");
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn http_only_options_and_bad_torrents_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), Vec::new()).await;
    let mut task = add(dir.path(), b"pas un torrent");
    assert!(engine.add(task.clone()).await.is_err());
    task.torrent = None;
    task.url = "magnet:?dn=sans-empreinte".into();
    assert!(engine.add(task.clone()).await.is_err());
    task.url = "magnet:?xt=urn:btih:7acf8fb590b2060dd9c3146ef770169d593433b0".into();
    task.max_connections = Some(4);
    assert!(engine.add(task).await.is_err());
    crate::testing::stop(&engine).await;
}

/// Un `.torrent` écrit à la main (bencode), avec des noms donnés.
fn handmade(name: &str, paths: &[&[&str]]) -> Vec<u8> {
    let bstr = |s: &str| format!("{}:{s}", s.len()).into_bytes();
    let mut files = b"l".to_vec();
    for path in paths {
        files.extend_from_slice(b"d6:lengthi3e4:pathl");
        for c in *path {
            files.extend(bstr(c));
        }
        files.extend_from_slice(b"ee");
    }
    files.push(b'e');
    let pieces_count = paths.len() * 3 / 16384 + 1;
    let mut out = b"d4:infod5:files".to_vec();
    out.extend(files);
    out.extend_from_slice(b"4:name");
    out.extend(bstr(name));
    out.extend_from_slice(b"12:piece lengthi16384e6:pieces");
    out.extend(format!("{}:", pieces_count * 20).into_bytes());
    out.extend(vec![7u8; pieces_count * 20]);
    out.extend_from_slice(b"ee");
    out
}

#[test]
fn names_from_a_torrent_are_cleaned_and_never_leave_the_folder() {
    let rules = NameRules::native();
    let metainfo = handmade("Haut", &[&["CON"], &["a<b", "c:d.txt"], &["ok.txt"]]);
    let layout = Layout::parse(&metainfo).unwrap();
    assert_eq!(layout.top, "Haut");
    let dest = Path::new("/dest");
    let paths: Vec<_> = layout
        .final_paths(dest, "Haut")
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(
        paths,
        vec![
            dest.join("Haut").join(sanitize("CON", rules)),
            dest.join("Haut")
                .join(sanitize("a<b", rules))
                .join(sanitize("c:d.txt", rules)),
            dest.join("Haut").join("ok.txt"),
        ]
    );
    for p in &paths {
        assert!(p.starts_with(dest.join("Haut")), "{p:?}");
    }
    // Une remontée dans l'arborescence est refusée dès l'ajout.
    assert!(inspect_metainfo(&handmade("Haut", &[&["..", "x"]])).is_err());
    assert!(
        inspect_metainfo(&handmade("..", &[&["x"]])).is_err() || {
            // Un nom de torrent « .. » est nettoyé, jamais pris tel quel.
            Layout::parse(&handmade("..", &[&["x"]])).unwrap().top != ".."
        }
    );
}

// ---------- Étape 3b : partage, réseau facturé, choix des fichiers ----------

/// Partage jusqu'au ratio 1 (décision de l'utilisateur du 2026-11-09) : après le
/// téléchargement, la tâche passe à `seeding`, envoie autant qu'elle a reçu à un pair qui
/// vient le chercher, puis se termine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sharing_goes_on_until_the_ratio_then_ends() {
    const LEN: usize = 200 * 1024;
    let seed = seeder("partage.bin", &[("", LEN)], None).await;
    let dir = tempfile::tempdir().unwrap();
    let listen = free_addr();
    let engine = sharing_engine(dir.path(), vec![seed.addr], listen);
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::Seeding).await;
    // Le fichier est déjà à sa place pendant le partage.
    assert_eq!(
        std::fs::read(dir.path().join("partage.bin")).unwrap(),
        data(0, LEN)
    );

    let (peer, _peer_dir) = leecher(&seed.metainfo, listen).await;
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(leeched(&peer), LEN as u64);
    crate::testing::stop(&engine).await;
    let conn = crate::store::open(&dir.path().join("auro.db")).unwrap();
    let row = crate::store::torrents::load_torrent(&conn, &id).unwrap();
    assert!(row.uploaded_bytes >= LEN as u64, "{}", row.uploaded_bytes);
}

/// Commande de fin (F-34, SPEC §7.21) : lancée quand les fichiers sont à leur nom, avant
/// le partage, qui suit jusqu'au ratio.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_end_command_runs_before_sharing() {
    const LEN: usize = 100 * 1024;
    let seed = seeder("commande.bin", &[("", LEN)], None).await;
    let dir = tempfile::tempdir().unwrap();
    let listen = free_addr();
    let engine = sharing_engine(dir.path(), vec![seed.addr], listen);
    let marker = dir.path().join("vu.txt");
    let marker_text = marker.to_str().unwrap();
    let command = if cfg!(windows) {
        serde_json::json!({"program": r"C:\Windows\System32\cmd.exe",
                           "args": ["/c", "echo", "{nom}", "{taille}>", marker_text]})
    } else {
        serde_json::json!({"program": "/bin/sh",
                           "args": ["-c", r#"echo "$1 $2" > "$3""#, "sh", "{nom}", "{taille}", marker_text]})
    };
    let rule = crate::rules::Rule::from_json(&serde_json::json!({"actions": {"command": command}}))
        .unwrap();
    engine.set_rules(vec![rule]).await.unwrap();
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::PostProcessing).await;
    wait_state(&mut events, &id, TaskState::Seeding).await;
    let seen = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        seen.split_whitespace().collect::<Vec<_>>(),
        ["commande.bin", &LEN.to_string()]
    );

    let (peer, _peer_dir) = leecher(&seed.metainfo, listen).await;
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(leeched(&peer), LEN as u64);
    crate::testing::stop(&engine).await;
}

/// Une pause pendant le partage l'arrête : la tâche est terminée, son fichier en place.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pausing_while_sharing_ends_the_task() {
    let seed = seeder("pause.bin", &[("", 50 * 1024)], None).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = sharing_engine(dir.path(), vec![seed.addr], free_addr());
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::Seeding).await;
    engine.pause(&id).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(
        std::fs::read(dir.path().join("pause.bin")).unwrap(),
        data(0, 50 * 1024)
    );
    crate::testing::stop(&engine).await;
}

/// Un partage interrompu par l'arrêt du moteur reprend au démarrage suivant, et finit au
/// ratio.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sharing_resumes_after_an_engine_restart() {
    const LEN: usize = 100 * 1024;
    let seed = seeder("reprise.bin", &[("", LEN)], None).await;
    let dir = tempfile::tempdir().unwrap();
    let listen = free_addr();
    let engine = sharing_engine(dir.path(), vec![seed.addr], listen);
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::Seeding).await;
    crate::testing::stop(&engine).await;
    assert_eq!(engine_state(dir.path(), &id), TaskState::Seeding);

    let engine = sharing_engine(dir.path(), vec![seed.addr], listen);
    let mut events = engine.subscribe();
    let (peer, _peer_dir) = leecher(&seed.metainfo, listen).await;
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(leeched(&peer), LEN as u64);
    crate::testing::stop(&engine).await;
}

/// Sur un réseau facturé au volume (Android), le partage attend : rien ne part. Il reprend
/// quand le réseau ne l'est plus.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_metered_network_holds_sharing_back() {
    const LEN: usize = 100 * 1024;
    let seed = seeder("facture.bin", &[("", LEN)], None).await;
    let dir = tempfile::tempdir().unwrap();
    let listen = free_addr();
    let engine = sharing_engine(dir.path(), vec![seed.addr], listen);
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::Seeding).await;
    engine.set_network(true, true).await.unwrap();
    // Le temps que le partage s'arrête et que la session se ferme.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let (peer, _peer_dir) = leecher(&seed.metainfo, listen).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(leeched(&peer), 0);
    assert_eq!(engine.get(&id).await.unwrap().state, TaskState::Seeding);

    engine.set_network(true, false).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    crate::testing::stop(&engine).await;
}

/// `task.setFiles` en cours de téléchargement : seuls les fichiers choisis vont jusqu'au
/// bout et prennent leur nom final ; la taille de la tâche suit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn choosing_files_downloads_only_those() {
    const LEN: usize = 256 * 1024;
    let files = [("a.bin", LEN), ("b.bin", LEN), ("c.bin", LEN)];
    let seed = seeder("Choix", &files, Some(64 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_progress(&mut events, &id, 1).await;

    let task = engine.set_files(&id, vec![1]).await.unwrap();
    assert_eq!(task.total_bytes, Some(LEN as u64));
    // Contrat du protocole (SPEC §9.1) : `task.setFiles` rend cette tâche.
    crate::rpc::contract::check_reply(
        "task.setFiles",
        &serde_json::json!({"result": crate::rpc::task_json(&task)}),
    );
    // Rien de choisi, ou un rang inconnu : refusé.
    assert!(engine.set_files(&id, vec![]).await.is_err());
    assert!(engine.set_files(&id, vec![9]).await.is_err());

    wait_state(&mut events, &id, TaskState::Completed).await;
    let top = dir.path().join("Choix");
    assert_eq!(std::fs::read(top.join("b.bin")).unwrap(), data(1, LEN));
    assert!(!top.join("a.bin").exists());
    assert!(!top.join("c.bin").exists());

    let details = engine
        .details(
            &id,
            crate::scheduler::Include {
                files: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let files = details.files.unwrap();
    let chosen: Vec<bool> = files.iter().map(|f| f.selected).collect();
    assert_eq!(chosen, vec![false, true, false]);
    // Le fichier choisi est donné complet à la fin, pas au dernier relevé.
    assert_eq!(files[1].done_bytes, LEN as u64);
    crate::testing::stop(&engine).await;
}

/// État d'une tâche lu dans la base, moteur arrêté.
fn engine_state(dir: &Path, task_id: &str) -> TaskState {
    let conn = crate::store::open(&dir.join("auro.db")).unwrap();
    crate::store::tasks::load_task(&conn, task_id)
        .unwrap()
        .state
}

/// Un torrent « à plusieurs fichiers » qui n'en contient qu'un garde son dossier, comme le
/// veut le format (librqbit, lui, le range sans dossier quand on ne lui en donne pas).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folder_torrent_with_one_file_keeps_its_folder() {
    const LEN: usize = 300 * 1024;
    let seed = seeder("Seul", &[("sous/f0.bin", LEN)], None).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(
        std::fs::read(dir.path().join("Seul").join("sous").join("f0.bin")).unwrap(),
        data(0, LEN)
    );
    crate::testing::stop(&engine).await;
}

// Liaison à une interface réseau (VPN, SPEC §7.16, étape 3d). Les interfaces simulées sont
// liées en vrai à l'interface de bouclage, où écoute le pair local : librqbit s'y lie
// réellement (sous Windows, par les copies corrigées de third_party).

/// Interfaces simulées : nom → interface. Chaque test a ses propres noms.
static FAKE_INTERFACES: std::sync::Mutex<Vec<(String, Binding)>> =
    std::sync::Mutex::new(Vec::new());

fn fake_interfaces(name: &str) -> Option<Binding> {
    FAKE_INTERFACES
        .lock()
        .unwrap()
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, b)| b.clone())
}

/// L'interface simulée `name` apparaît avec cet index (un VPN reconnecté en change), ou
/// disparaît (`None`).
fn set_fake(name: &str, index: Option<u32>) {
    let binding = index.map(|index| Binding {
        index,
        ..loopback()
    });
    let mut all = FAKE_INTERFACES.lock().unwrap();
    all.retain(|(n, _)| n != name);
    if let Some(binding) = binding {
        all.push((name.to_owned(), binding));
    }
}

/// L'interface de bouclage du système (« lo », « Loopback Pseudo-Interface 1 »).
fn loopback() -> Binding {
    use network_interface::{NetworkInterface, NetworkInterfaceConfig};
    let nics = NetworkInterface::show().unwrap();
    let nic = nics
        .iter()
        .find(|n| {
            n.addr
                .iter()
                .any(|a| a.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST))
        })
        .expect("interface de bouclage");
    system_interface(&nic.name).expect("interface de bouclage utilisable")
}

/// Un moteur aux interfaces simulées, lié à `interface`, sans partage.
async fn bound_engine(dir: &Path, peers: Vec<SocketAddr>, interface: &str) -> Engine {
    let torrent = Options {
        interfaces: fake_interfaces,
        ..Options::local(peers)
    };
    let engine = Engine::start(config(dir, torrent)).unwrap();
    engine.set_torrent_ratio(Some(0.0)).await.unwrap();
    engine
        .set_torrent_interface(Some(interface.to_owned()))
        .await
        .unwrap();
    engine
}

/// Aucun changement d'état de `task_id` vers `to` pendant `delay`.
async fn never_goes(
    events: &mut broadcast::Receiver<Event>,
    task_id: &str,
    to: TaskState,
    delay: Duration,
) {
    let seen = timeout(delay, async {
        loop {
            match events.recv().await {
                Ok(Event::StateChanged {
                    task_id: t, to: s, ..
                }) if t == task_id && s == to => return,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("moteur arrêté"),
            }
        }
    })
    .await;
    assert!(seen.is_err(), "{task_id} est passée à {to:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_torrent_waits_for_its_interface() {
    const LEN: usize = 300 * 1024;
    let seed = seeder("attente.iso", &[("", LEN)], None).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = bound_engine(dir.path(), vec![seed.addr], "essai-attente").await;
    assert!(!engine.snapshot().await.unwrap().torrent_interface_up);
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();

    never_goes(&mut events, &id, TaskState::Active, Duration::from_secs(3)).await;
    let task = engine.get(&id).await.unwrap();
    assert_eq!(task.state, TaskState::Queued);
    assert_eq!(task.done_bytes, 0);

    set_fake("essai-attente", Some(7));
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert!(engine.snapshot().await.unwrap().torrent_interface_up);
    assert_eq!(
        std::fs::read(dir.path().join("attente.iso")).unwrap(),
        data(0, LEN)
    );
    crate::testing::stop(&engine).await;
}

/// L'interface disparaît en plein téléchargement (VPN coupé) : le torrent s'arrête et
/// attend ; elle revient avec un autre index (VPN reconnecté) : il repart sur une session
/// liée à elle et finit sans défaut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_interface_stops_torrents_until_it_returns() {
    const LEN: usize = 2 * 1024 * 1024;
    let seed = seeder("coupure.bin", &[("", LEN)], Some(256 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    set_fake("essai-coupure", Some(7));
    let engine = bound_engine(dir.path(), vec![seed.addr], "essai-coupure").await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_progress(&mut events, &id, 64 * 1024).await;

    set_fake("essai-coupure", None);
    wait_state(&mut events, &id, TaskState::Queued).await;
    assert!(!engine.snapshot().await.unwrap().torrent_interface_up);
    never_goes(&mut events, &id, TaskState::Active, Duration::from_secs(4)).await;

    set_fake("essai-coupure", Some(8));
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(
        std::fs::read(dir.path().join("coupure.bin")).unwrap(),
        data(0, LEN)
    );
    no_part_left(dir.path());
    crate::testing::stop(&engine).await;
}

/// L'interface change sans disparaître (adresse ou index) : les torrents en cours
/// repartent sur une nouvelle session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_interface_restarts_its_torrents() {
    const LEN: usize = 2 * 1024 * 1024;
    let seed = seeder("bascule.bin", &[("", LEN)], Some(256 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    set_fake("essai-bascule", Some(7));
    let engine = bound_engine(dir.path(), vec![seed.addr], "essai-bascule").await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_progress(&mut events, &id, 64 * 1024).await;

    set_fake("essai-bascule", Some(9));
    wait_state(&mut events, &id, TaskState::Queued).await;
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(
        std::fs::read(dir.path().join("bascule.bin")).unwrap(),
        data(0, LEN)
    );
    crate::testing::stop(&engine).await;
}

/// Le réglage changé en cours de route : les torrents repartent liés à la nouvelle
/// interface ; retiré, ils passent par les routes du système.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changing_the_interface_setting_rebinds_running_torrents() {
    const LEN: usize = 2 * 1024 * 1024;
    let seed = seeder("reglage.bin", &[("", LEN)], Some(256 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    set_fake("essai-reglage-a", Some(7));
    let engine = bound_engine(dir.path(), vec![seed.addr], "essai-reglage-a").await;
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_progress(&mut events, &id, 64 * 1024).await;

    // Une interface absente : le torrent s'arrête et attend.
    engine
        .set_torrent_interface(Some("essai-reglage-b".into()))
        .await
        .unwrap();
    wait_state(&mut events, &id, TaskState::Queued).await;
    never_goes(&mut events, &id, TaskState::Active, Duration::from_secs(3)).await;

    // Plus d'interface : il repart et finit.
    engine.set_torrent_interface(None).await.unwrap();
    assert_eq!(engine.torrent_interface().await.unwrap(), None);
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(
        std::fs::read(dir.path().join("reglage.bin")).unwrap(),
        data(0, LEN)
    );
    crate::testing::stop(&engine).await;
}

/// La vraie interface de bouclage, trouvée par son nom : le pair local est joint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bound_to_the_loopback_the_local_peer_is_reached() {
    const LEN: usize = 300 * 1024;
    let seed = seeder("boucle.iso", &[("", LEN)], None).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    engine
        .set_torrent_interface(Some(loopback().device))
        .await
        .unwrap();
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::Completed).await;
    assert_eq!(
        std::fs::read(dir.path().join("boucle.iso")).unwrap(),
        data(0, LEN)
    );
    crate::testing::stop(&engine).await;
}

/// Liée à une autre interface (une carte réseau), la session ne joint pas le pair local,
/// qui n'est que sur l'interface de bouclage : rien ne passe par une autre interface.
/// Sans carte réseau configurée (machine isolée), le test ne prouve rien et s'arrête.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bound_elsewhere_the_local_peer_is_never_reached() {
    use network_interface::{NetworkInterface, NetworkInterfaceConfig};
    let nics = NetworkInterface::show().unwrap();
    // L'interface de bouclage peut porter d'autres adresses (WSL : 10.255.255.254 sur
    // `lo`) ; sous Linux, une entrée par adresse : on l'écarte par son nom.
    let loopbacks: Vec<&str> = nics
        .iter()
        .filter(|n| n.addr.iter().any(|a| a.ip().is_loopback()))
        .map(|n| n.name.as_str())
        .collect();
    let other = nics
        .iter()
        .filter(|n| !loopbacks.contains(&n.name.as_str()))
        .filter(|n| n.addr.iter().any(|a| a.ip().is_ipv4()))
        .find_map(|n| system_interface(&n.name));
    let Some(other) = other else {
        eprintln!("aucune carte réseau configurée : test sans objet");
        return;
    };
    let seed = seeder("ailleurs.iso", &[("", 300 * 1024)], None).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path(), vec![seed.addr]).await;
    engine
        .set_torrent_interface(Some(other.device.clone()))
        .await
        .unwrap();
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_state(&mut events, &id, TaskState::Active).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let task = engine.get(&id).await.unwrap();
    assert_eq!(task.state, TaskState::Active, "interface {}", other.device);
    assert_eq!(task.done_bytes, 0, "interface {}", other.device);
    assert!(
        !dir.path().join("ailleurs.iso").exists(),
        "interface {}",
        other.device
    );
    crate::testing::stop(&engine).await;
}
