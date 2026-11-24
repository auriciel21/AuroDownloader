//! Fichiers téléchargés ouverts ou montrés à la demande de l'utilisateur. Un exécutable
//! n'est jamais ouvert depuis l'application, seulement montré dans son dossier (SPEC §8.1).

use std::path::{Path, PathBuf};

/// Extensions de programmes, de scripts, d'installateurs, de raccourcis, et d'images de
/// disque, que Windows monte à l'ouverture et qui peuvent porter des programmes (audit C5,
/// d'après les listes de blocage d'Outlook et de Windows).
pub const EXECUTABLE: &[&str] = &[
    "appimage",
    "application",
    "appref-ms",
    "appx",
    "appxbundle",
    "bat",
    "chm",
    "cmd",
    "com",
    "command",
    "cpl",
    "deb",
    "desktop",
    "diagcab",
    "exe",
    "gadget",
    "hta",
    "img",
    "inf",
    "ins",
    "iso",
    "isp",
    "jar",
    "jnlp",
    "js",
    "jse",
    "library-ms",
    "lnk",
    "msc",
    "msh",
    "msh1",
    "msh2",
    "mshxml",
    "msi",
    "msix",
    "msixbundle",
    "msp",
    "pif",
    "ps1",
    "ps1xml",
    "ps2",
    "ps2xml",
    "psc1",
    "psc2",
    "psd1",
    "psm1",
    "py",
    "pyc",
    "pyo",
    "pyw",
    "pyz",
    "pyzw",
    "reg",
    "rpm",
    "run",
    "scf",
    "scr",
    "sct",
    "search-ms",
    "searchconnector-ms",
    "settingcontent-ms",
    "sh",
    "shb",
    "shs",
    "url",
    "vb",
    "vbe",
    "vbs",
    "vhd",
    "vhdx",
    "website",
    "ws",
    "wsc",
    "wsf",
    "wsh",
    "xbap",
    "xll",
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
            // Audit C5.
            "outil.py",
            "aide.chm",
            "console.msc",
            "image.ISO",
            "disque.vhdx",
            "app.application",
        ] {
            assert!(is_executable(name), "{name}");
        }
        for name in [
            "film.mkv",
            "photo.png",
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
