//! `auro` : ligne de commande d'AuroDownloader.
//!
//! Pour l'instant, une seule commande : `auro db migrate <chemin>` crée ou met à jour
//! la base du moteur. Les autres arriveront avec l'API JSON-RPC (SPEC §6).

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
    }
    Ok(())
}
