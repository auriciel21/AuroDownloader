//! `aurod` : moteur d'AuroDownloader en processus autonome (SPEC §4.6).
//!
//! Il exécute les téléchargements et sert le protocole JSON-RPC (SPEC §6) sur le point
//! d'accès local de l'utilisateur : canal nommé sous Windows, socket Unix ailleurs. Les
//! interfaces (bureau, CLI) s'y connectent ; les téléchargements continuent sans elles.
//! Il s'arrête proprement sur Ctrl-C, sur SIGTERM (Unix) ou par `engine.shutdown`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use auro_engine::net::Client;
use auro_engine::paths;
use auro_engine::power;
use auro_engine::rpc::local;
use auro_engine::scheduler::{Config, DISK_RECHECK, Engine, Planning};
use auro_engine::secrets::Vault;
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

    /// Essais : l'action de fin d'une planification (veille, extinction) est écrite dans
    /// le journal au lieu d'être exécutée
    #[arg(long)]
    simulate_power: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    check_test_setup(
        args.endpoint.as_deref(),
        args.db.as_deref(),
        &paths::endpoint(),
    )?;
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
    let planning = Planning {
        power: if args.simulate_power {
            power::simulate
        } else {
            power::system
        },
        ..Planning::default()
    };
    runtime.block_on(run(db, endpoint, client, planning))
}

/// Un point d'accès autre que celui par défaut sert aux essais : sans `--db`, il ouvrirait
/// pourtant la base par défaut, celle du moteur réel (incident du 2026-11-08). Refusé avant
/// toute ouverture de base.
fn check_test_setup(endpoint: Option<&str>, db: Option<&Path>, default: &str) -> Result<()> {
    match (endpoint, db) {
        (Some(e), None) if e != default => anyhow::bail!(
            "--endpoint {e} sans --db : un point d'accès autre que celui par défaut ne sert \
             jamais la base par défaut, celle du moteur réel ; préciser --db"
        ),
        _ => Ok(()),
    }
}

async fn run(db: PathBuf, endpoint: String, client: Client, planning: Planning) -> Result<()> {
    let engine = Engine::start(Config {
        db_path: db.clone(),
        client,
        // Cookies et jetons des tâches : le coffre du système (SPEC §5.1). Empreintes lues
        // dans les fichiers voisins (F-11), selon le réglage du moteur.
        transfer: Options {
            vault: Vault::system(),
            neighbor_checksum: true,
            ..Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_endpoint_never_opens_the_default_database() {
        let default = r"\.\pipe\aurodownloader-alice";
        assert!(check_test_setup(Some(r"\.\pipe\essai"), None, default).is_err());
        assert!(check_test_setup(Some(r"\.\pipe\essai"), Some(Path::new("e.db")), default).is_ok());
        assert!(check_test_setup(Some(default), None, default).is_ok());
        assert!(check_test_setup(None, None, default).is_ok());
    }
}
