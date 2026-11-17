//! Serveur SFTP de test (F-08, SPEC §7.22) : russh et russh-sftp sur un dossier temporaire,
//! en lecture seule ; identification par mot de passe ou par clé publique. Sa clé d'hôte
//! est neuve à chaque démarrage, et peut changer (`replace_host_key`) pour simuler une
//! usurpation. Il compte les connexions SSH.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};
use russh::server::{Auth, Msg, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, Data, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::net::TcpListener;

/// Ce que le serveur accepte.
#[derive(Clone, Default)]
pub struct SftpOptions {
    /// Nom et mot de passe acceptés.
    pub password: Option<(&'static str, &'static str)>,
    /// Nom et clé publique acceptés.
    pub key: Option<(&'static str, PublicKey)>,
}

/// Une clé fabriquée pour les tests, et sa forme OpenSSH (chiffrée si `passphrase`).
pub fn user_key(passphrase: Option<&str>) -> (PublicKey, String) {
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let public = key.public_key().clone();
    let key = match passphrase {
        Some(p) => key.encrypt(&mut rand::rng(), p).unwrap(),
        None => key,
    };
    let text = key
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .unwrap()
        .to_string();
    (public, text)
}

/// Un serveur SFTP de test, arrêté avec lui.
pub struct SftpSite {
    pub addr: SocketAddr,
    pub root: PathBuf,
    host_key: Arc<Mutex<PrivateKey>>,
    accepted: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for SftpSite {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SftpSite {
    pub async fn start(options: SftpOptions) -> SftpSite {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("racine");
        std::fs::create_dir(&root).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let host_key = Arc::new(Mutex::new(
            PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap(),
        ));
        let accepted: Arc<AtomicUsize> = Arc::default();
        let (served, keys, counted) = (root.clone(), Arc::clone(&host_key), Arc::clone(&accepted));
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                let config = Arc::new(russh::server::Config {
                    keys: vec![keys.lock().unwrap().clone()],
                    auth_rejection_time: Duration::from_millis(10),
                    auth_rejection_time_initial: Some(Duration::ZERO),
                    ..russh::server::Config::default()
                });
                let handler = SshSession {
                    root: served.clone(),
                    options: options.clone(),
                    channels: HashMap::new(),
                };
                tokio::spawn(async move {
                    if let Ok(session) = russh::server::run_stream(config, stream, handler).await {
                        let _ = session.await;
                    }
                });
            }
        });
        SftpSite {
            addr,
            root,
            host_key,
            accepted,
            task,
            _dir: dir,
        }
    }

    /// `sftp://127.0.0.1:port/chemin absolu du fichier servi`.
    pub fn url(&self, path: &str) -> String {
        format!("sftp://{}/{path}", self.addr)
    }

    pub fn put(&self, path: &str, bytes: &[u8]) {
        let file = self.root.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(file, bytes).unwrap();
    }

    /// Empreinte de la clé d'hôte, comme le moteur la montre.
    pub fn fingerprint(&self) -> String {
        self.host_key
            .lock()
            .unwrap()
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string()
    }

    /// Une nouvelle clé d'hôte pour les connexions suivantes.
    pub fn replace_host_key(&self) {
        *self.host_key.lock().unwrap() =
            PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    }

    /// Connexions SSH acceptées.
    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

struct SshSession {
    root: PathBuf,
    options: SftpOptions,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl russh::server::Handler for SshSession {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(match self.options.password {
            Some((u, p)) if u == user && p == password => Auth::Accept,
            _ => Auth::reject(),
        })
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: russh::server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(match &self.options.key {
            Some((u, public)) if *u == user && public.key_data() == key.public_key.key_data() => {
                Auth::Accept
            }
            _ => Auth::reject(),
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match (name, self.channels.remove(&channel_id)) {
            ("sftp", Some(channel)) => {
                session.channel_success(channel_id)?;
                let files = Files {
                    root: self.root.clone(),
                    open: HashMap::new(),
                    next: 0,
                };
                tokio::spawn(russh_sftp::server::run(channel.into_stream(), files));
            }
            _ => session.channel_failure(channel_id)?,
        }
        Ok(())
    }
}

/// Le dossier servi, en lecture seule.
struct Files {
    root: PathBuf,
    open: HashMap<String, std::fs::File>,
    next: u64,
}

impl Files {
    fn local(&self, path: &str) -> PathBuf {
        let relative = path.trim_start_matches('/');
        let mut local = self.root.clone();
        for part in Path::new(relative).components() {
            if let std::path::Component::Normal(p) = part {
                local.push(p);
            }
        }
        local
    }

    fn attrs(&self, id: u32, path: &Path) -> Result<Attrs, StatusCode> {
        let meta = std::fs::metadata(path).map_err(|_| StatusCode::NoSuchFile)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

impl russh_sftp::server::Handler for Files {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        _pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let file =
            std::fs::File::open(self.local(&filename)).map_err(|_| StatusCode::NoSuchFile)?;
        self.next += 1;
        let handle = self.next.to_string();
        self.open.insert(handle.clone(), file);
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let file = self.open.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| StatusCode::Failure)?;
        let mut data = vec![0u8; len.min(256 * 1024) as usize];
        let n = file.read(&mut data).map_err(|_| StatusCode::Failure)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(n);
        Ok(Data { id, data })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.open.remove(&handle);
        Ok(ok(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.attrs(id, &self.local(&path))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.attrs(id, &self.local(&path))
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let file = self.open.get(&handle).ok_or(StatusCode::Failure)?;
        let meta = file.metadata().map_err(|_| StatusCode::Failure)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&meta),
        })
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![russh_sftp::protocol::File::dummy(path)],
        })
    }
}
