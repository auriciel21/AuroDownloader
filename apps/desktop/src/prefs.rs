//! Préférences de l'application de bureau, propres à l'interface : dossier proposé pour
//! les ajouts, langue, repli dans la zone de notification. Les réglages du moteur (limites,
//! files) restent dans sa base et passent par le protocole.
//!
//! Fichier JSON dans le dossier de configuration de l'application ; aucun secret (SPEC §8.2).

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// Dossier proposé pour les nouveaux téléchargements ; vide : « Téléchargements ».
    pub dest_dir: String,
    /// `fr`, `en`, ou vide pour suivre le système.
    pub lang: String,
    /// Fermer la fenêtre la replie dans la zone de notification, quand il y en a une.
    pub close_to_tray: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            dest_dir: String::new(),
            lang: String::new(),
            close_to_tray: true,
        }
    }
}

impl Prefs {
    /// Préférences enregistrées ; celles par défaut si le fichier manque ou est illisible.
    pub fn load(path: &Path) -> Prefs {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Enregistre : écrit à côté, puis remplace d'un seul coup, pour ne jamais laisser un
    /// fichier à moitié écrit.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    }

    /// Langue retenue : la préférence, sinon celle du système (`fr` ou `en`).
    pub fn language(&self, system: Option<&str>) -> &'static str {
        let wanted = if self.lang.is_empty() {
            system.unwrap_or("")
        } else {
            &self.lang
        };
        if wanted.to_ascii_lowercase().starts_with("fr") {
            "fr"
        } else {
            "en"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_broken_files_give_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        assert_eq!(Prefs::load(&path), Prefs::default());
        std::fs::write(&path, b"{pas du json").unwrap();
        assert_eq!(Prefs::load(&path), Prefs::default());
    }

    #[test]
    fn saved_prefs_load_back_and_unknown_fields_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sous-dossier").join("prefs.json");
        let prefs = Prefs {
            dest_dir: "D:\\Téléchargements".into(),
            lang: "en".into(),
            close_to_tray: false,
        };
        prefs.save(&path).unwrap();
        assert_eq!(Prefs::load(&path), prefs);
        assert!(!path.with_extension("json.tmp").exists());

        // Fichier d'une version future, ou partiel : les champs connus sont gardés.
        std::fs::write(&path, br#"{"lang":"fr","nouveau":1}"#).unwrap();
        let loaded = Prefs::load(&path);
        assert_eq!(loaded.lang, "fr");
        assert!(loaded.close_to_tray);
    }

    #[test]
    fn language_follows_the_preference_then_the_system() {
        let mut prefs = Prefs::default();
        assert_eq!(prefs.language(Some("fr-FR")), "fr");
        assert_eq!(prefs.language(Some("de-DE")), "en");
        assert_eq!(prefs.language(None), "en");
        prefs.lang = "fr".into();
        assert_eq!(prefs.language(Some("en-US")), "fr");
    }
}
