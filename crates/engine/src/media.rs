//! Vidéo (SPEC §7.17, phase 5) : yt-dlp et ffmpeg, programmes externes lancés comme
//! processus séparés, jamais liés au moteur (leurs licences restent les leurs, SPEC §8.3).
//!
//! Décisions de l'utilisateur du 2026-11-09 : yt-dlp ne fait qu'extraire, le moteur
//! télécharge lui-même les flux directs et ne confie à yt-dlp que les flux fragmentés ;
//! ffmpeg fusionne. Les outils se téléchargent au besoin, sur accord de l'utilisateur,
//! empreinte vérifiée ; yt-dlp se met à jour seul, signature vérifiée. Windows et Linux
//! seulement en v1.
//!
//! Sous-étape 4a : les outils ([`tools`]) et la vérification des signatures ([`openpgp`]).

pub mod openpgp;
pub mod tools;

/// Réglages du module.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// D'où viennent les outils (tests : un serveur local).
    pub sources: tools::Sources,
    /// Dossier des outils ; `None` : `tools/` à côté de la base.
    pub dir: Option<std::path::PathBuf>,
}

/// La vidéo est prise en charge sur cette plateforme (Windows et Linux en v1).
pub const SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));
