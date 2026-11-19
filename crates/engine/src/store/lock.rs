//! Un seul moteur par base (audit R2) : verrou exclusif sur `<base>.lock`, pris avant
//! d'ouvrir la base, avant toute reprise, et gardé jusqu'à l'arrêt du moteur.
//!
//! Sans lui, un second `aurod` lancé en même temps (deux clients qui démarrent le moteur
//! ensemble, ou un client qui n'a pas pu joindre le premier) reprenait les tâches,
//! effaçait les noms réservés et ramassait les secrets avant de découvrir que le point
//! d'accès était pris : deux écrivains sur le même `.part`. Le système libère le verrou à
//! la mort du processus, même tué brutalement.
//!
//! Windows : fichier ouvert sans partage (`share_mode(0)`). Unix : `flock` exclusif.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use super::StoreError;

/// Verrou de la base, relâché quand il est lâché (ou à la mort du processus).
#[derive(Debug)]
pub struct DbLock {
    _file: File,
}

/// Chemin du verrou d'une base : `auro.db` → `auro.db.lock`.
pub fn lock_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// Prend le verrou de la base `db` ; [`StoreError::Locked`] si un autre moteur le tient.
pub fn acquire(db: &Path) -> Result<DbLock, StoreError> {
    let path = lock_path(db);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // ERROR_SHARING_VIOLATION : un autre processus a le fichier ouvert.
        const SHARING_VIOLATION: i32 = 32;
        options.share_mode(0);
        match options.open(&path) {
            Ok(file) => Ok(DbLock { _file: file }),
            Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) => {
                Err(StoreError::Locked { path })
            }
            Err(e) => Err(StoreError::Io(e)),
        }
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let file = options.open(&path)?;
        // SAFETY : `file` est ouvert ; `flock` ne lit ni n'écrit de mémoire.
        let done = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if done == 0 {
            return Ok(DbLock { _file: file });
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Err(StoreError::Locked { path })
        } else {
            Err(StoreError::Io(e))
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        Ok(DbLock {
            _file: options.open(&path)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_on_the_same_base_is_refused_until_the_first_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("auro.db");
        let first = acquire(&db).unwrap();
        assert!(matches!(acquire(&db), Err(StoreError::Locked { .. })));
        // Une autre base n'est pas gênée.
        let other = acquire(&dir.path().join("autre.db")).unwrap();
        drop(first);
        let again = acquire(&db).unwrap();
        drop((other, again));
        assert!(lock_path(&db).ends_with("auro.db.lock"));
    }
}
