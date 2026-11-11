//! Liens magnet, fichiers `.torrent` et fichiers Metalink (`.meta4`, `.metalink`, phase 6)
//! reçus de l'extérieur (SPEC §4.3, étape 5b) : arguments
//! de la ligne de commande (« Ouvrir avec… », association), relance de l'application,
//! glisser-déposer sur la fenêtre. Rien n'est ajouté d'office : la page préremplit la boîte
//! « Ajouter », que l'utilisateur confirme.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;

/// Taille maximale d'un fichier `.torrent` lu pour le moteur : un garde-fou (le moteur a le
/// sien).
pub const MAX_TORRENT_FILE: u64 = 16 << 20;
/// Taille maximale d'un fichier Metalink (celle du moteur, SPEC §7.7).
pub const MAX_METALINK_FILE: u64 = 4 << 20;

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
    if !path.is_absolute() {
        return Err("chemin absolu attendu".into());
    }
    let len = std::fs::metadata(path)
        .map_err(|e| format!("{} : {e}", path.display()))?
        .len();
    if len > limit {
        return Err(format!("{} : fichier trop gros", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{} : {e}", path.display()))?;
    Ok(TorrentFile {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
