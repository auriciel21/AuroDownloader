//! Espace disque (F-18) : place libre d'un volume, place occupée par un `.part`, et
//! réservation de la place d'un fichier (F-10).

use std::io;
use std::path::Path;

/// Octets libres pour l'utilisateur courant sur le volume qui contient `dir`.
#[cfg(windows)]
pub fn free_space(dir: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY : chemin terminé par un zéro et compteur valides pendant l'appel ; les deux
    // autres sorties sont facultatives.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}

/// Octets libres pour l'utilisateur courant sur le volume qui contient `dir`.
#[cfg(unix)]
pub fn free_space(dir: &Path) -> io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "chemin avec un octet nul"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY : chemin terminé par un zéro et structure de sortie valides pendant l'appel.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY : `statvfs` a réussi, la structure est remplie.
    let stat = unsafe { stat.assume_init() };
    // `f_bavail` : blocs libres pour un utilisateur sans privilège.
    #[allow(clippy::useless_conversion)] // u32 ou u64 selon la plateforme
    let (blocks, size) = (u64::from(stat.f_bavail), u64::from(stat.f_frsize));
    Ok(blocks.saturating_mul(size))
}

#[cfg(not(any(windows, unix)))]
pub fn free_space(_dir: &Path) -> io::Result<u64> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// Octets qu'un fichier occupe vraiment : sous Unix ses blocs, car un fichier creux occupe
/// moins que sa longueur ; ailleurs sa longueur (NTFS alloue toute la longueur d'un
/// fichier qui n'est pas déclaré creux).
pub fn allocated(path: &Path) -> io::Result<u64> {
    let meta = std::fs::metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(meta.blocks().saturating_mul(512))
    }
    #[cfg(not(unix))]
    {
        Ok(meta.len())
    }
}

/// Réserve sur le disque la place d'un fichier de `len` octets (F-10, SPEC §7.6 bis) : sa
/// longueur devient `len`, et ses blocs lui sont attribués sans écrire de zéros. Les
/// octets déjà écrits ne bougent pas. Renvoie `false`, sans rien faire, quand le système
/// de fichiers ne sait pas réserver vite : la place se prendra à l'écriture, comme avant.
///
/// Linux : `fallocate`, sans repli (`posix_fallocate` écrirait des zéros, très lent sur un
/// gros fichier). Windows : seulement NTFS et ReFS, où l'allongement réserve les
/// clusters ; FAT et exFAT pourraient remplir de zéros. Ailleurs : rien.
pub fn preallocate(file: &std::fs::File, len: u64) -> io::Result<bool> {
    if file.metadata()?.len() >= len && allocated_of(file)? >= len {
        return Ok(true);
    }
    reserve(file, len)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn reserve(file: &std::fs::File, len: u64) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    let len =
        libc::off_t::try_from(len).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    loop {
        // SAFETY : descripteur ouvert pendant l'appel ; aucun pointeur.
        if unsafe { libc::fallocate(file.as_raw_fd(), 0, 0, len) } == 0 {
            return Ok(true);
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EOPNOTSUPP | libc::ENOSYS) => return Ok(false),
            _ => return Err(e),
        }
    }
}

#[cfg(windows)]
fn reserve(file: &std::fs::File, len: u64) -> io::Result<bool> {
    if !matches!(file_system(file)?.as_str(), "NTFS" | "ReFS") {
        return Ok(false);
    }
    file.set_len(len)?;
    Ok(true)
}

#[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
fn reserve(_file: &std::fs::File, _len: u64) -> io::Result<bool> {
    Ok(false)
}

/// Nom du système de fichiers du volume qui contient `file` (« NTFS », « FAT32 »…).
#[cfg(windows)]
fn file_system(file: &std::fs::File) -> io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationByHandleW;
    let mut name = [0u16; 64];
    // SAFETY : poignée ouverte pendant l'appel ; tampon de `name.len()` caractères, les
    // autres sorties facultatives.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Ok(String::from_utf16_lossy(&name[..end]))
}

/// [`allocated`] d'un fichier ouvert.
fn allocated_of(file: &std::fs::File) -> io::Result<u64> {
    let meta = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(meta.blocks().saturating_mul(512))
    }
    #[cfg(not(unix))]
    {
        Ok(meta.len())
    }
}

/// Place qu'il reste à prendre sur le disque pour finir un fichier de `total` octets,
/// dont le `.part` est `part` (absent : tout reste à prendre).
pub fn space_needed(part: &Path, total: u64) -> u64 {
    total.saturating_sub(allocated(part).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_has_some_free_space() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_space(dir.path()).unwrap() > 0);
        assert!(free_space(&dir.path().join("absent")).is_err());
    }

    #[test]
    fn written_bytes_reduce_the_space_still_needed() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("f.part");
        assert_eq!(space_needed(&part, 1 << 20), 1 << 20);
        std::fs::write(&part, vec![7u8; 256 * 1024]).unwrap();
        assert!(allocated(&part).unwrap() >= 256 * 1024);
        assert!(space_needed(&part, 1 << 20) <= 768 * 1024);
        assert_eq!(space_needed(&part, 1000), 0);
    }

    #[test]
    fn preallocation_reserves_the_space_and_keeps_the_written_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("g.part");
        std::fs::write(&part, b"debut").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&part)
            .unwrap();
        let len = 4 << 20;
        let reserved = preallocate(&file, len).unwrap();
        // Les dossiers temporaires des postes d'essai sont en NTFS et ext4 : réservé.
        if cfg!(any(target_os = "linux", windows)) {
            assert!(reserved, "réservation attendue sur ce système");
        }
        if reserved {
            assert_eq!(std::fs::metadata(&part).unwrap().len(), len);
            assert!(allocated(&part).unwrap() >= len);
            assert_eq!(space_needed(&part, len), 0, "plus rien à prendre");
            // Déjà réservé : rien à refaire.
            assert!(preallocate(&file, len).unwrap());
        }
        drop(file);
        let bytes = std::fs::read(&part).unwrap();
        assert_eq!(&bytes[..5], b"debut");
        assert!(bytes[5..].iter().all(|&b| b == 0));
    }
}
