//! Droits Windows réservés à l'utilisateur du processus : canal nommé du protocole
//! (`rpc::local`) et fichier de la clé de l'interface distante (`remote::tls`) ; et, côté
//! client, le propriétaire du canal vérifié avant de s'y fier.

use std::ffi::c_void;
use std::io;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_OBJECT_TYPE,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub use windows_sys::Win32::Security::Authorization::SE_KERNEL_OBJECT;

/// Descripteur de sécurité qui n'autorise que l'utilisateur courant.
pub struct OwnerOnly {
    descriptor: PSECURITY_DESCRIPTOR,
}

// SAFETY : le descripteur n'est plus modifié après sa création ; seul `Drop` le
// libère.
unsafe impl Send for OwnerOnly {}
unsafe impl Sync for OwnerOnly {}

impl OwnerOnly {
    pub fn new() -> io::Result<Self> {
        let sid = current_user_sid()?;
        // Propriétaire : l'utilisateur (même pour un processus élevé, dont le propriétaire
        // par défaut serait le groupe Administrateurs), que les clients vérifient. Accès
        // complet pour ce SID seul ; « P » : aucun droit hérité.
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY : chaîne terminée par un zéro ; `descriptor` est une variable locale.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnerOnly { descriptor })
    }

    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        }
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        // SAFETY : alloué par ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.descriptor) };
    }
}

/// SID de l'utilisateur du processus, en texte (`S-1-5-21-…`).
fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY : pseudo-poignée du processus courant ; `token` est une variable locale.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut len = 0u32;
    // SAFETY : premier appel pour connaître la taille ; l'échec attendu est ignoré.
    unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len) };
    // Mots de 8 octets : TOKEN_USER contient un pointeur, aligné sur 8.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY : `buf` fait au moins `len` octets.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast::<c_void>(),
            len,
            &mut len,
        )
    };
    // SAFETY : poignée ouverte ci-dessus.
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY : GetTokenInformation a rempli un TOKEN_USER au début de `buf`.
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    // SAFETY : SID valide tant que `buf` existe.
    unsafe { sid_text(user.User.Sid) }
}

/// Un SID en texte (`S-1-5-21-…`).
///
/// # Safety
/// `sid` doit désigner un SID valide pendant l'appel.
unsafe fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text: *mut u16 = null_mut();
    // SAFETY : SID valide (contrat de la fonction) ; `text` est une variable locale.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY : chaîne terminée par un zéro, allouée par le système.
    let sid = unsafe {
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast::<c_void>());
        sid
    };
    Ok(sid)
}

/// L'objet de cette poignée (`kind` : [`SE_KERNEL_OBJECT`] pour un canal nommé,
/// `SE_FILE_OBJECT` pour un fichier) appartient-il à l'utilisateur du processus ? Un
/// client du canal nommé le vérifie avant d'y écrire : un autre utilisateur de la machine
/// qui aurait créé le canal le premier (revue de sécurité, étape 5) ne reçoit rien.
/// La poignée doit avoir le droit `READ_CONTROL` (compris dans `GENERIC_READ`).
pub fn owned_by_current_user(handle: HANDLE, kind: SE_OBJECT_TYPE) -> io::Result<bool> {
    let mut owner: PSID = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY : poignée valide fournie par l'appelant ; sorties dans des variables locales.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            kind,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    // SAFETY : `owner` pointe dans `descriptor`, libéré juste après.
    let found = unsafe { sid_text(owner) };
    // SAFETY : alloué par GetSecurityInfo.
    unsafe { LocalFree(descriptor) };
    Ok(found? == current_user_sid()?)
}

/// Crée un fichier qui n'existait pas, lisible et modifiable par l'utilisateur courant
/// seul (aucun droit hérité du dossier) : clé privée de l'interface distante.
pub fn create_owner_only(path: &std::path::Path) -> io::Result<std::fs::File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;

    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL};

    let security = OwnerOnly::new()?;
    let attributes = security.attributes();
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY : chemin terminé par un zéro ; `attributes` et son descripteur vivent jusqu'au
    // retour.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY : poignée valide, dont le fichier devient seul propriétaire.
    Ok(unsafe { std::fs::File::from_raw_handle(handle) })
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Security::Authorization::SE_FILE_OBJECT;

    use super::*;

    #[test]
    fn the_owner_of_an_object_is_read_from_its_handle() {
        let dir = tempfile::tempdir().unwrap();
        let mine = create_owner_only(&dir.path().join("cle.pem")).unwrap();
        assert!(owned_by_current_user(mine.as_raw_handle(), SE_FILE_OBJECT).unwrap());
        drop(mine);
        // Un fichier du système appartient à TrustedInstaller.
        let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let theirs =
            std::fs::File::open(std::path::Path::new(&system).join("System32\\kernel32.dll"))
                .unwrap();
        assert!(!owned_by_current_user(theirs.as_raw_handle(), SE_FILE_OBJECT).unwrap());
    }
}
