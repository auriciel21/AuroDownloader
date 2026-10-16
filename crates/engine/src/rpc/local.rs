//! Transports locaux du protocole (SPEC §6.2) : canal nommé sous Windows, socket Unix
//! ailleurs. Seul l'utilisateur qui a lancé le moteur peut s'y connecter :
//! - Windows : le canal porte un descripteur de sécurité qui n'autorise que son
//!   identifiant (SID), refuse les clients distants, et un second moteur ne peut pas
//!   prendre le même nom ;
//! - Unix : le socket est dans un dossier en `0700`, lui-même en `0600`.
//!
//! Le point d'accès par défaut vient de [`crate::paths::endpoint`].

use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::scheduler::Engine;

/// Moitiés lecture et écriture d'une connexion locale.
pub type Halves = (
    Box<dyn AsyncRead + Unpin + Send>,
    Box<dyn AsyncWrite + Unpin + Send>,
);

/// Sert le protocole sur `endpoint` jusqu'à l'arrêt du moteur. Échoue tout de suite si
/// un autre moteur l'occupe déjà.
pub async fn listen(engine: Engine, endpoint: &str) -> io::Result<()> {
    imp::listen(engine, endpoint).await
}

/// Connexion de client à `endpoint`.
pub async fn connect(endpoint: &str) -> io::Result<Halves> {
    imp::connect(endpoint).await
}

fn serve_connection<S>(engine: Engine, stream: S)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, write) = tokio::io::split(stream);
    tokio::spawn(super::serve(engine, read, write));
}

fn already_running(endpoint: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AddrInUse,
        format!("un moteur écoute déjà sur {endpoint}"),
    )
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use tokio::net::{UnixListener, UnixStream};

    use super::{Halves, already_running, serve_connection};
    use crate::scheduler::Engine;

    pub async fn listen(engine: Engine, endpoint: &str) -> io::Result<()> {
        let path = Path::new(endpoint);
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
            // Échoue si le dossier appartient à un autre utilisateur.
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        if path.exists() {
            if UnixStream::connect(path).await.is_ok() {
                return Err(already_running(endpoint));
            }
            // Reste d'un moteur arrêté brutalement.
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    if let Ok((stream, _)) = accepted {
                        serve_connection(engine.clone(), stream);
                    }
                }
                _ = engine.stopped() => break,
            }
        }
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    pub async fn connect(endpoint: &str) -> io::Result<Halves> {
        let stream = UnixStream::connect(endpoint).await?;
        let (read, write) = stream.into_split();
        Ok((Box::new(read), Box::new(write)))
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::time::Duration;

    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

    use super::{Halves, already_running, serve_connection};
    use crate::scheduler::Engine;

    /// Tous les canaux du serveur sont occupés : le client réessaie.
    const ERROR_PIPE_BUSY: i32 = 231;

    pub async fn listen(engine: Engine, endpoint: &str) -> io::Result<()> {
        let security = security::OwnerOnly::new()?;
        let mut server = create(endpoint, &security, true).map_err(|e| {
            if e.kind() == io::ErrorKind::PermissionDenied {
                already_running(endpoint)
            } else {
                e
            }
        })?;
        loop {
            tokio::select! {
                connected = server.connect() => {
                    connected?;
                    // Le prochain client trouve une instance libre dès maintenant.
                    let next = create(endpoint, &security, false)?;
                    serve_connection(engine.clone(), std::mem::replace(&mut server, next));
                }
                _ = engine.stopped() => break,
            }
        }
        Ok(())
    }

    fn create(
        endpoint: &str,
        security: &security::OwnerOnly,
        first: bool,
    ) -> io::Result<NamedPipeServer> {
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        let mut attributes = security.attributes();
        // SAFETY : `attributes` pointe vers un descripteur valide tant que `security`
        // existe ; CreateNamedPipeW le copie.
        unsafe {
            options.create_with_security_attributes_raw(
                endpoint,
                (&raw mut attributes).cast::<std::ffi::c_void>(),
            )
        }
    }

    pub async fn connect(endpoint: &str) -> io::Result<Halves> {
        let mut tries = 0;
        let client = loop {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => break client,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && tries < 40 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        };
        let (read, write) = tokio::io::split(client);
        Ok((Box::new(read), Box::new(write)))
    }

    mod security {
        use std::ffi::c_void;
        use std::io;
        use std::ptr::null_mut;

        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
        use windows_sys::Win32::Security::Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::{
            GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER, TokenUser,
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
    }
}
