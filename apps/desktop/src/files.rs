//! Fichiers téléchargés ouverts ou montrés à la demande de l'utilisateur. Un exécutable
//! n'est jamais ouvert depuis l'application, seulement montré dans son dossier (SPEC §8.1).

use std::path::{Path, PathBuf};

/// Extensions de programmes, de scripts, d'installateurs et de raccourcis.
pub const EXECUTABLE: &[&str] = &[
    "appimage",
    "appx",
    "appxbundle",
    "bat",
    "cmd",
    "com",
    "command",
    "cpl",
    "deb",
    "desktop",
    "exe",
    "hta",
    "jar",
    "js",
    "jse",
    "lnk",
    "msi",
    "msix",
    "msixbundle",
    "msp",
    "pif",
    "ps1",
    "psm1",
    "reg",
    "rpm",
    "run",
    "scr",
    "sh",
    "url",
    "vbe",
    "vbs",
    "wsf",
    "wsh",
];

/// Le nom désigne un programme ou un script.
pub fn is_executable(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXECUTABLE.contains(&e.to_ascii_lowercase().as_str()))
}

/// Chemin d'un fichier d'une tâche : `name` est un simple nom de fichier, sans dossier,
/// et le fichier existe.
pub fn task_file(dir: &str, name: &str) -> Result<PathBuf, String> {
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && Path::new(name).file_name() == Some(name.as_ref());
    if !plain {
        return Err("nom de fichier invalide".into());
    }
    let dir = Path::new(dir);
    if !dir.is_absolute() {
        return Err("dossier invalide".into());
    }
    let path = dir.join(name);
    if !path.is_file() {
        return Err("fichier introuvable".into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_and_scripts_are_recognised_whatever_the_case() {
        for name in [
            "setup.EXE",
            "a.msi",
            "run.sh",
            "x.AppImage",
            "f.ps1",
            "lien.lnk",
        ] {
            assert!(is_executable(name), "{name}");
        }
        for name in [
            "film.mkv",
            "image.iso",
            "archive.zip",
            "notes.txt",
            "sans-extension",
            "exe",
        ] {
            assert!(!is_executable(name), "{name}");
        }
    }

    #[test]
    fn only_a_plain_existing_name_in_an_absolute_folder_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("sous")).unwrap();
        let d = dir.path().to_str().unwrap();
        assert_eq!(task_file(d, "a.txt").unwrap(), dir.path().join("a.txt"));
        for name in [
            "",
            ".",
            "..",
            "../a.txt",
            "sous/a.txt",
            "sous\\a.txt",
            "absent.txt",
            "sous",
        ] {
            assert!(task_file(d, name).is_err(), "{name}");
        }
        assert!(task_file("relatif", "a.txt").is_err());
    }
}
