//! Association aux liens magnet et aux fichiers `.torrent` (SPEC §4.3, étape 5b ; décision
//! de l'utilisateur du 2026-11-09 : une option, désactivée par défaut). Activée, elle fait
//! d'AuroDownloader le programme de l'utilisateur pour les deux, et garde celui d'avant ;
//! désactivée, elle le rétablit. Rien n'est jamais pris sans cette option.
//!
//! - Windows : clés de l'utilisateur (`HKCU\Software\Classes`), le précédent gardé sous
//!   `HKCU\Software\AuroDownloader\Associations`. Un choix fait par l'utilisateur dans
//!   Windows (« Toujours utiliser cette application ») passe avant : Windows peut alors
//!   redemander quel programme utiliser.
//! - Linux : `mimeapps.list` de l'utilisateur et une entrée de bureau à nous, le précédent
//!   gardé à côté.

use serde::Serialize;

/// État de l'association.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Le système le permet.
    pub available: bool,
    /// AuroDownloader ouvre les liens magnet et les fichiers `.torrent`.
    pub enabled: bool,
}

/// Commande qui ouvre un lien ou un fichier avec ce programme.
fn program() -> Result<String, String> {
    // AppImage : le programme lancé est l'image, pas son contenu monté.
    if let Some(image) = std::env::var_os("APPIMAGE") {
        return Ok(image.to_string_lossy().into_owned());
    }
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
pub use windows::Places;
#[cfg(windows)]
mod windows {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

    use super::Status;

    /// Identifiant de type des fichiers `.torrent` d'AuroDownloader.
    pub const PROG_ID: &str = "AuroDownloader.Torrent";

    /// Où écrire, sous `HKCU` : les classes de l'utilisateur, et la sauvegarde.
    pub struct Places {
        pub classes: String,
        pub saved: String,
    }

    impl Default for Places {
        fn default() -> Self {
            Places {
                classes: r"Software\Classes".into(),
                saved: r"Software\AuroDownloader\Associations".into(),
            }
        }
    }

    fn hkcu() -> RegKey {
        RegKey::predef(HKEY_CURRENT_USER)
    }

    fn read(path: &str, name: &str) -> Option<String> {
        hkcu()
            .open_subkey_with_flags(path, KEY_READ)
            .ok()?
            .get_value::<String, _>(name)
            .ok()
    }

    fn write(path: &str, name: &str, value: &str) -> Result<(), String> {
        let (key, _) = hkcu().create_subkey(path).map_err(|e| e.to_string())?;
        key.set_value(name, &value.to_owned())
            .map_err(|e| e.to_string())
    }

    fn command_of(exe: &str) -> String {
        format!("\"{exe}\" \"%1\"")
    }

    impl Places {
        fn magnet(&self) -> String {
            format!(r"{}\magnet", self.classes)
        }
        fn magnet_command(&self) -> String {
            format!(r"{}\magnet\shell\open\command", self.classes)
        }
        fn extension(&self) -> String {
            format!(r"{}\.torrent", self.classes)
        }
        fn prog_id(&self) -> String {
            format!(r"{}\{PROG_ID}", self.classes)
        }

        pub fn status(&self, exe: &str) -> Status {
            let magnet = read(&self.magnet_command(), "").as_deref() == Some(&command_of(exe));
            let torrent = read(&self.extension(), "").as_deref() == Some(PROG_ID)
                && read(&format!(r"{}\shell\open\command", self.prog_id()), "").as_deref()
                    == Some(&command_of(exe));
            Status {
                available: true,
                enabled: magnet && torrent,
            }
        }

        pub fn enable(&self, exe: &str) -> Result<(), String> {
            let command = command_of(exe);
            let icon = format!("\"{exe}\",0");
            // Le programme d'avant, gardé une seule fois : une seconde activation ne doit
            // pas le remplacer par le nôtre.
            if let Some(previous) = read(&self.magnet_command(), "")
                && previous != command
            {
                write(&self.saved, "magnet", &previous)?;
            }
            if let Some(previous) = read(&self.extension(), "")
                && previous != PROG_ID
            {
                write(&self.saved, "torrent", &previous)?;
            }
            write(&self.magnet(), "", "URL:Magnet link")?;
            write(&self.magnet(), "URL Protocol", "")?;
            write(&format!(r"{}\DefaultIcon", self.magnet()), "", &icon)?;
            write(&self.magnet_command(), "", &command)?;
            write(&self.prog_id(), "", "Torrent")?;
            write(&format!(r"{}\DefaultIcon", self.prog_id()), "", &icon)?;
            write(
                &format!(r"{}\shell\open\command", self.prog_id()),
                "",
                &command,
            )?;
            write(&self.extension(), "", PROG_ID)?;
            write(
                &format!(r"{}\OpenWithProgids", self.extension()),
                PROG_ID,
                "",
            )?;
            Ok(())
        }

        /// Retire ce qui désigne ce programme, et rétablit le précédent s'il y en avait un.
        pub fn disable(&self, exe: &str) -> Result<(), String> {
            let ours = command_of(exe);
            let hkcu = hkcu();
            if read(&self.magnet_command(), "").as_deref() == Some(&ours) {
                match read(&self.saved, "magnet") {
                    Some(previous) => write(&self.magnet_command(), "", &previous)?,
                    None => {
                        let _ = hkcu.delete_subkey_all(self.magnet());
                    }
                }
            }
            if read(&self.extension(), "").as_deref() == Some(PROG_ID) {
                match read(&self.saved, "torrent") {
                    Some(previous) => write(&self.extension(), "", &previous)?,
                    None => {
                        if let Ok(key) = hkcu.open_subkey_with_flags(self.extension(), KEY_WRITE) {
                            let _ = key.delete_value("");
                        }
                    }
                }
            }
            if let Ok(key) = hkcu
                .open_subkey_with_flags(format!(r"{}\OpenWithProgids", self.extension()), KEY_WRITE)
            {
                let _ = key.delete_value(PROG_ID);
            }
            let _ = hkcu.delete_subkey_all(self.prog_id());
            let _ = hkcu.delete_subkey_all(&self.saved);
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::Places;
#[cfg(target_os = "linux")]
mod linux {
    use std::path::{Path, PathBuf};

    use super::Status;

    /// Entrée de bureau d'AuroDownloader pour les liens (cachée des menus).
    pub const DESKTOP_FILE: &str = "aurodownloader-liens.desktop";
    const MIMES: [&str; 2] = ["x-scheme-handler/magnet", "application/x-bittorrent"];
    const SECTION: &str = "[Default Applications]";

    /// Dossiers de configuration et de données de l'utilisateur (XDG).
    pub struct Places {
        pub config: PathBuf,
        pub data: PathBuf,
    }

    impl Default for Places {
        fn default() -> Self {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            let dir = |var: &str, fallback: &str| {
                std::env::var_os(var)
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .unwrap_or_else(|| home.join(fallback))
            };
            Places {
                config: dir("XDG_CONFIG_HOME", ".config"),
                data: dir("XDG_DATA_HOME", ".local/share"),
            }
        }
    }

    /// Valeur de `mime` dans la section des applications par défaut.
    fn default_of(list: &str, mime: &str) -> Option<String> {
        let mut inside = false;
        for line in list.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                inside = line == SECTION;
            } else if inside && let Some(value) = line.strip_prefix(&format!("{mime}=")) {
                return Some(value.to_owned());
            }
        }
        None
    }

    /// `mimeapps.list` avec `mime` réglé sur `value` (ou retiré), le reste intact.
    fn set_default(list: &str, mime: &str, value: Option<&str>) -> String {
        let mut out: Vec<String> = Vec::new();
        let (mut inside, mut found_section, mut done) = (false, false, false);
        for line in list.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                if inside
                    && !done
                    && let Some(v) = value
                {
                    out.push(format!("{mime}={v}"));
                    done = true;
                }
                inside = trimmed == SECTION;
                found_section |= inside;
            } else if inside && trimmed.starts_with(&format!("{mime}=")) {
                if let (Some(v), false) = (value, done) {
                    out.push(format!("{mime}={v}"));
                    done = true;
                }
                continue;
            }
            out.push(line.to_owned());
        }
        if let (Some(v), false) = (value, done) {
            if !found_section {
                out.push(SECTION.to_owned());
            }
            out.push(format!("{mime}={v}"));
        }
        let mut text = out.join("\n");
        text.push('\n');
        text
    }

    fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("auro-tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    impl Places {
        fn list(&self) -> PathBuf {
            self.config.join("mimeapps.list")
        }
        fn desktop(&self) -> PathBuf {
            self.data.join("applications").join(DESKTOP_FILE)
        }
        fn saved(&self) -> PathBuf {
            self.data
                .join("applications")
                .join("aurodownloader-liens.precedents")
        }
        fn read_list(&self) -> String {
            std::fs::read_to_string(self.list()).unwrap_or_default()
        }

        pub fn status(&self, _exe: &str) -> Status {
            let list = self.read_list();
            let ours = format!("{DESKTOP_FILE};");
            Status {
                available: true,
                enabled: self.desktop().is_file()
                    && MIMES.iter().all(|m| {
                        default_of(&list, m).is_some_and(|v| v == ours || v == DESKTOP_FILE)
                    }),
            }
        }

        pub fn enable(&self, exe: &str) -> Result<(), String> {
            let quoted = exe.replace('\\', "\\\\").replace('"', "\\\"");
            write_atomic(
                &self.desktop(),
                &format!(
                    "[Desktop Entry]\nType=Application\nName=AuroDownloader\n\
                     Exec=\"{quoted}\" %u\nNoDisplay=true\nTerminal=false\n\
                     MimeType={};\n",
                    MIMES.join(";")
                ),
            )?;
            let mut list = self.read_list();
            let ours = format!("{DESKTOP_FILE};");
            // Le programme d'avant, gardé une seule fois : une seconde activation ne doit pas
            // le remplacer par le nôtre.
            if !self.saved().is_file() {
                let mut saved = String::new();
                for mime in MIMES {
                    if let Some(previous) = default_of(&list, mime).filter(|v| *v != ours) {
                        saved.push_str(&format!("{mime}={previous}\n"));
                    }
                }
                write_atomic(&self.saved(), &saved)?;
            }
            for mime in MIMES {
                list = set_default(&list, mime, Some(&ours));
            }
            write_atomic(&self.list(), &list)
        }

        pub fn disable(&self, _exe: &str) -> Result<(), String> {
            let mut list = self.read_list();
            let saved = std::fs::read_to_string(self.saved()).unwrap_or_default();
            let ours = format!("{DESKTOP_FILE};");
            for mime in MIMES {
                if default_of(&list, mime).is_some_and(|v| v == ours || v == DESKTOP_FILE) {
                    let previous = saved
                        .lines()
                        .find_map(|l| l.strip_prefix(&format!("{mime}=")));
                    list = set_default(&list, mime, previous);
                }
            }
            write_atomic(&self.list(), &list)?;
            let _ = std::fs::remove_file(self.desktop());
            let _ = std::fs::remove_file(self.saved());
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn mimeapps_entries_are_set_and_removed_keeping_the_rest() {
            let list = "[Added Associations]\nx-scheme-handler/magnet=a.desktop;\n\n\
                        [Default Applications]\ntext/html=firefox.desktop\n\
                        x-scheme-handler/magnet=qbittorrent.desktop;\n";
            assert_eq!(
                default_of(list, "x-scheme-handler/magnet").as_deref(),
                Some("qbittorrent.desktop;")
            );
            let set = set_default(list, "x-scheme-handler/magnet", Some("auro.desktop;"));
            assert!(set.contains("[Added Associations]\nx-scheme-handler/magnet=a.desktop;"));
            assert!(set.contains("x-scheme-handler/magnet=auro.desktop;"));
            assert!(!set.contains("qbittorrent"));
            assert!(set.contains("text/html=firefox.desktop"));
            let removed = set_default(&set, "x-scheme-handler/magnet", None);
            assert_eq!(default_of(&removed, "x-scheme-handler/magnet"), None);
            // Sans section : ajoutée.
            let fresh = set_default("", "application/x-bittorrent", Some("auro.desktop;"));
            assert_eq!(
                fresh,
                "[Default Applications]\napplication/x-bittorrent=auro.desktop;\n"
            );
        }
    }
}

/// Ni Windows ni Linux : rien à déclarer.
#[cfg(not(any(windows, target_os = "linux")))]
#[derive(Default)]
pub struct Places;

#[cfg(not(any(windows, target_os = "linux")))]
impl Places {
    pub fn status(&self, _exe: &str) -> Status {
        Status {
            available: false,
            enabled: false,
        }
    }
    pub fn enable(&self, _exe: &str) -> Result<(), String> {
        Err("non pris en charge sur ce système".into())
    }
    pub fn disable(&self, _exe: &str) -> Result<(), String> {
        Ok(())
    }
}

/// État de l'association de ce programme.
pub fn status() -> Status {
    match program() {
        Ok(exe) => Places::default().status(&exe),
        Err(_) => Status {
            available: false,
            enabled: false,
        },
    }
}

/// Active ou retire l'association de ce programme ; rend le nouvel état.
pub fn set(enabled: bool) -> Result<Status, String> {
    let exe = program()?;
    let places = Places::default();
    if enabled {
        places.enable(&exe)?;
    } else {
        places.disable(&exe)?;
    }
    Ok(places.status(&exe))
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;

    /// Emplacements d'essai : jamais ceux de l'utilisateur.
    #[cfg(windows)]
    fn places(name: &str) -> (Places, impl Drop) {
        struct Clean(String);
        impl Drop for Clean {
            fn drop(&mut self) {
                let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
                let _ = hkcu.delete_subkey_all(&self.0);
                // Le parent, s'il ne reste rien d'autre dessous (`delete_subkey` refuse
                // une clé non vide).
                let _ = hkcu.delete_subkey(r"Software\AuroDownloaderEssai");
            }
        }
        let root = format!(
            r"Software\AuroDownloaderEssai\{name}-{}",
            std::process::id()
        );
        (
            Places {
                classes: format!(r"{root}\Classes"),
                saved: format!(r"{root}\Associations"),
            },
            Clean(root),
        )
    }

    #[cfg(target_os = "linux")]
    fn places(_name: &str) -> (Places, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (
            Places {
                config: dir.path().join("config"),
                data: dir.path().join("data"),
            },
            dir,
        )
    }

    /// Un autre client torrent installé avant : rétabli quand l'association est retirée.
    #[cfg(windows)]
    fn other_client(p: &Places) {
        let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let (cmd, _) = hkcu
            .create_subkey(format!(r"{}\magnet\shell\open\command", p.classes))
            .unwrap();
        cmd.set_value("", &r#""C:\qbt\qbittorrent.exe" "%1""#.to_owned())
            .unwrap();
        let (ext, _) = hkcu
            .create_subkey(format!(r"{}\.torrent", p.classes))
            .unwrap();
        ext.set_value("", &"qBittorrent.File.Torrent".to_owned())
            .unwrap();
    }

    #[cfg(windows)]
    fn previous_is_back(p: &Places) -> bool {
        let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let cmd: String = hkcu
            .open_subkey(format!(r"{}\magnet\shell\open\command", p.classes))
            .unwrap()
            .get_value("")
            .unwrap();
        let ext: String = hkcu
            .open_subkey(format!(r"{}\.torrent", p.classes))
            .unwrap()
            .get_value("")
            .unwrap();
        cmd.contains("qbittorrent") && ext == "qBittorrent.File.Torrent"
    }

    #[cfg(target_os = "linux")]
    fn other_client(p: &Places) {
        std::fs::create_dir_all(&p.config).unwrap();
        std::fs::write(
            p.config.join("mimeapps.list"),
            "[Default Applications]\nx-scheme-handler/magnet=qbittorrent.desktop;\n\
             text/html=firefox.desktop\n",
        )
        .unwrap();
    }

    #[cfg(target_os = "linux")]
    fn previous_is_back(p: &Places) -> bool {
        let list = std::fs::read_to_string(p.config.join("mimeapps.list")).unwrap();
        list.contains("x-scheme-handler/magnet=qbittorrent.desktop;")
            && !list.contains("application/x-bittorrent=")
            && list.contains("text/html=firefox.desktop")
    }

    #[test]
    fn enabling_takes_both_and_disabling_gives_the_previous_client_back() {
        let (p, _clean) = places("assoc");
        let exe = if cfg!(windows) {
            r"C:\Essai\aurodownloader.exe"
        } else {
            "/opt/essai/aurodownloader"
        };
        other_client(&p);
        assert!(!p.status(exe).enabled);
        p.enable(exe).unwrap();
        assert!(p.status(exe).enabled);
        // Une seconde activation ne perd pas le client d'avant.
        p.enable(exe).unwrap();
        // Un autre AuroDownloader (autre dossier) n'est pas celui-ci.
        let other = if cfg!(windows) {
            r"C:\Autre\aurodownloader.exe"
        } else {
            "/opt/autre/aurodownloader"
        };
        if cfg!(windows) {
            assert!(!p.status(other).enabled);
        }
        p.disable(exe).unwrap();
        assert!(!p.status(exe).enabled);
        assert!(previous_is_back(&p), "client d'avant rétabli");
        // Retirer sans l'avoir activée : rien ne bouge.
        p.disable(exe).unwrap();
        assert!(previous_is_back(&p));
    }
}
