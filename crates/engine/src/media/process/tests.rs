//! Garde du moteur sous Windows : un moteur tué entre le lancement d'un processus et son
//! rangement dans son Job Object ne le laisse pas suspendu pour toujours.

use std::sync::atomic::{AtomicBool, Ordering};

/// Fichier où le moteur d'essai écrit le numéro du processus lancé, avant de mourir.
const PID_FILE: &str = "AURO_TEST_STOPPED_PID_FILE";

/// Vrai seulement dans le processus d'essai lancé par le test ci-dessous.
static STOP: AtomicBool = AtomicBool::new(false);

/// Appelé par [`super::spawn`] juste après le lancement : le processus d'essai note le
/// numéro du lancé, puis meurt sans rien fermer de lui-même, comme un moteur tué.
pub(super) fn stop_here_if_asked(child: &tokio::process::Child) {
    if !STOP.load(Ordering::SeqCst) {
        return;
    }
    let path = std::env::var_os(PID_FILE).expect("fichier du numéro");
    std::fs::write(path, child.id().expect("numéro").to_string()).expect("numéro écrit");
    std::process::exit(0);
}

#[tokio::test]
#[ignore = "lancé dans un processus à part par a_killed_engine_leaves_no_suspended_process"]
async fn engine_stopped_after_a_spawn() {
    if std::env::var_os(PID_FILE).is_none() {
        return;
    }
    STOP.store(true, Ordering::SeqCst);
    let mut command = tokio::process::Command::new("cmd.exe");
    command.args(["/c", "exit"]);
    super::prepare(&mut command);
    let _ = super::spawn(&mut command);
    panic!("le processus d'essai aurait dû s'arrêter au lancement");
}

#[test]
fn a_killed_engine_leaves_no_suspended_process() {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "media::process::tests::engine_stopped_after_a_spawn",
            "--ignored",
            "--test-threads=1",
        ])
        .env(PID_FILE, &pid_file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "processus d'essai : {status}");
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .expect("le processus d'essai a lancé cmd.exe")
        .parse()
        .unwrap();
    // SAFETY : poignée ouverte puis fermée ici ; seule la fin du processus est attendue.
    let gone = unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            true
        } else {
            let ended = WaitForSingleObject(process, 10_000) == WAIT_OBJECT_0;
            CloseHandle(process);
            ended
        }
    };
    // Jamais tué ici : le numéro pourrait déjà être celui d'un autre processus.
    assert!(
        gone,
        "cmd.exe suspendu (PID {pid}) survit au moteur tué : à arrêter par son numéro"
    );
}
