//! Écriture positionnée dans le fichier `.part`, vidage sur disque et renommage final.
//! Voir SPEC §7.5, §5.6 (règle 1) et F-10.
//!
//! Toutes les connexions d'une tâche écrivent dans le même [`PartFile`], chacune à la
//! position de son segment, par un [`SegmentWriter`] qui tamponne les octets reçus. Aucun
//! verrou n'est partagé entre segments : chaque écrivain publie, par un compteur atomique,
//! le nombre d'octets qu'il a remis au système.
//!
//! Règle 1 : un compteur `written_bytes` ne s'enregistre qu'à partir d'un
//! [`SyncedProgress`], que seul ce module fabrique, après `sync_data` (`fdatasync` sous
//! Linux et Android, `FlushFileBuffers` sous Windows). Le compteur est relevé *avant* le
//! vidage : il ne compte donc que des octets que le vidage a couverts.
//!
//! Les vidages d'un même fichier sont groupés ([`SyncGate`]) : un seul à la fois, partagé
//! par tous ceux qui l'ont demandé pendant le précédent.
//!
//! Le fichier compte les octets remis au système depuis le dernier point de contrôle
//! ([`PartFile::checkpoint_after`]) : passé un seuil, il prévient la boucle de
//! coordination, qui avance le point suivant. Le disque écrit ainsi pendant la réception,
//! au lieu de tout recevoir avant le premier vidage (SPEC §7.5).
//!
//! L'espace disque (F-18) se mesure par [`free_space`] et [`space_needed`] (module
//! `space`), qui réserve aussi la place d'un fichier ([`preallocate`], F-10).
//!
//! Pas encore traité : le stockage Android par le sélecteur du système (SPEC §7.5).

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::net::filename::MAX_NAME_BYTES;
use crate::segmenter::Segment;

pub mod checksum;
mod space;
pub use space::{free_space, preallocate, space_needed};

/// Taille par défaut du tampon d'un écrivain : 256 Kio (SPEC §7.10).
pub const DEFAULT_WRITE_BUFFER: usize = 256 * 1024;

/// Durée au-delà de laquelle un écrivain écrit son tampon, même incomplet (SPEC §7.5) :
/// sur une connexion lente ou bridée, les octets reçus ne restent pas des secondes en
/// mémoire, hors des points de contrôle et de l'avancement affiché.
pub const MAX_HOLD: Duration = Duration::from_millis(500);

/// Erreurs du module d'écriture.
#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("erreur d'entrée-sortie sur {path} : {source}")]
    File {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("le fichier de destination {0} existe déjà")]
    DestinationExists(PathBuf),

    #[error("des écrivains de segments utilisent encore {0}")]
    StillInUse(PathBuf),

    #[error("le suivi du segment {idx} appartient à un autre fichier")]
    ForeignProgress { idx: u32 },

    #[error(
        "le segment {idx} ne peut pas finir à l'octet {end} : \
         {accepted} octets sont déjà reçus à partir de {start}"
    )]
    EndBeforeReceived {
        idx: u32,
        start: u64,
        end: u64,
        accepted: u64,
    },
}

impl IoError {
    /// Erreur système sous-jacente, s'il y en a une : sa nature (`StorageFull`,
    /// `PermissionDenied`…) donne le code d'erreur de la tâche (SPEC §6.8).
    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        match self {
            IoError::File { source, .. } => Some(source.kind()),
            _ => None,
        }
    }
}

fn file_error(path: &Path) -> impl FnOnce(io::Error) -> IoError + '_ {
    move |source| IoError::File {
        path: path.to_owned(),
        source,
    }
}

/// Chemin du fichier en cours de téléchargement : nom final suivi de `.part`.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

/// Les noms de fichier ignorent-ils la casse ? Sous Windows (NTFS), et sous Android, dont
/// le stockage partagé (`Download/AuroDownloader`) l'ignore aussi (audit S6).
pub(crate) const CASE_INSENSITIVE_NAMES: bool = cfg!(any(windows, target_os = "android"));

/// Deux noms désignent-ils le même fichier ([`CASE_INSENSITIVE_NAMES`]) ?
pub(crate) fn same_name(a: &str, b: &str) -> bool {
    fold_name(a) == fold_name(b)
}

/// Forme d'un nom pour comparer deux noms là où la casse ne compte pas. NTFS met chaque
/// caractère en majuscule (`ς` et `σ`, `ſ` et `s` sont le même nom) : majuscules d'abord,
/// puis minuscules, pour confondre au moins tout ce qu'il confond, quitte à refuser
/// quelques noms qu'il distinguerait (`ß` et `ss`) (audit V20). Ailleurs, le nom tel quel.
pub(crate) fn fold_name(name: &str) -> String {
    if CASE_INSENSITIVE_NAMES {
        name.chars()
            .flat_map(char::to_uppercase)
            .flat_map(char::to_lowercase)
            .collect()
    } else {
        name.to_owned()
    }
}

/// Nom libre dans `dir` pour un nouveau téléchargement : `name` si ni lui ni son `.part`
/// n'existent et qu'aucune autre tâche ne l'a réservé (`reserved`), sinon `nom (1).ext`,
/// `nom (2).ext`… Un fichier existant n'est jamais réutilisé ; [`PartFile::create`] et
/// [`PartFile::finish`] le refusent de toute façon. Pour un nouveau téléchargement,
/// passer par `crate::claims::claim`, qui note la réservation puis prend le nom sur
/// le disque.
pub fn available_name(dir: &Path, name: &str, reserved: &[String]) -> String {
    let free = |n: &str| {
        let path = dir.join(n);
        !path.exists() && !part_path(&path).exists() && !reserved.iter().any(|r| same_name(r, n))
    };
    if free(name) {
        return name.to_owned();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    (1u32..)
        .map(|i| {
            let suffix = format!(" ({i}){ext}");
            // Le nom reste dans la limite de longueur du nettoyage.
            let mut keep = MAX_NAME_BYTES.saturating_sub(suffix.len()).min(stem.len());
            while !stem.is_char_boundary(keep) {
                keep -= 1;
            }
            format!("{}{suffix}", &stem[..keep])
        })
        .find(|candidate| free(candidate))
        .expect("un nom libre finit par se trouver")
}

/// Octets d'un segment dont la présence sur disque est prouvée : relevés avant un vidage
/// réussi. Seul ce module en fabrique ; `store::segments::save_progress` n'accepte que
/// ce type pour écrire `written_bytes` (SPEC §5.6, règle 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncedProgress {
    idx: u32,
    written: u64,
}

impl SyncedProgress {
    /// Numéro du segment.
    pub fn idx(&self) -> u32 {
        self.idx
    }

    /// Octets écrits et vidés depuis le début du segment.
    pub fn written(&self) -> u64 {
        self.written
    }
}

/// Vidages groupés d'un fichier : un seul `sync_data` à la fois.
///
/// Plusieurs vidages simultanés du même fichier coûtent très cher : sous Linux (ext4,
/// mesuré sous WSL), quatre fils qui vident ensemble multiplient le temps système et
/// l'attente disque (`docs/MESURES.md`). Qui demande un vidage pendant qu'un autre est en
/// cours attend le suivant, qu'un seul fil fait pour tous. Règle 1 : un vidage commencé
/// après la demande couvre tout ce qui était écrit avant elle ; celui qui était déjà en
/// cours ne suffit donc pas.
#[derive(Debug, Default)]
struct SyncGate {
    state: Mutex<GateState>,
    finished: Condvar,
}

#[derive(Debug, Default)]
struct GateState {
    /// Numéro du dernier vidage commencé.
    started: u64,
    /// Numéro du dernier vidage réussi ; les vidages se suivent, jamais en même temps.
    succeeded: u64,
    running: bool,
    /// Premier vidage raté (genre et texte de l'erreur). Après lui, aucun vidage de ce
    /// descripteur ne prouve plus rien : sous Linux, l'erreur n'est rendue qu'une fois et
    /// les pages perdues sont marquées propres, si bien que le `fsync` suivant réussit sans
    /// qu'elles soient sur le disque (audit S1). La reprise rouvre le fichier.
    failure: Option<(io::ErrorKind, String)>,
}

impl SyncGate {
    fn lock(&self) -> MutexGuard<'_, GateState> {
        // Trois compteurs toujours cohérents : un verrou empoisonné reste utilisable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Revient quand un vidage commencé après l'appel a réussi ; `sync` fait ce vidage,
    /// si c'est à ce fil de le faire. Après un vidage raté, tous les appels, en cours ou à
    /// venir, échouent : la porte est définitivement fermée (voir `GateState::failure`).
    fn sync(&self, sync: impl Fn() -> io::Result<()>) -> io::Result<()> {
        let mut state = self.lock();
        // Premier vidage à commencer après cet appel.
        let needed = state.started + 1;
        loop {
            if let Some(e) = Self::failed(&state) {
                return Err(e);
            }
            if state.succeeded >= needed {
                return Ok(());
            }
            if state.running {
                state = self
                    .finished
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            state.running = true;
            state.started += 1;
            let mine = state.started;
            drop(state);
            let result = sync();
            state = self.lock();
            state.running = false;
            match &result {
                Ok(()) => state.succeeded = mine,
                Err(e) => state.failure = Some((e.kind(), e.to_string())),
            }
            self.finished.notify_all();
            // Ici `mine >= needed` : ce vidage a commencé après l'appel.
            return result;
        }
    }

    /// Erreur à rendre si un vidage a déjà raté.
    fn failed(state: &GateState) -> Option<io::Error> {
        state
            .failure
            .as_ref()
            .map(|(kind, text)| io::Error::new(*kind, format!("vidage précédent raté : {text}")))
    }

    /// Erreur du premier vidage raté, s'il y en a eu un.
    fn failure(&self) -> Option<io::Error> {
        Self::failed(&self.lock())
    }
}

/// Octets remis au système depuis le dernier point de contrôle, et seuil au-delà duquel
/// la boucle de coordination en demande un sans attendre son intervalle (SPEC §7.5).
#[derive(Debug)]
struct Volume {
    since: AtomicU64,
    /// `u64::MAX` : aucun seuil.
    threshold: AtomicU64,
    reached: tokio::sync::Notify,
}

impl Default for Volume {
    fn default() -> Self {
        Volume {
            since: AtomicU64::new(0),
            threshold: AtomicU64::new(u64::MAX),
            reached: tokio::sync::Notify::new(),
        }
    }
}

impl Volume {
    /// Compte `n` octets remis au système ; prévient une fois, au franchissement du seuil.
    fn add(&self, n: u64) {
        let before = self.since.fetch_add(n, Ordering::Relaxed);
        let threshold = self.threshold.load(Ordering::Relaxed);
        if before < threshold && before.saturating_add(n) >= threshold {
            // Sans attente en cours, la permission est gardée pour la prochaine.
            self.reached.notify_one();
        }
    }
}

/// Fichier `.part` d'une tâche, partagé par ses écrivains de segments.
#[derive(Debug)]
pub struct PartFile {
    file: Arc<File>,
    gate: Arc<SyncGate>,
    volume: Arc<Volume>,
    part: PathBuf,
    dest: PathBuf,
}

impl PartFile {
    /// Crée le `.part` de `dest` ; échoue s'il existe déjà, pour ne jamais écraser un
    /// téléchargement en cours.
    pub fn create(dest: &Path) -> Result<Self, IoError> {
        let part = part_path(dest);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(file_error(&part))?;
        // L'entrée du dossier doit survivre à un arrêt brutal, sinon les compteurs
        // enregistrés ensuite décriraient un fichier disparu.
        sync_parent_dir(&part)?;
        Ok(Self::from_parts(file, part, dest))
    }

    /// Rouvre le `.part` existant de `dest` pour une reprise (SPEC §7.4).
    pub fn open(dest: &Path) -> Result<Self, IoError> {
        let part = part_path(dest);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&part)
            .map_err(file_error(&part))?;
        Ok(Self::from_parts(file, part, dest))
    }

    fn from_parts(file: File, part: PathBuf, dest: &Path) -> Self {
        Self {
            file: Arc::new(file),
            gate: Arc::default(),
            volume: Arc::default(),
            part,
            dest: dest.to_owned(),
        }
    }

    /// Seuil du point de contrôle par volume : [`volume_reached`](Self::volume_reached)
    /// revient quand les écrivains ont remis `bytes` octets au système depuis le dernier
    /// point de contrôle. Sans appel, aucun seuil.
    pub fn checkpoint_after(&self, bytes: u64) {
        self.volume.threshold.store(bytes.max(1), Ordering::Relaxed);
    }

    /// Octets remis au système depuis le dernier point de contrôle.
    pub fn unsynced(&self) -> u64 {
        self.volume.since.load(Ordering::Relaxed)
    }

    /// Revient quand le seuil de [`checkpoint_after`](Self::checkpoint_after) a été
    /// franchi. Un franchissement survenu sans attente en cours est gardé ; il peut donc
    /// dater d'avant un point de contrôle : vérifier [`unsynced`](Self::unsynced).
    pub async fn volume_reached(&self) {
        self.volume.reached.notified().await;
    }

    /// Chemin du `.part`.
    pub fn path(&self) -> &Path {
        &self.part
    }

    /// Réserve la place de tout le fichier, `len` octets (F-10, [`preallocate`]) ;
    /// `false` si le système de fichiers ne le permet pas. Un manque de place est une
    /// erreur `StorageFull`, avant tout octet reçu.
    pub fn preallocate(&self, len: u64) -> Result<bool, IoError> {
        preallocate(&self.file, len).map_err(file_error(&self.part))
    }

    /// Taille actuelle du `.part` sur disque.
    pub fn size_on_disk(&self) -> Result<u64, IoError> {
        Ok(self.file.metadata().map_err(file_error(&self.part))?.len())
    }

    /// Écrivain pour `segment`, qui reprend après ses `written` octets déjà enregistrés.
    pub fn writer(&self, segment: &Segment, buffer: usize) -> SegmentWriter {
        SegmentWriter {
            file: Arc::clone(&self.file),
            gate: Arc::clone(&self.gate),
            volume: Arc::clone(&self.volume),
            part: self.part.clone(),
            idx: segment.idx,
            start: segment.start,
            bounds: Arc::new(Mutex::new(Bounds {
                end: segment.end,
                accepted: segment.written,
            })),
            issued: Arc::new(AtomicU64::new(segment.written)),
            buf: Vec::with_capacity(buffer),
            capacity: buffer.max(1),
            held_since: None,
        }
    }

    /// Point de contrôle (SPEC §7.5) : relève ce que chaque écrivain a remis au système,
    /// vide le fichier une fois (vidage groupé), puis renvoie ces relevés. Les écrivains
    /// continuent pendant ce temps ; ce qu'ils écrivent après le relevé attend le point
    /// suivant, et compte pour son seuil de volume.
    pub fn checkpoint(&self, progress: &[ProgressHandle]) -> Result<Vec<SyncedProgress>, IoError> {
        // Remis à zéro avant le relevé : un octet compté ici pour le point suivant est
        // peut-être déjà couvert par celui-ci, jamais l'inverse.
        self.volume.since.store(0, Ordering::Relaxed);
        let mut snapshot = Vec::with_capacity(progress.len());
        for p in progress {
            if !Arc::ptr_eq(&p.file, &self.file) {
                return Err(IoError::ForeignProgress { idx: p.idx });
            }
            snapshot.push(SyncedProgress {
                idx: p.idx,
                written: p.issued.load(Ordering::Acquire),
            });
        }
        self.gate
            .sync(|| self.file.sync_data())
            .map_err(file_error(&self.part))?;
        Ok(snapshot)
    }

    /// Fin du téléchargement (F-10) : vide le `.part`, le ferme et le renomme en `dest`.
    ///
    /// Refuse si un écrivain est encore ouvert, ou si `dest` existe : un fichier déjà
    /// présent n'est jamais écrasé. En cas de refus, le `.part` reste en place.
    pub fn finish(self) -> Result<PathBuf, IoError> {
        self.seal()?.finish()
    }

    /// Vide le `.part` complet et le ferme, sans le renommer : il ne change plus, et son
    /// empreinte peut se calculer avant qu'il prenne son nom (F-11, SPEC §7.9).
    ///
    /// Refuse si un écrivain est encore ouvert.
    pub fn seal(self) -> Result<SealedPart, IoError> {
        // Un vidage déjà raté : celui-ci pourrait réussir sans que tout soit sur le disque.
        if let Some(e) = self.gate.failure() {
            return Err(file_error(&self.part)(e));
        }
        let PartFile {
            file, part, dest, ..
        } = self;
        let file = Arc::try_unwrap(file).map_err(|_| IoError::StillInUse(part.clone()))?;
        file.sync_data().map_err(file_error(&part))?;
        // Windows refuse de renommer un fichier ouvert.
        drop(file);
        Ok(SealedPart { part, dest })
    }
}

/// `.part` complet, vidé sur disque et fermé, en attente de son renommage.
#[derive(Debug)]
pub struct SealedPart {
    part: PathBuf,
    dest: PathBuf,
}

impl SealedPart {
    /// `.part` de `dest`, déjà complet et vidé (tâche reprise en `verifying`).
    pub fn existing(dest: &Path) -> Self {
        SealedPart {
            part: part_path(dest),
            dest: dest.to_owned(),
        }
    }

    /// Chemin du `.part`.
    pub fn path(&self) -> &Path {
        &self.part
    }

    /// Renomme le `.part` en `dest`. Refuse si `dest` existe : un fichier déjà présent
    /// n'est jamais écrasé. En cas de refus, le `.part` reste en place.
    pub fn finish(self) -> Result<PathBuf, IoError> {
        let SealedPart { part, dest } = self;
        // Le moteur est seul à écrire dans ses dossiers de destination ; la fenêtre entre
        // ce test et le renommage ne concerne qu'un autre programme.
        if dest.try_exists().map_err(file_error(&dest))? {
            return Err(IoError::DestinationExists(dest));
        }
        mark_from_internet(&part);
        std::fs::rename(&part, &dest).map_err(file_error(&part))?;
        sync_parent_dir(&dest)?;
        Ok(dest)
    }
}

/// Marque du Web (Windows, audit C3) : le fichier vient d'Internet, comme s'il avait été
/// pris par un navigateur (flux `Zone.Identifier`, zone 3) ; SmartScreen et le Mode
/// protégé d'Office s'appliquent. Écrite sur le `.part` avant son renommage, qu'elle suit.
/// Ailleurs, rien. Au mieux : un volume sans flux nommés (FAT32, exFAT) ou un refus ne fait
/// pas échouer la tâche. L'adresse n'y est pas écrite : elle peut porter un jeton.
pub fn mark_from_internet(path: &Path) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_os_string();
        stream.push(":Zone.Identifier");
        if let Err(e) = std::fs::write(&stream, "[ZoneTransfer]\r\nZoneId=3\r\n") {
            tracing::debug!(erreur = %e, "marque du Web non écrite");
        }
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// Rend durable une création ou un renommage dans le dossier de `path`. Sous Windows, le
/// système de fichiers journalise les métadonnées et la bibliothèque standard ne sait pas
/// ouvrir un dossier : rien à faire.
pub(crate) fn sync_parent_dir(path: &Path) -> Result<(), IoError> {
    #[cfg(unix)]
    {
        let dir = match path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(file_error(dir))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Écriture positionnée de tout `buf` à `offset` (`pwrite`), sans curseur partagé.
#[cfg(unix)]
pub(crate) fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

/// Lecture positionnée de tout `buf` depuis `offset` (`pread`), sans curseur partagé ; un
/// fichier trop court rend `UnexpectedEof`.
#[cfg(unix)]
pub(crate) fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}

/// Lecture positionnée de tout `buf` depuis `offset`. Comme pour l'écriture, `seek_read`
/// déplace un curseur qui ne sert jamais ici.
#[cfg(windows)]
pub(crate) fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Écriture positionnée de tout `buf` à `offset`. `seek_write` déplace le curseur du
/// fichier, qui ne sert jamais ici : chaque écriture donne sa position.
#[cfg(windows)]
pub(crate) fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_write(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Borne de fin et octets acceptés d'un segment, partagés entre son écrivain et la boucle
/// de coordination, qui peut reculer la borne (redécoupage, SPEC §7.2). Le verrou est
/// propre au segment : aucun verrou n'est partagé entre segments.
#[derive(Debug)]
struct Bounds {
    end: u64,
    /// Octets acceptés depuis le début du segment : remis au système ou en tampon.
    accepted: u64,
}

fn lock(bounds: &Mutex<Bounds>) -> MutexGuard<'_, Bounds> {
    // Deux entiers toujours cohérents : un verrou empoisonné reste utilisable.
    bounds.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Recule la borne de fin à `end`, si aucun octet déjà accepté ne la dépasse.
fn shrink(bounds: &Mutex<Bounds>, idx: u32, start: u64, end: u64) -> Result<(), IoError> {
    let mut b = lock(bounds);
    if end > b.end || end < start || end + 1 < start + b.accepted {
        return Err(IoError::EndBeforeReceived {
            idx,
            start,
            end,
            accepted: b.accepted,
        });
    }
    b.end = end;
    Ok(())
}

/// Suivi d'un écrivain, partagé avec la boucle de coordination pendant que la connexion
/// écrit : points de contrôle ([`PartFile::checkpoint`]), avancement et redécoupage.
#[derive(Debug, Clone)]
pub struct ProgressHandle {
    file: Arc<File>,
    idx: u32,
    start: u64,
    bounds: Arc<Mutex<Bounds>>,
    issued: Arc<AtomicU64>,
}

impl ProgressHandle {
    /// Numéro du segment.
    pub fn idx(&self) -> u32 {
        self.idx
    }

    /// Premier octet du segment.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Octets remis au système depuis le début du segment, pas forcément vidés : pour
    /// l'affichage de l'avancement, jamais pour la base.
    pub fn handed_over(&self) -> u64 {
        self.issued.load(Ordering::Relaxed)
    }

    /// Octets acceptés par l'écrivain (remis au système ou en tampon), et borne de fin
    /// actuelle, lus ensemble.
    pub fn accepted_and_end(&self) -> (u64, u64) {
        let b = lock(&self.bounds);
        (b.accepted, b.end)
    }

    /// Recule la fin du segment à `end` (SPEC §7.2) : l'écrivain n'acceptera plus rien
    /// au-delà. Refusé si des octets déjà acceptés la dépassent.
    pub fn shrink_end(&self, end: u64) -> Result<(), IoError> {
        shrink(&self.bounds, self.idx, self.start, end)
    }
}

/// Écrivain d'un segment, propre à une connexion.
///
/// Les octets reçus passent par un tampon, écrit à sa position dès qu'il est plein, ou
/// quand il garde des octets depuis plus de [`MAX_HOLD`]. Un écrivain abandonné sans
/// [`finish`](Self::finish) perd son tampon, jamais compté.
#[derive(Debug)]
pub struct SegmentWriter {
    file: Arc<File>,
    gate: Arc<SyncGate>,
    volume: Arc<Volume>,
    part: PathBuf,
    idx: u32,
    start: u64,
    bounds: Arc<Mutex<Bounds>>,
    /// Octets remis au système depuis `start`, enregistrés compris.
    issued: Arc<AtomicU64>,
    buf: Vec<u8>,
    capacity: usize,
    /// Arrivée des plus anciens octets du tampon ; `None` s'il est vide.
    held_since: Option<Instant>,
}

impl SegmentWriter {
    /// Numéro du segment.
    pub fn idx(&self) -> u32 {
        self.idx
    }

    /// Octets acceptés depuis le début du segment : remis au système ou en tampon.
    pub fn accepted(&self) -> u64 {
        lock(&self.bounds).accepted
    }

    /// Octets qu'il reste à recevoir jusqu'à la fin du segment.
    pub fn remaining(&self) -> u64 {
        let b = lock(&self.bounds);
        (b.end - self.start + 1) - b.accepted
    }

    /// Suivi à confier à la boucle de coordination.
    pub fn progress(&self) -> ProgressHandle {
        ProgressHandle {
            file: Arc::clone(&self.file),
            idx: self.idx,
            start: self.start,
            bounds: Arc::clone(&self.bounds),
            issued: Arc::clone(&self.issued),
        }
    }

    /// Accepte au plus [`remaining`](Self::remaining) octets de `data` et renvoie leur
    /// nombre : au-delà de la fin du segment, les octets du serveur sont ignorés. Le
    /// tampon est écrit quand il est plein, qu'il garde des octets depuis plus de
    /// [`MAX_HOLD`], ou que le segment est complet.
    pub fn write(&mut self, data: &[u8]) -> Result<usize, IoError> {
        let (n, complete) = {
            let mut b = lock(&self.bounds);
            let remaining = (b.end - self.start + 1) - b.accepted;
            let n = usize::try_from(remaining).map_or(data.len(), |r| r.min(data.len()));
            b.accepted += n as u64;
            (n, b.accepted == b.end - self.start + 1)
        };
        self.buf.extend_from_slice(&data[..n]);
        let held = *self.held_since.get_or_insert_with(Instant::now);
        if self.buf.len() >= self.capacity || complete || held.elapsed() >= MAX_HOLD {
            self.flush()?;
        }
        Ok(n)
    }

    /// Écrit le tampon à sa position. En cas d'échec (disque plein…), le tampon est gardé
    /// pour un nouvel essai, qui réécrit les mêmes octets au même endroit.
    pub fn flush(&mut self) -> Result<(), IoError> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let issued = self.issued.load(Ordering::Relaxed);
        write_all_at(&self.file, &self.buf, self.start + issued).map_err(file_error(&self.part))?;
        // Publié après l'écriture : un relevé ne compte que des octets déjà remis.
        self.issued
            .store(issued + self.buf.len() as u64, Ordering::Release);
        self.volume.add(self.buf.len() as u64);
        self.buf.clear();
        self.held_since = None;
        Ok(())
    }

    /// Nouvelle fin du segment après un redécoupage (SPEC §7.2) : la connexion s'arrêtera
    /// à `end`. Refusé si des octets déjà reçus dépassent cette borne.
    #[cfg(test)]
    pub fn shrink_end(&mut self, end: u64) -> Result<(), IoError> {
        shrink(&self.bounds, self.idx, self.start, end)
    }

    /// Fin de la connexion (segment terminé, pause ou erreur réseau) : écrit le tampon,
    /// vide le fichier (vidage groupé) et renvoie le compteur à enregistrer (SPEC §7.5).
    pub fn finish(mut self) -> Result<SyncedProgress, IoError> {
        self.flush()?;
        let written = self.issued.load(Ordering::Acquire);
        self.gate
            .sync(|| self.file.sync_data())
            .map_err(file_error(&self.part))?;
        Ok(SyncedProgress {
            idx: self.idx,
            written,
        })
    }
}

/// Reprise, étape 1 (SPEC §7.4) : numéros des segments qui déclarent des octets écrits
/// au-delà de `file_len`, la taille du `.part` sur disque. Ils doivent repartir de zéro.
pub fn segments_beyond(file_len: u64, segments: &[Segment]) -> Vec<u32> {
    segments
        .iter()
        .filter(|s| s.written > 0 && s.start + s.written > file_len)
        .map(|s| s.idx)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segmenter::{SegmentState, initial_split};
    use proptest::prelude::*;

    /// Contenu attendu du fichier : un octet qui dépend de sa position.
    fn byte_at(offset: u64) -> u8 {
        (offset.wrapping_mul(31) ^ (offset >> 8)) as u8
    }

    fn pattern(start: u64, len: usize) -> Vec<u8> {
        (start..start + len as u64).map(byte_at).collect()
    }

    fn segment(idx: u32, start: u64, end: u64, written: u64) -> Segment {
        Segment {
            idx,
            start,
            end,
            written,
            state: SegmentState::Active,
        }
    }

    #[test]
    fn a_buffer_held_too_long_is_written_before_it_is_full() {
        let dir = tempfile::tempdir().unwrap();
        let part = PartFile::create(&dest_in(&dir)).unwrap();
        let mut writer = part.writer(&segment(0, 0, 1 << 20, 0), 256 * 1024);
        let progress = writer.progress();
        writer.write(&pattern(0, 1000)).unwrap();
        assert_eq!(progress.handed_over(), 0, "en tampon");
        std::thread::sleep(MAX_HOLD + Duration::from_millis(50));
        writer.write(&pattern(1000, 1000)).unwrap();
        assert_eq!(progress.handed_over(), 2000, "écrit, tampon pas plein");
        writer.write(&pattern(2000, 1000)).unwrap();
        assert_eq!(progress.handed_over(), 2000, "le délai repart de zéro");
        let synced = writer.finish().unwrap();
        assert_eq!(synced.written(), 3000);
    }

    /// Le seuil de volume a-t-il été signalé (sans attendre plus de 20 ms) ?
    async fn reached(part: &PartFile) -> bool {
        tokio::time::timeout(Duration::from_millis(20), part.volume_reached())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn crossing_the_volume_threshold_calls_for_a_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let part = PartFile::create(&dest_in(&dir)).unwrap();
        part.checkpoint_after(10_000);
        let mut writer = part.writer(&segment(0, 0, 99_999, 0), 4096);
        let progress = writer.progress();

        writer.write(&pattern(0, 8192)).unwrap();
        assert_eq!(part.unsynced(), 8192);
        assert!(!reached(&part).await, "seuil pas encore atteint");
        writer.write(&pattern(8192, 4096)).unwrap();
        assert!(reached(&part).await, "seuil franchi");
        assert!(!reached(&part).await, "un seul signal par franchissement");
        writer.write(&pattern(12_288, 8192)).unwrap();
        assert!(
            !reached(&part).await,
            "rien de plus avant le point de contrôle"
        );

        let synced = part.checkpoint(&[progress]).unwrap();
        assert_eq!(synced[0].written(), 20_480);
        assert_eq!(
            part.unsynced(),
            0,
            "le point de contrôle remet le compte à zéro"
        );
        writer.write(&pattern(20_480, 12_288)).unwrap();
        assert!(
            reached(&part).await,
            "nouveau franchissement après le point"
        );
        assert_eq!(writer.finish().unwrap().written(), 32_768);
    }

    #[tokio::test]
    async fn without_a_threshold_nothing_is_signalled() {
        let dir = tempfile::tempdir().unwrap();
        let part = PartFile::create(&dest_in(&dir)).unwrap();
        let mut writer = part.writer(&segment(0, 0, 99_999, 0), 4096);
        writer.write(&pattern(0, 100_000)).unwrap();
        assert_eq!(part.unsynced(), 100_000);
        assert!(!reached(&part).await);
    }

    fn dest_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("fichier.iso")
    }

    #[test]
    fn part_path_appends_the_extension() {
        assert_eq!(
            part_path(Path::new("dl/a.tar.gz")),
            PathBuf::from("dl/a.tar.gz.part")
        );
    }

    #[test]
    fn available_name_avoids_files_and_downloads_in_progress() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(available_name(dir.path(), "a.iso", &[]), "a.iso");
        std::fs::write(dir.path().join("a.iso"), b"").unwrap();
        assert_eq!(available_name(dir.path(), "a.iso", &[]), "a (1).iso");
        std::fs::write(dir.path().join("a (1).iso.part"), b"").unwrap();
        assert_eq!(available_name(dir.path(), "a.iso", &[]), "a (2).iso");
        std::fs::write(dir.path().join("LISEZMOI"), b"").unwrap();
        assert_eq!(available_name(dir.path(), "LISEZMOI", &[]), "LISEZMOI (1)");
        let long = "é".repeat(125);
        std::fs::write(dir.path().join(&long), b"").unwrap();
        let other = available_name(dir.path(), &long, &[]);
        assert!(
            other.len() <= MAX_NAME_BYTES && other.ends_with(" (1)"),
            "{other}"
        );
    }

    /// Un nom enregistré par une autre tâche reste à elle, même sans fichier sur le
    /// disque (`.part` pas encore créé, ou perdu).
    #[test]
    fn available_name_skips_names_reserved_by_other_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let reserved = vec!["b.iso".to_owned(), "b (1).iso".to_owned()];
        assert_eq!(available_name(dir.path(), "b.iso", &reserved), "b (2).iso");
        assert_eq!(available_name(dir.path(), "c.iso", &reserved), "c.iso");
        let other_case = vec!["B.ISO".to_owned()];
        let expected = if CASE_INSENSITIVE_NAMES {
            "b (1).iso"
        } else {
            "b.iso"
        };
        assert_eq!(available_name(dir.path(), "b.iso", &other_case), expected);
    }

    #[test]
    fn interleaved_segments_land_at_their_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dest_in(&dir);
        let part = PartFile::create(&dest).unwrap();
        let total = 10_000;
        let segments = initial_split(total, 3, 1);
        let mut writers: Vec<_> = segments.iter().map(|s| part.writer(s, 64)).collect();

        // Réception par petits morceaux, en alternant les connexions.
        let mut busy = true;
        while busy {
            busy = false;
            for w in &mut writers {
                if w.remaining() == 0 {
                    continue;
                }
                busy = true;
                let at = segments[w.idx() as usize].start + w.accepted();
                let n = w.write(&pattern(at, 37)).unwrap();
                assert!(n <= 37);
            }
        }
        let progress: Vec<_> = writers.into_iter().map(|w| w.finish().unwrap()).collect();
        for (p, s) in progress.iter().zip(&segments) {
            assert_eq!((p.idx(), p.written()), (s.idx, s.size()));
        }

        let final_path = part.finish().unwrap();
        assert_eq!(final_path, dest);
        assert!(!part_path(&dest).exists());
        assert_eq!(std::fs::read(&dest).unwrap(), pattern(0, total as usize));
    }

    #[test]
    fn bytes_past_the_segment_end_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let part = PartFile::create(&dest_in(&dir)).unwrap();
        let mut w = part.writer(&segment(0, 0, 9, 0), 4);
        assert_eq!(w.write(&pattern(0, 6)).unwrap(), 6);
        assert_eq!(w.write(&pattern(6, 100)).unwrap(), 4);
        let synced = part.checkpoint(&[w.progress()]).unwrap();
        assert_eq!(synced[0].written(), 10, "segment complet : tampon écrit");
        assert_eq!(w.write(b"x").unwrap(), 0);
        assert_eq!(w.finish().unwrap().written(), 10);
        assert_eq!(part.size_on_disk().unwrap(), 10);
    }

    #[test]
    fn checkpoint_counts_only_bytes_handed_to_the_system() {
        let dir = tempfile::tempdir().unwrap();
        let part = PartFile::create(&dest_in(&dir)).unwrap();
        let mut w = part.writer(&segment(0, 0, 999, 0), 100);
        let handle = w.progress();

        w.write(&pattern(0, 60)).unwrap();
        assert_eq!(w.accepted(), 60);
        let p = part.checkpoint(std::slice::from_ref(&handle)).unwrap();
        assert_eq!(p[0].written(), 0, "le tampon n'est pas encore sur disque");

        w.write(&pattern(60, 60)).unwrap(); // tampon plein : écrit
        let p = part.checkpoint(std::slice::from_ref(&handle)).unwrap();
        assert_eq!(p[0].written(), 120);

        // Connexion perdue sans finish : le relevé reste au dernier point de contrôle.
        w.write(&pattern(120, 10)).unwrap();
        drop(w);
        let p = part.checkpoint(&[handle]).unwrap();
        assert_eq!(p[0].written(), 120);
    }

    #[test]
    fn checkpoint_refuses_a_handle_of_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let a = PartFile::create(&dir.path().join("a")).unwrap();
        let b = PartFile::create(&dir.path().join("b")).unwrap();
        let w = b.writer(&segment(7, 0, 9, 0), 4);
        assert!(matches!(
            a.checkpoint(&[w.progress()]),
            Err(IoError::ForeignProgress { idx: 7 })
        ));
    }

    #[test]
    fn resumed_writer_continues_after_the_recorded_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dest_in(&dir);
        {
            let part = PartFile::create(&dest).unwrap();
            let mut w = part.writer(&segment(0, 0, 99, 0), 8);
            w.write(&pattern(0, 40)).unwrap();
            assert_eq!(w.finish().unwrap().written(), 40);
        }
        let part = PartFile::open(&dest).unwrap();
        assert_eq!(part.size_on_disk().unwrap(), 40);
        let mut w = part.writer(&segment(0, 0, 99, 40), 8);
        assert_eq!(w.remaining(), 60);
        w.write(&pattern(40, 60)).unwrap();
        assert_eq!(w.finish().unwrap().written(), 100);
        part.finish().unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), pattern(0, 100));
    }

    #[test]
    fn create_refuses_an_existing_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dest_in(&dir);
        let _part = PartFile::create(&dest).unwrap();
        let err = PartFile::create(&dest).unwrap_err();
        assert_eq!(err.io_kind(), Some(io::ErrorKind::AlreadyExists));
    }

    #[test]
    fn open_reports_a_missing_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = PartFile::open(&dest_in(&dir)).unwrap_err();
        assert_eq!(err.io_kind(), Some(io::ErrorKind::NotFound));
    }

    #[test]
    fn shrink_end_keeps_bytes_already_received() {
        let dir = tempfile::tempdir().unwrap();
        let part = PartFile::create(&dest_in(&dir)).unwrap();
        let mut w = part.writer(&segment(2, 100, 199, 10), 1000);
        w.write(&pattern(110, 20)).unwrap(); // reçus jusqu'à l'octet 129
        assert!(matches!(
            w.shrink_end(128),
            Err(IoError::EndBeforeReceived { accepted: 30, .. })
        ));
        assert!(w.shrink_end(250).is_err(), "une fin ne recule que");
        w.shrink_end(129).unwrap();
        assert_eq!(w.remaining(), 0);
        assert_eq!(w.write(b"x").unwrap(), 0);
    }

    /// Redécoupage vu de la boucle de coordination : la borne recule par le suivi pendant
    /// que la connexion écrit, et l'écrivain s'arrête à la nouvelle fin.
    #[test]
    fn handle_shrinks_a_running_writer() {
        let dir = tempfile::tempdir().unwrap();
        let part = PartFile::create(&dest_in(&dir)).unwrap();
        let mut w = part.writer(&segment(0, 0, 999, 0), 64);
        let handle = w.progress();
        w.write(&pattern(0, 100)).unwrap();
        assert_eq!(handle.accepted_and_end(), (100, 999));
        assert!(handle.shrink_end(98).is_err(), "100 octets déjà acceptés");
        handle.shrink_end(499).unwrap();
        assert_eq!(handle.accepted_and_end(), (100, 499));
        assert_eq!(w.remaining(), 400);
        assert_eq!(w.write(&pattern(100, 1000)).unwrap(), 400);
        assert_eq!(w.finish().unwrap().written(), 500);
        assert_eq!(std::fs::read(part.path()).unwrap(), pattern(0, 500));
    }

    #[test]
    fn finish_refuses_while_a_writer_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dest_in(&dir);
        let part = PartFile::create(&dest).unwrap();
        let _w = part.writer(&segment(0, 0, 9, 0), 4);
        assert!(matches!(part.finish(), Err(IoError::StillInUse(_))));
        assert!(part_path(&dest).exists());
        assert!(!dest.exists());
    }

    #[test]
    fn finish_never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dest_in(&dir);
        std::fs::write(&dest, b"ancien").unwrap();
        let part = PartFile::create(&dest).unwrap();
        let mut w = part.writer(&segment(0, 0, 2, 0), 4);
        w.write(b"neu").unwrap();
        w.finish().unwrap();
        assert!(matches!(part.finish(), Err(IoError::DestinationExists(_))));
        assert_eq!(std::fs::read(&dest).unwrap(), b"ancien");
        assert_eq!(std::fs::read(part_path(&dest)).unwrap(), b"neu");
    }

    #[test]
    fn segments_beyond_the_file_size_are_reported() {
        let segments = [
            segment(0, 0, 99, 100),
            segment(1, 100, 199, 50),
            segment(2, 200, 299, 0),
            segment(3, 300, 399, 1),
        ];
        assert_eq!(segments_beyond(400, &segments), Vec::<u32>::new());
        assert_eq!(segments_beyond(150, &segments), [3]);
        assert_eq!(segments_beyond(149, &segments), [1, 3]);
        assert_eq!(segments_beyond(0, &segments), [0, 1, 3]);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Règle 1, vue du fichier : à chaque point de contrôle, les octets que le relevé
        /// annonce pour chaque segment sont bien ceux du serveur, à leur place.
        #[test]
        fn checkpoints_never_announce_bytes_not_in_the_file(
            total in 1..20_000u64,
            connections in 1..=6u32,
            buffer in 1..600usize,
            steps in prop::collection::vec((any::<usize>(), 1..900usize, any::<bool>()), 1..80),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let dest = dest_in(&dir);
            let part = PartFile::create(&dest).unwrap();
            let segments = initial_split(total, connections, 1);
            let mut writers: Vec<_> = segments.iter().map(|s| part.writer(s, buffer)).collect();
            let handles: Vec<_> = writers.iter().map(SegmentWriter::progress).collect();

            for (pick, len, checkpoint) in steps {
                let k = pick % writers.len();
                let at = segments[k].start + writers[k].accepted();
                writers[k].write(&pattern(at, len)).unwrap();
                if checkpoint {
                    let synced = part.checkpoint(&handles).unwrap();
                    let file = std::fs::read(part.path()).unwrap();
                    for (p, s) in synced.iter().zip(&segments) {
                        prop_assert!(p.written() <= s.size());
                        if p.written() == 0 {
                            continue;
                        }
                        let from = s.start as usize;
                        let to = from + p.written() as usize;
                        prop_assert!(to <= file.len());
                        prop_assert_eq!(&file[from..to], &pattern(s.start, to - from)[..]);
                    }
                }
            }
        }
    }

    mod gate {
        use std::sync::Barrier;
        use std::sync::atomic::AtomicUsize;
        use std::thread;
        use std::time::Duration;

        use super::*;

        /// Chaque fil écrit, puis demande un vidage : à son retour, un vidage commencé
        /// après son écriture a réussi (règle 1), et les demandes simultanées se partagent
        /// quelques vidages au lieu d'en faire chacune un.
        #[test]
        fn simultaneous_requests_share_syncs_and_each_is_covered() {
            const THREADS: usize = 16;
            let gate = Arc::new(SyncGate::default());
            let written = Arc::new(AtomicU64::new(0));
            let durable = Arc::new(AtomicU64::new(0));
            let syncs = Arc::new(AtomicUsize::new(0));
            let barrier = Arc::new(Barrier::new(THREADS));
            let threads: Vec<_> = (0..THREADS)
                .map(|_| {
                    let (gate, written, durable) = (
                        Arc::clone(&gate),
                        Arc::clone(&written),
                        Arc::clone(&durable),
                    );
                    let (syncs, barrier) = (Arc::clone(&syncs), Arc::clone(&barrier));
                    thread::spawn(move || {
                        barrier.wait();
                        let mine = written.fetch_add(1, Ordering::SeqCst) + 1;
                        gate.sync(|| {
                            syncs.fetch_add(1, Ordering::SeqCst);
                            // Un vidage couvre ce qui était écrit quand il a commencé.
                            let covered = written.load(Ordering::SeqCst);
                            thread::sleep(Duration::from_millis(20));
                            durable.fetch_max(covered, Ordering::SeqCst);
                            Ok(())
                        })
                        .unwrap();
                        assert!(durable.load(Ordering::SeqCst) >= mine);
                    })
                })
                .collect();
            for t in threads {
                t.join().unwrap();
            }
            let n = syncs.load(Ordering::SeqCst);
            assert!(n <= THREADS / 2, "{n} vidages pour {THREADS} demandes");
        }

        /// Audit S1 : après un vidage raté, aucun vidage du même descripteur ne prouve plus
        /// rien (sous Linux, le suivant réussit sans les pages perdues). Toutes les
        /// demandes suivantes échouent, sans vider, avec le genre de la première erreur
        /// (disque plein : pause `disk_full`).
        #[test]
        fn a_failed_sync_fails_every_later_sync() {
            let gate = SyncGate::default();
            let calls = AtomicUsize::new(0);
            let failed = gate.sync(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::from(io::ErrorKind::StorageFull))
            });
            assert!(failed.is_err());
            for _ in 0..3 {
                let later = gate.sync(|| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                });
                assert_eq!(later.unwrap_err().kind(), io::ErrorKind::StorageFull);
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert!(gate.failure().is_some());
        }

        /// Un `.part` dont un vidage a raté ne se scelle pas : son vidage final pourrait
        /// réussir sans que tout soit sur le disque.
        #[test]
        fn a_part_whose_sync_failed_is_not_sealed() {
            let dir = tempfile::tempdir().unwrap();
            let part = PartFile::create(&dir.path().join("f.bin")).unwrap();
            let _ = part.gate.sync(|| Err(io::Error::other("disque")));
            assert!(part.seal().is_err());
            assert!(
                part_path(&dir.path().join("f.bin")).exists(),
                "le .part reste"
            );
        }

        /// Des demandes qui arrivent pendant un vidage raté échouent toutes, sans en
        /// refaire un.
        #[test]
        fn waiters_fail_after_a_failed_sync() {
            let gate = Arc::new(SyncGate::default());
            let attempts = Arc::new(AtomicUsize::new(0));
            let barrier = Arc::new(Barrier::new(4));
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    let (gate, attempts, barrier) = (
                        Arc::clone(&gate),
                        Arc::clone(&attempts),
                        Arc::clone(&barrier),
                    );
                    thread::spawn(move || {
                        barrier.wait();
                        // Le premier vidage échoue ; un second réussirait s'il était fait.
                        gate.sync(|| {
                            let n = attempts.fetch_add(1, Ordering::SeqCst);
                            thread::sleep(Duration::from_millis(20));
                            if n == 0 {
                                Err(io::Error::other("disque"))
                            } else {
                                Ok(())
                            }
                        })
                    })
                })
                .collect();
            for t in threads {
                assert!(t.join().unwrap().is_err());
            }
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
        }
    }
}
