//! Tests du module torrent (SPEC §7.16) : un essaim local, sans Internet. Un pair librqbit
//! partage des fichiers depuis un dossier temporaire ; le moteur les télécharge par ce seul
//! pair, sans DHT ni connexion entrante.

use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use librqbit::limits::LimitsConfig;
use librqbit::{AddTorrent, ListenerMode, ListenerOptions, Session, SessionOptions};
use sha1::{Digest, Sha1};
use tokio::sync::broadcast;
use tokio::time::timeout;

use super::{Layout, Options, inspect_metainfo};
use crate::io::part_path;
use crate::net::Client;
use crate::net::filename::{NameRules, sanitize};
use crate::scheduler::{AddTask, Config, DISK_RECHECK, Engine, Event, Planning};
use crate::store::tasks::{TaskKind, TaskState};
use crate::testing::content_range;
use crate::transfer::Options as TransferOptions;

const WAIT: Duration = Duration::from_secs(60);

/// Un pair qui partage.
struct Seeder {
    _session: Arc<Session>,
    addr: SocketAddr,
    metainfo: Vec<u8>,
    _dir: tempfile::TempDir,
}

/// Contenu d'un fichier partagé : différent d'un fichier à l'autre.
fn data(index: usize, len: usize) -> Vec<u8> {
    content_range(index as u64 * 7919, len)
}

/// Taille des morceaux des torrents de test.
const PIECE: usize = 16 * 1024;

/// Un `.torrent` (bencode), empreintes SHA-1 de chaque morceau comprises. Écrit ici plutôt
/// que par `librqbit::create_torrent`, qui oublie l'empreinte du dernier morceau quand le
/// dernier fichier est vide (constaté le 2026-11-09, librqbit 9.1.0).
fn make_torrent(name: &str, files: &[(&str, Vec<u8>)], single: bool) -> Vec<u8> {
    let bstr = |s: &[u8]| {
        let mut out = format!("{}:", s.len()).into_bytes();
        out.extend_from_slice(s);
        out
    };
    let all: Vec<u8> = files.iter().flat_map(|(_, d)| d.iter().copied()).collect();
    let pieces: Vec<u8> = all
        .chunks(PIECE)
        .flat_map(|c| Sha1::digest(c).to_vec())
        .collect();
    // Clés d'un dictionnaire bencode dans l'ordre : files/length, name, piece length,
    // pieces ; length, path.
    let mut info = b"d".to_vec();
    if single {
        info.extend(format!("6:lengthi{}e", all.len()).into_bytes());
    } else {
        info.extend_from_slice(b"5:filesl");
        for (path, d) in files {
            info.extend(format!("d6:lengthi{}e4:pathl", d.len()).into_bytes());
            for c in path.split('/') {
                info.extend(bstr(c.as_bytes()));
            }
            info.extend_from_slice(b"ee");
        }
        info.push(b'e');
    }
    info.extend_from_slice(b"4:name");
    info.extend(bstr(name.as_bytes()));
    info.extend(format!("12:piece lengthi{PIECE}e6:pieces").into_bytes());
    info.extend(bstr(&pieces));
    info.push(b'e');
    let mut out = b"d4:info".to_vec();
    out.extend(info);
    out.push(b'e');
    out
}

/// Un pair qui partage `files` : un seul fichier si son chemin est vide (`name` est alors
/// son nom), sinon un dossier `name` qui les contient. `upload_bps` le ralentit.
async fn seeder(name: &str, files: &[(&str, usize)], upload_bps: Option<u32>) -> Seeder {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("seed");
    let root = base.join(name);
    let single = matches!(files, [("", _)]);
    let contents: Vec<(&str, Vec<u8>)> = files
        .iter()
        .enumerate()
        .map(|(i, (rel, len))| (*rel, data(i, *len)))
        .collect();
    for (rel, d) in &contents {
        let path = if single { root.clone() } else { root.join(rel) };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, d).unwrap();
    }
    let metainfo = make_torrent(name, &contents, single);
    let session = Session::new_with_opts(
        base,
        SessionOptions {
            dht: None,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpOnly,
                listen_addr: (Ipv4Addr::LOCALHOST, 0).into(),
                ..Default::default()
            }),
            disable_local_service_discovery: true,
            ratelimits: LimitsConfig {
                upload_bps: upload_bps.and_then(NonZeroU32::new),
                download_bps: None,
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Ses fichiers sont déjà là : il les vérifie, puis les partage.
    session
        .add_torrent(
            AddTorrent::from_bytes(metainfo.clone()),
            Some(librqbit::AddTorrentOptions {
                overwrite: true,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let addr = session.listen_addr().expect("le pair écoute");
    Seeder {
        _session: session,
        addr,
        metainfo,
        _dir: dir,
    }
}

fn config(dir: &Path, peers: Vec<SocketAddr>) -> Config {
    Config {
        db_path: dir.join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: TransferOptions {
            torrent: Options::local(peers),
            ..TransferOptions::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    }
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
    let engine = Engine::start(config(dir.path(), vec![seed.addr])).unwrap();
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
    engine.shutdown().await;
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
    let engine = Engine::start(config(dir.path(), vec![seed.addr])).unwrap();
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
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_magnet_link_gets_its_metadata_from_the_peers() {
    let seed = seeder("magnet.bin", &[("", 100 * 1024)], None).await;
    let magnet = inspect_metainfo(&seed.metainfo).unwrap().magnet;
    assert!(magnet.starts_with("magnet:?xt=urn:btih:"), "{magnet}");
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(config(dir.path(), vec![seed.addr])).unwrap();
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
    engine.shutdown().await;
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
    let engine = Engine::start(config(dir.path(), vec![seed.addr])).unwrap();
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
    engine.shutdown().await;
}

/// Un torrent mis en pause, dont le `.part` est abîmé pendant la pause (écritures perdues,
/// SPEC §7.16) : la reprise revérifie tout, retélécharge ce qui manque, et le fichier final
/// est juste.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_damaged_part_is_found_and_fetched_again_on_resume() {
    const LEN: usize = 1024 * 1024;
    let seed = seeder("lent.bin", &[("", LEN)], Some(256 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(config(dir.path(), vec![seed.addr])).unwrap();
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
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupted_torrent_resumes_after_an_engine_restart() {
    const LEN: usize = 1024 * 1024;
    let files = [("un.bin", LEN / 2), ("deux.bin", LEN / 2)];
    let seed = seeder("Reprise", &files, Some(256 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(config(dir.path(), vec![seed.addr])).unwrap();
    let mut events = engine.subscribe();
    let id = engine.add(add(dir.path(), &seed.metainfo)).await.unwrap();
    wait_progress(&mut events, &id, LEN as u64 / 4).await;
    engine.shutdown().await;

    let engine = Engine::start(config(dir.path(), vec![seed.addr])).unwrap();
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
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_with_delete_files_leaves_nothing_behind() {
    const LEN: usize = 512 * 1024;
    let files = [("un.bin", LEN), ("deux/trois.bin", LEN)];
    let seed = seeder("Annule", &files, Some(128 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(config(dir.path(), vec![seed.addr])).unwrap();
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
    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_torrent_twice_is_refused() {
    let seed = seeder("double.bin", &[("", 64 * 1024)], Some(16 * 1024)).await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(config(dir.path(), vec![seed.addr])).unwrap();
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
    engine.shutdown().await;
}

#[tokio::test]
async fn http_only_options_and_bad_torrents_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(config(dir.path(), Vec::new())).unwrap();
    let mut task = add(dir.path(), b"pas un torrent");
    assert!(engine.add(task.clone()).await.is_err());
    task.torrent = None;
    task.url = "magnet:?dn=sans-empreinte".into();
    assert!(engine.add(task.clone()).await.is_err());
    task.url = "magnet:?xt=urn:btih:7acf8fb590b2060dd9c3146ef770169d593433b0".into();
    task.max_connections = Some(4);
    assert!(engine.add(task).await.is_err());
    engine.shutdown().await;
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
