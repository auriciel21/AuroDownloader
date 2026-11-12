//! Réservation d'un nom de fichier sur le disque, sûre face à un arrêt brutal (SPEC §5.6,
//! règle 7).
//!
//! Un nom se réserve en créant ce qu'il désignera (`.part`, dossier d'un torrent, fichier
//! d'un sous-titre) avant d'être enregistré dans la tâche : deux tâches ne peuvent pas
//! finir au même endroit. Un arrêt entre la création et l'enregistrement laissait la
//! création orpheline, et la tâche prenait le nom suivant à la reprise. Désormais :
//!
//! 1. la réservation est notée en base (`name_claims`), puis seulement créée sur le disque ;
//! 2. la transaction qui enregistre le nom efface la réservation
//!    (`store::claims::release`) ;
//! 3. au démarrage, avant toute exécution, chaque réservation restante désigne ce que le
//!    moteur a créé sans l'enregistrer ([`recover`]) : c'est effacé, puis la réservation.
//!
//! Ce qu'une réservation désigne a été créé par le moteur en exclusivité (`create_new`,
//! `create_dir`) : aucune autre tâche ne peut avoir pris ce nom tant qu'il existe.

use std::io;
use std::path::Path;

use rusqlite::Connection;

use crate::io::{available_name, part_path, same_name, sync_parent_dir};
use crate::store::StoreError;
use crate::store::claims::{self, ClaimKind};

/// Erreur d'une réservation : la base, ou le disque.
#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Disk(#[from] io::Error),
}

/// Réserve un nom libre proche de `wanted` dans `dir` pour la tâche `task_id` : la
/// réservation notée, puis la création sur le disque (`.part` vide, ou dossier vide pour
/// [`ClaimKind::Dir`] ; pour [`ClaimKind::File`], le `.part` vide aussi, que l'appelant
/// écrit puis renomme). Rend le nom, à enregistrer avec `store::claims::release` dans la
/// même transaction.
pub fn claim(
    conn: &Connection,
    dir: &Path,
    wanted: &str,
    reserved: &[String],
    kind: ClaimKind,
    task_id: &str,
) -> Result<String, ClaimError> {
    let dir_text = dir.to_string_lossy().into_owned();
    let mut skip = reserved.to_vec();
    let mut last = None;
    // Chaque échec laisse le nom à une autre tâche : le suivant l'évite. La borne ne sert
    // qu'à ne jamais boucler sans fin.
    for _ in 0..1000 {
        let candidate = available_name(dir, wanted, &skip);
        if !claims::insert(conn, &dir_text, &candidate, kind, task_id)? {
            // Réservé par une autre tâche, pas encore créé : un nom plus loin.
            skip.push(candidate);
            continue;
        }
        let path = dir.join(&candidate);
        let created = match kind {
            ClaimKind::Dir => std::fs::create_dir(&path),
            ClaimKind::Part | ClaimKind::File => std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(part_path(&path))
                .map(|_| ()),
        };
        match created {
            Ok(()) => {
                // Pris entre le choix et la création : une autre tâche a fini ce nom (son
                // `.part` renommé, son nom déjà enregistré). Défait, puis le nom suivant.
                let finished = kind != ClaimKind::Dir && path.exists();
                let recorded = crate::store::tasks::reserved_names(conn, &dir_text, task_id)?
                    .iter()
                    .any(|r| same_name(r, &candidate));
                if finished || recorded {
                    let undone = match kind {
                        ClaimKind::Dir => std::fs::remove_dir(&path),
                        ClaimKind::Part | ClaimKind::File => std::fs::remove_file(part_path(&path)),
                    };
                    undone?;
                    claims::delete(conn, &dir_text, &candidate)?;
                    skip.push(candidate);
                    continue;
                }
                // L'entrée du dossier doit survivre à un arrêt brutal (SPEC §5.6).
                sync_parent_dir(&path).map_err(io::Error::other)?;
                return Ok(candidate);
            }
            Err(e) => {
                claims::delete(conn, &dir_text, &candidate)?;
                if e.kind() == io::ErrorKind::AlreadyExists {
                    skip.push(candidate);
                    last = Some(e);
                } else {
                    return Err(e.into());
                }
            }
        }
    }
    Err(last
        .unwrap_or_else(|| io::Error::other("aucun nom libre"))
        .into())
}

/// Au démarrage, avant toute exécution : efface ce que les réservations restantes
/// désignent (un `.part` ou un dossier vides ; pour un sous-titre, son fichier), puis les
/// réservations. Rend le nombre de réservations reprises.
pub fn recover(conn: &Connection) -> Result<usize, StoreError> {
    let left = claims::all(conn)?;
    for claim in &left {
        let path = Path::new(&claim.dir).join(&claim.name);
        let part = part_path(&path);
        let removed = match claim.kind {
            // Vide : rien n'y est écrit avant l'enregistrement du nom.
            ClaimKind::Part => remove_if_empty(&part),
            ClaimKind::Dir => match std::fs::remove_dir(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
            ClaimKind::File => remove_present(&part).and_then(|()| remove_present(&path)),
        };
        match removed {
            Ok(()) => {
                let _ = sync_parent_dir(&path);
                tracing::info!(tache = %claim.task_id, "nom réservé avant un arrêt, effacé");
            }
            // Gardé tel quel : la réservation part quand même, le fichier n'est plus
            // jamais touché par le moteur.
            Err(e) => tracing::warn!(
                tache = %claim.task_id,
                erreur = %e,
                "nom réservé avant un arrêt, laissé"
            ),
        }
        claims::delete(conn, &claim.dir, &claim.name)?;
    }
    Ok(left.len())
}

fn remove_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn remove_if_empty(path: &Path) -> io::Result<()> {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(meta) if meta.is_file() && meta.len() == 0 => std::fs::remove_file(path),
        Ok(_) => Err(io::Error::other("non vide : gardé")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_in_memory;

    /// Un arrêt entre la réservation et l'enregistrement : au démarrage, le `.part`, le
    /// dossier et le sous-titre réservés sont effacés, et le nom redevient libre. Un nom
    /// enregistré (réservation effacée) n'est jamais touché.
    #[test]
    fn claims_left_by_a_crash_are_undone_at_startup() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let a = claim(&conn, d, "a.iso", &[], ClaimKind::Part, "t1").unwrap();
        assert_eq!(a, "a.iso");
        assert!(d.join("a.iso.part").exists());
        // Le même nom, voulu par une autre tâche : le suivant.
        let b = claim(&conn, d, "a.iso", &[], ClaimKind::Part, "t2").unwrap();
        assert_eq!(b, "a (1).iso");
        let multi = claim(&conn, d, "album", &[], ClaimKind::Dir, "t3").unwrap();
        assert!(d.join(&multi).is_dir());
        let sub = claim(&conn, d, "v.fr.vtt", &[], ClaimKind::File, "t4").unwrap();
        std::fs::write(d.join("v.fr.vtt.part"), b"WEBVTT").unwrap();
        std::fs::rename(d.join("v.fr.vtt.part"), d.join(&sub)).unwrap();
        // `t2` enregistre son nom : sa réservation part, son `.part` reste à lui.
        claims::release(&conn, "t2").unwrap();
        assert_eq!(claims::all(&conn).unwrap().len(), 3);

        assert_eq!(recover(&conn).unwrap(), 3);
        assert!(!d.join("a.iso.part").exists());
        assert!(!d.join("album").exists());
        assert!(!d.join("v.fr.vtt").exists());
        assert!(d.join("a (1).iso.part").exists(), "nom enregistré : gardé");
        assert!(claims::all(&conn).unwrap().is_empty());
        assert_eq!(
            claim(&conn, d, "a.iso", &[], ClaimKind::Part, "t1").unwrap(),
            "a.iso"
        );
    }

    /// Des tâches qui visent le même nom en même temps, chacune avec sa connexion à la base,
    /// obtiennent chacune le sien ; la création du `.part` précède le retour.
    #[test]
    fn simultaneous_claims_never_share_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("auro.db");
        drop(crate::store::open(&db).unwrap());
        let files = dir.path().join("dl");
        std::fs::create_dir(&files).unwrap();
        let start = std::sync::Arc::new(std::sync::Barrier::new(16));
        let names: Vec<String> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..16)
                .map(|i| {
                    let (start, db, files) = (std::sync::Arc::clone(&start), &db, &files);
                    s.spawn(move || {
                        let conn = crate::store::open(db).unwrap();
                        start.wait();
                        let task = format!("t{i}");
                        claim(&conn, files, "e.iso", &[], ClaimKind::Part, &task).unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let distinct: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(distinct.len(), 16, "{names:?}");
        for name in &names {
            assert!(files.join(format!("{name}.part")).exists(), "{name}");
        }
    }

    /// Un `.part` réservé qui n'est plus vide n'est pas effacé (prudence) ; la réservation
    /// part quand même.
    #[test]
    fn a_claimed_part_that_is_not_empty_is_kept() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let name = claim(&conn, dir.path(), "f.bin", &[], ClaimKind::Part, "t").unwrap();
        std::fs::write(dir.path().join("f.bin.part"), b"x").unwrap();
        assert_eq!(recover(&conn).unwrap(), 1);
        assert!(dir.path().join(format!("{name}.part")).exists());
        assert!(claims::all(&conn).unwrap().is_empty());
    }
}
