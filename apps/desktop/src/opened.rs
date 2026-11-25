//! Liens magnet, fichiers `.torrent` et fichiers Metalink (`.meta4`, `.metalink`, phase 6)
//! reçus de l'extérieur (SPEC §4.3, étape 5b) : arguments
//! de la ligne de commande (« Ouvrir avec… », association), relance de l'application,
//! glisser-déposer sur la fenêtre. Rien n'est ajouté d'office : la page préremplit la boîte
//! « Ajouter », que l'utilisateur confirme.

use std::path::{Path, PathBuf};

use auro_engine::io::{BoundedReadError, read_bounded};
use base64::Engine as _;
use serde::Serialize;

/// Fichiers que la page peut faire lire (`read_torrent`, `read_plugin`, `read_list`) :
/// seulement ceux choisis par l'utilisateur dans un sélecteur ou reçus du système (audit
/// C14). Une page détournée ne fait lire aucun autre fichier.
#[derive(Default)]
pub struct Granted(std::sync::Mutex<std::collections::HashSet<PathBuf>>);

impl Granted {
    pub fn grant(&self, path: impl Into<PathBuf>) {
        self.0.lock().unwrap().insert(path.into());
    }

    /// Accorde les fichiers de ce qui a été ouvert avec l'application.
    pub fn grant_opened(&self, opened: &[Opened]) {
        for o in opened {
            if let Opened::Torrent { path } | Opened::Metalink { path } = o {
                self.grant(path);
            }
        }
    }

    /// Le chemin a été accordé, tel quel.
    pub fn check(&self, path: &str) -> Result<(), String> {
        if self.0.lock().unwrap().contains(Path::new(path)) {
            Ok(())
        } else {
            Err("fichier non choisi par l'utilisateur : refusé".into())
        }
    }
}

/// Taille maximale d'un fichier `.torrent` lu pour le moteur : celle du moteur ; recopiée
/// dans l'extension et l'application Android, comparées par un test (audit C29).
pub const MAX_TORRENT_FILE: u64 = auro_engine::torrent::MAX_TORRENT_FILE as u64;
/// Taille maximale d'un fichier Metalink : celle du moteur (SPEC §7.7).
pub const MAX_METALINK_FILE: u64 = auro_engine::metalink::MAX_LEN as u64;

/// Ce qui a été ouvert avec l'application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Opened {
    /// Lien magnet, tel quel.
    Magnet { link: String },
    /// Fichier `.torrent`, par son chemin absolu.
    Torrent { path: String },
    /// Fichier Metalink, par son chemin absolu.
    Metalink { path: String },
    /// Adresse HTTP(S) d'un fichier, copiée puis choisie dans une notification (F-30).
    Link { link: String },
}

/// Un lien magnet : `magnet:?…`, sans caractère de contrôle, de taille raisonnable.
pub fn is_magnet(text: &str) -> bool {
    text.len() <= 64 * 1024
        && text
            .get(..8)
            .is_some_and(|start| start.eq_ignore_ascii_case("magnet:?"))
        && !text.chars().any(char::is_control)
}

/// Un nom de fichier `.torrent`.
pub fn is_torrent_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
}

/// Un nom de fichier Metalink : `.meta4` ou `.metalink`.
pub fn is_metalink_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("meta4") || e.eq_ignore_ascii_case("metalink"))
}

/// Un fichier `.torrent` ou Metalink existant, par son chemin absolu.
fn opened_file(path: &Path) -> Option<Opened> {
    if !path.is_absolute() || !path.is_file() {
        return None;
    }
    let text = path.to_string_lossy().into_owned();
    if is_torrent_path(path) {
        Some(Opened::Torrent { path: text })
    } else if is_metalink_path(path) {
        Some(Opened::Metalink { path: text })
    } else {
        None
    }
}

/// Liens magnet, fichiers `.torrent` et Metalink existants parmi des arguments ; un chemin relatif se
/// lit depuis `cwd`. Les options (`--endpoint`…) et le reste sont ignorés.
pub fn from_args<I: IntoIterator<Item = String>>(args: I, cwd: Option<&Path>) -> Vec<Opened> {
    let mut found = Vec::new();
    let mut skip_value = false;
    for arg in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if matches!(arg.as_str(), "--endpoint" | "--db" | "--prefs") {
            skip_value = true;
            continue;
        }
        if is_magnet(&arg) {
            found.push(Opened::Magnet { link: arg });
            continue;
        }
        let path = PathBuf::from(&arg);
        if arg.starts_with('-') || !(is_torrent_path(&path) || is_metalink_path(&path)) {
            continue;
        }
        let path = match (path.is_absolute(), cwd) {
            (true, _) => path,
            (false, Some(cwd)) => cwd.join(path),
            (false, None) => continue,
        };
        found.extend(opened_file(&path));
    }
    found
}

/// Fichiers `.torrent` et Metalink d'un glisser-déposer.
pub fn from_paths(paths: &[PathBuf]) -> Vec<Opened> {
    paths.iter().filter_map(|p| opened_file(p)).collect()
}

/// Un fichier `.torrent` ou Metalink lu pour le moteur.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentFile {
    /// Nom du fichier, pour l'affichage.
    pub name: String,
    /// Contenu en base64, tel que `task.add` ou `metalink.read` l'attend (SPEC §6.5).
    pub data: String,
}

/// Lit un fichier `.torrent` (16 Mio au plus) ou Metalink (4 Mio au plus), par son chemin
/// absolu.
pub fn read_torrent(path: &str) -> Result<TorrentFile, String> {
    let path = Path::new(path);
    let limit = match (is_torrent_path(path), is_metalink_path(path)) {
        (true, _) => MAX_TORRENT_FILE,
        (_, true) => MAX_METALINK_FILE,
        _ => return Err("fichier .torrent ou Metalink attendu".into()),
    };
    let (name, bytes) = read_chosen(path, limit, "fichier trop gros")?;
    Ok(TorrentFile {
        name,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Fichier choisi par l'utilisateur, lu pour le moteur : chemin absolu, `limit` octets au
/// plus (`io::read_bounded`, audit C31), refusé au-delà par `too_large` ; rend son nom,
/// pour l'affichage, et son contenu.
fn read_chosen(path: &Path, limit: u64, too_large: &str) -> Result<(String, Vec<u8>), String> {
    if !path.is_absolute() {
        return Err("chemin absolu attendu".into());
    }
    let bytes = read_bounded(path, limit).map_err(|e| match e {
        BoundedReadError::TooLarge { .. } => format!("{} : {too_large}", path.display()),
        BoundedReadError::Io(e) => format!("{} : {e}", path.display()),
    })?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok((name, bytes))
}

/// Taille maximale d'un fichier de plugin : celle du moteur (SPEC §7.20, audit C29).
pub const MAX_PLUGIN_FILE: u64 = auro_engine::plugins::manifest::MAX_FILE as u64;

/// Lit un fichier de plugin `.wasm` choisi par l'utilisateur (8 Mio au plus), par son chemin
/// absolu, pour `plugin.inspect` et `plugin.install` (base64). Rien n'en est exécuté ici.
pub fn read_plugin(path: &str) -> Result<TorrentFile, String> {
    let path = Path::new(path);
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wasm"))
    {
        return Err("fichier de plugin .wasm attendu".into());
    }
    let (name, bytes) = read_chosen(path, MAX_PLUGIN_FILE, "fichier trop gros")?;
    Ok(TorrentFile {
        name,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Taille maximale d'une liste de liens importée : celle du moteur (SPEC §7.18, audit C29).
pub const MAX_LIST_FILE: u64 = auro_engine::linkscan::MAX_INPUT as u64;

/// Une liste de liens importée (F-31), lue comme un texte.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFile {
    /// Nom du fichier, pour l'affichage.
    pub name: String,
    /// Contenu, en UTF-8 (un octet illisible devient « � »).
    pub text: String,
}

/// Lit une liste de liens choisie par l'utilisateur : chemin absolu, 4 Mio au plus, quelle
/// que soit son extension (texte, CSV, export d'un autre gestionnaire).
pub fn read_list(path: &str) -> Result<ListFile, String> {
    let (name, bytes) = read_chosen(Path::new(path), MAX_LIST_FILE, "liste trop grosse")?;
    Ok(ListFile {
        name,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    /// Audit C14 : seuls les fichiers choisis ou reçus peuvent être lus pour la page.
    #[test]
    fn only_granted_files_may_be_read() {
        let granted = super::Granted::default();
        assert!(granted.check("/etc/passwd").is_err());
        assert!(granted.check(r"C:\Users\moi\secret.txt").is_err());
        granted.grant("/home/moi/liste.txt");
        assert!(granted.check("/home/moi/liste.txt").is_ok());
        assert!(granted.check("/home/moi/autre.txt").is_err());
        granted.grant_opened(&[
            super::Opened::Torrent {
                path: "/tmp/a.torrent".into(),
            },
            super::Opened::Magnet {
                link: "magnet:?xt=urn:btih:00".into(),
            },
        ]);
        assert!(granted.check("/tmp/a.torrent").is_ok());
        assert!(granted.check("magnet:?xt=urn:btih:00").is_err());
    }

    #[test]
    fn a_list_is_read_as_text_within_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("liens.csv");
        std::fs::write(&path, b"url;nom\nhttps://a.example/f.iso;f\n\xff").unwrap();
        let list = read_list(path.to_str().unwrap()).unwrap();
        assert_eq!(list.name, "liens.csv");
        assert!(
            list.text
                .starts_with("url;nom\nhttps://a.example/f.iso;f\n")
        );
        assert!(list.text.ends_with('\u{FFFD}'));
        let big = dir.path().join("gros.txt");
        std::fs::write(&big, vec![b'a'; MAX_LIST_FILE as usize + 1]).unwrap();
        assert!(read_list(big.to_str().unwrap()).is_err());
        assert!(read_list("liens.csv").is_err());
        assert!(read_list(dir.path().join("absent.txt").to_str().unwrap()).is_err());
    }

    use super::*;

    #[test]
    fn a_plugin_file_is_read_in_base64_within_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Hebergeur.WASM");
        std::fs::write(&path, b"\0asm\x01\0\0\0").unwrap();
        let file = read_plugin(path.to_str().unwrap()).unwrap();
        assert_eq!(file.name, "Hebergeur.WASM");
        assert_eq!(file.data, "AGFzbQEAAAA=");
        // Pas un .wasm, relatif, absent, trop gros : refusé.
        std::fs::write(dir.path().join("p.txt"), b"x").unwrap();
        assert!(read_plugin(dir.path().join("p.txt").to_str().unwrap()).is_err());
        assert!(read_plugin("p.wasm").is_err());
        assert!(read_plugin(dir.path().join("absent.wasm").to_str().unwrap()).is_err());
        let big = dir.path().join("gros.wasm");
        std::fs::write(&big, vec![0u8; MAX_PLUGIN_FILE as usize + 1]).unwrap();
        assert!(read_plugin(big.to_str().unwrap()).is_err());
    }

    #[test]
    fn magnets_and_existing_torrent_files_are_taken_from_the_arguments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.torrent"), b"d4:infod4:name1:aee").unwrap();
        std::fs::write(dir.path().join("lot.meta4"), b"<metalink/>").unwrap();
        std::fs::write(dir.path().join("b.TORRENT"), b"x").unwrap();
        let abs = dir.path().join("b.TORRENT").to_string_lossy().into_owned();
        let args = [
            "--endpoint",
            "magnet:?ignoré-car-valeur-d-option",
            "magnet:?xt=urn:btih:abc",
            "a.torrent",
            &abs,
            "absent.torrent",
            "--hidden",
            "image.iso",
            "MAGNET:?xt=urn:btih:def",
            "magnet:",
            "lot.meta4",
        ]
        .map(String::from);
        let found = from_args(args, Some(dir.path()));
        assert_eq!(
            found,
            vec![
                Opened::Magnet {
                    link: "magnet:?xt=urn:btih:abc".into()
                },
                Opened::Torrent {
                    path: dir.path().join("a.torrent").to_string_lossy().into_owned()
                },
                Opened::Torrent { path: abs },
                Opened::Magnet {
                    link: "MAGNET:?xt=urn:btih:def".into()
                },
                Opened::Metalink {
                    path: dir.path().join("lot.meta4").to_string_lossy().into_owned()
                },
            ]
        );
        // Sans dossier courant, un chemin relatif ne désigne rien.
        assert!(from_args(["a.torrent".to_owned()], None).is_empty());
        assert!(!is_magnet("magnet:?a\nb"));
    }

    #[test]
    fn the_torrent_limit_of_the_extension_and_android_is_this_one() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mib = MAX_TORRENT_FILE >> 20;
        assert_eq!(MAX_TORRENT_FILE, mib << 20);
        let decide = std::fs::read_to_string(root.join("../../extension/decide.js")).unwrap();
        assert!(
            decide.contains(&format!("const MAX_TORRENT_BYTES = {mib} * 1024 * 1024;")),
            "limite des .torrent de l'extension différente"
        );
        let app = std::fs::read_to_string(
            root.join("../android/app/src/main/java/org/aurodownloader/AuroApp.kt"),
        )
        .unwrap();
        assert!(
            app.contains(&format!("const val MAX_TORRENT_FILE = {mib} * 1024 * 1024")),
            "limite des .torrent de l'application Android différente"
        );
    }

    #[test]
    fn a_torrent_file_is_read_in_base64_within_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Debian.torrent");
        std::fs::write(&path, b"d4:infoe").unwrap();
        let file = read_torrent(path.to_str().unwrap()).unwrap();
        assert_eq!(file.name, "Debian.torrent");
        assert_eq!(file.data, "ZDQ6aW5mb2U=");
        // Pas un .torrent, relatif, absent : refusé.
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        assert!(read_torrent(dir.path().join("notes.txt").to_str().unwrap()).is_err());
        assert!(read_torrent("Debian.torrent").is_err());
        assert!(read_torrent(dir.path().join("x.torrent").to_str().unwrap()).is_err());
        // Un Metalink : 4 Mio au plus.
        let meta = dir.path().join("lot.METALINK");
        std::fs::write(&meta, b"<metalink/>").unwrap();
        assert_eq!(
            read_torrent(meta.to_str().unwrap()).unwrap().name,
            "lot.METALINK"
        );
        let big = dir.path().join("gros.meta4");
        std::fs::write(&big, vec![b' '; MAX_METALINK_FILE as usize + 1]).unwrap();
        assert!(read_torrent(big.to_str().unwrap()).is_err());
        // Glisser-déposer : seulement les .torrent et Metalink existants.
        let dropped = from_paths(&[path.clone(), dir.path().join("notes.txt"), meta.clone()]);
        assert_eq!(
            dropped,
            [
                Opened::Torrent {
                    path: path.to_string_lossy().into_owned()
                },
                Opened::Metalink {
                    path: meta.to_string_lossy().into_owned()
                },
            ]
        );
    }
}
