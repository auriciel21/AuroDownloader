//! Planificateur : files, priorités, nouvelles tentatives, reprise au démarrage.
//! Voir SPEC §2.3 (F-14, F-17), §5.5, §7.4 et §7.8.
//!
//! [`Engine`] est la poignée du moteur. Une seule boucle tient l'état : elle reçoit les
//! commandes (ajout, pause, reprise, annulation, arrêt) par un canal, démarre les tâches
//! quand leur file a de la place, et apprend la fin de chaque exécution. Rien ne peut
//! donc se croiser : une pause ne rencontre jamais un démarrage en cours.
//!
//! Chaque exécution a sa propre connexion SQLite (`transfer::run`) ; la boucle a la
//! sienne. Les changements d'état passent tous par `store::tasks` (SPEC §5.5) et sont
//! annoncés par des [`Event`].
//!
//! Limites de débit (F-15, SPEC §7.6) : la boucle tient un seau global, un par file et un
//! par tâche en cours ; chaque exécution reçoit les trois. Changer une limite change le
//! débit du seau, que les connexions en cours suivent aussitôt.
//!
//! Espace disque (F-18) : une tâche arrêtée faute de place (pause `disk_full`) repart
//! d'elle-même quand la place revient ; la boucle le vérifie toutes les
//! [`DISK_RECHECK`].
//!
//! Plages horaires et action de fin (F-16, SPEC §7.11) : voir `planning`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::io::part_path;
use crate::limit::{Bucket, Throttle};
use crate::net::{self, Client, NetError};
use crate::schedule::{EndAction, LocalNow, Rules, Schedule};
use crate::segmenter::Segment;
use crate::store::segments::load_segments;
use crate::store::tasks::{
    self, Change, NewTask, PauseReason, QueueRow, SourceRow, TaskFilter, TaskRow, TaskState,
    change_state, load_task, next_retry_at, runnable_tasks,
};
use crate::store::{self, StoreError, now_ms, schedules, settings};
use crate::transfer::{self, LiveSegment, Options, Outcome, Progress, TransferError};

mod planning;
#[cfg(test)]
mod tests;

/// Nom de la file créée au premier démarrage.
pub const DEFAULT_QUEUE: &str = "default";

/// Intervalle de vérification de la place libre pour les tâches en pause `disk_full`
/// (SPEC §7.10).
pub const DISK_RECHECK: Duration = Duration::from_secs(30);

/// Taille du tampon des événements ; un abonné trop lent perd les plus anciens.
const EVENT_BUFFER: usize = 4096;

/// Réglages du moteur.
#[derive(Debug, Clone)]
pub struct Config {
    /// Base SQLite du moteur, créée si besoin.
    pub db_path: PathBuf,
    pub client: Client,
    /// Réglages des exécutions ; la place libre se mesure par `transfer.free_space`.
    pub transfer: Options,
    /// [`DISK_RECHECK`], sauf dans les tests.
    pub disk_recheck: Duration,
    /// Plages horaires et action de fin (F-16).
    pub planning: Planning,
}

/// Intervalle de contrôle des plages horaires : une file s'ouvre ou se ferme au plus
/// tard ce délai après l'heure prévue.
pub const SCHEDULE_CHECK: Duration = Duration::from_secs(15);

/// Délai entre l'annonce d'une action de fin et son exécution : le temps de l'annuler.
pub const END_ACTION_DELAY: Duration = Duration::from_secs(60);

/// Réglages de la planification (F-16, SPEC §7.11).
#[derive(Debug, Clone)]
pub struct Planning {
    /// Heure de la machine ; [`crate::schedule::local_now`], sauf dans les tests.
    pub clock: fn(&Connection) -> LocalNow,
    /// [`SCHEDULE_CHECK`], sauf dans les tests.
    pub check_every: Duration,
    /// [`END_ACTION_DELAY`], sauf dans les tests.
    pub end_delay: Duration,
    /// Exécute une action de fin. Par défaut, aucune : seul `aurod` branche la mise en
    /// veille et l'extinction réelles (`power::system`).
    pub power: fn(EndAction) -> std::io::Result<()>,
}

impl Default for Planning {
    fn default() -> Self {
        Planning {
            clock: crate::schedule::local_now,
            check_every: SCHEDULE_CHECK,
            end_delay: END_ACTION_DELAY,
            power: crate::power::unplugged,
        }
    }
}

/// Une planification, avec son ouverture à l'heure où elle est lue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleView {
    pub schedule: Schedule,
    /// Ses files peuvent télécharger ; toujours vrai si elle est désactivée.
    pub open: bool,
    /// Prochain changement d'ouverture, en millisecondes Unix.
    pub next_change_at: Option<i64>,
}

/// Action de fin annoncée, qui s'exécutera à `at_ms` sauf annulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEnd {
    pub queue_id: String,
    pub action: EndAction,
    /// Millisecondes Unix.
    pub at_ms: i64,
}

/// Champs modifiés d'une tâche (`task.update`) ; `None` : inchangé.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskUpdate {
    /// `Some(None)` : retire la limite.
    pub speed_limit_bps: Option<Option<u64>>,
}

impl TaskUpdate {
    fn is_empty(&self) -> bool {
        self.speed_limit_bps.is_none()
    }
}

/// Champs modifiés d'une file (`queue.update`) ; `None` : inchangé.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueChange {
    pub name: Option<String>,
    pub max_active: Option<u32>,
    /// `Some(None)` : retire la limite.
    pub speed_limit_bps: Option<Option<u64>>,
    /// `Some(None)` : retire la planification.
    pub schedule_id: Option<Option<String>>,
}

/// Ce qui arrive aux tâches et aux files, pour les interfaces (SPEC §6.5).
///
/// `rev` est la révision du moteur (SPEC §6.10) : elle augmente de 1 à chaque changement
/// (ajout, changement d'état, file modifiée) ; l'avancement porte la révision courante
/// sans l'augmenter. Elle repart de 0 à chaque démarrage du moteur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Added {
        rev: u64,
        task_id: String,
    },
    StateChanged {
        rev: u64,
        task_id: String,
        from: TaskState,
        to: TaskState,
        pause_reason: Option<String>,
        error_code: Option<String>,
    },
    /// Réglage d'une tâche modifié, sans changement d'état.
    TaskUpdated {
        rev: u64,
        task_id: String,
        update: TaskUpdate,
    },
    QueueUpdated {
        rev: u64,
        queue_id: String,
    },
    /// Préparation enregistrée par une exécution (SPEC §7.1) : nom du fichier retenu et
    /// taille annoncée, que ni l'ajout ni le passage à `active` ne connaissent. Annoncée à
    /// chaque exécution, après sa préparation.
    Prepared {
        rev: u64,
        task_id: String,
        file_name: Option<String>,
        total_bytes: Option<u64>,
    },
    /// Tâche retirée (`task.remove`) : elle n'existe plus.
    Removed {
        rev: u64,
        task_id: String,
    },
    /// Limite de débit globale changée (`None` : retirée).
    LimitsChanged {
        rev: u64,
        download_bps: Option<u64>,
    },
    /// Dossier des téléchargements changé : le dossier retenu désormais.
    DownloadDirChanged {
        rev: u64,
        download_dir: Option<String>,
    },
    Progress {
        rev: u64,
        task_id: String,
        progress: Progress,
    },
    /// Planification créée, modifiée, ou dont l'ouverture vient de changer.
    ScheduleChanged {
        rev: u64,
        view: ScheduleView,
    },
    ScheduleRemoved {
        rev: u64,
        schedule_id: String,
    },
    /// Action de fin annoncée (`Some`), puis exécutée ou annulée (`None`) ; `error` si
    /// le système l'a refusée.
    EndAction {
        rev: u64,
        pending: Option<PendingEnd>,
        error: Option<String>,
    },
}

/// Ce que `task.get` demande en plus de la tâche (SPEC §6.4, `include`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Include {
    pub segments: bool,
    pub sources: bool,
}

/// Une tâche avec ses segments et ses sources, si demandés.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDetails {
    pub task: TaskRow,
    /// Segments enregistrés, chacun avec ses octets reçus s'il est en cours de réception.
    pub segments: Option<Vec<(Segment, Option<u64>)>>,
    pub sources: Option<Vec<SourceRow>>,
}

/// État complet du moteur à une révision donnée (`sync.snapshot`, SPEC §6.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub rev: u64,
    pub tasks: Vec<TaskRow>,
    pub schedules: Vec<ScheduleView>,
    pub end_action: Option<PendingEnd>,
    pub queues: Vec<QueueRow>,
    /// Limite de débit globale (`None` : aucune).
    pub download_limit: Option<u64>,
    /// Dossier des ajouts sans `destDir` ; `None` s'il n'y en a aucun.
    pub download_dir: Option<String>,
}

/// Erreurs d'une commande au moteur.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] StoreError),

    #[error(transparent)]
    Url(#[from] NetError),

    #[error("file inconnue : {0}")]
    QueueNotFound(String),

    #[error("planification inconnue : {0}")]
    ScheduleNotFound(String),

    #[error("demande invalide : {0}")]
    BadRequest(String),

    #[error("dossier de destination refusé : {0}")]
    DestDir(String),

    #[error("la tâche {task_id} est dans l'état {state} : {action} impossible")]
    InvalidState {
        task_id: String,
        state: &'static str,
        action: &'static str,
    },

    #[error(
        "le fichier distant a changé depuis le début du téléchargement ; repartir de zéro \
         n'est pas encore possible (F-13)"
    )]
    FileChanged,

    #[error("impossible de supprimer {path} : {source}")]
    DeleteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("le moteur est arrêté")]
    Stopped,
}

/// Tâche à ajouter.
#[derive(Debug, Clone, Default)]
pub struct AddTask {
    pub url: String,
    /// Dossier de destination, en chemin absolu ; `None` : le dossier des téléchargements
    /// du moteur (réglage `downloads.dir`, sinon celui du système).
    pub dest_dir: Option<String>,
    /// File de la tâche ; `None` : la file par défaut.
    pub queue_id: Option<String>,
    /// `None` : nombre de connexions automatique (SPEC §7.3).
    pub max_connections: Option<u32>,
    /// Limite de débit de la tâche, en octets par seconde ; `None` : aucune.
    pub speed_limit_bps: Option<u64>,
    /// Ajoutée en pause (cause `user`), sans partir.
    pub start_paused: bool,
    /// Clé d'idempotence (SPEC §6.1) : un ajout répété avec la même clé rend la tâche déjà
    /// créée, tant qu'elle n'est pas retirée.
    pub idempotency_key: Option<String>,
}

type Reply<T> = oneshot::Sender<Result<T, EngineError>>;

enum Command {
    Add(AddTask, Reply<String>),
    Pause(String, Reply<()>),
    Resume(String, Reply<()>),
    Cancel {
        task_id: String,
        delete_files: bool,
        reply: Reply<()>,
    },
    Remove {
        task_id: String,
        delete_files: bool,
        reply: Reply<()>,
    },
    Get(String, Reply<TaskRow>),
    Details(String, Include, Reply<TaskDetails>),
    List(TaskFilter, Reply<(Vec<TaskRow>, u64)>),
    Snapshot(Reply<Snapshot>),
    Queues(Reply<Vec<QueueRow>>),
    UpdateTask {
        task_id: String,
        update: TaskUpdate,
        reply: Reply<TaskRow>,
    },
    CreateQueue {
        name: String,
        max_active: u32,
        speed_limit_bps: Option<u64>,
        reply: Reply<String>,
    },
    UpdateQueue {
        queue_id: String,
        change: QueueChange,
        reply: Reply<QueueRow>,
    },
    DownloadLimit(Reply<Option<u64>>),
    SetDownloadLimit(Option<u64>, Reply<()>),
    DownloadDir(Reply<Option<String>>),
    SetDownloadDir(Option<String>, Reply<Option<String>>),
    ListSchedules(Reply<Vec<ScheduleView>>),
    SetSchedule {
        schedule_id: Option<String>,
        name: String,
        enabled: bool,
        rules: Rules,
        reply: Reply<ScheduleView>,
    },
    DeleteSchedule(String, Reply<()>),
    CancelEndAction(Reply<bool>),
    Shutdown(oneshot::Sender<()>),
}

/// Poignée du moteur ; se clone librement. Le moteur s'arrête par [`Engine::shutdown`],
/// ou quand la dernière poignée disparaît.
#[derive(Debug, Clone)]
pub struct Engine {
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<Event>,
    default_queue: String,
    rev: Arc<AtomicU64>,
    stopped: watch::Receiver<bool>,
}

impl Engine {
    /// Ouvre la base, crée la file par défaut et lance la boucle du planificateur, qui
    /// reprend aussitôt les tâches interrompues par un arrêt du moteur (SPEC §7.4).
    /// Demande un environnement d'exécution `tokio`.
    pub fn start(config: Config) -> Result<Engine, EngineError> {
        let conn = store::open(&config.db_path)?;
        let default_queue = tasks::ensure_queue(&conn, DEFAULT_QUEUE)?;
        let download_limit = settings::download_limit(&conn)?;
        let (commands, inbox) = mpsc::channel(64);
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let (stopped_tx, stopped) = watch::channel(false);
        let (prepared_tx, prepared) = mpsc::unbounded_channel();
        let rev = Arc::new(AtomicU64::new(0));
        let state = Scheduler::new(
            conn,
            config,
            events.clone(),
            Arc::clone(&rev),
            default_queue.clone(),
            download_limit,
            prepared_tx,
        );
        tokio::spawn(async move {
            state.run(inbox, prepared).await;
            let _ = stopped_tx.send(true);
        });
        Ok(Engine {
            commands,
            events,
            default_queue,
            rev,
            stopped,
        })
    }

    /// Identifiant de la file par défaut.
    pub fn default_queue(&self) -> &str {
        &self.default_queue
    }

    /// Révision courante (SPEC §6.10).
    pub fn rev(&self) -> u64 {
        self.rev.load(Ordering::SeqCst)
    }

    /// Revient quand la boucle du moteur s'est arrêtée (`shutdown`, ou plus de poignée).
    pub async fn stopped(&self) {
        let mut stopped = self.stopped.clone();
        let _ = stopped.wait_for(|s| *s).await;
    }

    /// Une tâche.
    pub async fn get(&self, task_id: &str) -> Result<TaskRow, EngineError> {
        let id = task_id.to_owned();
        self.ask(|reply| Command::Get(id, reply)).await
    }

    /// Une tâche, avec ses segments (et ce que ses connexions en reçoivent en ce moment)
    /// et ses sources si `include` les demande.
    pub async fn details(
        &self,
        task_id: &str,
        include: Include,
    ) -> Result<TaskDetails, EngineError> {
        let id = task_id.to_owned();
        self.ask(|reply| Command::Details(id, include, reply)).await
    }

    /// Tâches selon `filter`, avec la révision à laquelle la liste a été lue.
    pub async fn list(&self, filter: TaskFilter) -> Result<(Vec<TaskRow>, u64), EngineError> {
        self.ask(|reply| Command::List(filter, reply)).await
    }

    /// Tâches et files, lues ensemble à une même révision.
    pub async fn snapshot(&self) -> Result<Snapshot, EngineError> {
        self.ask(Command::Snapshot).await
    }

    /// Files, dans leur ordre ; sans lire les tâches, contrairement à [`Engine::snapshot`].
    pub async fn queues(&self) -> Result<Vec<QueueRow>, EngineError> {
        self.ask(Command::Queues).await
    }

    /// Crée une file ; renvoie son identifiant.
    pub async fn create_queue(
        &self,
        name: &str,
        max_active: u32,
        speed_limit_bps: Option<u64>,
    ) -> Result<String, EngineError> {
        let name = name.to_owned();
        self.ask(|reply| Command::CreateQueue {
            name,
            max_active,
            speed_limit_bps,
            reply,
        })
        .await
    }

    /// Change le nom, le nombre de tâches simultanées ou la limite de débit d'une file ;
    /// une nouvelle limite s'applique aussitôt à ses tâches en cours.
    pub async fn update_queue(
        &self,
        queue_id: &str,
        change: QueueChange,
    ) -> Result<QueueRow, EngineError> {
        let queue_id = queue_id.to_owned();
        self.ask(|reply| Command::UpdateQueue {
            queue_id,
            change,
            reply,
        })
        .await
    }

    /// Change les réglages d'une tâche ; une nouvelle limite de débit s'applique aussitôt
    /// si elle tourne. Renvoie la tâche modifiée.
    pub async fn update_task(
        &self,
        task_id: &str,
        update: TaskUpdate,
    ) -> Result<TaskRow, EngineError> {
        let task_id = task_id.to_owned();
        self.ask(|reply| Command::UpdateTask {
            task_id,
            update,
            reply,
        })
        .await
    }

    /// Limite de débit globale, en octets par seconde ; `None` : aucune.
    pub async fn download_limit(&self) -> Result<Option<u64>, EngineError> {
        self.ask(Command::DownloadLimit).await
    }

    /// Change la limite de débit globale ; enregistrée, elle vaut aussi aux démarrages
    /// suivants, et s'applique aussitôt aux tâches en cours.
    pub async fn set_download_limit(&self, limit: Option<u64>) -> Result<(), EngineError> {
        self.ask(|reply| Command::SetDownloadLimit(limit, reply))
            .await
    }

    /// Dossier des ajouts sans dossier : le réglage, sinon le dossier « Téléchargements »
    /// du système ; `None` s'il n'y en a aucun.
    pub async fn download_dir(&self) -> Result<Option<String>, EngineError> {
        self.ask(Command::DownloadDir).await
    }

    /// Change le dossier des téléchargements (un dossier existant, en chemin absolu) ;
    /// `None` revient à celui du système. Renvoie le dossier retenu désormais.
    pub async fn set_download_dir(
        &self,
        dir: Option<String>,
    ) -> Result<Option<String>, EngineError> {
        self.ask(|reply| Command::SetDownloadDir(dir, reply)).await
    }

    /// Abonnement aux événements à partir de maintenant.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    async fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Command) -> Result<T, EngineError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(make(reply))
            .await
            .map_err(|_| EngineError::Stopped)?;
        answer.await.map_err(|_| EngineError::Stopped)?
    }

    /// Ajoute une tâche `queued` (ou `paused`) ; renvoie son identifiant.
    pub async fn add(&self, task: AddTask) -> Result<String, EngineError> {
        self.ask(|reply| Command::Add(task, reply)).await
    }

    /// Met une tâche en pause (cause `user`). Une tâche en cours s'arrête après son
    /// point de contrôle ; son changement d'état est annoncé quand il a lieu.
    pub async fn pause(&self, task_id: &str) -> Result<(), EngineError> {
        let id = task_id.to_owned();
        self.ask(|reply| Command::Pause(id, reply)).await
    }

    /// Remet en file une tâche en pause ou en échec.
    pub async fn resume(&self, task_id: &str) -> Result<(), EngineError> {
        let id = task_id.to_owned();
        self.ask(|reply| Command::Resume(id, reply)).await
    }

    /// Annule une tâche ; avec `delete_files`, supprime aussi son `.part`.
    pub async fn cancel(&self, task_id: &str, delete_files: bool) -> Result<(), EngineError> {
        let task_id = task_id.to_owned();
        self.ask(|reply| Command::Cancel {
            task_id,
            delete_files,
            reply,
        })
        .await
    }

    /// Retire une tâche finie (terminée, en échec ou annulée) ; avec `delete_files`,
    /// supprime d'abord son fichier : le fichier final d'une tâche terminée, le `.part`
    /// des autres.
    pub async fn remove(&self, task_id: &str, delete_files: bool) -> Result<(), EngineError> {
        let task_id = task_id.to_owned();
        self.ask(|reply| Command::Remove {
            task_id,
            delete_files,
            reply,
        })
        .await
    }

    /// Planifications, avec leur ouverture à cet instant.
    pub async fn schedules(&self) -> Result<Vec<ScheduleView>, EngineError> {
        self.ask(Command::ListSchedules).await
    }

    /// Crée une planification (`schedule_id` absent) ou la remplace.
    pub async fn set_schedule(
        &self,
        schedule_id: Option<String>,
        name: String,
        enabled: bool,
        rules: Rules,
    ) -> Result<ScheduleView, EngineError> {
        self.ask(|reply| Command::SetSchedule {
            schedule_id,
            name,
            enabled,
            rules,
            reply,
        })
        .await
    }

    /// Supprime une planification ; ses files téléchargent de nouveau à toute heure.
    pub async fn delete_schedule(&self, schedule_id: &str) -> Result<(), EngineError> {
        let id = schedule_id.to_owned();
        self.ask(|reply| Command::DeleteSchedule(id, reply)).await
    }

    /// Annule l'action de fin annoncée ; `false` s'il n'y en avait pas.
    pub async fn cancel_end_action(&self) -> Result<bool, EngineError> {
        self.ask(Command::CancelEndAction).await
    }

    /// Arrête le moteur : les tâches en cours s'arrêtent après leur point de contrôle et
    /// repartiront au prochain démarrage. Revient quand tout est enregistré.
    pub async fn shutdown(&self) {
        let (done, finished) = oneshot::channel();
        if self.commands.send(Command::Shutdown(done)).await.is_ok() {
            let _ = finished.await;
        }
    }
}

/// Ce qu'il faut faire d'une tâche quand son exécution s'arrête.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    /// Rien : l'issue enregistrée par `transfer::run` reste.
    Keep,
    /// La remettre en file : reprise demandée pendant l'arrêt, ou arrêt du moteur.
    Requeue,
    /// L'annuler, avec ou sans son `.part`.
    Cancel { delete_files: bool },
}

struct Running {
    queue_id: String,
    stop: watch::Sender<bool>,
    then: Then,
    /// Segments en cours de réception, publiés par l'exécution à chaque avancement.
    live: watch::Receiver<Arc<[LiveSegment]>>,
}

struct Scheduler {
    conn: Connection,
    config: Config,
    events: broadcast::Sender<Event>,
    /// Révision, que seule cette boucle augmente ; partagée pour la lecture.
    rev: Arc<AtomicU64>,
    default_queue: String,
    running: HashMap<String, Running>,
    runs: JoinSet<(String, Result<Outcome, TransferError>)>,
    /// Tâche de chaque exécution, pour retrouver celle qui a paniqué.
    run_ids: HashMap<tokio::task::Id, String>,
    /// Tâches dont l'exécution a échoué sans issue enregistrable : jamais relancées
    /// avant le prochain démarrage, pour ne pas boucler.
    broken: HashSet<String>,
    /// Arrêt demandé : à prévenir quand la dernière exécution est finie.
    shutdown: Option<oneshot::Sender<()>>,
    limits: Limits,
    /// Prochaine vérification de la place libre pour les tâches en pause `disk_full`.
    next_disk_check: Instant,
    /// Les exécutions y signalent leur préparation enregistrée ; la boucle l'annonce
    /// (`Event::Prepared`), car elle seule augmente la révision.
    prepared: mpsc::UnboundedSender<String>,
    /// Plages horaires et action de fin (F-16).
    planning: planning::State,
}

/// Seaux de débit (SPEC §7.6) : global, par file, et par tâche en cours.
struct Limits {
    global: Arc<Bucket>,
    queues: HashMap<String, Arc<Bucket>>,
    tasks: HashMap<String, Arc<Bucket>>,
}

impl Scheduler {
    fn new(
        conn: Connection,
        config: Config,
        events: broadcast::Sender<Event>,
        rev: Arc<AtomicU64>,
        default_queue: String,
        download_limit: Option<u64>,
        prepared: mpsc::UnboundedSender<String>,
    ) -> Self {
        Scheduler {
            conn,
            config,
            events,
            rev,
            default_queue,
            running: HashMap::new(),
            runs: JoinSet::new(),
            run_ids: HashMap::new(),
            broken: HashSet::new(),
            shutdown: None,
            limits: Limits {
                global: Bucket::new(download_limit),
                queues: HashMap::new(),
                tasks: HashMap::new(),
            },
            next_disk_check: Instant::now(),
            prepared,
            planning: planning::State::default(),
        }
    }

    async fn run(
        mut self,
        mut inbox: mpsc::Receiver<Command>,
        mut prepared: mpsc::UnboundedReceiver<String>,
    ) {
        loop {
            if self.shutdown.is_none() {
                self.recheck_disk();
                self.refresh_schedules(false);
                self.check_end_action();
                self.start_runnable();
            } else if self.running.is_empty() {
                break;
            }
            let wake = self.next_wake();
            tokio::select! {
                command = inbox.recv(), if self.shutdown.is_none() => match command {
                    Some(command) => self.handle(command),
                    // Plus aucune poignée : arrêt, comme `shutdown`.
                    None => self.begin_shutdown(None),
                },
                Some(done) = self.runs.join_next_with_id(), if !self.runs.is_empty() => {
                    match done {
                        Ok((id, (task_id, result))) => {
                            self.run_ids.remove(&id);
                            self.finished(&task_id, result);
                        }
                        // Une exécution a paniqué : sa tâche ne repart pas d'elle-même.
                        Err(e) => self.forget_panicked(e),
                    }
                }
                Some(task_id) = prepared.recv() => self.announce_prepared(&task_id),
                _ = tokio::time::sleep(wake), if self.shutdown.is_none() => {}
            }
        }
        if let Some(done) = self.shutdown.take() {
            let _ = done.send(());
        }
    }

    /// Délai jusqu'à la prochaine nouvelle tentative, vérification de la place libre,
    /// contrôle des plages horaires ou action de fin, au plus une minute.
    fn next_wake(&self) -> Duration {
        let now = Instant::now();
        let cap = Duration::from_secs(60)
            .min(self.next_disk_check.saturating_duration_since(now))
            .min(self.planning_wake(now));
        match next_retry_at(&self.conn) {
            Ok(Some(at)) => {
                Duration::from_millis(u64::try_from(at.saturating_sub(now_ms())).unwrap_or(0))
                    .min(cap)
            }
            _ => cap,
        }
    }

    /// Remet en file les tâches arrêtées faute de place quand la place revient (F-18) :
    /// celle qu'il leur reste à prendre, plus la réserve. Au plus une fois par
    /// `disk_recheck`.
    fn recheck_disk(&mut self) {
        let now = Instant::now();
        if now < self.next_disk_check {
            return;
        }
        self.next_disk_check = now + self.config.disk_recheck;
        let filter = TaskFilter {
            states: vec![TaskState::Paused],
            ..TaskFilter::default()
        };
        let Ok(paused) = tasks::find_tasks(&self.conn, &filter) else {
            return;
        };
        for task in paused {
            if task.pause_reason.as_deref() == Some(PauseReason::DiskFull.as_sql())
                && self.config.transfer.space_for(&task)
            {
                let _ = self.set_state(&task.id, &Change::to(TaskState::Queued));
            }
        }
    }

    /// Annonce un changement, avec la révision suivante.
    fn emit(&self, event: impl FnOnce(u64) -> Event) {
        let rev = self.rev.fetch_add(1, Ordering::SeqCst) + 1;
        // Personne à l'écoute : rien à faire.
        let _ = self.events.send(event(rev));
    }

    /// Annonce le nom et la taille qu'une exécution vient d'enregistrer. Peut arriver après
    /// la fin de l'exécution : l'annonce reste due, le nom servant à ouvrir le fichier.
    fn announce_prepared(&self, task_id: &str) {
        let Ok(task) = load_task(&self.conn, task_id) else {
            return;
        };
        self.emit(|rev| Event::Prepared {
            rev,
            task_id: task.id,
            file_name: task.file_name,
            total_bytes: task.total_bytes,
        });
    }

    /// Annonce un changement d'état, avec la cause de pause ou le code d'erreur
    /// enregistrés.
    fn changed(&self, task_id: &str, from: TaskState, to: TaskState) {
        if from == to {
            return;
        }
        let (pause_reason, error_code) = match load_task(&self.conn, task_id) {
            Ok(t) => (t.pause_reason, t.error_code),
            Err(_) => (None, None),
        };
        self.emit(|rev| Event::StateChanged {
            rev,
            task_id: task_id.to_owned(),
            from,
            to,
            pause_reason,
            error_code,
        });
    }

    /// Démarre les tâches prêtes, tant que leur file a de la place (F-14).
    fn start_runnable(&mut self) {
        let candidates = match runnable_tasks(&self.conn, now_ms()) {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut busy: HashMap<String, u32> = HashMap::new();
        for r in self.running.values() {
            *busy.entry(r.queue_id.clone()).or_default() += 1;
        }
        for c in candidates {
            if self.running.contains_key(&c.task_id) || self.broken.contains(&c.task_id) {
                continue;
            }
            // Hors de ses plages horaires, une file attend (F-16). Une tâche restée
            // `active` (arrêt brutal du moteur) y retourne en file, comme à la fermeture.
            if self.planning.closed.contains(&c.queue_id) {
                if c.state == TaskState::Active {
                    let paused = Change {
                        pause_reason: Some(PauseReason::Schedule),
                        ..Change::to(TaskState::Paused)
                    };
                    let requeued = self
                        .set_state(&c.task_id, &paused)
                        .and_then(|()| self.set_state(&c.task_id, &Change::to(TaskState::Queued)));
                    if requeued.is_err() {
                        self.broken.insert(c.task_id);
                    }
                }
                continue;
            }
            let used = busy.entry(c.queue_id.clone()).or_default();
            if *used >= c.max_active {
                continue;
            }
            *used += 1;
            self.launch(c.task_id, c.queue_id, c.state);
        }
    }

    fn launch(&mut self, task_id: String, queue_id: String, from: TaskState) {
        // L'état `active` est enregistré ici, avant d'être annoncé : un instantané pris à
        // la révision de l'annonce le montre déjà (SPEC §6.10). `transfer::run` accepte
        // une tâche `active`.
        if from != TaskState::Active
            && change_state(&mut self.conn, &task_id, &Change::to(TaskState::Active)).is_err()
        {
            self.broken.insert(task_id);
            return;
        }
        let throttle = self.throttle_for(&task_id, &queue_id);
        let (stop, stopped) = watch::channel(false);
        let (live_tx, live) = watch::channel(Arc::from([]));
        self.running.insert(
            task_id.clone(),
            Running {
                queue_id,
                stop,
                then: Then::Keep,
                live,
            },
        );
        self.changed(&task_id, from, TaskState::Active);
        let mut config = self.config.clone();
        config.transfer.throttle = throttle;
        let events = self.events.clone();
        let rev = Arc::clone(&self.rev);
        let prepared = self.prepared.clone();
        let id = task_id.clone();
        let handle = self.runs.spawn(async move {
            let result = execute(&config, &id, stopped, events, rev, prepared, live_tx).await;
            (id, result)
        });
        self.run_ids.insert(handle.id(), task_id);
    }

    /// Seaux d'une exécution : global, de sa file, et le sien, gardé pour qu'une nouvelle
    /// limite de la tâche s'applique en cours de route.
    fn throttle_for(&mut self, task_id: &str, queue_id: &str) -> Throttle {
        let task_limit = load_task(&self.conn, task_id)
            .ok()
            .and_then(|t| t.speed_limit_bps);
        let task = Bucket::new(task_limit);
        self.limits
            .tasks
            .insert(task_id.to_owned(), Arc::clone(&task));
        Throttle::new([
            Arc::clone(&self.limits.global),
            self.queue_bucket(queue_id),
            task,
        ])
    }

    /// Seau d'une file, créé à sa première tâche avec la limite enregistrée.
    fn queue_bucket(&mut self, queue_id: &str) -> Arc<Bucket> {
        if let Some(bucket) = self.limits.queues.get(queue_id) {
            return Arc::clone(bucket);
        }
        let limit = tasks::load_queue(&self.conn, queue_id)
            .ok()
            .flatten()
            .and_then(|q| q.speed_limit_bps);
        let bucket = Bucket::new(limit);
        self.limits
            .queues
            .insert(queue_id.to_owned(), Arc::clone(&bucket));
        bucket
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Add(task, reply) => {
                let _ = reply.send(self.add(task));
            }
            Command::Pause(task_id, reply) => {
                let _ = reply.send(self.pause(&task_id));
            }
            Command::Resume(task_id, reply) => {
                let _ = reply.send(self.resume(&task_id));
            }
            Command::Cancel {
                task_id,
                delete_files,
                reply,
            } => {
                let _ = reply.send(self.cancel(&task_id, delete_files));
            }
            Command::Remove {
                task_id,
                delete_files,
                reply,
            } => {
                let _ = reply.send(self.remove(&task_id, delete_files));
            }
            Command::Get(task_id, reply) => {
                let _ = reply.send(load_task(&self.conn, &task_id).map_err(Into::into));
            }
            Command::Details(task_id, include, reply) => {
                let _ = reply.send(self.details(&task_id, include));
            }
            Command::ListSchedules(reply) => {
                let _ = reply.send(self.schedule_views());
            }
            Command::SetSchedule {
                schedule_id,
                name,
                enabled,
                rules,
                reply,
            } => {
                let _ = reply.send(self.set_schedule(schedule_id, &name, enabled, rules));
            }
            Command::DeleteSchedule(schedule_id, reply) => {
                let _ = reply.send(self.delete_schedule(&schedule_id));
            }
            Command::CancelEndAction(reply) => {
                let _ = reply.send(Ok(self.cancel_end_action()));
            }
            Command::List(filter, reply) => {
                let rows = tasks::find_tasks(&self.conn, &filter);
                let _ = reply.send(rows.map(|r| (r, self.current_rev())).map_err(Into::into));
            }
            Command::Snapshot(reply) => {
                let _ = reply.send(self.snapshot());
            }
            Command::Queues(reply) => {
                let _ = reply.send(tasks::list_queues(&self.conn).map_err(Into::into));
            }
            Command::UpdateTask {
                task_id,
                update,
                reply,
            } => {
                let _ = reply.send(self.update_task(&task_id, update));
            }
            Command::CreateQueue {
                name,
                max_active,
                speed_limit_bps,
                reply,
            } => {
                let _ = reply.send(self.create_queue(&name, max_active, speed_limit_bps));
            }
            Command::UpdateQueue {
                queue_id,
                change,
                reply,
            } => {
                let _ = reply.send(self.update_queue(&queue_id, change));
            }
            Command::DownloadLimit(reply) => {
                let _ = reply.send(settings::download_limit(&self.conn).map_err(Into::into));
            }
            Command::SetDownloadLimit(limit, reply) => {
                let _ = reply.send(self.set_download_limit(limit));
            }
            Command::DownloadDir(reply) => {
                let _ = reply.send(self.download_dir());
            }
            Command::SetDownloadDir(dir, reply) => {
                let _ = reply.send(self.set_download_dir(dir));
            }
            Command::Shutdown(done) => self.begin_shutdown(Some(done)),
        }
    }

    fn current_rev(&self) -> u64 {
        self.rev.load(Ordering::SeqCst)
    }

    /// Lu dans la boucle, entre deux changements : tâches, files et révision concordent.
    fn snapshot(&self) -> Result<Snapshot, EngineError> {
        Ok(Snapshot {
            rev: self.current_rev(),
            tasks: tasks::find_tasks(&self.conn, &TaskFilter::default())?,
            schedules: self.schedule_views()?,
            end_action: self.planning.pending.clone(),
            queues: tasks::list_queues(&self.conn)?,
            download_limit: settings::download_limit(&self.conn)?,
            download_dir: self.download_dir()?,
        })
    }

    /// Dossier des ajouts sans dossier : le réglage, sinon le dossier « Téléchargements »
    /// du système.
    fn download_dir(&self) -> Result<Option<String>, EngineError> {
        Ok(settings::download_dir(&self.conn)?
            .or_else(|| dirs::download_dir().and_then(|d| d.to_str().map(str::to_owned))))
    }

    fn set_download_dir(&mut self, dir: Option<String>) -> Result<Option<String>, EngineError> {
        if let Some(dir) = &dir {
            check_dest_dir(dir)?;
        }
        settings::set_download_dir(&self.conn, dir.as_deref())?;
        let effective = self.download_dir()?;
        let announced = effective.clone();
        self.emit(|rev| Event::DownloadDirChanged {
            rev,
            download_dir: announced,
        });
        Ok(effective)
    }

    fn check_queue_limit(max_active: u32) -> Result<(), EngineError> {
        if max_active == 0 {
            return Err(EngineError::BadRequest(
                "une file accepte au moins une tâche à la fois".into(),
            ));
        }
        Ok(())
    }

    fn check_speed_limit(limit: Option<u64>) -> Result<(), EngineError> {
        if limit == Some(0) {
            return Err(EngineError::BadRequest(
                "une limite de débit est d'au moins un octet par seconde".into(),
            ));
        }
        Ok(())
    }

    fn set_download_limit(&mut self, limit: Option<u64>) -> Result<(), EngineError> {
        Self::check_speed_limit(limit)?;
        settings::set_download_limit(&self.conn, limit)?;
        self.limits.global.set_rate(limit);
        self.emit(|rev| Event::LimitsChanged {
            rev,
            download_bps: limit,
        });
        Ok(())
    }

    fn update_task(&mut self, task_id: &str, update: TaskUpdate) -> Result<TaskRow, EngineError> {
        if let Some(limit) = update.speed_limit_bps {
            Self::check_speed_limit(limit)?;
            tasks::set_task_speed_limit(&self.conn, task_id, limit)?;
            if let Some(bucket) = self.limits.tasks.get(task_id) {
                bucket.set_rate(limit);
            }
        }
        let task = load_task(&self.conn, task_id)?;
        if !update.is_empty() {
            let id = task_id.to_owned();
            self.emit(|rev| Event::TaskUpdated {
                rev,
                task_id: id,
                update,
            });
        }
        Ok(task)
    }

    fn create_queue(
        &mut self,
        name: &str,
        max_active: u32,
        speed_limit_bps: Option<u64>,
    ) -> Result<String, EngineError> {
        Self::check_queue_limit(max_active)?;
        Self::check_speed_limit(speed_limit_bps)?;
        if name.trim().is_empty() {
            return Err(EngineError::BadRequest("nom de file vide".into()));
        }
        if tasks::list_queues(&self.conn)?
            .iter()
            .any(|q| q.name == name)
        {
            return Err(EngineError::BadRequest(format!(
                "la file {name} existe déjà"
            )));
        }
        let queue_id = tasks::create_queue(&self.conn, name, max_active, speed_limit_bps)?;
        let id = queue_id.clone();
        self.emit(|rev| Event::QueueUpdated { rev, queue_id: id });
        Ok(queue_id)
    }

    fn update_queue(
        &mut self,
        queue_id: &str,
        change: QueueChange,
    ) -> Result<QueueRow, EngineError> {
        if let Some(limit) = change.max_active {
            Self::check_queue_limit(limit)?;
        }
        if let Some(limit) = change.speed_limit_bps {
            Self::check_speed_limit(limit)?;
        }
        if let Some(name) = change.name.as_deref() {
            let taken = tasks::list_queues(&self.conn)?
                .iter()
                .any(|q| q.name == name && q.id != queue_id);
            if name.trim().is_empty() || taken {
                return Err(EngineError::BadRequest(format!(
                    "nom de file refusé : {name}"
                )));
            }
        }
        // Tout est vérifié avant d'écrire : une demande refusée ne change rien.
        if let Some(Some(id)) = &change.schedule_id
            && schedules::load_schedule(&self.conn, id)?.is_none()
        {
            return Err(EngineError::ScheduleNotFound(id.clone()));
        }
        let updated = tasks::update_queue(
            &self.conn,
            queue_id,
            change.name.as_deref(),
            change.max_active,
            change.speed_limit_bps,
        )?;
        if !updated {
            return Err(EngineError::QueueNotFound(queue_id.to_owned()));
        }
        if let Some(schedule_id) = &change.schedule_id {
            self.attach_schedule(queue_id, schedule_id.as_deref())?;
        }
        if let (Some(limit), Some(bucket)) =
            (change.speed_limit_bps, self.limits.queues.get(queue_id))
        {
            bucket.set_rate(limit);
        }
        let id = queue_id.to_owned();
        self.emit(|rev| Event::QueueUpdated { rev, queue_id: id });
        // Une limite relevée laisse partir des tâches au prochain tour de boucle.
        tasks::load_queue(&self.conn, queue_id)?
            .ok_or_else(|| EngineError::QueueNotFound(queue_id.to_owned()))
    }

    fn add(&mut self, task: AddTask) -> Result<String, EngineError> {
        // Déjà ajoutée avec cette clé : la même tâche, rien de nouveau à annoncer.
        if let Some(key) = &task.idempotency_key
            && let Some(existing) = tasks::task_by_idempotency_key(&self.conn, key)?
        {
            return Ok(existing);
        }
        net::check_url(&task.url)?;
        Self::check_speed_limit(task.speed_limit_bps)?;
        let dest_dir = match task.dest_dir {
            Some(dir) => dir,
            None => self.download_dir()?.ok_or_else(|| {
                EngineError::DestDir(
                    "aucun dossier indiqué, et ni réglage ni dossier « Téléchargements » sur \
                     cette machine"
                        .into(),
                )
            })?,
        };
        check_dest_dir(&dest_dir)?;
        let queue_id = task.queue_id.unwrap_or_else(|| self.default_queue.clone());
        if !tasks::queue_exists(&self.conn, &queue_id)? {
            return Err(EngineError::QueueNotFound(queue_id));
        }
        let display_name = transfer::display_name_for(&task.url);
        let (task_id, _) = tasks::create_task(
            &mut self.conn,
            &NewTask {
                queue_id: &queue_id,
                url: &task.url,
                display_name: &display_name,
                dest_dir: &dest_dir,
                max_connections: task.max_connections,
                speed_limit_bps: task.speed_limit_bps,
                idempotency_key: task.idempotency_key.as_deref(),
            },
        )?;
        let id = task_id.clone();
        self.emit(|rev| Event::Added { rev, task_id: id });
        if task.start_paused {
            self.set_state(&task_id, &user_pause())?;
        }
        Ok(task_id)
    }

    /// Change l'état d'une tâche qui ne tourne pas, et l'annonce.
    fn set_state(&mut self, task_id: &str, change: &Change<'_>) -> Result<(), EngineError> {
        let from = change_state(&mut self.conn, task_id, change)?;
        self.changed(task_id, from, change.to);
        Ok(())
    }

    fn invalid(task: &TaskRow, action: &'static str) -> EngineError {
        EngineError::InvalidState {
            task_id: task.id.clone(),
            state: task.state.as_sql(),
            action,
        }
    }

    fn pause(&mut self, task_id: &str) -> Result<(), EngineError> {
        if let Some(run) = self.running.get_mut(task_id) {
            run.then = Then::Keep;
            let _ = run.stop.send(true);
            return Ok(());
        }
        let task = load_task(&self.conn, task_id)?;
        match task.state {
            TaskState::Queued | TaskState::RetryWait => self.set_state(task_id, &user_pause()),
            TaskState::Paused => Ok(()),
            _ => Err(Self::invalid(&task, "mise en pause")),
        }
    }

    fn resume(&mut self, task_id: &str) -> Result<(), EngineError> {
        if let Some(run) = self.running.get_mut(task_id) {
            // En train de s'arrêter après une pause : elle repartira.
            if run.then == Then::Keep && *run.stop.borrow() {
                run.then = Then::Requeue;
            }
            return Ok(());
        }
        let task = load_task(&self.conn, task_id)?;
        match task.state {
            TaskState::Paused
                if task.pause_reason.as_deref() == Some(PauseReason::FileChanged.as_sql()) =>
            {
                Err(EngineError::FileChanged)
            }
            TaskState::Paused | TaskState::Failed => {
                self.broken.remove(task_id);
                self.set_state(task_id, &Change::to(TaskState::Queued))
            }
            TaskState::Queued | TaskState::Active | TaskState::RetryWait => Ok(()),
            _ => Err(Self::invalid(&task, "reprise")),
        }
    }

    fn cancel(&mut self, task_id: &str, delete_files: bool) -> Result<(), EngineError> {
        if let Some(run) = self.running.get_mut(task_id) {
            run.then = Then::Cancel { delete_files };
            let _ = run.stop.send(true);
            return Ok(());
        }
        let task = load_task(&self.conn, task_id)?;
        match task.state {
            TaskState::Queued | TaskState::Paused | TaskState::RetryWait => {
                self.set_state(task_id, &Change::to(TaskState::Cancelled))?;
                if delete_files {
                    delete_part(&task)?;
                }
                Ok(())
            }
            TaskState::Cancelled => Ok(()),
            _ => Err(Self::invalid(&task, "annulation")),
        }
    }

    /// Tâche, segments et sources (`task.get`, SPEC §6.4). Les octets reçus d'un segment en
    /// cours viennent de son exécution ; ceux de la base ne suivent que les points de
    /// contrôle.
    fn details(&self, task_id: &str, include: Include) -> Result<TaskDetails, EngineError> {
        let task = load_task(&self.conn, task_id)?;
        let segments = if include.segments {
            let live = self
                .running
                .get(task_id)
                .map(|r| Arc::clone(&r.live.borrow()))
                .unwrap_or_else(|| Arc::from([]));
            let received = |idx: u32| live.iter().find(|l| l.idx == idx).map(|l| l.received);
            Some(
                load_segments(&self.conn, task_id)?
                    .into_iter()
                    .map(|s| {
                        let now = received(s.idx);
                        (s, now)
                    })
                    .collect(),
            )
        } else {
            None
        };
        let sources = if include.sources {
            Some(tasks::load_sources(&self.conn, task_id)?)
        } else {
            None
        };
        Ok(TaskDetails {
            task,
            segments,
            sources,
        })
    }

    /// Retire une tâche finie. Le fichier demandé est supprimé d'abord : si cela échoue, la
    /// tâche reste, et l'utilisateur peut réessayer.
    fn remove(&mut self, task_id: &str, delete_files: bool) -> Result<(), EngineError> {
        let task = load_task(&self.conn, task_id)?;
        let finished = matches!(
            task.state,
            TaskState::Completed | TaskState::Failed | TaskState::Cancelled
        );
        if !finished || self.running.contains_key(task_id) {
            return Err(Self::invalid(&task, "retrait"));
        }
        if delete_files {
            match task.state {
                TaskState::Completed => delete_final(&task)?,
                _ => delete_part(&task)?,
            }
        }
        tasks::remove_task(&mut self.conn, task_id)?;
        self.broken.remove(task_id);
        let id = task_id.to_owned();
        self.emit(|rev| Event::Removed { rev, task_id: id });
        Ok(())
    }

    fn begin_shutdown(&mut self, done: Option<oneshot::Sender<()>>) {
        for run in self.running.values_mut() {
            if run.then == Then::Keep && !*run.stop.borrow() {
                run.then = Then::Requeue;
            }
            let _ = run.stop.send(true);
        }
        // Une seconde demande d'arrêt attend la même fin que la première.
        self.shutdown = Some(match (self.shutdown.take(), done) {
            (_, Some(done)) => done,
            (Some(previous), None) => previous,
            (None, None) => oneshot::channel().0,
        });
    }

    /// Fin d'une exécution : annonce son issue, puis applique ce qui a été demandé
    /// pendant qu'elle tournait.
    fn finished(&mut self, task_id: &str, result: Result<Outcome, TransferError>) {
        let run = self.running.remove(task_id);
        if let (Some(run), Ok(Outcome::Completed(_))) = (&run, &result) {
            self.arm_end_action(&run.queue_id);
        }
        let then = run.map_or(Then::Keep, |r| r.then);
        self.limits.tasks.remove(task_id);
        if let Err(e) = &result {
            self.record_internal_error(task_id, e);
        }
        let Ok(task) = load_task(&self.conn, task_id) else {
            return;
        };
        self.changed(task_id, TaskState::Active, task.state);
        let stopped_here = matches!(result, Ok(Outcome::Paused));
        let _ = match then {
            Then::Keep => Ok(()),
            Then::Requeue if stopped_here => {
                self.set_state(task_id, &Change::to(TaskState::Queued))
            }
            Then::Requeue => Ok(()),
            Then::Cancel { delete_files } => match task.state {
                TaskState::Paused | TaskState::RetryWait => self
                    .set_state(task_id, &Change::to(TaskState::Cancelled))
                    .and_then(|()| match delete_files {
                        true => delete_part(&task),
                        false => Ok(()),
                    }),
                _ => Ok(()),
            },
        };
    }

    /// Une exécution s'est arrêtée sur une erreur du moteur (base illisible, incohérence)
    /// sans enregistrer d'issue : la tâche passe en `failed`, code `INTERNAL`, et ne repart
    /// pas d'elle-même. Si même cela échoue, elle est écartée jusqu'au prochain démarrage.
    fn record_internal_error(&mut self, task_id: &str, error: &TransferError) {
        let detail = error.to_string();
        let change = Change {
            error: Some(("INTERNAL", &detail)),
            ..Change::to(TaskState::Failed)
        };
        if change_state(&mut self.conn, task_id, &change).is_err() {
            self.broken.insert(task_id.to_owned());
        }
    }

    /// Une exécution a paniqué : sa tâche est enregistrée en échec `INTERNAL`, comme pour
    /// une erreur du moteur.
    fn forget_panicked(&mut self, error: tokio::task::JoinError) {
        let Some(task_id) = self.run_ids.remove(&error.id()) else {
            return;
        };
        self.running.remove(&task_id);
        self.limits.tasks.remove(&task_id);
        self.record_internal_error(&task_id, &TransferError::Join(error));
        if let Ok(task) = load_task(&self.conn, &task_id) {
            self.changed(&task_id, TaskState::Active, task.state);
        }
    }
}

/// Un dossier de destination : chemin absolu d'un dossier existant.
fn check_dest_dir(dir: &str) -> Result<(), EngineError> {
    let path = Path::new(dir);
    if !path.is_absolute() || !path.is_dir() {
        return Err(EngineError::DestDir(format!(
            "{dir} : un dossier existant, en chemin absolu, est attendu"
        )));
    }
    Ok(())
}

fn user_pause() -> Change<'static> {
    Change {
        pause_reason: Some(PauseReason::User),
        ..Change::to(TaskState::Paused)
    }
}

/// Supprime le `.part` d'une tâche annulée, s'il existe.
fn delete_part(task: &TaskRow) -> Result<(), EngineError> {
    let Some(name) = &task.file_name else {
        return Ok(());
    };
    delete_file(part_path(&Path::new(&task.dest_dir).join(name)))
}

/// Supprime le fichier final d'une tâche terminée, s'il existe encore.
fn delete_final(task: &TaskRow) -> Result<(), EngineError> {
    let Some(name) = &task.file_name else {
        return Ok(());
    };
    delete_file(Path::new(&task.dest_dir).join(name))
}

fn delete_file(path: PathBuf) -> Result<(), EngineError> {
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(EngineError::DeleteFile { path, source }),
    }
}

/// Une exécution, avec sa propre connexion à la base.
async fn execute(
    config: &Config,
    task_id: &str,
    stop: watch::Receiver<bool>,
    events: broadcast::Sender<Event>,
    rev: Arc<AtomicU64>,
    prepared: mpsc::UnboundedSender<String>,
    live: watch::Sender<Arc<[LiveSegment]>>,
) -> Result<Outcome, TransferError> {
    let mut conn = store::open(&config.db_path)?;
    // `transfer::run` enregistre la préparation avant le premier avancement : celui-ci
    // la signale à la boucle.
    let mut first = true;
    let mut on_progress = |progress: Progress| {
        if std::mem::take(&mut first) {
            let _ = prepared.send(task_id.to_owned());
        }
        live.send_replace(Arc::clone(&progress.segments));
        let _ = events.send(Event::Progress {
            rev: rev.load(Ordering::SeqCst),
            task_id: task_id.to_owned(),
            progress,
        });
    };
    transfer::run(
        &mut conn,
        &config.client,
        task_id,
        &config.transfer,
        stop,
        &mut on_progress,
    )
    .await
}
