//! Prototype FTP, FTPS et SFTP (SPEC F-08, 7.22, phase 7, étape 6a). Télécharge un fichier
//! sur N connexions (une plage chacune), mesure le temps et le débit, vérifie l'empreinte
//! SHA-256 si elle est donnée. Ne fait pas partie du produit.
//!
//! ```text
//! proto ftp  <ftp://[user[:pass]@]hôte[:port]/chemin> [-c N] [-o fichier] [--tls|--implicit] [--sha256 HEX]
//! proto sftp <sftp://user[:pass]@hôte[:port]/chemin>  [-c N] [-o fichier] [--key fichier] [--accept EMPREINTE] [--sha256 HEX]
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

struct Args {
    url: url::Url,
    connections: u64,
    out: PathBuf,
    tls: Tls,
    key: Option<PathBuf>,
    accept: Option<String>,
    sha256: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Tls {
    None,
    Explicit,
    Implicit,
}

fn args() -> Result<(String, Args)> {
    let mut it = std::env::args().skip(1);
    let mode = it.next().context("mode : ftp ou sftp")?;
    let url = url::Url::parse(&it.next().context("adresse")?)?;
    let mut a = Args {
        url,
        connections: 1,
        out: PathBuf::from("sortie.bin"),
        tls: Tls::None,
        key: None,
        accept: None,
        sha256: None,
    };
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "-c" => a.connections = it.next().context("-c N")?.parse()?,
            "-o" => a.out = it.next().context("-o fichier")?.into(),
            "--tls" => a.tls = Tls::Explicit,
            "--implicit" => a.tls = Tls::Implicit,
            "--key" => a.key = Some(it.next().context("--key fichier")?.into()),
            "--accept" => a.accept = Some(it.next().context("--accept empreinte")?),
            "--sha256" => a.sha256 = Some(it.next().context("--sha256")?.to_lowercase()),
            other => bail!("option inconnue {other}"),
        }
    }
    if a.url.scheme() == "ftps" {
        a.tls = Tls::Implicit;
    }
    Ok((mode, a))
}

/// Plages de N connexions sur `total` octets.
fn ranges(total: u64, n: u64) -> Vec<(u64, u64)> {
    let n = n.max(1).min(total.max(1));
    let size = total.div_ceil(n);
    (0..n)
        .map(|i| (i * size, ((i + 1) * size).min(total)))
        .filter(|(a, b)| a < b)
        .collect()
}

fn decoded(text: &str) -> String {
    url::form_urlencoded::parse(format!("x={text}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    // Serveur FTP anonyme local du banc (libunftp) : `proto serve-ftp <dossier> <adresse>`.
    let mut raw = std::env::args().skip(1);
    if raw.next().as_deref() == Some("serve-ftp") {
        let home = PathBuf::from(raw.next().context("dossier")?);
        let bind = raw.next().unwrap_or_else(|| "127.0.0.1:2121".into());
        let server = libunftp::ServerBuilder::new(Box::new(move || {
            unftp_sbe_fs::Filesystem::new(home.clone()).expect("dossier servi")
        }))
        .greeting("banc AuroDownloader")
        .passive_ports(50000..=50100)
        .build()?;
        println!("FTP sur {bind}");
        server.listen(bind).await?;
        return Ok(());
    }
    let (mode, a) = args()?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&a.out)?;
    drop(file);
    let start = Instant::now();
    let total = match mode.as_str() {
        "ftp" => ftp::download(&a).await?,
        "sftp" => sftp::download(&a).await?,
        other => bail!("mode inconnu {other}"),
    };
    let secs = start.elapsed().as_secs_f64();
    println!(
        "{total} octets en {secs:.2} s, {:.1} Mio/s, {} connexion(s)",
        total as f64 / secs / 1_048_576.0,
        a.connections
    );
    if let Some(expected) = &a.sha256 {
        let data = std::fs::read(&a.out)?;
        let got: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
        println!("SHA-256 {}", if &got == expected { "juste" } else { "FAUSSE" });
    }
    Ok(())
}

mod ftp {
    use super::*;
    use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream};
    use suppaftp::types::FileType;

    fn connector() -> AsyncRustlsConnector {
        use rustls_platform_verifier::BuilderVerifierExt;
        let config = rustls::ClientConfig::builder()
            .with_platform_verifier()
            .expect("vérificateur du système")
            .with_no_client_auth();
        AsyncRustlsConnector::from(tokio_rustls::TlsConnector::from(Arc::new(config)))
    }

    /// Connexion ouverte, identifiée, en binaire.
    async fn open(a: &Args) -> Result<AsyncRustlsFtpStream> {
        let host = a.url.host_str().context("hôte")?.to_owned();
        let port = a.url.port().unwrap_or(if a.tls == Tls::Implicit { 990 } else { 21 });
        let mut ftp = match a.tls {
            Tls::Implicit => {
                AsyncRustlsFtpStream::connect_secure_implicit((host.as_str(), port), connector(), &host)
                    .await?
            }
            _ => AsyncRustlsFtpStream::connect((host.as_str(), port)).await?,
        };
        if a.tls == Tls::Explicit {
            ftp = ftp.into_secure(connector(), &host).await?;
        }
        // TLS implicite : suppaftp ne demande pas la protection des données (RFC 4217).
        if a.tls == Tls::Implicit {
            use suppaftp::Status;
            ftp.custom_command("PBSZ 0", &[Status::CommandOk]).await?;
            ftp.custom_command("PROT P", &[Status::CommandOk]).await?;
        }
        let user = match a.url.username() {
            "" => "anonymous".to_owned(),
            u => decoded(u),
        };
        let pass = a.url.password().map(decoded).unwrap_or_else(|| "essai@".into());
        // EPSV (RFC 2428) : le seul mode passif possible en IPv6 ; l'adresse des données est
        // celle de la connexion de commande. Variable AURO_PASV=1 : PASV, adresse corrigée.
        if std::env::var("AURO_PASV").is_ok() {
            ftp.set_mode(suppaftp::Mode::Passive);
            ftp.set_passive_nat_workaround(true);
        } else {
            ftp.set_mode(suppaftp::Mode::ExtendedPassive);
        }
        ftp.login(user.as_str(), pass.as_str()).await?;
        ftp.transfer_type(FileType::Binary).await?;
        Ok(ftp)
    }

    pub async fn download(a: &Args) -> Result<u64> {
        let path = decoded(a.url.path());
        let mut first = Some(open(a).await?);
        let total = first.as_mut().expect("ouverte").size(&path).await? as u64;
        println!("taille : {total} ; accueil : {:?}", first.as_ref().expect("ouverte").get_welcome_msg().map(|m| m.lines().next()));
        let parts = ranges(total, a.connections);
        let mut jobs = tokio::task::JoinSet::new();
        for (start, end) in parts {
            let a_url = a.url.clone();
            let out = a.out.clone();
            let path = path.clone();
            let reuse = first.take();
            let args = Args { url: a_url, connections: 1, out: out.clone(), tls: a.tls, key: None, accept: None, sha256: None };
            jobs.spawn(async move {
                let mut ftp = match reuse {
                    Some(f) => f,
                    None => open(&args).await?,
                };
                ftp.resume_transfer(start as usize).await?;
                let mut stream = ftp.retr_as_stream(&path).await?;
                // Le fichier de sortie, ouvert une fois par plage, à sa place.
                let mut sink = tokio::fs::OpenOptions::new().write(true).open(&out).await?;
                sink.seek(std::io::SeekFrom::Start(start)).await?;
                let mut sink = tokio::io::BufWriter::with_capacity(1 << 20, sink);
                let mut want = end - start;
                let mut at = start;
                let mut buf = vec![0u8; 256 * 1024];
                while want > 0 {
                    let room = want.min(buf.len() as u64) as usize;
                    let n = stream.read(&mut buf[..room]).await?;
                    if n == 0 {
                        bail!("fin du flux à {at}, {want} octets manquants");
                    }
                    sink.write_all(&buf[..n]).await?;
                    at += n as u64;
                    want -= n as u64;
                }
                sink.flush().await?;
                // Plage reçue : la fin du fichier, ou un arrêt demandé au serveur.
                if end == total {
                    stream.finish().await?;
                } else {
                    ftp.abort(stream).await?;
                }
                let _ = ftp.quit().await;
                anyhow::Ok(end - start)
            });
        }

        let mut got = 0;
        while let Some(done) = jobs.join_next().await {
            got += done??;
        }
        Ok(got)
    }
}

mod sftp {
    use super::*;
    use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
    use russh_sftp::client::SftpSession;

    struct Client {
        accept: Option<String>,
    }

    impl russh::client::Handler for Client {
        type Error = anyhow::Error;

        async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
            let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
            println!("clé du serveur : {} {fingerprint}", key.public_key().algorithm());
            Ok(self.accept.as_deref() == Some(fingerprint.as_str()))
        }
    }

    async fn session(a: &Args) -> Result<SftpSession> {
        let host = a.url.host_str().context("hôte")?.to_owned();
        let port = a.url.port().unwrap_or(22);
        let config = Arc::new(russh::client::Config::default());
        let client = Client { accept: a.accept.clone() };
        let mut ssh = russh::client::connect(config, (host.as_str(), port), client).await?;
        let user = decoded(a.url.username());
        let ok = match (&a.key, a.url.password()) {
            (Some(key), _) => {
                let key = russh::keys::load_secret_key(key, None)?;
                let hash = ssh.best_supported_rsa_hash().await?.flatten();
                ssh.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                    .await?
                    .success()
            }
            (None, Some(pass)) => ssh.authenticate_password(user, decoded(pass)).await?.success(),
            (None, None) => bail!("mot de passe ou --key"),
        };
        if !ok {
            bail!("authentification refusée");
        }
        let channel = ssh.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        Ok(SftpSession::new(channel.into_stream()).await?)
    }

    pub async fn download(a: &Args) -> Result<u64> {
        let path = decoded(a.url.path());
        let first = session(a).await?;
        let total = first.metadata(&path).await?.size.context("taille inconnue")?;
        println!("taille : {total}");
        let first = Arc::new(first);
        // AURO_SEP=1 : une connexion SSH par plage ; sinon toutes sur la même.
        let separate = std::env::var("AURO_SEP").is_ok();
        let mut jobs = tokio::task::JoinSet::new();
        for (i, (start, end)) in ranges(total, a.connections).into_iter().enumerate() {
            let sftp = if separate && i > 0 { Arc::new(session(a).await?) } else { Arc::clone(&first) };
            let (path, out) = (path.clone(), a.out.clone());
            jobs.spawn(async move {
                let mut file = sftp.open(&path).await?;
                file.seek(std::io::SeekFrom::Start(start)).await?;
                // Le fichier de sortie, ouvert une fois par plage, à sa place.
                let mut sink = tokio::fs::OpenOptions::new().write(true).open(&out).await?;
                sink.seek(std::io::SeekFrom::Start(start)).await?;
                let mut sink = tokio::io::BufWriter::with_capacity(1 << 20, sink);
                let mut want = end - start;
                let mut at = start;
                let mut buf = vec![0u8; 256 * 1024];
                while want > 0 {
                    let room = want.min(buf.len() as u64) as usize;
                    let n = file.read(&mut buf[..room]).await?;
                    if n == 0 {
                        bail!("fin du fichier à {at}");
                    }
                    sink.write_all(&buf[..n]).await?;
                    at += n as u64;
                    want -= n as u64;
                }
                sink.flush().await?;
                anyhow::Ok(end - start)
            });
        }
        let mut got = 0;
        while let Some(done) = jobs.join_next().await {
            got += done??;
        }
        Ok(got)
    }
}
