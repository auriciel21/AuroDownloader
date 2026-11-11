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
    use crate::winsec as security;

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
}
