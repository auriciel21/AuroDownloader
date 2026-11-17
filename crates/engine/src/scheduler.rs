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

use crate::io::checksum::Checksum;
use crate::io::part_path;
use crate::limit::{Bucket, Throttle};
use crate::linkscan;
use crate::media;
use crate::metalink::{self, Metalink};
use crate::mirrors;
use crate::net::{self, Client, NetError};
use crate::schedule::{EndAction, LocalNow, Rules, Schedule};
use crate::secrets::{Vault, VaultError};
use crate::segmenter::Segment;
use crate::store::headers::{self, Stored, StoredHeader};
use crate::store::history::{self, HistoryFilter, HistoryRow};
use crate::store::segments::load_segments;
use crate::store::settings::Flag;
use crate::store::tasks::{
    self, Change, NewTask, PauseReason, QueueRow, SourceRow, TaskFilter, TaskKind, TaskRow,
    TaskState, change_state, load_task, next_retry_at, runnable_tasks,
};
use crate::store::{self, StoreError, now_ms, schedules, settings};
use crate::torrent::{self, Torrents};
use crate::transfer::{self, LiveSegment, Options, Outcome, Progress, TransferError};

mod planning;
#[cfg(test)]
mod tests;

/// Nom de la file créée au premier démarrage.
pub const DEFAULT_QUEUE: &str = "default";

/// Tailles lues en même temps par `linkscan.probe` (SPEC §7.18).
pub const LINK_PROBES: usize = 4;
/// Intervalle de vérification de la place libre pour les tâches en pause `disk_full`
/// (SPEC §7.10).
pub const DISK_RECHECK: Duration = Duration::from_secs(30);

/// Intervalle de surveillance de l'interface réseau des torrents (VPN, SPEC §7.16) : au
/// plus ce délai entre sa disparition et l'arrêt des torrents, qui ne passent de toute
/// façon plus par une autre (sockets liées à elle).
pub const INTERFACE_CHECK: Duration = Duration::from_secs(2);

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
    /// Le réseau convient aux téléchargements dès le démarrage (SPEC §7.15) ; vrai par
    /// défaut. Faux pour l'application Android : rien ne part avant qu'elle le dise
    /// (`Engine::set_network`), une fois les téléchargements protégés par Android.
    pub network_usable: bool,
}

impl Default for Planning {
    fn default() -> Self {
        Planning {
            clock: crate::schedule::local_now,
            check_every: SCHEDULE_CHECK,
            end_delay: END_ACTION_DELAY,
            power: crate::power::unplugged,
            network_usable: true,
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
    /// Empreinte attendue (F-11) ; `Some(None)` : la retire.
    pub checksum: Option<Option<Checksum>>,
}

impl TaskUpdate {
    fn is_empty(&self) -> bool {
        self.speed_limit_bps.is_none() && self.checksum.is_none()
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
        /// Dossier : celui d'une règle de rangement est choisi à la préparation (SPEC §7.21).
        dest_dir: String,
        file_name: Option<String>,
        /// Nom affiché : celui du fichier, ou le titre d'une vidéo (SPEC §7.17).
        display_name: String,
        total_bytes: Option<u64>,
        /// Empreinte lue dans un fichier voisin (F-11), avec le nom de ce fichier.
        found_checksum: Option<(Checksum, String)>,
    },
    /// Tâche qui repart de zéro (`task.restart`) : nom, taille, octets et erreur effacés,
    /// avant son passage à `queued`.
    Restarted {
        rev: u64,
        task_id: String,
        /// Empreinte gardée : celle de l'utilisateur ; une empreinte lue dans un fichier
        /// voisin est oubliée (F-11).
        checksum: Option<Checksum>,
    },
    /// Dossier ou nom d'une tâche changé (« Déplacer/Renommer », SPEC §7.12) ; aussi au
    /// début d'un déplacement vers un autre volume (`moving`), et à sa fin, réussie ou non
    /// (`error`).
    Relocated {
        rev: u64,
        task_id: String,
        dest_dir: String,
        file_name: Option<String>,
        display_name: String,
        moving: bool,
        error: Option<String>,
    },
    /// Adresse d'une tâche remplacée (« Actualiser l'adresse », SPEC §7.13).
    SourceChanged {
        rev: u64,
        task_id: String,
        display_name: String,
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
        /// Envoi (torrents, SPEC §7.16).
        upload_bps: Option<u64>,
    },
    /// Réglage oui ou non changé (réservation de la place, empreintes voisines).
    FlagChanged {
        rev: u64,
        flag: Flag,
        on: bool,
    },
    /// L'interface réseau des torrents (SPEC §7.16) est là, ou non : sans elle, les
    /// torrents attendent.
    TorrentInterfaceChanged {
        rev: u64,
        up: bool,
    },
    /// Les outils de la vidéo ont changé (SPEC §7.17) : installation commencée, finie ou
    /// en échec, yt-dlp mis à jour.
    ToolsChanged {
        rev: u64,
        status: media::tools::Status,
    },
    /// Avertissement (`engine.warning`) : `YTDLP_UPDATED`, `YTDLP_UPDATE_FAILED`,
    /// `SUBTITLES_MISSING` (SPEC §7.17).
    Warning {
        rev: u64,
        code: &'static str,
        message: String,
        /// La tâche concernée, s'il y en a une.
        task_id: Option<String>,
    },
    /// Le réseau convient, ou non, aux téléchargements (SPEC §7.15) ; `metered` : facturé
    /// au volume, sans partage de torrents (SPEC §7.16).
    NetworkChanged {
        rev: u64,
        usable: bool,
        metered: bool,
    },
    /// Réglage changé, hors réglages oui ou non et dossier : sa clé et sa nouvelle valeur
    /// (`torrent.ratioLimit`, SPEC §7.16).
    SettingChanged {
        rev: u64,
        key: &'static str,
        value: serde_json::Value,
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
    /// Un jeton de l'interface distante a été révoqué (SPEC §7.19) : ses connexions se
    /// ferment.
    TokenRevoked {
        rev: u64,
        token_id: String,
    },
}

/// Ce que `task.get` demande en plus de la tâche (SPEC §6.4, `include`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Include {
    pub segments: bool,
    pub sources: bool,
    /// Fichiers d'un torrent (SPEC §7.16).
    pub files: bool,
}

/// Une tâche avec ses segments, ses sources et ses fichiers, si demandés.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDetails {
    pub task: TaskRow,
    /// Segments enregistrés, chacun avec ses octets reçus et sa source (SPEC §7.7) s'il est
    /// en cours de réception.
    pub segments: Option<Vec<(Segment, Option<LiveSegment>)>>,
    pub sources: Option<Vec<SourceRow>>,
    /// Fichiers d'un torrent ; vide pour une tâche HTTP, ou avant la préparation.
    pub files: Option<Vec<store::torrents::TorrentFile>>,
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
    /// Limite d'envoi globale (torrents ; `None` : aucune).
    pub upload_limit: Option<u64>,
    /// Dossier des ajouts sans `destDir` ; `None` s'il n'y en a aucun.
    pub download_dir: Option<String>,
    /// Réglages oui ou non (F-10, F-11), dans l'ordre de [`Flag::ALL`].
    pub flags: Vec<(Flag, bool)>,
    /// Ratio de partage des torrents : un nombre, ou `null` pour sans limite.
    pub torrent_ratio: serde_json::Value,
    /// Le réseau convient aux téléchargements (SPEC §7.15).
    pub network_usable: bool,
    /// Réseau facturé au volume : pas de partage de torrents (SPEC §7.16).
    pub network_metered: bool,
    /// Interface réseau des torrents (réglage `torrent.interface`) ; `None` : aucune.
    pub torrent_interface: Option<String>,
    /// L'interface des torrents est là, ou aucune n'est demandée (SPEC §7.16).
    pub torrent_interface_up: bool,
    /// Niveau des journaux (réglage `logs.level`, N-12).
    pub logs_level: crate::logs::Level,
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

    /// Connexion à jeton : hors du dossier des téléchargements (SPEC §7.19).
    #[error("une tâche ajoutée à distance n'écrit que sous le dossier des téléchargements")]
    DestForbidden,

    #[error("la tâche {task_id} est dans l'état {state} : {action} impossible")]
    InvalidState {
        task_id: String,
        state: &'static str,
        action: &'static str,
    },

    #[error(
        "le fichier distant a changé depuis le début du téléchargement : il ne se reprend \
         pas, il se retélécharge depuis le début (task.restart, F-13)"
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

    #[error(transparent)]
    Vault(#[from] VaultError),

    #[error("la tâche {0} : déplacement de son fichier en cours, attendre sa fin")]
    Moving(String),

    #[error("{} existe déjà : le moteur n'écrase jamais un fichier", .0.display())]
    FileExists(PathBuf),

    #[error("{} n'existe plus : rien à déplacer", .0.display())]
    FileMissing(PathBuf),

    #[error("déplacement impossible : {0}")]
    Move(#[source] std::io::Error),

    #[error(
        "la nouvelle adresse donne un fichier de {found} au lieu de {expected} octets : ce \
         n'est pas le même fichier"
    )]
    SourceMismatch { expected: u64, found: String },

    #[error("{}", unreachable_text(.0))]
    SourceUnreachable(#[source] NetError),

    /// Un miroir qui ne sert pas le même fichier (SPEC §7.7).
    #[error("miroir refusé : {0}")]
    MirrorRefused(String),

    #[error("identifiants inconnus : {0}")]
    LoginNotFound(String),

    /// Fonction absente de cette plateforme (la vidéo hors de Windows et Linux).
    #[error("{0}")]
    CapabilityMissing(String),

    /// yt-dlp ou ffmpeg n'est pas installé (SPEC §7.17) : `media.installTools`.
    #[error("{0}")]
    ToolsMissing(String),

    /// yt-dlp n'a pas pu lire la page (SPEC §7.17) ; message sans l'adresse.
    #[error("yt-dlp : {0}")]
    Extractor(String),

    /// Un plugin d'hébergeur s'est arrêté, ou a rendu une sortie refusée (SPEC §7.20).
    #[error("plugin : {0}")]
    PluginFailed(String),

    /// Plugin inconnu, aucun plugin activé pour ce site, ou résolution expirée.
    #[error("{0}")]
    PluginNotFound(String),

    /// La page d'un plugin donne plusieurs liens : à choisir (`plugin.resolve`).
    #[error(
        "le plugin a trouvé {0} liens sur cette page : les choisir (plugin.resolve, plugin.add)"
    )]
    PluginChoice(usize),
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
    /// En-têtes HTTP de la tâche (`Referer`, `Cookie`…) ; les secrets vont au coffre du
    /// système (SPEC §5.1).
    pub headers: Vec<(String, String)>,
    /// Nom du fichier choisi (SPEC §7.12) ; `None` : celui du serveur.
    pub file_name: Option<String>,
    /// Empreinte attendue du fichier (F-11), vérifiée avant son renommage.
    pub checksum: Option<Checksum>,
    /// Contenu d'un fichier `.torrent` (SPEC §7.16) ; `url` est alors ignorée. Un lien
    /// `magnet:` dans `url` fait aussi une tâche torrent.
    pub torrent: Option<Vec<u8>>,
    /// Vidéo d'une page (SPEC §7.17) ; `url` est alors ignorée.
    pub media: Option<MediaRequest>,
    /// Miroirs du même fichier (SPEC §7.7, phase 6) : adresses HTTP(S) après la référence
    /// `url`, validées à la première exécution.
    pub mirrors: Vec<String>,
    /// Dossiers sous `dest_dir`, créés s'ils manquent (fichiers d'un Metalink, SPEC §7.7) ;
    /// chacun doit être un nom sûr tel quel. Tâche HTTP seulement.
    pub sub_dir: Vec<String>,
    /// Ajout par une connexion à jeton (SPEC §7.19) : fichier sous le dossier des
    /// téléchargements.
    pub remote: bool,
    /// Réseau public seulement, pour toute la vie de la tâche (connexion à jeton, sauf
    /// réglage `remote.allowPrivate` ; lien rendu par un plugin).
    pub public_only: bool,
    /// Lien rendu par un plugin d'hébergeur (SPEC §7.20) : la page et le plugin, pour
    /// redemander un lien neuf quand celui-ci expire.
    pub plugin: Option<PluginOrigin>,
    /// Adresse ajoutée telle quelle, même si un plugin prend son site en charge
    /// (`plugin: false` de `task.add`, SPEC §7.20).
    pub no_plugin: bool,
}

/// Poignée faible du moteur ([`Engine::downgrade`]).
#[derive(Clone)]
pub(crate) struct WeakEngine {
    commands: mpsc::WeakSender<Command>,
    /// Le reste de la poignée, sans canal vers la boucle ni annonces.
    rest: Engine,
}

impl WeakEngine {
    /// Une poignée, tant que le moteur tourne.
    pub(crate) fn upgrade(&self) -> Option<Engine> {
        Some(Engine {
            commands: self.commands.upgrade()?,
            ..self.rest.clone()
        })
    }

    /// Le moteur s'est arrêté.
    pub(crate) fn stopped(&self) -> watch::Receiver<bool> {
        self.rest.stopped.clone()
    }
}

/// Page d'origine d'une tâche dont le lien vient d'un plugin (SPEC §7.20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginOrigin {
    pub page: String,
    pub plugin_id: String,
}

/// Opérations sur la table des plugins, faites par la boucle du planificateur.
#[derive(Debug)]
pub enum PluginOp {
    List,
    Install(Box<store::plugins::PluginRow>),
    Remove(String),
    SetEnabled(String, bool),
}

/// Réponse à une [`PluginOp`] ; les empreintes rendues sont celles de fichiers que plus
/// aucun plugin n'utilise, à effacer.
#[derive(Debug)]
pub enum PluginAnswer {
    Rows(Vec<store::plugins::PluginRow>),
    Installed(store::plugins::PluginRow, Option<String>),
    Removed { found: bool, unused: Option<String> },
    Enabled(Option<store::plugins::PluginRow>),
}

/// Fichier Metalink à lire (`metalink.read`, `metalink.add`, SPEC §7.7).
#[derive(Debug, Clone)]
pub enum MetalinkSource {
    /// Contenu du fichier.
    Data(Vec<u8>),
    /// Adresse HTTP(S) du fichier, lue par le moteur.
    Url(String),
}

/// Vidéo demandée par `task.add` (SPEC §7.17).
#[derive(Debug, Clone, Default)]
pub struct MediaRequest {
    pub page_url: String,
    /// Sélecteur de formats de yt-dlp ; `None` : `bv*+ba/b`.
    pub format: Option<String>,
    /// `mp4`, `mkv` ou `webm` ; `None` : automatique.
    pub container: Option<String>,
    /// Langues des sous-titres (étape 4e).
    pub subtitles: Vec<String>,
    /// Titre connu du client (`media.probe`), nom affiché en attendant la résolution.
    pub title: Option<String>,
}

/// Sélecteur de formats par défaut : la meilleure vidéo et le meilleur son, sinon le
/// meilleur format complet.
pub const DEFAULT_FORMAT: &str = "bv*+ba/b";

type Reply<T> = oneshot::Sender<Result<T, EngineError>>;

/// Identifiants d'un site tels que le protocole les montre : jamais le mot de passe
/// (SPEC §7.14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginView {
    pub id: String,
    /// Forme normalisée du site.
    pub site: String,
    pub username: String,
}

impl From<store::logins::LoginRow> for LoginView {
    fn from(row: store::logins::LoginRow) -> Self {
        LoginView {
            id: row.id,
            site: row.site,
            username: row.username,
        }
    }
}

/// Proxy à enregistrer (`proxy.set`, F-07) : ses réglages, et le mot de passe (`None` :
/// gardé ; `Some(None)` : retiré ; `Some(Some(…))` : nouveau).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyChange {
    pub settings: net::proxy::Settings,
    pub password: Option<Option<String>>,
}

/// Proxy tel que le protocole le montre : jamais le mot de passe, seulement s'il y en a un.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyView {
    pub settings: net::proxy::Settings,
    pub has_password: bool,
}

/// Identifiants à enregistrer (`login.set`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginChange {
    /// `None` : de nouveaux identifiants.
    pub id: Option<String>,
    pub site: String,
    pub username: String,
    /// Obligatoire pour de nouveaux identifiants ; `None` garde le mot de passe enregistré.
    pub password: Option<String>,
}

/// Longueur maximale d'un nom d'utilisateur, en octets.
pub const MAX_USERNAME: usize = 256;

/// Ce que la boucle donne pour vérifier une nouvelle adresse (SPEC §7.13).
#[derive(Debug, Clone)]
pub struct SourceCheck {
    /// Taille à retrouver à la nouvelle adresse ; `None` : rien à vérifier (tâche pas
    /// encore préparée, de taille inconnue, terminée ou annulée).
    expect_bytes: Option<u64>,
    /// En-têtes à envoyer ; sans les secrets si l'hôte change.
    headers: Vec<StoredHeader>,
    /// L'hôte change : les secrets de la tâche (cookies) ne suivent pas.
    other_host: bool,
    /// Identifiants du site de la nouvelle adresse (SPEC §7.14).
    login: Option<store::logins::LoginRow>,
}

/// Ce qu'il faut pour valider un nouveau miroir (`task.addSource`, SPEC §7.7).
pub struct MirrorCheck {
    /// Source de référence de la tâche.
    reference: SourceRow,
    /// Taille du fichier, si le miroir peut être validé tout de suite (tâche préparée,
    /// référence qui accepte les plages) ; `None` : à la première exécution.
    total: Option<u64>,
    /// En-têtes de la tâche ; le miroir n'en reçoit que ce que `mirrors::headers_for` permet.
    headers: Vec<StoredHeader>,
    /// Identifiants du site de la référence et de celui du miroir (SPEC §7.14).
    reference_login: Option<store::logins::LoginRow>,
    mirror_login: Option<store::logins::LoginRow>,
}

/// Ce que le moteur a appris de la nouvelle adresse.
#[derive(Debug, Clone)]
pub struct SourceFacts {
    total_bytes: u64,
    accepts_ranges: bool,
    etag: Option<String>,
    last_modified: Option<String>,
}

enum Command {
    // Dans une boîte : la demande d'ajout (miroirs, en-têtes) est la plus grosse commande.
    Add(Box<AddTask>, Reply<String>),
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
    Restart {
        task_id: String,
        delete_files: bool,
        reply: Reply<()>,
    },
    Relocate {
        task_id: String,
        dest_dir: Option<String>,
        file_name: Option<String>,
        remote: bool,
        reply: Reply<TaskRow>,
    },
    /// Ce qu'il faut pour vérifier une nouvelle adresse, hors de la boucle.
    CheckSource {
        task_id: String,
        url: String,
        reply: Reply<SourceCheck>,
    },
    ReplaceUrl {
        task_id: String,
        url: String,
        facts: Option<SourceFacts>,
        public_only: bool,
        reply: Reply<TaskRow>,
    },
    CheckMirror {
        task_id: String,
        url: String,
        reply: Reply<MirrorCheck>,
    },
    AddMirror {
        task_id: String,
        url: String,
        facts: Option<mirrors::Facts>,
        public_only: bool,
        reply: Reply<String>,
    },
    Get(String, Reply<TaskRow>),
    Details(String, Include, Reply<TaskDetails>),
    List(TaskFilter, Reply<(Vec<TaskRow>, u64)>),
    History(HistoryFilter, Reply<Vec<HistoryRow>>),
    ClearHistory(Option<i64>, Reply<u64>),
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
    Flag(Flag, Reply<bool>),
    SetFlag(Flag, bool, Reply<()>),
    SetNetwork(bool, bool, Reply<()>),
    UploadLimit(Reply<Option<u64>>),
    SetUploadLimit(Option<u64>, Reply<()>),
    TorrentRatio(Reply<Option<f64>>),
    SetTorrentRatio(Option<f64>, Reply<()>),
    TorrentInterface(Reply<Option<String>>),
    /// Les outils de la vidéo ont changé (installation, mise à jour) : à annoncer.
    ToolsChanged(media::tools::Status),
    /// Avertissement du moteur à annoncer (`engine.warning`).
    Warn(&'static str, String),
    SetTorrentInterface(Option<String>, Reply<()>),
    LogsLevel(Reply<crate::logs::Level>),
    Proxy(Reply<ProxyView>),
    SetProxy(ProxyChange, Reply<ProxyView>),
    Rules(Reply<Vec<crate::rules::Rule>>),
    SetRules(Vec<crate::rules::Rule>, Reply<Vec<crate::rules::Rule>>),
    SetLogsLevel(crate::logs::Level, Reply<()>),
    RemoteListen(Reply<std::net::SocketAddr>),
    SetRemoteListen(Option<std::net::SocketAddr>, Reply<std::net::SocketAddr>),
    RemoteConfig(Reply<settings::RemoteConfig>),
    SetRemoteHosts(Vec<String>, Reply<()>),
    SetRemoteTls(Option<(String, String)>, Reply<()>),
    SetFiles(String, Vec<u32>, Reply<TaskRow>),
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
    Logins(Reply<Vec<LoginView>>),
    SetLogin(LoginChange, Reply<LoginView>),
    DeleteLogin(String, Reply<()>),
    /// Jetons et sessions de l'interface distante (SPEC §7.19).
    Tokens(crate::remote::TokenOp, Reply<crate::remote::TokenAnswer>),
    /// Plugins installés (SPEC §7.20).
    Plugins(PluginOp, Reply<PluginAnswer>),
    /// Page et plugin d'origine d'une tâche (SPEC §7.20).
    PluginOrigin(String, Reply<Option<(String, String)>>),
    /// En-têtes d'une tâche remplacés (lien neuf rendu par un plugin).
    ReplaceHeaders(String, Vec<(String, String)>, Reply<()>),
    /// Secrets relevés dans le coffre au démarrage, pour le ramassage.
    CollectSecrets(Vec<String>),
    Shutdown(oneshot::Sender<()>),
}

/// Poignée du moteur ; se clone librement. Le moteur s'arrête par [`Engine::shutdown`],
/// ou quand la dernière poignée disparaît.
#[derive(Debug, Clone)]
pub struct Engine {
    commands: mpsc::Sender<Command>,
    /// Pour vérifier une nouvelle adresse hors de la boucle (`set_url`).
    client: Client,
    vault: Vault,
    events: broadcast::Sender<Event>,
    default_queue: String,
    rev: Arc<AtomicU64>,
    stopped: watch::Receiver<bool>,
    /// Outils de la vidéo (SPEC §7.17) ; `None` hors de Windows et Linux.
    tools: Option<Arc<media::tools::Tools>>,
    /// État de l'écoute de l'interface distante (SPEC §7.19), partagé par les copies.
    remote_state: Arc<std::sync::Mutex<crate::remote::RemoteState>>,
    /// Plugins d'hébergeurs (SPEC §7.20) : fichiers, places, résolutions gardées.
    plugins: Arc<crate::plugins::Plugins>,
    /// Dossier de la base : certificat fabriqué de l'interface distante (SPEC §7.19).
    data_dir: PathBuf,
    /// Poignée d'une connexion à jeton (SPEC §7.19) : ajouts et déplacements sous le dossier
    /// des téléchargements ; voir [`Engine::restricted`].
    remote: bool,
    /// Et réseau public seulement, sauf réglage `remote.allowPrivate`.
    public_only: bool,
}

impl Engine {
    /// Ouvre la base, crée la file par défaut et lance la boucle du planificateur, qui
    /// reprend aussitôt les tâches interrompues par un arrêt du moteur (SPEC §7.4).
    /// Demande un environnement d'exécution `tokio`.
    pub fn start(mut config: Config) -> Result<Engine, EngineError> {
        let mut conn = store::open(&config.db_path)?;
        let data_dir = config
            .db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        // Déplacements interrompus par un arrêt : finis ou annulés avant toute exécution
        // (SPEC §7.12).
        crate::moves::recover(&mut conn)?;
        // Noms réservés sans être enregistrés, avant un arrêt (SPEC §5.6, règle 7).
        crate::claims::recover(&conn)?;
        let default_queue = tasks::ensure_queue(&conn, DEFAULT_QUEUE)?;
        let download_limit = settings::download_limit(&conn)?;
        // Les secrets de cette base seulement (voir `secrets`).
        let vault = config
            .transfer
            .vault
            .scoped(&settings::vault_namespace(&conn)?);
        config.transfer.vault = vault.clone();
        let vault_for_handle = vault.clone();
        // Proxy (F-07) : appliqué avant tout téléchargement, le mot de passe lu au coffre.
        // Des réglages refusés, ou un coffre fermé, laissent le proxy du système.
        let (proxy_settings, proxy_secret) = settings::proxy(&conn)?;
        let password = proxy_secret.as_deref().and_then(|id| {
            vault.get(id).unwrap_or_else(|e| {
                tracing::warn!(erreur = %e, "mot de passe du proxy illisible au coffre");
                None
            })
        });
        let proxy = net::proxy::Effective {
            settings: proxy_settings,
            password,
        };
        if let Err(e) = config.client.set_proxy(proxy) {
            tracing::warn!(erreur = %e, "réglages du proxy refusés : proxy du système");
        }
        let config_proxy = config.client.proxy_shared();
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
        let tools = state.tools.clone();
        let client = state.config.client.clone();
        tokio::spawn(async move {
            state.run(inbox, prepared).await;
            let _ = stopped_tx.send(true);
        });
        // Ramassage des secrets orphelins : le relevé du coffre, hors de la boucle, puis
        // la comparaison avec la base, dans la boucle.
        let collect = commands.clone();
        tokio::spawn(async move {
            match tokio::task::spawn_blocking(move || vault.ids()).await {
                Ok(Ok(found)) if !found.is_empty() => {
                    let _ = collect.send(Command::CollectSecrets(found)).await;
                }
                Ok(Err(e)) => tracing::warn!(erreur = %e, "relevé du coffre impossible"),
                _ => {}
            }
        });
        let engine = Engine {
            commands,
            client,
            vault: vault_for_handle,
            events,
            default_queue,
            rev,
            stopped,
            tools,
            public_only: false,
            remote: false,
            remote_state: Arc::default(),
            plugins: Arc::new(crate::plugins::Plugins::new(&data_dir, config_proxy)),
            data_dir,
        };
        // Liens expirés des tâches venues d'un plugin (SPEC §7.20) : poignée faible, qui
        // n'empêche pas le moteur de s'arrêter quand ses poignées disparaissent.
        tokio::spawn(crate::plugins::watch(
            engine.downgrade(),
            engine.subscribe(),
        ));
        Ok(engine)
    }

    /// La poignée d'une connexion à jeton (SPEC §7.19) : ses lectures du réseau passent
    /// par le client restreint au réseau public ; ses ajouts, ses miroirs et ses
    /// adresses marquent la tâche (`public_only`), qui n'écrit que sous le dossier des
    /// téléchargements et ne joint que le réseau public, pour toute sa vie.
    /// `allow_private` : réglage `remote.allowPrivate`, qui lève la limite au réseau public
    /// (pas celle du dossier).
    pub fn restricted(&self, allow_private: bool) -> Result<Engine, EngineError> {
        let client = if allow_private {
            self.client.clone()
        } else {
            self.client.restricted().map_err(EngineError::Url)?
        };
        Ok(Engine {
            client,
            remote: true,
            public_only: !allow_private,
            ..self.clone()
        })
    }

    /// Poignée d'une connexion à jeton ?
    pub fn is_restricted(&self) -> bool {
        self.remote
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

    /// Historique (F-35) selon `filter`, les entrées les plus récentes d'abord.
    pub async fn history(&self, filter: HistoryFilter) -> Result<Vec<HistoryRow>, EngineError> {
        self.ask(|reply| Command::History(filter, reply)).await
    }

    /// Efface l'historique, ou ses entrées finies avant `before` ; renvoie leur nombre.
    pub async fn clear_history(&self, before: Option<i64>) -> Result<u64, EngineError> {
        self.ask(|reply| Command::ClearHistory(before, reply)).await
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

    /// Réglage oui ou non (réservation de la place F-10, empreintes voisines F-11), vrai
    /// par défaut.
    pub async fn flag(&self, flag: Flag) -> Result<bool, EngineError> {
        self.ask(|reply| Command::Flag(flag, reply)).await
    }

    /// Change un réglage oui ou non ; vaut pour les exécutions qui démarrent ensuite.
    pub async fn set_flag(&self, flag: Flag, on: bool) -> Result<(), EngineError> {
        self.ask(|reply| Command::SetFlag(flag, on, reply)).await
    }

    /// Le réseau convient, ou non, aux téléchargements (SPEC §7.15) : sans lui, les tâches
    /// en cours retournent en file et rien ne démarre. Dit par l'application ; vrai au
    /// démarrage du moteur. `metered` : réseau facturé au volume, sans partage de torrents
    /// (SPEC §7.16).
    pub async fn set_network(&self, usable: bool, metered: bool) -> Result<(), EngineError> {
        self.ask(|reply| Command::SetNetwork(usable, metered, reply))
            .await
    }

    /// Limite d'envoi globale (torrents, SPEC §7.16) ; `None` : aucune.
    pub async fn upload_limit(&self) -> Result<Option<u64>, EngineError> {
        self.ask(Command::UploadLimit).await
    }

    /// Change la limite d'envoi globale ; enregistrée, elle s'applique aussitôt.
    pub async fn set_upload_limit(&self, limit: Option<u64>) -> Result<(), EngineError> {
        self.ask(|reply| Command::SetUploadLimit(limit, reply))
            .await
    }

    /// Ratio de partage des torrents ; `None` : sans limite.
    pub async fn torrent_ratio(&self) -> Result<Option<f64>, EngineError> {
        self.ask(Command::TorrentRatio).await
    }

    /// Change le ratio de partage des torrents (`0` : pas de partage ; `None` : sans
    /// limite). Vaut pour les partages qui commencent ensuite.
    pub async fn set_torrent_ratio(&self, ratio: Option<f64>) -> Result<(), EngineError> {
        self.ask(|reply| Command::SetTorrentRatio(ratio, reply))
            .await
    }

    /// Outils de la vidéo (SPEC §7.17), ou `CapabilityMissing` hors de Windows et Linux.
    fn media_tools_handle(&self) -> Result<&Arc<media::tools::Tools>, EngineError> {
        self.tools.as_ref().ok_or_else(|| {
            EngineError::CapabilityMissing(
                "vidéo : seulement sous Windows et Linux en v1 (SPEC §7.17)".into(),
            )
        })
    }

    /// État des outils de la vidéo (`media.tools`).
    pub fn media_tools(&self) -> Result<media::tools::Status, EngineError> {
        Ok(self.media_tools_handle()?.status())
    }

    /// Installe les outils de la vidéo qui manquent (tous si `force`), en arrière-plan :
    /// rend l'état aussitôt, installation en cours. La fin s'annonce par
    /// `Event::ToolsChanged`. Seulement sur la demande de l'utilisateur (SPEC §7.17).
    pub async fn install_media_tools(
        &self,
        force: bool,
    ) -> Result<media::tools::Status, EngineError> {
        let tools = Arc::clone(self.media_tools_handle()?);
        if tools.begin_install() {
            let commands = self.commands.clone();
            let installing = Arc::clone(&tools);
            tokio::spawn(async move {
                if let Err(e) = installing.install(force).await {
                    tracing::warn!(erreur = %e, "outils de la vidéo non installés");
                }
                let _ = commands
                    .send(Command::ToolsChanged(installing.status()))
                    .await;
            });
        }
        let status = tools.status();
        let _ = self
            .commands
            .send(Command::ToolsChanged(status.clone()))
            .await;
        Ok(status)
    }

    /// Décrit la vidéo d'une page (`media.probe`, SPEC §7.17) par yt-dlp. Adresses `http`
    /// et `https` seulement. À cette occasion, yt-dlp se met à jour en arrière-plan si
    /// c'est le moment (`media.autoUpdate`).
    pub async fn probe_media(&self, page_url: &str) -> Result<media::extract::Probe, EngineError> {
        let tools = Arc::clone(self.media_tools_handle()?);
        crate::net::check_url(page_url)?;
        if self.public_only {
            let url = net::parse_url(page_url)?;
            net::reach::check_public(&url)
                .await
                .map_err(EngineError::SourceUnreachable)?;
        }
        self.update_ytdlp_if_due(&tools).await;
        let Some((ytdlp, _running)) = tools.ytdlp().await else {
            return Err(EngineError::ToolsMissing(
                "yt-dlp n'est pas installé (media.installTools)".into(),
            ));
        };
        media::extract::probe(
            &ytdlp,
            &self.client,
            page_url,
            media::extract::PROBE_TIMEOUT,
        )
        .await
        .map_err(|e| EngineError::Extractor(e.to_string()))
    }

    /// Lance la mise à jour de yt-dlp en arrière-plan si le réglage le permet ; elle-même
    /// n'agit qu'une fois par jour au plus. Annoncée par `engine.warning`.
    async fn update_ytdlp_if_due(&self, tools: &Arc<media::tools::Tools>) {
        if !self.flag(Flag::MediaAutoUpdate).await.unwrap_or(false) {
            return;
        }
        let tools = Arc::clone(tools);
        let commands = self.commands.clone();
        tokio::spawn(async move {
            let warning = match tools.update_ytdlp_if_due(crate::store::now_ms()).await {
                Ok(None) => return,
                Ok(Some(version)) => (
                    "YTDLP_UPDATED",
                    format!("yt-dlp mis à jour : version {version}"),
                ),
                Err(e) => (
                    "YTDLP_UPDATE_FAILED",
                    format!("mise à jour de yt-dlp impossible, l'ancienne version reste : {e}"),
                ),
            };
            let _ = commands.send(Command::Warn(warning.0, warning.1)).await;
            let _ = commands.send(Command::ToolsChanged(tools.status())).await;
        });
    }

    /// Interface réseau des torrents (VPN, SPEC §7.16) ; `None` : aucune.
    pub async fn torrent_interface(&self) -> Result<Option<String>, EngineError> {
        self.ask(Command::TorrentInterface).await
    }

    /// Lie les torrents à une interface réseau (`None` : aucune). Les torrents en cours
    /// repartent sur une session liée à la nouvelle ; sans elle, ils attendent.
    pub async fn set_torrent_interface(&self, name: Option<String>) -> Result<(), EngineError> {
        self.ask(|reply| Command::SetTorrentInterface(name, reply))
            .await
    }

    /// Proxy du moteur (F-07), sans son mot de passe.
    pub async fn proxy(&self) -> Result<ProxyView, EngineError> {
        self.ask(Command::Proxy).await
    }

    /// Change le proxy : vérifié, mot de passe au coffre, appliqué aux prochaines
    /// connexions ; les téléchargements en cours finissent par l'ancien chemin.
    pub async fn set_proxy(&self, change: ProxyChange) -> Result<ProxyView, EngineError> {
        self.ask(|reply| Command::SetProxy(change, reply)).await
    }

    /// Règles de rangement, dans leur ordre (SPEC §7.21).
    pub async fn rules(&self) -> Result<Vec<crate::rules::Rule>, EngineError> {
        self.ask(Command::Rules).await
    }

    /// Remplace toutes les règles de rangement, dans l'ordre donné (vérifiées par
    /// `Rule::from_json`) ; rend la liste enregistrée, chaque règle avec son identifiant.
    /// Les tâches déjà préparées gardent leur rangement.
    pub async fn set_rules(
        &self,
        rules: Vec<crate::rules::Rule>,
    ) -> Result<Vec<crate::rules::Rule>, EngineError> {
        crate::rules::check_list(&rules).map_err(EngineError::BadRequest)?;
        self.ask(|reply| Command::SetRules(rules, reply)).await
    }

    /// Niveau des journaux (réglage `logs.level`, N-12).
    pub async fn logs_level(&self) -> Result<crate::logs::Level, EngineError> {
        self.ask(Command::LogsLevel).await
    }

    /// Change le niveau des journaux ; [`crate::logs::follow`] l'applique aussitôt.
    pub async fn set_logs_level(&self, level: crate::logs::Level) -> Result<(), EngineError> {
        self.ask(|reply| Command::SetLogsLevel(level, reply)).await
    }

    /// Adresse et port d'écoute de l'interface distante (`remote.listen`, SPEC §7.19).
    pub async fn remote_listen(&self) -> Result<std::net::SocketAddr, EngineError> {
        self.ask(Command::RemoteListen).await
    }

    /// Change l'écoute de l'interface distante (`None` : celle par défaut) ; rend celle
    /// retenue. L'écoute en cours repart sur la nouvelle (`remote::run`).
    pub async fn set_remote_listen(
        &self,
        addr: Option<std::net::SocketAddr>,
    ) -> Result<std::net::SocketAddr, EngineError> {
        self.ask(|reply| Command::SetRemoteListen(addr, reply))
            .await
    }

    /// Réglages de l'écoute de l'interface distante (SPEC §7.19).
    pub async fn remote_config(&self) -> Result<settings::RemoteConfig, EngineError> {
        self.ask(Command::RemoteConfig).await
    }

    /// Remplace les noms ajoutés de l'interface distante (`remote.hosts`) : noms DNS ou
    /// adresses IP, [`settings::REMOTE_HOSTS_MAX`] au plus.
    pub async fn set_remote_hosts(&self, hosts: Vec<String>) -> Result<(), EngineError> {
        self.ask(|reply| Command::SetRemoteHosts(hosts, reply))
            .await
    }

    /// Certificat et clé fournis (chemins absolus de fichiers PEM), ou `None` pour celui
    /// que le moteur fabrique (SPEC §7.19, étape 3b). Lus à l'ouverture de l'écoute.
    pub async fn set_remote_tls(&self, files: Option<(String, String)>) -> Result<(), EngineError> {
        self.ask(|reply| Command::SetRemoteTls(files, reply)).await
    }

    /// Dossier de la base du moteur.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// État de l'écoute de l'interface distante, tenu par `remote::run`.
    pub fn remote_state(&self) -> crate::remote::RemoteState {
        self.remote_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set_remote_state(&self, state: crate::remote::RemoteState) {
        *self
            .remote_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    }

    /// Choisit les fichiers d'un torrent à télécharger (`task.setFiles`, SPEC §7.16), par
    /// rang ; renvoie la tâche, avec sa nouvelle taille.
    pub async fn set_files(
        &self,
        task_id: &str,
        selected: Vec<u32>,
    ) -> Result<TaskRow, EngineError> {
        let id = task_id.to_owned();
        self.ask(|reply| Command::SetFiles(id, selected, reply))
            .await
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
    pub async fn add(&self, mut task: AddTask) -> Result<String, EngineError> {
        // Page d'un site qu'un plugin prend en charge : son lien direct (SPEC §7.20).
        if let Some(through) = self.through_plugin(&task).await? {
            task = through;
        }
        task.remote |= self.remote;
        if self.public_only {
            task.public_only = true;
            // yt-dlp résout lui-même le nom de la page : vérifié ici, avant l'ajout
            // (SPEC §7.19).
            if let Some(media) = &task.media {
                let url = net::parse_url(&media.page_url)?;
                net::reach::check_public(&url)
                    .await
                    .map_err(EngineError::SourceUnreachable)?;
            }
        }
        self.ask(|reply| Command::Add(Box::new(task), reply)).await
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

    /// Retélécharge une tâche (SPEC §5.5) : terminée, en échec, annulée, en pause ou en
    /// attente d'un nouvel essai, elle repart de zéro. Son `.part` est supprimé ; avec
    /// `delete_files`, le fichier final d'une tâche terminée aussi (sinon il reste, et le
    /// nouveau prend un autre nom).
    pub async fn restart(&self, task_id: &str, delete_files: bool) -> Result<(), EngineError> {
        let task_id = task_id.to_owned();
        self.ask(|reply| Command::Restart {
            task_id,
            delete_files,
            reply,
        })
        .await
    }

    /// Déplace ou renomme le fichier d'une tâche qui ne tourne pas (SPEC §7.12) : le
    /// fichier final d'une tâche terminée, le `.part` des autres ; une tâche pas encore
    /// préparée prendra ce dossier et ce nom. Sur le même volume, le fichier est à sa place
    /// au retour. Vers un autre volume, la copie continue après le retour (`moving`) ; sa
    /// fin est annoncée par [`Event::Relocated`].
    pub async fn relocate(
        &self,
        task_id: &str,
        dest_dir: Option<String>,
        file_name: Option<String>,
    ) -> Result<TaskRow, EngineError> {
        let task_id = task_id.to_owned();
        self.ask(|reply| Command::Relocate {
            task_id,
            dest_dir,
            file_name,
            remote: self.remote,
            reply,
        })
        .await
    }

    /// Remplace l'adresse d'une tâche qui ne tourne pas (« Actualiser l'adresse », SPEC
    /// §7.13), par exemple un lien expiré. Pour une tâche commencée, la nouvelle adresse
    /// est d'abord interrogée : elle doit donner un fichier de même taille. Les cookies de
    /// la tâche ne suivent pas vers un autre hôte.
    pub async fn set_url(&self, task_id: &str, url: &str) -> Result<TaskRow, EngineError> {
        net::check_url(url)?;
        if self.public_only {
            check_public_literal(url)?;
        }
        let (id, address) = (task_id.to_owned(), url.to_owned());
        let check = self
            .ask(|reply| Command::CheckSource {
                task_id: id,
                url: address,
                reply,
            })
            .await?;
        let facts = match check.expect_bytes {
            None => None,
            Some(expected) => {
                let headers = reveal(check.headers, &self.vault).await?;
                let auth = reveal_site_login(check.login, &self.vault).await?;
                let probe = self
                    .client
                    .probe_as(url, &headers, auth.as_ref())
                    .await
                    .map_err(EngineError::SourceUnreachable)?;
                if probe.total_bytes != Some(expected) {
                    return Err(EngineError::SourceMismatch {
                        expected,
                        found: probe
                            .total_bytes
                            .map_or_else(|| "taille inconnue".into(), |n| n.to_string()),
                    });
                }
                Some(SourceFacts {
                    total_bytes: expected,
                    accepts_ranges: probe.accepts_ranges,
                    etag: probe.etag,
                    last_modified: probe.last_modified,
                })
            }
        };
        let (task_id, url) = (task_id.to_owned(), url.to_owned());
        self.ask(|reply| Command::ReplaceUrl {
            task_id,
            url,
            facts,
            public_only: self.public_only,
            reply,
        })
        .await
    }

    /// Ajoute un miroir à une tâche HTTP pas encore terminée (`task.addSource`, SPEC §7.7) ;
    /// renvoie l'identifiant de la source. Une tâche préparée (taille connue, plages) voit
    /// son miroir validé tout de suite (taille et échantillons) : un autre fichier est
    /// refusé ici ; sinon il l'est à la première exécution.
    /// Lit un fichier Metalink (F-27, SPEC §7.7) : ses octets, ou une adresse que le moteur
    /// lit lui-même (4 Mio au plus, sans en-têtes ni identifiants). Rend le document et les
    /// octets lus, pour un ajout qui porte sur ce même document.
    pub async fn read_metalink(
        &self,
        source: MetalinkSource,
    ) -> Result<(Metalink, Vec<u8>), EngineError> {
        let bytes = match source {
            MetalinkSource::Data(bytes) => bytes,
            MetalinkSource::Url(url) => {
                match self.client.fetch_small_at(&url, metalink::MAX_LEN).await {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) => {
                        return Err(EngineError::BadRequest(format!(
                            "pas de fichier Metalink à cette adresse (réponse autre que 200, \
                             ou plus de {} Mio)",
                            metalink::MAX_LEN >> 20
                        )));
                    }
                    Err(e @ (NetError::InvalidUrl(_) | NetError::UnsupportedScheme(_))) => {
                        return Err(EngineError::Url(e));
                    }
                    Err(e) => return Err(EngineError::SourceUnreachable(e)),
                }
            }
        };
        let doc = metalink::parse(&bytes).map_err(|e| EngineError::BadRequest(e.to_string()))?;
        Ok((doc, bytes))
    }

    /// Les liens d'une page (analyse de liens, F-29, SPEC §7.18) : la page est lue par le
    /// moteur (4 Mio au plus, sans en-têtes ni identifiants), HTML seul. Rend son adresse
    /// après les redirections et ses liens.
    pub async fn scan_page(&self, url: &str) -> Result<(String, Vec<linkscan::Link>), EngineError> {
        match self.client.fetch_page(url, linkscan::MAX_INPUT).await {
            Ok(Some((final_url, bytes))) => {
                let html = String::from_utf8_lossy(&bytes);
                let links = linkscan::from_html(&html, &final_url);
                Ok((final_url.to_string(), links))
            }
            Ok(None) => Err(EngineError::BadRequest(format!(
                "pas de page lisible à cette adresse (réponse autre que 200, ou plus de {} Mio)",
                linkscan::MAX_INPUT >> 20
            ))),
            Err(e @ (NetError::InvalidUrl(_) | NetError::UnsupportedScheme(_))) => {
                Err(EngineError::Url(e))
            }
            Err(e) => Err(EngineError::SourceUnreachable(e)),
        }
    }

    /// Taille et nom de chaque fichier (analyse de liens, SPEC §7.18) : une requête `HEAD`,
    /// au besoin une requête d'un octet, [`LINK_PROBES`] à la fois ; les résultats dans
    /// l'ordre des adresses.
    pub async fn probe_links(&self, urls: Vec<String>) -> Vec<Result<net::Probe, NetError>> {
        let mut results: Vec<Option<Result<net::Probe, NetError>>> =
            urls.iter().map(|_| None).collect();
        let mut pending = urls.into_iter().enumerate();
        let mut running = tokio::task::JoinSet::new();
        loop {
            while running.len() < LINK_PROBES {
                let Some((i, url)) = pending.next() else {
                    break;
                };
                let client = self.client.clone();
                running.spawn(async move { (i, client.probe(&url, &[]).await) });
            }
            let Some(done) = running.join_next().await else {
                break;
            };
            if let Ok((i, result)) = done {
                results[i] = Some(result);
            }
        }
        results
            .into_iter()
            .map(|r| r.unwrap_or(Err(NetError::BadResponse("lecture interrompue"))))
            .collect()
    }

    pub async fn add_source(&self, task_id: &str, url: &str) -> Result<String, EngineError> {
        net::check_url(url)?;
        if self.public_only {
            check_public_literal(url)?;
        }
        let (id, address) = (task_id.to_owned(), url.to_owned());
        let check = self
            .ask(|reply| Command::CheckMirror {
                task_id: id,
                url: address,
                reply,
            })
            .await?;
        let facts = match check.total {
            None => None,
            Some(total) => {
                let headers = reveal(check.headers, &self.vault).await?;
                let reference_auth = reveal_site_login(check.reference_login, &self.vault).await?;
                let mirror_auth = reveal_site_login(check.mirror_login, &self.vault).await?;
                let mirror_headers = mirrors::headers_for(&headers, &check.reference.url, url);
                let if_range = net::headers::if_range_validator(
                    check.reference.etag.as_deref(),
                    check.reference.last_modified.as_deref(),
                );
                let reference = mirrors::Side {
                    url: &check.reference.url,
                    headers: &headers,
                    auth: reference_auth.as_ref(),
                    if_range: if_range.as_deref(),
                };
                let mirror = mirrors::Side {
                    url,
                    headers: &mirror_headers,
                    auth: mirror_auth.as_ref(),
                    if_range: None,
                };
                match mirrors::validate(&self.client, &reference, &mirror, total).await {
                    mirrors::Verdict::Accepted(facts) => Some(facts),
                    mirrors::Verdict::Refused(reason) => {
                        return Err(EngineError::MirrorRefused(reason));
                    }
                    mirrors::Verdict::Unreachable(e) => {
                        return Err(EngineError::SourceUnreachable(e));
                    }
                    // La référence ne répond pas comme prévu : le miroir sera validé à la
                    // prochaine exécution.
                    mirrors::Verdict::Reference(_) => None,
                }
            }
        };
        let (task_id, url) = (task_id.to_owned(), url.to_owned());
        self.ask(|reply| Command::AddMirror {
            task_id,
            url,
            facts,
            public_only: self.public_only,
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

    /// Identifiants de sites enregistrés, sans leurs mots de passe (SPEC §7.14).
    pub async fn logins(&self) -> Result<Vec<LoginView>, EngineError> {
        self.ask(Command::Logins).await
    }

    /// Enregistre ou modifie les identifiants d'un site ; le mot de passe va au coffre du
    /// système.
    pub async fn set_login(&self, change: LoginChange) -> Result<LoginView, EngineError> {
        self.ask(|reply| Command::SetLogin(change, reply)).await
    }

    /// Retire les identifiants d'un site, et leur mot de passe du coffre.
    pub async fn delete_login(&self, id: &str) -> Result<(), EngineError> {
        let id = id.to_owned();
        self.ask(|reply| Command::DeleteLogin(id, reply)).await
    }

    async fn token_op(
        &self,
        op: crate::remote::TokenOp,
    ) -> Result<crate::remote::TokenAnswer, EngineError> {
        self.ask(|reply| Command::Tokens(op, reply)).await
    }

    /// Crée un jeton de l'interface distante (SPEC §7.19) ; rend sa ligne et le jeton
    /// lui-même, qui ne sera plus jamais montré. Portée `read`, `add` ou `control`.
    pub async fn create_token(
        &self,
        name: &str,
        scope: &str,
    ) -> Result<(store::remote::TokenRow, String), EngineError> {
        if crate::remote::token_scope(scope).is_none() {
            return Err(EngineError::BadRequest(format!(
                "portée d'un jeton : read, add ou control, pas {scope}"
            )));
        }
        let secret = crate::remote::new_secret();
        let op = crate::remote::TokenOp::Create {
            name: name.to_owned(),
            scope: scope.to_owned(),
            digest: crate::remote::digest(&secret),
        };
        match self.token_op(op).await? {
            crate::remote::TokenAnswer::Token(row) => Ok((row, secret)),
            _ => Err(EngineError::Stopped),
        }
    }

    /// Les jetons de l'interface distante, sans leur valeur.
    pub async fn tokens(&self) -> Result<Vec<store::remote::TokenRow>, EngineError> {
        match self.token_op(crate::remote::TokenOp::List).await? {
            crate::remote::TokenAnswer::Tokens(rows) => Ok(rows),
            _ => Err(EngineError::Stopped),
        }
    }

    /// Révoque un jeton : ses sessions s'effacent, ses connexions se ferment. Faux s'il
    /// n'existait pas.
    pub async fn revoke_token(&self, id: &str) -> Result<bool, EngineError> {
        match self
            .token_op(crate::remote::TokenOp::Revoke(id.to_owned()))
            .await?
        {
            crate::remote::TokenAnswer::Done(done) => Ok(done),
            _ => Err(EngineError::Stopped),
        }
    }

    // ---------- Plugins d'hébergeurs (SPEC §7.20) ----------

    async fn plugin_op(&self, op: PluginOp) -> Result<PluginAnswer, EngineError> {
        self.ask(|reply| Command::Plugins(op, reply)).await
    }

    /// Lit et vérifie un fichier de plugin sans l'installer : manifeste, permissions et
    /// empreinte, à montrer à l'utilisateur avant son accord.
    pub async fn inspect_plugin(
        &self,
        bytes: Vec<u8>,
    ) -> Result<crate::plugins::Inspected, EngineError> {
        tokio::task::spawn_blocking(move || crate::plugins::inspect(&bytes))
            .await
            .map_err(|_| EngineError::Stopped)?
            .map_err(EngineError::BadRequest)
    }

    /// Installe un plugin dont l'utilisateur a accepté l'empreinte `accepted`, après avoir
    /// vu son manifeste et ses permissions ; remplace la version installée sous le même
    /// identifiant.
    pub async fn install_plugin(
        &self,
        bytes: Vec<u8>,
        accepted: &str,
    ) -> Result<store::plugins::PluginRow, EngineError> {
        let digest = crate::plugins::digest_of(&bytes);
        if !digest.eq_ignore_ascii_case(accepted.trim()) {
            return Err(EngineError::BadRequest(
                "l'empreinte acceptée n'est pas celle de ce fichier : le montrer de nouveau".into(),
            ));
        }
        let (inspected, bytes) =
            tokio::task::spawn_blocking(move || (crate::plugins::inspect(&bytes), bytes))
                .await
                .map_err(|_| EngineError::Stopped)?;
        let inspected = inspected.map_err(EngineError::BadRequest)?;
        let plugins = Arc::clone(&self.plugins);
        let file_digest = digest.clone();
        tokio::task::spawn_blocking(move || plugins.save(&file_digest, &bytes))
            .await
            .map_err(|_| EngineError::Stopped)?
            .map_err(|e| EngineError::Store(StoreError::Io(e)))?;
        let m = inspected.manifest;
        let row = store::plugins::PluginRow {
            id: m.id,
            name: m.name,
            version: m.version,
            author: m.author,
            description: m.description,
            digest,
            sites: m.sites,
            fetch: m.fetch,
            enabled: true,
            installed_at: 0,
        };
        match self.plugin_op(PluginOp::Install(Box::new(row))).await? {
            PluginAnswer::Installed(row, unused) => {
                if let Some(old) = unused {
                    self.plugins.delete_file(&old);
                }
                Ok(row)
            }
            _ => Err(EngineError::Stopped),
        }
    }

    /// Les plugins installés.
    pub async fn plugins(&self) -> Result<Vec<store::plugins::PluginRow>, EngineError> {
        match self.plugin_op(PluginOp::List).await? {
            PluginAnswer::Rows(rows) => Ok(rows),
            _ => Err(EngineError::Stopped),
        }
    }

    fn plugin_unknown(id: &str) -> EngineError {
        EngineError::PluginNotFound(format!("plugin inconnu : {id}"))
    }

    /// Retire un plugin, et son fichier.
    pub async fn remove_plugin(&self, id: &str) -> Result<(), EngineError> {
        match self.plugin_op(PluginOp::Remove(id.to_owned())).await? {
            PluginAnswer::Removed { found: false, .. } => Err(Self::plugin_unknown(id)),
            PluginAnswer::Removed { unused, .. } => {
                if let Some(digest) = unused {
                    self.plugins.delete_file(&digest);
                }
                Ok(())
            }
            _ => Err(EngineError::Stopped),
        }
    }

    /// Active ou désactive un plugin.
    pub async fn enable_plugin(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<store::plugins::PluginRow, EngineError> {
        match self
            .plugin_op(PluginOp::SetEnabled(id.to_owned(), enabled))
            .await?
        {
            PluginAnswer::Enabled(Some(row)) => Ok(row),
            PluginAnswer::Enabled(None) => Err(Self::plugin_unknown(id)),
            _ => Err(EngineError::Stopped),
        }
    }

    /// Le plugin activé qui prend en charge cette page, s'il y en a un.
    pub async fn plugin_for(
        &self,
        url: &str,
    ) -> Result<Option<store::plugins::PluginRow>, EngineError> {
        let rows = self.plugins().await?;
        Ok(crate::plugins::matching(&rows, url).cloned())
    }

    /// Lance le plugin de cette page ; la résolution est gardée 10 minutes pour
    /// [`Engine::add_from_plugin`].
    pub async fn resolve_with_plugin(
        &self,
        url: &str,
    ) -> Result<crate::plugins::Resolution, EngineError> {
        net::check_url(url)?;
        let row = self.plugin_for(url).await?.ok_or_else(|| {
            EngineError::PluginNotFound("aucun plugin activé ne prend en charge ce site".into())
        })?;
        let resolution = self
            .plugins
            .run(&row, url)
            .await
            .map_err(EngineError::PluginFailed)?;
        self.plugins.keep(resolution.clone());
        Ok(resolution)
    }

    /// La tâche d'un lien rendu par un plugin : réseau public seulement, `Referer` (celui
    /// du plugin, sinon la page) et cookies du site, page retenue pour un lien neuf.
    fn plugin_task(
        &self,
        resolution: &crate::plugins::Resolution,
        link: &crate::plugins::Link,
        mut task: AddTask,
    ) -> AddTask {
        let referer = link
            .referer
            .clone()
            .unwrap_or_else(|| resolution.page.clone());
        let mut headers = vec![("Referer".to_owned(), referer)];
        if let Some(cookie) = resolution.cookie_for(link) {
            headers.push(("Cookie".to_owned(), cookie));
        }
        task.url = link.url.clone();
        task.file_name = task.file_name.or_else(|| link.name.clone());
        task.headers = headers;
        task.public_only = !self.plugins.private_ok();
        task.torrent = None;
        task.media = None;
        task.mirrors = Vec::new();
        task.plugin = Some(PluginOrigin {
            page: resolution.page.clone(),
            plugin_id: resolution.plugin_id.clone(),
        });
        task
    }

    /// Ajoute les liens choisis (`picked`, rangs dans la résolution) d'une résolution
    /// gardée : une tâche par lien, sur le modèle de `base` (dossier, file, pause…).
    /// Chaque ajout réussit ou échoue pour lui-même.
    pub async fn add_from_plugin(
        &self,
        resolution_id: &str,
        picked: &[usize],
        base: AddTask,
    ) -> Result<Vec<Result<String, EngineError>>, EngineError> {
        let resolution = self.plugins.kept(resolution_id).ok_or_else(|| {
            EngineError::PluginNotFound(
                "résolution inconnue ou expirée (10 minutes) : relancer plugin.resolve".into(),
            )
        })?;
        if picked.is_empty()
            || picked.iter().any(|&i| i >= resolution.links.len())
            || (1..picked.len()).any(|i| picked[..i].contains(&picked[i]))
        {
            return Err(EngineError::BadRequest(format!(
                "liens à ajouter : rangs distincts de 0 à {}",
                resolution.links.len().saturating_sub(1)
            )));
        }
        let mut added = Vec::with_capacity(picked.len());
        for &i in picked {
            let task = self.plugin_task(&resolution, &resolution.links[i], base.clone());
            added.push(self.add(task).await);
        }
        Ok(added)
    }

    /// Impose l'adresse d'un nom aux requêtes des plugins (tests : serveur local).
    #[cfg(any(test, feature = "testing"))]
    pub fn plugins_resolve_to(&self, name: &str, addr: std::net::SocketAddr) {
        self.plugins.resolve_to(name, addr);
    }

    /// Les liens des plugins joignent aussi le réseau interne (tests : serveur local).
    #[cfg(any(test, feature = "testing"))]
    pub fn plugins_allow_private(&self) {
        self.plugins.allow_private();
    }

    /// Si `task` est l'adresse d'une page qu'un plugin activé prend en charge (adresse
    /// seule : ni en-têtes, ni miroirs, ni torrent, ni vidéo, ni `plugin: false`), la tâche
    /// de son lien direct, s'il est seul (SPEC §7.20).
    async fn through_plugin(&self, task: &AddTask) -> Result<Option<AddTask>, EngineError> {
        if task.no_plugin
            || task.plugin.is_some()
            || !task.headers.is_empty()
            || !task.mirrors.is_empty()
            || task.torrent.is_some()
            || task.media.is_some()
        {
            return Ok(None);
        }
        let Some(row) = self.plugin_for(&task.url).await? else {
            return Ok(None);
        };
        let resolution = self
            .plugins
            .run(&row, &task.url)
            .await
            .map_err(EngineError::PluginFailed)?;
        match resolution.links.len() {
            0 => Err(EngineError::PluginFailed(
                "aucun lien trouvé sur cette page".into(),
            )),
            1 => Ok(Some(self.plugin_task(
                &resolution,
                &resolution.links[0],
                task.clone(),
            ))),
            n => Err(EngineError::PluginChoice(n)),
        }
    }

    /// Page et plugin d'origine d'une tâche, si son lien vient d'un plugin.
    pub async fn plugin_origin(
        &self,
        task_id: &str,
    ) -> Result<Option<(String, String)>, EngineError> {
        let id = task_id.to_owned();
        self.ask(|reply| Command::PluginOrigin(id, reply)).await
    }

    /// Lien neuf d'une tâche venue d'un plugin, en échec sur un lien expiré (`403`, `410`,
    /// SPEC §7.20) : le plugin relit la page, le lien de même nom (ou le seul) remplace
    /// l'ancien, avec le `Referer` et les cookies de cette lecture, puis la tâche reprend.
    /// Faux si la tâche n'est pas dans ce cas.
    pub async fn refresh_plugin_link(&self, task_id: &str) -> Result<bool, EngineError> {
        let task = self.get(task_id).await?;
        let expired = task.state == TaskState::Failed
            && task.error_code.as_deref() == Some("HTTP_STATUS")
            && task
                .error_detail
                .as_deref()
                .and_then(net::status_in_detail)
                .is_some_and(|status| status == 403 || status == 410);
        if !expired {
            return Ok(false);
        }
        let Some((page, plugin_id)) = self.plugin_origin(task_id).await? else {
            return Ok(false);
        };
        let row = self
            .plugins()
            .await?
            .into_iter()
            .find(|r| r.id == plugin_id && r.enabled)
            .ok_or_else(|| {
                EngineError::PluginNotFound(format!("plugin {plugin_id} absent ou désactivé"))
            })?;
        let resolution = self
            .plugins
            .run(&row, &page)
            .await
            .map_err(EngineError::PluginFailed)?;
        let link = resolution
            .links
            .iter()
            .find(|l| l.name.as_deref() == Some(task.display_name.as_str()))
            .or_else(|| (resolution.links.len() == 1).then(|| &resolution.links[0]))
            .ok_or_else(|| {
                EngineError::PluginFailed("aucun lien de même nom sur la page".into())
            })?;
        let fresh = self.plugin_task(&resolution, link, AddTask::default());
        // Les en-têtes du lien neuf d'abord : sa vérification peut demander ses cookies.
        let (id, headers) = (task_id.to_owned(), fresh.headers.clone());
        self.ask(|reply| Command::ReplaceHeaders(id, headers, reply))
            .await?;
        // Le lien d'un plugin ne se vérifie que sur le réseau public, comme sa tâche.
        let checker = match fresh.public_only {
            true => self.restricted(false)?,
            false => self.clone(),
        };
        if let Err(e) = checker.set_url(task_id, &fresh.url).await {
            // Lien neuf refusé : la tâche garde son ancienne adresse, qui ne doit pas
            // recevoir les cookies du lien neuf (revue de sécurité, étape 5). Seuls les
            // en-têtes en clair restent.
            let plain: Vec<(String, String)> = fresh
                .headers
                .into_iter()
                .filter(|(name, _)| !net::is_secret_header(name))
                .collect();
            let id = task_id.to_owned();
            self.ask(|reply| Command::ReplaceHeaders(id, plain, reply))
                .await?;
            return Err(e);
        }
        self.resume(task_id).await?;
        Ok(true)
    }

    /// Une poignée faible : elle n'empêche pas le moteur de s'arrêter quand ses poignées
    /// disparaissent (superviseurs internes).
    pub(crate) fn downgrade(&self) -> WeakEngine {
        let (inert, _) = mpsc::channel(1);
        let (events, _) = broadcast::channel(1);
        WeakEngine {
            commands: self.commands.downgrade(),
            rest: Engine {
                commands: inert,
                events,
                ..self.clone()
            },
        }
    }

    /// Le jeton présenté par une connexion, s'il existe.
    pub async fn authenticate(
        &self,
        secret: &str,
    ) -> Result<Option<store::remote::TokenRow>, EngineError> {
        if !crate::remote::looks_like_secret(secret) {
            return Ok(None);
        }
        let op = crate::remote::TokenOp::UseToken(crate::remote::digest(secret));
        match self.token_op(op).await? {
            crate::remote::TokenAnswer::Found(found) => Ok(found),
            _ => Err(EngineError::Stopped),
        }
    }

    /// Ouvre une session de navigateur pour ce jeton ; rend la valeur du cookie.
    pub async fn open_session(&self, token_id: &str) -> Result<String, EngineError> {
        let secret = crate::remote::new_secret();
        let op = crate::remote::TokenOp::OpenSession {
            digest: crate::remote::digest(&secret),
            token_id: token_id.to_owned(),
            expires_at: store::now_ms() + crate::remote::SESSION_TTL_MS,
        };
        self.token_op(op).await?;
        Ok(secret)
    }

    /// Le jeton d'une session encore valable.
    pub async fn session(
        &self,
        cookie: &str,
    ) -> Result<Option<store::remote::TokenRow>, EngineError> {
        if !crate::remote::looks_like_secret(cookie) {
            return Ok(None);
        }
        let op = crate::remote::TokenOp::UseSession(crate::remote::digest(cookie));
        match self.token_op(op).await? {
            crate::remote::TokenAnswer::Found(found) => Ok(found),
            _ => Err(EngineError::Stopped),
        }
    }

    /// Ferme une session (`POST /logout`).
    pub async fn close_session(&self, cookie: &str) -> Result<(), EngineError> {
        if crate::remote::looks_like_secret(cookie) {
            let op = crate::remote::TokenOp::CloseSession(crate::remote::digest(cookie));
            self.token_op(op).await?;
        }
        Ok(())
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
    /// Finir le partage d'un torrent (pause pendant `seeding`) : la tâche est terminée.
    Complete,
}

struct Running {
    queue_id: String,
    stop: watch::Sender<bool>,
    then: Then,
    /// Segments en cours de réception, publiés par l'exécution à chaque avancement.
    live: watch::Receiver<Arc<[LiveSegment]>>,
    /// Dernier état annoncé de l'exécution : `active`, ou `verifying` pendant le calcul de
    /// l'empreinte (F-11). Le changement d'état de sa fin part de lui.
    shown: TaskState,
    /// Torrent, lancé à cette génération de l'interface (`Scheduler::iface_generation`) ;
    /// `None` : téléchargement HTTP.
    torrent: Option<u64>,
}

/// Ce qu'une exécution signale à la boucle, en plus de son issue.
#[derive(Debug)]
enum RunNote {
    /// Préparation enregistrée (premier avancement).
    Prepared(String),
    /// Passage à `verifying` enregistré (F-11).
    Verifying(String),
    /// Passage d'un torrent à `seeding` enregistré (SPEC §7.16).
    Seeding(String),
    /// Passage d'une vidéo à `post_processing` (fusion) enregistré (SPEC §7.17).
    PostProcessing(String),
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
    prepared: mpsc::UnboundedSender<RunNote>,
    /// Plages horaires et action de fin (F-16).
    planning: planning::State,
    /// Les copies vers un autre volume (SPEC §7.12) y signalent leur fin.
    moved: mpsc::UnboundedSender<(String, std::io::Result<()>)>,
    moved_rx: Option<mpsc::UnboundedReceiver<(String, std::io::Result<()>)>>,
    /// Le réseau convient aux téléchargements (SPEC §7.15) : faux, aucune tâche ne démarre
    /// et celles en cours retournent en file. Dit par l'application (Android : réseau
    /// perdu, ou facturé avec « Wi-Fi seulement ») ; vrai au démarrage.
    network_usable: bool,
    /// Réseau facturé au volume (SPEC §7.16) : les torrents ne partagent pas. Dit par
    /// l'application Android ; faux au démarrage.
    network_metered: bool,
    /// Session des torrents (SPEC §7.16), créée au premier torrent à démarrer.
    torrents: Torrents,
    /// Interface réseau des torrents (réglage `torrent.interface`, SPEC §7.16) : son nom,
    /// ce qu'elle était à la dernière vérification (`None` : absente), et sa génération,
    /// augmentée à chaque changement. Un torrent ne part pas tant qu'un torrent lancé à une
    /// génération précédente tourne encore : la session est liée à l'ancienne interface.
    iface_name: Option<String>,
    iface_binding: Option<torrent::Binding>,
    iface_generation: u64,
    next_iface_check: Instant,
    /// Outils de la vidéo (SPEC §7.17) ; `None` hors de Windows et Linux.
    tools: Option<Arc<media::tools::Tools>>,
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
        prepared: mpsc::UnboundedSender<RunNote>,
    ) -> Self {
        let (moved, moved_rx) = mpsc::unbounded_channel();
        let network_usable = config.planning.network_usable;
        // La table du DHT se garde à côté de la base.
        let state_dir = config
            .db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let torrents = Torrents::new(config.transfer.torrent.clone(), state_dir);
        // Limite d'envoi (torrents) : lue ici ; une base illisible la laisse ouverte.
        let upload_limit = settings::upload_limit(&conn).ok().flatten();
        torrents.set_limits(download_limit, upload_limit);
        // Outils de la vidéo : `tools/` à côté de la base, sauf réglage contraire.
        let tools = media::SUPPORTED.then(|| {
            let dir = config.transfer.media.dir.clone().unwrap_or_else(|| {
                config
                    .db_path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default()
                    .join("tools")
            });
            Arc::new(media::tools::Tools::open(
                dir,
                config.transfer.media.sources.clone(),
                config.client.clone(),
            ))
        });
        let iface_name = settings::torrent_interface(&conn).ok().flatten();
        let iface_binding = iface_name.as_deref().and_then(|n| torrents.resolve(n));
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
            moved,
            moved_rx: Some(moved_rx),
            network_usable,
            network_metered: false,
            torrents,
            iface_name,
            iface_binding,
            iface_generation: 0,
            next_iface_check: Instant::now() + INTERFACE_CHECK,
            tools,
        }
    }

    async fn run(
        mut self,
        mut inbox: mpsc::Receiver<Command>,
        mut prepared: mpsc::UnboundedReceiver<RunNote>,
    ) {
        let mut moved = self
            .moved_rx
            .take()
            .expect("la boucle ne tourne qu'une fois");
        loop {
            if self.shutdown.is_none() {
                self.recheck_disk();
                self.check_interface();
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
                Some(note) = prepared.recv() => self.noted(note),
                Some((task_id, result)) = moved.recv() => self.moved(&task_id, result),
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
        // L'interface des torrents se surveille tant qu'il y en a une.
        let cap = match self.iface_name {
            Some(_) => cap.min(self.next_iface_check.saturating_duration_since(now)),
            None => cap,
        };
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

    fn noted(&mut self, note: RunNote) {
        match note {
            RunNote::Prepared(task_id) => self.announce_prepared(&task_id),
            // Arrivé après la fin de l'exécution : son issue, déjà annoncée depuis
            // `active`, suffit.
            RunNote::Verifying(task_id) => {
                let Some(run) = self.running.get_mut(&task_id) else {
                    return;
                };
                if run.shown == TaskState::Active {
                    run.shown = TaskState::Verifying;
                    self.changed(&task_id, TaskState::Active, TaskState::Verifying);
                }
            }
            RunNote::Seeding(task_id) => {
                let Some(run) = self.running.get_mut(&task_id) else {
                    return;
                };
                // Après la commande de fin d'un torrent aussi (SPEC §7.21).
                if matches!(run.shown, TaskState::Active | TaskState::PostProcessing) {
                    let from = std::mem::replace(&mut run.shown, TaskState::Seeding);
                    self.changed(&task_id, from, TaskState::Seeding);
                }
            }
            // Fusion d'une vidéo, ou commande de fin (SPEC §7.21) : depuis `active`, ou
            // `verifying` après une empreinte juste.
            RunNote::PostProcessing(task_id) => {
                let Some(run) = self.running.get_mut(&task_id) else {
                    return;
                };
                if matches!(run.shown, TaskState::Active | TaskState::Verifying) {
                    let from = std::mem::replace(&mut run.shown, TaskState::PostProcessing);
                    self.changed(&task_id, from, TaskState::PostProcessing);
                }
            }
        }
    }

    /// Annonce le dossier, le nom et la taille qu'une exécution vient d'enregistrer. Peut
    /// arriver après la fin de l'exécution : l'annonce reste due, le nom servant à ouvrir
    /// le fichier.
    fn announce_prepared(&self, task_id: &str) {
        let Ok(task) = load_task(&self.conn, task_id) else {
            return;
        };
        let found_checksum = task.checksum.zip(task.checksum_source);
        self.emit(|rev| Event::Prepared {
            rev,
            task_id: task.id,
            dest_dir: task.dest_dir,
            file_name: task.file_name,
            display_name: task.display_name,
            total_bytes: task.total_bytes,
            found_checksum,
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
        // Un partage de torrent ne prend pas de place dans sa file (SPEC §7.16).
        for r in self
            .running
            .values()
            .filter(|r| r.shown != TaskState::Seeding)
        {
            *busy.entry(r.queue_id.clone()).or_default() += 1;
        }
        for c in candidates {
            if self.running.contains_key(&c.task_id) || self.broken.contains(&c.task_id) {
                continue;
            }
            // Torrent lié à une interface (VPN, SPEC §7.16) : il attend qu'elle soit là, et
            // que les torrents lancés sur la précédente aient quitté sa session. Un torrent
            // resté `active` (arrêt brutal) retourne en file pendant qu'elle manque.
            if c.kind == TaskKind::Torrent && !self.torrents_may_start() {
                if c.state == TaskState::Active && !self.interface_up() {
                    self.requeue_crashed(&c.task_id, PauseReason::Network);
                }
                continue;
            }
            // Partage interrompu : il reprend quand le réseau convient et n'est pas facturé
            // au volume, sans compter dans la file ni suivre ses plages horaires.
            if c.state == TaskState::Seeding {
                if self.network_usable && !self.network_metered {
                    self.launch(c.task_id, c.queue_id, c.state, c.kind);
                }
                continue;
            }
            // Réseau qui ne convient pas (SPEC §7.15) : rien ne démarre, sauf le calcul d'une
            // empreinte, qui n'en a pas besoin ; une tâche restée `active` (arrêt brutal)
            // retourne en file.
            if !self.network_usable && c.state != TaskState::Verifying {
                if c.state == TaskState::Active {
                    self.requeue_crashed(&c.task_id, PauseReason::Network);
                }
                continue;
            }
            // Hors de ses plages horaires, une file attend (F-16). Une tâche restée
            // `active` ou `verifying` (arrêt brutal du moteur) y retourne en file, comme à
            // la fermeture.
            if self.planning.closed.contains(&c.queue_id) {
                if matches!(c.state, TaskState::Active | TaskState::Verifying) {
                    self.requeue_crashed(&c.task_id, PauseReason::Schedule);
                }
                continue;
            }
            let used = busy.entry(c.queue_id.clone()).or_default();
            if *used >= c.max_active {
                continue;
            }
            *used += 1;
            self.launch(c.task_id, c.queue_id, c.state, c.kind);
        }
    }

    fn launch(&mut self, task_id: String, queue_id: String, from: TaskState, kind: TaskKind) {
        // L'état `active` est enregistré ici, avant d'être annoncé : un instantané pris à
        // la révision de l'annonce le montre déjà (SPEC §6.10). `transfer::run` accepte
        // une tâche `active`, ou `verifying` (arrêt brutal pendant le calcul de
        // l'empreinte), qui reste dans son état.
        let resumed = matches!(
            from,
            TaskState::Active
                | TaskState::Verifying
                | TaskState::Seeding
                // Arrêt pendant la fusion d'une vidéo : elle recommence (SPEC §7.17).
                | TaskState::PostProcessing
        );
        if !resumed
            && change_state(&mut self.conn, &task_id, &Change::to(TaskState::Active)).is_err()
        {
            self.broken.insert(task_id);
            return;
        }
        let shown = match from {
            TaskState::Verifying => TaskState::Verifying,
            TaskState::Seeding => TaskState::Seeding,
            TaskState::PostProcessing => TaskState::PostProcessing,
            _ => TaskState::Active,
        };
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
                shown,
                torrent: (kind == TaskKind::Torrent).then_some(self.iface_generation),
            },
        );
        self.changed(&task_id, from, shown);
        let mut config = self.config.clone();
        config.transfer.throttle = throttle;
        // Le réglage, dans la limite de la configuration (les bancs peuvent l'interdire).
        config.transfer.preallocate &=
            settings::flag(&self.conn, Flag::Preallocate).unwrap_or(true);
        config.transfer.neighbor_checksum &=
            settings::flag(&self.conn, Flag::NeighborChecksum).unwrap_or(true);
        let events = self.events.clone();
        let rev = Arc::clone(&self.rev);
        let prepared = self.prepared.clone();
        let torrents = self.torrents.clone();
        let tools = self.tools.clone();
        let id = task_id.clone();
        let handle = self.runs.spawn(async move {
            let result = execute(
                &config,
                &torrents,
                tools.as_ref(),
                &id,
                stopped,
                events,
                rev,
                prepared,
                live_tx,
            )
            .await;
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
                let _ = reply.send(self.add(*task));
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
            Command::Restart {
                task_id,
                delete_files,
                reply,
            } => {
                let _ = reply.send(self.restart(&task_id, delete_files));
            }
            Command::Relocate {
                task_id,
                dest_dir,
                file_name,
                remote,
                reply,
            } => {
                let _ = reply.send(self.relocate(&task_id, dest_dir, file_name, remote));
            }
            Command::CheckSource {
                task_id,
                url,
                reply,
            } => {
                let _ = reply.send(self.check_source(&task_id, &url));
            }
            Command::ReplaceUrl {
                task_id,
                url,
                facts,
                public_only,
                reply,
            } => {
                let _ = reply.send(self.replace_url(&task_id, &url, facts, public_only));
            }
            Command::CheckMirror {
                task_id,
                url,
                reply,
            } => {
                let _ = reply.send(self.check_mirror(&task_id, &url));
            }
            Command::AddMirror {
                task_id,
                url,
                facts,
                public_only,
                reply,
            } => {
                let _ = reply.send(self.add_mirror(&task_id, &url, facts, public_only));
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
            Command::Logins(reply) => {
                let rows = store::logins::list(&self.conn);
                let _ = reply.send(
                    rows.map(|r| r.into_iter().map(LoginView::from).collect())
                        .map_err(Into::into),
                );
            }
            Command::SetLogin(change, reply) => {
                let _ = reply.send(self.set_login(change));
            }
            Command::DeleteLogin(id, reply) => {
                let _ = reply.send(self.delete_login(&id));
            }
            Command::Tokens(op, reply) => {
                let _ = reply.send(self.tokens(op));
            }
            Command::Plugins(op, reply) => {
                let _ = reply.send(self.plugin_op(op));
            }
            Command::PluginOrigin(id, reply) => {
                let _ = reply.send(tasks::plugin_origin(&self.conn, &id).map_err(Into::into));
            }
            Command::ReplaceHeaders(id, headers, reply) => {
                let _ = reply.send(self.replace_headers(&id, headers));
            }
            Command::CollectSecrets(found) => self.collect_secrets(found),
            Command::List(filter, reply) => {
                let rows = tasks::find_tasks(&self.conn, &filter);
                let _ = reply.send(rows.map(|r| (r, self.current_rev())).map_err(Into::into));
            }
            Command::History(filter, reply) => {
                let _ = reply.send(history::list(&self.conn, &filter).map_err(Into::into));
            }
            Command::ClearHistory(before, reply) => {
                let _ = reply.send(history::clear(&self.conn, before).map_err(Into::into));
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
            Command::Flag(flag, reply) => {
                let _ = reply.send(settings::flag(&self.conn, flag).map_err(Into::into));
            }
            Command::SetNetwork(usable, metered, reply) => {
                self.set_network(usable, metered);
                let _ = reply.send(Ok(()));
            }
            Command::UploadLimit(reply) => {
                let _ = reply.send(settings::upload_limit(&self.conn).map_err(Into::into));
            }
            Command::SetUploadLimit(limit, reply) => {
                let _ = reply.send(self.set_upload_limit(limit));
            }
            Command::TorrentRatio(reply) => {
                let _ = reply.send(settings::torrent_ratio(&self.conn).map_err(Into::into));
            }
            Command::SetTorrentRatio(ratio, reply) => {
                let _ = reply.send(self.set_torrent_ratio(ratio));
            }
            Command::ToolsChanged(status) => {
                self.emit(|rev| Event::ToolsChanged { rev, status });
            }
            Command::Warn(code, message) => {
                self.emit(|rev| Event::Warning {
                    rev,
                    code,
                    message,
                    task_id: None,
                });
            }
            Command::TorrentInterface(reply) => {
                let _ = reply.send(Ok(self.iface_name.clone()));
            }
            Command::SetTorrentInterface(name, reply) => {
                let _ = reply.send(self.set_torrent_interface(name));
            }
            Command::Proxy(reply) => {
                let _ = reply.send(self.proxy_view());
            }
            Command::SetProxy(change, reply) => {
                let _ = reply.send(self.set_proxy(change));
            }
            Command::Rules(reply) => {
                let _ = reply.send(store::rules::list(&self.conn).map_err(Into::into));
            }
            Command::SetRules(rules, reply) => {
                let _ = reply
                    .send(store::rules::replace_all(&mut self.conn, &rules).map_err(Into::into));
            }
            Command::LogsLevel(reply) => {
                let _ = reply.send(settings::logs_level(&self.conn).map_err(Into::into));
            }
            Command::SetLogsLevel(level, reply) => {
                let _ = reply.send(self.set_logs_level(level));
            }
            Command::RemoteListen(reply) => {
                let _ = reply.send(settings::remote_listen(&self.conn).map_err(Into::into));
            }
            Command::SetRemoteListen(addr, reply) => {
                let _ = reply.send(self.set_remote_listen(addr));
            }
            Command::RemoteConfig(reply) => {
                let _ = reply.send(settings::remote_config(&self.conn).map_err(Into::into));
            }
            Command::SetRemoteHosts(hosts, reply) => {
                let _ = reply.send(self.set_remote_hosts(hosts));
            }
            Command::SetRemoteTls(files, reply) => {
                let _ = reply.send(self.set_remote_tls(files));
            }
            Command::SetFiles(task_id, selected, reply) => {
                let _ = reply.send(self.set_files(&task_id, &selected));
            }
            Command::SetFlag(flag, on, reply) => {
                let result = settings::set_flag(&self.conn, flag, on);
                if result.is_ok() {
                    self.emit(|rev| Event::FlagChanged { rev, flag, on });
                }
                let _ = reply.send(result.map_err(Into::into));
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
            upload_limit: settings::upload_limit(&self.conn)?,
            download_dir: self.download_dir()?,
            flags: Flag::ALL
                .into_iter()
                .map(|f| Ok((f, settings::flag(&self.conn, f)?)))
                .collect::<Result<_, StoreError>>()?,
            torrent_ratio: ratio_json(settings::torrent_ratio(&self.conn)?),
            network_usable: self.network_usable,
            network_metered: self.network_metered,
            torrent_interface: self.iface_name.clone(),
            torrent_interface_up: self.interface_up(),
            logs_level: settings::logs_level(&self.conn)?,
        })
    }

    /// Dossier des ajouts sans dossier : le réglage, sinon le dossier « Téléchargements »
    /// du système.
    fn download_dir(&self) -> Result<Option<String>, EngineError> {
        Ok(settings::download_dir(&self.conn)?
            .or_else(|| dirs::download_dir().and_then(|d| d.to_str().map(str::to_owned))))
    }

    /// Une connexion à jeton n'écrit que sous le dossier des téléchargements (SPEC §7.19,
    /// décision du 2026-11-11) : chemins résolus (liens suivis), comparés composant par
    /// composant.
    fn check_within_downloads(&self, dir: &str) -> Result<(), EngineError> {
        let refused = || EngineError::DestForbidden;
        let base = self.download_dir()?.ok_or_else(refused)?;
        let base = std::fs::canonicalize(base).map_err(|_| refused())?;
        let target = std::fs::canonicalize(dir).map_err(|_| refused())?;
        if target.starts_with(&base) {
            Ok(())
        } else {
            Err(refused())
        }
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
        self.limits_changed()
    }

    /// Limite d'envoi globale (torrents, SPEC §7.16).
    fn set_upload_limit(&mut self, limit: Option<u64>) -> Result<(), EngineError> {
        Self::check_speed_limit(limit)?;
        settings::set_upload_limit(&self.conn, limit)?;
        self.limits_changed()
    }

    /// Applique les limites globales aux torrents et les annonce.
    fn limits_changed(&mut self) -> Result<(), EngineError> {
        let download_bps = settings::download_limit(&self.conn)?;
        let upload_bps = settings::upload_limit(&self.conn)?;
        self.torrents.set_limits(download_bps, upload_bps);
        self.emit(|rev| Event::LimitsChanged {
            rev,
            download_bps,
            upload_bps,
        });
        Ok(())
    }

    /// Ratio de partage des torrents (SPEC §7.16) : un nombre réel positif ou nul, ou
    /// `None` pour sans limite.
    fn set_torrent_ratio(&mut self, ratio: Option<f64>) -> Result<(), EngineError> {
        if ratio.is_some_and(|r| !r.is_finite() || r < 0.0) {
            return Err(EngineError::BadRequest(
                "torrent.ratioLimit : un nombre positif ou nul, ou null".into(),
            ));
        }
        settings::set_torrent_ratio(&self.conn, ratio)?;
        self.emit(|rev| Event::SettingChanged {
            rev,
            key: settings::TORRENT_RATIO,
            value: ratio_json(ratio),
        });
        Ok(())
    }

    /// Fichiers choisis d'un torrent (`task.setFiles`, SPEC §7.16) : avant la fin du
    /// téléchargement seulement ; une exécution en cours suit le nouveau choix.
    fn set_files(&mut self, task_id: &str, selected: &[u32]) -> Result<TaskRow, EngineError> {
        let task = load_task(&self.conn, task_id)?;
        if task.kind != TaskKind::Torrent {
            return Err(EngineError::BadRequest(
                "task.setFiles : seulement pour un torrent".into(),
            ));
        }
        if !matches!(
            task.state,
            TaskState::Queued
                | TaskState::Active
                | TaskState::Paused
                | TaskState::RetryWait
                | TaskState::Failed
        ) {
            return Err(Self::invalid(&task, "choix des fichiers"));
        }
        if task.file_name.is_none() {
            return Err(EngineError::BadRequest(
                "task.setFiles : fichiers pas encore connus (métadonnées attendues)".into(),
            ));
        }
        store::torrents::set_selected(&mut self.conn, task_id, selected).map_err(|e| match e {
            StoreError::BadValue(m) => EngineError::BadRequest(format!("task.setFiles : {m}")),
            e => e.into(),
        })?;
        self.announce_prepared(task_id);
        Ok(load_task(&self.conn, task_id)?)
    }

    fn update_task(&mut self, task_id: &str, update: TaskUpdate) -> Result<TaskRow, EngineError> {
        // Tout est vérifié avant de changer quoi que ce soit.
        if let Some(limit) = update.speed_limit_bps {
            Self::check_speed_limit(limit)?;
        }
        if let Some(checksum) = &update.checksum {
            // L'empreinte se lit à la fin du téléchargement, en cours compris ; pas pendant
            // son calcul, ni sur un fichier déjà terminé.
            let task = load_task(&self.conn, task_id)?;
            // Un torrent porte les empreintes de chacun de ses morceaux (SPEC §7.16).
            if task.kind == TaskKind::Torrent {
                return Err(EngineError::BadRequest(
                    "empreinte : un torrent vérifie déjà chacun de ses morceaux".into(),
                ));
            }
            // Une vidéo est assemblée par ffmpeg et contrôlée par ffprobe (SPEC §7.17) :
            // aucune empreinte publiée ne peut lui correspondre.
            if task.kind == TaskKind::Media {
                return Err(EngineError::BadRequest(
                    "empreinte : une vidéo est assemblée sur place, sans empreinte publiée".into(),
                ));
            }
            if matches!(
                task.state,
                TaskState::Verifying | TaskState::Completed | TaskState::Cancelled
            ) {
                return Err(Self::invalid(&task, "changement d'empreinte"));
            }
            tasks::set_checksum(&self.conn, task_id, checksum.as_ref(), None)?;
        }
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
        // Sans dossier choisi : les règles pourront la ranger à sa préparation
        // (SPEC §7.21). Elle ne part qu'à un prochain passage de la boucle, après.
        let ruled = task.dest_dir.is_none() && task.sub_dir.is_empty();
        let task_id = self.add_new(task)?;
        if ruled {
            store::rules::set_rule_base(&self.conn, &task_id)?;
        }
        Ok(task_id)
    }

    fn add_new(&mut self, task: AddTask) -> Result<String, EngineError> {
        let mut task = task;
        if !task.sub_dir.is_empty() {
            task.dest_dir = Some(self.resolve_sub_dir(&task)?);
            task.sub_dir.clear();
        }
        if task.torrent.is_some() || is_magnet(&task.url) {
            if !task.mirrors.is_empty() {
                return Err(EngineError::BadRequest(
                    "un torrent n'a qu'une source : pas de miroir".into(),
                ));
            }
            return self.add_torrent(task);
        }
        if task.media.is_some() {
            if !task.mirrors.is_empty() {
                return Err(EngineError::BadRequest(
                    "une vidéo n'a qu'une source : pas de miroir".into(),
                ));
            }
            return self.add_media(task);
        }
        net::check_url(&task.url)?;
        // Miroirs (SPEC §7.7) : des adresses HTTP(S), toutes différentes.
        for mirror in &task.mirrors {
            net::check_url(mirror)?;
        }
        if task.public_only {
            for url in std::iter::once(&task.url).chain(&task.mirrors) {
                check_public_literal(url)?;
            }
        }
        let mut all: Vec<&str> = std::iter::once(task.url.as_str())
            .chain(task.mirrors.iter().map(String::as_str))
            .collect();
        all.sort_unstable();
        if all.windows(2).any(|w| w[0] == w[1]) {
            return Err(EngineError::BadRequest(
                "la même adresse deux fois parmi les sources".into(),
            ));
        }
        if all.len() > store::sources::MAX_SOURCES {
            return Err(EngineError::BadRequest(format!(
                "{} sources au plus par tâche",
                store::sources::MAX_SOURCES
            )));
        }
        net::check_task_headers(&task.headers)
            .map_err(|e| EngineError::BadRequest(e.to_string()))?;
        Self::check_speed_limit(task.speed_limit_bps)?;
        if let Some(name) = &task.file_name {
            check_file_name(name)?;
        }
        let (dest_dir, queue_id) = self.destination(task.dest_dir, task.queue_id, task.remote)?;
        let display_name = task
            .file_name
            .clone()
            .unwrap_or_else(|| transfer::display_name_for(&task.url));
        // Les secrets d'abord, leurs références ensuite (SPEC §5.1) ; si la tâche n'est
        // pas créée, ils sont effacés.
        let stored = self.store_secrets(task.headers)?;
        let created = tasks::create_task_with_mirrors(
            &mut self.conn,
            &NewTask {
                queue_id: &queue_id,
                url: &task.url,
                display_name: &display_name,
                dest_dir: &dest_dir,
                max_connections: task.max_connections,
                speed_limit_bps: task.speed_limit_bps,
                idempotency_key: task.idempotency_key.as_deref(),
                requested_name: task.file_name.as_deref(),
                checksum: task.checksum.as_ref(),
                headers: &stored,
                parent_id: None,
                public_only: task.public_only,
            },
            &task.mirrors,
        );
        let (task_id, _) = match created {
            Ok(created) => created,
            Err(e) => {
                self.vault().forget(secret_ids(stored));
                return Err(e.into());
            }
        };
        if let Some(origin) = &task.plugin {
            tasks::set_plugin_origin(&self.conn, &task_id, &origin.page, &origin.plugin_id)?;
        }
        let id = task_id.clone();
        self.emit(|rev| Event::Added { rev, task_id: id });
        if task.start_paused {
            self.set_state(&task_id, &user_pause())?;
        }
        Ok(task_id)
    }

    /// Dossier et file d'un ajout : ceux demandés, sinon le dossier des téléchargements du
    /// moteur et la file par défaut.
    /// Dossier et file d'un ajout. `remote` : ajout par une connexion à jeton, qui
    /// n'écrit que sous le dossier des téléchargements (SPEC §7.19).
    fn destination(
        &self,
        dest_dir: Option<String>,
        queue_id: Option<String>,
        remote: bool,
    ) -> Result<(String, String), EngineError> {
        let dest_dir = match dest_dir {
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
        if remote {
            self.check_within_downloads(&dest_dir)?;
        }
        let queue_id = queue_id.unwrap_or_else(|| self.default_queue.clone());
        if !tasks::queue_exists(&self.conn, &queue_id)? {
            return Err(EngineError::QueueNotFound(queue_id));
        }
        Ok((dest_dir, queue_id))
    }

    /// Le sous-dossier d'un ajout (Metalink, `subDir` de la page distante), créé sous sa
    /// destination ; pour tout téléchargement (HTTP, torrent, vidéo). Les noms sont vérifiés
    /// tels quels, et pour une connexion à jeton le dossier obtenu doit rester sous celui
    /// des téléchargements (un sous-dossier déjà là pourrait être un lien vers ailleurs).
    fn resolve_sub_dir(&self, task: &AddTask) -> Result<String, EngineError> {
        for part in &task.sub_dir {
            if part == "." || part == ".." {
                return Err(EngineError::BadRequest(format!(
                    "sous-dossier refusé : {part:?}"
                )));
            }
            check_file_name(part)?;
        }
        let (base, _) =
            self.destination(task.dest_dir.clone(), task.queue_id.clone(), task.remote)?;
        let dir = sub_dir(&base, &task.sub_dir)?;
        if task.remote {
            self.check_within_downloads(&dir)?;
        }
        Ok(dir)
    }

    /// Ajout d'un torrent (SPEC §7.16) : lien magnet dans `url`, ou fichier `.torrent`.
    /// Ajoute une vidéo (SPEC §7.17) : les outils doivent être là ; tout est vérifié avant
    /// la création.
    fn add_media(&mut self, task: AddTask) -> Result<String, EngineError> {
        let request = task.media.clone().unwrap_or_default();
        let tools = self.tools.as_ref().ok_or_else(|| {
            EngineError::CapabilityMissing(
                "vidéo : seulement sous Windows et Linux en v1 (SPEC §7.17)".into(),
            )
        })?;
        if !tools.status().ready() {
            return Err(EngineError::ToolsMissing(
                "yt-dlp, ffmpeg ou ffprobe n'est pas installé (media.installTools)".into(),
            ));
        }
        net::check_url(&request.page_url)?;
        Self::check_speed_limit(task.speed_limit_bps)?;
        let format = request.format.as_deref().unwrap_or(DEFAULT_FORMAT);
        if format.trim().is_empty() || format.len() > 200 || format.chars().any(char::is_control) {
            return Err(EngineError::BadRequest(
                "media.format : un sélecteur de yt-dlp, 1 à 200 caractères".into(),
            ));
        }
        if let Some(c) = &request.container
            && !matches!(c.as_str(), "mp4" | "mkv" | "webm")
        {
            return Err(EngineError::BadRequest(
                "media.container : mp4, mkv ou webm".into(),
            ));
        }
        if request.subtitles.len() > 20
            || request.subtitles.iter().any(|l| {
                l.is_empty()
                    || l.len() > 20
                    || !l
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
        {
            return Err(EngineError::BadRequest(
                "media.subtitles : au plus 20 codes de langue".into(),
            ));
        }
        let (dest_dir, queue_id) = self.destination(task.dest_dir, task.queue_id, task.remote)?;
        let display_name = request
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.chars().take(200).collect::<String>())
            .unwrap_or_else(|| transfer::display_name_for(&request.page_url));
        let task_id = store::media::create_media_task(
            &mut self.conn,
            &store::media::NewMedia {
                queue_id: &queue_id,
                page_url: &request.page_url,
                display_name: &display_name,
                dest_dir: &dest_dir,
                speed_limit_bps: task.speed_limit_bps,
                idempotency_key: task.idempotency_key.as_deref(),
                format,
                container: request.container.as_deref(),
                subtitles: &request.subtitles,
                public_only: task.public_only,
            },
        )?;
        let id = task_id.clone();
        self.emit(|rev| Event::Added { rev, task_id: id });
        if task.start_paused {
            self.set_state(&task_id, &user_pause())?;
        }
        Ok(task_id)
    }

    fn add_torrent(&mut self, task: AddTask) -> Result<String, EngineError> {
        if !task.headers.is_empty() || task.checksum.is_some() || task.max_connections.is_some() {
            return Err(EngineError::BadRequest(
                "headers, checksum et maxConnections ne s'appliquent pas à un torrent".into(),
            ));
        }
        Self::check_speed_limit(task.speed_limit_bps)?;
        if let Some(name) = &task.file_name {
            check_file_name(name)?;
        }
        let added = match &task.torrent {
            Some(metainfo) => torrent::inspect_metainfo(metainfo),
            None => torrent::inspect_magnet(&task.url),
        }
        .map_err(EngineError::BadRequest)?;
        if let Some(other) = store::torrents::unfinished_with_hash(&self.conn, &added.info_hash)? {
            return Err(EngineError::BadRequest(format!(
                "ce torrent est déjà dans la liste (tâche {other})"
            )));
        }
        let (dest_dir, queue_id) = self.destination(task.dest_dir, task.queue_id, task.remote)?;
        let display_name = task.file_name.clone().unwrap_or(added.name);
        let task_id = store::torrents::create_torrent_task(
            &mut self.conn,
            &store::torrents::NewTorrent {
                queue_id: &queue_id,
                display_name: &display_name,
                dest_dir: &dest_dir,
                magnet: &added.magnet,
                info_hash: &added.info_hash,
                metainfo: task.torrent.as_deref(),
                speed_limit_bps: task.speed_limit_bps,
                idempotency_key: task.idempotency_key.as_deref(),
                requested_name: task.file_name.as_deref(),
            },
        )?;
        let id = task_id.clone();
        self.emit(|rev| Event::Added { rev, task_id: id });
        if task.start_paused {
            self.set_state(&task_id, &user_pause())?;
        }
        Ok(task_id)
    }

    fn vault(&self) -> &Vault {
        &self.config.transfer.vault
    }

    /// Range les en-têtes secrets dans le coffre ; les autres restent en clair. Si le
    /// coffre refuse, ceux déjà rangés sont effacés.
    fn store_secrets(
        &self,
        headers: Vec<(String, String)>,
    ) -> Result<Vec<StoredHeader>, EngineError> {
        let mut stored = Vec::with_capacity(headers.len());
        for (name, value) in headers {
            if !net::is_secret_header(&name) {
                stored.push((name, Stored::Plain(value)));
                continue;
            }
            match self.vault().put(&value) {
                Ok(id) => stored.push((name, Stored::Secret(id))),
                Err(e) => {
                    self.vault().forget(secret_ids(stored));
                    return Err(e.into());
                }
            }
        }
        Ok(stored)
    }

    /// Enregistre des identifiants (SPEC §7.14). Tout est vérifié avant le coffre ; le
    /// nouveau mot de passe est écrit avant sa référence, l'ancien effacé après (SPEC §5.1).
    fn set_login(&mut self, change: LoginChange) -> Result<LoginView, EngineError> {
        let site = crate::net::auth::Site::parse(&change.site)
            .map_err(EngineError::BadRequest)?
            .text();
        let username = change.username.trim();
        if username.is_empty()
            || username.len() > MAX_USERNAME
            || username.contains(':')
            || username.chars().any(char::is_control)
        {
            return Err(EngineError::BadRequest(format!(
                "nom d'utilisateur : de 1 à {MAX_USERNAME} octets, sans « : » ni caractère \
                 de contrôle"
            )));
        }
        if change
            .password
            .as_ref()
            .is_some_and(|p| p.is_empty() || p.chars().any(char::is_control))
        {
            return Err(EngineError::BadRequest(
                "mot de passe vide, ou avec un caractère de contrôle".into(),
            ));
        }
        let existing = match &change.id {
            Some(id) => Some(
                store::logins::load(&self.conn, id)?
                    .ok_or_else(|| EngineError::LoginNotFound(id.clone()))?,
            ),
            None => None,
        };
        if let Some(other) = store::logins::by_site(&self.conn, &site)?
            && existing.as_ref().is_none_or(|e| e.id != other.id)
        {
            return Err(EngineError::BadRequest(format!(
                "des identifiants sont déjà enregistrés pour {site} : les modifier"
            )));
        }
        if existing.is_none() && change.password.is_none() {
            return Err(EngineError::BadRequest(
                "mot de passe manquant pour de nouveaux identifiants".into(),
            ));
        }
        let secret = match &change.password {
            Some(password) => Some(self.vault().put(password)?),
            None => None,
        };
        let saved = match &existing {
            None => store::logins::insert(
                &self.conn,
                &site,
                username,
                secret.as_deref().expect("vérifié plus haut"),
            )
            .map(|id| (id, None)),
            Some(row) => {
                store::logins::update(&self.conn, &row.id, &site, username, secret.as_deref())
                    .map(|old| (row.id.clone(), old))
            }
        };
        let (id, replaced) = match saved {
            Ok(saved) => saved,
            Err(e) => {
                self.vault().forget(secret.into_iter().collect());
                return Err(e.into());
            }
        };
        self.vault().forget(replaced.into_iter().collect());
        Ok(LoginView {
            id,
            site,
            username: username.to_owned(),
        })
    }

    /// Remplace les en-têtes d'une tâche arrêtée (lien neuf d'un plugin, SPEC §7.20) : les
    /// nouveaux secrets au coffre d'abord, les anciens effacés après (SPEC §5.1).
    fn replace_headers(
        &mut self,
        task_id: &str,
        headers: Vec<(String, String)>,
    ) -> Result<(), EngineError> {
        net::check_task_headers(&headers).map_err(|e| EngineError::BadRequest(e.to_string()))?;
        let task = load_task(&self.conn, task_id)?;
        if self.running.contains_key(task_id) {
            return Err(EngineError::InvalidState {
                task_id: task_id.to_owned(),
                state: task.state.as_sql(),
                action: "remplacer ses en-têtes",
            });
        }
        let stored = self.store_secrets(headers)?;
        let new_ids: Vec<String> = stored
            .iter()
            .filter_map(|(_, s)| match s {
                Stored::Secret(id) => Some(id.clone()),
                Stored::Plain(_) => None,
            })
            .collect();
        match store::headers::replace(&mut self.conn, task_id, &stored) {
            Ok(old) => {
                self.vault().forget(old);
                Ok(())
            }
            Err(e) => {
                self.vault().forget(new_ids);
                Err(e.into())
            }
        }
    }

    /// Table des plugins (SPEC §7.20). Les empreintes rendues sont celles de fichiers que
    /// plus aucun plugin n'utilise.
    fn plugin_op(&mut self, op: PluginOp) -> Result<PluginAnswer, EngineError> {
        use store::plugins as rows;
        let unused =
            |conn: &Connection, digest: Option<String>| -> Result<Option<String>, EngineError> {
                Ok(match digest {
                    Some(d) if !rows::digest_in_use(conn, &d)? => Some(d),
                    _ => None,
                })
            };
        Ok(match op {
            PluginOp::List => PluginAnswer::Rows(rows::list(&self.conn)?),
            PluginOp::Install(row) => {
                let (saved, replaced) = rows::install(&self.conn, &row)?;
                PluginAnswer::Installed(saved, unused(&self.conn, replaced)?)
            }
            PluginOp::Remove(id) => {
                let digest = rows::remove(&self.conn, &id)?;
                PluginAnswer::Removed {
                    found: digest.is_some(),
                    unused: unused(&self.conn, digest)?,
                }
            }
            PluginOp::SetEnabled(id, on) => {
                rows::set_enabled(&self.conn, &id, on)?;
                PluginAnswer::Enabled(rows::load(&self.conn, &id)?)
            }
        })
    }

    /// Retire des identifiants : la référence d'abord, puis le mot de passe du coffre.
    fn tokens(
        &mut self,
        op: crate::remote::TokenOp,
    ) -> Result<crate::remote::TokenAnswer, EngineError> {
        use crate::remote::{TokenAnswer, TokenOp};
        use store::remote as rows;
        Ok(match op {
            TokenOp::Create {
                name,
                scope,
                digest,
            } => TokenAnswer::Token(rows::create_token(&self.conn, &name, &scope, &digest)?),
            TokenOp::List => TokenAnswer::Tokens(rows::list_tokens(&self.conn)?),
            TokenOp::Revoke(id) => {
                let done = rows::revoke_token(&self.conn, &id)?;
                if done {
                    self.emit(|rev| Event::TokenRevoked { rev, token_id: id });
                }
                TokenAnswer::Done(done)
            }
            TokenOp::UseToken(digest) => TokenAnswer::Found(rows::use_token(&self.conn, &digest)?),
            TokenOp::OpenSession {
                digest,
                token_id,
                expires_at,
            } => {
                rows::create_session(&self.conn, &digest, &token_id, expires_at)?;
                TokenAnswer::Done(true)
            }
            TokenOp::UseSession(digest) => {
                TokenAnswer::Found(rows::use_session(&self.conn, &digest)?)
            }
            TokenOp::CloseSession(digest) => {
                rows::delete_session(&self.conn, &digest)?;
                TokenAnswer::Done(true)
            }
        })
    }

    fn delete_login(&mut self, id: &str) -> Result<(), EngineError> {
        let secret = store::logins::delete(&self.conn, id)?
            .ok_or_else(|| EngineError::LoginNotFound(id.to_owned()))?;
        self.vault().forget(vec![secret]);
        Ok(())
    }

    /// Retire de la base les secrets d'une tâche terminée, puis les efface du coffre.
    fn drop_secrets(&mut self, task_id: &str) {
        if let Ok(ids) = headers::drop_secrets(&self.conn, task_id) {
            self.vault().forget(ids);
        }
    }

    /// Ramassage (voir `secrets`) : efface du coffre les secrets `found` dont la base n'a
    /// pas la référence. `found` a été relevé avant ce tour de boucle : un ajout qui a
    /// rangé un secret avant ce relevé a aussi enregistré sa référence, dans le même tour.
    fn collect_secrets(&mut self, found: Vec<String>) {
        let Ok(known) = headers::all_secret_refs(&self.conn) else {
            return;
        };
        let orphans: Vec<String> = found.into_iter().filter(|id| !known.contains(id)).collect();
        if !orphans.is_empty() {
            tracing::info!(
                secrets = orphans.len(),
                "secrets orphelins effacés du coffre"
            );
        }
        self.vault().forget(orphans);
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
            // Fusion ou commande de fin (SPEC §7.21) : le fichier est fait, elle va jusqu'au
            // bout (ou jusqu'à son délai).
            if run.shown == TaskState::PostProcessing {
                return Ok(());
            }
            // Pause d'un partage : il s'arrête, et la tâche est terminée (SPEC §7.16).
            run.then = if run.shown == TaskState::Seeding {
                Then::Complete
            } else {
                Then::Keep
            };
            let _ = run.stop.send(true);
            return Ok(());
        }
        let task = load_task(&self.conn, task_id)?;
        match task.state {
            TaskState::Queued | TaskState::RetryWait => self.set_state(task_id, &user_pause()),
            TaskState::Paused => Ok(()),
            TaskState::Seeding => self.stop_sharing(task_id),
            _ => Err(Self::invalid(&task, "mise en pause")),
        }
    }

    /// Termine le partage d'un torrent qui ne tourne pas (SPEC §7.16).
    fn stop_sharing(&mut self, task_id: &str) -> Result<(), EngineError> {
        torrent::finish_sharing(&mut self.conn, task_id).map_err(|e| match e {
            TransferError::Store(e) => e.into(),
            e => EngineError::BadRequest(e.to_string()),
        })?;
        self.drop_secrets(task_id);
        self.changed(task_id, TaskState::Seeding, TaskState::Completed);
        Ok(())
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
            TaskState::Queued
            | TaskState::Active
            | TaskState::Verifying
            | TaskState::RetryWait
            | TaskState::Seeding => Ok(()),
            _ => Err(Self::invalid(&task, "reprise")),
        }
    }

    fn cancel(&mut self, task_id: &str, delete_files: bool) -> Result<(), EngineError> {
        if let Some(run) = self.running.get_mut(task_id) {
            // Commande de fin ou fusion : le fichier est fait, elle va jusqu'au bout (une
            // fusion l'a toujours fait : elle n'écoute pas l'arrêt).
            if run.shown == TaskState::PostProcessing {
                return Ok(());
            }
            run.then = Then::Cancel { delete_files };
            let _ = run.stop.send(true);
            return Ok(());
        }
        let task = load_task(&self.conn, task_id)?;
        if task.moving {
            return Err(EngineError::Moving(task_id.to_owned()));
        }
        match task.state {
            TaskState::Queued | TaskState::Paused | TaskState::RetryWait => {
                self.set_state(task_id, &Change::to(TaskState::Cancelled))?;
                if delete_files {
                    delete_part(&self.conn, &task)?;
                }
                Ok(())
            }
            // Partage interrompu : ses fichiers sont terminés (SPEC §7.16).
            TaskState::Seeding => {
                self.set_state(task_id, &Change::to(TaskState::Cancelled))?;
                if delete_files {
                    delete_final(&self.conn, &task)?;
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
            let received = |idx: u32| live.iter().find(|l| l.idx == idx).cloned();
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
        let files = if include.files {
            Some(store::torrents::load_files(&self.conn, task_id)?)
        } else {
            None
        };
        Ok(TaskDetails {
            task,
            segments,
            sources,
            files,
        })
    }

    /// Retéléchargement : l'état et l'adresse d'abord, puis les fichiers, puis la base.
    fn restart(&mut self, task_id: &str, delete_files: bool) -> Result<(), EngineError> {
        let task = load_task(&self.conn, task_id)?;
        if task.moving {
            return Err(EngineError::Moving(task_id.to_owned()));
        }
        let allowed = matches!(
            task.state,
            TaskState::Completed
                | TaskState::Failed
                | TaskState::Cancelled
                | TaskState::Paused
                | TaskState::RetryWait
        );
        if !allowed || self.running.contains_key(task_id) {
            return Err(Self::invalid(&task, "retéléchargement"));
        }
        if tasks::load_sources(&self.conn, task_id)?.is_empty() {
            return Err(EngineError::BadRequest(format!(
                "tâche {task_id} : adresse non conservée (terminée avant le 2026-11-07) ; \
                 l'ajouter de nouveau"
            )));
        }
        if delete_files && task.state == TaskState::Completed {
            delete_final(&self.conn, &task)?;
        }
        delete_part(&self.conn, &task)?;
        let from = tasks::restart_task(&mut self.conn, task_id)?;
        self.broken.remove(task_id);
        let id = task_id.to_owned();
        let checksum = load_task(&self.conn, task_id).ok().and_then(|t| t.checksum);
        self.emit(|rev| Event::Restarted {
            rev,
            task_id: id,
            checksum,
        });
        self.changed(task_id, from, TaskState::Queued);
        Ok(())
    }

    /// Une tâche qui ne tourne pas et dont le fichier ne bouge pas : ce qu'exigent un
    /// déplacement et un changement d'adresse.
    fn idle(&self, task_id: &str, action: &'static str) -> Result<TaskRow, EngineError> {
        let task = load_task(&self.conn, task_id)?;
        if task.moving {
            return Err(EngineError::Moving(task_id.to_owned()));
        }
        // Déplacer, renommer, changer d'adresse : pas pour un torrent en v1 (SPEC §7.16), ni
        // pour une vidéo : ses sous-titres ne suivraient pas, et son adresse est celle de
        // la page, relue à chaque exécution (SPEC §7.17).
        match task.kind {
            TaskKind::Torrent => {
                return Err(EngineError::BadRequest(format!(
                    "{action} : pas encore pour un torrent"
                )));
            }
            TaskKind::Media => {
                return Err(EngineError::BadRequest(format!(
                    "{action} : pas encore pour une vidéo"
                )));
            }
            TaskKind::Http => {}
        }
        let allowed = matches!(
            task.state,
            TaskState::Queued
                | TaskState::Paused
                | TaskState::RetryWait
                | TaskState::Failed
                | TaskState::Cancelled
                | TaskState::Completed
        );
        if !allowed || self.running.contains_key(task_id) {
            return Err(Self::invalid(&task, action));
        }
        Ok(task)
    }

    /// Annonce le dossier, le nom et le déplacement en cours d'une tâche.
    fn relocated(&self, task_id: &str, error: Option<String>) -> Result<TaskRow, EngineError> {
        let task = load_task(&self.conn, task_id)?;
        let t = task.clone();
        self.emit(|rev| Event::Relocated {
            rev,
            task_id: t.id,
            dest_dir: t.dest_dir,
            file_name: t.file_name,
            display_name: t.display_name,
            moving: t.moving,
            error,
        });
        Ok(task)
    }

    /// « Déplacer/Renommer » (SPEC §7.12). Tout est vérifié avant de toucher au disque ;
    /// le journal est écrit avant le premier changement.
    fn relocate(
        &mut self,
        task_id: &str,
        dest_dir: Option<String>,
        name: Option<String>,
        remote: bool,
    ) -> Result<TaskRow, EngineError> {
        let task = self.idle(task_id, "déplacement")?;
        // Connexion à jeton : un autre dossier, seulement sous celui des téléchargements.
        if remote && let Some(dir) = &dest_dir {
            check_dest_dir(dir)?;
            self.check_within_downloads(dir)?;
        }
        if let Some(name) = &name {
            check_file_name(name)?;
        }
        let dir = dest_dir.unwrap_or_else(|| task.dest_dir.clone());
        check_dest_dir(&dir)?;
        let Some(current) = task.file_name.clone() else {
            // Pas encore préparée : aucun fichier, la préparation prendra ce dossier et ce
            // nom.
            store::moves::set_location(&self.conn, task_id, &dir, name.as_deref())?;
            return self.relocated(task_id, None);
        };
        let new_name = name.unwrap_or_else(|| current.clone());
        let old_final = Path::new(&task.dest_dir).join(&current);
        let new_final = Path::new(&dir).join(&new_name);
        if old_final == new_final {
            return Ok(task);
        }
        // Sous Windows, changer la casse du nom désigne le même fichier.
        let same_file = dir == task.dest_dir && crate::io::same_name(&current, &new_name);
        if !same_file {
            for path in [&new_final, &part_path(&new_final)] {
                if path.exists() {
                    return Err(EngineError::FileExists(path.clone()));
                }
            }
            let reserved = tasks::reserved_names(&self.conn, &dir, task_id)?;
            if reserved.iter().any(|r| crate::io::same_name(r, &new_name)) {
                return Err(EngineError::FileExists(new_final));
            }
        }
        let completed = task.state == TaskState::Completed;
        let (from, to) = if completed {
            (old_final, new_final)
        } else {
            (part_path(&old_final), part_path(&new_final))
        };
        if !from.exists() {
            if completed {
                return Err(EngineError::FileMissing(from));
            }
            // `.part` absent : la reprise le recréera au nouvel endroit (SPEC §7.4).
            store::moves::set_location(&self.conn, task_id, &dir, Some(&new_name))?;
            return self.relocated(task_id, None);
        }
        let journal = store::moves::FileMove {
            task_id: task_id.to_owned(),
            from_path: from.to_string_lossy().into_owned(),
            to_path: to.to_string_lossy().into_owned(),
            temp_path: None,
            dest_dir: dir,
            file_name: new_name,
            phase: store::moves::Phase::Moving,
        };
        store::moves::begin(&self.conn, &journal)?;
        match crate::moves::rename(&from, &to) {
            Ok(true) => {
                store::moves::commit(&mut self.conn, task_id, store::moves::Phase::Moving)?;
                self.relocated(task_id, None)
            }
            Ok(false) => self.copy_across(task_id, from, to),
            Err(e) => {
                store::moves::forget(&self.conn, task_id)?;
                Err(EngineError::Move(e))
            }
        }
    }

    /// Vers un autre volume : la copie se fait hors de la boucle ; sa fin revient par
    /// `moved`.
    fn copy_across(
        &mut self,
        task_id: &str,
        from: PathBuf,
        to: PathBuf,
    ) -> Result<TaskRow, EngineError> {
        let temp = crate::moves::temp_for(&to);
        if temp.exists() {
            store::moves::forget(&self.conn, task_id)?;
            return Err(EngineError::FileExists(temp));
        }
        store::moves::set_temp(&self.conn, task_id, &temp.to_string_lossy())?;
        let done = self.moved.clone();
        let id = task_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let result = crate::moves::copy_across(&from, &temp, &to);
            let _ = done.send((id, result));
        });
        self.relocated(task_id, None)
    }

    /// Fin d'une copie vers un autre volume : la base passe au nouvel emplacement, puis
    /// l'original est effacé. Un échec laisse la tâche à sa place, copie partielle effacée.
    fn moved(&mut self, task_id: &str, result: std::io::Result<()>) {
        let Ok(Some(journal)) = store::moves::load(&self.conn, task_id) else {
            return;
        };
        let error = match result {
            Ok(()) => {
                match store::moves::commit(&mut self.conn, task_id, store::moves::Phase::Moved) {
                    Ok(()) => {
                        crate::moves::remove_quietly(
                            Path::new(&journal.from_path),
                            "fin d'un déplacement",
                        );
                        None
                    }
                    Err(e) => Some(e.to_string()),
                }
            }
            Err(e) => {
                if let Some(temp) = &journal.temp_path {
                    crate::moves::remove_quietly(Path::new(temp), "copie abandonnée");
                }
                Some(EngineError::Move(e).to_string())
            }
        };
        if store::moves::forget(&self.conn, task_id).is_err() {
            return;
        }
        let _ = self.relocated(task_id, error);
    }

    /// Avant d'interroger une nouvelle adresse (SPEC §7.13) : la taille à retrouver et les
    /// en-têtes à envoyer.
    fn check_source(&self, task_id: &str, url: &str) -> Result<SourceCheck, EngineError> {
        let task = self.idle(task_id, "changement d'adresse")?;
        let old = tasks::primary_source(&self.conn, task_id)?;
        let other_host = transfer::host_of(&old.url) != transfer::host_of(url);
        let headers: Vec<StoredHeader> = headers::load(&self.conn, task_id)?
            .into_iter()
            .filter(|(_, s)| !other_host || matches!(s, Stored::Plain(_)))
            .collect();
        // Identifiants du site de la nouvelle adresse (SPEC §7.14), sauf `Authorization`
        // propre à la tâche.
        let explicit = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"));
        let login = match explicit {
            true => None,
            false => store::logins::best_for(&self.conn, url)?,
        };
        // Une tâche terminée ou annulée ne reprendra pas : son retéléchargement
        // réapprendra tout.
        let resumable = !matches!(task.state, TaskState::Completed | TaskState::Cancelled);
        Ok(SourceCheck {
            expect_bytes: task.total_bytes.filter(|_| resumable),
            headers,
            other_host,
            login,
        })
    }

    /// Peut-on ajouter le miroir `url` à la tâche ? Une tâche HTTP pas encore terminée,
    /// sans cette adresse parmi ses sources, qui en a moins de 16.
    fn check_mirror(&self, task_id: &str, url: &str) -> Result<MirrorCheck, EngineError> {
        let task = load_task(&self.conn, task_id)?;
        if task.kind != TaskKind::Http || store::sources::is_internal(&self.conn, task_id)? {
            return Err(EngineError::BadRequest(
                "seul un téléchargement HTTP prend des miroirs".into(),
            ));
        }
        if matches!(task.state, TaskState::Completed | TaskState::Cancelled) {
            return Err(Self::invalid(&task, "ajout d'un miroir"));
        }
        let sources = tasks::load_sources(&self.conn, task_id)?;
        if sources.iter().any(|s| s.url == url) {
            return Err(EngineError::BadRequest(
                "adresse déjà présente parmi les sources de la tâche".into(),
            ));
        }
        if sources.len() >= store::sources::MAX_SOURCES {
            return Err(EngineError::BadRequest(format!(
                "{} sources au plus par tâche",
                store::sources::MAX_SOURCES
            )));
        }
        let reference = tasks::primary_source(&self.conn, task_id)?;
        let headers = headers::load(&self.conn, task_id)?;
        let explicit = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"));
        let same_host = transfer::host_of(&reference.url) == transfer::host_of(url);
        let reference_login = match explicit {
            true => None,
            false => store::logins::best_for(&self.conn, &reference.url)?,
        };
        // Un `Authorization` propre à la tâche ne part pas vers un autre hôte : le miroir y
        // prend les identifiants de son site.
        let mirror_login = match explicit && same_host {
            true => None,
            false => store::logins::best_for(&self.conn, url)?,
        };
        let total = task
            .total_bytes
            .filter(|_| task.file_name.is_some() && reference.accepts_ranges == Some(true));
        Ok(MirrorCheck {
            reference,
            total,
            headers,
            reference_login,
            mirror_login,
        })
    }

    /// Enregistre le miroir vérifié par `add_source` ; avec `facts`, il est déjà validé.
    /// Une tâche en cours repart aussitôt, pour s'en servir (étape 1b).
    fn add_mirror(
        &mut self,
        task_id: &str,
        url: &str,
        facts: Option<mirrors::Facts>,
        public_only: bool,
    ) -> Result<String, EngineError> {
        self.check_mirror(task_id, url)?;
        // Miroir donné par une connexion à jeton : la tâche passe au réseau public seulement,
        // avant que ce miroir ne serve (SPEC §7.19).
        if public_only {
            tasks::restrict_to_public(&self.conn, task_id)?;
        }
        let source_id = store::sources::add_source(&mut self.conn, task_id, url)?;
        if let Some(facts) = &facts {
            let checked = store::sources::Checked {
                accepts_ranges: true,
                etag: facts.etag.as_deref(),
                last_modified: facts.last_modified.as_deref(),
                content_length: Some(facts.content_length),
            };
            store::sources::set_checked(&self.conn, &source_id, &checked)?;
        }
        if let Some(run) = self.running.get_mut(task_id)
            && run.then == Then::Keep
            && !*run.stop.borrow()
        {
            run.then = Then::Requeue;
            let _ = run.stop.send(true);
        }
        Ok(source_id)
    }

    /// Enregistre la nouvelle adresse, vérifiée par `set_url`. La tâche a pu changer
    /// pendant la vérification : elle est relue.
    fn replace_url(
        &mut self,
        task_id: &str,
        url: &str,
        facts: Option<SourceFacts>,
        public_only: bool,
    ) -> Result<TaskRow, EngineError> {
        let check = self.check_source(task_id, url)?;
        if check.expect_bytes != facts.as_ref().map(|f| f.total_bytes) {
            return Err(EngineError::BadRequest(format!(
                "la tâche {task_id} a changé pendant la vérification de l'adresse : réessayer"
            )));
        }
        // Adresse donnée par une connexion à jeton : la tâche passe au réseau public
        // seulement (SPEC §7.19).
        if public_only {
            tasks::restrict_to_public(&self.conn, task_id)?;
        }
        let task = load_task(&self.conn, task_id)?;
        // Nom tiré de l'adresse, tant que ni le serveur ni l'utilisateur n'en ont donné.
        let display_name = (task.file_name.is_none() && task.requested_name.is_none())
            .then(|| transfer::display_name_for(url));
        // Les cookies de l'ancien hôte n'iront pas au nouveau (référence effacée avant le
        // secret, SPEC §5.1).
        if check.other_host {
            self.drop_secrets(task_id);
        }
        let revalidation = facts.as_ref().map(|f| tasks::Revalidation {
            accepts_ranges: f.accepts_ranges,
            etag: f.etag.as_deref(),
            last_modified: f.last_modified.as_deref(),
            total_bytes: Some(f.total_bytes),
        });
        tasks::replace_url(
            &mut self.conn,
            task_id,
            url,
            revalidation.as_ref(),
            display_name.as_deref(),
        )?;
        let task = load_task(&self.conn, task_id)?;
        let (id, name) = (task.id.clone(), task.display_name.clone());
        self.emit(|rev| Event::SourceChanged {
            rev,
            task_id: id,
            display_name: name,
        });
        Ok(task)
    }

    /// Retire une tâche finie. Le fichier demandé est supprimé d'abord : si cela échoue, la
    /// tâche reste, et l'utilisateur peut réessayer.
    fn remove(&mut self, task_id: &str, delete_files: bool) -> Result<(), EngineError> {
        let task = load_task(&self.conn, task_id)?;
        if task.moving {
            return Err(EngineError::Moving(task_id.to_owned()));
        }
        let finished = matches!(
            task.state,
            TaskState::Completed | TaskState::Failed | TaskState::Cancelled
        );
        if !finished || self.running.contains_key(task_id) {
            return Err(Self::invalid(&task, "retrait"));
        }
        if delete_files {
            match task.state {
                TaskState::Completed => delete_final(&self.conn, &task)?,
                _ => delete_part(&self.conn, &task)?,
            }
        }
        let secrets = headers::secret_refs(&self.conn, task_id)?;
        tasks::remove_task(&mut self.conn, task_id)?;
        self.vault().forget(secrets);
        self.broken.remove(task_id);
        let id = task_id.to_owned();
        self.emit(|rev| Event::Removed { rev, task_id: id });
        Ok(())
    }

    /// L'interface des torrents est là, ou aucune n'est demandée.
    fn interface_up(&self) -> bool {
        self.iface_name.is_none() || self.iface_binding.is_some()
    }

    /// Un torrent peut partir : son interface est là, et aucun torrent lancé avant le
    /// dernier changement d'interface ne tourne encore.
    fn torrents_may_start(&self) -> bool {
        self.interface_up()
            && self
                .running
                .values()
                .all(|r| r.torrent.is_none_or(|g| g == self.iface_generation))
    }

    /// Une tâche restée `active` par un arrêt brutal, qui ne peut pas repartir : elle
    /// retourne en file, par une pause de cette cause.
    fn requeue_crashed(&mut self, task_id: &str, reason: PauseReason) {
        let paused = Change {
            pause_reason: Some(reason),
            ..Change::to(TaskState::Paused)
        };
        let requeued = self
            .set_state(task_id, &paused)
            .and_then(|()| self.set_state(task_id, &Change::to(TaskState::Queued)));
        if requeued.is_err() {
            self.broken.insert(task_id.to_owned());
        }
    }

    /// Surveille l'interface des torrents (toutes les [`INTERFACE_CHECK`]) : disparue,
    /// revenue ou changée (VPN reconnecté, adresse changée), elle change de génération.
    fn check_interface(&mut self) {
        let now = Instant::now();
        let Some(name) = &self.iface_name else {
            return;
        };
        if now < self.next_iface_check {
            return;
        }
        self.next_iface_check = now + INTERFACE_CHECK;
        let binding = self.torrents.resolve(name);
        if binding != self.iface_binding {
            let was_up = self.interface_up();
            self.rebind(binding, was_up);
        }
    }

    /// Nouvelle génération de l'interface : les torrents en cours s'arrêtent et retournent
    /// en file (un partage reste `seeding`) ; ils repartiront, sur une session liée à la
    /// nouvelle interface, quand elle sera là. Sans elle, rien ne passe par une autre
    /// (décision de l'utilisateur du 2026-11-09 : les torrents attendent).
    /// `was_up` : l'interface était là avant le changement.
    fn rebind(&mut self, binding: Option<torrent::Binding>, was_up: bool) {
        self.iface_binding = binding;
        self.iface_generation += 1;
        for run in self.running.values_mut() {
            if run.torrent.is_some() && run.then == Then::Keep && !*run.stop.borrow() {
                run.then = Then::Requeue;
                let _ = run.stop.send(true);
            }
        }
        let up = self.interface_up();
        if up != was_up {
            self.emit(|rev| Event::TorrentInterfaceChanged { rev, up });
        }
    }

    /// Interface réseau des torrents (SPEC §7.16) : un nom, ou `None` pour aucune.
    fn set_remote_listen(
        &mut self,
        addr: Option<std::net::SocketAddr>,
    ) -> Result<std::net::SocketAddr, EngineError> {
        settings::set_remote_listen(&self.conn, addr)?;
        let effective = settings::remote_listen(&self.conn)?;
        self.emit(|rev| Event::SettingChanged {
            rev,
            key: settings::REMOTE_LISTEN,
            value: serde_json::Value::from(effective.to_string()),
        });
        Ok(effective)
    }

    fn set_remote_hosts(&mut self, hosts: Vec<String>) -> Result<(), EngineError> {
        if hosts.len() > settings::REMOTE_HOSTS_MAX {
            return Err(EngineError::BadRequest(format!(
                "{} : {} noms au plus",
                settings::REMOTE_HOSTS,
                settings::REMOTE_HOSTS_MAX
            )));
        }
        if let Some(bad) = hosts.iter().find(|h| !settings::valid_host_name(h)) {
            return Err(EngineError::BadRequest(format!(
                "{} : {bad} n'est ni un nom DNS ni une adresse IP",
                settings::REMOTE_HOSTS
            )));
        }
        let mut kept: Vec<String> = Vec::new();
        for host in hosts {
            let host = host.to_ascii_lowercase();
            if !kept.contains(&host) {
                kept.push(host);
            }
        }
        settings::set_remote_hosts(&self.conn, &kept)?;
        self.emit(|rev| Event::SettingChanged {
            rev,
            key: settings::REMOTE_HOSTS,
            value: serde_json::Value::from(kept),
        });
        Ok(())
    }

    fn set_remote_tls(&mut self, files: Option<(String, String)>) -> Result<(), EngineError> {
        if let Some((cert, key)) = &files {
            for (setting, path) in [
                (settings::REMOTE_TLS_CERT, cert),
                (settings::REMOTE_TLS_KEY, key),
            ] {
                if !Path::new(path).is_absolute() {
                    return Err(EngineError::BadRequest(format!(
                        "{setting} : un chemin absolu est attendu"
                    )));
                }
            }
        }
        settings::set_remote_tls(
            &mut self.conn,
            files.as_ref().map(|(c, k)| (c.as_str(), k.as_str())),
        )?;
        let (cert, key) = files.map_or(
            (serde_json::Value::Null, serde_json::Value::Null),
            |(c, k)| (c.into(), k.into()),
        );
        // Deux annonces ; l'écoute ne repart qu'une fois (elle compare ses réglages).
        self.emit(|rev| Event::SettingChanged {
            rev,
            key: settings::REMOTE_TLS_CERT,
            value: cert,
        });
        self.emit(|rev| Event::SettingChanged {
            rev,
            key: settings::REMOTE_TLS_KEY,
            value: key,
        });
        Ok(())
    }

    fn set_torrent_interface(&mut self, name: Option<String>) -> Result<(), EngineError> {
        if name
            .as_deref()
            .is_some_and(|n| !settings::valid_interface_name(n))
        {
            return Err(EngineError::BadRequest(format!(
                "{} : un nom d'interface (1 à {} caractères, sans caractère de contrôle), \
                 ou null",
                settings::TORRENT_INTERFACE,
                settings::INTERFACE_NAME_MAX
            )));
        }
        if name == self.iface_name {
            return Ok(());
        }
        settings::set_torrent_interface(&self.conn, name.as_deref())?;
        let binding = name.as_deref().and_then(|n| self.torrents.resolve(n));
        let was_up = self.interface_up();
        self.iface_name = name.clone();
        self.next_iface_check = Instant::now() + INTERFACE_CHECK;
        self.rebind(binding, was_up);
        self.emit(|rev| Event::SettingChanged {
            rev,
            key: settings::TORRENT_INTERFACE,
            value: name.map_or(serde_json::Value::Null, serde_json::Value::from),
        });
        Ok(())
    }

    fn proxy_view(&self) -> Result<ProxyView, EngineError> {
        let (settings, secret) = settings::proxy(&self.conn)?;
        Ok(ProxyView {
            settings,
            has_password: secret.is_some(),
        })
    }

    /// Proxy changé (F-07). Ordre des secrets : le nouveau mot de passe au coffre avant sa
    /// référence en base, l'ancien effacé après ; rien ne change si le client ne se fait
    /// pas avec ces réglages.
    fn set_proxy(&mut self, change: ProxyChange) -> Result<ProxyView, EngineError> {
        net::proxy::validate(&change.settings).map_err(EngineError::BadRequest)?;
        let (_, old) = settings::proxy(&self.conn)?;
        let keep_user = change.settings.username.is_some();
        let (secret, password, fresh) = match (keep_user, change.password) {
            (false, _) | (true, Some(None)) => (None, None, None),
            (true, None) => {
                let password = match &old {
                    Some(id) => self.vault().get(id)?,
                    None => None,
                };
                (old.clone(), password, None)
            }
            (true, Some(Some(password))) => {
                let id = self.vault().put(&password)?;
                (Some(id.clone()), Some(password), Some(id))
            }
        };
        let effective = net::proxy::Effective {
            settings: change.settings.clone(),
            password,
        };
        let applied = self
            .config
            .client
            .set_proxy(effective)
            .map_err(|e| EngineError::BadRequest(e.to_string()))
            .and_then(|()| {
                settings::set_proxy(&self.conn, &change.settings, secret.as_deref())
                    .map_err(EngineError::from)
            });
        if let Err(e) = applied {
            if let Some(id) = fresh {
                self.vault().forget(vec![id]);
            }
            return Err(e);
        }
        if let Some(old) = old.filter(|o| secret.as_ref() != Some(o)) {
            self.vault().forget(vec![old]);
        }
        tracing::info!(mode = change.settings.mode.as_str(), "proxy changé");
        self.proxy_view()
    }

    fn set_logs_level(&mut self, level: crate::logs::Level) -> Result<(), EngineError> {
        settings::set_logs_level(&self.conn, level)?;
        self.emit(|rev| Event::SettingChanged {
            rev,
            key: settings::LOGS_LEVEL,
            value: serde_json::Value::from(level.as_str()),
        });
        Ok(())
    }

    /// Réseau utilisable ou non (SPEC §7.15) : sans lui, les tâches en cours retournent en
    /// file, sauf le calcul d'une empreinte, et rien ne démarre ; avec lui, elles repartent.
    fn set_network(&mut self, usable: bool, metered: bool) {
        if self.network_usable == usable && self.network_metered == metered {
            return;
        }
        self.network_usable = usable;
        self.network_metered = metered;
        if usable {
            // Une erreur de réseau survenue avant que l'application dise la coupure : la tâche
            // repart maintenant, sans attendre son délai (SPEC §7.15).
            let _ = tasks::retry_network_waits(&self.conn, now_ms());
        }
        for run in self.running.values_mut() {
            // Sans réseau, tout s'arrête ; sur un réseau facturé, seuls les partages de
            // torrents (SPEC §7.16). Une tâche que l'utilisateur arrête ou annule garde sa
            // suite ; un partage arrêté reste `seeding` et reprendra.
            let stops = !usable || (metered && run.shown == TaskState::Seeding);
            if stops
                && run.shown != TaskState::Verifying
                && run.shown != TaskState::PostProcessing
                && run.then == Then::Keep
                && !*run.stop.borrow()
            {
                run.then = Then::Requeue;
                let _ = run.stop.send(true);
            }
        }
        self.emit(|rev| Event::NetworkChanged {
            rev,
            usable,
            metered,
        });
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
        if let Ok(Outcome::Completed(_)) = &result {
            self.drop_secrets(task_id);
        }
        let shown = run.as_ref().map_or(TaskState::Active, |r| r.shown);
        let then = run.map_or(Then::Keep, |r| r.then);
        self.limits.tasks.remove(task_id);
        if let Err(e) = &result {
            self.record_internal_error(task_id, e);
        }
        let Ok(task) = load_task(&self.conn, task_id) else {
            return;
        };
        self.changed(task_id, shown, task.state);
        if task.kind == TaskKind::Media && matches!(result, Ok(Outcome::Completed(_))) {
            self.warn_missing_subtitles(&task);
        }
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
                        true => delete_part(&self.conn, &task),
                        false => Ok(()),
                    }),
                // Partage arrêté (SPEC §7.16) : les fichiers sont terminés.
                TaskState::Seeding => self
                    .set_state(task_id, &Change::to(TaskState::Cancelled))
                    .and_then(|()| match delete_files {
                        true => delete_final(&self.conn, &task),
                        false => Ok(()),
                    }),
                _ => Ok(()),
            },
            // Pause pendant le partage : la tâche est terminée.
            Then::Complete if task.state == TaskState::Seeding => self.stop_sharing(task_id),
            Then::Complete => Ok(()),
        };
    }

    /// Vidéo terminée sans certains des sous-titres demandés (F-24, SPEC §7.17) : absents
    /// de la page ou refusés par le site. Annoncé par `engine.warning`.
    fn warn_missing_subtitles(&mut self, task: &TaskRow) {
        let missing = match store::media::missing_subtitles(&self.conn, &task.id) {
            Ok(missing) if !missing.is_empty() => missing,
            _ => return,
        };
        let message = format!(
            "« {} » : sous-titres absents ou refusés par le site : {}",
            task.display_name,
            missing.join(", ")
        );
        let task_id = Some(task.id.clone());
        self.emit(|rev| Event::Warning {
            rev,
            code: "SUBTITLES_MISSING",
            message,
            task_id,
        });
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
        let shown = self
            .running
            .remove(&task_id)
            .map_or(TaskState::Active, |r| r.shown);
        self.limits.tasks.remove(&task_id);
        self.record_internal_error(&task_id, &TransferError::Join(error));
        if let Ok(task) = load_task(&self.conn, &task_id) {
            self.changed(&task_id, shown, task.state);
        }
    }
}

/// Références des secrets parmi des en-têtes enregistrés.
fn secret_ids(stored: Vec<StoredHeader>) -> Vec<String> {
    stored
        .into_iter()
        .filter_map(|(_, s)| match s {
            Stored::Secret(id) => Some(id),
            Stored::Plain(_) => None,
        })
        .collect()
}

/// Un nom de fichier choisi par l'utilisateur : tel quel, il doit être un nom valide sur ce
/// système (SPEC §8.1) ; le moteur ne le corrige pas en silence.
fn check_file_name(name: &str) -> Result<(), EngineError> {
    let clean = net::filename::sanitize(name, net::filename::NameRules::native());
    if clean != name {
        return Err(EngineError::BadRequest(format!(
            "nom de fichier refusé : {name:?} (caractères interdits, nom réservé, points ou \
             espaces en bord, ou trop long) ; proposition : {clean:?}"
        )));
    }
    Ok(())
}

/// Identifiants d'un site, mot de passe lu dans le coffre, hors de la boucle (SPEC §7.14).
async fn reveal_site_login(
    login: Option<store::logins::LoginRow>,
    vault: &Vault,
) -> Result<Option<crate::net::auth::Auth>, EngineError> {
    let Some(login) = login else {
        return Ok(None);
    };
    let Ok(site) = crate::net::auth::Site::parse(&login.site) else {
        return Ok(None);
    };
    let (vault, id) = (vault.clone(), login.secret_ref.clone());
    let password = tokio::task::spawn_blocking(move || vault.get(&id))
        .await
        .map_err(|_| EngineError::Stopped)??;
    Ok(password.map(|password| {
        crate::net::auth::Auth::new(crate::net::auth::Login {
            site,
            username: login.username,
            password,
        })
    }))
}

/// En-têtes à envoyer, secrets lus dans le coffre, hors de la boucle. Un secret absent du
/// coffre n'est pas envoyé, comme pour une exécution.
async fn reveal(
    stored: Vec<StoredHeader>,
    vault: &Vault,
) -> Result<Vec<(String, String)>, EngineError> {
    let mut list = Vec::new();
    for (name, stored) in stored {
        match stored {
            Stored::Plain(value) => list.push((name, value)),
            Stored::Secret(id) => {
                let vault = vault.clone();
                let found = tokio::task::spawn_blocking(move || vault.get(&id))
                    .await
                    .map_err(|_| EngineError::Stopped)??;
                if let Some(secret) = found {
                    list.push((name, secret));
                }
            }
        }
    }
    Ok(list)
}

/// Ratio de partage tel que le protocole le montre : un nombre, ou `null` (sans limite).
fn ratio_json(ratio: Option<f64>) -> serde_json::Value {
    ratio.map_or(serde_json::Value::Null, serde_json::Value::from)
}

/// Un lien magnet (SPEC §7.16), quelle que soit la casse du schéma.
fn is_magnet(url: &str) -> bool {
    url.get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("magnet:"))
}

/// Un dossier de destination : chemin absolu d'un dossier existant.
/// Crée les dossiers `parts` sous `base` (noms déjà vérifiés) et rend le chemin obtenu.
fn sub_dir(base: &str, parts: &[String]) -> Result<String, EngineError> {
    let path: PathBuf = std::iter::once(base)
        .chain(parts.iter().map(String::as_str))
        .collect();
    std::fs::create_dir_all(&path)
        .map_err(|e| EngineError::DestDir(format!("{} : {e}", path.display())))?;
    path.into_os_string()
        .into_string()
        .map_err(|_| EngineError::DestDir("chemin de destination illisible".into()))
}

/// Texte de [`EngineError::SourceUnreachable`] : une adresse interne refusée (SPEC §7.19) se
/// dit telle quelle.
fn unreachable_text(e: &NetError) -> String {
    match e {
        NetError::AddressForbidden => e.to_string(),
        _ => format!("nouvelle adresse injoignable : {e}"),
    }
}

/// Connexion à jeton : une adresse écrite telle quelle hors du réseau public est refusée
/// dès l'ajout ; un nom l'est à la connexion, après sa résolution (SPEC §7.19).
fn check_public_literal(url: &str) -> Result<(), EngineError> {
    let parsed = net::parse_url(url)?;
    if net::reach::literal_forbidden(&parsed) {
        return Err(EngineError::SourceUnreachable(NetError::AddressForbidden));
    }
    Ok(())
}

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

/// Supprime le `.part` d'une tâche annulée, s'il existe ; ceux de chaque fichier d'un
/// torrent (SPEC §7.16).
fn delete_part(conn: &Connection, task: &TaskRow) -> Result<(), EngineError> {
    let Some(name) = &task.file_name else {
        return Ok(());
    };
    let path = Path::new(&task.dest_dir).join(name);
    if task.kind == TaskKind::Torrent {
        return torrent::delete_files(conn, task, false)
            .map_err(|source| EngineError::DeleteFile { path, source });
    }
    if task.kind == TaskKind::Media {
        return media::run::delete_files(conn, task, false)
            .map_err(|source| EngineError::DeleteFile { path, source });
    }
    delete_file(part_path(&path))
}

/// Supprime le fichier final d'une tâche terminée, s'il existe encore ; tous ceux d'un
/// torrent, et son dossier s'il reste vide.
fn delete_final(conn: &Connection, task: &TaskRow) -> Result<(), EngineError> {
    let Some(name) = &task.file_name else {
        return Ok(());
    };
    let path = Path::new(&task.dest_dir).join(name);
    if task.kind == TaskKind::Torrent {
        return torrent::delete_files(conn, task, true)
            .map_err(|source| EngineError::DeleteFile { path, source });
    }
    if task.kind == TaskKind::Media {
        return media::run::delete_files(conn, task, true)
            .map_err(|source| EngineError::DeleteFile { path, source });
    }
    delete_file(path)
}

fn delete_file(path: PathBuf) -> Result<(), EngineError> {
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(EngineError::DeleteFile { path, source }),
    }
}

/// Une exécution, avec sa propre connexion à la base : un téléchargement HTTP
/// (`transfer::run`) ou un torrent (`torrent::run`, SPEC §7.16).
#[allow(clippy::too_many_arguments)]
async fn execute(
    config: &Config,
    torrents: &Torrents,
    tools: Option<&Arc<media::tools::Tools>>,
    task_id: &str,
    stop: watch::Receiver<bool>,
    events: broadcast::Sender<Event>,
    rev: Arc<AtomicU64>,
    prepared: mpsc::UnboundedSender<RunNote>,
    live: watch::Sender<Arc<[LiveSegment]>>,
) -> Result<Outcome, TransferError> {
    let mut conn = store::open(&config.db_path)?;
    // `transfer::run` enregistre la préparation avant le premier avancement : celui-ci
    // la signale à la boucle.
    // Le premier avancement de la vérification de l'empreinte suit le passage à
    // `verifying` (F-11), qu'il signale de même.
    let (mut first, mut verifying, mut seeding, mut merging) = (true, false, false, false);
    let mut on_progress = |progress: Progress| {
        if std::mem::take(&mut first) {
            let _ = prepared.send(RunNote::Prepared(task_id.to_owned()));
        }
        if progress.verified.is_some() && !std::mem::replace(&mut verifying, true) {
            let _ = prepared.send(RunNote::Verifying(task_id.to_owned()));
        }
        // Un torrent passe au partage (SPEC §7.16) : annoncé comme `verifying`.
        if progress.seeding && !std::mem::replace(&mut seeding, true) {
            let _ = prepared.send(RunNote::Seeding(task_id.to_owned()));
        }
        // Une vidéo passe à la fusion (SPEC §7.17).
        if progress.merging && !std::mem::replace(&mut merging, true) {
            let _ = prepared.send(RunNote::PostProcessing(task_id.to_owned()));
        }
        live.send_replace(Arc::clone(&progress.segments));
        let _ = events.send(Event::Progress {
            rev: rev.load(Ordering::SeqCst),
            task_id: task_id.to_owned(),
            progress,
        });
    };
    let task = load_task(&conn, task_id)?;
    // Ajoutée par une connexion à jeton : réseau public seulement, reprises comprises
    // (SPEC §7.19).
    let client = config
        .client
        .reaching(task.public_only)
        .map_err(TransferError::RestrictedClient)?;
    let mut command_stop = stop.clone();
    // Commande de fin arrêtée avec le moteur (SPEC §7.21) : relancée telle quelle. Une
    // vidéo en `post_processing` peut aussi être en fusion : `media::run` le démêle.
    let mut outcome = if task.state == TaskState::PostProcessing && task.kind != TaskKind::Media {
        Outcome::Completed(crate::postproc::final_path(&conn, task_id)?)
    } else {
        match task.kind {
            TaskKind::Torrent => {
                torrent::run(&mut conn, torrents, task_id, stop.clone(), &mut on_progress).await?
            }
            // Vidéo (SPEC §7.17) : ses flux directs passent par `transfer::run`.
            TaskKind::Media => {
                let ctx = media::run::Context {
                    client: &client,
                    opts: &config.transfer,
                    tools,
                };
                media::run::run(&mut conn, &ctx, task_id, stop.clone(), &mut on_progress).await?
            }
            TaskKind::Http => {
                transfer::run(
                    &mut conn,
                    &client,
                    task_id,
                    &config.transfer,
                    stop.clone(),
                    &mut on_progress,
                )
                .await?
            }
        }
    };
    // Fichier à son nom, commande de fin en attente (`postproc::finish`) : lancée ici.
    loop {
        let Outcome::Completed(path) = &outcome else {
            return Ok(outcome);
        };
        if load_task(&conn, task_id)?.state != TaskState::PostProcessing {
            return Ok(outcome);
        }
        let path = path.clone();
        let _ = prepared.send(RunNote::PostProcessing(task_id.to_owned()));
        match crate::postproc::run(&mut conn, task_id, &mut command_stop).await? {
            crate::postproc::Ran::Failed(failed) => return Ok(failed),
            crate::postproc::Ran::Stopped => return Ok(Outcome::PostProcessing),
            // Torrent : le partage suit la commande, si son ratio le demande.
            crate::postproc::Ran::Done if task.kind == TaskKind::Torrent => {
                if !torrent::after_command(&mut conn, task_id)? {
                    return Ok(Outcome::Completed(path));
                }
                outcome =
                    torrent::run(&mut conn, torrents, task_id, stop.clone(), &mut on_progress)
                        .await?;
            }
            crate::postproc::Ran::Done => return crate::postproc::complete(&mut conn, task_id),
        }
    }
}
