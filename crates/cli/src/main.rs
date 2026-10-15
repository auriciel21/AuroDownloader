//! `auro` : ligne de commande d'AuroDownloader.
//!
//! En phase 1, la CLI appelle le moteur comme une bibliothèque (SPEC §9.3) :
//! `auro db migrate <chemin>` crée ou met à jour la base du moteur, `auro probe <url>`
//! prépare une tâche sans rien télécharger (SPEC §7.1).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "auro", version, about = "AuroDownloader : ligne de commande")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Gestion de la base de données du moteur
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Interroge un serveur : taille, plages acceptées, validateurs et nom du fichier
    Probe {
        /// Adresse HTTP ou HTTPS du fichier
        url: String,
    },
}

#[derive(Subcommand)]
enum DbAction {
    /// Crée la base ou la met à jour vers le dernier schéma
    Migrate {
        /// Chemin du fichier de base de données
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Db {
            action: DbAction::Migrate { path },
        } => {
            let conn = auro_engine::store::open(&path)
                .with_context(|| format!("impossible d'ouvrir la base {}", path.display()))?;
            let version = auro_engine::store::current_version(&conn)?;
            println!("Base prête : {} (schéma version {version})", path.display());
        }
        Command::Probe { url } => probe(&url)?,
    }
    Ok(())
}

fn probe(url: &str) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let client = auro_engine::net::Client::new()?;
    let probe = match runtime.block_on(client.probe(url, &[])) {
        Ok(probe) => probe,
        Err(error) => {
            let code = error.error_code().unwrap_or("demande invalide");
            let retry = if error.retryable() {
                "nouvelle tentative possible"
            } else {
                "échec définitif"
            };
            anyhow::bail!("{error} [{code}, {retry}]");
        }
    };

    // Sans la requête : elle peut porter un jeton (SPEC §8.2).
    let mut shown = probe.final_url.clone();
    shown.set_query(None);
    println!("Adresse finale  : {shown}");
    println!("Nom du fichier  : {}", probe.file_name);
    match probe.total_bytes {
        Some(total) => println!("Taille          : {total} octets"),
        None => println!("Taille          : inconnue"),
    }
    if probe.accepts_ranges {
        println!("Plages          : acceptées (segmentation et reprise possibles)");
    } else {
        println!("Plages          : refusées (un seul flux, reprise impossible)");
    }
    println!(
        "ETag            : {}",
        probe.etag.as_deref().unwrap_or("absent")
    );
    println!(
        "Last-Modified   : {}",
        probe.last_modified.as_deref().unwrap_or("absent")
    );
    if probe.downgraded {
        println!("Attention       : une redirection est passée de HTTPS à HTTP");
    }
    Ok(())
}
