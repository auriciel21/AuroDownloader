//! Processus externes de la vidéo (yt-dlp, ffmpeg), tués avec tout leur arbre.
//!
//! yt-dlp est publié en « onefile » PyInstaller : le programme lancé en démarre un second,
//! l'interpréteur Python. Tuer le premier laisserait le second tourner. Le moteur tue donc
//! tout l'arbre : sous Windows par un Job Object (`KILL_ON_JOB_CLOSE`), qui le tue aussi si
//! le moteur lui-même s'arrête brutalement ; sous Linux par un groupe de processus.

use std::io;

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
pub(crate) fn spawn(
    command: &mut tokio::process::Command,
) -> io::Result<(tokio::process::Child, Tree)> {
    let child = command.spawn()?;
    let tree = Tree::adopt(&child)?;
    Ok((child, tree))
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
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("processus déjà terminé"))?;
        // SAFETY : appels sur des poignées valides ; la structure vit pendant l'appel.
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
            if set == 0 || AssignProcessToJobObject(job, process as _) == 0 {
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
