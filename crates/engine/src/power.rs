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
    eprintln!("aurod : action de fin simulée : {}", action.as_str());
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
    // Veille (pas d'hibernation), sans forcer les applications, réveils permis.
    // SAFETY : fonction sans pointeur.
    if unsafe { SetSuspendState(false, false, false) } {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
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
}
