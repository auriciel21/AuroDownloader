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
//! Pas encore traités : limites de débit (F-15), espace disque (F-18), plages horaires
//! (F-16), révision `rev` et protocole JSON-RPC (module `rpc`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinSet;

use crate::io::part_path;
use crate::net::{self, Client, NetError};
use crate::store::tasks::{
    self, Change, NewTask, PauseReason, TaskRow, TaskState, change_state, load_task, next_retry_at,
    runnable_tasks,
};
use crate::store::{self, StoreError, now_ms};
use crate::transfer::{self, Options, Outcome, Progress, TransferError};

#[cfg(test)]
mod tests;

/// Nom de la file créée au premier démarrage.
pub const DEFAULT_QUEUE: &str = "default";

/// Taille du tampon des événements ; un abonné trop lent perd les plus anciens.
const EVENT_BUFFER: usize = 4096;

/// Réglages du moteur.
#[derive(Debug, Clone)]
pub struct Config {
    /// Base SQLite du moteur, créée si besoin.
    pub db_path: PathBuf,
    pub client: Client,
    pub transfer: Options,
}

/// Ce qui arrive aux tâches, pour les interfaces (SPEC §6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Added {
        task_id: String,
    },
    StateChanged {
        task_id: String,
        from: TaskState,
        to: TaskState,
    },
    Progress {
        task_id: String,
        progress: Progress,
    },
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
    pub dest_dir: String,
    /// File de la tâche ; `None` : la file par défaut.
    pub queue_id: Option<String>,
    /// `None` : nombre de connexions automatique (SPEC §7.3).
    pub max_connections: Option<u32>,
    /// Ajoutée en pause (cause `user`), sans partir.
    pub start_paused: bool,
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
    Shutdown(oneshot::Sender<()>),
}

/// Poignée du moteur ; se clone librement. Le moteur s'arrête par [`Engine::shutdown`],
/// ou quand la dernière poignée disparaît.
#[derive(Debug, Clone)]
pub struct Engine {
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<Event>,
    default_queue: String,
}

impl Engine {
    /// Ouvre la base, crée la file par défaut et lance la boucle du planificateur, qui
    /// reprend aussitôt les tâches interrompues par un arrêt du moteur (SPEC §7.4).
    /// Demande un environnement d'exécution `tokio`.
    pub fn start(config: Config) -> Result<Engine, EngineError> {
        let conn = store::open(&config.db_path)?;
        let default_queue = tasks::ensure_queue(&conn, DEFAULT_QUEUE)?;
        let (commands, inbox) = mpsc::channel(64);
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let state = Scheduler {
            conn,
            config,
            events: events.clone(),
            default_queue: default_queue.clone(),
            running: HashMap::new(),
            runs: JoinSet::new(),
            run_ids: HashMap::new(),
            broken: HashSet::new(),
            shutdown: None,
        };
        tokio::spawn(state.run(inbox));
        Ok(Engine {
            commands,
            events,
            default_queue,
        })
    }

    /// Identifiant de la file par défaut.
    pub fn default_queue(&self) -> &str {
        &self.default_queue
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
}

struct Scheduler {
    conn: Connection,
    config: Config,
    events: broadcast::Sender<Event>,
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
}

impl Scheduler {
    async fn run(mut self, mut inbox: mpsc::Receiver<Command>) {
        loop {
            if self.shutdown.is_none() {
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
                _ = tokio::time::sleep(wake), if self.shutdown.is_none() => {}
            }
        }
        if let Some(done) = self.shutdown.take() {
            let _ = done.send(());
        }
    }

    /// Délai jusqu'à la prochaine nouvelle tentative, au plus une minute.
    fn next_wake(&self) -> Duration {
        let cap = Duration::from_secs(60);
        match next_retry_at(&self.conn) {
            Ok(Some(at)) => {
                Duration::from_millis(u64::try_from(at.saturating_sub(now_ms())).unwrap_or(0))
                    .min(cap)
            }
            _ => cap,
        }
    }

    fn emit(&self, event: Event) {
        // Personne à l'écoute : rien à faire.
        let _ = self.events.send(event);
    }

    fn changed(&self, task_id: &str, from: TaskState, to: TaskState) {
        if from != to {
            self.emit(Event::StateChanged {
                task_id: task_id.to_owned(),
                from,
                to,
            });
        }
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
            let used = busy.entry(c.queue_id.clone()).or_default();
            if *used >= c.max_active {
                continue;
            }
            *used += 1;
            self.launch(c.task_id, c.queue_id, c.state);
        }
    }

    fn launch(&mut self, task_id: String, queue_id: String, from: TaskState) {
        let (stop, stopped) = watch::channel(false);
        self.running.insert(
            task_id.clone(),
            Running {
                queue_id,
                stop,
                then: Then::Keep,
            },
        );
        self.changed(&task_id, from, TaskState::Active);
        let (config, events) = (self.config.clone(), self.events.clone());
        let id = task_id.clone();
        let handle = self.runs.spawn(async move {
            let result = execute(&config, &id, stopped, events).await;
            (id, result)
        });
        self.run_ids.insert(handle.id(), task_id);
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
            Command::Shutdown(done) => self.begin_shutdown(Some(done)),
        }
    }

    fn add(&mut self, task: AddTask) -> Result<String, EngineError> {
        net::check_url(&task.url)?;
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
                dest_dir: &task.dest_dir,
                max_connections: task.max_connections,
            },
        )?;
        self.emit(Event::Added {
            task_id: task_id.clone(),
        });
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
            TaskState::Paused if task.pause_reason.as_deref() == Some("file_changed") => {
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
        let then = self.running.remove(task_id).map_or(Then::Keep, |r| r.then);
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
        self.record_internal_error(&task_id, &TransferError::Join(error));
        if let Ok(task) = load_task(&self.conn, &task_id) {
            self.changed(&task_id, TaskState::Active, task.state);
        }
    }
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
    let path = part_path(&Path::new(&task.dest_dir).join(name));
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
) -> Result<Outcome, TransferError> {
    let mut conn = store::open(&config.db_path)?;
    let mut on_progress = |progress: Progress| {
        let _ = events.send(Event::Progress {
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
