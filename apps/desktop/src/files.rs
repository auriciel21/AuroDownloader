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

/// Un simple nom, sans dossier : ni vide, ni `.`, ni `..`, ni séparateur, ni lecteur.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && Path::new(name).file_name() == Some(name.as_ref())
}

/// Chemin de ce qu'une tâche a écrit (fichier, ou dossier d'un torrent ou d'une copie de
/// site) : `name` est un simple nom, `dir` un chemin absolu, et le chemin existe.
pub fn task_entry(dir: &str, name: &str) -> Result<PathBuf, String> {
    if !plain_name(name) {
        return Err("nom de fichier invalide".into());
    }
    let dir = Path::new(dir);
    if !dir.is_absolute() {
        return Err("dossier invalide".into());
    }
    let path = dir.join(name);
    if !path.exists() {
        return Err("fichier introuvable".into());
    }
    Ok(path)
}

/// Chemin d'un fichier d'une tâche : `name` est un simple nom de fichier, sans dossier,
/// et le fichier existe.
pub fn task_file(dir: &str, name: &str) -> Result<PathBuf, String> {
    let path = task_entry(dir, name)?;
    if !path.is_file() {
        return Err("fichier introuvable".into());
    }
    Ok(path)
}

/// Page de départ d'une copie de site (SPEC §7.18 ter) : `start`, relatif au dossier `name`
/// de la tâche, parties séparées par `/`. Une page HTML qui existe dans le dossier de la
/// copie, sans en sortir (ni `..`, ni lien vers l'extérieur).
pub fn site_page(dir: &str, name: &str, start: &str) -> Result<PathBuf, String> {
    let root = task_entry(dir, name)?;
    if !root.is_dir() {
        return Err("dossier de la copie introuvable".into());
    }
    if !start.split('/').all(plain_name) {
        return Err("page de départ invalide".into());
    }
    let page = start.split('/').fold(root.clone(), |p, part| p.join(part));
    let html = page
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "html" | "htm"));
    if !html {
        return Err("la page de départ n'est pas une page HTML".into());
    }
    let inside = match (root.canonicalize(), page.canonicalize()) {
        (Ok(root), Ok(real)) => real.starts_with(root) && real.is_file(),
        _ => false,
    };
    if !inside {
        return Err("page de départ introuvable dans la copie".into());
    }
    Ok(page)
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
        // Le dossier d'une tâche se montre, il ne s'ouvre pas comme un fichier.
        assert_eq!(
            task_entry(dir.path().to_str().unwrap(), "sous").unwrap(),
            dir.path().join("sous")
        );
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

    #[test]
    fn the_start_page_of_a_site_copy_stays_inside_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("exemple.org");
        std::fs::create_dir_all(root.join("exemple.org/doc")).unwrap();
        std::fs::write(root.join("exemple.org/index.html"), b"<p>").unwrap();
        std::fs::write(root.join("exemple.org/doc/f.exe"), b"MZ").unwrap();
        std::fs::write(dir.path().join("dehors.html"), b"<p>").unwrap();
        let d = dir.path().to_str().unwrap();
        assert_eq!(
            site_page(d, "exemple.org", "exemple.org/index.html").unwrap(),
            root.join("exemple.org").join("index.html")
        );
        for start in [
            "../dehors.html",
            "exemple.org/../../dehors.html",
            "exemple.org/doc/f.exe",
            "exemple.org/absente.html",
            "exemple.org\\index.html",
            "/exemple.org/index.html",
            "",
        ] {
            assert!(site_page(d, "exemple.org", start).is_err(), "{start}");
        }
        assert!(site_page(d, "..", "dehors.html").is_err());
        assert!(site_page("relatif", "exemple.org", "exemple.org/index.html").is_err());
    }
}
