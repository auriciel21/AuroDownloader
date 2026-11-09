//! Déplacement du fichier d'une tâche (« Déplacer/Renommer », SPEC §7.12) : renommage
//! sur le même volume, copie vers un autre, et reprise après un arrêt brutal.
//!
//! Le journal (`store::moves`) est écrit avant de toucher au disque. Le fichier d'origine
//! n'est effacé qu'une fois la base passée au nouvel emplacement : à tout instant, la
//! tâche désigne un fichier complet, et [`recover`] sait, au démarrage, finir ou annuler
//! ce qu'un arrêt a interrompu.

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::io::{IoError, sync_parent_dir};
use crate::store::StoreError;
use crate::store::moves::{self, FileMove, Phase};

/// Suffixe de la copie en cours vers un autre volume, à côté de sa destination.
pub const TEMP_SUFFIX: &str = ".auro-move";

/// Dossier que [`rename`] traite, dans les tests, comme un autre volume.
#[cfg(test)]
pub const SIMULATED_VOLUME: &str = "autre-volume";

/// Copie temporaire d'un déplacement vers `to`.
pub fn temp_for(to: &Path) -> PathBuf {
    let mut name = to.as_os_str().to_os_string();
    name.push(TEMP_SUFFIX);
    PathBuf::from(name)
}

fn io_error(e: IoError) -> io::Error {
    match e {
        IoError::File { source, .. } => source,
        other => io::Error::other(other.to_string()),
    }
}

/// Renomme `from` en `to` sur le même volume, de façon durable. `Ok(false)` : les deux sont
/// sur des volumes différents, rien n'a changé ; il faut copier ([`copy_across`]).
pub fn rename(from: &Path, to: &Path) -> io::Result<bool> {
    // Les tests simulent un autre volume par un dossier ainsi nommé.
    #[cfg(test)]
    if to.to_string_lossy().contains(SIMULATED_VOLUME) {
        return Ok(false);
    }
    match std::fs::rename(from, to) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => return Ok(false),
        Err(e) => return Err(e),
    }
    sync_parent_dir(to).map_err(io_error)?;
    sync_parent_dir(from).map_err(io_error)?;
    Ok(true)
}

/// Copie `from` vers un autre volume : dans `temp` d'abord, vidée sur disque avec la date
/// de modification d'origine, puis renommée en `to`, qui ne doit pas exister. L'original
/// reste ; une copie interrompue laisse `temp`, que l'appelant efface.
pub fn copy_across(from: &Path, temp: &Path, to: &Path) -> io::Result<()> {
    let modified = std::fs::metadata(from)?.modified()?;
    std::fs::copy(from, temp)?;
    let copy = OpenOptions::new().write(true).open(temp)?;
    copy.set_modified(modified)?;
    copy.sync_all()?;
    drop(copy);
    // Le moteur n'écrase jamais un fichier (SPEC §7.5).
    if to.try_exists()? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "un fichier de ce nom est apparu à la destination pendant la copie",
        ));
    }
    std::fs::rename(temp, to)?;
    sync_parent_dir(to).map_err(io_error)?;
    Ok(())
}

/// Efface `path` s'il existe ; une erreur est signalée dans le journal du moteur, sans plus.
pub fn remove_quietly(path: &Path, why: &str) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => eprintln!(
            "aurod : {why} : impossible d'effacer {} : {e}",
            path.display()
        ),
    }
}

fn len(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// Ce que [`recover`] a fait d'un déplacement interrompu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovered {
    /// Le fichier était à sa nouvelle place : la tâche l'y suit.
    Finished,
    /// Le fichier n'avait pas bougé (ou rien ne permet de l'affirmer) : la tâche garde
    /// son emplacement.
    Abandoned,
}

/// Au démarrage, avant toute exécution : finit ou annule les déplacements qu'un arrêt a
/// interrompus (SPEC §7.12).
///
/// - Phase `moved` : la base désigne déjà la nouvelle place ; l'original est effacé.
/// - Phase `moving`, copie temporaire présente : elle est effacée (incomplète, ou complète
///   mais pas encore renommée).
/// - Phase `moving`, seule la destination existe : le renommage a eu lieu, la tâche suit.
/// - Phase `moving`, les deux existent après une copie vers un autre volume, de même
///   taille : la copie était complète et renommée ; la tâche suit, l'original est effacé.
/// - Sinon : la tâche garde son emplacement, et aucun fichier n'est effacé.
pub fn recover(conn: &mut Connection) -> Result<Vec<(String, Recovered)>, StoreError> {
    let mut done = Vec::new();
    for m in moves::all(conn)? {
        let outcome = recover_one(conn, &m)?;
        eprintln!(
            "aurod : tâche {} : déplacement interrompu {}",
            m.task_id,
            match outcome {
                Recovered::Finished => "terminé",
                Recovered::Abandoned => "annulé, la tâche garde son emplacement",
            }
        );
        done.push((m.task_id, outcome));
    }
    Ok(done)
}

fn recover_one(conn: &mut Connection, m: &FileMove) -> Result<Recovered, StoreError> {
    let (from, to) = (Path::new(&m.from_path), Path::new(&m.to_path));
    if m.phase == Phase::Moved {
        if to.exists() {
            remove_quietly(from, "fin d'un déplacement");
        }
        moves::forget(conn, &m.task_id)?;
        return Ok(Recovered::Finished);
    }
    if let Some(temp) = &m.temp_path {
        remove_quietly(Path::new(temp), "copie interrompue");
    }
    let (from_len, to_len) = (len(from), len(to));
    match (from_len, to_len) {
        (None, Some(_)) => {
            moves::commit(conn, &m.task_id, Phase::Moving)?;
            Ok(Recovered::Finished)
        }
        (Some(a), Some(b)) if m.temp_path.is_some() && a == b => {
            moves::commit(conn, &m.task_id, Phase::Moved)?;
            remove_quietly(from, "fin d'un déplacement");
            moves::forget(conn, &m.task_id)?;
            Ok(Recovered::Finished)
        }
        _ => {
            moves::forget(conn, &m.task_id)?;
            Ok(Recovered::Abandoned)
        }
    }
}

/// Ouvre `path` en écriture pour vérifier qu'il n'est pas tenu par un autre programme ;
/// utilisé par les tests.
#[cfg(test)]
fn writable(path: &Path) -> bool {
    OpenOptions::new().append(true).open(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_in_memory;
    use crate::store::tasks::{self, NewTask, load_task};

    struct Case {
        conn: Connection,
        dir: tempfile::TempDir,
        id: String,
    }

    /// Tâche préparée `a.iso`, dans `dir/src`, à déplacer vers `dir/dst/b.iso`.
    fn case() -> Case {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::create_dir(dir.path().join("dst")).unwrap();
        let mut conn = open_in_memory().unwrap();
        let queue = tasks::ensure_queue(&conn, "default").unwrap();
        let src = dir.path().join("src");
        let (id, _) = tasks::create_task(
            &mut conn,
            &NewTask {
                queue_id: &queue,
                url: "https://exemple.org/a.iso",
                display_name: "a.iso",
                dest_dir: src.to_str().unwrap(),
                max_connections: None,
                speed_limit_bps: None,
                idempotency_key: None,
                requested_name: None,
                checksum: None,
                headers: &[],
                parent_id: None,
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET file_name = 'a.iso', state = 'paused', pause_reason = 'user'
             WHERE id = ?1",
            [&id],
        )
        .unwrap();
        Case { conn, dir, id }
    }

    impl Case {
        fn from(&self) -> PathBuf {
            self.dir.path().join("src").join("a.iso.part")
        }
        fn to(&self) -> PathBuf {
            self.dir.path().join("dst").join("b.iso.part")
        }
        fn location(&self) -> (String, Option<String>) {
            let t = load_task(&self.conn, &self.id).unwrap();
            assert!(!t.moving, "le journal est vidé");
            (t.dest_dir, t.file_name)
        }
        fn src(&self) -> String {
            self.dir.path().join("src").to_string_lossy().into_owned()
        }
        fn dst(&self) -> String {
            self.dir.path().join("dst").to_string_lossy().into_owned()
        }
    }

    #[test]
    fn rename_moves_the_file_within_a_volume() {
        let c = case();
        std::fs::write(c.from(), b"octets").unwrap();
        assert!(rename(&c.from(), &c.to()).unwrap());
        assert!(!c.from().exists());
        assert_eq!(std::fs::read(c.to()).unwrap(), b"octets");
    }

    #[test]
    fn copy_across_keeps_the_original_and_its_date() {
        let c = case();
        std::fs::write(c.from(), b"octets").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        OpenOptions::new()
            .write(true)
            .open(c.from())
            .unwrap()
            .set_modified(old)
            .unwrap();
        let temp = temp_for(&c.to());
        copy_across(&c.from(), &temp, &c.to()).unwrap();
        assert!(
            c.from().exists(),
            "l'original reste jusqu'à l'enregistrement"
        );
        assert!(!temp.exists());
        assert_eq!(std::fs::read(c.to()).unwrap(), b"octets");
        assert_eq!(std::fs::metadata(c.to()).unwrap().modified().unwrap(), old);
        assert!(writable(&c.to()));
    }

    #[test]
    fn copy_across_never_overwrites() {
        let c = case();
        std::fs::write(c.from(), b"octets").unwrap();
        std::fs::write(c.to(), b"autre").unwrap();
        let temp = temp_for(&c.to());
        let e = copy_across(&c.from(), &temp, &c.to()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(c.to()).unwrap(), b"autre");
    }

    /// Arrêt avant le renommage : le fichier est resté, la tâche aussi.
    #[test]
    fn a_move_interrupted_before_the_rename_is_abandoned() {
        let mut c = case();
        std::fs::write(c.from(), b"octets").unwrap();
        let m = journal(&c, false);
        moves::begin(&c.conn, &m).unwrap();
        let done = recover(&mut c.conn).unwrap();
        assert_eq!(done, vec![(c.id.clone(), Recovered::Abandoned)]);
        assert_eq!(c.location(), (c.src(), Some("a.iso".into())));
        assert!(c.from().exists());
    }

    /// Arrêt entre le renommage et l'enregistrement : la tâche suit son fichier.
    #[test]
    fn a_rename_not_yet_recorded_is_finished() {
        let mut c = case();
        std::fs::write(c.to(), b"octets").unwrap();
        moves::begin(&c.conn, &journal(&c, false)).unwrap();
        let done = recover(&mut c.conn).unwrap();
        assert_eq!(done, vec![(c.id.clone(), Recovered::Finished)]);
        assert_eq!(c.location(), (c.dst(), Some("b.iso".into())));
    }

    /// Arrêt pendant une copie vers un autre volume : la copie partielle disparaît.
    #[test]
    fn a_partial_copy_is_erased() {
        let mut c = case();
        std::fs::write(c.from(), b"octets").unwrap();
        std::fs::write(temp_for(&c.to()), b"oct").unwrap();
        moves::begin(&c.conn, &journal(&c, true)).unwrap();
        assert_eq!(
            recover(&mut c.conn).unwrap(),
            vec![(c.id.clone(), Recovered::Abandoned)]
        );
        assert!(!temp_for(&c.to()).exists());
        assert!(c.from().exists());
        assert_eq!(c.location(), (c.src(), Some("a.iso".into())));
    }

    /// Arrêt après le renommage de la copie complète, avant l'enregistrement : la tâche
    /// suit la copie, l'original est effacé.
    #[test]
    fn a_complete_copy_not_yet_recorded_is_finished() {
        let mut c = case();
        std::fs::write(c.from(), b"octets").unwrap();
        std::fs::write(c.to(), b"octets").unwrap();
        moves::begin(&c.conn, &journal(&c, true)).unwrap();
        assert_eq!(
            recover(&mut c.conn).unwrap(),
            vec![(c.id.clone(), Recovered::Finished)]
        );
        assert!(!c.from().exists());
        assert_eq!(c.location(), (c.dst(), Some("b.iso".into())));
    }

    /// Les deux fichiers sont là mais diffèrent : rien n'est effacé, la tâche reste.
    #[test]
    fn two_different_files_are_both_kept() {
        let mut c = case();
        std::fs::write(c.from(), b"octets").unwrap();
        std::fs::write(c.to(), b"autre chose").unwrap();
        moves::begin(&c.conn, &journal(&c, true)).unwrap();
        assert_eq!(
            recover(&mut c.conn).unwrap(),
            vec![(c.id.clone(), Recovered::Abandoned)]
        );
        assert!(c.from().exists() && c.to().exists());
        assert_eq!(c.location(), (c.src(), Some("a.iso".into())));
    }

    /// Arrêt après l'enregistrement, avant l'effacement de l'original.
    #[test]
    fn a_recorded_copy_erases_the_original() {
        let mut c = case();
        std::fs::write(c.from(), b"octets").unwrap();
        std::fs::write(c.to(), b"octets").unwrap();
        moves::begin(&c.conn, &journal(&c, true)).unwrap();
        moves::commit(&mut c.conn, &c.id, Phase::Moved).unwrap();
        assert_eq!(
            recover(&mut c.conn).unwrap(),
            vec![(c.id.clone(), Recovered::Finished)]
        );
        assert!(!c.from().exists());
        assert_eq!(c.location(), (c.dst(), Some("b.iso".into())));
    }

    fn journal(c: &Case, temp: bool) -> FileMove {
        FileMove {
            task_id: c.id.clone(),
            from_path: c.from().to_string_lossy().into_owned(),
            to_path: c.to().to_string_lossy().into_owned(),
            temp_path: temp.then(|| temp_for(&c.to()).to_string_lossy().into_owned()),
            dest_dir: c.dst(),
            file_name: "b.iso".into(),
            phase: Phase::Moving,
        }
    }
}
