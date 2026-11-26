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
    /// Un téléchargement qui attendait dans sa file démarre (place libérée, plage horaire
    /// ouverte) : la fenêtre passe au premier plan (SPEC §4.6, 2026-11-26). Pas les
    /// nouvelles tentatives, ni les reprises au démarrage du moteur.
    pub show_on_start: bool,
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
            show_on_start: true,
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

    /// Enregistre : écrit à côté, vidé sur disque, puis remplace d'un seul coup, pour ne
    /// jamais laisser un fichier à moitié écrit. Le nom provisoire est propre à l'appel :
    /// deux enregistrements simultanés ne se prennent pas leur fichier (audit C10) ; les
    /// appelants les mettent en ordre sous le verrou des préférences.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!(
            "json.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let written = (|| {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written
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
            show_on_start: false,
            watch_clipboard: true,
            check_updates: false,
            last_update_check: Some(1_790_000_000),
            update_notice_shown: true,
        };
        prefs.save(&path).unwrap();
        assert_eq!(Prefs::load(&path), prefs);
        let leftovers = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "aucun fichier provisoire ne reste");

        // Fichier d'une autre version (dont l'ancien `destDir`, passé au moteur le
        // 2026-11-07), ou partiel : les champs connus sont gardés.
        std::fs::write(&path, br#"{"lang":"fr","destDir":"D:\\dl","nouveau":1}"#).unwrap();
        let loaded = Prefs::load(&path);
        assert_eq!(loaded.lang, "fr");
        assert!(loaded.close_to_tray);
        // Fichier d'avant la préférence (2026-11-08) : la fenêtre apparaît aux ajouts.
        assert!(loaded.show_on_add);
        // Fichier d'avant le démarrage en file (2026-11-26) : la fenêtre apparaît aussi.
        assert!(loaded.show_on_start);
        // Fichier d'avant les mises à jour (étape 8b) : recherche active, avis à montrer.
        assert!(loaded.check_updates);
        assert_eq!(loaded.last_update_check, None);
        assert!(!loaded.update_notice_shown);
    }

    /// Audit C10 : des enregistrements simultanés réussissent tous, et le fichier reste
    /// lisible (avant : un nom provisoire commun, que l'un renommait sous l'autre).
    #[test]
    fn simultaneous_saves_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        std::thread::scope(|scope| {
            for t in 0..4 {
                let path = &path;
                scope.spawn(move || {
                    for i in 0..50 {
                        let prefs = Prefs {
                            last_update_check: Some(t * 1000 + i),
                            ..Prefs::default()
                        };
                        prefs.save(path).unwrap();
                    }
                });
            }
        });
        assert!(Prefs::load(&path).last_update_check.is_some());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
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
