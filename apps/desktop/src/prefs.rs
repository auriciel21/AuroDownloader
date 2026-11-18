//! Préférences de l'application de bureau, propres à l'interface : langue, repli dans la
//! zone de notification. Les réglages du moteur (dossier des téléchargements, limites,
//! files) restent dans sa base et passent par le protocole.
//!
//! Fichier JSON dans le dossier de configuration de l'application ; aucun secret (SPEC §8.2).

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// `fr`, `en`, ou vide pour suivre le système.
    pub lang: String,
    /// Fermer la fenêtre la replie dans la zone de notification, quand il y en a une.
    pub close_to_tray: bool,
    /// Un téléchargement ajouté (extension, ligne de commande) fait apparaître la fenêtre
    /// au premier plan (SPEC §4.6).
    pub show_on_add: bool,
    /// Une adresse de fichier ou un lien magnet copié fait apparaître une notification qui
    /// propose de l'ajouter (F-30, SPEC §7.18) ; désactivé par défaut.
    pub watch_clipboard: bool,
    /// Recherche des mises à jour, au plus une fois par jour (SPEC §4.6, étape 8b) ; active
    /// par défaut.
    pub check_updates: bool,
    /// Heure de la dernière recherche (secondes depuis 1970) ; tenue par l'application seule
    /// (`save_prefs` garde la sienne).
    pub last_update_check: Option<u64>,
    /// L'avis du premier lancement sur les mises à jour a été montré.
    pub update_notice_shown: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            lang: String::new(),
            close_to_tray: true,
            show_on_add: true,
            watch_clipboard: false,
            check_updates: true,
            last_update_check: None,
            update_notice_shown: false,
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

    /// Préférences envoyées par la page : tout sauf l'heure de la dernière recherche, que
    /// l'application tient seule (la page en a une copie qui peut être plus ancienne).
    pub fn from_page(mut page: Prefs, current: &Prefs) -> Prefs {
        page.last_update_check = current.last_update_check;
        page
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
            lang: "en".into(),
            close_to_tray: false,
            show_on_add: false,
            watch_clipboard: true,
            check_updates: false,
            last_update_check: Some(1_790_000_000),
            update_notice_shown: true,
        };
        prefs.save(&path).unwrap();
        assert_eq!(Prefs::load(&path), prefs);
        assert!(!path.with_extension("json.tmp").exists());

        // Fichier d'une autre version (dont l'ancien `destDir`, passé au moteur le
        // 2026-11-07), ou partiel : les champs connus sont gardés.
        std::fs::write(&path, br#"{"lang":"fr","destDir":"D:\\dl","nouveau":1}"#).unwrap();
        let loaded = Prefs::load(&path);
        assert_eq!(loaded.lang, "fr");
        assert!(loaded.close_to_tray);
        // Fichier d'avant la préférence (2026-11-08) : la fenêtre apparaît aux ajouts.
        assert!(loaded.show_on_add);
        // Fichier d'avant les mises à jour (étape 8b) : recherche active, avis à montrer.
        assert!(loaded.check_updates);
        assert_eq!(loaded.last_update_check, None);
        assert!(!loaded.update_notice_shown);
    }

    #[test]
    fn the_page_does_not_set_the_last_update_check() {
        let current = Prefs {
            last_update_check: Some(2_000),
            ..Prefs::default()
        };
        let page = Prefs {
            lang: "en".into(),
            check_updates: false,
            last_update_check: Some(1_000),
            ..Prefs::default()
        };
        let saved = Prefs::from_page(page, &current);
        assert_eq!(saved.last_update_check, Some(2_000));
        assert_eq!(saved.lang, "en");
        assert!(!saved.check_updates);
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
