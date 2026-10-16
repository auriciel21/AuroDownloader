//! `aurod` : moteur d'AuroDownloader en processus autonome (SPEC §4.6).
//!
//! Il exécute les téléchargements et sert le protocole JSON-RPC (SPEC §6) sur le point
//! d'accès local de l'utilisateur : canal nommé sous Windows, socket Unix ailleurs. Les
//! interfaces (bureau, CLI) s'y connectent ; les téléchargements continuent sans elles.
//! Il s'arrête proprement sur Ctrl-C, sur SIGTERM (Unix) ou par `engine.shutdown`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use auro_engine::net::Client;
use auro_engine::paths;
use auro_engine::rpc::local;
use auro_engine::scheduler::{Config, Engine};
use auro_engine::transfer::Options;
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "aurod",
    version,
    about = "AuroDownloader : moteur de téléchargement"
)]
struct Args {
    /// Base du moteur (par défaut, dans le dossier de données de l'utilisateur)
    #[arg(long, value_name = "FICHIER")]
    db: Option<PathBuf>,

    /// Point d'accès local : canal nommé (Windows) ou chemin du socket (Unix)
    #[arg(long, value_name = "NOM")]
    endpoint: Option<String>,

    /// Autorités de certification à ajouter à celles du système (fichier PEM). La
    /// validation des certificats reste stricte.
    #[arg(long, value_name = "FICHIER")]
    ca_cert: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let db = match args.db {
        Some(db) => db,
        None => paths::database().context("dossier de données introuvable ; préciser --db")?,
    };
    if let Some(dir) = db.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("impossible de créer {}", dir.display()))?;
    }
    let endpoint = args.endpoint.unwrap_or_else(paths::endpoint);
    let client = match &args.ca_cert {
        None => Client::new()?,
        Some(path) => {
            let pem = std::fs::read(path)
                .with_context(|| format!("impossible de lire {}", path.display()))?;
            Client::with_extra_roots(&pem)?
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run(db, endpoint, client))
}

async fn run(db: PathBuf, endpoint: String, client: Client) -> Result<()> {
    let engine = Engine::start(Config {
        db_path: db.clone(),
        client,
        transfer: Options::default(),
    })
    .with_context(|| format!("impossible de démarrer le moteur sur {}", db.display()))?;
    let mut listening = tokio::spawn({
        let (engine, endpoint) = (engine.clone(), endpoint.clone());
        async move { local::listen(engine, &endpoint).await }
    });
    eprintln!("aurod : base {}, point d'accès {endpoint}", db.display());

    tokio::select! {
        served = &mut listening => {
            // Le point d'accès n'a pas pu s'ouvrir (déjà occupé…) : rien ne tourne encore.
            engine.shutdown().await;
            served?.with_context(|| format!("point d'accès {endpoint}"))?;
            return Ok(());
        }
        _ = stop_signal() => {
            eprintln!("aurod : arrêt demandé, enregistrement des tâches en cours…");
            engine.shutdown().await;
        }
        _ = engine.stopped() => {}
    }
    listening.await??;
    eprintln!("aurod : arrêté");
    Ok(())
}

/// Ctrl-C, ou SIGTERM sous Unix (arrêt par systemd).
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(term) => term,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
