//! Actions de fin d'une planification (F-16, SPEC §7.11) : mise en veille et extinction
//! de la machine.
//!
//! Le moteur ne les exécute que par la fonction que lui donne son hôte
//! (`scheduler::Planning::power`). Par défaut, [`unplugged`] refuse : une bibliothèque, un
//! test ou un banc n'éteint jamais la machine. `aurod` branche [`system`], ou [`simulate`]
//! pour un essai.

use std::io;

use crate::schedule::EndAction;

/// Aucune action branchée : refus.
pub fn unplugged(action: EndAction) -> io::Result<()> {
    match action {
        EndAction::None => Ok(()),
        _ => Err(io::Error::other(
            "aucune action de fin n'est branchée sur ce moteur",
        )),
    }
}

/// Écrit l'action dans le journal au lieu de l'exécuter (essais manuels).
pub fn simulate(action: EndAction) -> io::Result<()> {
    tracing::info!(action = action.as_str(), "action de fin simulée");
    Ok(())
}

/// Met en veille ou éteint la machine.
pub fn system(action: EndAction) -> io::Result<()> {
    match action {
        EndAction::None => Ok(()),
        EndAction::Sleep => sleep(),
        EndAction::Shutdown => shutdown(),
    }
}

#[cfg(windows)]
fn sleep() -> io::Result<()> {
    use windows_sys::Win32::System::Power::SetSuspendState;
    enable_shutdown_privilege()?;
    // Veille (pas d'hibernation), sans forcer les applications, réveils permis. Revient
    // au réveil de la machine.
    // SAFETY : fonction sans pointeur.
    if unsafe { SetSuspendState(false, false, false) } {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `SetSuspendState` demande le privilège `SeShutdownPrivilege` activé : un utilisateur
/// ordinaire l'a, mais désactivé par défaut. Il ne vaut que pour ce processus.
#[cfg(windows)]
fn enable_shutdown_privilege() -> io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, GetLastError, HANDLE, LUID};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let name: Vec<u16> = "SeShutdownPrivilege\0".encode_utf16().collect();
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY : poignée du processus courant (pseudo-poignée) ; `token` reçoit une poignée
    // fermée plus bas.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut luid = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    // SAFETY : `name` est terminé par un zéro ; `luid` est un emplacement valide.
    let found = unsafe { LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) } != 0;
    let result = if !found {
        Err(io::Error::last_os_error())
    } else {
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        // SAFETY : `token` est ouvert avec le droit d'ajuster ses privilèges ; aucun état
        // précédent demandé.
        let ok = unsafe {
            AdjustTokenPrivileges(
                token,
                0,
                &privileges,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } != 0;
        // Réussi sans le privilège : `ERROR_NOT_ALL_ASSIGNED` dans `GetLastError`.
        // SAFETY : sans argument.
        match (ok, unsafe { GetLastError() }) {
            (true, ERROR_SUCCESS) => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    };
    // SAFETY : poignée ouverte ci-dessus, fermée une seule fois.
    unsafe { CloseHandle(token) };
    result
}

#[cfg(windows)]
fn shutdown() -> io::Result<()> {
    // Chemin complet : jamais un `shutdown` trouvé ailleurs dans le PATH.
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let program = std::path::Path::new(&root).join(r"System32\shutdown.exe");
    run(std::process::Command::new(program).args(["/s", "/t", "0"]))
}

#[cfg(all(unix, not(target_os = "android")))]
fn systemctl(verb: &str) -> io::Result<()> {
    let program = ["/usr/bin/systemctl", "/bin/systemctl"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .ok_or_else(|| io::Error::other("systemctl introuvable"))?;
    run(std::process::Command::new(program).arg(verb))
}

#[cfg(all(unix, not(target_os = "android")))]
fn sleep() -> io::Result<()> {
    systemctl("suspend")
}

#[cfg(all(unix, not(target_os = "android")))]
fn shutdown() -> io::Result<()> {
    systemctl("poweroff")
}

#[cfg(not(any(windows, all(unix, not(target_os = "android")))))]
fn sleep() -> io::Result<()> {
    Err(io::Error::other("mise en veille non prise en charge ici"))
}

#[cfg(not(any(windows, all(unix, not(target_os = "android")))))]
fn shutdown() -> io::Result<()> {
    Err(io::Error::other("extinction non prise en charge ici"))
}

#[cfg(any(windows, all(unix, not(target_os = "android"))))]
fn run(command: &mut std::process::Command) -> io::Result<()> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "refusée par le système ({status})"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_default_nothing_is_plugged() {
        assert!(unplugged(EndAction::None).is_ok());
        assert!(unplugged(EndAction::Sleep).is_err());
        assert!(unplugged(EndAction::Shutdown).is_err());
    }

    /// Activer le privilège n'éteint rien : il ne vaut que pour le processus des tests.
    #[cfg(windows)]
    #[test]
    fn the_shutdown_privilege_can_be_enabled() {
        enable_shutdown_privilege().unwrap();
    }
}
