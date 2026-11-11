//! Droits Windows réservés à l'utilisateur du processus : canal nommé du protocole
//! (`rpc::local`) et fichier de la clé de l'interface distante (`remote::tls`).

use std::ffi::c_void;
use std::io;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

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
        // Accès complet pour ce SID seul ; « P » : aucun droit hérité.
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
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
    let mut text: *mut u16 = null_mut();
    // SAFETY : SID valide tant que `buf` existe ; `text` est une variable locale.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
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
