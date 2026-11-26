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
mod clipboard;
mod files;
mod links;
mod opened;
mod prefs;
mod qr;
mod update;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bridge::{Bridge, CallError, Outgoing, QueuedStarts, Retry, Sink, Status};
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

/// Identifiant de la variante d'essai (`tauri.essai.json`).
const TRIAL_ID: &str = "org.aurodownloader.essai";

/// Options d'essai qui manquent à la variante d'essai : sans elles, elle prendrait le
/// moteur, la base ou les préférences réels de l'utilisateur (audit C23). Vide hors de la
/// variante d'essai, ou si rien ne manque.
fn missing_trial_options(identifier: &str, o: &Options) -> Vec<&'static str> {
    if identifier != TRIAL_ID {
        return Vec::new();
    }
    [
        ("--endpoint", o.endpoint.is_none()),
        ("--db", o.db.is_none()),
        ("--prefs", o.prefs.is_none()),
    ]
    .into_iter()
    .filter_map(|(name, missing)| missing.then_some(name))
    .collect()
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
    /// Fichiers que la page peut faire lire : choisis ou reçus (audit C14).
    granted: opened::Granted,
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
struct WindowSink(AppHandle, Mutex<QueuedStarts>);

impl WindowSink {
    fn new(app: AppHandle) -> Self {
        WindowSink(app, Mutex::new(QueuedStarts::default()))
    }
}

impl Sink for WindowSink {
    fn send(&self, out: Outgoing) {
        let new_task = out.announces_new_task();
        let queued_start = self
            .1
            .lock()
            .unwrap()
            .shows(&out, std::time::Instant::now());
        let _ = self.0.emit_to("main", "engine", out);
        // Un téléchargement pris par l'extension ou la ligne de commande, ou qui démarre
        // après avoir attendu dans sa file : la fenêtre passe au premier plan, pour qu'on
        // le voie commencer (SPEC §4.6). Ajouté depuis la fenêtre, il la trouve déjà devant.
        let wanted = self.0.try_state::<App>().is_some_and(|state| {
            let prefs = state.prefs.lock().unwrap();
            (new_task && prefs.show_on_add) || (queued_start && prefs.show_on_start)
        });
        if wanted {
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
    // Verrou tenu de la lecture au remplacement : une heure de recherche notée entre-temps
    // (`record_check`) n'est pas perdue, et les deux écritures ne se croisent pas (C10).
    {
        let mut current = state.prefs.lock().unwrap();
        let prefs = Prefs::from_page(prefs, &current);
        prefs
            .save(&state.prefs_path)
            .map_err(|e| format!("préférences non enregistrées : {e}"))?;
        *current = prefs;
    }
    let info = info(&app, &state);
    if let Some(menu) = state.menu.lock().unwrap().as_ref() {
        menu.translate(info.lang);
    }
    Ok(info)
}

/// Préférences de la mise à jour (`update`), tenues par l'état de l'application.
struct AppPrefs(AppHandle);

impl update::Prefs for AppPrefs {
    fn enabled(&self) -> bool {
        self.0.state::<App>().prefs.lock().unwrap().check_updates
    }

    fn last_check(&self) -> Option<u64> {
        self.0
            .state::<App>()
            .prefs
            .lock()
            .unwrap()
            .last_update_check
    }

    fn record_check(&self, at: u64) {
        let state = self.0.state::<App>();
        let mut prefs = state.prefs.lock().unwrap();
        prefs.last_update_check = Some(at);
        let _ = prefs.save(&state.prefs_path);
    }
}

/// État de la mise à jour : type d'installation, clé présente, préférence, version trouvée.
#[tauri::command]
fn update_status(app: AppHandle) -> update::Status {
    update::status(&app, &AppPrefs(app.clone()))
}

/// Recherche une version nouvelle ; `force` : sans attendre un jour (« Rechercher
/// maintenant »). `None` : pas encore due.
#[tauri::command]
async fn update_check(app: AppHandle, force: bool) -> Result<Option<update::Checked>, String> {
    update::check(&app, &AppPrefs(app.clone()), force).await
}

/// Installe la version trouvée, sur accord de l'utilisateur, puis relance l'application.
#[tauri::command]
async fn update_install(app: AppHandle) -> Result<(), String> {
    update::install(&app).await
}

/// Ouvre la page de la publication d'une version dans le navigateur (paquet `.deb`).
#[tauri::command]
fn update_open_page(app: AppHandle, version: String) -> Result<(), String> {
    app.opener()
        .open_url(update::release_page(&version), None::<&str>)
        .map_err(|e| e.to_string())
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

/// Ouvre dans le navigateur la page de départ d'une copie de site (SPEC §7.18 ter).
#[tauri::command]
fn open_site(app: AppHandle, dir: String, name: String, start: String) -> Result<(), String> {
    let page = files::site_page(&dir, &name, &start)?;
    app.opener()
        .open_path(page.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Montre un fichier dans son dossier (ou le dossier d'un torrent ou d'une copie de site).
#[tauri::command]
fn reveal_file(app: AppHandle, dir: String, name: String) -> Result<(), String> {
    let path = files::task_entry(&dir, &name)?;
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
        state.granted.grant_opened(&found);
        state.opened.lock().unwrap().extend(found);
    }
    show(app);
    let _ = app.emit_to("main", "opened", ());
}

/// Lit un fichier `.torrent` ou Metalink choisi ou reçu, pour `task.add` ou `metalink.read`
/// (base64).
#[tauri::command]
fn read_torrent(state: State<'_, App>, path: String) -> Result<opened::TorrentFile, String> {
    state.granted.check(&path)?;
    opened::read_torrent(&path)
}

/// Choix de fichiers `.torrent` ou Metalink ; aucun si l'utilisateur renonce.
#[tauri::command]
async fn pick_torrents(app: AppHandle, state: State<'_, App>) -> Result<Vec<String>, ()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Torrent, Metalink", &["torrent", "meta4", "metalink"])
        .pick_files(move |files| {
            let _ = tx.send(files.unwrap_or_default());
        });
    let picked: Vec<String> = rx
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    for path in &picked {
        state.granted.grant(path);
    }
    Ok(picked)
}

/// Choix d'une liste de liens à importer (F-31) ; `None` si l'utilisateur renonce.
#[tauri::command]
async fn pick_list(app: AppHandle, state: State<'_, App>) -> Result<Option<String>, ()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Liste", &["txt", "csv", "tsv", "lst", "list"])
        .add_filter("*", &["*"])
        .pick_file(move |file| {
            let _ = tx.send(file);
        });
    Ok(granted(&state, rx.await.ok().flatten()))
}

/// Chemin d'un fichier choisi dans un sélecteur, accordé à la page (audit C14).
fn granted(state: &App, file: Option<tauri_plugin_dialog::FilePath>) -> Option<String> {
    let path = file
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())?;
    state.granted.grant(&path);
    Some(path)
}

/// QR code de l'accès à distance (étape 3d) : adresse de la page et jeton dans l'ancre.
#[tauri::command]
fn qr_code(text: String) -> Result<qr::Qr, String> {
    qr::encode(&text)
}

/// Choix d'un certificat ou d'une clé PEM fournis pour l'accès à distance (réglages
/// `remote.tlsCert` et `remote.tlsKey`) ; `None` si l'utilisateur renonce. Le moteur les lit
/// lui-même.
#[tauri::command]
async fn pick_pem(app: AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("PEM", &["pem", "crt", "cer", "key"])
        .add_filter("*", &["*"])
        .pick_file(move |file| {
            let _ = tx.send(file);
        });
    rx.await
        .ok()
        .flatten()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Choix d'un fichier de clé privée SSH (SFTP, SPEC §7.22), en partant de `~/.ssh` s'il
/// existe, sinon du dossier personnel (jamais du dossier courant de l'application) ; `None`
/// si l'utilisateur renonce. Le moteur le lit lui-même, l'application jamais.
#[tauri::command]
async fn pick_key(app: AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file();
    if let Ok(home) = app.path().home_dir() {
        let ssh = home.join(".ssh");
        dialog = dialog.set_directory(if ssh.is_dir() { ssh } else { home });
    }
    dialog.pick_file(move |file| {
        let _ = tx.send(file);
    });
    rx.await
        .ok()
        .flatten()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Choix d'un fichier de plugin `.wasm` (SPEC §7.20) ; `None` si l'utilisateur renonce.
#[tauri::command]
async fn pick_plugin(app: AppHandle, state: State<'_, App>) -> Result<Option<String>, ()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Plugin", &["wasm"])
        .pick_file(move |file| {
            let _ = tx.send(file);
        });
    Ok(granted(&state, rx.await.ok().flatten()))
}

/// Lit un fichier de plugin choisi (base64), pour `plugin.inspect` et `plugin.install` ;
/// le moteur le vérifie, rien n'en est exécuté par l'application.
#[tauri::command]
fn read_plugin(state: State<'_, App>, path: String) -> Result<opened::TorrentFile, String> {
    state.granted.check(&path)?;
    opened::read_plugin(&path)
}

/// Enregistre le diagnostic montré (`engine.diagnostic`, N-12) dans une archive choisie par
/// l'utilisateur ; `None` s'il renonce. Un fichier choisi qui existe déjà (remplacement
/// confirmé par la boîte du système) est remplacé.
#[tauri::command]
async fn save_diagnostic(
    app: AppHandle,
    diagnostic: serde_json::Value,
) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name("diagnostic-aurodownloader.zip")
        .add_filter("Zip", &["zip"])
        .save_file(move |path| {
            let _ = tx.send(path.and_then(|p| p.into_path().ok()));
        });
    let Some(path) = rx.await.ok().flatten() else {
        return Ok(None);
    };
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|e| format!("{} : {e}", path.display()))?;
    }
    auro_engine::logs::write_archive(&path, &diagnostic)
        .map_err(|e| format!("{} : {e}", path.display()))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Lit une liste de liens choisie, comme un texte, pour `linkscan.run`.
#[tauri::command]
fn read_list(state: State<'_, App>, path: String) -> Result<opened::ListFile, String> {
    state.granted.check(&path)?;
    opened::read_list(&path)
}

/// Association aux liens magnet et aux fichiers `.torrent` (option, désactivée par défaut).
#[tauri::command]
fn links_status() -> links::Status {
    links::status()
}

/// Windows n'affichera pas les notifications du presse-papiers (avertissement des Options).
#[tauri::command]
fn notifications_blocked(app: tauri::AppHandle) -> bool {
    clipboard::notifications_blocked(&app)
}

#[tauri::command]
fn set_links(enabled: bool) -> Result<links::Status, String> {
    links::set(enabled)
}

/// Imprime la page (le guide : sa feuille de style d'impression ne garde que la copie de
/// `#print-area`). Par la fenêtre : WebView2 y appelle `window.print`, WebKitGTK ouvre sa
/// boîte d'impression, que `window.print` n'ouvre pas.
#[tauri::command]
fn print_page(window: tauri::WebviewWindow) -> Result<(), String> {
    window
        .print()
        .map_err(|e| format!("impression impossible : {e}"))
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
        WindowSink::new(app.clone()).send(Outgoing::Resync);
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
        // Mise à jour signée (étape 8b) : seulement par les commandes `update_*`.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(update::Updates::default())
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
            open_site,
            reveal_file,
            save_text,
            save_diagnostic,
            take_opened,
            read_torrent,
            pick_torrents,
            pick_list,
            read_list,
            links_status,
            set_links,
            notifications_blocked,
            qr_code,
            pick_pem,
            pick_key,
            pick_plugin,
            read_plugin,
            update_status,
            update_check,
            update_install,
            update_open_page,
            print_page,
            quit
        ])
        .setup(move |app| {
            // Variante d'essai sans ses options : dit, puis fermée, sans rien toucher (C23).
            let missing = missing_trial_options(&app.config().identifier, &options);
            if !missing.is_empty() {
                let handle = app.handle().clone();
                app.dialog()
                    .message(format!(
                        "La variante d'essai exige ses options d'essai, pour ne jamais toucher au \
                         moteur, à la base ni aux préférences réels. Manquent : {}.",
                        missing.join(", ")
                    ))
                    .title("AuroDownloader Essai")
                    .show(move |_| handle.exit(2));
                return Ok(());
            }
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
                granted: opened::Granted::default(),
                // Pris par la page à son démarrage (`take_opened`).
                opened: Mutex::new(Vec::new()),
            };
            state.granted.grant_opened(&first_opened);
            *state.opened.lock().unwrap() = first_opened;
            let lang = info(app.handle(), &state).lang;
            if let Some(menu) = state.menu.lock().unwrap().as_ref() {
                menu.translate(lang);
            }
            app.manage(state);

            // Liens copiés (F-30) : une notification, puis la boîte « Ajouter » sur clic.
            clipboard::spawn(
                app.handle().clone(),
                |app| {
                    let state = app.try_state::<App>()?;
                    let on = state.prefs.lock().unwrap().watch_clipboard;
                    on.then(|| info(app, &state).lang)
                },
                |app| {
                    app.get_webview_window("main")
                        .and_then(|w| w.is_focused().ok())
                        .unwrap_or(false)
                },
                |app, link| {
                    let opened = if opened::is_magnet(&link) {
                        Opened::Magnet { link }
                    } else {
                        Opened::Link { link }
                    };
                    receive(app, vec![opened]);
                },
            );

            // Mises à jour (étape 8b) : au plus une fois par jour, proposées à la page.
            update::spawn(app.handle().clone(), |app| {
                app.try_state::<App>().map(|_| AppPrefs(app.clone()))
            });

            let sink: Arc<dyn Sink> = Arc::new(WindowSink::new(app.handle().clone()));
            // Lancements d'`aurod` limités, et suspendus pendant une mise à jour (C6, C8).
            let starts = bridge::Starts::new();
            app.manage(Arc::clone(&starts));
            let connect = bridge::local_connect(endpoint.clone(), db.clone(), true, starts);
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
            // Fichiers .torrent (étape 5b) et Metalink (phase 6) glissés sur la fenêtre.
            WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                receive(window.app_handle(), opened::from_paths(paths));
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("échec de l'application de bureau");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit C23 : la variante d'essai exige ses trois options ; la vraie application, aucune.
    #[test]
    fn the_trial_variant_needs_its_trial_options() {
        let none = Options::default();
        assert_eq!(
            missing_trial_options(TRIAL_ID, &none),
            ["--endpoint", "--db", "--prefs"]
        );
        assert!(missing_trial_options("org.aurodownloader.desktop", &none).is_empty());
        let partial = Options {
            endpoint: Some(r"\.\pipe\auro-essai".into()),
            db: Some(PathBuf::from("essai.db")),
            ..Options::default()
        };
        assert_eq!(missing_trial_options(TRIAL_ID, &partial), ["--prefs"]);
        let all = Options {
            prefs: Some(PathBuf::from("essai.json")),
            ..partial
        };
        assert!(missing_trial_options(TRIAL_ID, &all).is_empty());
    }
}
