//! Exécution d'une copie de site (SPEC §7.18 ter) : exploration, pages et ressources lues
//! sur le disque au fil de l'eau, fichiers cochés (tâches HTTP internes), puis réécriture
//! des liens en `post_processing`.
//!
//! Tout se reprend depuis la table des éléments (`store::site`) : un élément fait reste
//! fait, un élément en cours repart de zéro (son `.part` est vidé), les pages en attente
//! gardent leur niveau. Ordre d'écriture (SPEC §5.6, règle 1) : octets vidés sur disque,
//! puis l'élément enregistré.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Url;
use rusqlite::Connection;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};

use super::{css, html, layout};
use crate::claims;
use crate::io::{part_path, sync_parent_dir};
use crate::limit::Throttle;
use crate::linkscan::crawl::{Judge, MAX_CRAWL_DELAY, MAX_RETRY_WAIT, Visit, read_robots};
use crate::linkscan::robots::Robots;
use crate::linkscan::{MAX_INPUT, PAGE_EXTENSIONS, extension};
use crate::net::auth::Auth;
use crate::net::filename::{NameRules, name_from_url, sanitize};
use crate::net::{Client, NetError, Opened};
use crate::store::claims::ClaimKind;
use crate::store::now_ms;
use crate::store::site::{self, Item, ItemKind, ItemState, NewItem, Phase};
use crate::store::tasks::{
    self, Change, NewTask, PauseReason, TaskRow, TaskState, change_state, load_task, reserved_names,
};
use crate::transfer::{
    self, MAX_FAILURES, Outcome, Progress, TransferError, jitter, retry_delay, wait_stop,
};

/// Page lue au plus (comme l'exploration).
pub const MAX_PAGE: u64 = MAX_INPUT as u64;
/// Ressource d'affichage copiée au plus (décision).
pub const MAX_RESOURCE: u64 = 50 << 20;
/// Volume des pages et ressources, par défaut (décision).
pub const DEFAULT_BYTES: u64 = 1 << 30;
/// Volume le plus petit, et le plus grand (décision : 20 Gio).
pub const MIN_BYTES: u64 = 1 << 20;
pub const MAX_BYTES: u64 = 20 << 30;
/// Requêtes en même temps au plus vers un même hôte, et pour toute la copie.
const PER_HOST: usize = 2;
const IN_FLIGHT: usize = 4;
/// Erreurs de réseau de suite qui arrêtent l'exécution (`retry_wait`).
const NET_FAILURES: u32 = 3;

/// Ce dont une exécution a besoin.
pub struct Context<'a> {
    pub client: &'a Client,
    pub opts: &'a transfer::Options,
}

/// Fin d'une exécution, avant son enregistrement.
#[derive(Debug)]
enum End {
    Completed(PathBuf),
    Stopped,
    DiskFull(String),
    RetryLater { code: &'static str, detail: String },
    Failed { code: &'static str, detail: String },
}

fn failed(code: &'static str, detail: impl Into<String>) -> End {
    End::Failed {
        code,
        detail: detail.into(),
    }
}

/// Une erreur du disque : la place manque (pause `disk_full`), ou autre (`DISK_IO`).
fn disk(e: &std::io::Error) -> End {
    if e.kind() == std::io::ErrorKind::StorageFull {
        End::DiskFull("plus assez de place pour la copie du site".into())
    } else {
        failed("DISK_IO", e.to_string())
    }
}

/// Exécute la copie `task_id` jusqu'à sa fin ou son arrêt, et enregistre son issue.
pub async fn run(
    conn: &mut Connection,
    ctx: &Context<'_>,
    task_id: &str,
    mut stop: watch::Receiver<bool>,
    cause: &transfer::StopCause,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Outcome, TransferError> {
    let task = load_task(conn, task_id)?;
    match task.state {
        TaskState::Queued | TaskState::RetryWait => {
            change_state(conn, task_id, &Change::to(TaskState::Active))?;
        }
        TaskState::Active | TaskState::PostProcessing => {}
        other => {
            return Err(TransferError::NotRunnable {
                task_id: task_id.to_owned(),
                state: other.as_sql(),
            });
        }
    }
    let end = copy(conn, ctx, task_id, &mut stop, on_progress).await?;
    record(conn, task_id, end, cause)
}

/// Enregistre l'issue, comme `transfer::run` pour une tâche HTTP.
fn record(
    conn: &mut Connection,
    task_id: &str,
    end: End,
    cause: &transfer::StopCause,
) -> Result<Outcome, TransferError> {
    match end {
        End::Completed(path) => Ok(Outcome::Completed(path)),
        End::Stopped => {
            let change = Change {
                pause_reason: Some(cause.get()),
                ..Change::to(TaskState::Paused)
            };
            change_state(conn, task_id, &change)?;
            Ok(Outcome::Paused)
        }
        End::DiskFull(detail) => {
            let change = Change {
                pause_reason: Some(PauseReason::DiskFull),
                error: Some(("DISK_FULL", &detail)),
                ..Change::to(TaskState::Paused)
            };
            change_state(conn, task_id, &change)?;
            Ok(Outcome::DiskFull)
        }
        End::RetryLater { code, detail } => {
            let task = load_task(conn, task_id)?;
            if task.retry_count + 1 >= MAX_FAILURES {
                return record(conn, task_id, End::Failed { code, detail }, cause);
            }
            let delay = retry_delay(task.retry_count, None, jitter());
            let at_ms =
                now_ms().saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX));
            let change = Change {
                error: Some((code, &detail)),
                next_retry_at: Some(at_ms),
                ..Change::to(TaskState::RetryWait)
            };
            change_state(conn, task_id, &change)?;
            Ok(Outcome::RetryLater {
                code,
                detail,
                at_ms,
            })
        }
        End::Failed { code, detail } => {
            let change = Change {
                error: Some((code, &detail)),
                ..Change::to(TaskState::Failed)
            };
            change_state(conn, task_id, &change)?;
            Ok(Outcome::Failed { code, detail })
        }
    }
}

/// Octets écrits par la copie : éléments lus ou faits.
fn written(conn: &Connection, task_id: &str) -> Result<u64, TransferError> {
    Ok(site::items(conn, task_id)?
        .iter()
        .filter(|i| matches!(i.state, ItemState::Fetched | ItemState::Done))
        .filter_map(|i| i.size)
        .sum())
}

/// Temps entre deux relevés des nombres d'éléments pendant l'exploration (une requête
/// sur tous les éléments de la copie).
const COUNTS_EVERY: Duration = Duration::from_millis(500);

/// Avancement de la copie : octets écrits, et ses nombres d'éléments (`task.progress`).
fn progress(received: u64, merging: bool, counts: site::Counts) -> Progress {
    Progress {
        received,
        total: None,
        connections: 0,
        segments: Arc::from([]),
        verified: None,
        uploaded: None,
        upload_bps: None,
        seeding: false,
        merging,
        site: Some(counts),
    }
}

async fn copy(
    conn: &mut Connection,
    ctx: &Context<'_>,
    task_id: &str,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<End, TransferError> {
    // Retéléchargement (`task.restart`) : la préparation est oubliée (dossier), la copie
    // repart de zéro ; ses fichiers ont été effacés avant.
    if load_task(conn, task_id)?.file_name.is_none() {
        let job = site::load_job(conn, task_id)?;
        let fresh = job.phase == Phase::Copy
            && site::items(conn, task_id)?
                .iter()
                .all(|i| i.state == ItemState::Queued && i.path.is_none() && i.child_id.is_none());
        if !fresh {
            site::reset(conn, task_id)?;
        }
    }
    let root = match root(conn, task_id)? {
        Ok(root) => root,
        Err(end) => return Ok(end),
    };
    if site::load_job(conn, task_id)?.phase == Phase::Copy {
        let mut explorer = Explorer::new(conn, ctx, task_id, &root)?;
        if let Some(end) = explorer.explore(conn, stop, on_progress).await? {
            return Ok(end);
        }
        site::set_phase(conn, task_id, Phase::Files)?;
    }
    if site::load_job(conn, task_id)?.phase == Phase::Files {
        if let Some(end) = files(conn, ctx, task_id, &root, stop, on_progress).await? {
            return Ok(end);
        }
        site::set_phase(conn, task_id, Phase::Links)?;
    }
    if load_task(conn, task_id)?.state != TaskState::PostProcessing {
        change_state(conn, task_id, &Change::to(TaskState::PostProcessing))?;
    }
    // Annoncé par le planificateur, comme la fusion d'une vidéo.
    on_progress(progress(
        written(conn, task_id)?,
        true,
        site::counts(conn, task_id)?,
    ));
    links(conn, task_id, &root).await
}

/// Le dossier de la copie, réservé à la première exécution (`<dossier>/<hôte>`, ou
/// `<hôte> (1)`…, SPEC §7.18 ter).
fn root(conn: &mut Connection, task_id: &str) -> Result<Result<PathBuf, End>, TransferError> {
    let task = load_task(conn, task_id)?;
    let dest = PathBuf::from(&task.dest_dir);
    if let Some(name) = &task.file_name {
        return Ok(Ok(dest.join(name)));
    }
    if let Err(e) = std::fs::create_dir_all(&dest) {
        return Ok(Err(disk(&e)));
    }
    let job = site::load_job(conn, task_id)?;
    let wanted = match Url::parse(&job.page_url) {
        Ok(url) => layout::parts_of(&url, layout::Content::Other)
            .into_iter()
            .next()
            .unwrap_or_else(|| task.display_name.clone()),
        Err(_) => sanitize(&task.display_name, NameRules::native()),
    };
    let reserved = reserved_names(conn, &task.dest_dir, task_id)?;
    let name = match claims::claim(conn, &dest, &wanted, &reserved, ClaimKind::Dir, task_id) {
        Ok(name) => name,
        Err(claims::ClaimError::Store(e)) => return Err(e.into()),
        Err(claims::ClaimError::Disk(e)) => return Ok(Err(disk(&e))),
    };
    site::set_root(conn, task_id, &name)?;
    Ok(Ok(dest.join(name)))
}

/// Le chemin d'un élément sur le disque.
fn on_disk(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

/// Ce que donne la lecture d'un élément.
#[derive(Debug)]
enum Fetch {
    /// Lu en entier et vidé sur disque dans son `.part`.
    Done {
        final_url: Url,
        size: u64,
        charset: Option<String>,
    },
    Status {
        status: u16,
        retry_after: Option<Duration>,
    },
    /// Une page qui n'est pas du HTML.
    NotHtml,
    /// Une page dont la redirection mène à une page refusée : corps non lu.
    Elsewhere,
    TooBig,
    Net(NetError),
    Disk(std::io::Error),
}

/// Une lecture à lancer.
struct Request {
    url: Url,
    part: PathBuf,
    limit: u64,
    /// Une page : HTML seulement, arrivée jugée (sauf la page de départ).
    page: bool,
    judge: Option<Arc<Judge>>,
    auth: Option<Arc<Auth>>,
}

/// Lit un élément dans son `.part` (vidé d'abord), dans la limite de taille et de débit.
async fn fetch(client: Client, throttle: Throttle, request: Request) -> Fetch {
    let opened = match client.open_get(&request.url, request.auth.as_deref()).await {
        Ok(opened) => opened,
        Err(e) => return Fetch::Net(e),
    };
    let (final_url, content_type, charset, length, attachment, mut body) = match opened {
        Opened::Status {
            status,
            retry_after,
        } => {
            return Fetch::Status {
                status,
                retry_after,
            };
        }
        Opened::Ok {
            final_url,
            content_type,
            charset,
            length,
            attachment,
            body,
        } => (final_url, content_type, charset, length, attachment, body),
    };
    if request.page {
        let html = !attachment
            && content_type
                .as_deref()
                .is_none_or(|t| matches!(t, "text/html" | "application/xhtml+xml"));
        if !html {
            return Fetch::NotHtml;
        }
        if let Some(judge) = &request.judge
            && judge.refuses(&final_url).is_some()
        {
            return Fetch::Elsewhere;
        }
    }
    if length.is_some_and(|l| l > request.limit) {
        return Fetch::TooBig;
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&request.part)
    {
        Ok(file) => file,
        Err(e) => return Fetch::Disk(e),
    };
    let mut size: u64 = 0;
    loop {
        throttle.ready().await;
        let chunk = match body.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(e) => return Fetch::Net(e),
        };
        throttle.pay(chunk.len());
        size += chunk.len() as u64;
        if size > request.limit {
            return Fetch::TooBig;
        }
        if let Err(e) = file.write_all(&chunk) {
            return Fetch::Disk(e);
        }
    }
    // Règle 1 : les octets sur le disque avant l'enregistrement de l'élément.
    let synced = tokio::task::spawn_blocking(move || file.sync_data()).await;
    match synced {
        Ok(Ok(())) => Fetch::Done {
            final_url,
            size,
            charset: if request.page { charset } else { None },
        },
        Ok(Err(e)) => Fetch::Disk(e),
        Err(e) => Fetch::Disk(std::io::Error::other(e)),
    }
}

/// Une page à suivre ? Une adresse de sorte page (extension de page web, ou aucune),
/// comme pour l'analyse d'une page (SPEC §7.18).
fn page_like(url: &Url) -> bool {
    match name_from_url(url).as_deref().and_then(extension) {
        Some(ext) => PAGE_EXTENSIONS.contains(&ext.as_str()),
        None => true,
    }
}

/// L'exploration d'une copie et la lecture de ses pages et ressources.
struct Explorer {
    task_id: String,
    root: PathBuf,
    client: Client,
    throttle: Throttle,
    spacing: Duration,
    public_only: bool,
    vault: crate::secrets::Vault,
    job: site::SiteJob,
    exclude: Option<regex::Regex>,
    judge: Option<Arc<Judge>>,
    /// Éléments à lire, dans l'ordre de leur découverte.
    queue: VecDeque<Item>,
    last_seq: i64,
    /// Pages lues (réponses comprises) et octets enregistrés (pages et ressources).
    pages_read: u32,
    bytes: u64,
    /// Octets écrits en tout, fichiers cochés compris.
    written: u64,
    /// Par hôte : lectures en cours, début de la dernière.
    running: HashMap<String, usize>,
    started: HashMap<String, Instant>,
    /// Éléments en cours : leur hôte, et s'ils sont des pages.
    in_flight: HashMap<i64, (String, bool)>,
    /// Éléments déjà redemandés après `429` ou `503`, et quand.
    retried: HashSet<i64>,
    not_before: HashMap<i64, Instant>,
    net_failures: u32,
    logins: HashMap<String, Option<Arc<Auth>>>,
    stopped: bool,
    /// Derniers nombres d'éléments annoncés, et quand ils ont été relevés.
    counts: Option<(Instant, site::Counts)>,
}

impl Explorer {
    fn new(
        conn: &Connection,
        ctx: &Context<'_>,
        task_id: &str,
        root: &Path,
    ) -> Result<Self, TransferError> {
        let task = load_task(conn, task_id)?;
        let job = site::load_job(conn, task_id)?;
        let items = site::items(conn, task_id)?;
        // Vérifiée à l'ajout.
        let exclude = job
            .settings
            .exclude
            .as_deref()
            .and_then(|pattern| crate::rules::compile(pattern).ok());
        let pages_read = items
            .iter()
            .filter(|i| {
                i.kind == ItemKind::Page
                    && matches!(
                        i.state,
                        ItemState::Fetched | ItemState::Done | ItemState::Failed
                    )
            })
            .count();
        let bytes = items
            .iter()
            .filter(|i| i.kind != ItemKind::File)
            .filter(|i| matches!(i.state, ItemState::Fetched | ItemState::Done))
            .filter_map(|i| i.size)
            .sum();
        let written = items
            .iter()
            .filter(|i| matches!(i.state, ItemState::Fetched | ItemState::Done))
            .filter_map(|i| i.size)
            .sum();
        Ok(Explorer {
            task_id: task_id.to_owned(),
            root: root.to_path_buf(),
            client: ctx.client.clone(),
            throttle: ctx.opts.throttle.clone(),
            spacing: ctx.opts.site_spacing,
            public_only: task.public_only,
            vault: ctx.opts.vault.clone(),
            last_seq: items.last().map_or(-1, |i| i.seq),
            queue: items
                .into_iter()
                .filter(|i| i.state == ItemState::Queued && i.kind != ItemKind::File)
                .collect(),
            job,
            exclude,
            judge: None,
            pages_read: u32::try_from(pages_read).unwrap_or(u32::MAX),
            bytes,
            written,
            running: HashMap::new(),
            started: HashMap::new(),
            in_flight: HashMap::new(),
            retried: HashSet::new(),
            not_before: HashMap::new(),
            net_failures: 0,
            logins: HashMap::new(),
            stopped: false,
            counts: None,
        })
    }

    /// Le juge des pages, une fois la page de départ lue (robots.txt lu alors, et à chaque
    /// reprise).
    async fn set_judge(&mut self, start: &Url) {
        let robots = if self.job.settings.ignore_robots {
            Robots::allow_all()
        } else {
            read_robots(&self.client, start).await.0
        };
        let delay = robots.crawl_delay.unwrap_or_default().min(MAX_CRAWL_DELAY);
        self.spacing = self.spacing.max(delay);
        self.judge = Some(Arc::new(Judge::new(
            start,
            self.job.settings.depth,
            self.exclude.clone(),
            self.job.settings.same_folder,
            robots,
        )));
    }

    /// Nombres d'éléments de la copie, relevés au plus toutes les [`COUNTS_EVERY`].
    fn counts(&mut self, conn: &Connection) -> Result<site::Counts, TransferError> {
        let now = Instant::now();
        if let Some((at, counts)) = self.counts
            && now.duration_since(at) < COUNTS_EVERY
        {
            return Ok(counts);
        }
        let counts = site::counts(conn, &self.task_id)?;
        self.counts = Some((now, counts));
        Ok(counts)
    }

    /// Identifiants du site d'une adresse : canal local seulement (SPEC §7.18 bis).
    async fn login(&mut self, conn: &mut Connection, url: &Url) -> Option<Arc<Auth>> {
        if self.public_only {
            return None;
        }
        let host = url.host_str()?.to_owned();
        if let Some(known) = self.logins.get(&host) {
            return known.clone();
        }
        let row = transfer::login_for(conn, url.as_str(), false)
            .ok()
            .flatten();
        let auth = transfer::reveal_auth(row, &self.vault)
            .await
            .ok()
            .flatten()
            .map(Arc::new);
        self.logins.insert(host, auth.clone());
        auth
    }

    async fn explore(
        &mut self,
        conn: &mut Connection,
        stop: &mut watch::Receiver<bool>,
        on_progress: &mut (dyn FnMut(Progress) + Send),
    ) -> Result<Option<End>, TransferError> {
        // Reprise : la page de départ est déjà lue.
        if let Some(start) = self.job.start_url.clone()
            && let Ok(start) = Url::parse(&start)
        {
            self.set_judge(&start).await;
        }
        let mut set: JoinSet<(i64, Fetch)> = JoinSet::new();
        loop {
            // Lance ce qui peut l'être.
            loop {
                self.check_limits(conn)?;
                let Some(index) = self.next_ready() else {
                    break;
                };
                let item = self.queue.remove(index).expect("élément choisi");
                if let Some(end) = self.launch(conn, &mut set, item).await? {
                    set.abort_all();
                    return Ok(Some(end));
                }
            }
            if set.is_empty() && self.queue.is_empty() {
                break;
            }
            let wake = self.next_wake();
            tokio::select! {
                joined = set.join_next(), if !set.is_empty() => {
                    let Some(joined) = joined else { continue };
                    let (seq, fetched) = match joined {
                        Ok(pair) => pair,
                        Err(e) => {
                            set.abort_all();
                            return Ok(Some(failed("INTERNAL", e.to_string())));
                        }
                    };
                    if let Some(end) = self.handle(conn, seq, fetched).await? {
                        set.abort_all();
                        return Ok(Some(end));
                    }
                    site::save_progress(conn, &self.task_id, self.written)?;
                    let counts = self.counts(conn)?;
                    on_progress(progress(self.written, false, counts));
                }
                () = sleep_until(wake.unwrap_or_else(Instant::now)), if wake.is_some() => {}
                () = wait_stop(stop) => {
                    // Les éléments en cours restent en attente : leur `.part` sera vidé.
                    set.abort_all();
                    return Ok(Some(End::Stopped));
                }
            }
        }
        if !self.stopped {
            site::set_stop(conn, &self.task_id, "finished")?;
        }
        let (pages, items) = (self.pages_read, self.last_seq + 1);
        tracing::info!(tache = %self.task_id, pages, items, "copie d'un site : exploration finie");
        Ok(None)
    }

    /// Limites de pages et de volume : l'exploration s'arrête (SPEC §7.18 ter).
    fn check_limits(&mut self, conn: &Connection) -> Result<(), TransferError> {
        if self.bytes >= self.job.settings.max_bytes {
            self.stop_exploring(conn, "maxBytes", false)?;
        }
        let pages_running = self.in_flight.values().filter(|(_, page)| *page).count();
        let pages = self.pages_read as usize + pages_running;
        if pages >= self.job.settings.max_pages as usize
            && self.queue.iter().any(|i| i.kind == ItemKind::Page)
        {
            self.stop_exploring(conn, "maxPages", true)?;
        }
        Ok(())
    }

    fn stop_exploring(
        &mut self,
        conn: &Connection,
        why: &str,
        pages_only: bool,
    ) -> Result<(), TransferError> {
        site::set_stop(conn, &self.task_id, why)?;
        site::skip_queued(conn, &self.task_id, pages_only)?;
        self.queue
            .retain(|i| pages_only && i.kind != ItemKind::Page);
        self.stopped = true;
        Ok(())
    }

    fn host_of(url: &str) -> String {
        Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// Le premier élément qui peut partir : place libre pour son hôte et pour la copie,
    /// pause écoulée depuis la lecture précédente sur son hôte. Avant la lecture de la page
    /// de départ, rien d'autre.
    fn next_ready(&self) -> Option<usize> {
        if self.in_flight.len() >= IN_FLIGHT {
            return None;
        }
        let now = Instant::now();
        for (index, item) in self.queue.iter().enumerate() {
            if self.judge.is_none() && item.seq != 0 {
                continue;
            }
            if self.not_before.get(&item.seq).is_some_and(|&t| t > now) {
                continue;
            }
            let host = Self::host_of(&item.url);
            if self.running.get(&host).copied().unwrap_or(0) >= PER_HOST {
                continue;
            }
            if self
                .started
                .get(&host)
                .is_some_and(|&t| t + self.spacing > now)
            {
                continue;
            }
            return Some(index);
        }
        None
    }

    /// Le prochain instant où un élément en attente pourra partir, s'il n'attend que le
    /// temps ; une place qui se libère réveille par la fin d'une lecture.
    fn next_wake(&self) -> Option<Instant> {
        if self.in_flight.len() >= IN_FLIGHT {
            return None;
        }
        let mut wake: Option<Instant> = None;
        for item in &self.queue {
            if self.judge.is_none() && item.seq != 0 {
                continue;
            }
            let host = Self::host_of(&item.url);
            if self.running.get(&host).copied().unwrap_or(0) >= PER_HOST {
                continue;
            }
            let mut at = self
                .started
                .get(&host)
                .map_or_else(Instant::now, |&t| t + self.spacing);
            if let Some(&later) = self.not_before.get(&item.seq) {
                at = at.max(later);
            }
            wake = Some(wake.map_or(at, |w| w.min(at)));
        }
        wake
    }

    /// Réserve le nom d'un élément (s'il n'en a pas), puis lance sa lecture.
    async fn launch(
        &mut self,
        conn: &mut Connection,
        set: &mut JoinSet<(i64, Fetch)>,
        mut item: Item,
    ) -> Result<Option<End>, TransferError> {
        let Ok(url) = Url::parse(&item.url) else {
            site::give_up(
                conn,
                &self.task_id,
                item.seq,
                ItemState::Failed,
                Some("URL_INVALID"),
            )?;
            return Ok(None);
        };
        let content = match item.kind {
            ItemKind::Page => layout::Content::Html,
            ItemKind::Style => layout::Content::Css,
            ItemKind::Resource | ItemKind::File => layout::Content::Other,
        };
        if item.path.is_none() {
            match self.reserve(conn, &url, content, item.seq)? {
                Ok(path) => item.path = Some(path),
                Err(Some(end)) => return Ok(Some(end)),
                Err(None) => return Ok(None),
            }
        }
        let path = item.path.clone().unwrap_or_default();
        let auth = self.login(conn, &url).await;
        let host = Self::host_of(&item.url);
        let page = item.kind == ItemKind::Page;
        let request = Request {
            part: part_path(&on_disk(&self.root, &path)),
            limit: match item.kind {
                ItemKind::Page => MAX_PAGE,
                _ => MAX_RESOURCE,
            },
            page,
            // La page de départ, demandée par l'utilisateur : où qu'elle mène.
            judge: if item.seq == 0 {
                None
            } else {
                self.judge.clone()
            },
            auth,
            url,
        };
        *self.running.entry(host.clone()).or_default() += 1;
        self.started.insert(host.clone(), Instant::now());
        self.in_flight.insert(item.seq, (host, page));
        let (client, throttle, seq) = (self.client.clone(), self.throttle.clone(), item.seq);
        set.spawn(async move { (seq, fetch(client, throttle, request).await) });
        Ok(None)
    }

    /// Le nom d'un élément, réservé dans son dossier et enregistré (SPEC §5.6, règle 7).
    /// `Err(None)` : l'élément est laissé en ligne ; `Err(Some(fin))` : la copie s'arrête.
    fn reserve(
        &self,
        conn: &mut Connection,
        url: &Url,
        content: layout::Content,
        seq: i64,
    ) -> Result<Result<String, Option<End>>, TransferError> {
        let parts = layout::parts_of(url, content);
        let (name, dirs) = parts.split_last().expect("au moins l'hôte et un nom");
        let dir = dirs.iter().fold(self.root.clone(), |p, d| p.join(d));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            if e.kind() == std::io::ErrorKind::StorageFull {
                return Ok(Err(Some(disk(&e))));
            }
            // Un fichier de la copie porte déjà le nom d'un de ses dossiers.
            site::give_up(conn, &self.task_id, seq, ItemState::Failed, Some("DISK_IO"))?;
            return Ok(Err(None));
        }
        let taken = match claims::claim(conn, &dir, name, &[], ClaimKind::Part, &self.task_id) {
            Ok(taken) => taken,
            Err(claims::ClaimError::Store(e)) => return Err(e.into()),
            Err(claims::ClaimError::Disk(e)) => return Ok(Err(Some(disk(&e)))),
        };
        let mut path: Vec<&str> = dirs.iter().map(String::as_str).collect();
        path.push(&taken);
        let path = path.join("/");
        site::set_path(conn, &self.task_id, seq, &path)?;
        Ok(Ok(path))
    }

    /// Une lecture finie.
    async fn handle(
        &mut self,
        conn: &mut Connection,
        seq: i64,
        fetched: Fetch,
    ) -> Result<Option<End>, TransferError> {
        let (host, _) = self.in_flight.remove(&seq).unwrap_or_default();
        if let Some(n) = self.running.get_mut(&host) {
            *n = n.saturating_sub(1);
        }
        let item = site::item(conn, &self.task_id, seq)?;
        let path = on_disk(&self.root, item.path.as_deref().unwrap_or_default());
        let part = part_path(&path);
        if item.kind == ItemKind::Page && !matches!(fetched, Fetch::Net(_)) {
            self.pages_read += 1;
        }
        let task_id = self.task_id.clone();
        let give_up = |conn: &Connection, code: &str| -> Result<(), TransferError> {
            let _ = std::fs::remove_file(&part);
            site::give_up(conn, &task_id, seq, ItemState::Failed, Some(code))?;
            Ok(())
        };
        match fetched {
            Fetch::Done {
                final_url,
                size,
                charset,
            } => {
                self.net_failures = 0;
                self.bytes += size;
                self.written += size;
                match item.kind {
                    ItemKind::Page | ItemKind::Style => {
                        if seq == 0 {
                            site::set_start(
                                conn,
                                &self.task_id,
                                final_url.as_str(),
                                item.path.as_deref().unwrap_or_default(),
                            )?;
                            self.set_judge(&final_url).await;
                        }
                        let found = match std::fs::read(&part) {
                            Ok(bytes) => self.found(&item, &final_url, &bytes),
                            Err(e) => return Ok(Some(disk(&e))),
                        };
                        let found: Vec<NewItem<'_>> = found
                            .iter()
                            .map(|(url, kind, depth)| NewItem {
                                url,
                                kind: *kind,
                                depth: *depth,
                            })
                            .collect();
                        let final_text = final_url.as_str();
                        let added = site::fetched(
                            conn,
                            &self.task_id,
                            seq,
                            (final_text != item.url).then_some(final_text),
                            size,
                            charset.as_deref(),
                            &found,
                        )?;
                        if added.full && !self.stopped {
                            site::set_stop(conn, &self.task_id, "maxItems")?;
                            self.stopped = true;
                        }
                        self.load_new(conn)?;
                    }
                    ItemKind::Resource | ItemKind::File => {
                        let renamed = std::fs::rename(&part, &path)
                            .map_err(|e| e.to_string())
                            .and_then(|()| sync_parent_dir(&path).map_err(|e| e.to_string()));
                        if let Err(e) = renamed {
                            return Ok(Some(failed("DISK_IO", e)));
                        }
                        crate::io::mark_from_internet(&path);
                        let final_text = final_url.as_str();
                        site::done(
                            conn,
                            &self.task_id,
                            seq,
                            (final_text != item.url).then_some(final_text),
                            size,
                        )?;
                    }
                }
            }
            Fetch::Status {
                status,
                retry_after,
            } => {
                if matches!(status, 429 | 503)
                    && let Some(wait) = retry_after
                    && self.retried.insert(seq)
                {
                    self.not_before
                        .insert(seq, Instant::now() + wait.min(MAX_RETRY_WAIT));
                    self.requeue(item);
                    return Ok(None);
                }
                if seq == 0 {
                    let e = NetError::Status {
                        status,
                        retry_after,
                    };
                    let detail = e.to_string();
                    return Ok(Some(if e.retryable() {
                        End::RetryLater {
                            code: "HTTP_STATUS",
                            detail,
                        }
                    } else {
                        failed("HTTP_STATUS", detail)
                    }));
                }
                give_up(conn, "HTTP_STATUS")?;
            }
            Fetch::NotHtml if seq == 0 => {
                return Ok(Some(failed(
                    "SITE_NOT_HTML",
                    "la page de départ n'est pas une page web (HTML)",
                )));
            }
            Fetch::NotHtml => give_up(conn, "NOT_HTML")?,
            Fetch::Elsewhere => give_up(conn, "REDIRECTED")?,
            Fetch::TooBig if seq == 0 => {
                return Ok(Some(failed(
                    "SITE_NOT_HTML",
                    format!("page de départ de plus de {} Mio", MAX_PAGE >> 20),
                )));
            }
            Fetch::TooBig => give_up(conn, "TOO_BIG")?,
            Fetch::Net(e) => {
                let code = e.error_code().unwrap_or("NET_UNREACHABLE");
                if e.retryable() {
                    // Le réseau, pas l'élément : il reste en attente.
                    self.net_failures += 1;
                    if seq == 0 || self.net_failures >= NET_FAILURES {
                        return Ok(Some(End::RetryLater {
                            code,
                            detail: e.to_string(),
                        }));
                    }
                    self.not_before
                        .insert(seq, Instant::now() + Duration::from_secs(1));
                    self.requeue(item);
                    return Ok(None);
                }
                if seq == 0 {
                    return Ok(Some(failed(code, e.to_string())));
                }
                give_up(conn, code)?;
            }
            Fetch::Disk(e) => return Ok(Some(disk(&e))),
        }
        Ok(None)
    }

    /// Un élément remis en tête de l'attente.
    fn requeue(&mut self, item: Item) {
        self.queue.push_front(item);
    }

    /// Les éléments ajoutés depuis la dernière lecture de la table.
    fn load_new(&mut self, conn: &Connection) -> Result<(), TransferError> {
        let fresh: Vec<Item> = site::items(conn, &self.task_id)?
            .into_iter()
            .filter(|i| i.seq > self.last_seq)
            .collect();
        if let Some(last) = fresh.last() {
            self.last_seq = last.seq;
        }
        self.queue.extend(
            fresh
                .into_iter()
                .filter(|i| i.state == ItemState::Queued && i.kind != ItemKind::File)
                .filter(|i| !(self.stopped && i.kind == ItemKind::Page)),
        );
        Ok(())
    }

    /// Les liens d'une page ou d'un style lus : pages à suivre (même site, profondeur,
    /// robots.txt, exclusions), ressources d'affichage et styles de tout hôte.
    fn found(
        &self,
        item: &Item,
        final_url: &Url,
        bytes: &[u8],
    ) -> Vec<(String, ItemKind, Option<u8>)> {
        let mut out = Vec::new();
        if item.kind == ItemKind::Style {
            for r in css::refs(bytes) {
                if let Some(url) = html::target(final_url, &r.url) {
                    let kind = if r.import {
                        ItemKind::Style
                    } else {
                        ItemKind::Resource
                    };
                    out.push((url.to_string(), kind, None));
                }
            }
            return out;
        }
        if !html::rewritable(bytes) {
            return out;
        }
        let depth = item.depth.unwrap_or(0).saturating_add(1);
        let scan = html::scan(bytes);
        for (url, role) in html::links(bytes, &scan, final_url) {
            match role {
                html::Role::Navigate => {
                    let follow = !self.stopped
                        && page_like(&url)
                        && self
                            .judge
                            .as_ref()
                            .is_some_and(|j| j.visit(&url, depth) == Visit::Queued);
                    if follow {
                        out.push((url.to_string(), ItemKind::Page, Some(depth)));
                    }
                }
                html::Role::Resource => out.push((url.to_string(), ItemKind::Resource, None)),
                html::Role::Style => out.push((url.to_string(), ItemKind::Style, None)),
                html::Role::Other => {}
            }
        }
        out
    }
}

/// Les fichiers cochés, chacun par sa tâche HTTP interne, un à la fois.
async fn files(
    conn: &mut Connection,
    ctx: &Context<'_>,
    task_id: &str,
    root: &Path,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Option<End>, TransferError> {
    let task = load_task(conn, task_id)?;
    let waiting: Vec<Item> = site::items(conn, task_id)?
        .into_iter()
        .filter(|i| i.kind == ItemKind::File && i.state == ItemState::Queued)
        .collect();
    for item in waiting {
        let base = written(conn, task_id)?;
        if let Some(end) = file(conn, ctx, &task, root, &item, stop, base, on_progress).await? {
            return Ok(Some(end));
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
async fn file(
    conn: &mut Connection,
    ctx: &Context<'_>,
    task: &TaskRow,
    root: &Path,
    item: &Item,
    stop: &mut watch::Receiver<bool>,
    base: u64,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Option<End>, TransferError> {
    let Ok(url) = Url::parse(&item.url) else {
        site::give_up(
            conn,
            &task.id,
            item.seq,
            ItemState::Failed,
            Some("URL_INVALID"),
        )?;
        return Ok(None);
    };
    let parts = layout::parts_of(&url, layout::Content::Other);
    let (name, dirs) = parts.split_last().expect("au moins l'hôte et un nom");
    let dir = dirs.iter().fold(root.to_path_buf(), |p, d| p.join(d));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        if e.kind() == std::io::ErrorKind::StorageFull {
            return Ok(Some(disk(&e)));
        }
        site::give_up(conn, &task.id, item.seq, ItemState::Failed, Some("DISK_IO"))?;
        return Ok(None);
    }
    let child_id = match &item.child_id {
        Some(id) => id.clone(),
        None => {
            let dir_text = dir.to_string_lossy();
            let (id, _) = tasks::create_task(
                conn,
                &NewTask {
                    queue_id: &task.queue_id,
                    url: &item.url,
                    display_name: name,
                    dest_dir: &dir_text,
                    max_connections: task.max_connections,
                    speed_limit_bps: None,
                    idempotency_key: None,
                    requested_name: Some(name),
                    checksum: None,
                    headers: &[],
                    parent_id: Some(&task.id),
                    public_only: task.public_only,
                },
            )?;
            site::set_child(conn, &task.id, item.seq, &id)?;
            id
        }
    };
    let mut restarted = false;
    loop {
        let child = load_task(conn, &child_id)?;
        match child.state {
            TaskState::Completed => {
                return done_file(conn, task, root, item, &child, base).map(|()| None);
            }
            TaskState::Paused | TaskState::Failed => {
                change_state(conn, &child_id, &Change::to(TaskState::Queued))?;
            }
            _ => {}
        }
        // Les nombres ne changent pas pendant un fichier.
        let counts = site::counts(conn, &task.id)?;
        let mut forward = |p: Progress| on_progress(progress(base + p.received, false, counts));
        let outcome = transfer::run(
            conn,
            ctx.client,
            &child_id,
            ctx.opts,
            stop.clone(),
            &mut forward,
        )
        .await?;
        return Ok(match outcome {
            Outcome::Completed(_) => {
                let child = load_task(conn, &child_id)?;
                done_file(conn, task, root, item, &child, base)?;
                None
            }
            Outcome::Paused => Some(End::Stopped),
            Outcome::DiskFull => Some(End::DiskFull(
                "plus assez de place pour un fichier de la copie".into(),
            )),
            Outcome::FileChanged if !restarted => {
                restarted = true;
                if let Some(name) = &load_task(conn, &child_id)?.file_name {
                    let _ = std::fs::remove_file(part_path(&dir.join(name)));
                }
                tasks::restart_task(conn, &child_id)?;
                continue;
            }
            Outcome::FileChanged => {
                site::give_up(
                    conn,
                    &task.id,
                    item.seq,
                    ItemState::Failed,
                    Some("FILE_CHANGED"),
                )?;
                None
            }
            Outcome::RetryLater { code, detail, .. } => Some(End::RetryLater { code, detail }),
            // Un fichier coché mort : son lien reste en ligne, comme une page morte.
            Outcome::Failed { code, .. } => {
                site::give_up(conn, &task.id, item.seq, ItemState::Failed, Some(code))?;
                None
            }
            Outcome::Seeding | Outcome::PostProcessing => Some(failed(
                "INTERNAL",
                "issue inattendue pour un fichier de la copie",
            )),
        });
    }
}

/// Un fichier coché terminé : son chemin dans la copie.
fn done_file(
    conn: &mut Connection,
    task: &TaskRow,
    root: &Path,
    item: &Item,
    child: &TaskRow,
    base: u64,
) -> Result<(), TransferError> {
    let full = Path::new(&child.dest_dir).join(child.file_name.as_deref().unwrap_or_default());
    let relative = full
        .strip_prefix(root)
        .map(|p| {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default();
    let size = child.total_bytes.unwrap_or(child.done_bytes);
    site::set_path(conn, &task.id, item.seq, &relative)?;
    site::done(conn, &task.id, item.seq, None, size)?;
    site::save_progress(conn, &task.id, base + size)?;
    Ok(())
}

/// Réécriture des liens de chaque page et de chaque style lus, puis fin de la copie.
async fn links(conn: &mut Connection, task_id: &str, root: &Path) -> Result<End, TransferError> {
    let items = site::items(conn, task_id)?;
    // Adresse (demandée ou d'arrivée) d'un élément copié : son chemin.
    let mut copied: HashMap<String, String> = HashMap::new();
    for item in &items {
        if !matches!(item.state, ItemState::Fetched | ItemState::Done) {
            continue;
        }
        let Some(path) = &item.path else { continue };
        if let Some(final_url) = &item.final_url {
            copied
                .entry(final_url.clone())
                .or_insert_with(|| path.clone());
        }
        copied
            .entry(item.url.clone())
            .or_insert_with(|| path.clone());
    }
    for item in items.iter().filter(|i| i.state == ItemState::Fetched) {
        let Some(path) = item.path.clone() else {
            continue;
        };
        let target = on_disk(root, &path);
        let part = part_path(&target);
        let raw = match std::fs::read(&part) {
            Ok(raw) => raw,
            Err(e) => return Ok(disk(&e)),
        };
        let Ok(page) = Url::parse(item.final_url.as_deref().unwrap_or(&item.url)) else {
            continue;
        };
        let mut local = |url: &Url| {
            copied
                .get(url.as_str())
                .map(|to| layout::relative(&path, to))
        };
        let out = match item.kind {
            ItemKind::Page if html::rewritable(&raw) => {
                html::rewrite(&raw, &page, item.charset.as_deref(), &mut local)
            }
            ItemKind::Style => css::rewrite(&raw, &mut |url, _| {
                html::rewritten(&page, url, &mut local).map(|(text, _)| text)
            }),
            _ => raw,
        };
        let size = out.len() as u64;
        let temp = {
            let mut name = target.as_os_str().to_os_string();
            name.push(".rewrite");
            PathBuf::from(name)
        };
        let written = tokio::task::spawn_blocking({
            let (temp, target) = (temp.clone(), target.clone());
            move || -> std::io::Result<()> {
                let mut file = std::fs::File::create(&temp)?;
                file.write_all(&out)?;
                file.sync_data()?;
                drop(file);
                std::fs::rename(&temp, &target)?;
                sync_parent_dir(&target).map_err(std::io::Error::other)
            }
        })
        .await?;
        if let Err(e) = written {
            return Ok(disk(&e));
        }
        crate::io::mark_from_internet(&target);
        site::done(conn, task_id, item.seq, None, size)?;
        // Le brut ne sert plus : la réécriture est enregistrée.
        let _ = std::fs::remove_file(&part);
    }
    let total = written(conn, task_id)?;
    site::set_phase(conn, task_id, Phase::Done)?;
    crate::postproc::finish(conn, task_id, root, total)?;
    let counts = site::counts(conn, task_id)?;
    tracing::info!(
        tache = %task_id,
        pages = counts.pages,
        resources = counts.resources,
        files = counts.files,
        failed = counts.failed,
        "copie d'un site finie"
    );
    Ok(End::Completed(root.to_path_buf()))
}

/// Efface ce que la copie a écrit : les fichiers de ses éléments (`.part` et fichiers
/// provisoires compris), ceux de ses fichiers cochés, puis les dossiers devenus vides,
/// dossier de la copie compris ; jamais un fichier qu'elle n'a pas écrit (SPEC §7.18 ter).
/// `finals` : les fichiers finis aussi (sinon seulement les `.part`).
pub fn delete_files(conn: &Connection, task: &TaskRow, finals: bool) -> std::io::Result<()> {
    let Some(name) = &task.file_name else {
        return Ok(());
    };
    // Une copie inachevée n'a rien de fini : tout ce qu'elle a écrit part.
    let finals = finals || task.state != TaskState::Completed;
    let root = Path::new(&task.dest_dir).join(name);
    let remove = |path: &Path| match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    };
    let items = site::items(conn, &task.id).map_err(std::io::Error::other)?;
    let mut dirs: HashSet<PathBuf> = HashSet::new();
    for item in &items {
        let Some(path) = &item.path else { continue };
        let target = on_disk(&root, path);
        remove(&part_path(&target))?;
        let mut temp = target.as_os_str().to_os_string();
        temp.push(".rewrite");
        remove(Path::new(&temp))?;
        if finals {
            remove(&target)?;
        }
        let mut dir = target.parent();
        while let Some(d) = dir {
            if d == root || !d.starts_with(&root) {
                break;
            }
            dirs.insert(d.to_path_buf());
            dir = d.parent();
        }
    }
    // Un fichier coché en cours : le `.part` de sa tâche interne (son nom peut porter un
    // numéro).
    for child in site::children(conn, &task.id).map_err(std::io::Error::other)? {
        let Ok(row) = load_task(conn, &child) else {
            continue;
        };
        let mut dir = Some(Path::new(&row.dest_dir));
        while let Some(d) = dir {
            if d == root || !d.starts_with(&root) {
                break;
            }
            dirs.insert(d.to_path_buf());
            dir = d.parent();
        }
        if let Some(name) = row.file_name {
            let path = Path::new(&row.dest_dir).join(name);
            remove(&part_path(&path))?;
            if finals {
                remove(&path)?;
            }
        }
    }
    // Les dossiers vides, du plus profond au moins profond ; un dossier où reste un
    // fichier d'ailleurs est gardé.
    let mut dirs: Vec<PathBuf> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for dir in dirs {
        let _ = std::fs::remove_dir(&dir);
    }
    let _ = std::fs::remove_dir(&root);
    Ok(())
}

#[cfg(test)]
mod tests;
