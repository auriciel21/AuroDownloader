//! Emplacements par défaut, propres à l'utilisateur : base du moteur et point d'accès du
//! protocole local (SPEC §4.6 et §6.2).

use std::path::PathBuf;

/// Nom du dossier des données, sous Linux.
const APP: &str = "aurodownloader";

/// Dossier des données du moteur : `%LOCALAPPDATA%\AuroDownloader` sous Windows,
/// `$XDG_DATA_HOME/aurodownloader` (ou `~/.local/share/aurodownloader`) ailleurs.
/// `None` si l'environnement ne permet pas de le trouver.
pub fn data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("AuroDownloader"));
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join(APP))
}

/// Base du moteur par défaut.
pub fn database() -> Option<PathBuf> {
    data_dir().map(|d| d.join("auro.db"))
}

/// Point d'accès local par défaut du protocole : canal nommé `\\.\pipe\aurodownloader-<nom>`
/// sous Windows, socket `$XDG_RUNTIME_DIR/aurodownloader/aurod.sock` ailleurs (ou
/// `/tmp/aurodownloader-<nom>/aurod.sock` sans `XDG_RUNTIME_DIR`). Le nom d'utilisateur
/// sépare les moteurs de plusieurs utilisateurs d'une même machine. `AURO_ENDPOINT` le
/// remplace, pour les essais (un navigateur d'essai le transmet au relais qu'il lance).
pub fn endpoint() -> String {
    if let Some(endpoint) = std::env::var_os("AURO_ENDPOINT").and_then(|e| e.into_string().ok()) {
        return endpoint;
    }
    default_endpoint()
}

/// Point d'accès par défaut, sans tenir compte d'`AURO_ENDPOINT` : celui du moteur réel,
/// le seul qui sert la base par défaut. Les gardes contre un moteur d'essai ouvert sur la
/// base réelle le comparent au point d'accès effectif (audit R3 : avec la variable,
/// `endpoint()` rendait le point d'essai, pris alors pour celui par défaut).
pub fn default_endpoint() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".into())
        .replace(['\\', '/'], "_");
    if cfg!(windows) {
        return format!(r"\\.\pipe\aurodownloader-{user}");
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|d| d.join(APP))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("{APP}-{user}")));
    dir.join("aurod.sock").to_string_lossy().into_owned()
}
