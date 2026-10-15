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
//! Pas encore traités : la préallocation d'espace (F-10, en option), la vérification de
//! l'espace libre (F-18) et le stockage Android par le sélecteur du système (SPEC §7.5).

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::net::filename::MAX_NAME_BYTES;
use crate::segmenter::Segment;

/// Taille par défaut du tampon d'un écrivain : 256 Kio (SPEC §7.10).
pub const DEFAULT_WRITE_BUFFER: usize = 256 * 1024;

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

/// Nom libre dans `dir` pour un nouveau téléchargement : `name` si ni lui ni son `.part`
/// n'existent, sinon `nom (1).ext`, `nom (2).ext`… Un fichier existant n'est jamais
/// réutilisé ; [`PartFile::create`] et [`PartFile::finish`] le refusent de toute façon.
pub fn available_name(dir: &Path, name: &str) -> String {
    let free = |n: &str| {
        let path = dir.join(n);
        !path.exists() && !part_path(&path).exists()
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

/// Fichier `.part` d'une tâche, partagé par ses écrivains de segments.
#[derive(Debug)]
pub struct PartFile {
    file: Arc<File>,
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
            part,
            dest: dest.to_owned(),
        }
    }

    /// Chemin du `.part`.
    pub fn path(&self) -> &Path {
        &self.part
    }

    /// Taille actuelle du `.part` sur disque.
    pub fn size_on_disk(&self) -> Result<u64, IoError> {
        Ok(self.file.metadata().map_err(file_error(&self.part))?.len())
    }

    /// Écrivain pour `segment`, qui reprend après ses `written` octets déjà enregistrés.
    pub fn writer(&self, segment: &Segment, buffer: usize) -> SegmentWriter {
        SegmentWriter {
            file: Arc::clone(&self.file),
            part: self.part.clone(),
            idx: segment.idx,
            start: segment.start,
            end: segment.end,
            issued: Arc::new(AtomicU64::new(segment.written)),
            buf: Vec::with_capacity(buffer),
            capacity: buffer.max(1),
        }
    }

    /// Point de contrôle (SPEC §7.5) : relève ce que chaque écrivain a remis au système,
    /// vide le fichier une fois, puis renvoie ces relevés. Les écrivains continuent
    /// pendant ce temps ; ce qu'ils écrivent après le relevé attend le point suivant.
    pub fn checkpoint(&self, progress: &[ProgressHandle]) -> Result<Vec<SyncedProgress>, IoError> {
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
        self.file.sync_data().map_err(file_error(&self.part))?;
        Ok(snapshot)
    }

    /// Fin du téléchargement (F-10) : vide le `.part`, le ferme et le renomme en `dest`.
    ///
    /// Refuse si un écrivain est encore ouvert, ou si `dest` existe : un fichier déjà
    /// présent n'est jamais écrasé. En cas de refus, le `.part` reste en place.
    pub fn finish(self) -> Result<PathBuf, IoError> {
        let PartFile { file, part, dest } = self;
        let file = Arc::try_unwrap(file).map_err(|_| IoError::StillInUse(part.clone()))?;
        file.sync_data().map_err(file_error(&part))?;
        // Windows refuse de renommer un fichier ouvert.
        drop(file);

        // Le moteur est seul à écrire dans ses dossiers de destination ; la fenêtre entre
        // ce test et le renommage ne concerne qu'un autre programme.
        if dest.try_exists().map_err(file_error(&dest))? {
            return Err(IoError::DestinationExists(dest));
        }
        std::fs::rename(&part, &dest).map_err(file_error(&part))?;
        sync_parent_dir(&dest)?;
        Ok(dest)
    }
}

/// Rend durable une création ou un renommage dans le dossier de `path`. Sous Windows, le
/// système de fichiers journalise les métadonnées et la bibliothèque standard ne sait pas
/// ouvrir un dossier : rien à faire.
fn sync_parent_dir(path: &Path) -> Result<(), IoError> {
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
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

/// Écriture positionnée de tout `buf` à `offset`. `seek_write` déplace le curseur du
/// fichier, qui ne sert jamais ici : chaque écriture donne sa position.
#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
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

/// Suivi d'un écrivain, pour [`PartFile::checkpoint`] : se partage avec la tâche qui fait
/// les points de contrôle pendant que la connexion écrit.
#[derive(Debug, Clone)]
pub struct ProgressHandle {
    file: Arc<File>,
    idx: u32,
    issued: Arc<AtomicU64>,
}

impl ProgressHandle {
    /// Numéro du segment.
    pub fn idx(&self) -> u32 {
        self.idx
    }

    /// Octets remis au système depuis le début du segment, pas forcément vidés : pour
    /// l'affichage de l'avancement, jamais pour la base.
    pub fn handed_over(&self) -> u64 {
        self.issued.load(Ordering::Relaxed)
    }
}

/// Écrivain d'un segment, propre à une connexion.
///
/// Les octets reçus passent par un tampon, écrit à sa position dès qu'il est plein. Un
/// écrivain abandonné sans [`finish`](Self::finish) perd son tampon, jamais compté.
#[derive(Debug)]
pub struct SegmentWriter {
    file: Arc<File>,
    part: PathBuf,
    idx: u32,
    start: u64,
    end: u64,
    /// Octets remis au système depuis `start`, enregistrés compris.
    issued: Arc<AtomicU64>,
    buf: Vec<u8>,
    capacity: usize,
}

impl SegmentWriter {
    /// Numéro du segment.
    pub fn idx(&self) -> u32 {
        self.idx
    }

    /// Octets acceptés depuis le début du segment : remis au système ou en tampon.
    pub fn accepted(&self) -> u64 {
        self.issued.load(Ordering::Relaxed) + self.buf.len() as u64
    }

    /// Octets qu'il reste à recevoir jusqu'à la fin du segment.
    pub fn remaining(&self) -> u64 {
        (self.end - self.start + 1) - self.accepted()
    }

    /// Suivi à confier à la tâche des points de contrôle.
    pub fn progress(&self) -> ProgressHandle {
        ProgressHandle {
            file: Arc::clone(&self.file),
            idx: self.idx,
            issued: Arc::clone(&self.issued),
        }
    }

    /// Accepte au plus [`remaining`](Self::remaining) octets de `data` et renvoie leur
    /// nombre : au-delà de la fin du segment, les octets du serveur sont ignorés. Le
    /// tampon est écrit quand il est plein ou que le segment est complet.
    pub fn write(&mut self, data: &[u8]) -> Result<usize, IoError> {
        let n = usize::try_from(self.remaining()).map_or(data.len(), |r| r.min(data.len()));
        self.buf.extend_from_slice(&data[..n]);
        if self.buf.len() >= self.capacity || self.remaining() == 0 {
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
        self.buf.clear();
        Ok(())
    }

    /// Nouvelle fin du segment après un redécoupage (SPEC §7.2) : la connexion s'arrêtera
    /// à `end`. Refusé si des octets déjà reçus dépassent cette borne.
    pub fn shrink_end(&mut self, end: u64) -> Result<(), IoError> {
        let accepted = self.accepted();
        if end > self.end || end < self.start || end + 1 < self.start + accepted {
            return Err(IoError::EndBeforeReceived {
                idx: self.idx,
                start: self.start,
                end,
                accepted,
            });
        }
        self.end = end;
        Ok(())
    }

    /// Fin de la connexion (segment terminé, pause ou erreur réseau) : écrit le tampon,
    /// vide le fichier et renvoie le compteur à enregistrer (SPEC §7.5).
    pub fn finish(mut self) -> Result<SyncedProgress, IoError> {
        self.flush()?;
        let written = self.issued.load(Ordering::Acquire);
        self.file.sync_data().map_err(file_error(&self.part))?;
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
        assert_eq!(available_name(dir.path(), "a.iso"), "a.iso");
        std::fs::write(dir.path().join("a.iso"), b"").unwrap();
        assert_eq!(available_name(dir.path(), "a.iso"), "a (1).iso");
        std::fs::write(dir.path().join("a (1).iso.part"), b"").unwrap();
        assert_eq!(available_name(dir.path(), "a.iso"), "a (2).iso");
        std::fs::write(dir.path().join("LISEZMOI"), b"").unwrap();
        assert_eq!(available_name(dir.path(), "LISEZMOI"), "LISEZMOI (1)");
        let long = "é".repeat(125);
        std::fs::write(dir.path().join(&long), b"").unwrap();
        let other = available_name(dir.path(), &long);
        assert!(
            other.len() <= MAX_NAME_BYTES && other.ends_with(" (1)"),
            "{other}"
        );
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
}
