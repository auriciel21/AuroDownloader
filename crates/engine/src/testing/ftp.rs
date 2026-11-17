//! Serveur FTP et FTPS de test (F-08, SPEC §7.22) : libunftp sur un dossier temporaire,
//! accès anonyme ou identifiant fixe, TLS explicite (`AUTH TLS`) par un certificat de
//! [`TestTls`], exigé ou non. Il compte les connexions de commande, et refuse par `421`
//! celles qui dépassent un plafond, comme un serveur qui limite les connexions par adresse.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use libunftp::options::FtpsRequired;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use unftp_core::auth::{AuthenticationError, Authenticator, Credentials, DefaultUser, Principal};

use super::TestTls;

/// Ce que le serveur accepte.
#[derive(Debug, Clone, Default)]
pub struct FtpOptions {
    /// Identifiant exigé ; `None` : accès anonyme (tout nom).
    pub login: Option<(&'static str, &'static str)>,
    /// TLS explicite proposé.
    pub tls: bool,
    /// TLS exigé pour s'identifier.
    pub require_tls: bool,
    /// Connexions de commande ouvertes ensemble au plus ; au-delà, `421`.
    pub max_open: Option<usize>,
}

#[derive(Debug)]
struct Fixed(&'static str, &'static str);

#[async_trait::async_trait]
impl Authenticator for Fixed {
    async fn authenticate(
        &self,
        username: &str,
        creds: &Credentials,
    ) -> Result<Principal, AuthenticationError> {
        if username != self.0 {
            return Err(AuthenticationError::BadUser);
        }
        if creds.password.as_deref() != Some(self.1) {
            return Err(AuthenticationError::BadPassword);
        }
        Ok(Principal {
            username: username.to_owned(),
        })
    }
}

/// Un serveur FTP de test, arrêté avec lui.
pub struct FtpSite {
    pub addr: SocketAddr,
    /// Dossier servi.
    pub root: PathBuf,
    /// Autorité du certificat (FTPS), à ajouter au client.
    pub ca_pem: Option<String>,
    accepted: Arc<AtomicUsize>,
    refused: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for FtpSite {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FtpSite {
    /// Démarre un serveur sur `127.0.0.1`, dans le runtime courant.
    pub async fn start(options: FtpOptions) -> FtpSite {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("racine");
        std::fs::create_dir(&root).unwrap();
        let tls = options.tls.then(|| {
            let tls = TestTls::new();
            std::fs::write(dir.path().join("cert.pem"), &tls.cert_pem).unwrap();
            std::fs::write(dir.path().join("key.pem"), &tls.key_pem).unwrap();
            tls
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted: Arc<AtomicUsize> = Arc::default();
        let refused: Arc<AtomicUsize> = Arc::default();
        let open: Arc<AtomicUsize> = Arc::default();
        let peak: Arc<AtomicUsize> = Arc::default();
        let highest = Arc::clone(&peak);
        let (served, keys) = (root.clone(), dir.path().to_owned());
        let (counted, turned) = (Arc::clone(&accepted), Arc::clone(&refused));
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let now = open.fetch_add(1, Ordering::SeqCst) + 1;
                if options.max_open.is_some_and(|max| now > max) {
                    open.fetch_sub(1, Ordering::SeqCst);
                    turned.fetch_add(1, Ordering::SeqCst);
                    let _ = stream
                        .write_all(b"421 Trop de connexions depuis cette adresse.\r\n")
                        .await;
                    continue;
                }
                counted.fetch_add(1, Ordering::SeqCst);
                highest.fetch_max(now, Ordering::SeqCst);
                let server = build(&served, &keys, &options);
                let open = Arc::clone(&open);
                tokio::spawn(async move {
                    let _ = server.service(stream).await;
                    open.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        FtpSite {
            addr,
            root,
            ca_pem: tls.map(|t| t.ca_pem),
            accepted,
            refused,
            peak,
            task,
            _dir: dir,
        }
    }

    /// `ftp://127.0.0.1:port/chemin` (ou un autre protocole, `ftps`).
    pub fn url(&self, path: &str) -> String {
        format!("ftp://{}/{path}", self.addr)
    }

    /// Écrit un fichier servi.
    pub fn put(&self, path: &str, bytes: &[u8]) {
        let file = self.root.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(file, bytes).unwrap();
    }

    /// Connexions de commande acceptées, et refusées par `421`.
    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    pub fn refused(&self) -> usize {
        self.refused.load(Ordering::SeqCst)
    }

    /// Le plus de connexions de commande ouvertes ensemble.
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

fn build(
    root: &Path,
    keys: &Path,
    options: &FtpOptions,
) -> libunftp::Server<unftp_sbe_fs::Filesystem, DefaultUser> {
    let root = root.to_owned();
    let storage = Box::new(move || unftp_sbe_fs::Filesystem::new(root.clone()).unwrap());
    let mut builder = match options.login {
        Some((user, pass)) => {
            libunftp::ServerBuilder::with_authenticator(storage, Arc::new(Fixed(user, pass)))
        }
        None => libunftp::ServerBuilder::new(storage),
    };
    if options.tls {
        builder = builder.ftps(keys.join("cert.pem"), keys.join("key.pem"));
        if options.require_tls {
            builder = builder.ftps_required(FtpsRequired::All, FtpsRequired::All);
        }
    }
    builder.build().unwrap()
}
