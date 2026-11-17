//! Exécution d'une tâche HTTP(S) : préparation, connexions, points de contrôle, reprise et
//! fin (SPEC §7.1, §7.2, §7.4 et §7.5).
//!
//! Ce module enchaîne les autres : `net` reçoit les octets, `io` les écrit à leur place,
//! `store` enregistre l'état. Chaque connexion télécharge un segment avec son propre
//! écrivain ; la boucle de coordination, seule à toucher la base, enregistre les
//! compteurs vidés sur disque (règle 1) à la fin de chaque segment et à chaque point de
//! contrôle.
//!
//! Chaque lecture réseau passe par les seaux de débit de l'exécution (`limit`, SPEC §7.6).
//! L'espace disque (F-18) est contrôlé avant le départ, puis à chaque point de contrôle :
//! s'il manque, la tâche s'arrête proprement en pause `disk_full`, comme sur un disque
//! plein constaté à l'écriture.
//!
//! Pas encore : plusieurs sources (§7.7). Une tâche en `retry_wait` est relancée par
//! l'appelant (le planificateur).

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior, interval_at};

use crate::io::checksum::{self, Algo, Checksum};
use crate::io::{
    self, DEFAULT_WRITE_BUFFER, IoError, PartFile, ProgressHandle, SealedPart, SegmentWriter,
    SyncedProgress, segments_beyond,
};
use crate::limit::Throttle;
use crate::mirrors;
use crate::net::auth::{Auth, Login, Site};
use crate::net::{Client, NetError, RangeRequest, headers, sums};
use crate::pieces;
use crate::secrets::{Vault, VaultError};
use crate::segmenter::{
    DEFAULT_MIN_SEGMENT_SIZE, Segment, SegmentState, initial_split, split_largest,
};
use crate::store::claims::ClaimKind;
use crate::store::headers::{self as stored_headers, Stored, StoredHeader};
use crate::store::logins::{self, LoginRow};
use crate::store::segments::{
    check_task_coverage, load_segments, reset_segments, save_progress, save_split,
};
use crate::store::tasks::{
    Change, PauseReason, Preparation, SourceRow, SourceState, TaskRow, TaskState, change_state,
    load_sources, load_task, primary_source, reserved_names, save_preparation_placed, set_checksum,
};
use crate::store::{self, sources};
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

/// Place que le moteur laisse toujours libre sur le disque de destination (SPEC §7.10).
pub const KEEP_FREE: u64 = 64 * 1024 * 1024;

/// Volume reçu qui avance le point de contrôle suivant (SPEC §7.5 et §7.10) : un
/// téléchargement rapide vide pendant qu'il reçoit, pas tout à la fin.
pub const CHECKPOINT_BYTES: u64 = 32 * 1024 * 1024;

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
    /// Intervalle des points de contrôle (SPEC §7.10 : 2 s) ; l'espace libre se contrôle
    /// au même rythme.
    pub checkpoint_every: Duration,
    /// Octets remis au système qui avancent le point de contrôle suivant
    /// ([`CHECKPOINT_BYTES`]).
    pub checkpoint_bytes: u64,
    /// Seaux de débit de l'exécution (SPEC §7.6) ; vide : aucune limite.
    pub throttle: Throttle,
    /// Mesure de la place libre d'un volume ; les tests la remplacent.
    pub free_space: fn(&Path) -> std::io::Result<u64>,
    /// Place à laisser libre sur le disque ([`KEEP_FREE`]).
    pub keep_free: u64,
    /// Coffre des en-têtes secrets des tâches (SPEC §5.1) ; la mémoire par défaut, le
    /// coffre du système pour `aurod`.
    pub vault: Vault,
    /// Réserver la place du fichier au départ (F-10, réglage `downloads.preallocate`).
    pub preallocate: bool,
    /// Chercher l'empreinte dans les fichiers voisins quand la tâche n'en a pas (F-11, SPEC §7.9,
    /// réglage `downloads.neighborChecksum`). Faux par défaut : la bibliothèque, ses tests et
    /// ses bancs n'envoient pas de requêtes de plus ; `aurod` l'active, et le réglage décide.
    pub neighbor_checksum: bool,
    /// Session des torrents (SPEC §7.16).
    pub torrent: crate::torrent::Options,
    /// Outils de la vidéo (SPEC §7.17).
    pub media: crate::media::Options,
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
            checkpoint_bytes: CHECKPOINT_BYTES,
            throttle: Throttle::default(),
            free_space: io::free_space,
            keep_free: KEEP_FREE,
            vault: Vault::default(),
            preallocate: true,
            neighbor_checksum: false,
            torrent: crate::torrent::Options::default(),
            media: crate::media::Options::default(),
        }
    }
}

impl Options {
    /// Place manquante pour écrire encore `needed` octets dans `dir` en laissant
    /// `keep_free` libres ; `None` si elle suffit, ou si la mesure est impossible (un
    /// disque vraiment plein arrêtera la tâche proprement à l'écriture).
    fn short_of_space(&self, dir: &Path, needed: u64) -> Option<Failure> {
        let free = (self.free_space)(dir).ok()?;
        (free < needed.saturating_add(self.keep_free)).then_some(Failure::NoSpace {
            needed,
            free,
            keep: self.keep_free,
        })
    }

    /// La tâche, arrêtée faute de place, peut-elle repartir ? Même règle qu'au départ :
    /// la place encore à prendre pour finir, plus la réserve.
    pub fn space_for(&self, task: &TaskRow) -> bool {
        let Some(name) = &task.file_name else {
            return self.short_of_space(Path::new(&task.dest_dir), 0).is_none();
        };
        let dest = Path::new(&task.dest_dir).join(name);
        let needed = task
            .total_bytes
            .map_or(0, |total| io::space_needed(&io::part_path(&dest), total));
        self.short_of_space(Path::new(&task.dest_dir), needed)
            .is_none()
    }
}

/// Avancement, pour l'affichage : des octets reçus, pas forcément vidés sur disque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub received: u64,
    pub total: Option<u64>,
    pub connections: usize,
    /// Segments en cours de réception, par numéro croissant.
    pub segments: Arc<[LiveSegment]>,
    /// Pendant la vérification de l'empreinte (`verifying`, F-11) : octets du fichier déjà
    /// lus ; `None` pendant la réception.
    pub verified: Option<u64>,
    /// Torrent (SPEC §7.16) : octets envoyés aux autres pairs, cumulés ; `None` en HTTP.
    pub uploaded: Option<u64>,
    /// Torrent : débit d'envoi, en octets par seconde.
    pub upload_bps: Option<u64>,
    /// Torrent en partage (`seeding`) : la tâche vient d'y passer, ou y est.
    pub seeding: bool,
    /// Vidéo en fusion (`post_processing`, SPEC §7.17) : la tâche vient d'y passer.
    pub merging: bool,
}

/// Un segment qu'une connexion reçoit en ce moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSegment {
    pub idx: u32,
    /// Octets reçus depuis le début du segment, pas forcément vidés sur disque.
    pub received: u64,
    /// Source qui l'envoie (SPEC §7.7) : la référence ou un miroir.
    pub source_id: Arc<str>,
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
    /// Torrent dont le partage s'arrête avant son terme (arrêt du moteur, réseau) ; tâche
    /// toujours `seeding`, qui reprendra (SPEC §7.16).
    Seeding,
    /// Commande de fin arrêtée avec le moteur ; tâche toujours `post_processing`, qui la
    /// relancera (SPEC §7.21).
    PostProcessing,
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

    /// Le client restreint au réseau public n'a pas pu être fabriqué (SPEC §7.19) : la
    /// tâche ne part pas, plutôt que sans la restriction.
    #[error("client réseau restreint indisponible : {0}")]
    RestrictedClient(NetError),
}

/// Cause d'échec d'un téléchargement.
#[derive(Debug)]
enum Failure {
    Net(NetError),
    Io(IoError),
    /// Le serveur a fermé le flux avant la fin, sans erreur.
    Truncated,
    /// Pas assez de place pour écrire encore `needed` octets en laissant `keep` libres
    /// (F-18).
    NoSpace {
        needed: u64,
        free: u64,
        keep: u64,
    },
    /// Le coffre du système n'a pas rendu un secret de la tâche (trousseau verrouillé…).
    Vault(VaultError),
    /// L'empreinte du fichier complet n'est pas celle attendue (F-11) ; le `.part` reste.
    Checksum {
        algo: Algo,
        expected: String,
        got: String,
    },
    /// Des morceaux encore faux après les tours permis, ou sans plages pour les redemander
    /// (F-12, SPEC §7.9) ; le `.part` reste.
    Pieces {
        algo: Algo,
        bad: usize,
        count: usize,
        rounds: u32,
    },
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
            Failure::NoSpace { .. } => "DISK_FULL",
            Failure::Vault(_) => "SECRET_UNAVAILABLE",
            Failure::Checksum { .. } | Failure::Pieces { .. } => "CHECKSUM_MISMATCH",
        }
    }

    /// Un échec qui tient à la source (réseau, serveur), et non au disque ou au coffre :
    /// avec des miroirs, il n'écarte que cette source (SPEC §7.7).
    fn caused_by_source(&self) -> bool {
        matches!(self, Failure::Net(_) | Failure::Truncated)
    }

    /// Un échec définitif pour la source : son fichier a changé, ou le serveur refuse.
    fn source_gone(&self) -> bool {
        match self {
            Failure::Net(NetError::FileChanged) => true,
            Failure::Net(e) => !e.retryable(),
            _ => false,
        }
    }

    fn retryable(&self) -> bool {
        match self {
            Failure::Net(e) => e.retryable(),
            Failure::Io(_)
            | Failure::NoSpace { .. }
            | Failure::Checksum { .. }
            | Failure::Pieces { .. } => false,
            Failure::Truncated => true,
            Failure::Vault(_) => true,
        }
    }

    /// Le disque manque de place : la tâche attend en pause qu'il s'en libère.
    fn disk_full(&self) -> bool {
        match self {
            Failure::Io(e) => e.io_kind() == Some(std::io::ErrorKind::StorageFull),
            Failure::NoSpace { .. } => true,
            _ => false,
        }
    }

    fn detail(&self) -> String {
        let mib = |n: u64| format!("{:.1} Mio", n as f64 / (1024.0 * 1024.0));
        match self {
            Failure::Net(e) => e.to_string(),
            Failure::Io(e) => e.to_string(),
            Failure::Vault(e) => e.to_string(),
            Failure::Truncated => "le serveur a fermé la connexion avant la fin".into(),
            Failure::Checksum {
                algo,
                expected,
                got,
            } => format!(
                "empreinte {} du fichier : {got}, attendue : {expected}",
                algo.label()
            ),
            Failure::Pieces {
                algo,
                bad,
                count,
                rounds,
            } => format!(
                "{bad} morceaux sur {count} encore faux (empreintes {}) après {rounds} tours \
                 de reprise",
                algo.label()
            ),
            Failure::NoSpace { needed, free, keep } => format!(
                "espace disque insuffisant : {} encore à écrire et {} à laisser libres, \
                 {} libres",
                mib(*needed),
                mib(*keep),
                mib(*free)
            ),
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
/// La tâche doit être `queued` ou `retry_wait`, ou `active` ou `verifying` après un arrêt
/// brutal du moteur (SPEC §7.4 et §7.9). `on_progress` est appelé régulièrement pour l'affichage.
pub async fn run(
    conn: &mut Connection,
    client: &Client,
    task_id: &str,
    opts: &Options,
    mut stop: watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Outcome, TransferError> {
    let task = load_task(conn, task_id)?;
    match task.state {
        TaskState::Queued | TaskState::RetryWait => {
            change_state(conn, task_id, &Change::to(TaskState::Active))?;
        }
        // `verifying` : arrêt brutal pendant le calcul de l'empreinte, qui reprend (F-11).
        TaskState::Active | TaskState::Verifying => {}
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
        f if f.disk_full() => {
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

/// En-têtes de la tâche, secrets lus dans le coffre. Un secret qui n'y est plus (coffre en
/// mémoire après un redémarrage du moteur, effacé à la main) : l'en-tête n'est pas envoyé,
/// et le serveur dira s'il en avait besoin. Un coffre qui ne répond pas : échec temporaire.
async fn request_headers(
    stored: Vec<StoredHeader>,
    task_id: &str,
    vault: &Vault,
) -> Step<Vec<(String, String)>> {
    let mut list = Vec::new();
    for (name, stored) in stored {
        let id = match stored {
            Stored::Plain(value) => {
                list.push((name, value));
                continue;
            }
            Stored::Secret(id) => id,
        };
        let vault = vault.clone();
        match tokio::task::spawn_blocking(move || vault.get(&id)).await? {
            Ok(Some(secret)) => list.push((name, secret)),
            Ok(None) => {
                tracing::warn!(tache = %task_id, entete = %name, "en-tête absent du coffre, non envoyé")
            }
            Err(e) => return fail(Failure::Vault(e)),
        }
    }
    Ok(Ok(list))
}

/// Identifiants enregistrés pour le site de `url` (SPEC §7.14). Aucun si la tâche porte
/// son propre en-tête `Authorization` : il passe avant.
fn site_login(
    conn: &Connection,
    url: &str,
    task_headers: &[(String, String)],
) -> Result<Option<LoginRow>, TransferError> {
    if task_headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    {
        return Ok(None);
    }
    Ok(logins::best_for(conn, url)?)
}

/// Lit dans le coffre le mot de passe de `login`. Absent du coffre (coffre en mémoire après
/// un redémarrage) : la tâche part sans, et le serveur dira s'il en avait besoin. Un coffre
/// qui ne répond pas : échec temporaire, comme pour les en-têtes.
async fn reveal_login(login: Option<LoginRow>, vault: &Vault) -> Step<Option<Auth>> {
    let Some(login) = login else {
        return Ok(Ok(None));
    };
    let Ok(site) = Site::parse(&login.site) else {
        return Ok(Ok(None));
    };
    let (vault, id) = (vault.clone(), login.secret_ref.clone());
    match tokio::task::spawn_blocking(move || vault.get(&id)).await? {
        Ok(Some(password)) => Ok(Ok(Some(Auth::new(Login {
            site,
            username: login.username,
            password,
        })))),
        Ok(None) => {
            tracing::warn!(host = %login.site, "mot de passe absent du coffre, non envoyé");
            Ok(Ok(None))
        }
        Err(e) => fail(Failure::Vault(e)),
    }
}

async fn execute(
    conn: &mut Connection,
    client: &Client,
    task_id: &str,
    opts: &Options,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Step<PathBuf> {
    let mut task = load_task(conn, task_id)?;
    if task.state == TaskState::Verifying {
        match resume_verifying(conn, &task, stop, on_progress).await? {
            Ok(Finish::Done(path)) => return Ok(Ok(path)),
            // Des morceaux faux (F-12) : la suite les télécharge de nouveau.
            Ok(Finish::Redo) => task = load_task(conn, task_id)?,
            Err(stop) => return Ok(Err(stop)),
        }
    }
    let source = primary_source(conn, task_id)?;
    let stored = stored_headers::load(conn, task_id)?;
    let task_headers = match request_headers(stored, task_id, &opts.vault).await? {
        Ok(list) => list,
        Err(stop) => return Ok(Err(stop)),
    };
    // Identifiants du site (SPEC §7.14), sauf si la tâche porte son propre `Authorization`.
    let login = site_login(conn, &source.url, &task_headers)?;
    let auth = match reveal_login(login, &opts.vault).await? {
        Ok(auth) => auth.map(Arc::new),
        Err(stop) => return Ok(Err(stop)),
    };

    // Préparation (SPEC §7.1), une seule fois par tâche.
    if task.file_name.is_none() {
        let probe = match client
            .probe_as(&source.url, &task_headers, auth.as_deref())
            .await
        {
            Ok(probe) => probe,
            Err(e) => return fail(Failure::Net(e)),
        };
        // Empreinte lue dans un fichier voisin (F-11, SPEC §7.9), enregistrée avant la
        // préparation : après un arrêt entre les deux, la recherche n'est pas refaite.
        if opts.neighbor_checksum && task.checksum.is_none() {
            let names = [probe.file_name.as_str()];
            let lookup = sums::find(client, &source.url, &names);
            tokio::pin!(lookup);
            // Émetteur disparu (exécution sans planificateur) : plus aucun arrêt ne viendra.
            let mut gone = false;
            let found = loop {
                tokio::select! {
                    found = &mut lookup => break found,
                    asked = async { stop.wait_for(|h| *h).await.is_ok() }, if !gone => {
                        if asked {
                            return Ok(Err(Stop::User));
                        }
                        gone = true;
                    }
                }
            };
            if let Some((checksum, origin)) = found {
                set_checksum(conn, task_id, Some(&checksum), Some(&origin))?;
            }
        }
        // Le nom choisi par l'utilisateur passe avant celui du serveur (SPEC §7.12).
        let named = task.requested_name.as_deref().unwrap_or(&probe.file_name);
        // Règles de rangement (SPEC §7.21) : dossier et nom, avant de prendre le nom.
        let host = crate::net::parse_url(&source.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned));
        let subject = crate::rules::Subject {
            url: &source.url,
            host: host.as_deref(),
            name: named,
        };
        let placement = crate::store::rules::place_task(conn, &task, &subject, true)?;
        let dest_dir = match &placement.dir {
            Some(dir) => {
                if let Err(source) = std::fs::create_dir_all(dir) {
                    return fail(Failure::Io(io::IoError::File {
                        path: dir.clone(),
                        source,
                    }));
                }
                dir.to_string_lossy().into_owned()
            }
            None => task.dest_dir.clone(),
        };
        let wanted = placement.name.as_deref().unwrap_or(named);
        // Le nom est pris sur le disque (`.part` créé) avant d'être enregistré : aucune
        // autre tâche ne peut plus le choisir, ni ouvrir ce `.part` comme le sien.
        let reserved = reserved_names(conn, &dest_dir, task_id)?;
        // Réservation notée d'abord, effacée par `save_preparation` (SPEC §5.6, règle 7).
        let dir = Path::new(&dest_dir);
        let claimed = crate::claims::claim(conn, dir, wanted, &reserved, ClaimKind::Part, task_id);
        let name = match claimed {
            Ok(name) => name,
            Err(crate::claims::ClaimError::Store(e)) => return Err(e.into()),
            Err(crate::claims::ClaimError::Disk(source)) => {
                return fail(Failure::Io(io::IoError::File {
                    path: dir.to_path_buf(),
                    source,
                }));
            }
        };
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
        let placed = crate::store::rules::Placed::of(&placement, &dest_dir);
        save_preparation_placed(conn, task_id, &prep, &segments, &placed)?;
    }
    // Un tour de plus quand la vérification trouve des morceaux faux (F-12, SPEC §7.9) :
    // leurs segments rouverts se téléchargent, puis le fichier se vérifie de nouveau.
    loop {
        let task = load_task(conn, task_id)?;
        match download(
            conn,
            client,
            &task,
            &task_headers,
            &auth,
            opts,
            stop,
            on_progress,
        )
        .await?
        {
            Ok(Finish::Done(path)) => return Ok(Ok(path)),
            Ok(Finish::Redo) => continue,
            Err(stop) => return Ok(Err(stop)),
        }
    }
}

/// Un téléchargement préparé, de la reprise du `.part` à sa fin : connexions, puis
/// vérification et nom final.
#[allow(clippy::too_many_arguments)]
async fn download(
    conn: &mut Connection,
    client: &Client,
    task: &TaskRow,
    task_headers: &[(String, String)],
    auth: &Option<Arc<Auth>>,
    opts: &Options,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Step<Finish> {
    let task_id = task.id.as_str();
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
                crate::postproc::finish(conn, task_id, &dest, total)?;
                return Ok(Ok(Finish::Done(dest)));
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
                crate::postproc::finish(conn, task_id, &dest, len)?;
                return Ok(Ok(Finish::Done(dest)));
            }
            Err(e) => return fail(Failure::Io(e)),
        },
    };

    // Espace libre (SPEC §7.1, étape 5, et F-18) : de quoi finir, plus la réserve.
    let dir = Path::new(&task.dest_dir);
    let needed = total.map_or(0, |total| io::space_needed(part.path(), total));
    if let Some(f) = opts.short_of_space(dir, needed) {
        return fail(f);
    }
    // Réservation de la place (F-10) : un manque se voit maintenant, pas au bout de
    // plusieurs heures. Après la reprise du `.part`, dont la longueur juge ce qui manque.
    if let (true, Some(total)) = (opts.preallocate, total)
        && let Err(e) = part.preallocate(total)
    {
        return fail(Failure::Io(e));
    }

    let ranged = source.accepts_ranges == Some(true);
    let fetch = match total {
        Some(total) if ranged => Fetch::Ranges { total },
        total => Fetch::Whole { total },
    };
    let plan = if ranged {
        plan_for(client, &source.url, task, opts)
    } else {
        Plan::Fixed(1)
    };
    let (targets, checks) =
        match mirror_targets(conn, task_id, &source, task_headers, &fetch, opts).await? {
            Ok(found) => found,
            Err(stop) => return Ok(Err(stop)),
        };
    let mut all = vec![Arc::new(Target::of(
        &source,
        task_headers.to_vec(),
        auth.clone(),
    ))];
    all.extend(targets);
    part.checkpoint_after(opts.checkpoint_bytes);
    let part = Arc::new(part);
    let ctx = Pool {
        client,
        targets: all,
        checks,
        fetch,
        plan,
        host: host_of(&source.url),
        opts,
        dir,
        part: Arc::clone(&part),
        persist: total.is_some(),
    };
    // Rien à recevoir : le `.part` était complet (arrêt pendant le calcul de l'empreinte).
    let received = match (todo.is_empty(), total) {
        (true, Some(total)) => total,
        _ => match pool(conn, task_id, &ctx, todo, stop, on_progress).await? {
            Ok(received) => received,
            Err(stop) => return Ok(Err(stop)),
        },
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
    let sealed = match tokio::task::spawn_blocking(move || part.seal()).await? {
        Ok(sealed) => sealed,
        Err(e) => return fail(Failure::Io(e)),
    };
    finish_part(
        conn,
        task_id,
        sealed,
        total.unwrap_or(received),
        stop,
        on_progress,
    )
    .await
}

/// Miroirs d'une exécution (SPEC §7.7) : ceux déjà validés deviennent des cibles, ceux à
/// valider le seront pendant le téléchargement. Seulement avec des plages et une taille.
/// Une source validée puis écartée (fichier changé, échecs) est réessayée à chaque
/// exécution ; un miroir refusé à sa validation (jamais validé) ne l'est plus.
async fn mirror_targets(
    conn: &mut Connection,
    task_id: &str,
    reference: &SourceRow,
    task_headers: &[(String, String)],
    fetch: &Fetch,
    opts: &Options,
) -> Step<(Vec<Arc<Target>>, Vec<PendingCheck>)> {
    let (mut targets, mut checks) = (Vec::new(), Vec::new());
    let Fetch::Ranges { total } = *fetch else {
        return Ok(Ok((targets, checks)));
    };
    for mirror in load_sources(conn, task_id)?
        .into_iter()
        .filter(|s| s.rank > 0)
    {
        let validated = mirror.content_length.is_some();
        let pending = mirror.state == SourceState::Unknown;
        if !pending && !(validated && mirror.content_length == Some(total)) {
            continue;
        }
        let headers = mirrors::headers_for(task_headers, &reference.url, &mirror.url);
        let login = site_login(conn, &mirror.url, &headers)?;
        // Un mot de passe du coffre illisible pour un miroir : il part sans.
        let auth = match reveal_login(login, &opts.vault).await? {
            Ok(auth) => auth.map(Arc::new),
            Err(_) => None,
        };
        if pending {
            checks.push(PendingCheck {
                source_id: mirror.id,
                url: mirror.url,
                headers,
                auth,
            });
        } else {
            targets.push(Arc::new(Target::of(&mirror, headers, auth)));
        }
    }
    Ok(Ok((targets, checks)))
}

/// Issue de la fin d'un téléchargement.
enum Finish {
    /// Le fichier a son nom final.
    Done(PathBuf),
    /// Des morceaux faux (F-12, SPEC §7.9) : leurs segments sont rouverts, la tâche est
    /// repassée à `active`.
    Redo,
}

/// Fin d'une tâche dont le `.part` est complet et vidé sur disque : son empreinte, s'il
/// en a une, est vérifiée en `verifying` (F-11, SPEC §7.9) ; puis le `.part` prend son
/// nom et la tâche se termine. Une empreinte différente laisse le `.part` à sa place,
/// sous son nom provisoire, et la tâche en échec `CHECKSUM_MISMATCH`.
///
/// Avec des empreintes de morceaux (F-12), une seule lecture donne celle de chaque morceau
/// et celle du fichier. Des morceaux faux : un tour de reprise s'enregistre
/// ([`store::pieces::start_round`]) et l'appelant les télécharge de nouveau ; après
/// [`pieces::MAX_ROUNDS`] tours, ou sans plages pour les redemander, l'échec.
async fn finish_part(
    conn: &mut Connection,
    task_id: &str,
    sealed: SealedPart,
    total: u64,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Step<Finish> {
    // Relue ici : l'empreinte a pu changer pendant le téléchargement (`task.update`).
    let task = load_task(conn, task_id)?;
    // Des morceaux qui conviennent à la taille reçue ; sinon, l'empreinte entière seule.
    let pieces = store::pieces::load(conn, task_id)?.filter(|p| p.pieces.fits(total));
    if task.checksum.is_some() || pieces.is_some() {
        // Les octets sont déjà vidés (`seal`) : `verifying` décrit un `.part` complet.
        if task.state != TaskState::Verifying {
            change_state(conn, task_id, &Change::to(TaskState::Verifying))?;
        }
    }
    if let Some(found) = pieces {
        let (algo, length) = (found.pieces.algo(), found.pieces.length());
        let whole = task.checksum.as_ref().map(Checksum::algo);
        let got = match read_part(
            sealed.path(),
            total,
            stop,
            on_progress,
            move |path, read| checksum::hash_pieces(path, algo, length, whole, read),
        )
        .await?
        {
            Ok(got) => got,
            Err(stop) => return Ok(Err(stop)),
        };
        let bad = found.pieces.bad(&got.pieces);
        if !bad.is_empty() {
            // Les morceaux se redemandent par plages, à une tâche segmentée.
            let ranged = primary_source(conn, task_id)?.accepts_ranges == Some(true)
                && !load_segments(conn, task_id)?.is_empty();
            if found.rounds >= pieces::MAX_ROUNDS || !ranged {
                return fail(Failure::Pieces {
                    algo,
                    bad: bad.len(),
                    count: found.pieces.count(),
                    rounds: found.rounds,
                });
            }
            // Le `.part` est fermé : la reprise le rouvre pour les morceaux.
            drop(sealed);
            let each: Vec<(u64, u64)> = bad.iter().map(|&i| found.pieces.range(i, total)).collect();
            let merged = found.pieces.ranges(&bad, total);
            let set_aside = store::pieces::start_round(conn, task_id, &each, &merged)?;
            tracing::warn!(
                tache = task_id,
                morceaux = bad.len(),
                tour = found.rounds + 1,
                sources_ecartees = set_aside.len(),
                "morceaux faux, téléchargés de nouveau"
            );
            return Ok(Ok(Finish::Redo));
        }
        if let (Some(expected), Some(got)) = (&task.checksum, got.whole)
            && got != expected.value()
        {
            return fail(Failure::Checksum {
                algo: expected.algo(),
                expected: expected.value().to_owned(),
                got,
            });
        }
    } else if let Some(expected) = task.checksum {
        let algo = expected.algo();
        let got = match read_part(
            sealed.path(),
            total,
            stop,
            on_progress,
            move |path, read| checksum::hash_file(path, algo, read),
        )
        .await?
        {
            Ok(got) => got,
            Err(stop) => return Ok(Err(stop)),
        };
        if got != expected.value() {
            return fail(Failure::Checksum {
                algo,
                expected: expected.value().to_owned(),
                got,
            });
        }
    }
    let path = match tokio::task::spawn_blocking(move || sealed.finish()).await? {
        Ok(path) => path,
        Err(e) => return fail(Failure::Io(e)),
    };
    crate::postproc::finish(conn, task_id, &path, total)?;
    Ok(Ok(Finish::Done(path)))
}

/// Reprise d'une tâche restée en `verifying` (arrêt brutal pendant le calcul) : le
/// `.part` complet est vérifié de nouveau, depuis le début. S'il n'est plus là mais que le
/// fichier final l'est, l'arrêt a eu lieu entre le renommage, qui ne suit qu'une
/// empreinte juste, et l'enregistrement de la fin.
async fn resume_verifying(
    conn: &mut Connection,
    task: &TaskRow,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Step<Finish> {
    let file_name = task
        .file_name
        .as_deref()
        .ok_or_else(|| StoreError::BadValue(format!("tâche {} : nom absent", task.id)))?;
    let dest = Path::new(&task.dest_dir).join(file_name);
    let part = io::part_path(&dest);
    let len = |path: &Path| std::fs::metadata(path).map(|m| m.len());
    match len(&part) {
        Ok(total) => {
            let sealed = SealedPart::existing(&dest);
            finish_part(conn, &task.id, sealed, total, stop, on_progress).await
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match len(&dest) {
            Ok(total) => {
                crate::postproc::finish(conn, &task.id, &dest, total)?;
                Ok(Ok(Finish::Done(dest)))
            }
            Err(_) => fail(Failure::Io(IoError::File {
                path: part,
                source: e,
            })),
        },
        Err(source) => fail(Failure::Io(IoError::File { path: part, source })),
    }
}

/// Lit le `.part` complet (`total` octets) par `work` (calcul d'empreintes), hors de la
/// boucle asynchrone ; l'avancement s'annonce toutes les demi-secondes, et `stop`
/// interrompt la lecture (la tâche passe alors en pause et la recommencera à la reprise).
/// `work` reçoit le chemin et la fonction à rappeler après chaque lecture.
async fn read_part<T: Send + 'static>(
    path: &Path,
    total: u64,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
    work: impl FnOnce(&Path, &mut dyn FnMut(u64) -> bool) -> std::io::Result<Option<T>> + Send + 'static,
) -> Step<T> {
    let report = |read: u64| Progress {
        received: total,
        total: Some(total),
        connections: 0,
        segments: Arc::from([]),
        verified: Some(read),
        uploaded: None,
        upload_bps: None,
        seeding: false,
        merging: false,
    };
    on_progress(report(0));
    let read = Arc::new(AtomicU64::new(0));
    // Arrêt déjà demandé : le calcul s'arrête après sa première lecture.
    let mut stopped = *stop.borrow();
    let halt = Arc::new(AtomicBool::new(stopped));
    let mut job = {
        let path = path.to_owned();
        let (read, halt) = (Arc::clone(&read), Arc::clone(&halt));
        tokio::task::spawn_blocking(move || {
            work(&path, &mut |n| {
                read.store(n, Ordering::Relaxed);
                !halt.load(Ordering::Relaxed)
            })
            .map_err(|source| IoError::File { path, source })
        })
    };
    let mut display = interval_at(
        Instant::now() + Duration::from_millis(500),
        Duration::from_millis(500),
    );
    display.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Émetteur disparu (exécution sans planificateur) : plus aucun arrêt ne viendra.
    let mut gone = false;
    let result = loop {
        tokio::select! {
            result = &mut job => break result?,
            _ = display.tick() => on_progress(report(read.load(Ordering::Relaxed))),
            asked = async { stop.wait_for(|h| *h).await.is_ok() }, if !stopped && !gone => {
                if asked {
                    stopped = true;
                    halt.store(true, Ordering::Relaxed);
                } else {
                    gone = true;
                }
            }
        }
    };
    match result {
        Ok(Some(got)) if !stopped => {
            on_progress(report(total));
            Ok(Ok(got))
        }
        Ok(_) => Ok(Err(Stop::User)),
        Err(e) => fail(Failure::Io(e)),
    }
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
    // Tous les segments finis, donc vidés (règle 1) : le `.part` est complet, même sans
    // plages (arrêt pendant le calcul de l'empreinte, F-11).
    let complete = size >= total && segments.iter().all(|s| s.state == SegmentState::Done);
    let lost = if complete {
        Vec::new()
    } else if source.accepts_ranges == Some(true) {
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
    /// Requêtes par plage, avec l'`If-Range` de chaque source.
    Ranges { total: u64 },
    /// Un seul flux depuis le début ; taille inconnue si `None`.
    Whole { total: Option<u64> },
}

/// Pause avant de rouvrir une connexion après un refus du serveur (SPEC §7.3), sauf
/// `Retry-After`, plafonné à [`REFUSAL_PAUSE_MAX`].
pub const REFUSAL_PAUSE: Duration = Duration::from_secs(1);
pub const REFUSAL_PAUSE_MAX: Duration = Duration::from_secs(30);
/// Épisodes de refus d'affilée, sans connexion finie entre eux, à partir desquels le
/// moteur descend aux connexions encore actives, même sous ce que le serveur a déjà
/// accepté ensemble.
pub const REFUSALS_TO_LOWER: u32 = 3;

/// Nombre de connexions automatique (SPEC §7.3), comme le démarrage de TCP : montée
/// rapide tant qu'elle paie, réduction rapide quand le serveur résiste.
///
/// À chaque fenêtre de mesure, le nombre de connexions double (dans la limite du plafond)
/// si le débit a progressé d'au moins 10 % depuis la dernière augmentation ; sinon
/// l'augmentation s'arrête pour cette exécution. Sur un refus du serveur, les connexions
/// encore actives deviennent la cible et le plafond.
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

    /// Le serveur refuse une connexion (429, 503, connexion refusée) alors que `alive`
    /// autres tournent encore : celles-là, il les accepte. Elles deviennent la cible et le
    /// plafond (au moins une) ; renvoie le plafond à retenir pour l'hôte pendant la session.
    ///
    /// Une salve de refus (montée de 8 à 16 face à un serveur qui en accepte 8) ne fait
    /// donc descendre qu'à ce que le serveur accepte, et non de moitié à chaque refus :
    /// avant le 2026-11-07, 16, 8, 4, 2 puis 1 connexion.
    pub fn on_overload(&mut self, alive: u32) -> u32 {
        self.width = alive.clamp(1, self.width);
        self.cap = self.cap.min(self.width);
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

pub(crate) fn host_of(url: &str) -> Option<String> {
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

/// Adresse d'une tâche et en-têtes de ses requêtes, secrets compris (SPEC §5.1), et les
/// connexions dont le serveur a accepté la requête (SPEC §7.3).
struct Target {
    /// Source de la tâche (SPEC §7.7) : la référence ou un miroir.
    source_id: Arc<str>,
    url: String,
    headers: Vec<(String, String)>,
    /// Identifiants du site, s'il en a (SPEC §7.14).
    auth: Option<Arc<Auth>>,
    /// `If-Range` de cette source (SPEC §7.4) : `ETag` fort ou date, propres à chaque
    /// serveur.
    if_range: Option<String>,
    /// Connexions acceptées et ouvertes en ce moment.
    open: AtomicUsize,
    /// Le plus de connexions acceptées en même temps pendant l'exécution : ce que le
    /// serveur tolère, au moins.
    peak: AtomicUsize,
    /// Octets reçus de cette source pendant l'exécution, pour son débit (SPEC §7.7).
    received: AtomicU64,
    /// Morceaux faux déjà envoyés (F-12, SPEC §7.9) : une source qui en a moins passe
    /// avant elle.
    bad_pieces: u32,
}

impl Target {
    fn new(
        source_id: String,
        url: String,
        headers: Vec<(String, String)>,
        auth: Option<Arc<Auth>>,
        if_range: Option<String>,
    ) -> Self {
        Target {
            source_id: source_id.into(),
            url,
            headers,
            auth,
            if_range,
            open: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            received: AtomicU64::new(0),
            bad_pieces: 0,
        }
    }

    /// Cible d'une source enregistrée : son `If-Range` vient de ses validateurs (un
    /// `ETag` fort et bien formé, sinon la date, RFC 9110 §13.1.5).
    fn of(source: &SourceRow, headers: Vec<(String, String)>, auth: Option<Arc<Auth>>) -> Self {
        let if_range =
            headers::if_range_validator(source.etag.as_deref(), source.last_modified.as_deref());
        Target {
            bad_pieces: source.bad_pieces,
            ..Target::new(
                source.id.clone(),
                source.url.clone(),
                headers,
                auth,
                if_range,
            )
        }
    }

    /// Le serveur a accepté une requête : compte la connexion jusqu'à la fin du garde.
    fn accepted(&self) -> Accepted<'_> {
        let now = self.open.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        Accepted(self)
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

/// Une connexion acceptée, comptée dans [`Target::open`] tant qu'elle vit.
struct Accepted<'a>(&'a Target);

impl Drop for Accepted<'_> {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Ce que partagent les connexions d'une exécution.
struct Pool<'a> {
    client: &'a Client,
    /// Sources utilisables au départ : la référence d'abord, puis les miroirs déjà validés
    /// (SPEC §7.7). Une tâche sans plages n'a que la référence.
    targets: Vec<Arc<Target>>,
    /// Miroirs à valider pendant le téléchargement ; acceptés, ils s'y joignent.
    checks: Vec<PendingCheck>,
    fetch: Fetch,
    plan: Plan,
    /// Hôte de la source, pour le plafond appris d'un serveur qui résiste.
    host: Option<String>,
    opts: &'a Options,
    /// Dossier de destination, dont l'espace libre est contrôlé.
    dir: &'a Path,
    part: Arc<PartFile>,
    /// Taille connue : les compteurs sont enregistrés en base.
    persist: bool,
}

/// Fin d'une connexion : compteur vidé sur disque et issue.
struct ConnEnd {
    idx: u32,
    /// Rang de sa source dans les sources de l'exécution.
    slot: usize,
    progress: Result<SyncedProgress, IoError>,
    result: Result<(), Failure>,
}

/// Un miroir à valider pendant le téléchargement (SPEC §7.7).
#[derive(Clone)]
struct PendingCheck {
    source_id: String,
    url: String,
    headers: Vec<(String, String)>,
    auth: Option<Arc<Auth>>,
}

/// Issue de la validation d'un miroir.
struct CheckEnd {
    check: PendingCheck,
    verdict: mirrors::Verdict,
}

/// Valide un miroir face à la référence (taille et échantillons).
async fn check_mirror(
    client: Client,
    reference: Arc<Target>,
    check: PendingCheck,
    total: u64,
) -> CheckEnd {
    let reference_side = mirrors::Side {
        url: &reference.url,
        headers: &reference.headers,
        auth: reference.auth.as_deref(),
        if_range: reference.if_range.as_deref(),
    };
    let side = mirrors::Side {
        url: &check.url,
        headers: &check.headers,
        auth: check.auth.as_deref(),
        if_range: None,
    };
    let verdict = mirrors::validate(&client, &reference_side, &side, total).await;
    CheckEnd { check, verdict }
}

/// Échecs d'affilée d'une source avant son repos (SPEC §7.7).
const SOURCE_STRIKES: u32 = 3;
/// Repos d'une source après [`SOURCE_STRIKES`] échecs : doublé à chaque fois, plafonné.
const SOURCE_REST: Duration = Duration::from_secs(30);
const SOURCE_REST_MAX: Duration = Duration::from_secs(600);
/// Mesures de débit (une par affichage) avant de juger une source lente.
const SLOW_AFTER_MEASURES: u32 = 4;
/// Écart minimal entre deux mesures de débit.
const MIN_MEASURE_SECONDS: f64 = 0.2;

/// État d'une source pendant l'exécution.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SlotState {
    /// Reçoit de nouveaux segments.
    Usable,
    /// Trop lente : garde ses connexions, n'en reçoit plus (SPEC §7.7).
    Slow,
    /// Au repos jusqu'à cet instant, après des échecs d'affilée.
    Resting(Instant),
    /// Écartée pour cette exécution : fichier changé, refus du serveur.
    Dropped,
}

/// Une source de l'exécution : sa cible, son état et son débit mesuré.
struct Slot {
    target: Arc<Target>,
    state: SlotState,
    /// Connexions ouvertes sur elle.
    connections: usize,
    /// Échecs d'affilée, et repos déjà pris.
    strikes: u32,
    rests: u32,
    /// Débit lissé (octets par seconde) et mesures prises.
    rate: f64,
    measures: u32,
    /// Octets reçus à la dernière mesure.
    last_received: u64,
}

impl Slot {
    fn new(target: Arc<Target>) -> Slot {
        Slot {
            target,
            state: SlotState::Usable,
            connections: 0,
            strikes: 0,
            rests: 0,
            rate: 0.0,
            measures: 0,
            last_received: 0,
        }
    }

    /// Un échec qui tient à elle. Définitif : écartée. Sinon, au repos après
    /// [`SOURCE_STRIKES`] échecs d'affilée. Rend l'état à enregistrer, s'il change.
    fn on_failure(&mut self, gone: bool, now: Instant) -> Option<SlotState> {
        if gone {
            self.state = SlotState::Dropped;
            return Some(self.state);
        }
        self.strikes += 1;
        if self.strikes < SOURCE_STRIKES {
            return None;
        }
        self.strikes = 0;
        let rest = SOURCE_REST
            .saturating_mul(1 << self.rests.min(8))
            .min(SOURCE_REST_MAX);
        self.rests += 1;
        self.state = SlotState::Resting(now + rest);
        Some(self.state)
    }
}

/// Source à qui confier une nouvelle connexion : parmi celles qui reçoivent des segments
/// et ont moins de `width` connexions ouvertes, celle dont le débit par connexion ouverte serait le meilleur ; une source pas encore
/// mesurée compte pour la médiane des autres, et, à égalité, la référence passe avant.
///
/// Morceaux faux (F-12, SPEC §7.9) : seules les sources qui en ont envoyé le moins sont
/// choisies, parmi celles qui reçoivent des segments ; les autres servent quand celles-ci
/// ne le peuvent plus (écartées, au repos).
fn pick_slot(slots: &[Slot], width: usize) -> Option<usize> {
    let measured: Vec<f64> = slots
        .iter()
        .filter(|s| s.state == SlotState::Usable && s.measures > 0)
        .map(|s| s.rate)
        .collect();
    let fallback = median(&measured).unwrap_or(1.0).max(1.0);
    let fewest = slots
        .iter()
        .filter(|s| s.state == SlotState::Usable)
        .map(|s| s.target.bad_pieces)
        .min()?;
    slots
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.state == SlotState::Usable && s.connections < width && s.target.bad_pieces == fewest
        })
        .map(|(i, s)| {
            let rate = if s.measures > 0 {
                s.rate.max(1.0)
            } else {
                fallback
            };
            (i, rate / (s.connections + 1) as f64)
        })
        .fold(None, |best: Option<(usize, f64)>, (i, score)| match best {
            Some((_, b)) if b >= score => best,
            _ => Some((i, score)),
        })
        .map(|(i, _)| i)
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    Some(sorted[sorted.len() / 2])
}

/// Débits de chaque source, depuis la mesure précédente il y a `elapsed` ; une source qui
/// tombe sous le quart de la médiane des autres passe à `slow` (SPEC §7.7). Rend les
/// sources devenues lentes ; `None` si la mesure est remise (trop peu de temps écoulé).
fn measure_slots(slots: &mut [Slot], elapsed: f64) -> Option<Vec<usize>> {
    // Trop tôt (premier affichage, au départ) : la mesure attend, les octets s'ajoutent.
    if elapsed < MIN_MEASURE_SECONDS {
        return None;
    }
    for slot in slots.iter_mut() {
        let received = slot.target.received.load(Ordering::Relaxed);
        let delta = received.saturating_sub(slot.last_received);
        slot.last_received = received;
        if slot.connections == 0 && delta == 0 {
            continue;
        }
        let now = delta as f64 / elapsed;
        slot.rate = if slot.measures == 0 {
            now
        } else {
            0.7 * slot.rate + 0.3 * now
        };
        slot.measures += 1;
    }
    let rates: Vec<f64> = slots
        .iter()
        .filter(|s| s.state == SlotState::Usable && s.measures >= SLOW_AFTER_MEASURES)
        .map(|s| s.rate)
        .collect();
    let mut slowed = Vec::new();
    if rates.len() < 2 {
        return Some(slowed);
    }
    let median = median(&rates).unwrap_or(0.0);
    for (i, slot) in slots.iter_mut().enumerate() {
        if slot.state == SlotState::Usable
            && slot.measures >= SLOW_AFTER_MEASURES
            && slot.rate < median / 4.0
        {
            slot.state = SlotState::Slow;
            slowed.push(i);
        }
    }
    Some(slowed)
}

/// Enregistre l'état et le débit d'une source.
fn save_slot(conn: &Connection, slot: &Slot, reason: Option<&str>) -> Result<(), TransferError> {
    let bps = (slot.measures > 0).then_some(slot.rate as u64);
    let state = match slot.state {
        SlotState::Usable => SourceState::Ok,
        SlotState::Slow => SourceState::Slow,
        SlotState::Resting(_) | SlotState::Dropped => SourceState::Failed,
    };
    sources::save_run(conn, &slot.target.source_id, state, bps, reason)?;
    Ok(())
}

/// Boucle de coordination : remplit les connexions libres avec les segments à faire,
/// enregistre les compteurs et s'arrête au premier échec ou sur `stop`. Renvoie les
/// octets écrits au total.
///
/// Miroirs (SPEC §7.7, phase 6) : chaque connexion va à une source choisie par
/// [`pick_slot`] ; un échec qui tient à une source, tant qu'une autre reste, ne fait que
/// l'écarter ou la mettre au repos, et son segment va aux autres. Le dernier recours suit
/// la règle d'une source unique.
async fn pool(
    conn: &mut Connection,
    task_id: &str,
    ctx: &Pool<'_>,
    todo: Vec<Segment>,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Step<u64> {
    let mut todo: VecDeque<Segment> = todo.into();
    // Octets enregistrés de chaque segment ; ceux en cours sont lus sur leur suivi.
    let mut saved: HashMap<u32, u64> = load_segments(conn, task_id)?
        .iter()
        .map(|s| (s.idx, s.written))
        .collect();
    let total = match ctx.fetch {
        Fetch::Ranges { total } => Some(total),
        Fetch::Whole { total } => total,
    };
    let mut slots: Vec<Slot> = ctx.targets.iter().cloned().map(Slot::new).collect();
    let mut checks: JoinSet<CheckEnd> = JoinSet::new();
    if let Fetch::Ranges { total } = ctx.fetch {
        for check in &ctx.checks {
            checks.spawn(check_mirror(
                ctx.client.clone(),
                Arc::clone(&slots[0].target),
                check.clone(),
                total,
            ));
        }
    }
    let (halt_tx, halt_rx) = watch::channel(false);
    let mut running: JoinSet<ConnEnd> = JoinSet::new();
    let mut handles: HashMap<u32, ProgressHandle> = HashMap::new();
    // Source de chaque segment en cours, pour l'affichage (SPEC §7.7).
    let mut owners: HashMap<u32, Arc<str>> = HashMap::new();
    let mut failure: Option<Failure> = None;
    let (mut user_stop, mut stop_gone) = (false, false);

    let start = Instant::now();
    let mut checkpoint = interval_at(start + ctx.opts.checkpoint_every, ctx.opts.checkpoint_every);
    checkpoint.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut space = interval_at(start + ctx.opts.checkpoint_every, ctx.opts.checkpoint_every);
    space.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut display = interval_at(start, Duration::from_millis(500));
    display.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut rated = start;
    let (mut width, mut auto) = match &ctx.plan {
        Plan::Fixed(n) => (*n as usize, None),
        Plan::Auto(a) => (a.width() as usize, Some(a.clone())),
    };
    let mut measure = interval_at(start + ctx.opts.measure_every, ctx.opts.measure_every);
    measure.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut measured = (
        start,
        progress(&saved, &handles, &owners, total, 0).received,
    );
    // Refus du serveur (SPEC §7.3) : épisodes d'affilée sans connexion finie entre eux,
    // connexion finie depuis le dernier, et pause avant de rouvrir une connexion.
    let (mut strikes, mut ended_ok, mut hold): (u32, bool, Option<Instant>) = (0, false, None);

    loop {
        // Les sources au repos dont le délai est passé reçoivent de nouveau des segments.
        let now = Instant::now();
        for slot in &mut slots {
            if matches!(slot.state, SlotState::Resting(until) if until <= now) {
                slot.state = SlotState::Usable;
                save_slot(conn, slot, None)?;
            }
        }
        // Après un refus, pas de nouvelle connexion avant la fin de la pause.
        let held = hold.filter(|t| Instant::now() < *t);
        // Le nombre de connexions (`width`) vaut pour chaque source, comme les connexions
        // par serveur d'aria2 : un miroir accepté a les siennes (SPEC §7.7).
        while failure.is_none() && !user_stop && held.is_none() {
            let Some(pick) = pick_slot(&slots, width) else {
                break;
            };
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
            // La source de chaque segment est notée : celle d'un morceau faux est mise en
            // cause (F-12, SPEC §7.9).
            if ctx.persist {
                store::segments::set_source(
                    conn,
                    task_id,
                    segment.idx,
                    &slots[pick].target.source_id,
                )?;
            }
            let writer = ctx.part.writer(&segment, ctx.opts.buffer);
            handles.insert(segment.idx, writer.progress());
            owners.insert(segment.idx, Arc::clone(&slots[pick].target.source_id));
            slots[pick].connections += 1;
            running.spawn(connection(
                ctx.client.clone(),
                Arc::clone(&slots[pick].target),
                pick,
                ctx.fetch.clone(),
                segment.start,
                writer,
                ctx.opts.throttle.clone(),
                halt_rx.clone(),
            ));
        }
        if running.is_empty() {
            // Plus aucune connexion, mais des segments attendent : la fin d'une pause
            // après un refus, ou le retour d'une source au repos.
            let resting = slots
                .iter()
                .filter_map(|s| match s.state {
                    SlotState::Resting(until) => Some(until),
                    _ => None,
                })
                .min();
            let usable = slots.iter().any(|s| s.state == SlotState::Usable);
            let wake = match (held, usable) {
                (Some(until), _) => Some(until),
                (None, false) => resting,
                (None, true) => None,
            };
            match wake {
                Some(until) if failure.is_none() && !user_stop && !todo.is_empty() => {
                    tokio::select! {
                        _ = tokio::time::sleep_until(until) => {}
                        changed = async { stop.wait_for(|s| *s).await.is_ok() }, if !stop_gone => {
                            if changed {
                                user_stop = true;
                            } else {
                                stop_gone = true;
                            }
                        }
                    }
                    continue;
                }
                _ => break,
            }
        }

        let first_failure = |f: Failure, failure: &mut Option<Failure>| {
            if failure.is_none() {
                *failure = Some(f);
            }
            let _ = halt_tx.send(true);
        };
        // Un segment interrompu attend son tour depuis ses octets enregistrés.
        let requeue = |handle: Option<ProgressHandle>,
                       written: Option<u64>,
                       idx: u32,
                       todo: &mut VecDeque<Segment>| {
            if let (Some(h), Some(written)) = (handle, written) {
                let (_, last) = h.accepted_and_end();
                todo.push_front(Segment {
                    idx,
                    start: h.start(),
                    end: last,
                    written,
                    state: SegmentState::Pending,
                });
            }
        };
        tokio::select! {
            joined = running.join_next() => {
                let end = joined.expect("au moins une connexion")?;
                let handle = handles.remove(&end.idx);
                owners.remove(&end.idx);
                slots[end.slot].connections -= 1;
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
                // Une autre source qui travaille peut prendre le relais (SPEC §7.7). Une
                // source au repos ne compte pas : sans aucune autre, la règle d'une source
                // unique s'applique (nouvel essai par le planificateur, pause, échec), et
                // jamais une attente sans fin.
                let others = slots.iter().enumerate().any(|(i, s)| {
                    i != end.slot && matches!(s.state, SlotState::Usable | SlotState::Slow)
                });
                match end.result {
                    Ok(()) => {
                        ended_ok = true;
                        slots[end.slot].strikes = 0;
                    }
                    // Le serveur refuse une connexion alors que d'autres tournent : elles
                    // deviennent la cible et le plafond retenu pour l'hôte, et le segment
                    // attend son tour depuis ses octets enregistrés (SPEC §7.3).
                    Err(Failure::Net(e))
                        if e.is_overload()
                            && failure.is_none()
                            && (!running.is_empty() || strikes + 1 < REFUSALS_TO_LOWER)
                            && auto.is_some() =>
                    {
                        let a = auto.as_mut().expect("mode automatique");
                        let now = Instant::now();
                        // Un nouvel épisode de refus : le premier, ou un refus d'une connexion
                        // ouverte après la pause du précédent. Ceux qui arrivent pendant la
                        // pause sont de la même salve.
                        if hold.is_none_or(|t| now >= t) {
                            strikes = if ended_ok { 1 } else { strikes + 1 };
                            ended_ok = false;
                            let pause = e.retry_after().unwrap_or(REFUSAL_PAUSE);
                            hold = Some(now + pause.min(REFUSAL_PAUSE_MAX));
                        }
                        // Le serveur refuse encore alors qu'aucune connexion n'a fini entre
                        // deux épisodes : il a baissé sa limite, on descend aux connexions
                        // actives. Sinon (montée trop haute, ou remplacement d'une connexion
                        // que le serveur compte encore), pas sous ce qu'il a déjà accepté
                        // ensemble.
                        let keep = match strikes {
                            n if n >= REFUSALS_TO_LOWER => slots[end.slot].connections,
                            _ => slots[end.slot].connections.max(slots[end.slot].target.peak()),
                        };
                        let cap = a.on_overload(u32::try_from(keep).unwrap_or(u32::MAX));
                        width = a.width() as usize;
                        if let Some(host) = &ctx.host {
                            ctx.client.limit_host(host, cap);
                        }
                        requeue(handle, written, end.idx, &mut todo);
                    }
                    // Échec d'une source alors qu'une autre reste : seule cette source est
                    // écartée ou mise au repos (décision de l'utilisateur du 2026-11-10 :
                    // un fichier changé n'arrête pas la tâche s'il reste un miroir).
                    Err(f) if failure.is_none() && f.caused_by_source() && others => {
                        let slot = &mut slots[end.slot];
                        if let Some(_changed) = slot.on_failure(f.source_gone(), Instant::now()) {
                            save_slot(conn, slot, Some(&f.detail()))?;
                        }
                        requeue(handle, written, end.idx, &mut todo);
                    }
                    Err(f) => first_failure(f, &mut failure),
                }
            }
            // Un miroir validé pendant le téléchargement s'y joint ; refusé, il est écarté
            // avec la raison ; injoignable, il reste à valider.
            Some(done) = checks.join_next(), if !checks.is_empty() => {
                let Ok(CheckEnd { check, verdict }) = done else { continue };
                match verdict {
                    mirrors::Verdict::Accepted(facts) => {
                        let checked = sources::Checked {
                            accepts_ranges: true,
                            etag: facts.etag.as_deref(),
                            last_modified: facts.last_modified.as_deref(),
                            content_length: Some(facts.content_length),
                        };
                        sources::set_checked(conn, &check.source_id, &checked)?;
                        let if_range = headers::if_range_validator(
                            facts.etag.as_deref(),
                            facts.last_modified.as_deref(),
                        );
                        slots.push(Slot::new(Arc::new(Target::new(
                            check.source_id,
                            check.url,
                            check.headers,
                            check.auth,
                            if_range,
                        ))));
                    }
                    mirrors::Verdict::Refused(reason) => {
                        sources::set_failed(conn, &check.source_id, &reason)?;
                    }
                    mirrors::Verdict::Unreachable(e) => {
                        sources::set_unreachable(conn, &check.source_id, &e.to_string())?;
                    }
                    mirrors::Verdict::Reference(_) => {}
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
                // Débit de chaque source, pour l'affichage (avec plusieurs sources).
                if slots.len() > 1 {
                    for slot in slots.iter().filter(|s| s.measures > 0) {
                        sources::save_speed(conn, &slot.target.source_id, slot.rate as u64)?;
                    }
                }
            }
            // Assez d'octets reçus depuis le dernier point : le suivant tout de suite, puis
            // l'intervalle repart de lui (SPEC §7.5). Un signal gardé d'avant un point déjà
            // pris est ignoré.
            _ = ctx.part.volume_reached(), if ctx.persist && !handles.is_empty() => {
                if ctx.part.unsynced() >= ctx.opts.checkpoint_bytes {
                    checkpoint.reset_immediately();
                }
            }
            // Le disque se remplit (un autre programme, une autre tâche) : arrêt propre
            // avant que la réserve ne soit prise (F-18).
            _ = space.tick(), if failure.is_none() && !user_stop => {
                let (dir, opts) = (ctx.dir.to_owned(), ctx.opts.clone());
                let short = tokio::task::spawn_blocking(move || opts.short_of_space(&dir, 0)).await?;
                if let Some(f) = short {
                    first_failure(f, &mut failure);
                }
            }
            _ = measure.tick(),
                if auto.as_ref().is_some_and(|a| !a.is_frozen()) && failure.is_none() && !user_stop =>
            {
                let now = Instant::now();
                let received = progress(&saved, &handles, &owners, total, 0).received;
                let elapsed = now.duration_since(measured.0).as_secs_f64();
                if let (Some(a), true) = (auto.as_mut(), elapsed > 0.0) {
                    let rate = received.saturating_sub(measured.1) as f64 / elapsed;
                    width = a.on_rate(rate) as usize;
                }
                measured = (now, received);
            }
            _ = display.tick() => {
                on_progress(progress(&saved, &handles, &owners, total, running.len()));
                let now = Instant::now();
                if let Some(slowed) = measure_slots(&mut slots, now.duration_since(rated).as_secs_f64()) {
                    for i in slowed {
                        save_slot(conn, &slots[i], None)?;
                    }
                    rated = now;
                }
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
    on_progress(progress(&saved, &handles, &owners, total, 0));
    // Débits de cette exécution, gardés pour la suivante et pour l'affichage.
    if slots.len() > 1 {
        for slot in slots.iter().filter(|s| s.measures > 0) {
            sources::save_speed(conn, &slot.target.source_id, slot.rate as u64)?;
        }
    }

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
    owners: &HashMap<u32, Arc<str>>,
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
    let mut segments: Vec<LiveSegment> = handles
        .iter()
        // Chaque segment en cours a sa source : les deux tables changent ensemble.
        .filter_map(|(&idx, h)| {
            Some(LiveSegment {
                idx,
                received: h.handed_over().max(saved.get(&idx).copied().unwrap_or(0)),
                source_id: Arc::clone(owners.get(&idx)?),
            })
        })
        .collect();
    segments.sort_by_key(|s| s.idx);
    Progress {
        received,
        total,
        connections,
        segments: segments.into(),
        verified: None,
        uploaded: None,
        upload_bps: None,
        seeding: false,
        merging: false,
    }
}

/// Une connexion : télécharge le segment de `writer`, puis vide ce qu'elle a reçu.
#[allow(clippy::too_many_arguments)]
async fn connection(
    client: Client,
    target: Arc<Target>,
    slot: usize,
    fetch: Fetch,
    start: u64,
    mut writer: SegmentWriter,
    throttle: Throttle,
    mut halt: watch::Receiver<bool>,
) -> ConnEnd {
    let result = pump(
        &client,
        &target,
        &fetch,
        start,
        &mut writer,
        &throttle,
        &mut halt,
    )
    .await;
    let idx = writer.idx();
    let progress = tokio::task::spawn_blocking(move || writer.finish())
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
    ConnEnd {
        idx,
        slot,
        progress,
        result,
    }
}

/// Reçoit les octets du segment jusqu'à sa fin ou jusqu'à `halt`. Une réponse plus courte
/// que demandé est suivie d'une nouvelle requête pour le reste. Chaque lecture attend
/// que les seaux de débit le permettent, puis les paie (SPEC §7.6).
async fn pump(
    client: &Client,
    target: &Target,
    fetch: &Fetch,
    start: u64,
    writer: &mut SegmentWriter,
    throttle: &Throttle,
    halt: &mut watch::Receiver<bool>,
) -> Result<(), Failure> {
    while writer.remaining() > 0 {
        let first = start + writer.accepted();
        let last = first + writer.remaining() - 1;
        let request = async {
            match fetch {
                Fetch::Ranges { total } => {
                    let req = RangeRequest {
                        first,
                        last,
                        total: *total,
                        if_range: target.if_range.as_deref(),
                    };
                    client
                        .get_range_as(&target.url, &target.headers, &req, target.auth.as_deref())
                        .await
                }
                Fetch::Whole { total } => {
                    client
                        .get_all_as(&target.url, &target.headers, *total, target.auth.as_deref())
                        .await
                }
            }
        };
        let mut body = tokio::select! {
            biased;
            _ = halted(halt) => return Ok(()),
            body = request => body.map_err(Failure::Net)?,
        };
        let _accepted = target.accepted();
        let before = writer.accepted();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = halted(halt) => return Ok(()),
                chunk = async {
                    throttle.ready().await;
                    body.chunk().await
                } => chunk.map_err(Failure::Net)?,
            };
            let Some(chunk) = chunk else { break };
            throttle.pay(chunk.len());
            target
                .received
                .fetch_add(chunk.len() as u64, Ordering::Relaxed);
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
mod mirror_tests;
#[cfg(test)]
mod piece_tests;
#[cfg(test)]
mod tests;
