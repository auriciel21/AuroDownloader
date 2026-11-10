//! Lecture des fichiers `.torrent` et des liens magnet face à des entrées quelconques
//! (SPEC §8.1 et §9.1 : fuzzing des lecteurs de fichiers `.torrent`, porte de la phase 5).
//! Tests de propriétés : jamais de panique ; un torrent accepté n'écrit jamais hors de son
//! dossier, et chacun de ses noms est un seul composant déjà nettoyé.

use std::path::{Component, Path};

use proptest::prelude::*;

use super::{Layout, inspect_magnet, inspect_metainfo};
use crate::net::filename::{NameRules, sanitize};

/// Valeur bencode, pour fabriquer des `.torrent` (clés de dictionnaire triées à l'écriture).
#[derive(Debug, Clone)]
enum Bencode {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Bencode>),
    Dict(Vec<(Vec<u8>, Bencode)>),
}

impl Bencode {
    fn text(s: &str) -> Bencode {
        Bencode::Bytes(s.as_bytes().to_vec())
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Bencode::Int(i) => out.extend(format!("i{i}e").into_bytes()),
            Bencode::Bytes(b) => {
                out.extend(format!("{}:", b.len()).into_bytes());
                out.extend_from_slice(b);
            }
            Bencode::List(items) => {
                out.push(b'l');
                items.iter().for_each(|i| i.write(out));
                out.push(b'e');
            }
            Bencode::Dict(entries) => {
                let mut sorted = entries.clone();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                out.push(b'd');
                for (k, v) in &sorted {
                    Bencode::Bytes(k.clone()).write(out);
                    v.write(out);
                }
                out.push(b'e');
            }
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }
}

const PIECE: u64 = 16 * 1024;

/// Un nom venu d'un torrent : quelconque, ou parmi ceux qui ont déjà piégé des clients
/// (remontée de dossier, chemin absolu, noms réservés de Windows, caractères de contrôle).
fn hostile_name() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        "[a-zA-Z0-9 ._-]{0,12}",
        Just(String::new()),
        Just(".".to_owned()),
        Just("..".to_owned()),
        Just("../../etc/passwd".to_owned()),
        Just("/etc/passwd".to_owned()),
        Just("C:\\Windows\\System32".to_owned()),
        Just("\\\\serveur\\partage".to_owned()),
        Just("CON".to_owned()),
        Just("nul.txt".to_owned()),
        Just("a\u{0}b".to_owned()),
        Just("fin. ".to_owned()),
        Just("\u{202e}gpj.exe".to_owned()),
        Just("x".repeat(400)),
    ]
}

/// Un fichier d'un torrent : composants du chemin, taille, remplissage (BEP 47).
#[derive(Debug, Clone)]
struct FileSpec {
    path: Vec<String>,
    length: u64,
    padding: bool,
}

fn file_spec() -> impl Strategy<Value = FileSpec> {
    (
        prop::collection::vec(hostile_name(), 0..4),
        0u64..200_000,
        prop::bool::weighted(0.1),
    )
        .prop_map(|(path, length, padding)| FileSpec {
            path,
            length,
            padding,
        })
}

/// Un `.torrent` fabriqué : nom, fichiers (un seul, ou une liste), et, parfois, un nombre
/// d'empreintes de morceaux faux.
#[derive(Debug, Clone)]
struct TorrentSpec {
    name: String,
    multi: bool,
    files: Vec<FileSpec>,
    wrong_pieces: bool,
    announce: Option<String>,
}

fn torrent_spec() -> impl Strategy<Value = TorrentSpec> {
    (
        hostile_name(),
        any::<bool>(),
        prop::collection::vec(file_spec(), 1..6),
        prop::bool::weighted(0.1),
        prop::option::of(any::<String>()),
    )
        .prop_map(|(name, multi, files, wrong_pieces, announce)| TorrentSpec {
            name,
            multi,
            files,
            wrong_pieces,
            announce,
        })
}

fn metainfo(spec: &TorrentSpec) -> Vec<u8> {
    let total: u64 = if spec.multi {
        spec.files.iter().map(|f| f.length).sum()
    } else {
        spec.files[0].length
    };
    let mut pieces = total.div_ceil(PIECE);
    if spec.wrong_pieces {
        pieces += 1;
    }
    let mut info = vec![
        (b"name".to_vec(), Bencode::text(&spec.name)),
        (b"piece length".to_vec(), Bencode::Int(PIECE as i64)),
        (
            b"pieces".to_vec(),
            Bencode::Bytes((0..pieces * 20).map(|i| (i % 251) as u8).collect()),
        ),
    ];
    if spec.multi {
        let files = spec
            .files
            .iter()
            .map(|f| {
                let mut entry = vec![
                    (b"length".to_vec(), Bencode::Int(f.length as i64)),
                    (
                        b"path".to_vec(),
                        Bencode::List(f.path.iter().map(|c| Bencode::text(c)).collect()),
                    ),
                ];
                if f.padding {
                    entry.push((b"attr".to_vec(), Bencode::text("p")));
                }
                Bencode::Dict(entry)
            })
            .collect();
        info.push((b"files".to_vec(), Bencode::List(files)));
    } else {
        info.push((b"length".to_vec(), Bencode::Int(total as i64)));
    }
    let mut top = vec![(b"info".to_vec(), Bencode::Dict(info))];
    if let Some(a) = &spec.announce {
        top.push((b"announce".to_vec(), Bencode::text(a)));
    }
    Bencode::Dict(top).encode()
}

/// Un nom est sûr : un seul composant normal, déjà nettoyé (SPEC §8.1).
fn single_safe_component(name: &str) -> Result<(), TestCaseError> {
    prop_assert!(!name.is_empty());
    prop_assert!(!name.contains(['/', '\\']), "{name:?}");
    prop_assert_eq!(sanitize(name, NameRules::native()), name.to_owned());
    let mut parts = Path::new(name).components();
    prop_assert!(
        matches!(parts.next(), Some(Component::Normal(_))),
        "{name:?}"
    );
    prop_assert!(parts.next().is_none(), "{name:?}");
    Ok(())
}

/// Un torrent accepté reste dans son dossier : noms sûrs, chemins finals sous
/// `dest/<nom>`, sans composant autre que normal.
fn stays_inside(layout: &Layout) -> Result<(), TestCaseError> {
    single_safe_component(&layout.top)?;
    for file in &layout.files {
        for c in &file.components {
            single_safe_component(c)?;
        }
    }
    let dest = Path::new("dossier-essai");
    let root = dest.join(&layout.top);
    let finals: Vec<_> = layout
        .final_paths(dest, &layout.top)
        .into_iter()
        .flatten()
        .collect();
    // Deux fichiers du torrent n'écrivent jamais au même endroit, même quand le nettoyage
    // rend deux noms différents identiques (`a\0b` et `a_b`), ni l'un dans l'autre.
    for (i, a) in finals.iter().enumerate() {
        for b in &finals[i + 1..] {
            prop_assert!(!a.starts_with(b) && !b.starts_with(a), "{a:?} et {b:?}");
        }
    }
    for path in finals {
        let inside = path.strip_prefix(&root);
        prop_assert!(inside.is_ok(), "{path:?}");
        prop_assert!(
            inside
                .unwrap()
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
            "{path:?}"
        );
    }
    for file in layout.task_files(&layout.top) {
        prop_assert!(
            file.path
                .split('/')
                .all(|c| c != ".." && c != "." && !c.is_empty()),
            "{:?}",
            file.path
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Des octets quelconques : refusés ou acceptés, jamais de panique.
    #[test]
    fn any_bytes_are_read_without_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(layout) = Layout::parse(&bytes) {
            stays_inside(&layout)?;
        }
        let _ = inspect_metainfo(&bytes);
    }

    /// Des `.torrent` fabriqués avec des noms hostiles : un torrent accepté reste dans son
    /// dossier, et son nom est celui que le moteur annonce.
    #[test]
    fn hostile_names_never_leave_the_folder(spec in torrent_spec()) {
        let bytes = metainfo(&spec);
        match Layout::parse(&bytes) {
            Ok(layout) => {
                prop_assert!(!spec.wrong_pieces, "nombre d'empreintes faux accepté");
                stays_inside(&layout)?;
                let added = inspect_metainfo(&bytes).expect("lu deux fois de même");
                prop_assert_eq!(added.name, layout.top.clone());
            }
            Err(e) => prop_assert!(e.starts_with("fichier .torrent invalide"), "{e}"),
        }
    }

    /// Un `.torrent` valide abîmé (coupé, octets changés, ajoutés) : jamais de panique.
    #[test]
    fn damaged_torrents_are_read_without_panic(
        spec in torrent_spec(),
        cut in any::<prop::sample::Index>(),
        edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>(), any::<bool>()), 0..8),
    ) {
        let mut bytes = metainfo(&spec);
        for (at, byte, insert) in edits {
            let i = at.index(bytes.len() + 1);
            if insert || i == bytes.len() {
                bytes.insert(i, byte);
            } else {
                bytes[i] = byte;
            }
        }
        let keep = cut.index(bytes.len() + 1);
        for slice in [&bytes[..], &bytes[..keep]] {
            if let Ok(layout) = Layout::parse(slice) {
                stays_inside(&layout)?;
            }
        }
    }

    /// Un lien magnet quelconque : refusé ou accepté, jamais de panique ; accepté, son nom
    /// (`dn`) est un seul composant nettoyé.
    #[test]
    fn any_magnet_is_read_without_panic(text in any::<String>(), name in hostile_name()) {
        if let Ok(added) = inspect_magnet(&text) {
            single_safe_component(&added.name)?;
        }
        let encoded: String =
            percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC).collect();
        let link = format!("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn={encoded}");
        let added = inspect_magnet(&link).expect("lien magnet bien formé");
        single_safe_component(&added.name)?;
    }
}

/// Un torrent fabriqué à partir de noms de fichiers donnés.
fn torrent_of(name: &str, paths: &[&[&str]]) -> Vec<u8> {
    metainfo(&TorrentSpec {
        name: name.to_owned(),
        multi: true,
        files: paths
            .iter()
            .enumerate()
            .map(|(i, p)| FileSpec {
                path: p.iter().map(|c| (*c).to_owned()).collect(),
                length: 10 + i as u64 * 20_000,
                padding: false,
            })
            .collect(),
        wrong_pieces: false,
        announce: None,
    })
}

/// Première barrière, celle de librqbit : un composant `..`, un séparateur dans un nom, ou
/// un fichier sans nom font refuser le torrent entier.
#[test]
fn traversal_separators_and_empty_names_are_refused() {
    for paths in [
        &[&["..", "etc", "passwd"][..]][..],
        &[&["a", "..", "b"][..]],
        &[&["../../etc/passwd"][..]],
        &[&["/etc/passwd"][..]],
        &[&[r"C:\Windows", "x"][..]],
        &[&[r"\\serveur\partage"][..]],
        &[&[][..]],
        &[&[""][..]],
    ] {
        let refused = Layout::parse(&torrent_of("essai", paths));
        assert!(refused.is_err(), "{paths:?} accepté");
    }
}

/// Seconde barrière, celle du moteur : les noms hostiles qui passent la première sont
/// nettoyés (`sanitize`, SPEC §8.1), et le torrent reste dans son dossier.
#[test]
fn hostile_names_that_pass_are_cleaned() {
    let long = "x".repeat(400);
    let bytes = torrent_of(
        "nul.txt",
        &[
            &[".", "CON", "a\u{0}b"],
            &["fin. ", "\u{202e}gpj.exe"],
            &[&long, "aux.c"],
            &["COM1", ".hidden", "f\u{7}"],
        ],
    );
    let layout = Layout::parse(&bytes).expect("accepté par librqbit");
    assert_eq!(layout.files.len(), 4);
    stays_inside(&layout).unwrap();
    // Le premier fichier commence par `.` : jamais un composant « dossier courant ».
    assert!(layout.files[0].components.iter().all(|c| c != "."));
    if NameRules::native() == NameRules::Windows {
        assert_ne!(layout.top, "nul.txt");
        assert!(layout.files[0].components.iter().all(|c| c != "CON"));
    }
}

/// Deux fichiers au même chemin, d'origine ou une fois les noms nettoyés : le torrent est
/// refusé (décision du 2026-11-10). Des noms qui ne diffèrent que par la casse ne se
/// distinguent pas sous Windows ni dans le stockage partagé d'Android.
#[test]
fn files_at_the_same_path_are_refused() {
    let same = "deux fichiers au même chemin";
    for paths in [
        // Un fichier et un dossier de même nom.
        &[&["fin"][..], &["fin", "x"][..]][..],
        &[&["d", "e"][..], &["d"][..]],
        // Deux noms différents, identiques une fois nettoyés.
        &[&["a\u{0}b"][..], &["a_b"][..]],
        &[&["x\u{200b}y"][..], &["xy"][..]],
    ] {
        let refused = Layout::parse(&torrent_of("essai", paths)).expect_err("refusé");
        assert!(refused.contains(same), "{paths:?} : {refused}");
    }
    let windows_like = cfg!(any(windows, target_os = "android"));
    for paths in [
        &[&["CON"][..], &["_CON"][..]][..],
        &[&["fin. "][..], &["fin"][..]],
    ] {
        let read = Layout::parse(&torrent_of("essai", paths));
        assert_eq!(
            read.is_err(),
            NameRules::native() == NameRules::Windows,
            "{paths:?}"
        );
    }
    let case = Layout::parse(&torrent_of("essai", &[&["README"], &["readme"]]));
    assert_eq!(case.is_err(), windows_like);
    // Un fichier de remplissage n'est jamais écrit : il ne gêne pas.
    let mut spec = TorrentSpec {
        name: "essai".to_owned(),
        multi: true,
        files: vec![
            FileSpec {
                path: vec!["a".to_owned()],
                length: 10,
                padding: false,
            },
            FileSpec {
                path: vec!["a".to_owned(), "pad".to_owned()],
                length: 5,
                padding: true,
            },
        ],
        wrong_pieces: false,
        announce: None,
    };
    assert!(Layout::parse(&metainfo(&spec)).is_ok());
    spec.files[1].padding = false;
    assert!(Layout::parse(&metainfo(&spec)).is_err());
}

/// Noms bizarres mais permis par librqbit (sans séparateur, ni `..`, ni vide).
fn odd_name() -> impl Strategy<Value = String> {
    hostile_name().prop_filter("refusé par librqbit", |n| {
        !n.is_empty() && n != ".." && !n.contains(['/', '\\'])
    })
}

proptest! {
    /// Avec des noms permis par librqbit, presque tous les torrents fabriqués sont
    /// acceptés : les propriétés de noms portent bien sur des torrents lus.
    #[test]
    fn odd_torrents_are_accepted_and_stay_inside(
        name in odd_name(),
        paths in prop::collection::vec(prop::collection::vec(odd_name(), 1..4), 1..5),
    ) {
        // Mêmes noms bruts : refusés par librqbit (« duplicate filenames »).
        let distinct: std::collections::HashSet<_> = paths.iter().collect();
        prop_assume!(distinct.len() == paths.len());
        let refs: Vec<Vec<&str>> = paths.iter().map(|p| p.iter().map(String::as_str).collect()).collect();
        let slices: Vec<&[&str]> = refs.iter().map(Vec::as_slice).collect();
        match Layout::parse(&torrent_of(&name, &slices)) {
            Ok(layout) => stays_inside(&layout)?,
            // Seuls refus permis : deux fichiers au même chemin, vus par le moteur une fois
            // les noms nettoyés, ou par librqbit à sa manière.
            Err(e) => prop_assert!(e.contains("même chemin") || e.contains("duplicate filenames"), "{e}"),
        }
    }
}
