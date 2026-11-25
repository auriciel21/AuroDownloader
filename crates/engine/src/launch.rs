//! Démarrage d'`aurod` par ses clients (SPEC §4.6) : la CLI, le relais de l'extension et
//! l'application de bureau le lancent, détaché, quand personne n'écoute sur son point
//! d'accès ([`connect_or_start`]).

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::paths;

/// Délai de réponse d'un `aurod` lancé par un client.
pub const START_WAIT: Duration = Duration::from_secs(10);

/// Personne n'écoute sur le point d'accès : seul cas où un client lance `aurod`. Un canal
/// d'un autre utilisateur, un dossier refusé ou un canal occupé ne lancent rien (audit C8).
pub fn engine_absent(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

/// `aurod` peut-il être lancé sur `endpoint` avec la base `db` ? Sans base, il prendrait
/// celle du moteur réel : seulement sur le point d'accès par défaut, jamais sur un point
/// d'accès d'essai, fût-il celui d'`AURO_ENDPOINT` (incident du 2026-11-08, audit R3).
pub fn may_start(endpoint: &str, db: Option<&Path>) -> bool {
    db.is_some() || endpoint == paths::default_endpoint()
}

/// Pourquoi un client n'a pas joint le moteur ([`connect_or_start`]).
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// Personne n'écoute, et ce client ne lance pas `aurod`.
    #[error("aucun moteur sur {0}")]
    Absent(String),
    /// Personne n'écoute sur un point d'accès d'essai, sans base d'essai ([`may_start`]).
    #[error("aucun moteur d'essai sur {0} : il se démarre avec sa base")]
    TestEndpoint(String),
    /// Lancement refusé par le client (trop de lancements, mise à jour en cours…).
    #[error("{0}")]
    NotPermitted(String),
    /// Connexion impossible pour une autre raison qu'un moteur absent, ou lancement
    /// impossible.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// `aurod` lancé, mais muet au bout de [`START_WAIT`].
    #[error("aurod lancé, mais ne répond pas : {0}")]
    NoAnswer(io::Error),
}

impl From<ConnectError> for io::Error {
    fn from(error: ConnectError) -> Self {
        match error {
            ConnectError::Io(e) => e,
            ConnectError::NoAnswer(ref e) => io::Error::new(e.kind(), error.to_string()),
            other => io::Error::new(io::ErrorKind::NotFound, other.to_string()),
        }
    }
}

/// Connexion d'un client au moteur par `connect`. Si personne n'écoute (seul cas, audit C8)
/// et que `autostart` le demande, lance `aurod` (avec `db` et `ca_cert`), sous le garde de
/// [`may_start`] et l'accord de `permit`, puis réessaie toutes les 100 ms pendant
/// [`START_WAIT`]. Seul chemin des clients qui lancent le moteur (audit C30).
pub async fn connect_or_start<T, F, Fut>(
    endpoint: &str,
    autostart: bool,
    db: Option<&Path>,
    ca_cert: Option<&Path>,
    permit: impl FnOnce() -> Result<(), String>,
    mut connect: F,
) -> Result<T, ConnectError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    match connect().await {
        Ok(found) => return Ok(found),
        Err(e) if !engine_absent(&e) => return Err(ConnectError::Io(e)),
        Err(_) if !autostart => return Err(ConnectError::Absent(endpoint.to_owned())),
        Err(_) => {}
    }
    if !may_start(endpoint, db) {
        return Err(ConnectError::TestEndpoint(endpoint.to_owned()));
    }
    permit().map_err(ConnectError::NotPermitted)?;
    start_daemon(endpoint, db, ca_cert)?;
    let deadline = tokio::time::Instant::now() + START_WAIT;
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        match connect().await {
            Ok(found) => return Ok(found),
            Err(e) if tokio::time::Instant::now() >= deadline => {
                return Err(ConnectError::NoAnswer(e));
            }
            Err(_) => {}
        }
    }
}

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

    use super::*;

    /// Audit C8 : `aurod` n'est lancé que si personne n'écoute ; jamais pour un canal d'un
    /// autre utilisateur, un dossier refusé ou un canal occupé.
    #[test]
    fn only_an_absent_engine_is_started() {
        use std::io::{Error, ErrorKind};
        assert!(engine_absent(&Error::from(ErrorKind::NotFound)));
        assert!(engine_absent(&Error::from(ErrorKind::ConnectionRefused)));
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::TimedOut,
            ErrorKind::AddrInUse,
            ErrorKind::Other,
        ] {
            assert!(!engine_absent(&Error::from(kind)), "{kind:?}");
        }
    }

    /// Audit R3 et C30 : sans base, rien n'est lancé sur un point d'accès d'essai ; un
    /// moteur présent est joint sans rien lancer ; une autre erreur ne lance rien non plus.
    #[tokio::test]
    async fn a_client_starts_aurod_only_when_it_may() {
        let test = r"\\.\pipe\auro-essai-lancement".to_owned();
        assert_ne!(test, paths::default_endpoint());
        assert!(!may_start(&test, None));
        assert!(may_start(&test, Some(Path::new("essai.db"))));
        assert!(may_start(&paths::default_endpoint(), None));

        let absent = || async { Err::<(), _>(io::Error::from(io::ErrorKind::NotFound)) };
        let refused = || -> Result<(), String> { panic!("aucun lancement attendu") };
        let e = connect_or_start(&test, true, None, None, refused, absent).await;
        assert!(matches!(e, Err(ConnectError::TestEndpoint(_))), "{e:?}");
        let e = connect_or_start(&test, false, None, None, refused, absent).await;
        assert!(matches!(e, Err(ConnectError::Absent(_))), "{e:?}");
        let other = || async { Err::<(), _>(io::Error::from(io::ErrorKind::PermissionDenied)) };
        let e = connect_or_start(&test, true, Some(Path::new("e.db")), None, refused, other).await;
        assert!(matches!(e, Err(ConnectError::Io(_))), "{e:?}");
        let present = || async { Ok::<_, io::Error>(7) };
        let found = connect_or_start(&test, true, None, None, refused, present).await;
        assert_eq!(found.unwrap(), 7);
        // Lancement refusé par le client : rien n'est lancé.
        let e = connect_or_start(
            &test,
            true,
            Some(Path::new("e.db")),
            None,
            || Err("non".into()),
            absent,
        )
        .await;
        assert!(matches!(e, Err(ConnectError::NotPermitted(_))), "{e:?}");
    }

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
