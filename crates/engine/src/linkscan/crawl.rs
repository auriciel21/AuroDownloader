//! Exploration d'un site (F-42, SPEC §7.18 bis, étape A du « Grabber ») : le moteur lit les
//! pages d'un site de lien en lien et rend les liens trouvés, comme l'analyse d'une page.
//!
//! Décisions de l'utilisateur du 2026-11-26 : pages du même hôte seulement, fichiers des
//! autres sites relevés ; robots.txt respecté (sauf case contraire) ; 2 lectures à la fois,
//! 250 ms entre deux ; profondeur 1 à 5, 200 pages par défaut, 2 000 au plus ; identifiants
//! des sites ; pages exclues par une expression. L'exploration tourne dans une tâche
//! `tokio` tenue par sa connexion ([`Handle`]) ; rien n'est enregistré en base.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use regex::Regex;
use reqwest::Url;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, sleep, sleep_until};

use super::robots::Robots;
use super::{Link, LinkKind, MAX_INPUT, MAX_LINKS, from_html_page};
use crate::net::auth::Auth;
use crate::net::filename::{NameRules, sanitize};
use crate::net::{Client, Fetched, NetError, TextWant};
use crate::scheduler::{CrawlSlot, Engine, EngineError};

/// Profondeur la plus grande (décision).
pub const MAX_DEPTH: u8 = 5;
/// Pages lues au plus, par défaut (décision).
pub const DEFAULT_PAGES: u32 = 200;
/// Pages lues au plus, réglage le plus grand (décision).
pub const MAX_PAGES: u32 = 2_000;
/// Octets lus au plus par une exploration (décision).
pub const MAX_BYTES: u64 = 256 << 20;
/// Explorations en cours au plus dans le moteur.
pub const MAX_RUNNING: usize = 4;
/// Pages à ne pas ouvrir, par défaut des clients (décision).
pub const DEFAULT_EXCLUDE: &str = "(?i)logout|signout|deconnexion|déconnexion";
/// Lectures en même temps au plus (décision).
const PARALLEL: usize = 2;
/// Temps entre deux débuts de lecture, au moins (décision).
pub const SPACING: Duration = Duration::from_millis(250);
/// `Crawl-delay` pris en compte, au plus.
const MAX_CRAWL_DELAY: Duration = Duration::from_secs(10);
/// Attente d'une nouvelle tentative après `429` ou `503`, au plus.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);
/// robots.txt lu au plus (RFC 9309 : 500 Kio au moins).
const ROBOTS_LIMIT: usize = 500 << 10;
/// Parties d'un `sitePath` au plus (celles d'un `subDir`).
const MAX_SITE_PARTS: usize = 8;

/// Réglages d'une exploration.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Niveau le plus profond lu (1 à [`MAX_DEPTH`]) : la page de départ est au niveau 0.
    pub depth: u8,
    /// Pages lues au plus, page de départ comprise (1 à [`MAX_PAGES`]).
    pub max_pages: u32,
    /// Pages à ne pas ouvrir : comparée à l'adresse décodée.
    pub exclude: Option<Regex>,
    /// Seulement les pages sous le dossier de la page de départ.
    pub same_folder: bool,
    /// Lire même les pages que robots.txt refuse aux robots.
    pub ignore_robots: bool,
    /// Temps entre deux débuts de lecture ([`SPACING`] ; plus court dans les tests).
    pub spacing: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            depth: 1,
            max_pages: DEFAULT_PAGES,
            exclude: None,
            same_folder: false,
            ignore_robots: false,
            spacing: SPACING,
        }
    }
}

/// Ce qu'il est advenu d'un lien de navigation de sorte `page`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visit {
    /// En attente de lecture.
    Queued,
    /// Lue.
    Read,
    /// Au-delà de la profondeur choisie.
    Depth,
    /// Un autre hôte, ou de `https` vers `http`.
    Offsite,
    /// Hors du dossier de départ.
    Folder,
    /// Refusée par robots.txt.
    Robots,
    /// Répond à l'expression des pages à ne pas ouvrir.
    Excluded,
    /// Réponse autre que `200`, page trop grosse, erreur.
    Failed,
    /// L'exploration s'est arrêtée avant de la lire.
    Limit,
}

impl Visit {
    pub fn as_str(self) -> &'static str {
        match self {
            Visit::Queued => "queued",
            Visit::Read => "read",
            Visit::Depth => "depth",
            Visit::Offsite => "offsite",
            Visit::Folder => "folder",
            Visit::Robots => "robots",
            Visit::Excluded => "excluded",
            Visit::Failed => "failed",
            Visit::Limit => "limit",
        }
    }
}

/// Un lien trouvé par l'exploration.
#[derive(Debug, Clone)]
pub struct Found {
    pub link: Link,
    /// Niveau du lien : celui de la page qui le cite, plus un.
    pub depth: u8,
    /// Adresse de la première page lue qui le cite.
    pub found_on: String,
    /// Sous-dossier qui garde l'arborescence du site : l'hôte, puis les dossiers du chemin.
    pub site_path: String,
    /// Lien de navigation de sorte `page` : ce qu'il en est advenu ; `None` sinon.
    pub visit: Option<Visit>,
}

/// État d'une exploration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Cancelled,
    Failed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Done => "done",
            State::Cancelled => "cancelled",
            State::Failed => "failed",
        }
    }
}

/// Pourquoi une exploration finie s'est arrêtée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Finished,
    MaxPages,
    MaxBytes,
    MaxLinks,
}

impl Stop {
    pub fn as_str(self) -> &'static str {
        match self {
            Stop::Finished => "finished",
            Stop::MaxPages => "maxPages",
            Stop::MaxBytes => "maxBytes",
            Stop::MaxLinks => "maxLinks",
        }
    }
}

/// Échec de la page de départ : un code de la SPEC §6.7 et un message sans adresse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
}

/// Avancement d'une exploration.
#[derive(Debug, Clone)]
pub struct Progress {
    pub state: State,
    pub pages_read: u32,
    /// Pages en attente ou en cours de lecture.
    pub pages_queued: u32,
    pub links: usize,
    pub bytes_read: u64,
    /// À la fin d'une exploration `done`.
    pub stop: Option<Stop>,
    /// Exploration `failed`.
    pub error: Option<Failure>,
    /// Page de départ après ses redirections, une fois lue.
    pub page_url: Option<String>,
}

/// Ce qu'une exploration a trouvé, lu par sa connexion pendant qu'elle tourne.
#[derive(Debug)]
pub struct Shared {
    inner: Mutex<Inner>,
    changed: watch::Sender<u64>,
}

#[derive(Debug)]
struct Inner {
    progress: Progress,
    links: Vec<Found>,
    index: HashMap<String, usize>,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            inner: Mutex::new(Inner {
                progress: Progress {
                    state: State::Running,
                    pages_read: 0,
                    pages_queued: 0,
                    links: 0,
                    bytes_read: 0,
                    stop: None,
                    error: None,
                    page_url: None,
                },
                links: Vec::new(),
                index: HashMap::new(),
            }),
            changed: watch::channel(0).0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// L'avancement.
    pub fn progress(&self) -> Progress {
        self.lock().progress.clone()
    }

    /// L'avancement et les liens trouvés jusqu'ici.
    pub fn result(&self) -> (Progress, Vec<Found>) {
        let inner = self.lock();
        (inner.progress.clone(), inner.links.clone())
    }

    /// Change à chaque nouvelle page lue, et à la fin.
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Modifie l'exploration si elle tourne encore (une exploration arrêtée ne bouge plus).
    fn update(&self, change: impl FnOnce(&mut Inner)) {
        {
            let mut inner = self.lock();
            if inner.progress.state != State::Running {
                return;
            }
            change(&mut inner);
        }
        self.changed.send_modify(|n| *n += 1);
    }

    /// Fin : les pages encore en attente ne seront pas lues.
    fn finish(&self, state: State, stop: Option<Stop>, error: Option<Failure>) {
        self.update(|inner| {
            for found in &mut inner.links {
                if found.visit == Some(Visit::Queued) {
                    found.visit = Some(Visit::Limit);
                }
            }
            let progress = &mut inner.progress;
            progress.state = state;
            progress.stop = stop;
            progress.error = error;
            progress.pages_queued = 0;
        });
    }
}

/// Une exploration en cours, tenue par sa connexion : la lâcher l'arrête.
#[derive(Debug)]
pub struct Handle {
    pub id: String,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Handle {
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Arrête l'exploration ; ses liens restent lisibles.
    pub fn cancel(&self) {
        self.task.abort();
        self.shared.finish(State::Cancelled, None, None);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Lance l'exploration à partir de la page `start` : `CrawlBusy` si le moteur en a déjà
/// [`MAX_RUNNING`] en cours.
pub fn start(
    engine: &Engine,
    id: String,
    start: Url,
    settings: Settings,
) -> Result<Handle, EngineError> {
    let slot = engine.crawl_slot()?;
    let shared = Arc::new(Shared::new());
    let task = tokio::spawn(run(
        engine.clone(),
        id.clone(),
        start,
        settings,
        Arc::clone(&shared),
        slot,
    ));
    Ok(Handle { id, shared, task })
}

/// Ce qu'une page peut être : même site, dossier, exclusion, robots.txt, profondeur.
#[derive(Debug)]
struct Judge {
    host: String,
    port: Option<u16>,
    https: bool,
    /// Dossier de départ, avec la case « Rester sous le dossier de départ ».
    folder: Option<String>,
    exclude: Option<Regex>,
    robots: Robots,
    depth: u8,
}

impl Judge {
    /// Pourquoi la page `url` ne se lit pas, quel que soit son niveau ; `None` si elle se lit.
    fn refuses(&self, url: &Url) -> Option<Visit> {
        if url.host_str() != Some(self.host.as_str())
            || url.port() != self.port
            || (self.https && url.scheme() != "https")
            || !matches!(url.scheme(), "http" | "https")
        {
            return Some(Visit::Offsite);
        }
        if let Some(folder) = &self.folder
            && !url.path().starts_with(folder.as_str())
        {
            return Some(Visit::Folder);
        }
        if let Some(exclude) = &self.exclude {
            let decoded = percent_encoding::percent_decode_str(url.as_str()).decode_utf8_lossy();
            if exclude.is_match(&decoded) {
                return Some(Visit::Excluded);
            }
        }
        if !self.robots.allows(url) {
            return Some(Visit::Robots);
        }
        None
    }

    /// Ce qu'il advient d'une page de niveau `depth`.
    fn visit(&self, url: &Url, depth: u8) -> Visit {
        match self.refuses(url) {
            Some(Visit::Robots) | None if depth > self.depth => Visit::Depth,
            Some(visit) => visit,
            None => Visit::Queued,
        }
    }
}

async fn run(
    engine: Engine,
    id: String,
    start: Url,
    settings: Settings,
    shared: Arc<Shared>,
    _slot: CrawlSlot,
) {
    let client = engine.http_client().clone();
    let mut logins = HashMap::new();
    // La page de départ, demandée par l'utilisateur : lue même si robots.txt la refuse.
    let auth = engine.crawl_login(start.as_str(), &mut logins).await;
    let want = TextWant {
        limit: MAX_INPUT,
        truncate: false,
        html_only: true,
    };
    shared.update(|inner| {
        inner.progress.pages_read = 1;
        inner.progress.pages_queued = 1;
    });
    let failure = |code: &'static str, message: String| Some(Failure { code, message });
    let (page_url, bytes) = match fetch(&client, &start, want, auth, &|_| true).await {
        Ok(Fetched::Body { final_url, bytes }) => (final_url, bytes),
        Ok(Fetched::Other { .. }) => {
            let why = "l'adresse de départ n'est pas une page web (HTML)".to_owned();
            return shared.finish(State::Failed, None, failure("INVALID_PARAMS", why));
        }
        Ok(Fetched::TooBig { .. }) => {
            let why = format!("page de départ de plus de {} Mio", MAX_INPUT >> 20);
            return shared.finish(State::Failed, None, failure("INVALID_PARAMS", why));
        }
        Ok(Fetched::Status {
            status,
            retry_after,
        }) => {
            let why = NetError::Status {
                status,
                retry_after,
            }
            .to_string();
            return shared.finish(State::Failed, None, failure("SOURCE_UNREACHABLE", why));
        }
        Ok(Fetched::Elsewhere { .. }) => {
            let why = "page de départ injoignable".to_owned();
            return shared.finish(State::Failed, None, failure("SOURCE_UNREACHABLE", why));
        }
        Err(e) => {
            let code = match e {
                NetError::AddressForbidden => "ADDRESS_FORBIDDEN",
                NetError::InvalidUrl(_) | NetError::UnsupportedScheme(_) => "URL_INVALID",
                _ => "SOURCE_UNREACHABLE",
            };
            return shared.finish(State::Failed, None, failure(code, e.to_string()));
        }
    };
    let host = page_url.host_str().unwrap_or_default().to_owned();
    let mut bytes_read = bytes.len() as u64;
    let robots = if settings.ignore_robots {
        Robots::allow_all()
    } else {
        let (robots, read) = read_robots(&client, &page_url).await;
        bytes_read += read;
        robots
    };
    let spacing = settings
        .spacing
        .max(robots.crawl_delay.unwrap_or_default().min(MAX_CRAWL_DELAY));
    let path = page_url.path();
    let judge = Judge {
        host: host.clone(),
        port: page_url.port(),
        https: page_url.scheme() == "https",
        folder: settings
            .same_folder
            .then(|| path[..=path.rfind('/').unwrap_or(0)].to_owned()),
        exclude: settings.exclude.clone(),
        robots,
        depth: settings.depth,
    };
    let mut crawler = Crawler {
        client,
        engine,
        shared: Arc::clone(&shared),
        judge: Arc::new(judge),
        logins,
        considered: HashSet::from([start.to_string(), page_url.to_string()]),
        start: HashSet::from([start.to_string(), page_url.to_string()]),
        queue: VecDeque::new(),
        pages_read: 1,
        bytes_read,
        stop: None,
        max_pages: settings.max_pages,
        spacing,
    };
    shared.update(|inner| inner.progress.page_url = Some(page_url.to_string()));
    crawler.read(&page_url, &bytes, 0);
    crawler.publish(0);
    let stop = crawler.explore().await;
    let (pages, links) = (crawler.pages_read, shared.progress().links);
    tracing::info!(crawl = %id, host = %host, pages, links, stop = stop.as_str(), "exploration d'un site finie");
    shared.finish(State::Done, Some(stop), None);
}

/// robots.txt du site de `page` ; avec les octets lus. Absent ou `4xx` : tout est permis ;
/// `429`, `5xx` ou injoignable : rien (RFC 9309, §2.3.1).
async fn read_robots(client: &Client, page: &Url) -> (Robots, u64) {
    let Ok(url) = page.join("/robots.txt") else {
        return (Robots::allow_all(), 0);
    };
    let want = TextWant {
        limit: ROBOTS_LIMIT,
        truncate: true,
        html_only: false,
    };
    match client.fetch_text(&url, want, None, &|_| true).await {
        Ok(Fetched::Body { bytes, .. }) => (
            Robots::parse(&String::from_utf8_lossy(&bytes)),
            bytes.len() as u64,
        ),
        Ok(Fetched::Status { status, .. }) if (400..500).contains(&status) && status != 429 => {
            (Robots::allow_all(), 0)
        }
        _ => (Robots::deny_all(), 0),
    }
}

/// Une page, avec une seule nouvelle tentative après `429` ou `503` annoncés.
async fn fetch(
    client: &Client,
    url: &Url,
    want: TextWant,
    auth: Option<Arc<Auth>>,
    arrival: &(dyn Fn(&Url) -> bool + Send + Sync),
) -> Result<Fetched, NetError> {
    let first = client.fetch_text(url, want, auth.as_deref(), arrival).await;
    if let Ok(Fetched::Status {
        status: 429 | 503,
        retry_after: Some(wait),
    }) = &first
    {
        sleep((*wait).min(MAX_RETRY_WAIT)).await;
        return client.fetch_text(url, want, auth.as_deref(), arrival).await;
    }
    first
}

/// L'exploration après la page de départ.
struct Crawler {
    client: Client,
    engine: Engine,
    shared: Arc<Shared>,
    judge: Arc<Judge>,
    /// Identifiants des sites déjà lus dans le coffre, par identifiant.
    logins: HashMap<String, Arc<Auth>>,
    /// Pages déjà jugées (lues, en attente ou écartées).
    considered: HashSet<String>,
    /// La page de départ, avant et après ses redirections : jamais dans la liste.
    start: HashSet<String>,
    queue: VecDeque<(Url, u8)>,
    pages_read: u32,
    bytes_read: u64,
    stop: Option<Stop>,
    max_pages: u32,
    spacing: Duration,
}

/// Une lecture finie : la page, son niveau, la réponse.
type Done = (Url, u8, Result<Fetched, NetError>);

impl Crawler {
    /// Lit les pages en attente jusqu'à la fin ou à une limite.
    async fn explore(&mut self) -> Stop {
        let mut running: JoinSet<Done> = JoinSet::new();
        let mut next_start = Instant::now() + self.spacing;
        loop {
            self.check_limits();
            if self.stop.is_none() && !self.queue.is_empty() && running.len() < PARALLEL {
                if Instant::now() >= next_start {
                    let (url, depth) = self.queue.pop_front().expect("page en attente");
                    let auth = self
                        .engine
                        .crawl_login(url.as_str(), &mut self.logins)
                        .await;
                    self.pages_read += 1;
                    let (client, judge) = (self.client.clone(), Arc::clone(&self.judge));
                    running.spawn(async move {
                        let want = TextWant {
                            limit: MAX_INPUT,
                            truncate: false,
                            html_only: true,
                        };
                        let arrival = move |u: &Url| judge.refuses(u).is_none();
                        let result = fetch(&client, &url, want, auth, &arrival).await;
                        (url, depth, result)
                    });
                    next_start = Instant::now() + self.spacing;
                    self.publish(running.len());
                    continue;
                }
                if running.is_empty() {
                    sleep_until(next_start).await;
                    continue;
                }
                tokio::select! {
                    done = running.join_next() => {
                        if let Some(Ok(done)) = done {
                            self.done(done);
                        }
                        self.publish(running.len());
                    }
                    () = sleep_until(next_start) => {}
                }
                continue;
            }
            match running.join_next().await {
                None => break,
                Some(Ok(done)) => self.done(done),
                // Une lecture qui a paniqué : sa page reste en attente, puis « limit ».
                Some(Err(_)) => {}
            }
            self.publish(running.len());
        }
        self.stop.unwrap_or(Stop::Finished)
    }

    fn check_limits(&mut self) {
        if self.stop.is_some() || self.queue.is_empty() {
            return;
        }
        if self.pages_read >= self.max_pages {
            self.stop = Some(Stop::MaxPages);
        } else if self.bytes_read >= MAX_BYTES {
            self.stop = Some(Stop::MaxBytes);
        }
    }

    /// Une lecture finie.
    fn done(&mut self, (url, depth, result): Done) {
        let asked = url.to_string();
        match result {
            Ok(Fetched::Body { final_url, bytes }) => {
                self.bytes_read += bytes.len() as u64;
                self.set_visit(&asked, Some(Visit::Read));
                // Une redirection vers une page déjà jugée : rien de plus à lire.
                let arrived = final_url.to_string();
                if arrived == asked || self.considered.insert(arrived) {
                    self.read(&final_url, &bytes, depth);
                }
            }
            // Un fichier, pas une page : il apparaît parmi les fichiers.
            Ok(Fetched::Other { .. }) => self.shared.update(|inner| {
                if let Some(&i) = inner.index.get(&asked) {
                    inner.links[i].link.kind = LinkKind::File;
                    inner.links[i].visit = None;
                }
            }),
            Ok(Fetched::Elsewhere { final_url }) => {
                let visit = self.judge.refuses(&final_url).unwrap_or(Visit::Offsite);
                self.set_visit(&asked, Some(visit));
            }
            Ok(Fetched::TooBig { .. } | Fetched::Status { .. }) | Err(_) => {
                self.set_visit(&asked, Some(Visit::Failed));
            }
        }
    }

    fn set_visit(&self, url: &str, visit: Option<Visit>) {
        self.shared.update(|inner| {
            if let Some(&i) = inner.index.get(url) {
                inner.links[i].visit = visit;
            }
        });
    }

    /// Les liens d'une page lue au niveau `depth`, et les pages à lire ensuite.
    fn read(&mut self, page: &Url, bytes: &[u8], depth: u8) {
        let html = String::from_utf8_lossy(bytes);
        let found = from_html_page(&html, page);
        let child = depth.saturating_add(1);
        let page_text = page.to_string();
        let (judge, considered, queue) = (&self.judge, &mut self.considered, &mut self.queue);
        let start = &self.start;
        let mut full = false;
        self.shared.update(|inner| {
            for link in found.links {
                let i = match inner.index.get(&link.url) {
                    Some(&i) => i,
                    // La page de départ n'est pas dans la liste.
                    None if start.contains(&link.url) => continue,
                    None => {
                        if inner.links.len() >= MAX_LINKS {
                            full = true;
                            break;
                        }
                        inner.index.insert(link.url.clone(), inner.links.len());
                        inner.links.push(Found {
                            site_path: site_path(&link.url),
                            depth: child,
                            found_on: page_text.clone(),
                            visit: None,
                            link,
                        });
                        inner.links.len() - 1
                    }
                };
                let found_link = &mut inner.links[i];
                if found_link.link.kind != LinkKind::Page
                    || !found.follow.contains(&found_link.link.url)
                    || !considered.insert(found_link.link.url.clone())
                {
                    continue;
                }
                let Ok(url) = Url::parse(&found_link.link.url) else {
                    continue;
                };
                let visit = judge.visit(&url, child);
                found_link.visit = Some(visit);
                if visit == Visit::Queued {
                    queue.push_back((url, child));
                }
            }
            inner.progress.links = inner.links.len();
        });
        if full && self.stop.is_none() {
            self.stop = Some(Stop::MaxLinks);
        }
    }

    /// Les nombres de l'avancement ; `in_flight` : lectures en cours.
    fn publish(&self, in_flight: usize) {
        let (pages_read, bytes_read) = (self.pages_read, self.bytes_read);
        let queued = (self.queue.len() + in_flight) as u32;
        self.shared.update(|inner| {
            let progress = &mut inner.progress;
            progress.pages_read = pages_read;
            progress.bytes_read = bytes_read;
            progress.pages_queued = if self.stop.is_some() {
                in_flight as u32
            } else {
                queued
            };
            progress.links = inner.links.len();
        });
    }
}

/// Sous-dossier qui garde l'arborescence du site : l'hôte (et son port), puis les dossiers
/// du chemin, décodés et nettoyés comme un nom de fichier ; 8 parties au plus.
pub fn site_path(url: &str) -> String {
    let Ok(url) = Url::parse(url) else {
        return String::new();
    };
    let Some(host) = url.host_str() else {
        return String::new();
    };
    let host = match url.port() {
        Some(port) => format!("{host}_{port}"),
        None => host.to_owned(),
    };
    let mut parts = Vec::new();
    let mut dirs: Vec<&str> = url
        .path_segments()
        .map(Iterator::collect)
        .unwrap_or_default();
    // Le dernier segment est le nom du fichier (vide pour un dossier).
    dirs.pop();
    for part in std::iter::once(host.as_str()).chain(dirs) {
        let decoded = percent_encoding::percent_decode_str(part).decode_utf8_lossy();
        let clean = sanitize(&decoded, NameRules::native());
        if clean.is_empty() || clean == "." || clean == ".." {
            continue;
        }
        parts.push(clean);
        if parts.len() == MAX_SITE_PARTS {
            break;
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests;
