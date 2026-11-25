//! Lecture bornée d'un fichier choisi par l'utilisateur (`.torrent`, Metalink, plugin, liste
//! de liens), commune aux clients : la CLI et l'application de bureau (audit C31).

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Échec de [`read_bounded`].
#[derive(Debug, thiserror::Error)]
pub enum BoundedReadError {
    /// Plus de `limit` octets : `len` au moins.
    #[error("fichier trop gros ({len} octets, {limit} au plus)")]
    TooLarge { len: u64, limit: u64 },
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Lit tout le fichier `path`, `limit` octets au plus. Plus gros, il est refusé avant la
/// lecture, et aussi s'il grandit pendant celle-ci : jamais plus de `limit + 1` octets en
/// mémoire.
pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, BoundedReadError> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > limit {
        return Err(BoundedReadError::TooLarge { len, limit });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    let read = bytes.len() as u64;
    if read > limit {
        return Err(BoundedReadError::TooLarge { len: read, limit });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_read_up_to_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("liste.txt");
        std::fs::write(&path, b"0123456789").unwrap();
        assert_eq!(read_bounded(&path, 10).unwrap(), b"0123456789");
        match read_bounded(&path, 9) {
            Err(BoundedReadError::TooLarge { len: 10, limit: 9 }) => {}
            other => panic!("{other:?}"),
        }
        let missing = read_bounded(&dir.path().join("absent"), 10);
        assert!(
            matches!(missing, Err(BoundedReadError::Io(e)) if e.kind() == io::ErrorKind::NotFound)
        );
    }
}
