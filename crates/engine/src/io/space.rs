//! Espace disque (F-18) : place libre d'un volume, place occupée par un `.part`.

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
}
