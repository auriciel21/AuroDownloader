//! Processus externes de la vidéo (yt-dlp, ffmpeg), tués avec tout leur arbre.
//!
//! yt-dlp est publié en « onefile » PyInstaller : le programme lancé en démarre un second,
//! l'interpréteur Python. Tuer le premier laisserait le second tourner. Le moteur tue donc
//! tout l'arbre : sous Windows par un Job Object (`KILL_ON_JOB_CLOSE`), qui le tue aussi si
//! le moteur lui-même s'arrête brutalement ; sous Linux par un groupe de processus.

use std::io;

#[cfg(all(test, windows))]
mod tests;

/// Prépare une commande pour [`spawn`] : sous Linux, son propre groupe de processus, et
/// `SIGTERM` à la mort du moteur (`PR_SET_PDEATHSIG`).
///
/// Un `aurod` tué brutalement (`SIGKILL`) ne tue pas son groupe : sans ce signal, yt-dlp
/// ou ffmpeg continueraient seuls, et pourraient écrire un flux après la fin de sa vidéo
/// (banc de reprise des vidéos, 2026-11-09). Le chargeur PyInstaller de yt-dlp transmet
/// `SIGTERM` à l'interpréteur ; ffmpeg s'arrête. Le signal suit un `exec`, pas un `fork`.
/// Il part à la fin du fil qui a lancé le processus : les lancements se font depuis les
/// fils de travail du moteur, qui durent autant que lui.
pub(crate) fn prepare(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(target_os = "linux")]
    // SAFETY : entre `fork` et `exec`, seuls des appels sûrs dans ce contexte (`prctl`,
    // `getppid`, `kill` sur soi-même).
    unsafe {
        let parent = libc::getpid();
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Le moteur est mort avant le réglage : rien ne doit lui survivre.
            if libc::getppid() != parent {
                libc::kill(libc::getpid(), libc::SIGTERM);
            }
            Ok(())
        });
    }
    #[cfg(not(unix))]
    let _ = command;
}

/// Lance la commande préparée par [`prepare`] ; tout son arbre meurt avec le garde rendu.
///
/// Sous Windows, sans fenêtre et suspendu : le processus n'avance qu'une fois dans son Job
/// Object, et aucun petit-processus ne peut naître avant, hors de l'arbre (audit V18).
pub(crate) fn spawn(
    command: &mut tokio::process::Command,
) -> io::Result<(tokio::process::Child, Tree)> {
    #[cfg(windows)]
    {
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        guard_engine();
    }
    let mut child = command.spawn()?;
    #[cfg(all(test, windows))]
    tests::stop_here_if_asked(&child);
    let tree = match Tree::adopt(&child) {
        Ok(tree) => tree,
        Err(e) => {
            let _ = child.start_kill();
            return Err(e);
        }
    };
    #[cfg(windows)]
    // Échec : `tree` tue le processus suspendu.
    resume(&child)?;
    Ok((child, tree))
}

/// Range le moteur lui-même, une fois, dans un Job Object tué à sa fermeture.
///
/// Un processus lancé entre de lui-même dans le Job Object de son parent, dès sa création.
/// Sans ce garde, un moteur tué entre le lancement et [`Tree::adopt`] laissait le processus
/// suspendu pour toujours, sans parent (banc des vidéos, 2026-11-24) : désormais la
/// fermeture de ce Job Object, à la mort du moteur, le tue aussi. Échec (moteur déjà dans un
/// Job Object qui l'interdit) : noté, le lancement continue sans ce garde.
#[cfg(windows)]
fn guard_engine() {
    static GUARD: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    GUARD.get_or_init(|| {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let job = match kill_on_close_job() {
            Ok(job) => job,
            Err(e) => {
                tracing::warn!(error = %e, "Job Object du moteur non créé");
                return;
            }
        };
        // SAFETY : poignée valide ; elle n'est jamais fermée après le rangement (la fermer
        // tuerait le moteur), seulement si le rangement échoue, Job Object vide.
        unsafe {
            if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
                let e = io::Error::last_os_error();
                tracing::warn!(error = %e, "moteur non rangé dans son Job Object");
                CloseHandle(job);
            }
        }
    });
}

/// Un Job Object neuf qui tue tous ses processus quand sa dernière poignée se ferme.
#[cfg(windows)]
fn kill_on_close_job() -> io::Result<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    // SAFETY : appels sur une poignée valide ; la structure vit pendant l'appel.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set == 0 {
            let error = io::Error::last_os_error();
            CloseHandle(job);
            return Err(error);
        }
        Ok(job)
    }
}

/// Relance les fils d'un processus lancé suspendu (un seul : son fil principal).
#[cfg(windows)]
fn resume(child: &tokio::process::Child) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("processus déjà terminé"))?;
    // SAFETY : instantané et poignées de fils valides, fermés ici ; la structure vit
    // pendant les appels, sa taille renseignée comme l'exige l'API.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0;
        let mut more = Thread32First(snapshot, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    if ResumeThread(thread) != u32::MAX {
                        resumed += 1;
                    }
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        if resumed == 0 {
            return Err(io::Error::other("processus lancé mais pas relancé"));
        }
    }
    Ok(())
}

/// Attend la fin du processus de tête, tue ce qui reste de son arbre, puis le libère.
///
/// Sous Linux, l'arbre est un groupe désigné par le numéro du processus de tête : tué
/// après la libération de celui-ci, ce numéro pouvait déjà être celui d'un autre groupe
/// (audit V19). La fin est donc attendue sans libérer (`waitid` avec `WNOWAIT`) : le
/// processus de tête, encore « zombie », garde le numéro réservé jusqu'au signal.
/// Abandonné en route (délai, arrêt), le garde tue l'arbre, processus de tête vivant.
pub(crate) async fn wait(
    child: &mut tokio::process::Child,
    tree: Tree,
) -> io::Result<std::process::ExitStatus> {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let exited = tokio::task::spawn_blocking(move || {
            // SAFETY : `info` est un emplacement valide ; `waitid` sans `WNOWAIT` n'est
            // jamais appelé ici, le processus reste à libérer par tokio.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            loop {
                let done = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid as libc::id_t,
                        &mut info,
                        libc::WEXITED | libc::WNOWAIT,
                    )
                };
                if done == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    return done == 0;
                }
            }
        });
        let _ = exited.await;
        drop(tree);
        return child.wait().await;
    }
    // Sous Windows, le Job Object tient l'arbre sans numéro réutilisable : la fin d'abord,
    // puis l'arbre fermé (le fermer avant tuerait le processus attendu).
    let status = child.wait().await;
    drop(tree);
    status
}

/// L'arbre d'un processus : tué quand le garde disparaît.
pub(crate) struct Tree {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    group: Option<i32>,
}

// Le Job Object n'est qu'une poignée du noyau, utilisable de n'importe quel fil.
#[cfg(windows)]
unsafe impl Send for Tree {}
#[cfg(windows)]
unsafe impl Sync for Tree {}

impl Tree {
    #[cfg(windows)]
    fn adopt(child: &tokio::process::Child) -> io::Result<Tree> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("processus déjà terminé"))?;
        let job = kill_on_close_job()?;
        // SAFETY : poignées valides ; celle du Job Object n'est fermée qu'ici ou par le garde.
        unsafe {
            if AssignProcessToJobObject(job, process as _) == 0 {
                let error = io::Error::last_os_error();
                CloseHandle(job);
                return Err(error);
            }
            Ok(Tree { job })
        }
    }

    #[cfg(unix)]
    fn adopt(child: &tokio::process::Child) -> io::Result<Tree> {
        Ok(Tree {
            group: child.id().and_then(|id| i32::try_from(id).ok()),
        })
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY : la poignée vient de `CreateJobObjectW` et n'est fermée qu'ici ; la
        // fermer tue tous les processus du Job Object.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
        #[cfg(unix)]
        if let Some(group) = self.group {
            // SAFETY : simple envoi de signal ; un groupe déjà disparu rend une erreur
            // ignorée.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
    }
}
