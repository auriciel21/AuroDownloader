//! Plugins d'hébergeurs (F-41, SPEC §7.20) : un plugin, fichier WebAssembly exécuté par
//! wasmi dans un bac à sable, trouve le lien direct d'un fichier sur la page d'un
//! hébergeur ; le moteur télécharge ensuite lui-même.
//!
//! - [`manifest`] : le manifeste, section `auro-plugin` du fichier, et ses domaines ;
//! - [`runtime`] : l'exécution (instance neuve, limites de calcul, de mémoire et de pile) ;
//! - [`fetch`] : les requêtes du plugin, faites par le moteur vers ses domaines déclarés,
//!   réseau public seulement, et les cookies du site, que le plugin ne voit pas.
//!
//! Les fichiers installés sont gardés sous `plugins/<empreinte>.wasm` à côté de la base,
//! l'empreinte revérifiée à chaque chargement ; la table `plugins` (`store::plugins`) garde
//! le manifeste et les permissions acceptées.

pub mod fetch;
pub mod manifest;
pub mod runtime;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use reqwest::Url;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::net::filename::{NameRules, sanitize};
use crate::store::plugins::PluginRow;
pub use manifest::Manifest;

/// Résolutions en même temps, au plus.
const SLOTS: usize = 2;
/// Durée d'une résolution, requêtes comprises.
const DEADLINE: Duration = Duration::from_secs(60);
/// Liens rendus par une résolution, au plus.
pub const MAX_LINKS: usize = 1000;
/// Résolutions gardées pour `plugin.add`, et leur durée.
const KEPT: usize = 32;
const KEEP_FOR: Duration = Duration::from_secs(600);

/// Un fichier de plugin lu, vérifié, pas encore installé.
#[derive(Debug, Clone)]
pub struct Inspected {
    pub manifest: Manifest,
    /// SHA-256 du fichier, hexadécimal minuscule.
    pub digest: String,
    pub size: usize,
}

/// SHA-256 d'un fichier, en hexadécimal minuscule.
pub fn digest_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Lit et vérifie un fichier de plugin : manifeste, puis WebAssembly servable (imports et
/// exports de l'interface). Calcul court (compilation par wasmi), sans réseau.
pub fn inspect(bytes: &[u8]) -> Result<Inspected, String> {
    let manifest = manifest::read(bytes)?;
    runtime::check(bytes)?;
    Ok(Inspected {
        manifest,
        digest: digest_of(bytes),
        size: bytes.len(),
    })
}

/// Le plugin activé qui prend en charge la page `url`, s'il y en a un (le premier par nom).
pub fn matching<'a>(rows: &'a [PluginRow], url: &str) -> Option<&'a PluginRow> {
    let url = Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    rows.iter().find(|row| {
        row.enabled
            && row
                .sites
                .iter()
                .any(|site| manifest::domain_covers(site, host))
    })
}

/// Un lien rendu par un plugin, vérifié.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    /// Nom proposé, déjà nettoyé (`net::filename::sanitize`).
    pub name: Option<String>,
    pub size: Option<u64>,
    pub referer: Option<String>,
}

/// Une résolution : la page, son plugin, les liens trouvés et les cookies du site.
#[derive(Debug, Clone)]
pub struct Resolution {
    /// Identifiant rendu au client, pour `plugin.add`.
    pub id: String,
    pub plugin_id: String,
    pub plugin_name: String,
    pub page: String,
    pub links: Vec<Link>,
    pub jar: fetch::Jar,
    pub logs: Vec<String>,
    created: Instant,
}

impl Resolution {
    /// L'en-tête `Cookie` du lien `link`, s'il y a des cookies du site pour son hôte.
    pub fn cookie_for(&self, link: &Link) -> Option<String> {
        let host = Url::parse(&link.url).ok()?.host_str()?.to_owned();
        self.jar.header_for(&host)
    }
}

/// Un texte du plugin montré à l'utilisateur : sans caractère de contrôle, 200 au plus.
fn shown(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(200).collect()
}

fn http_url(text: &str) -> Option<String> {
    let url = Url::parse(text).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then(|| url.to_string())
}

/// La sortie de `auro_resolve`, lue et vérifiée.
pub fn links_of(output: &[u8]) -> Result<Vec<Link>, String> {
    let value: Value = serde_json::from_slice(output)
        .map_err(|_| "le plugin a rendu une sortie illisible (JSON attendu)".to_owned())?;
    if let Some(error) = value.get("error") {
        let text = error
            .as_str()
            .map_or_else(|| error.to_string(), str::to_owned);
        return Err(format!("le plugin dit : {}", shown(&text)));
    }
    let list = value
        .get("links")
        .and_then(Value::as_array)
        .ok_or("le plugin n'a rendu ni links ni error")?;
    if list.len() > MAX_LINKS {
        return Err(format!("le plugin a rendu plus de {MAX_LINKS} liens"));
    }
    let mut links = Vec::with_capacity(list.len());
    for item in list {
        let url = item["url"]
            .as_str()
            .and_then(http_url)
            .ok_or("le plugin a rendu un lien qui n'est pas une adresse http(s)")?;
        let name = item["name"]
            .as_str()
            .map(|n| sanitize(n, NameRules::native()))
            .filter(|n| !n.is_empty());
        let size = item["size"].as_u64();
        let referer = item["referer"].as_str().and_then(http_url);
        links.push(Link {
            url,
            name,
            size,
            referer,
        });
    }
    Ok(links)
}

/// État des plugins d'un moteur : dossier des fichiers, places, client des requêtes et
/// résolutions gardées pour `plugin.add`.
pub struct Plugins {
    dir: PathBuf,
    slots: tokio::sync::Semaphore,
    http: Mutex<Option<reqwest::Client>>,
    /// Noms imposés au client des requêtes (tests : serveur local).
    resolve_to: Mutex<Vec<(String, SocketAddr)>>,
    /// Liens rendus permis vers le réseau interne (tests : serveur local).
    private_ok: std::sync::atomic::AtomicBool,
    kept: Mutex<VecDeque<Resolution>>,
}

impl std::fmt::Debug for Plugins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugins").field("dir", &self.dir).finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Plugins {
    /// Plugins gardés sous `plugins/` dans `data_dir`.
    pub fn new(data_dir: &Path) -> Self {
        Plugins {
            dir: data_dir.join("plugins"),
            slots: tokio::sync::Semaphore::new(SLOTS),
            http: Mutex::default(),
            resolve_to: Mutex::default(),
            private_ok: std::sync::atomic::AtomicBool::new(false),
            kept: Mutex::default(),
        }
    }

    fn file(&self, digest: &str) -> PathBuf {
        self.dir.join(format!("{digest}.wasm"))
    }

    /// Range le fichier d'un plugin sous son empreinte : écrit sous un nom provisoire,
    /// vidé sur disque, puis renommé ; rien à faire s'il est déjà là.
    pub fn save(&self, digest: &str, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        let path = self.file(digest);
        if path.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let partial = self.dir.join(format!("{digest}.wasm.tmp"));
        {
            let mut file = std::fs::File::create(&partial)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&partial, &path)
    }

    /// Efface le fichier d'un plugin (absent : rien à faire).
    pub fn delete_file(&self, digest: &str) {
        let _ = std::fs::remove_file(self.file(digest));
    }

    /// Le fichier d'un plugin installé, son empreinte revérifiée.
    pub fn load(&self, row: &PluginRow) -> Result<Vec<u8>, String> {
        let bytes = std::fs::read(self.file(&row.digest))
            .map_err(|_| format!("fichier du plugin {} introuvable : le réinstaller", row.id))?;
        if digest_of(&bytes) != row.digest {
            return Err(format!(
                "fichier du plugin {} modifié depuis son installation : le réinstaller",
                row.id
            ));
        }
        Ok(bytes)
    }

    /// Impose l'adresse d'un nom aux requêtes des plugins (tests : serveur local).
    #[cfg(any(test, feature = "testing"))]
    pub fn resolve_to(&self, name: &str, addr: SocketAddr) {
        lock(&self.resolve_to).push((name.to_owned(), addr));
        *lock(&self.http) = None;
    }

    /// Les liens des plugins joignent aussi le réseau interne (tests : serveur local).
    #[cfg(any(test, feature = "testing"))]
    pub fn allow_private(&self) {
        self.private_ok
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Liens rendus permis vers le réseau interne ? (Jamais hors des tests.)
    pub fn private_ok(&self) -> bool {
        self.private_ok.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn http(&self) -> Result<reqwest::Client, String> {
        let mut slot = lock(&self.http);
        if let Some(http) = slot.as_ref() {
            return Ok(http.clone());
        }
        let http = crate::net::plugin_http(&lock(&self.resolve_to)).map_err(|e| e.to_string())?;
        *slot = Some(http.clone());
        Ok(http)
    }

    /// Lance le plugin `row` sur la page `page`, dans le bac à sable.
    pub async fn run(&self, row: &PluginRow, page: &str) -> Result<Resolution, String> {
        let bytes = self.load(row)?;
        let http = self.http()?;
        let _permit = self
            .slots
            .acquire()
            .await
            .map_err(|_| "moteur arrêté".to_owned())?;
        let session = Arc::new(Mutex::new(fetch::Session::new(
            http,
            row.fetch.clone(),
            Instant::now() + DEADLINE,
        )));
        let handle = tokio::runtime::Handle::current();
        let shared = Arc::clone(&session);
        let serve: runtime::Fetch = Box::new(move |request| {
            let mut session = lock(&shared);
            handle.block_on(session.serve(request))
        });
        let input = serde_json::to_vec(&json!({ "url": page })).unwrap_or_default();
        let run = tokio::task::spawn_blocking(move || {
            runtime::resolve(&bytes, &input, serve, &runtime::LIMITS)
        })
        .await
        .map_err(|_| "le plugin s'est arrêté brutalement".to_owned())?;
        let links = links_of(&run.output?)?;
        let jar = lock(&session).jar.clone();
        Ok(Resolution {
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: row.id.clone(),
            plugin_name: row.name.clone(),
            page: page.to_owned(),
            links,
            jar,
            logs: run.logs,
            created: Instant::now(),
        })
    }

    /// Garde une résolution pour `plugin.add` (10 minutes, 32 au plus).
    pub fn keep(&self, resolution: Resolution) {
        let mut kept = lock(&self.kept);
        kept.retain(|r| r.created.elapsed() < KEEP_FOR);
        if kept.len() >= KEPT {
            kept.pop_front();
        }
        kept.push_back(resolution);
    }

    /// Une résolution gardée, si elle n'a pas expiré.
    pub fn kept(&self, id: &str) -> Option<Resolution> {
        let kept = lock(&self.kept);
        kept.iter()
            .find(|r| r.id == id && r.created.elapsed() < KEEP_FOR)
            .cloned()
    }
}

/// Délai avant de renouveler de nouveau le lien d'une même tâche.
const REFRESH_AGAIN: Duration = Duration::from_secs(600);

/// Surveille les tâches venues d'un plugin (SPEC §7.20) : une tâche en échec sur un lien
/// expiré (`403`, `410`) reçoit un lien neuf de son plugin, une fois (de nouveau 10 minutes
/// plus tard au plus tôt), puis reprend. Tient une poignée faible : s'arrête avec le
/// moteur.
pub(crate) async fn watch(
    engine: crate::scheduler::WeakEngine,
    mut events: tokio::sync::broadcast::Receiver<crate::scheduler::Event>,
) {
    use crate::scheduler::Event;
    use crate::store::tasks::TaskState;
    use tokio::sync::broadcast::error::RecvError;

    let mut stopped = engine.stopped();
    let mut refreshed: std::collections::HashMap<String, Instant> = Default::default();
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            _ = stopped.changed() => break,
        };
        let task_id = match event {
            Ok(Event::StateChanged {
                task_id,
                to: TaskState::Failed,
                error_code: Some(code),
                ..
            }) if code == "HTTP_STATUS" => task_id,
            Ok(_) | Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        refreshed.retain(|_, at| at.elapsed() < REFRESH_AGAIN);
        if refreshed.contains_key(&task_id) {
            continue;
        }
        let Some(handle) = engine.upgrade() else {
            break;
        };
        refreshed.insert(task_id.clone(), Instant::now());
        tokio::spawn(async move {
            if let Err(e) = handle.refresh_plugin_link(&task_id).await {
                eprintln!("aurod : lien du plugin non renouvelé pour la tâche {task_id} : {e}");
            }
        });
    }
}
