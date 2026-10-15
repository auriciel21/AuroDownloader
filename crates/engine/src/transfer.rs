//! Exécution d'une tâche HTTP(S) : préparation, connexions, points de contrôle, reprise et
//! fin (SPEC §7.1, §7.2, §7.4 et §7.5).
//!
//! Ce module enchaîne les autres : `net` reçoit les octets, `io` les écrit à leur place,
//! `store` enregistre l'état. Chaque connexion télécharge un segment avec son propre
//! écrivain ; la boucle de coordination, seule à toucher la base, enregistre les
//! compteurs vidés sur disque (règle 1) à la fin de chaque segment et à chaque point de
//! contrôle.
//!
//! Pas encore : redécoupage dynamique (F-02) — une connexion libre prend un segment
//! `pending`, sinon s'arrête ; nombre de connexions automatique (SPEC §7.3) ; plusieurs
//! sources (§7.7) ; limitation de débit (§7.6). Une tâche en `retry_wait` est relancée par
//! l'appelant (le planificateur, en phase 2).

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior, interval_at};

use crate::io::{
    self, DEFAULT_WRITE_BUFFER, IoError, PartFile, ProgressHandle, SegmentWriter, SyncedProgress,
    segments_beyond,
};
use crate::net::{Client, NetError, RangeRequest, headers};
use crate::segmenter::{
    DEFAULT_MIN_SEGMENT_SIZE, Segment, SegmentState, initial_split, split_largest,
};
use crate::store::segments::{
    check_task_coverage, load_segments, reset_segments, save_progress, save_split,
};
use crate::store::tasks::{
    Change, PauseReason, Preparation, SourceRow, TaskRow, TaskState, change_state, complete_task,
    load_task, primary_source, save_preparation,
};
use crate::store::{StoreError, now_ms};

/// Nombre de connexions automatique (SPEC §7.3) : départ, plafond, et gain de débit
/// exigé pour ajouter une connexion.
pub const AUTO_START_CONNECTIONS: u32 = 4;
pub const AUTO_MAX_CONNECTIONS: u32 = 16;
pub const AUTO_MIN_GAIN: f64 = 1.10;

/// Connexions au plus, en réglage manuel (F-01).
pub const MAX_CONNECTIONS: u32 = 32;

/// Échecs d'affilée avant l'abandon (SPEC §7.8).
pub const MAX_FAILURES: u32 = 10;

/// Délai de reprise : base et plafond (SPEC §7.8).
pub const RETRY_BASE: Duration = Duration::from_secs(2);
pub const RETRY_CAP: Duration = Duration::from_secs(5 * 60);

/// Réglages d'une exécution.
#[derive(Debug, Clone)]
pub struct Options {
    /// Connexions imposées pour cette exécution ; `None` : celles de la tâche
    /// (`max_connections`), sinon nombre automatique (SPEC §7.3).
    pub connections: Option<u32>,
    pub auto_start: u32,
    pub auto_max: u32,
    /// Fenêtre de mesure du débit (SPEC §7.10 : 2 s).
    pub measure_every: Duration,
    pub min_segment: u64,
    pub buffer: usize,
    /// Intervalle des points de contrôle (SPEC §7.10 : 2 s).
    pub checkpoint_every: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            connections: None,
            auto_start: AUTO_START_CONNECTIONS,
            auto_max: AUTO_MAX_CONNECTIONS,
            measure_every: Duration::from_secs(2),
            min_segment: DEFAULT_MIN_SEGMENT_SIZE,
            buffer: DEFAULT_WRITE_BUFFER,
            checkpoint_every: Duration::from_secs(2),
        }
    }
}

/// Avancement, pour l'affichage : des octets reçus, pas forcément vidés sur disque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub received: u64,
    pub total: Option<u64>,
    pub connections: usize,
}

/// Issue d'une exécution, déjà enregistrée dans la base.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Fichier en place ; tâche `completed`.
    Completed(PathBuf),
    /// Arrêt demandé ; tâche `paused` (cause `user`).
    Paused,
    /// Le fichier distant a changé ; tâche `paused` (cause `file_changed`), en attente de
    /// la décision de l'utilisateur (F-13).
    FileChanged,
    /// Disque plein ; tâche `paused` (cause `disk_full`).
    DiskFull,
    /// Échec temporaire ; tâche `retry_wait` jusqu'à `at_ms` (millisecondes Unix).
    RetryLater {
        code: &'static str,
        detail: String,
        at_ms: i64,
    },
    /// Échec définitif ; tâche `failed`.
    Failed { code: &'static str, detail: String },
}

/// Erreurs qui empêchent d'exécuter ou d'enregistrer : base illisible, état incohérent.
/// Les échecs du téléchargement lui-même sont des [`Outcome`].
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error(transparent)]
    Store(#[from] StoreError),

    #[error("la tâche {task_id} est dans l'état {state}, qui ne s'exécute pas")]
    NotRunnable {
        task_id: String,
        state: &'static str,
    },

    #[error("la tâche {task_id} n'est pas terminée alors que ses connexions le sont")]
    Incomplete { task_id: String },

    #[error("une connexion s'est arrêtée anormalement : {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Cause d'échec d'un téléchargement.
#[derive(Debug)]
enum Failure {
    Net(NetError),
    Io(IoError),
    /// Le serveur a fermé le flux avant la fin, sans erreur.
    Truncated,
}

impl Failure {
    /// Code de la SPEC §6.8.
    fn code(&self) -> &'static str {
        match self {
            Failure::Net(e) => e.error_code().unwrap_or("HTTP_STATUS"),
            Failure::Io(e) => match e.io_kind() {
                Some(std::io::ErrorKind::StorageFull) => "DISK_FULL",
                Some(std::io::ErrorKind::PermissionDenied) => "DEST_PERMISSION",
                _ => "DISK_IO",
            },
            Failure::Truncated => "NET_UNREACHABLE",
        }
    }

    fn retryable(&self) -> bool {
        match self {
            Failure::Net(e) => e.retryable(),
            Failure::Io(_) => false,
            Failure::Truncated => true,
        }
    }

    fn detail(&self) -> String {
        match self {
            Failure::Net(e) => e.to_string(),
            Failure::Io(e) => e.to_string(),
            Failure::Truncated => "le serveur a fermé la connexion avant la fin".into(),
        }
    }
}

/// Raison de l'arrêt d'une exécution avant la fin.
enum Stop {
    User,
    Failure(Failure),
}

type Step<T> = Result<Result<T, Stop>, TransferError>;

fn fail<T>(failure: Failure) -> Step<T> {
    Ok(Err(Stop::Failure(failure)))
}

/// Délai avant la nouvelle tentative `n` (à partir de 0) : `min(plafond, base × 2^n)`
/// plus `jitter`, sauf si le serveur a fixé le sien (`Retry-After`, SPEC §7.8).
pub fn retry_delay(n: u32, retry_after: Option<Duration>, jitter: Duration) -> Duration {
    if let Some(asked) = retry_after {
        return asked;
    }
    let factor = 1u32.checked_shl(n).unwrap_or(u32::MAX);
    RETRY_BASE.saturating_mul(factor).min(RETRY_CAP) + jitter
}

/// Aléa de 0 à 1 s, pour que des tâches échouées ensemble ne repartent pas ensemble.
fn jitter() -> Duration {
    let random = RandomState::new().build_hasher().finish();
    Duration::from_millis(random % 1000)
}

/// Exécute la tâche `task_id` jusqu'à sa fin, un échec ou `stop`, et enregistre l'issue.
///
/// La tâche doit être `queued` ou `retry_wait`, ou `active` après un arrêt brutal du
/// moteur (SPEC §7.4). `on_progress` est appelé régulièrement pour l'affichage.
pub async fn run(
    conn: &mut Connection,
    client: &Client,
    task_id: &str,
    opts: &Options,
    mut stop: watch::Receiver<bool>,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<Outcome, TransferError> {
    let task = load_task(conn, task_id)?;
    match task.state {
        TaskState::Queued | TaskState::RetryWait => {
            change_state(conn, task_id, &Change::to(TaskState::Active))?;
        }
        TaskState::Active => {}
        other => {
            return Err(TransferError::NotRunnable {
                task_id: task_id.to_owned(),
                state: other.as_sql(),
            });
        }
    }
    match execute(conn, client, task_id, opts, &mut stop, on_progress).await? {
        Ok(path) => Ok(Outcome::Completed(path)),
        Err(Stop::User) => {
            let change = Change {
                pause_reason: Some(PauseReason::User),
                ..Change::to(TaskState::Paused)
            };
            change_state(conn, task_id, &change)?;
            Ok(Outcome::Paused)
        }
        Err(Stop::Failure(failure)) => record_failure(conn, task_id, failure),
    }
}

/// Enregistre un échec selon la SPEC §6.8 et §7.8.
fn record_failure(
    conn: &mut Connection,
    task_id: &str,
    failure: Failure,
) -> Result<Outcome, TransferError> {
    let (code, detail) = (failure.code(), failure.detail());
    let error = Some((code, detail.as_str()));
    let pause = |reason| Change {
        pause_reason: Some(reason),
        error,
        ..Change::to(TaskState::Paused)
    };
    let task = load_task(conn, task_id)?;
    let outcome = match &failure {
        Failure::Net(NetError::FileChanged) => {
            change_state(conn, task_id, &pause(PauseReason::FileChanged))?;
            Outcome::FileChanged
        }
        Failure::Io(e) if e.io_kind() == Some(std::io::ErrorKind::StorageFull) => {
            change_state(conn, task_id, &pause(PauseReason::DiskFull))?;
            Outcome::DiskFull
        }
        f if f.retryable() && task.retry_count + 1 < MAX_FAILURES => {
            let asked = match f {
                Failure::Net(e) => e.retry_after(),
                _ => None,
            };
            let delay = retry_delay(task.retry_count, asked, jitter());
            let at_ms =
                now_ms().saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX));
            let change = Change {
                error,
                next_retry_at: Some(at_ms),
                ..Change::to(TaskState::RetryWait)
            };
            change_state(conn, task_id, &change)?;
            Outcome::RetryLater {
                code,
                detail: detail.clone(),
                at_ms,
            }
        }
        _ => {
            let change = Change {
                error,
                ..Change::to(TaskState::Failed)
            };
            change_state(conn, task_id, &change)?;
            Outcome::Failed {
                code,
                detail: detail.clone(),
            }
        }
    };
    Ok(outcome)
}

async fn execute(
    conn: &mut Connection,
    client: &Client,
    task_id: &str,
    opts: &Options,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut dyn FnMut(Progress),
) -> Step<PathBuf> {
    let mut task = load_task(conn, task_id)?;
    let source = primary_source(conn, task_id)?;

    // Préparation (SPEC §7.1), une seule fois par tâche.
    if task.file_name.is_none() {
        let probe = match client.probe(&source.url, &[]).await {
            Ok(probe) => probe,
            Err(e) => return fail(Failure::Net(e)),
        };
        let name = io::available_name(Path::new(&task.dest_dir), &probe.file_name);
        let connections = if probe.accepts_ranges {
            first_width(client, &source.url, &task, opts)
        } else {
            1
        };
        let segments = probe
            .total_bytes
            .map(|total| initial_split(total, connections, opts.min_segment))
            .unwrap_or_default();
        let prep = Preparation {
            source_id: &source.id,
            accepts_ranges: probe.accepts_ranges,
            etag: probe.etag.as_deref(),
            last_modified: probe.last_modified.as_deref(),
            total_bytes: probe.total_bytes,
            file_name: &name,
        };
        save_preparation(conn, task_id, &prep, &segments)?;
        task = load_task(conn, task_id)?;
    }
    let source = primary_source(conn, task_id)?;
    let file_name = task
        .file_name
        .as_deref()
        .ok_or_else(|| StoreError::BadValue(format!("tâche {task_id} : nom absent")))?;
    let dest = Path::new(&task.dest_dir).join(file_name);

    let (part, todo, total) = match task.total_bytes {
        Some(total) => match resume_part(conn, task_id, &source, &dest, total)? {
            Resume::Part(part, todo) => (part, todo, Some(total)),
            Resume::AlreadyRenamed => {
                complete_task(conn, task_id, &dest.to_string_lossy(), total)?;
                return Ok(Ok(dest));
            }
            Resume::Failed(f) => return fail(f),
        },
        None => match fresh_stream_part(&dest) {
            Ok(Some(part)) => {
                let all = Segment {
                    idx: 0,
                    start: 0,
                    end: u64::MAX - 1,
                    written: 0,
                    state: SegmentState::Active,
                };
                (part, vec![all], None)
            }
            Ok(None) => {
                let len = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                complete_task(conn, task_id, &dest.to_string_lossy(), len)?;
                return Ok(Ok(dest));
            }
            Err(e) => return fail(Failure::Io(e)),
        },
    };

    let ranged = source.accepts_ranges == Some(true);
    let fetch = match total {
        Some(total) if ranged => Fetch::Ranges {
            total,
            // `If-Range` n'accepte qu'un ETag fort, sinon la date (RFC 9110 §13.1.5).
            if_range: source
                .etag
                .clone()
                .filter(|e| headers::is_strong_etag(e))
                .or_else(|| source.last_modified.clone()),
        },
        total => Fetch::Whole { total },
    };
    let plan = if ranged {
        plan_for(client, &source.url, &task, opts)
    } else {
        Plan::Fixed(1)
    };
    let part = Arc::new(part);
    let ctx = Pool {
        client,
        url: &source.url,
        fetch,
        plan,
        host: host_of(&source.url),
        opts,
        part: Arc::clone(&part),
        persist: total.is_some(),
    };
    let received = match pool(conn, task_id, &ctx, todo, stop, on_progress).await? {
        Ok(received) => received,
        Err(stop) => return Ok(Err(stop)),
    };
    drop(ctx);

    if let Some(total) = total {
        let all_done = load_segments(conn, task_id)?
            .iter()
            .all(|s| s.state == SegmentState::Done);
        if !all_done {
            return Err(TransferError::Incomplete {
                task_id: task_id.to_owned(),
            });
        }
        debug_assert_eq!(received, total);
    }
    let part = Arc::try_unwrap(part).map_err(|_| TransferError::Incomplete {
        task_id: task_id.to_owned(),
    })?;
    let path = match tokio::task::spawn_blocking(move || part.finish()).await? {
        Ok(path) => path,
        Err(e) => return fail(Failure::Io(e)),
    };
    complete_task(
        conn,
        task_id,
        &path.to_string_lossy(),
        total.unwrap_or(received),
    )?;
    Ok(Ok(path))
}

enum Resume {
    Part(PartFile, Vec<Segment>),
    AlreadyRenamed,
    Failed(Failure),
}

/// Reprise d'une tâche de taille connue (SPEC §7.4, étapes 1 et 2) : ouvre ou recrée le
/// `.part`, remet à zéro les segments dont les octets manquent, vérifie la couverture et
/// renvoie les segments à télécharger.
fn resume_part(
    conn: &mut Connection,
    task_id: &str,
    source: &SourceRow,
    dest: &Path,
    total: u64,
) -> Result<Resume, TransferError> {
    let part = match PartFile::open(dest) {
        Ok(part) => part,
        Err(e) if e.io_kind() == Some(std::io::ErrorKind::NotFound) => {
            let segments = load_segments(conn, task_id)?;
            // Arrêt entre le renommage du fichier et l'enregistrement de la fin.
            let all_done = segments.iter().all(|s| s.state == SegmentState::Done);
            let dest_len = std::fs::metadata(dest).ok().map(|m| m.len());
            if all_done && dest_len == Some(total) {
                return Ok(Resume::AlreadyRenamed);
            }
            // `.part` perdu : tout ce qui était écrit est à refaire.
            let written: Vec<u32> = segments
                .iter()
                .filter(|s| s.written > 0)
                .map(|s| s.idx)
                .collect();
            reset_segments(conn, task_id, &written)?;
            match PartFile::create(dest) {
                Ok(part) => part,
                Err(e) => return Ok(Resume::Failed(Failure::Io(e))),
            }
        }
        Err(e) => return Ok(Resume::Failed(Failure::Io(e))),
    };
    let size = match part.size_on_disk() {
        Ok(size) => size,
        Err(e) => return Ok(Resume::Failed(Failure::Io(e))),
    };
    let segments = load_segments(conn, task_id)?;
    let lost = if source.accepts_ranges == Some(true) {
        segments_beyond(size, &segments)
    } else {
        // Sans plages, aucune reprise : le flux repart du début.
        segments
            .iter()
            .filter(|s| s.written > 0)
            .map(|s| s.idx)
            .collect()
    };
    reset_segments(conn, task_id, &lost)?;
    check_task_coverage(conn, task_id)?;
    let todo = load_segments(conn, task_id)?
        .into_iter()
        .filter(|s| s.state != SegmentState::Done)
        .collect();
    Ok(Resume::Part(part, todo))
}

/// Flux de taille inconnue : pas de reprise possible, le `.part` repart de zéro.
/// `Ok(None)` : `.part` absent et fichier final présent, le renommage a déjà eu lieu.
fn fresh_stream_part(dest: &Path) -> Result<Option<PartFile>, IoError> {
    let part = io::part_path(dest);
    if !part.exists() && dest.exists() {
        return Ok(None);
    }
    match std::fs::remove_file(&part) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(IoError::File { path: part, source }),
    }
    PartFile::create(dest).map(Some)
}

/// Manière de demander les octets d'un segment.
#[derive(Debug, Clone)]
enum Fetch {
    /// Requêtes par plage, avec `If-Range`.
    Ranges {
        total: u64,
        if_range: Option<String>,
    },
    /// Un seul flux depuis le début ; taille inconnue si `None`.
    Whole { total: Option<u64> },
}

/// Nombre de connexions automatique (SPEC §7.3), comme le démarrage de TCP : montée
/// rapide tant qu'elle paie, réduction rapide quand le serveur résiste.
///
/// À chaque fenêtre de mesure, le nombre de connexions double (dans la limite du plafond)
/// si le débit a progressé d'au moins 10 % depuis la dernière augmentation ; sinon
/// l'augmentation s'arrête pour cette exécution. Sur un serveur qui résiste, moitié moins
/// de connexions, et un plafond égal à la valeur atteinte moins une.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoConnections {
    width: u32,
    cap: u32,
    frozen: bool,
    /// Débit de la fenêtre qui a précédé la dernière augmentation, en octets par seconde.
    baseline: Option<f64>,
}

impl AutoConnections {
    pub fn new(start: u32, cap: u32) -> Self {
        let cap = cap.max(1);
        AutoConnections {
            width: start.clamp(1, cap),
            cap,
            frozen: false,
            baseline: None,
        }
    }

    /// Connexions visées.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// L'augmentation est terminée pour cette exécution.
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Débit total mesuré sur la dernière fenêtre ; renvoie les connexions visées.
    pub fn on_rate(&mut self, rate: f64) -> u32 {
        // Un débit nul (serveur muet) ne dit rien du gain d'une connexion de plus.
        if self.frozen || rate <= 0.0 || self.width >= self.cap {
            return self.width;
        }
        match self.baseline {
            Some(before) if rate < before * AUTO_MIN_GAIN => self.frozen = true,
            _ => {
                self.baseline = Some(rate);
                self.width = self.width.saturating_mul(2).min(self.cap);
            }
        }
        self.width
    }

    /// Le serveur résiste (429, 503, connexion refusée) : divise par deux et renvoie le
    /// plafond à retenir pour l'hôte pendant la session.
    pub fn on_overload(&mut self) -> u32 {
        let reached = self.width;
        self.width = (reached / 2).max(1);
        self.cap = self.cap.min(reached.saturating_sub(1).max(1));
        self.frozen = true;
        self.cap
    }
}

/// Connexions d'une exécution : imposées, ou automatiques.
#[derive(Debug, Clone)]
enum Plan {
    Fixed(u32),
    Auto(AutoConnections),
}

fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url).ok()?.host_str().map(str::to_owned)
}

/// Plafond automatique : celui des réglages, abaissé par ce que l'hôte a appris à la
/// session (SPEC §7.3).
fn auto_cap(client: &Client, url: &str, opts: &Options) -> u32 {
    host_of(url)
        .and_then(|h| client.host_cap(&h))
        .map_or(opts.auto_max, |c| c.min(opts.auto_max))
}

/// Connexions de l'exécution : imposées pour celle-ci, sinon celles de la tâche, sinon
/// automatiques.
fn plan_for(client: &Client, url: &str, task: &TaskRow, opts: &Options) -> Plan {
    match opts.connections.or(task.max_connections) {
        Some(n) => Plan::Fixed(n.clamp(1, MAX_CONNECTIONS)),
        None => Plan::Auto(AutoConnections::new(
            opts.auto_start,
            auto_cap(client, url, opts),
        )),
    }
}

/// Nombre de segments du découpage initial.
fn first_width(client: &Client, url: &str, task: &TaskRow, opts: &Options) -> u32 {
    match plan_for(client, url, task, opts) {
        Plan::Fixed(n) => n,
        Plan::Auto(a) => a.width(),
    }
}

/// Ce que partagent les connexions d'une exécution.
struct Pool<'a> {
    client: &'a Client,
    url: &'a str,
    fetch: Fetch,
    plan: Plan,
    /// Hôte de la source, pour le plafond appris d'un serveur qui résiste.
    host: Option<String>,
    opts: &'a Options,
    part: Arc<PartFile>,
    /// Taille connue : les compteurs sont enregistrés en base.
    persist: bool,
}

/// Fin d'une connexion : compteur vidé sur disque et issue.
struct ConnEnd {
    idx: u32,
    progress: Result<SyncedProgress, IoError>,
    result: Result<(), Failure>,
}

/// Boucle de coordination : remplit les connexions libres avec les segments à faire,
/// enregistre les compteurs et s'arrête au premier échec ou sur `stop`. Renvoie les
/// octets écrits au total.
async fn pool(
    conn: &mut Connection,
    task_id: &str,
    ctx: &Pool<'_>,
    todo: Vec<Segment>,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut dyn FnMut(Progress),
) -> Step<u64> {
    let mut todo: VecDeque<Segment> = todo.into();
    // Octets enregistrés de chaque segment ; ceux en cours sont lus sur leur suivi.
    let mut saved: HashMap<u32, u64> = load_segments(conn, task_id)?
        .iter()
        .map(|s| (s.idx, s.written))
        .collect();
    let total = match ctx.fetch {
        Fetch::Ranges { total, .. } => Some(total),
        Fetch::Whole { total } => total,
    };
    let (halt_tx, halt_rx) = watch::channel(false);
    let mut running: JoinSet<ConnEnd> = JoinSet::new();
    let mut handles: HashMap<u32, ProgressHandle> = HashMap::new();
    let mut failure: Option<Failure> = None;
    let (mut user_stop, mut stop_gone) = (false, false);

    let start = Instant::now();
    let mut checkpoint = interval_at(start + ctx.opts.checkpoint_every, ctx.opts.checkpoint_every);
    checkpoint.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut display = interval_at(start, Duration::from_millis(500));
    display.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let (mut width, mut auto) = match &ctx.plan {
        Plan::Fixed(n) => (*n as usize, None),
        Plan::Auto(a) => (a.width() as usize, Some(a.clone())),
    };
    let mut measure = interval_at(start + ctx.opts.measure_every, ctx.opts.measure_every);
    measure.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut measured = (start, progress(&saved, &handles, total, 0).received);

    loop {
        while failure.is_none() && !user_stop && running.len() < width {
            let segment = match todo.pop_front() {
                Some(segment) => segment,
                // Plus rien en attente : la connexion libre coupe le plus gros segment en
                // cours (F-02).
                None if ctx.persist => {
                    match split_running(conn, task_id, &handles, ctx.opts.min_segment)? {
                        Some(segment) => {
                            saved.insert(segment.idx, 0);
                            segment
                        }
                        None => break,
                    }
                }
                None => break,
            };
            let writer = ctx.part.writer(&segment, ctx.opts.buffer);
            handles.insert(segment.idx, writer.progress());
            running.spawn(connection(
                ctx.client.clone(),
                ctx.url.to_owned(),
                ctx.fetch.clone(),
                segment.start,
                writer,
                halt_rx.clone(),
            ));
        }
        if running.is_empty() {
            break;
        }

        let first_failure = |f: Failure, failure: &mut Option<Failure>| {
            if failure.is_none() {
                *failure = Some(f);
            }
            let _ = halt_tx.send(true);
        };
        tokio::select! {
            joined = running.join_next() => {
                let end = joined.expect("au moins une connexion")?;
                let handle = handles.remove(&end.idx);
                let mut written = None;
                match end.progress {
                    Ok(p) => {
                        if ctx.persist {
                            save_progress(conn, task_id, &[p])?;
                        }
                        saved.insert(p.idx(), p.written());
                        written = Some(p.written());
                    }
                    Err(e) => first_failure(Failure::Io(e), &mut failure),
                }
                match end.result {
                    Ok(()) => {}
                    // Le serveur résiste alors que d'autres connexions tournent : moitié
                    // moins de connexions, plafond retenu pour l'hôte, et le segment
                    // attend son tour depuis ses octets enregistrés (SPEC §7.3).
                    Err(Failure::Net(e))
                        if e.is_overload()
                            && failure.is_none()
                            && !running.is_empty()
                            && auto.is_some() =>
                    {
                        let a = auto.as_mut().expect("mode automatique");
                        let cap = a.on_overload();
                        width = a.width() as usize;
                        if let Some(host) = &ctx.host {
                            ctx.client.limit_host(host, cap);
                        }
                        if let (Some(h), Some(written)) = (handle, written) {
                            let (_, last) = h.accepted_and_end();
                            todo.push_front(Segment {
                                idx: end.idx,
                                start: h.start(),
                                end: last,
                                written,
                                state: SegmentState::Pending,
                            });
                        }
                    }
                    Err(f) => first_failure(f, &mut failure),
                }
            }
            _ = checkpoint.tick(), if ctx.persist && !handles.is_empty() => {
                let list: Vec<ProgressHandle> = handles.values().cloned().collect();
                let part = Arc::clone(&ctx.part);
                match tokio::task::spawn_blocking(move || part.checkpoint(&list)).await? {
                    Ok(synced) => {
                        save_progress(conn, task_id, &synced)?;
                        for p in synced {
                            let entry = saved.entry(p.idx()).or_default();
                            *entry = (*entry).max(p.written());
                        }
                    }
                    Err(e) => first_failure(Failure::Io(e), &mut failure),
                }
            }
            _ = measure.tick(),
                if auto.as_ref().is_some_and(|a| !a.is_frozen()) && failure.is_none() && !user_stop =>
            {
                let now = Instant::now();
                let received = progress(&saved, &handles, total, 0).received;
                let elapsed = now.duration_since(measured.0).as_secs_f64();
                if let (Some(a), true) = (auto.as_mut(), elapsed > 0.0) {
                    let rate = received.saturating_sub(measured.1) as f64 / elapsed;
                    width = a.on_rate(rate) as usize;
                }
                measured = (now, received);
            }
            _ = display.tick() => {
                on_progress(progress(&saved, &handles, total, running.len()));
            }
            changed = async { stop.wait_for(|s| *s).await.is_ok() }, if !user_stop && !stop_gone => {
                if changed {
                    user_stop = true;
                    let _ = halt_tx.send(true);
                } else {
                    stop_gone = true;
                }
            }
        }
    }
    on_progress(progress(&saved, &handles, total, 0));

    if let Some(f) = failure {
        return fail(f);
    }
    if user_stop {
        return Ok(Err(Stop::User));
    }
    Ok(Ok(saved.values().sum()))
}

/// Redécoupage dynamique (SPEC §7.2) : coupe en deux le segment en cours qui a le plus
/// d'octets restants et renvoie la seconde moitié, à confier à la connexion libre.
///
/// Ordre : l'écrivain du segment coupé recule d'abord sa borne, puis le découpage est
/// enregistré (couverture vérifiée dans la transaction). Un arrêt entre les deux laisse
/// la base à l'ancien découpage, toujours juste. Si la connexion a dépassé le milieu
/// entre la lecture et la coupe, le calcul est refait.
fn split_running(
    conn: &mut Connection,
    task_id: &str,
    handles: &HashMap<u32, ProgressHandle>,
    min_size: u64,
) -> Result<Option<Segment>, TransferError> {
    if handles.is_empty() {
        return Ok(None);
    }
    for _ in 0..3 {
        let mut segments = load_segments(conn, task_id)?;
        for s in &mut segments {
            match handles.get(&s.idx) {
                Some(h) => {
                    let (accepted, end) = h.accepted_and_end();
                    s.end = end;
                    s.written = accepted;
                    s.state = SegmentState::Active;
                }
                // Seuls les segments en cours de téléchargement se coupent.
                None if s.state == SegmentState::Active => s.state = SegmentState::Pending,
                None => {}
            }
        }
        let Some(split) = split_largest(&mut segments, min_size) else {
            return Ok(None);
        };
        let find = |idx: u32| segments.iter().find(|s| s.idx == idx).cloned();
        let (Some(from), Some(new)) = (find(split.from), find(split.new)) else {
            return Ok(None);
        };
        if handles[&split.from].shrink_end(from.end).is_err() {
            continue;
        }
        save_split(conn, task_id, &segments, split)?;
        return Ok(Some(new));
    }
    Ok(None)
}

fn progress(
    saved: &HashMap<u32, u64>,
    handles: &HashMap<u32, ProgressHandle>,
    total: Option<u64>,
    connections: usize,
) -> Progress {
    let received = saved
        .iter()
        .map(|(idx, &w)| handles.get(idx).map_or(w, |h| h.handed_over().max(w)))
        .chain(
            handles
                .iter()
                .filter(|(idx, _)| !saved.contains_key(idx))
                .map(|(_, h)| h.handed_over()),
        )
        .sum();
    Progress {
        received,
        total,
        connections,
    }
}

/// Une connexion : télécharge le segment de `writer`, puis vide ce qu'elle a reçu.
async fn connection(
    client: Client,
    url: String,
    fetch: Fetch,
    start: u64,
    mut writer: SegmentWriter,
    mut halt: watch::Receiver<bool>,
) -> ConnEnd {
    let result = pump(&client, &url, &fetch, start, &mut writer, &mut halt).await;
    let idx = writer.idx();
    let progress = tokio::task::spawn_blocking(move || writer.finish())
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
    ConnEnd {
        idx,
        progress,
        result,
    }
}

/// Reçoit les octets du segment jusqu'à sa fin ou jusqu'à `halt`. Une réponse plus courte
/// que demandé est suivie d'une nouvelle requête pour le reste.
async fn pump(
    client: &Client,
    url: &str,
    fetch: &Fetch,
    start: u64,
    writer: &mut SegmentWriter,
    halt: &mut watch::Receiver<bool>,
) -> Result<(), Failure> {
    while writer.remaining() > 0 {
        let first = start + writer.accepted();
        let last = first + writer.remaining() - 1;
        let request = async {
            match fetch {
                Fetch::Ranges { total, if_range } => {
                    let req = RangeRequest {
                        first,
                        last,
                        total: *total,
                        if_range: if_range.as_deref(),
                    };
                    client.get_range(url, &[], &req).await
                }
                Fetch::Whole { total } => client.get_all(url, &[], *total).await,
            }
        };
        let mut body = tokio::select! {
            biased;
            _ = halted(halt) => return Ok(()),
            body = request => body.map_err(Failure::Net)?,
        };
        let before = writer.accepted();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = halted(halt) => return Ok(()),
                chunk = body.chunk() => chunk.map_err(Failure::Net)?,
            };
            let Some(chunk) = chunk else { break };
            writer.write(&chunk).map_err(Failure::Io)?;
            if writer.remaining() == 0 {
                return Ok(());
            }
        }
        match fetch {
            // Taille inconnue : la fin du flux est la fin du fichier.
            Fetch::Whole { total: None } => return Ok(()),
            // Un flux unique ne reprend pas en cours de route.
            Fetch::Whole { .. } => return Err(Failure::Truncated),
            Fetch::Ranges { .. } if writer.accepted() == before => return Err(Failure::Truncated),
            Fetch::Ranges { .. } => {}
        }
    }
    Ok(())
}

/// Attend l'ordre d'arrêt des connexions (ou la disparition de son émetteur).
async fn halted(halt: &mut watch::Receiver<bool>) {
    let _ = halt.wait_for(|h| *h).await;
}

/// Nom affiché d'une tâche avant sa préparation : le nom tiré de l'URL, ou l'hôte.
pub fn display_name_for(url: &str) -> String {
    let parsed = reqwest::Url::parse(url).ok();
    parsed
        .as_ref()
        .and_then(crate::net::filename::name_from_url)
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|u| u.host_str().map(str::to_owned))
        })
        .unwrap_or_else(|| crate::net::filename::FALLBACK_NAME.to_owned())
}

/// Relance manuelle d'une tâche `paused` ou `failed` : elle repasse dans la file (SPEC §5.5).
pub fn requeue(conn: &mut Connection, task_id: &str) -> Result<(), TransferError> {
    change_state(conn, task_id, &Change::to(TaskState::Queued))?;
    Ok(())
}

#[cfg(test)]
mod tests;
