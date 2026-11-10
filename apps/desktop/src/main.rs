//! Application de bureau d'AuroDownloader (SPEC §4.3, §4.6, F-36).
//!
//! La fenêtre affiche la page de `ui/`, la même que l'interface web distante. Cette partie
//! Rust relie la page au moteur (`bridge`), démarre `aurod` s'il ne tourne pas, tient
//! l'icône de la zone de notification et les préférences de l'interface, et ouvre les
//! fichiers terminés à la demande. Aucune logique de téléchargement ici.
//!
//! Options : `--endpoint`, `--db` et `--prefs` (essais sur un moteur et des préférences à
//! part), `--hidden` (démarrage dans la zone de notification, utilisé par le démarrage avec
//! la session), `--unregister-links` (retire l'association aux liens magnet et aux
//! `.torrent`, puis quitte : appelé par la désinstallation). Un lien magnet ou un fichier
//! `.torrent` passé en argument préremplit la boîte « Ajouter » (étape 5b).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod files;
mod links;
mod opened;
mod prefs;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bridge::{Bridge, CallError, Outgoing, Retry, Sink, Status};
use opened::Opened;
use prefs::Prefs;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent, Wry};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::DialogExt as _;
use tauri_plugin_opener::OpenerExt as _;

/// Options de la ligne de commande.
#[derive(Debug, Default)]
struct Options {
    endpoint: Option<String>,
    db: Option<PathBuf>,
    prefs: Option<PathBuf>,
    hidden: bool,
}

fn options() -> Options {
    let mut o = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--endpoint" => o.endpoint = args.next(),
            "--db" => o.db = args.next().map(PathBuf::from),
            "--prefs" => o.prefs = args.next().map(PathBuf::from),
            "--hidden" => o.hidden = true,
            _ => {}
        }
    }
    o
}

/// État partagé par les commandes.
struct App {
    prefs: Mutex<Prefs>,
    prefs_path: PathBuf,
    /// Une zone de notification existe : fermer la fenêtre peut l'y replier.
    tray: bool,
    menu: Mutex<Option<TrayMenu>>,
    /// Liens et fichiers reçus du système, pas encore pris par la page.
    opened: Mutex<Vec<Opened>>,
}

struct TrayMenu {
    open: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    stop: MenuItem<Wry>,
}

impl TrayMenu {
    fn translate(&self, lang: &str) {
        let english = lang == "en";
        let label = |fr: &'static str, en: &'static str| if english { en } else { fr };
        let _ = self
            .open
            .set_text(label("Ouvrir AuroDownloader", "Open AuroDownloader"));
        let _ = self.quit.set_text(label(
            "Quitter (les téléchargements continuent)",
            "Quit (downloads continue)",
        ));
        let _ = self.stop.set_text(label(
            "Quitter et arrêter les téléchargements",
            "Quit and stop downloads",
        ));
    }
}

/// Envoie à la page ce que relaie la liaison.
struct WindowSink(AppHandle);

impl Sink for WindowSink {
    fn send(&self, out: Outgoing) {
        let new_task = out.announces_new_task();
        let _ = self.0.emit_to("main", "engine", out);
        // Un téléchargement pris par l'extension ou la ligne de commande : la fenêtre passe
        // au premier plan, pour qu'on le voie commencer (SPEC §4.6). Ajouté depuis la
        // fenêtre, il la trouve déjà devant.
        if new_task
            && self
                .0
                .try_state::<App>()
                .is_some_and(|state| state.prefs.lock().unwrap().show_on_add)
        {
            show(&self.0);
        }
    }

    fn hidden(&self) -> bool {
        self.0
            .get_webview_window("main")
            .is_none_or(|w| !w.is_visible().unwrap_or(true) || w.is_minimized().unwrap_or(false))
    }
}

/// Appel d'une méthode du moteur par la page.
#[tauri::command]
async fn rpc(
    bridge: State<'_, Arc<Bridge>>,
    method: String,
    params: Value,
) -> Result<Value, CallError> {
    bridge.call(&method, params).await
}

/// État de la connexion, pour une page qui vient de s'abonner à l'événement `engine`.
#[tauri::command]
fn engine_status(bridge: State<'_, Arc<Bridge>>) -> Status {
    bridge.status()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    version: &'static str,
    platform: &'static str,
    prefs: Prefs,
    /// Langue retenue : `fr` ou `en`.
    lang: &'static str,
    tray: bool,
    autostart: bool,
    executable_extensions: &'static [&'static str],
}

fn info(app: &AppHandle, state: &App) -> Info {
    let prefs = state.prefs.lock().unwrap().clone();
    let lang = prefs.language(sys_locale::get_locale().as_deref());
    Info {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        lang,
        prefs,
        tray: state.tray,
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        executable_extensions: files::EXECUTABLE,
    }
}

#[tauri::command]
fn app_info(app: AppHandle, state: State<'_, App>) -> Info {
    info(&app, &state)
}

/// Enregistre les préférences ; renvoie les informations mises à jour.
#[tauri::command]
fn save_prefs(app: AppHandle, state: State<'_, App>, prefs: Prefs) -> Result<Info, String> {
    prefs
        .save(&state.prefs_path)
        .map_err(|e| format!("préférences non enregistrées : {e}"))?;
    *state.prefs.lock().unwrap() = prefs;
    let info = info(&app, &state);
    if let Some(menu) = state.menu.lock().unwrap().as_ref() {
        menu.translate(info.lang);
    }
    Ok(info)
}

/// Démarrage avec la session (SPEC §4.6), dans la zone de notification.
#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let launcher = app.autolaunch();
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|e| e.to_string())?;
    launcher.is_enabled().map_err(|e| e.to_string())
}

/// Choix d'un dossier ; `None` si l'utilisateur renonce.
#[tauri::command]
async fn pick_folder(app: AppHandle, current: String) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file();
    if !current.is_empty() {
        dialog = dialog.set_directory(current);
    }
    dialog.pick_folder(move |folder| {
        let _ = tx.send(folder.and_then(|f| f.into_path().ok()));
    });
    rx.await
        .ok()
        .flatten()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Enregistre `text` dans un fichier choisi par l'utilisateur (export CSV de l'historique,
/// F-35), proposé sous le nom `name` ; rend son chemin, ou `None` s'il renonce. Seule la
/// boîte d'enregistrement du système désigne le fichier : la page ne choisit aucun chemin.
#[tauri::command]
async fn save_text(app: AppHandle, name: String, text: String) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(&name)
        .add_filter("CSV", &["csv"])
        .save_file(move |path| {
            let _ = tx.send(path.and_then(|p| p.into_path().ok()));
        });
    let Some(path) = rx.await.ok().flatten() else {
        return Ok(None);
    };
    std::fs::write(&path, text).map_err(|e| format!("{} : {e}", path.display()))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Ouvre un fichier terminé avec le programme associé ; jamais un exécutable.
#[tauri::command]
fn open_file(app: AppHandle, dir: String, name: String) -> Result<(), String> {
    let path = files::task_file(&dir, &name)?;
    if files::is_executable(&name) {
        return Err("programme ou script : affiché dans son dossier, jamais ouvert".into());
    }
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Montre un fichier dans son dossier.
#[tauri::command]
fn reveal_file(app: AppHandle, dir: String, name: String) -> Result<(), String> {
    let path = files::task_file(&dir, &name)?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|e| e.to_string())
}

/// Liens magnet et fichiers `.torrent` reçus du système depuis la dernière demande ; la page
/// les met dans la boîte « Ajouter », à confirmer.
#[tauri::command]
fn take_opened(state: State<'_, App>) -> Vec<Opened> {
    std::mem::take(&mut *state.opened.lock().unwrap())
}

/// Garde ce qui a été ouvert avec l'application et prévient la page (événement `opened`).
fn receive(app: &AppHandle, found: Vec<Opened>) {
    if found.is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<App>() {
        state.opened.lock().unwrap().extend(found);
    }
    show(app);
    let _ = app.emit_to("main", "opened", ());
}

/// Lit un fichier `.torrent` choisi ou reçu, pour `task.add` (base64).
#[tauri::command]
fn read_torrent(path: String) -> Result<opened::TorrentFile, String> {
    opened::read_torrent(&path)
}

/// Choix de fichiers `.torrent` ; aucun si l'utilisateur renonce.
#[tauri::command]
async fn pick_torrents(app: AppHandle) -> Vec<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Torrent", &["torrent"])
        .pick_files(move |files| {
            let _ = tx.send(files.unwrap_or_default());
        });
    rx.await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Association aux liens magnet et aux fichiers `.torrent` (option, désactivée par défaut).
#[tauri::command]
fn links_status() -> links::Status {
    links::status()
}

#[tauri::command]
fn set_links(enabled: bool) -> Result<links::Status, String> {
    links::set(enabled)
}

/// Quitte l'application ; avec `stop_engine`, arrête aussi le moteur, après sauvegarde.
#[tauri::command]
async fn quit(app: AppHandle, bridge: State<'_, Arc<Bridge>>, stop_engine: bool) -> Result<(), ()> {
    leave(&app, &bridge, stop_engine).await;
    Ok(())
}

async fn leave(app: &AppHandle, bridge: &Bridge, stop_engine: bool) {
    if stop_engine {
        let _ = bridge
            .call("engine.shutdown", json!({"graceful": true}))
            .await;
    }
    app.exit(0);
}

/// Affiche la fenêtre ; la page recharge l'état si de l'avancement lui a manqué.
fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    resync_if_missed(app);
}

fn resync_if_missed(app: &AppHandle) {
    let bridge = app.state::<Arc<Bridge>>();
    if bridge.take_missed() {
        WindowSink(app.clone()).send(Outgoing::Resync);
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<TrayMenu> {
    let open = MenuItem::with_id(app, "open", "Ouvrir AuroDownloader", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quitter", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "Quitter et arrêter", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit, &stop])?;
    TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .cloned()
                .expect("icône de l'application"),
        )
        .tooltip("AuroDownloader")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show(app),
            "quit" | "stop" => {
                let stop = event.id.as_ref() == "stop";
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let bridge = Arc::clone(&app.state::<Arc<Bridge>>());
                    leave(&app, &bridge, stop).await;
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(TrayMenu { open, quit, stop })
}

/// Une zone de notification existe. Sous Linux, seulement si un hôte d'icônes
/// (`org.kde.StatusNotifierWatcher`) est sur le bus de session : GNOME sans extension n'en
/// a pas, et une fenêtre repliée ne pourrait plus être rouverte.
#[cfg(target_os = "linux")]
fn tray_available() -> bool {
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return false;
    };
    let Ok(proxy) = zbus::blocking::fdo::DBusProxy::new(&connection) else {
        return false;
    };
    let Ok(name) = "org.kde.StatusNotifierWatcher".try_into() else {
        return false;
    };
    proxy.name_has_owner(name).unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn tray_available() -> bool {
    true
}

/// Sous Linux, WebKitGTK compose la page en OpenGL ; sans accélération graphique, cela
/// coûte 10 à 12 % d'un cœur pour une liste mise à jour deux fois par seconde, contre 2 %
/// sans composition (experiments/desktop/README.md). Une valeur déjà donnée est respectée.
#[cfg(target_os = "linux")]
fn prepare_webview() {
    if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
        // SAFETY : appelé au début de `main`, avant tout autre fil d'exécution.
        unsafe { std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1") };
    }
}

#[cfg(not(target_os = "linux"))]
fn prepare_webview() {}

fn main() {
    // Désinstallation : retirer l'association, sans rien ouvrir.
    if std::env::args().skip(1).any(|a| a == "--unregister-links") {
        let _ = links::set(false);
        return;
    }
    prepare_webview();
    let options = options();
    let first_opened = opened::from_args(
        std::env::args().skip(1),
        std::env::current_dir().ok().as_deref(),
    );
    let endpoint = options
        .endpoint
        .clone()
        .unwrap_or_else(auro_engine::paths::endpoint);
    let db = options.db.clone();
    let bridge = Bridge::new();
    let tray = tray_available();

    tauri::Builder::default()
        // Une seule instance : relancer l'application rouvre sa fenêtre ; un lien magnet ou
        // un .torrent passé à la nouvelle instance va à la boîte « Ajouter » de celle-ci.
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            show(app);
            let found =
                opened::from_args(args.into_iter().skip(1), Some(std::path::Path::new(&cwd)));
            receive(app, found);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .manage(Arc::clone(&bridge))
        .invoke_handler(tauri::generate_handler![
            rpc,
            engine_status,
            app_info,
            save_prefs,
            set_autostart,
            pick_folder,
            open_file,
            reveal_file,
            save_text,
            take_opened,
            read_torrent,
            pick_torrents,
            links_status,
            set_links,
            quit
        ])
        .setup(move |app| {
            let prefs_path = match options.prefs.clone() {
                Some(path) => path,
                None => app.path().app_config_dir()?.join("prefs.json"),
            };
            let prefs = Prefs::load(&prefs_path);
            let menu = if tray { Some(build_tray(app)?) } else { None };
            let state = App {
                prefs: Mutex::new(prefs),
                prefs_path,
                tray,
                menu: Mutex::new(menu),
                // Pris par la page à son démarrage (`take_opened`).
                opened: Mutex::new(first_opened),
            };
            let lang = info(app.handle(), &state).lang;
            if let Some(menu) = state.menu.lock().unwrap().as_ref() {
                menu.translate(lang);
            }
            app.manage(state);

            let sink: Arc<dyn Sink> = Arc::new(WindowSink(app.handle().clone()));
            let connect = bridge::local_connect(endpoint.clone(), db.clone(), true);
            tauri::async_runtime::spawn(Arc::clone(&bridge).run(connect, sink, Retry::default()));

            if !(options.hidden && tray) {
                show(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Fermer replie dans la zone de notification s'il y en a une (F-36) ; sinon
            // l'application se ferme, et le moteur continue.
            WindowEvent::CloseRequested { api, .. } => {
                let app = window.app_handle();
                let state = app.state::<App>();
                if state.tray && state.prefs.lock().unwrap().close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            WindowEvent::Focused(true) => resync_if_missed(window.app_handle()),
            // Fichiers .torrent glissés sur la fenêtre (étape 5b).
            WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                receive(window.app_handle(), opened::from_paths(paths));
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("échec de l'application de bureau");
}
