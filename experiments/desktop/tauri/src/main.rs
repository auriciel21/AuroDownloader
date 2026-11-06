//! Prototype Tauri de l'application de bureau (experiments/desktop/README.md).
//!
//! La partie Rust relaie le protocole du moteur (SPEC §6) entre `aurod` et la page : elle
//! reprend `auro_engine::rpc::local` (canal nommé ou socket Unix, un message par ligne),
//! passe chaque appel de la page au moteur et chaque notification du moteur à la page.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use auro_engine::rpc::local;
use serde_json::{Value, json};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent, Wry};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

struct Options {
    endpoint: String,
    dest: String,
    lang: String,
}

fn options() -> Options {
    let mut o = Options {
        endpoint: auro_engine::paths::endpoint(),
        dest: String::new(),
        lang: String::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().unwrap_or_default();
        match arg.as_str() {
            "--endpoint" => o.endpoint = value,
            "--dest" => o.dest = value,
            "--lang" => o.lang = value,
            _ => {}
        }
    }
    o
}

/// Liaison avec le moteur : file des messages à écrire, réponses attendues.
struct Bridge {
    out: mpsc::UnboundedSender<String>,
    waiting: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    next_id: AtomicU64,
    down: AtomicBool,
}

struct Tray {
    open: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

/// Appel d'une méthode du moteur par la page.
#[tauri::command]
async fn rpc(bridge: State<'_, Arc<Bridge>>, method: String, params: Value) -> Result<Value, String> {
    if bridge.down.load(Ordering::Relaxed) {
        return Err("moteur injoignable".into());
    }
    let id = bridge.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    bridge.waiting.lock().unwrap().insert(id, tx);
    let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    bridge
        .out
        .send(request.to_string())
        .map_err(|_| "moteur injoignable".to_string())?;
    let reply = rx.await.map_err(|_| "connexion au moteur perdue".to_string())?;
    match reply.get("error") {
        Some(error) => Err(error["message"].as_str().unwrap_or("erreur du moteur").to_owned()),
        None => Ok(reply["result"].clone()),
    }
}

/// Réglages passés en ligne de commande, pour la page.
#[tauri::command]
fn config(options: State<'_, Options>) -> Value {
    json!({"dest": options.dest, "lang": options.lang})
}

/// Langue retenue par la page : libellés du menu de l'icône.
#[tauri::command]
fn set_lang(tray: State<'_, Tray>, lang: String) {
    let english = lang.starts_with("en");
    let _ = tray.open.set_text(if english { "Open" } else { "Ouvrir" });
    let _ = tray.quit.set_text(if english { "Quit" } else { "Quitter" });
}

/// Première image affichée avec les tâches : mesure du démarrage.
#[tauri::command]
fn ready(epoch_ms: f64) {
    println!("PREMIERE_IMAGE {}", epoch_ms as i64);
}

/// Se connecte au moteur, écrit les messages de la file, relaie ce qu'il envoie.
async fn serve(app: AppHandle, bridge: Arc<Bridge>, endpoint: String, mut queue: mpsc::UnboundedReceiver<String>) {
    let (read, mut write) = match local::connect(&endpoint).await {
        Ok(halves) => halves,
        Err(e) => {
            bridge.down.store(true, Ordering::Relaxed);
            bridge.waiting.lock().unwrap().clear();
            let _ = app.emit("engine-error", format!("moteur injoignable : {e}"));
            return;
        }
    };
    let writer = tauri::async_runtime::spawn(async move {
        while let Some(mut line) = queue.recv().await {
            line.push('\n');
            if write.write_all(line.as_bytes()).await.is_err() || write.flush().await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        match message.get("id").and_then(Value::as_u64) {
            Some(id) => {
                if let Some(tx) = bridge.waiting.lock().unwrap().remove(&id) {
                    let _ = tx.send(message);
                }
            }
            None => {
                // Fenêtre repliée : l'avancement n'est pas transmis à la page, qui
                // recharge l'état complet à son retour (événement `resync`).
                let hidden = app
                    .get_webview_window("main")
                    .is_some_and(|w| !w.is_visible().unwrap_or(true));
                if hidden && message["method"] == "task.progress" {
                    continue;
                }
                let _ = app.emit("engine", message);
            }
        }
    }
    bridge.down.store(true, Ordering::Relaxed);
    bridge.waiting.lock().unwrap().clear();
    writer.abort();
    let _ = app.emit("engine-error", "connexion au moteur perdue");
}

fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = app.emit("resync", ());
    }
}

fn main() {
    let options = options();
    let endpoint = options.endpoint.clone();
    let (out, queue) = mpsc::unbounded_channel();
    let bridge = Arc::new(Bridge {
        out,
        waiting: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        down: AtomicBool::new(false),
    });
    tauri::Builder::default()
        .manage(options)
        .manage(Arc::clone(&bridge))
        .invoke_handler(tauri::generate_handler![rpc, config, set_lang, ready])
        .setup(move |app| {
            let open = MenuItem::with_id(app, "open", "Ouvrir", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quitter", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().expect("icône"))
                .tooltip("AuroDownloader")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show(app),
                    "quit" => app.exit(0),
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
            app.manage(Tray { open, quit });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(serve(handle, bridge, endpoint, queue));
            Ok(())
        })
        // Fermer la fenêtre la replie dans la zone de notification (F-36).
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("échec du prototype Tauri");
}
