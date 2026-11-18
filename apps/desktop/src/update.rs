//! Mise à jour de l'application (SPEC §4.6, N-14 ; phase 7, étape 8b).
//!
//! `latest.json`, signé, est joint à la dernière publication GitHub ; aucun autre serveur, et
//! GitHub ne voit que l'adresse IP et la version demandée (aucune télémétrie). La recherche
//! se fait au plus une fois par jour, préférence `checkUpdates` (active par défaut) ; une
//! version nouvelle est proposée, et ne s'installe que sur accord. La signature est vérifiée
//! par le greffon de Tauri avec la clé publique de `tauri.conf.json` : vide, la mise à jour
//! est « non configurée » et rien n'est cherché.
//!
//! Selon le paquet installé : l'installateur NSIS et l'AppImage s'installent sur accord ; le
//! paquet `.deb` ne fait qu'avertir, avec le lien de la publication (l'utilisateur installe
//! le paquet lui-même) ; une construction sans paquet (`cargo run`, variante d'essai) ne
//! cherche jamais. Proxy : celui du système seulement.
//!
//! Essais : `AURO_UPDATE_KIND` (`install`, `notify`) force le type de paquet et
//! `AURO_UPDATE_ENDPOINT` remplace l'adresse de `latest.json` ; la signature reste vérifiée
//! avec la clé de la construction.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, Emitter, Manager, Runtime, Url};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Publications du projet.
pub const RELEASES: &str = "https://github.com/aurodownloader/aurodownloader/releases";

/// Une recherche par jour au plus.
pub const DAY: u64 = 24 * 3600;

/// Premier coup d'œil après le démarrage, puis toutes les trois heures, si un jour a passé.
const FIRST_LOOK: Duration = Duration::from_secs(60);
const LOOK_EVERY: Duration = Duration::from_secs(3 * 3600);

/// Comment cette installation se met à jour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Téléchargée, vérifiée et installée sur accord (NSIS, AppImage).
    Install,
    /// Seulement signalée, avec le lien de la publication (`.deb`, `.rpm`).
    Notify,
    /// Jamais cherchée (construction sans paquet, MSI).
    None,
}

/// Type d'après le paquet ; `forced` : valeur d'`AURO_UPDATE_KIND`.
pub fn kind_for(bundle: Option<BundleType>, forced: Option<&str>) -> Kind {
    match forced {
        Some("install") => return Kind::Install,
        Some("notify") => return Kind::Notify,
        Some("none") => return Kind::None,
        _ => {}
    }
    match bundle {
        Some(BundleType::Nsis | BundleType::AppImage) => Kind::Install,
        Some(BundleType::Deb | BundleType::Rpm) => Kind::Notify,
        _ => Kind::None,
    }
}

pub fn kind() -> Kind {
    let forced = std::env::var("AURO_UPDATE_KIND").ok();
    kind_for(tauri::utils::platform::bundle_type(), forced.as_deref())
}

/// Une recherche est due : jamais faite, faite il y a un jour ou plus, ou l'horloge a reculé.
pub fn due(last: Option<u64>, now: u64) -> bool {
    last.is_none_or(|last| now < last || now - last >= DAY)
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Page de la publication d'une version.
pub fn release_page(version: &str) -> String {
    format!("{RELEASES}/tag/v{}", version.trim_start_matches('v'))
}

/// Clé publique de la construction (`plugins.updater.pubkey`), si elle est renseignée.
fn configured<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|key| key.as_str())
        .is_some_and(|key| !key.trim().is_empty())
}

/// Version proposée à la page. Les notes sont du texte, montré tel quel (jamais du HTML).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub version: String,
    pub current: String,
    pub notes: Option<String>,
    /// Date de publication (secondes depuis 1970), si `latest.json` la donne.
    pub date: Option<i64>,
    pub page: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub kind: Kind,
    pub configured: bool,
    pub enabled: bool,
    pub last_check: Option<u64>,
    pub found: Option<Found>,
}

/// Résultat d'une recherche.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checked {
    pub checked_at: u64,
    pub found: Option<Found>,
}

/// Mise à jour trouvée, en attente d'accord ; une seule installation à la fois.
#[derive(Default)]
pub struct Updates {
    pending: Mutex<Option<Update>>,
    found: Mutex<Option<Found>>,
    installing: AtomicBool,
    checking: tokio::sync::Mutex<()>,
}

/// Préférences lues et écrites par l'application (`App` de main.rs).
pub trait Prefs: Send + Sync + 'static {
    fn enabled(&self) -> bool;
    fn last_check(&self) -> Option<u64>;
    /// Note l'heure de la recherche, et l'enregistre.
    fn record_check(&self, at: u64);
}

pub fn status<R: Runtime>(app: &AppHandle<R>, prefs: &dyn Prefs) -> Status {
    let updates = app.state::<Updates>();
    let found = updates.found.lock().unwrap().clone();
    Status {
        kind: kind(),
        configured: configured(app),
        enabled: prefs.enabled(),
        last_check: prefs.last_check(),
        found,
    }
}

/// Cherche une version nouvelle ; `force` : même si un jour n'a pas passé (bouton
/// « Rechercher maintenant »). L'heure est notée, réussite ou échec : un serveur en panne
/// n'est pas réinterrogé avant le lendemain.
pub async fn check<R: Runtime>(
    app: &AppHandle<R>,
    prefs: &dyn Prefs,
    force: bool,
) -> Result<Option<Checked>, String> {
    if kind() == Kind::None {
        return Err("cette installation ne se met pas à jour".into());
    }
    if !configured(app) {
        return Err("mises à jour non configurées dans cette version".into());
    }
    let updates = app.state::<Updates>();
    let _one = updates.checking.lock().await;
    let at = now();
    if !force && !due(prefs.last_check(), at) {
        return Ok(None);
    }
    let mut builder = app.updater_builder().timeout(Duration::from_secs(30));
    if let Ok(endpoint) = std::env::var("AURO_UPDATE_ENDPOINT") {
        let url = Url::parse(&endpoint).map_err(|e| format!("AURO_UPDATE_ENDPOINT : {e}"))?;
        builder = builder.endpoints(vec![url]).map_err(|e| e.to_string())?;
    }
    let result = match builder.build() {
        Ok(updater) => updater.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    prefs.record_check(at);
    let update = result.map_err(|e| format!("recherche impossible : {e}"))?;
    let found = update.as_ref().map(|u| Found {
        version: u.version.clone(),
        current: u.current_version.clone(),
        notes: u.body.clone(),
        date: u.date.map(|d| d.unix_timestamp()),
        page: release_page(&u.version),
    });
    *updates.found.lock().unwrap() = found.clone();
    *updates.pending.lock().unwrap() = update;
    Ok(Some(Checked {
        checked_at: at,
        found,
    }))
}

/// Avancement du téléchargement, pour la page (événement `update-progress`).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: u64,
    total: Option<u64>,
}

/// Installe la version trouvée, sur accord de l'utilisateur. Windows : l'installateur NSIS
/// (mode passif) remplace l'application, qui se ferme ; ses crochets arrêtent le moteur, et
/// il relance l'application. AppImage : remplacée, puis relancée ici. Les téléchargements
/// reprennent au redémarrage du moteur.
pub async fn install<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if kind() != Kind::Install {
        return Err(
            "cette installation ne s'installe pas d'elle-même : voir la page de la version".into(),
        );
    }
    let updates = app.state::<Updates>();
    if updates.installing.swap(true, Ordering::SeqCst) {
        return Err("installation déjà en cours".into());
    }
    let update = updates.pending.lock().unwrap().take();
    let Some(update) = update else {
        updates.installing.store(false, Ordering::SeqCst);
        return Err("aucune version à installer".into());
    };
    let mut done = 0u64;
    let emitter = app.clone();
    let result = update
        .download_and_install(
            move |chunk, total| {
                done += chunk as u64;
                let _ = emitter.emit_to("main", "update-progress", Progress { done, total });
            },
            || {},
        )
        .await;
    match result {
        Ok(()) => app.restart(),
        Err(e) => {
            updates.installing.store(false, Ordering::SeqCst);
            // La version reste proposée : un nouvel essai refait la recherche.
            Err(format!("installation impossible : {e}"))
        }
    }
}

/// Recherche de fond : peu après le démarrage, puis toutes les trois heures, si la
/// préférence le permet et qu'un jour a passé. Une version trouvée est proposée à la page
/// (événement `update-available`).
pub fn spawn<R: Runtime, P: Prefs>(
    app: AppHandle<R>,
    prefs: impl Fn(&AppHandle<R>) -> Option<P> + Send + 'static,
) {
    if kind() == Kind::None {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_LOOK).await;
        loop {
            if let Some(p) = prefs(&app)
                && p.enabled()
                && configured(&app)
                && let Ok(Some(Checked {
                    found: Some(found), ..
                })) = check(&app, &p, false).await
            {
                let _ = app.emit_to("main", "update-available", found);
            }
            tokio::time::sleep(LOOK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_follows_the_bundle() {
        assert_eq!(kind_for(Some(BundleType::Nsis), None), Kind::Install);
        assert_eq!(kind_for(Some(BundleType::AppImage), None), Kind::Install);
        assert_eq!(kind_for(Some(BundleType::Deb), None), Kind::Notify);
        assert_eq!(kind_for(Some(BundleType::Rpm), None), Kind::Notify);
        // Sans paquet (cargo run, variante d'essai) ou MSI : jamais.
        assert_eq!(kind_for(None, None), Kind::None);
        assert_eq!(kind_for(Some(BundleType::Msi), None), Kind::None);
        // Essais : AURO_UPDATE_KIND.
        assert_eq!(kind_for(None, Some("install")), Kind::Install);
        assert_eq!(
            kind_for(Some(BundleType::Nsis), Some("notify")),
            Kind::Notify
        );
        assert_eq!(kind_for(Some(BundleType::Nsis), Some("none")), Kind::None);
        assert_eq!(kind_for(Some(BundleType::Deb), Some("autre")), Kind::Notify);
    }

    #[test]
    fn at_most_once_a_day() {
        assert!(due(None, 1_000_000));
        assert!(!due(Some(1_000_000), 1_000_000));
        assert!(!due(Some(1_000_000), 1_000_000 + DAY - 1));
        assert!(due(Some(1_000_000), 1_000_000 + DAY));
        // Horloge qui recule : la recherche n'est pas bloquée pour toujours.
        assert!(due(Some(1_000_000), 999_999));
    }

    #[test]
    fn release_page_of_a_version() {
        assert_eq!(
            release_page("0.2.0"),
            "https://github.com/aurodownloader/aurodownloader/releases/tag/v0.2.0"
        );
        assert_eq!(
            release_page("v0.2.0"),
            "https://github.com/aurodownloader/aurodownloader/releases/tag/v0.2.0"
        );
    }
}
