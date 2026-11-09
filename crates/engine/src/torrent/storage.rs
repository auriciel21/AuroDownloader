//! Stockage d'un torrent (SPEC §7.16) : librqbit y lit et y écrit les morceaux, par
//! fichier et par position, dans les `.part` du moteur.
//!
//! Chaque fichier s'écrit sous `<nom>.part` jusqu'à la fin du torrent. Un arrêt entre le
//! renommage final et l'enregistrement de la fin laisse des fichiers sous leur nom final :
//! le stockage les reprend alors tels quels, et la vérification de départ refait le point.
//! Les fichiers s'ouvrent à leur premier usage ; une lecture d'un fichier absent échoue,
//! et librqbit tient alors ses morceaux pour manquants.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use librqbit::storage::{BoxStorageFactory, StorageFactory, StorageFactoryExt, TorrentStorage};
use librqbit::{ManagedTorrentShared, TorrentMetadata};

use crate::io::{part_path, read_exact_at, write_all_at};

/// Première erreur d'écriture d'un torrent, gardée pour en dire la cause (disque plein,
/// permission) : librqbit ne rend qu'un message.
pub(super) type FirstError = Arc<Mutex<Option<io::Error>>>;

/// Fabrique du stockage d'un torrent : le chemin final de chaque fichier, par rang ;
/// `None` pour un fichier de remplissage (BEP 47), jamais écrit.
#[derive(Clone)]
pub(super) struct Factory {
    files: Arc<Vec<Option<PathBuf>>>,
    error: FirstError,
    opened: Opened,
}

/// Fichiers ouverts par les stockages d'un torrent, pour savoir quand tous sont fermés.
type Opened = Arc<Mutex<Vec<Weak<File>>>>;

impl Factory {
    pub(super) fn new(files: Vec<Option<PathBuf>>) -> Self {
        Factory {
            files: Arc::new(files),
            error: FirstError::default(),
            opened: Opened::default(),
        }
    }

    pub(super) fn error(&self) -> FirstError {
        Arc::clone(&self.error)
    }

    /// Attend, au plus `limit`, que tous les fichiers ouverts pour ce torrent soient
    /// fermés : librqbit lâche son stockage peu après le retrait du torrent, et des lectures
    /// ou écritures en cours peuvent encore en tenir un. Sous Windows, un fichier ouvert ne
    /// se renomme ni ne s'efface tout à fait. Faux si le délai passe.
    pub(super) async fn wait_closed(&self, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        loop {
            let open = self
                .opened
                .lock()
                .map(|o| o.iter().any(|f| f.strong_count() > 0))
                .unwrap_or(false);
            if !open {
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl StorageFactory for Factory {
    type Storage = Storage;

    fn create(
        &self,
        _shared: &ManagedTorrentShared,
        _metadata: &TorrentMetadata,
    ) -> anyhow::Result<Storage> {
        Ok(Storage::new(
            Arc::clone(&self.files),
            Arc::clone(&self.error),
            Arc::clone(&self.opened),
        ))
    }

    fn clone_box(&self) -> BoxStorageFactory {
        self.clone().boxed()
    }
}

pub(super) struct Storage {
    files: Arc<Vec<Option<PathBuf>>>,
    /// Fichier ouvert, par rang ; ouvert à son premier usage, fermé par [`Storage::take`].
    open: Vec<RwLock<Option<Arc<File>>>>,
    error: FirstError,
    opened: Opened,
}

impl Storage {
    fn new(files: Arc<Vec<Option<PathBuf>>>, error: FirstError, opened: Opened) -> Self {
        let open = files.iter().map(|_| RwLock::new(None)).collect();
        Storage {
            files,
            open,
            error,
            opened,
        }
    }

    /// Le fichier `file_id`, ouvert au besoin : son `.part`, sinon le fichier final s'il est
    /// déjà renommé ; avec `create`, un `.part` neuf (et ses dossiers) s'il n'y a ni l'un
    /// ni l'autre. `Ok(None)` : fichier de remplissage.
    fn file(&self, file_id: usize, create: bool) -> io::Result<Option<Arc<File>>> {
        let (Some(slot), Some(path)) = (self.open.get(file_id), self.files.get(file_id)) else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "fichier inconnu"));
        };
        let Some(path) = path else {
            return Ok(None);
        };
        if let Some(file) = slot.read().map_err(|_| poisoned())?.as_ref() {
            return Ok(Some(Arc::clone(file)));
        }
        let mut slot = slot.write().map_err(|_| poisoned())?;
        if let Some(file) = slot.as_ref() {
            return Ok(Some(Arc::clone(file)));
        }
        let file = Arc::new(open(path, create)?);
        if let Ok(mut opened) = self.opened.lock() {
            opened.retain(|f| f.strong_count() > 0);
            opened.push(Arc::downgrade(&file));
        }
        *slot = Some(Arc::clone(&file));
        Ok(Some(file))
    }

    /// Garde la première erreur d'écriture, puis la rend à librqbit.
    fn failed(&self, e: io::Error) -> anyhow::Error {
        let message = anyhow::anyhow!("{e}");
        if let Ok(mut first) = self.error.lock()
            && first.is_none()
        {
            *first = Some(e);
        }
        message
    }
}

fn poisoned() -> io::Error {
    io::Error::other("verrou du stockage empoisonné")
}

/// Ouvre le `.part` de `path`, ou `path` lui-même s'il a déjà pris son nom final.
fn open(path: &Path, create: bool) -> io::Result<File> {
    let part = part_path(path);
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    match options.open(&part) {
        Ok(f) => return Ok(f),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        Err(_) => {}
    }
    match options.open(path) {
        Ok(f) => return Ok(f),
        Err(e) if e.kind() != io::ErrorKind::NotFound || !create => return Err(e),
        Err(_) => {}
    }
    if let Some(dir) = part.parent() {
        std::fs::create_dir_all(dir)?;
    }
    options.create(true).truncate(false).open(&part)
}

impl TorrentStorage for Storage {
    fn init(
        &mut self,
        _shared: &ManagedTorrentShared,
        _metadata: &TorrentMetadata,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn pread_exact(&self, file_id: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        match self.file(file_id, false)? {
            Some(file) => Ok(read_exact_at(&file, buf, offset)?),
            None => {
                buf.fill(0);
                Ok(())
            }
        }
    }

    fn pwrite_all(&self, file_id: usize, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        let file = match self.file(file_id, true) {
            Ok(Some(file)) => file,
            Ok(None) => return Ok(()),
            Err(e) => return Err(self.failed(e)),
        };
        write_all_at(&file, buf, offset).map_err(|e| self.failed(e))
    }

    // Les fichiers d'une tâche ne s'effacent que par le moteur (annulation, retrait).
    fn remove_file(&self, _file_id: usize, _filename: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn remove_directory_if_empty(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    /// Appelé au démarrage pour chaque fichier choisi : le `.part` prend sa taille finale,
    /// comme la réservation de la place (F-10).
    fn ensure_file_length(&self, file_id: usize, length: u64) -> anyhow::Result<()> {
        let file = match self.file(file_id, true) {
            Ok(Some(file)) => file,
            Ok(None) => return Ok(()),
            Err(e) => return Err(self.failed(e)),
        };
        if file.metadata()?.len() != length {
            file.set_len(length).map_err(|e| self.failed(e))?;
        }
        Ok(())
    }

    /// Pause d'un torrent : librqbit garde le stockage rendu, et celui-ci ferme ses
    /// fichiers.
    fn take(&self) -> anyhow::Result<Box<dyn TorrentStorage>> {
        for slot in &self.open {
            slot.write().map_err(|_| poisoned())?.take();
        }
        Ok(Box::new(Storage::new(
            Arc::clone(&self.files),
            Arc::clone(&self.error),
            Arc::clone(&self.opened),
        )))
    }
}
