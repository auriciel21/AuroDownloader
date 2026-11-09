//! Torrent (SPEC §7.16, phase 5) : librqbit, derrière ce module. Rien d'autre dans le
//! moteur ne connaît la bibliothèque, pour pouvoir en changer (SPEC §9.2).
//!
//! Une seule session par moteur, ouverte avec le premier torrent en cours et fermée avec le
//! dernier : un moteur sans torrent en cours n'ouvre ni port ni DHT. Chaque exécution
//! ajoute son torrent à la session pour le télécharger, l'en retire pour vider et renommer
//! ses fichiers, puis l'y remet pour le partager (`seeding`) jusqu'au ratio.
//!
//! La reprise ne fait confiance à aucun état enregistré : ni la reprise rapide ni la
//! persistance de librqbit ne servent, et chaque démarrage revérifie l'empreinte des
//! morceaux présents sur disque. Le prototype l'a montré (experiments/torrent) : après des
//! écritures perdues, librqbit et libtorrent finissent tous deux avec un fichier faux s'ils
//! se fient à leur état de reprise.

mod storage;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::net::{Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use librqbit::dht::DhtPersistenceConfig;
use librqbit::limits::LimitsConfig;
use librqbit::storage::StorageFactoryExt;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, ListenerMode,
    ListenerOptions, Magnet, Session, SessionOptions,
};
use rusqlite::Connection;
use serde_json::Value;
use tokio::sync::watch;

use crate::io::{available_name, part_path, sync_parent_dir};
use crate::net::filename::{NameRules, sanitize};
use crate::store::now_ms;
use crate::store::settings;
use crate::store::tasks::{
    Change, PauseReason, TaskRow, TaskState, change_state, complete_task, load_task,
    primary_source, reserved_names,
};
use crate::store::torrents::{
    TorrentFile, load_files, load_torrent, save_file_progress, save_metainfo, save_preparation,
    save_progress, save_uploaded,
};
use crate::transfer::{MAX_FAILURES, Outcome, Progress, TransferError, retry_delay};

/// Réglage du port d'écoute (SPEC §7.16) : tiré au hasard au premier démarrage de la
/// session, puis gardé.
pub const LISTEN_PORT: &str = "torrent.listenPort";

/// Intervalle de suivi d'un torrent : avancement, fin, arrêt.
const POLL: Duration = Duration::from_secs(1);

/// Intervalle d'enregistrement de l'avancement, qui n'est qu'une indication.
const SAVE_EVERY: Duration = Duration::from_secs(5);

/// Attente, au plus, de la fermeture des fichiers d'un torrent retiré de la session.
const CLOSE_WAIT: Duration = Duration::from_secs(10);

/// Réglages de la session.
#[derive(Debug, Clone)]
pub struct Options {
    /// DHT actif, avec sa table gardée dans le dossier de données du moteur. Faux dans les
    /// tests, qui tiennent leur essaim en local.
    pub dht: bool,
    /// Connexions entrantes.
    pub listen: Listen,
    /// Pairs ajoutés à chaque torrent, en plus de ceux que trouvent DHT et trackers ; les
    /// tests y mettent leur pair local.
    pub initial_peers: Vec<SocketAddr>,
}

/// Connexions entrantes (décision de l'utilisateur du 2026-11-09 : port d'écoute, sans
/// UPnP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listen {
    /// Aucune.
    Off,
    /// En TCP, sur le port du réglage [`LISTEN_PORT`].
    Setting,
    /// À cette adresse (tests : un pair local vient chercher ce que le moteur partage).
    Addr(SocketAddr),
}

impl Default for Options {
    fn default() -> Self {
        Options {
            dht: true,
            listen: Listen::Setting,
            initial_peers: Vec::new(),
        }
    }
}

impl Options {
    /// Sans DHT ni connexion entrante, avec ces seuls pairs : un essaim local (tests).
    pub fn local(peers: Vec<SocketAddr>) -> Self {
        Options {
            dht: false,
            listen: Listen::Off,
            initial_peers: peers,
        }
    }
}

/// La session librqbit du moteur, partagée par ses exécutions : ouverte avec le premier
/// torrent en cours, fermée avec le dernier. Un moteur sans torrent en cours n'a ni port
/// ouvert ni DHT ; un réseau qui ne convient pas (SPEC §7.15) arrête les torrents, donc la
/// session.
#[derive(Clone)]
pub struct Torrents {
    inner: Arc<Inner>,
}

struct Inner {
    options: Options,
    /// Dossier de données du moteur : table du DHT.
    state_dir: PathBuf,
    slot: tokio::sync::Mutex<Slot>,
    /// Limites globales de réception et d'envoi (SPEC §7.16), et la session ouverte, pour
    /// les y appliquer aussitôt depuis la boucle du planificateur, qui n'attend pas.
    limits: Mutex<Limits>,
}

#[derive(Default)]
struct Slot {
    session: Option<Arc<Session>>,
    /// Exécutions qui s'en servent.
    users: usize,
}

#[derive(Default)]
struct Limits {
    download: Option<u64>,
    upload: Option<u64>,
    open: Option<Arc<Session>>,
}

/// Ce qu'il faut pour ouvrir la session, lu dans la base avant de l'attendre : une
/// connexion partagée ne traverse pas une attente.
struct Opening {
    listen: Option<SocketAddr>,
    upnp: bool,
}

impl std::fmt::Debug for Torrents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Torrents")
            .field("options", &self.inner.options)
            .finish()
    }
}

/// Un débit en octets par seconde pour librqbit, qui le compte sur 32 bits.
fn bps(limit: Option<u64>) -> Option<NonZeroU32> {
    limit.and_then(|l| NonZeroU32::new(u32::try_from(l).unwrap_or(u32::MAX)))
}

impl Torrents {
    pub fn new(options: Options, state_dir: PathBuf) -> Self {
        Torrents {
            inner: Arc::new(Inner {
                options,
                state_dir,
                slot: tokio::sync::Mutex::new(Slot::default()),
                limits: Mutex::new(Limits::default()),
            }),
        }
    }

    /// Limites globales (réception, envoi), appliquées tout de suite à la session ouverte et
    /// à celles qui suivront.
    pub fn set_limits(&self, download: Option<u64>, upload: Option<u64>) {
        let Ok(mut limits) = self.inner.limits.lock() else {
            return;
        };
        limits.download = download;
        limits.upload = upload;
        if let Some(session) = &limits.open {
            session.ratelimits.set_download_bps(bps(download));
            session.ratelimits.set_upload_bps(bps(upload));
        }
    }

    fn opening(&self, conn: &Connection) -> anyhow::Result<Opening> {
        Ok(Opening {
            listen: match self.inner.options.listen {
                Listen::Off => None,
                Listen::Setting => Some((Ipv6Addr::UNSPECIFIED, listen_port(conn)?).into()),
                Listen::Addr(addr) => Some(addr),
            },
            upnp: settings::flag(conn, settings::Flag::TorrentUpnp)?,
        })
    }

    /// La session, ouverte si aucune exécution ne s'en sert encore. Chaque `acquire`
    /// réussi demande son [`Torrents::release`].
    async fn acquire(&self, opening: Opening) -> anyhow::Result<Arc<Session>> {
        let mut slot = self.inner.slot.lock().await;
        if let Some(session) = &slot.session {
            let session = Arc::clone(session);
            slot.users += 1;
            return Ok(session);
        }
        let (download, upload) = self
            .inner
            .limits
            .lock()
            .map(|l| (l.download, l.upload))
            .unwrap_or_default();
        let state_dir = self.inner.state_dir.clone();
        let dht = self.inner.options.dht.then(|| DhtSessionConfig {
            persistence: Some(DhtPersistenceConfig {
                config_filename: Some(state_dir.join("torrent-dht.json")),
                ..Default::default()
            }),
            ..Default::default()
        });
        let listen = opening.listen.map(|addr| ListenerOptions {
            mode: ListenerMode::TcpOnly,
            listen_addr: addr,
            enable_upnp_port_forwarding: opening.upnp,
            ..Default::default()
        });
        let session = Session::new_with_opts(
            state_dir,
            SessionOptions {
                dht,
                listen,
                // La vérité vient des données (SPEC §7.16).
                fastresume: false,
                persistence: None,
                disable_local_service_discovery: true,
                ratelimits: LimitsConfig {
                    download_bps: bps(download),
                    upload_bps: bps(upload),
                },
                client_name_and_version: Some(format!(
                    "AuroDownloader {}",
                    env!("CARGO_PKG_VERSION")
                )),
                ..Default::default()
            },
        )
        .await?;
        if let Ok(mut limits) = self.inner.limits.lock() {
            limits.open = Some(Arc::clone(&session));
            // Une limite changée pendant l'ouverture.
            session.ratelimits.set_download_bps(bps(limits.download));
            session.ratelimits.set_upload_bps(bps(limits.upload));
        }
        slot.session = Some(Arc::clone(&session));
        slot.users = 1;
        Ok(session)
    }

    /// Une exécution a fini de se servir de la session ; la dernière la ferme.
    async fn release(&self) {
        let mut slot = self.inner.slot.lock().await;
        slot.users = slot.users.saturating_sub(1);
        if slot.users > 0 {
            return;
        }
        if let Some(session) = slot.session.take() {
            if let Ok(mut limits) = self.inner.limits.lock() {
                limits.open = None;
            }
            session.stop().await;
        }
    }
}

/// Port d'écoute : celui du réglage, ou un port tiré au hasard entre 49152 et 65535,
/// enregistré pour les fois suivantes.
fn listen_port(conn: &Connection) -> anyhow::Result<u16> {
    if let Some(port) = settings::get(conn, LISTEN_PORT)?
        .and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .filter(|&p| p > 0)
    {
        return Ok(port);
    }
    let random = u16::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..2].try_into()?);
    let port = 49152 + random % (65535 - 49152 + 1);
    settings::set(conn, LISTEN_PORT, &Value::from(port))?;
    Ok(port)
}

/// Ce qu'un torrent donne à l'ajout (`task.add`) : son empreinte d'information, son nom et
/// son lien magnet. Un fichier `.torrent` invalide, ou un torrent v2 seul, est refusé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub info_hash: String,
    pub name: String,
    pub magnet: String,
}

pub fn inspect_metainfo(metainfo: &[u8]) -> Result<Added, String> {
    let layout = Layout::parse(metainfo)?;
    let magnet = Magnet::from_id20(layout.info_hash_id, layout.trackers.clone(), None);
    Ok(Added {
        info_hash: layout.info_hash.clone(),
        name: layout.top.clone(),
        magnet: with_name(&magnet.to_string(), &layout.top),
    })
}

pub fn inspect_magnet(link: &str) -> Result<Added, String> {
    let magnet = Magnet::parse(link).map_err(|e| format!("lien magnet invalide : {e}"))?;
    let id = magnet.as_id20().ok_or_else(|| {
        "lien magnet sans empreinte BitTorrent v1 (torrents v2 non pris en charge)".to_owned()
    })?;
    let info_hash = id.as_string();
    let name = magnet
        .name
        .as_deref()
        .map(|n| sanitize(n, NameRules::native()))
        .unwrap_or_else(|| info_hash.clone());
    Ok(Added {
        info_hash,
        name,
        magnet: link.to_owned(),
    })
}

/// Ajoute le nom (`dn`) à un lien magnet produit par librqbit, qui ne le met pas.
fn with_name(magnet: &str, name: &str) -> String {
    if magnet.contains("&dn=") || magnet.contains("?dn=") {
        return magnet.to_owned();
    }
    let encoded: String =
        percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC).collect();
    format!("{magnet}&dn={encoded}")
}

/// Disposition d'un torrent sur le disque.
#[derive(Debug, Clone)]
struct Layout {
    info_hash: String,
    info_hash_id: librqbit::dht::Id20,
    trackers: Vec<String>,
    /// Nom du premier élément, nettoyé : le fichier d'un torrent à un fichier, le dossier
    /// d'un torrent à plusieurs.
    top: String,
    multi: bool,
    files: Vec<LayoutFile>,
}

#[derive(Debug, Clone)]
struct LayoutFile {
    /// Composants nettoyés du chemin dans le torrent (vide pour un torrent à un fichier).
    components: Vec<String>,
    length: u64,
    /// Fichier de remplissage (BEP 47) : jamais écrit.
    padding: bool,
}

impl Layout {
    fn parse(metainfo: &[u8]) -> Result<Layout, String> {
        let torrent = librqbit::torrent_from_bytes(metainfo)
            .map_err(|e| format!("fichier .torrent invalide : {e}"))?;
        let info_hash_id = torrent.info_hash;
        let info_hash = info_hash_id.as_string();
        let trackers = torrent
            .iter_announce()
            .filter_map(|t| std::str::from_utf8(t.as_ref()).ok().map(str::to_owned))
            .collect();
        let multi = torrent.info.data.files.is_some();
        let info = torrent
            .info
            .data
            .validate()
            .map_err(|e| format!("fichier .torrent invalide : {e}"))?;
        // Une empreinte par morceau, ni plus ni moins : librqbit ne le vérifie pas, et un
        // torrent qui en manque échoue plus tard, à la vérification.
        let expected = usize::try_from(info.lengths().total_pieces()).unwrap_or(usize::MAX);
        if info.info().pieces.as_ref().len() != expected.saturating_mul(20) {
            return Err(
                "fichier .torrent invalide : nombre d'empreintes de morceaux faux".to_owned(),
            );
        }
        let rules = NameRules::native();
        let top = info
            .name()
            .map(|n| sanitize(&n, rules))
            .unwrap_or_else(|| info_hash.clone());
        let files = info
            .iter_file_details()
            .map(|details| LayoutFile {
                components: if multi {
                    details
                        .filename
                        .to_vec()
                        .iter()
                        .map(|c| sanitize(c, rules))
                        .collect()
                } else {
                    Vec::new()
                },
                length: details.len,
                padding: details.attrs().padding,
            })
            .collect();
        Ok(Layout {
            info_hash,
            info_hash_id,
            trackers,
            top,
            multi,
            files,
        })
    }

    /// Chemin final de chaque fichier, par rang, sous `dest/top` ; `None` pour un fichier
    /// de remplissage.
    fn final_paths(&self, dest: &Path, top: &str) -> Vec<Option<PathBuf>> {
        self.files
            .iter()
            .map(|f| {
                if f.padding {
                    return None;
                }
                let mut path = dest.join(top);
                for c in &f.components {
                    path.push(c);
                }
                Some(path)
            })
            .collect()
    }

    /// Les fichiers à enregistrer, sans ceux de remplissage, tous choisis.
    fn task_files(&self, top: &str) -> Vec<TorrentFile> {
        self.files
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.padding)
            .map(|(i, f)| TorrentFile {
                index: u32::try_from(i).unwrap_or(u32::MAX),
                path: if self.multi {
                    f.components.join("/")
                } else {
                    top.to_owned()
                },
                length: f.length,
                selected: true,
                done_bytes: 0,
            })
            .collect()
    }

    fn total(&self) -> u64 {
        self.files
            .iter()
            .filter(|f| !f.padding)
            .map(|f| f.length)
            .sum()
    }
}

#[derive(Debug)]
enum Failure {
    /// La session ne démarre pas (port pris, réseau) : nouvel essai plus tard.
    Session(String),
    /// Métadonnées invalides, erreur de la bibliothèque.
    Torrent(String),
    /// Écriture ou renommage sur le disque.
    Disk(std::io::Error),
}

impl Failure {
    fn code(&self) -> &'static str {
        match self {
            Failure::Session(_) => "TORRENT_SESSION",
            Failure::Torrent(_) => "TORRENT_FAILED",
            Failure::Disk(e) if is_disk_full(e) => "DISK_FULL",
            Failure::Disk(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                "DEST_PERMISSION"
            }
            Failure::Disk(_) => "DISK_IO",
        }
    }

    fn detail(&self) -> String {
        match self {
            Failure::Session(e) | Failure::Torrent(e) => e.clone(),
            Failure::Disk(e) => e.to_string(),
        }
    }
}

fn is_disk_full(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::StorageFull
}

/// Ce qu'une exécution a fait de son torrent.
enum End {
    /// Fichiers en place ; tâche `completed` (enregistré).
    Completed(PathBuf),
    /// Arrêt demandé pendant le téléchargement.
    Stopped,
    /// Arrêt pendant le partage (moteur, réseau) : la tâche reste `seeding`.
    SeedingStopped,
    /// Échec pendant le téléchargement.
    Failed(Failure),
    /// Échec pendant le partage : les fichiers sont là, le partage s'arrête.
    SeedingFailed(Failure),
}

/// Exécute une tâche torrent jusqu'à sa fin ou son arrêt, et enregistre son issue, comme
/// `transfer::run` pour un téléchargement HTTP. Une tâche `seeding` (partage interrompu)
/// reprend son partage.
pub async fn run(
    conn: &mut Connection,
    torrents: &Torrents,
    task_id: &str,
    mut stop: watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Outcome, TransferError> {
    let task = load_task(conn, task_id)?;
    match task.state {
        TaskState::Queued | TaskState::RetryWait => {
            change_state(conn, task_id, &Change::to(TaskState::Active))?;
        }
        TaskState::Active | TaskState::Seeding => {}
        other => {
            return Err(TransferError::NotRunnable {
                task_id: task_id.to_owned(),
                state: other.as_sql(),
            });
        }
    }
    let seeding = task.state == TaskState::Seeding;
    let end = if stopping(&stop) {
        if seeding {
            End::SeedingStopped
        } else {
            End::Stopped
        }
    } else {
        let opened = match torrents.opening(conn) {
            Ok(opening) => torrents.acquire(opening).await,
            Err(e) => Err(e),
        };
        match opened {
            Ok(session) => {
                let end = drive(conn, torrents, &session, task_id, &mut stop, on_progress).await;
                torrents.release().await;
                end?
            }
            Err(e) if seeding => End::SeedingFailed(Failure::Session(format!("{e:#}"))),
            Err(e) => End::Failed(Failure::Session(format!("{e:#}"))),
        }
    };
    match end {
        End::Completed(path) => Ok(Outcome::Completed(path)),
        End::Stopped => {
            let change = Change {
                pause_reason: Some(PauseReason::User),
                ..Change::to(TaskState::Paused)
            };
            change_state(conn, task_id, &change)?;
            Ok(Outcome::Paused)
        }
        End::SeedingStopped => Ok(Outcome::Seeding),
        End::Failed(failure) => record_failure(conn, task_id, &failure),
        End::SeedingFailed(failure) => {
            eprintln!(
                "aurod : tâche {task_id} : partage arrêté : {} [{}]",
                failure.detail(),
                failure.code()
            );
            finish_sharing(conn, task_id)
        }
    }
}

/// Termine une tâche dont le partage s'arrête : ses fichiers sont en place.
pub fn finish_sharing(conn: &mut Connection, task_id: &str) -> Result<Outcome, TransferError> {
    let task = load_task(conn, task_id)?;
    let top = Path::new(&task.dest_dir).join(task.file_name.as_deref().unwrap_or_default());
    complete_task(
        conn,
        task_id,
        &top.to_string_lossy(),
        task.total_bytes.unwrap_or(task.done_bytes),
    )?;
    Ok(Outcome::Completed(top))
}

fn record_failure(
    conn: &mut Connection,
    task_id: &str,
    failure: &Failure,
) -> Result<Outcome, TransferError> {
    let (code, detail) = (failure.code(), failure.detail());
    let error = Some((code, detail.as_str()));
    let task = load_task(conn, task_id)?;
    if code == "DISK_FULL" {
        let change = Change {
            pause_reason: Some(PauseReason::DiskFull),
            error,
            ..Change::to(TaskState::Paused)
        };
        change_state(conn, task_id, &change)?;
        return Ok(Outcome::DiskFull);
    }
    if matches!(failure, Failure::Session(_)) && task.retry_count + 1 < MAX_FAILURES {
        let delay = retry_delay(task.retry_count, None, Duration::ZERO);
        let at_ms = now_ms().saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX));
        let change = Change {
            error,
            next_retry_at: Some(at_ms),
            ..Change::to(TaskState::RetryWait)
        };
        change_state(conn, task_id, &change)?;
        return Ok(Outcome::RetryLater {
            code,
            detail,
            at_ms,
        });
    }
    let change = Change {
        error,
        ..Change::to(TaskState::Failed)
    };
    change_state(conn, task_id, &change)?;
    Ok(Outcome::Failed { code, detail })
}

/// Arrêt demandé : valeur vraie, ou plus personne pour le demander.
fn stopping(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow() || stop.has_changed().is_err()
}

/// Avancement d'une exécution, d'un passage à l'autre.
struct Counters {
    /// Octets des morceaux vérifiés des fichiers choisis.
    done: u64,
    /// Taille des fichiers choisis.
    total: u64,
    /// Octets envoyés, cumulés depuis l'ajout du torrent.
    uploaded: u64,
    /// Octets à envoyer avant de finir le partage ; `None` : sans limite.
    target: Option<u64>,
}

async fn drive(
    conn: &mut Connection,
    torrents: &Torrents,
    session: &Arc<Session>,
    task_id: &str,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<End, TransferError> {
    let peers = torrents.inner.options.initial_peers.clone();
    let seeding = load_task(conn, task_id)?.state == TaskState::Seeding;
    let failed = |f: Failure| {
        if seeding {
            End::SeedingFailed(f)
        } else {
            End::Failed(f)
        }
    };

    // Métadonnées : celles du fichier .torrent, ou celles d'un lien magnet, demandées aux
    // pairs puis gardées.
    let row = load_torrent(conn, task_id)?;
    let metainfo = match row.metainfo.clone() {
        Some(m) => Bytes::from(m),
        None => {
            let magnet = match row.magnet.clone() {
                Some(m) => m,
                None => primary_source(conn, task_id)?.url,
            };
            match resolve(session, &magnet, &peers, stop).await {
                Ok(Some(bytes)) => {
                    save_metainfo(conn, task_id, &bytes)?;
                    bytes
                }
                Ok(None) => return Ok(End::Stopped),
                Err(e) => return Ok(failed(Failure::Torrent(e))),
            }
        }
    };
    let layout = match Layout::parse(&metainfo) {
        Ok(l) => l,
        Err(e) => return Ok(failed(Failure::Torrent(e))),
    };

    // Préparation : nom du premier élément (libre dans le dossier), fichiers, taille.
    let mut task = load_task(conn, task_id)?;
    if task.file_name.is_none() {
        if let Err(e) = prepare(conn, &task, &layout) {
            return match e {
                PrepareError::Store(e) => Err(e.into()),
                PrepareError::Disk(e) => Ok(failed(Failure::Disk(e))),
            };
        }
        task = load_task(conn, task_id)?;
    }
    let top = task.file_name.clone().unwrap_or_else(|| layout.top.clone());
    let paths = layout.final_paths(Path::new(&task.dest_dir), &top);
    let total = task.total_bytes.unwrap_or_else(|| layout.total());
    let ratio = match row.ratio_limit {
        Some(r) => Some(r),
        None => settings::torrent_ratio(conn)?,
    };
    let mut counters = Counters {
        done: if seeding { total } else { task.done_bytes },
        total,
        uploaded: row.uploaded_bytes,
        target: ratio.map(|r| target_bytes(r, total)),
    };
    let pass = Pass {
        task_id,
        dest_dir: &task.dest_dir,
        session,
        metainfo: &metainfo,
        paths: &paths,
        peers: &peers,
        speed_limit: task.speed_limit_bps,
    };

    if !seeding {
        match pass
            .follow(conn, stop, on_progress, &mut counters, false)
            .await?
        {
            Passed::Reached => {}
            Passed::Stopped => return Ok(End::Stopped),
            Passed::Failed(f) => return Ok(End::Failed(f)),
        }
        // Fichiers choisis vidés sur disque, puis sous leur nom final.
        let selected = selected_files(conn, task_id)?;
        for (i, path) in paths.iter().enumerate() {
            if let Some(path) = path
                && selected.contains(&i)
                && let Err(e) = seal(path)
            {
                return Ok(End::Failed(Failure::Disk(e)));
            }
        }
        counters.total = load_task(conn, task_id)?
            .total_bytes
            .unwrap_or(counters.total);
        counters.done = counters.total;
        counters.target = ratio.map(|r| target_bytes(r, counters.total));
        if counters.target == Some(0) {
            return complete(conn, task_id, &task.dest_dir, &top, counters.total);
        }
        // Partage (décision de l'utilisateur du 2026-11-09 : jusqu'au ratio 1).
        change_state(conn, task_id, &Change::to(TaskState::Seeding))?;
        on_progress(counters.progress(0, None, true));
    }
    match pass
        .follow(conn, stop, on_progress, &mut counters, true)
        .await?
    {
        Passed::Reached => complete(conn, task_id, &task.dest_dir, &top, counters.total),
        Passed::Stopped => Ok(End::SeedingStopped),
        Passed::Failed(f) => Ok(End::SeedingFailed(f)),
    }
}

/// Octets à envoyer pour un ratio et une taille.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn target_bytes(ratio: f64, total: u64) -> u64 {
    (ratio * total as f64).ceil() as u64
}

fn complete(
    conn: &mut Connection,
    task_id: &str,
    dest_dir: &str,
    top: &str,
    total: u64,
) -> Result<End, TransferError> {
    let path = Path::new(dest_dir).join(top);
    complete_task(conn, task_id, &path.to_string_lossy(), total)?;
    Ok(End::Completed(path))
}

/// Rangs des fichiers choisis.
fn selected_files(conn: &Connection, task_id: &str) -> Result<HashSet<usize>, TransferError> {
    Ok(load_files(conn, task_id)?
        .iter()
        .filter(|f| f.selected)
        .filter_map(|f| usize::try_from(f.index).ok())
        .collect())
}

impl Counters {
    fn progress(&self, connections: usize, upload_bps: Option<u64>, seeding: bool) -> Progress {
        Progress {
            received: self.done,
            total: Some(self.total),
            connections,
            segments: Arc::from([]),
            verified: None,
            uploaded: Some(self.uploaded),
            upload_bps,
            seeding,
        }
    }
}

/// Un passage du torrent dans la session : téléchargement jusqu'à la fin des fichiers
/// choisis, ou partage jusqu'au ratio.
struct Pass<'a> {
    task_id: &'a str,
    dest_dir: &'a str,
    session: &'a Arc<Session>,
    metainfo: &'a Bytes,
    paths: &'a [Option<PathBuf>],
    peers: &'a [SocketAddr],
    speed_limit: Option<u64>,
}

enum Passed {
    /// Fichiers choisis complets, ou ratio atteint.
    Reached,
    Stopped,
    Failed(Failure),
}

/// Les fichiers à télécharger pour librqbit ; `None` : tous.
fn only_files(selected: &HashSet<usize>, paths: &[Option<PathBuf>]) -> Option<Vec<usize>> {
    let all = paths
        .iter()
        .enumerate()
        .all(|(i, p)| p.is_none() || selected.contains(&i));
    if all {
        return None;
    }
    let mut list: Vec<usize> = selected.iter().copied().collect();
    list.sort_unstable();
    Some(list)
}

impl Pass<'_> {
    async fn follow(
        &self,
        conn: &mut Connection,
        stop: &mut watch::Receiver<bool>,
        on_progress: &mut (dyn FnMut(Progress) + Send),
        counters: &mut Counters,
        sharing: bool,
    ) -> Result<Passed, TransferError> {
        let task_id = self.task_id;
        let factory = storage::Factory::new(self.paths.to_vec());
        let first_error = factory.error();
        let mut selected = selected_files(conn, task_id)?;
        let options = AddTorrentOptions {
            overwrite: true,
            output_folder: Some(self.dest_dir.to_owned()),
            storage_factory: Some(factory.clone().boxed()),
            only_files: only_files(&selected, self.paths),
            initial_peers: (!self.peers.is_empty()).then(|| self.peers.to_vec()),
            ratelimits: LimitsConfig {
                upload_bps: None,
                download_bps: bps(self.speed_limit),
            },
            ..Default::default()
        };
        let handle = match self
            .session
            .add_torrent(AddTorrent::from_bytes(self.metainfo.clone()), Some(options))
            .await
        {
            Ok(AddTorrentResponse::Added(_, handle)) => handle,
            Ok(AddTorrentResponse::AlreadyManaged(..)) => {
                return Ok(Passed::Failed(Failure::Torrent(
                    "ce torrent est déjà en cours dans une autre tâche".into(),
                )));
            }
            Ok(AddTorrentResponse::ListOnly(_)) => {
                return Ok(Passed::Failed(Failure::Torrent(
                    "réponse inattendue de la bibliothèque".into(),
                )));
            }
            Err(e) => return Ok(Passed::Failed(Failure::Torrent(format!("{e:#}")))),
        };
        let id = handle.id();
        // Octets envoyés avant ce passage : librqbit compte depuis l'ajout.
        let before = counters.uploaded;

        let mut tick = tokio::time::interval(POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_saved = Instant::now();
        let passed = loop {
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        break Passed::Stopped;
                    }
                }
                _ = tick.tick() => {
                    if let Some(e) = first_error.lock().ok().and_then(|mut e| e.take()) {
                        break Passed::Failed(Failure::Disk(e));
                    }
                    let stats = handle.stats();
                    if let Some(e) = &stats.error {
                        break Passed::Failed(Failure::Torrent(e.clone()));
                    }
                    let live = stats.live.as_ref();
                    // Fichiers choisis changés en cours (`task.setFiles`).
                    if !sharing {
                        let now = selected_files(conn, task_id)?;
                        if now != selected {
                            if let Err(e) = self.session.update_only_files(&handle, &now).await {
                                eprintln!("aurod : tâche {task_id} : choix des fichiers : {e:#}");
                            }
                            selected = now;
                            counters.total =
                                load_task(conn, task_id)?.total_bytes.unwrap_or(counters.total);
                        }
                    }
                    // Pendant la vérification de départ, librqbit compte les octets lus : la
                    // tâche montre encore son dernier avancement connu.
                    if live.is_some() && !sharing {
                        counters.done = stats.progress_bytes.min(counters.total);
                    }
                    counters.uploaded = before.saturating_add(stats.uploaded_bytes);
                    let connections = live.map_or(0, |l| {
                        usize::try_from(l.snapshot.peer_stats.live).unwrap_or(usize::MAX)
                    });
                    let upload_bps = live.map(|l| l.upload_speed.as_bytes());
                    on_progress(counters.progress(connections, upload_bps, sharing));
                    if last_saved.elapsed() >= SAVE_EVERY {
                        let files = (!sharing).then_some(&stats.file_progress);
                        save_counters(conn, task_id, counters, files)?;
                        last_saved = Instant::now();
                    }
                    if !sharing && stats.finished {
                        break Passed::Reached;
                    }
                    if sharing && live.is_some() {
                        // Les fichiers terminés ont changé sur le disque : librqbit voudrait
                        // en retélécharger une partie. Le partage s'arrête là.
                        if !stats.finished {
                            break Passed::Failed(Failure::Torrent(
                                "fichiers changés depuis la fin du téléchargement".into(),
                            ));
                        }
                        if counters.target.is_some_and(|t| counters.uploaded >= t) {
                            break Passed::Reached;
                        }
                    }
                }
            }
        };
        // Retiré de la session, fichiers gardés : librqbit lâche le stockage, puis ses
        // lectures et écritures en cours finissent. Les fichiers ne se renomment ni ne
        // s'effacent qu'une fois tous fermés.
        if let Err(e) = self.session.delete(id.into(), false).await {
            eprintln!("aurod : tâche {task_id} : torrent non retiré de la session : {e:#}");
        }
        let file_progress = handle.stats().file_progress;
        drop(handle);
        if !factory.wait_closed(CLOSE_WAIT).await {
            eprintln!("aurod : tâche {task_id} : fichiers du torrent encore ouverts");
        }
        let files = (!sharing).then_some(&file_progress);
        save_counters(conn, task_id, counters, files)?;
        Ok(passed)
    }
}

/// Enregistre l'avancement (indicatif), les octets envoyés et, au téléchargement,
/// l'avancement de chaque fichier.
fn save_counters(
    conn: &mut Connection,
    task_id: &str,
    counters: &Counters,
    files: Option<&Vec<u64>>,
) -> Result<(), TransferError> {
    save_progress(conn, task_id, counters.done)?;
    save_uploaded(conn, task_id, counters.uploaded)?;
    if let Some(files) = files {
        let done: Vec<(u32, u64)> = files
            .iter()
            .enumerate()
            .filter_map(|(i, d)| u32::try_from(i).ok().map(|i| (i, *d)))
            .collect();
        save_file_progress(conn, task_id, &done)?;
    }
    Ok(())
}

/// Demande les métadonnées d'un lien magnet aux pairs ; `Ok(None)` si l'arrêt est demandé
/// avant.
async fn resolve(
    session: &Arc<Session>,
    magnet: &str,
    peers: &[SocketAddr],
    stop: &mut watch::Receiver<bool>,
) -> Result<Option<Bytes>, String> {
    let options = AddTorrentOptions {
        list_only: true,
        initial_peers: (!peers.is_empty()).then(|| peers.to_vec()),
        ..Default::default()
    };
    let asked = session.add_torrent(AddTorrent::from_url(magnet), Some(options));
    tokio::select! {
        answer = asked => match answer {
            Ok(AddTorrentResponse::ListOnly(list)) => Ok(Some(list.torrent_bytes)),
            Ok(_) => Err("réponse inattendue de la bibliothèque".into()),
            Err(e) => Err(format!("métadonnées du lien magnet introuvables : {e:#}")),
        },
        _ = wait_stop(stop) => Ok(None),
    }
}

async fn wait_stop(stop: &mut watch::Receiver<bool>) {
    loop {
        if stopping(stop) {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

enum PrepareError {
    Store(crate::store::StoreError),
    Disk(std::io::Error),
}

impl From<crate::store::StoreError> for PrepareError {
    fn from(e: crate::store::StoreError) -> Self {
        PrepareError::Store(e)
    }
}

/// Choisit le nom du premier élément et le prend aussitôt sur le disque (son dossier, ou
/// le `.part` de son fichier), comme `io::claim_name` : deux tâches ne peuvent pas finir
/// au même endroit. Puis enregistre les fichiers.
fn prepare(conn: &mut Connection, task: &TaskRow, layout: &Layout) -> Result<(), PrepareError> {
    let dir = Path::new(&task.dest_dir);
    let wanted = task
        .requested_name
        .clone()
        .unwrap_or_else(|| layout.top.clone());
    let reserved = reserved_names(conn, &task.dest_dir, &task.id)?;
    let mut last = None;
    let mut name = None;
    for _ in 0..1000 {
        let candidate = available_name(dir, &wanted, &reserved);
        let path = dir.join(&candidate);
        let claimed = if layout.multi {
            std::fs::create_dir(&path)
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(part_path(&path))
                .map(|_| ())
        };
        match claimed {
            Ok(()) => {
                name = Some(candidate);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(PrepareError::Disk(e)),
        }
    }
    let Some(name) = name else {
        return Err(PrepareError::Disk(
            last.unwrap_or_else(|| std::io::Error::other("aucun nom libre")),
        ));
    };
    let _ = sync_parent_dir(&dir.join(&name));
    save_preparation(
        conn,
        &task.id,
        &name,
        &name,
        layout.total(),
        &layout.task_files(&name),
    )?;
    Ok(())
}

/// Vide le `.part` de `path` sur disque et lui donne son nom final. Un fichier vide n'a
/// jamais été écrit : il est créé. Un fichier déjà renommé (arrêt pendant une fin
/// précédente) est seulement vidé.
fn seal(path: &Path) -> std::io::Result<()> {
    let part = part_path(path);
    match std::fs::OpenOptions::new().write(true).open(&part) {
        Ok(file) => {
            file.sync_all()?;
            drop(file);
            std::fs::rename(&part, path)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::OpenOptions::new().write(true).open(path) {
                Ok(file) => file.sync_all()?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    std::fs::File::create(path)?.sync_all()?;
                }
                Err(e) => return Err(e),
            }
        }
        Err(e) => return Err(e),
    }
    sync_parent_dir(path).map_err(|e| std::io::Error::other(e.to_string()))
}

/// Les fichiers d'une tâche torrent sur le disque, pour l'annulation et le retrait : avec
/// `finals`, les fichiers terminés aussi. Le dossier d'un torrent à plusieurs fichiers
/// part s'il est vide ensuite.
pub fn delete_files(conn: &Connection, task: &TaskRow, finals: bool) -> std::io::Result<()> {
    let Some(top) = &task.file_name else {
        return Ok(());
    };
    let files = crate::store::torrents::load_files(conn, &task.id)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let top_path = Path::new(&task.dest_dir).join(top);
    let multi = top_path.is_dir();
    for f in &files {
        let path = if multi {
            let mut p = top_path.clone();
            for c in f.path.split('/') {
                p.push(c);
            }
            p
        } else {
            top_path.clone()
        };
        remove(&part_path(&path))?;
        if finals {
            remove(&path)?;
        }
    }
    if multi {
        remove_empty_dirs(&top_path);
    }
    Ok(())
}

fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Supprime les dossiers vides sous `dir`, et `dir` lui-même s'il l'est.
fn remove_empty_dirs(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_dirs(&entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}
