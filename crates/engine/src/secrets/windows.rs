//! Gestionnaire d'identification de Windows : identifiants génériques, propres à
//! l'utilisateur, gardés sur cette machine (`CRED_PERSIST_LOCAL_MACHINE`, pas d'itinérance).
//!
//! Nom de chaque identifiant : `AuroDownloader:<espace>:<id>:<n>`, `n` numérotant les
//! morceaux de 2 560 octets d'un secret ; le secret est la suite des morceaux jusqu'au
//! premier absent.

use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
use windows_sys::Win32::Security::Credentials::{
    CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW,
    CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW,
};

use super::{Backend, MAX_SECRET, VaultError};

const PREFIX: &str = "AuroDownloader";
const CHUNK: usize = CRED_MAX_CREDENTIAL_BLOB_SIZE as usize;
/// Morceaux d'un secret, au plus.
const MAX_CHUNKS: usize = MAX_SECRET.div_ceil(CHUNK) + 1;

pub(super) struct CredentialManager;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn target(namespace: &str, id: &str, n: usize) -> Vec<u16> {
    wide(&format!("{PREFIX}:{namespace}:{id}:{n}"))
}

fn last_error(what: &str) -> VaultError {
    VaultError(format!("{what} : {}", std::io::Error::last_os_error()))
}

/// Lit le morceau `n` ; `None` s'il n'existe pas.
fn read_chunk(namespace: &str, id: &str, n: usize) -> Result<Option<Vec<u8>>, VaultError> {
    let name = target(namespace, id, n);
    let mut cred: *mut CREDENTIALW = ptr::null_mut();
    // SAFETY : `name` est terminé par un zéro ; `cred` reçoit un bloc alloué par le
    // système, lu puis rendu par `CredFree`.
    unsafe {
        if CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) == 0 {
            return if GetLastError() == ERROR_NOT_FOUND {
                Ok(None)
            } else {
                Err(last_error("lecture"))
            };
        }
        let c = &*cred;
        let bytes = if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
        };
        CredFree(cred.cast());
        Ok(Some(bytes))
    }
}

/// Efface le morceau `n` ; `false` s'il n'existait pas.
fn delete_chunk(namespace: &str, id: &str, n: usize) -> Result<bool, VaultError> {
    let name = target(namespace, id, n);
    // SAFETY : `name` est terminé par un zéro.
    unsafe {
        if CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) == 0 {
            return if GetLastError() == ERROR_NOT_FOUND {
                Ok(false)
            } else {
                Err(last_error("effacement"))
            };
        }
    }
    Ok(true)
}

impl Backend for CredentialManager {
    fn kind(&self) -> &'static str {
        "Gestionnaire d'identification"
    }

    fn put(&self, namespace: &str, id: &str, secret: &[u8]) -> Result<(), VaultError> {
        let mut user = wide(PREFIX);
        let mut comment = wide("En-tête d'un téléchargement (AuroDownloader)");
        // Un secret vide tient tout de même un morceau, pour exister.
        let chunks: Vec<&[u8]> = if secret.is_empty() {
            vec![&[]]
        } else {
            secret.chunks(CHUNK).collect()
        };
        for (n, chunk) in chunks.iter().enumerate() {
            let mut name = target(namespace, id, n);
            let cred = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                TargetName: name.as_mut_ptr(),
                Comment: comment.as_mut_ptr(),
                CredentialBlobSize: chunk.len() as u32,
                CredentialBlob: chunk.as_ptr().cast_mut(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                UserName: user.as_mut_ptr(),
                ..Default::default()
            };
            // SAFETY : les chaînes et le morceau vivent jusqu'à la fin de l'appel, qui ne
            // les modifie pas.
            if unsafe { CredWriteW(&cred, 0) } == 0 {
                let error = last_error("écriture");
                let _ = self.delete(namespace, id);
                return Err(error);
            }
        }
        Ok(())
    }

    fn get(&self, namespace: &str, id: &str) -> Result<Option<Vec<u8>>, VaultError> {
        let mut secret = Vec::new();
        for n in 0..MAX_CHUNKS {
            match read_chunk(namespace, id, n)? {
                Some(chunk) => secret.extend_from_slice(&chunk),
                None if n == 0 => return Ok(None),
                None => break,
            }
        }
        Ok(Some(secret))
    }

    fn delete(&self, namespace: &str, id: &str) -> Result<(), VaultError> {
        // Jusqu'au premier morceau absent ; le premier en dernier, pour qu'un effacement
        // interrompu laisse un secret encore énuméré (et ramassé plus tard).
        let mut count = 0;
        while count < MAX_CHUNKS && read_chunk(namespace, id, count)?.is_some() {
            count += 1;
        }
        for n in (0..count).rev() {
            delete_chunk(namespace, id, n)?;
        }
        Ok(())
    }

    fn ids(&self, namespace: &str) -> Result<Vec<String>, VaultError> {
        let prefix = format!("{PREFIX}:{namespace}:");
        let filter = wide(&format!("{prefix}*"));
        let mut count = 0u32;
        let mut list: *mut *mut CREDENTIALW = ptr::null_mut();
        let mut ids = Vec::new();
        // SAFETY : `filter` est terminé par un zéro ; `list` reçoit `count` pointeurs dans
        // un bloc alloué par le système, rendu par `CredFree`.
        unsafe {
            if CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut list) == 0 {
                return if GetLastError() == ERROR_NOT_FOUND {
                    Ok(ids)
                } else {
                    Err(last_error("énumération"))
                };
            }
            for i in 0..count as usize {
                let c = &**list.add(i);
                if c.TargetName.is_null() {
                    continue;
                }
                let len = (0..).take_while(|&k| *c.TargetName.add(k) != 0).count();
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(c.TargetName, len));
                // `<id>:<n>` après le préfixe : un nom par secret, au morceau 0.
                if let Some(id) = name
                    .strip_prefix(&prefix)
                    .and_then(|rest| rest.strip_suffix(":0"))
                    .filter(|id| !id.contains(':'))
                {
                    ids.push(id.to_owned());
                }
            }
            CredFree(list.cast());
        }
        Ok(ids)
    }
}
