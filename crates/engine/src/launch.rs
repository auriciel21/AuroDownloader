//! Démarrage d'`aurod` par ses clients (SPEC §4.6) : la CLI et l'application de bureau
//! le lancent, détaché, quand il ne répond pas sur son point d'accès.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::paths;

/// `aurod` à côté du programme qui l'appelle, sinon dans le `PATH`. `AURO_DAEMON` le
/// remplace.
pub fn daemon_program() -> PathBuf {
    if let Some(path) = std::env::var_os("AURO_DAEMON") {
        return PathBuf::from(path);
    }
    let name = format!("aurod{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Lance `aurod` détaché : il survit au programme qui l'a lancé et ne reçoit pas son
/// Ctrl-C. Sa sortie d'erreur va dans `aurod.log`, à côté de sa base.
pub fn start_daemon(endpoint: &str, db: Option<&Path>, ca_cert: Option<&Path>) -> io::Result<()> {
    let program = daemon_program();
    let mut command = Command::new(&program);
    command.arg("--endpoint").arg(endpoint);
    if let Some(db) = db {
        command.arg("--db").arg(db);
    }
    if let Some(ca) = ca_cert {
        command.arg("--ca-cert").arg(ca);
    }
    // À côté de la base si elle est donnée, sinon dans le dossier de données.
    let log_dir = match db {
        Some(db) => std::path::absolute(db)
            .ok()
            .and_then(|db| db.parent().map(Path::to_path_buf)),
        None => paths::data_dir(),
    };
    let log = log_dir.and_then(|dir| open_log(&dir));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log.map_or_else(Stdio::null, Stdio::from));
    #[cfg(windows)]
    let spawned = {
        use std::os::windows::process::CommandExt;
        // Ni fenêtre ni console partagée : le Ctrl-C d'un terminal ne l'atteint pas.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // Hors du groupe de processus (job) du lanceur : un navigateur range son hôte de
        // messagerie native (le relais) dans un job qu'il ferme à la fin, et le moteur
        // doit lui survivre. Sans effet hors d'un job ; si le job interdit d'en sortir,
        // le moteur part quand même, dedans.
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let flags = CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
        keep_own_handles();
        match command
            .creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
        {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                command.creation_flags(flags).spawn()
            }
            other => other,
        }
    };
    #[cfg(unix)]
    let spawned = {
        use std::os::unix::process::CommandExt;
        // Son propre groupe : le Ctrl-C du terminal ne l'atteint pas.
        command.process_group(0).spawn()
    };
    spawned.map(drop).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("impossible de lancer {} : {e}", program.display()),
        )
    })
}

/// Taille au-delà de laquelle `aurod.log` passe en `aurod.log.1` à un lancement.
const LOG_MAX: u64 = 1 << 20;

/// `aurod.log` de `dir`, ouvert en ajout ; trop gros, il devient `aurod.log.1` (l'ancien
/// effacé) et un neuf commence : relancé sans fin, `aurod` ne remplit plus le disque
/// (audit C8). Il ne reçoit que les erreurs de démarrage ; les journaux sont dans `logs/`.
fn open_log(dir: &Path) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join("aurod.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX) {
        let _ = std::fs::rename(&path, dir.join("aurod.log.1"));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// Rend non héritables les entrée et sorties de ce processus. Sinon `aurod`, qui vit
/// plus longtemps que lui, en garde une copie : un programme qui lit la sortie de la CLI
/// (tube de PowerShell, script) attendrait sa fin indéfiniment.
#[cfg(windows)]
fn keep_own_handles() {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY : poignées standard de ce processus ; un échec (poignée absente) est
        // sans conséquence.
        unsafe {
            let handle = GetStdHandle(which);
            if !handle.is_null() {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    #[test]
    fn a_large_log_is_set_aside_at_launch() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("aurod.log");
        std::fs::write(&log, vec![b'a'; super::LOG_MAX as usize + 1]).unwrap();
        std::fs::write(dir.path().join("aurod.log.1"), b"ancien").unwrap();
        let mut file = super::open_log(dir.path()).unwrap();
        file.write_all(b"neuf").unwrap();
        drop(file);
        assert_eq!(std::fs::read(&log).unwrap(), b"neuf");
        let kept = std::fs::read(dir.path().join("aurod.log.1")).unwrap();
        assert_eq!(kept.len(), super::LOG_MAX as usize + 1);
        // Petit, il est gardé et complété.
        let mut file = super::open_log(dir.path()).unwrap();
        file.write_all(b" et suite").unwrap();
        drop(file);
        assert_eq!(std::fs::read(&log).unwrap(), b"neuf et suite");
    }
}
