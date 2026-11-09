//! Essaim local pour les tests et les bancs des torrents (SPEC §7.16) : `.torrent` écrits à
//! la main, et un pair librqbit qui partage des fichiers depuis un dossier.
//!
//! Les `.torrent` ne viennent pas de `librqbit::create_torrent`, qui oublie l'empreinte du
//! dernier morceau quand le dernier fichier est vide (constaté le 2026-11-09, 9.1.0).

use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, ListenerMode, ListenerOptions, Session, SessionOptions,
};
use sha1::{Digest, Sha1};

/// Taille des morceaux des torrents de test.
pub const PIECE: usize = 16 * 1024;

/// Un `.torrent` (bencode), empreintes SHA-1 de chaque morceau comprises : un seul fichier
/// (`single`, nommé `name`), ou un dossier `name` qui contient `files` (chemins séparés par
/// `/`).
pub fn make_torrent(name: &str, files: &[(&str, Vec<u8>)], single: bool) -> Vec<u8> {
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

/// Un pair qui partage ses torrents en local, sans DHT ; `upload_bps` le ralentit.
pub struct Seeder {
    pub session: Arc<Session>,
    pub addr: SocketAddr,
    base: PathBuf,
}

impl Seeder {
    /// Un pair dont les fichiers vivent sous `base`.
    pub async fn start(base: &Path, upload_bps: Option<u32>) -> Seeder {
        std::fs::create_dir_all(base).unwrap();
        let session = Session::new_with_opts(
            base.to_path_buf(),
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
        let addr = session.listen_addr().expect("le pair écoute");
        Seeder {
            session,
            addr,
            base: base.to_path_buf(),
        }
    }

    /// Écrit `files` et les partage ; rend le `.torrent`. Un seul fichier si son chemin est
    /// vide (`name` est alors son nom), sinon un dossier `name` qui les contient.
    pub async fn share(&self, name: &str, files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let single = matches!(files, [("", _)]);
        let root = self.base.join(name);
        for (rel, d) in files {
            let path = if single { root.clone() } else { root.join(rel) };
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, d).unwrap();
        }
        let metainfo = make_torrent(name, files, single);
        // Ses fichiers sont déjà là : il les vérifie, puis les partage. Le dossier est donné :
        // sans lui, librqbit range sans son dossier un torrent « à plusieurs fichiers » qui
        // n'en contient qu'un, et le pair ne trouverait pas ses données.
        let folder = if single { &self.base } else { &root };
        self.session
            .add_torrent(
                AddTorrent::from_bytes(metainfo.clone()),
                Some(AddTorrentOptions {
                    overwrite: true,
                    output_folder: Some(folder.to_string_lossy().into_owned()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        metainfo
    }
}
