//! `auro` : ligne de commande d'AuroDownloader (F-40).
//!
//! La CLI est un client du moteur `aurod`, qu'elle joint par le protocole JSON-RPC sur le
//! point d'accès local (SPEC §6) et qu'elle démarre s'il ne tourne pas. Commandes : `add`,
//! `get` (ajoute et suit jusqu'à la fin), `list`, `pause`, `resume`, `cancel`, `watch`,
//! `queue`, `engine`. Sans moteur : `probe` prépare une tâche sans rien télécharger
//! (SPEC §7.1), `db migrate` crée ou met à jour une base, et `direct` exécute une tâche
//! avec la bibliothèque du moteur (bancs, dépannage).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use auro_engine::net::Client;
use auro_engine::paths;
use clap::{Parser, Subcommand};
use serde_json::json;

mod client;
mod direct;
mod display;
mod remote;

use client::{Rpc, Target};
use remote::AddOptions;

#[derive(Parser)]
#[command(name = "auro", version, about = "AuroDownloader : ligne de commande")]
struct Cli {
    /// Point d'accès du moteur : canal nommé (Windows) ou socket (Unix)
    #[arg(long, global = true, value_name = "NOM")]
    endpoint: Option<String>,

    /// Ne pas démarrer le moteur s'il ne tourne pas
    #[arg(long, global = true)]
    no_start: bool,

    /// Autorités de certification à ajouter à celles du système (fichier PEM) : pour
    /// `probe`, `direct` et le moteur démarré par cette commande. La validation des
    /// certificats reste stricte.
    #[arg(long, global = true, value_name = "FICHIER")]
    ca_cert: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// Connexions : de 1 à 32.
fn connections() -> clap::builder::RangedI64ValueParser<u32> {
    clap::value_parser!(u32).range(1..=32)
}

#[derive(Subcommand)]
enum Command {
    /// Ajoute des téléchargements au moteur, sans attendre
    Add {
        /// Adresses HTTP ou HTTPS
        #[arg(required = true)]
        urls: Vec<String>,
        /// Dossier de destination
        #[arg(short = 'o', long, default_value = ".")]
        dir: PathBuf,
        /// Nombre de connexions (1 à 32 ; automatique sinon)
        #[arg(short = 'c', long, value_parser = connections())]
        connections: Option<u32>,
        /// File (nom ou identifiant)
        #[arg(short = 'q', long)]
        queue: Option<String>,
        /// Ajouter en pause
        #[arg(long)]
        paused: bool,
    },
    /// Télécharge un fichier et suit l'avancement (Ctrl-C met la tâche en pause)
    Get {
        /// Adresse HTTP ou HTTPS
        url: String,
        /// Dossier de destination
        #[arg(short = 'o', long, default_value = ".")]
        dir: PathBuf,
        /// Nombre de connexions (1 à 32 ; automatique sinon)
        #[arg(short = 'c', long, value_parser = connections())]
        connections: Option<u32>,
        /// File (nom ou identifiant)
        #[arg(short = 'q', long)]
        queue: Option<String>,
    },
    /// Liste les tâches
    List {
        /// États à montrer (queued, active, paused, retry_wait, completed, failed…)
        #[arg(short = 's', long = "state")]
        states: Vec<String>,
        /// File (nom ou identifiant)
        #[arg(short = 'q', long)]
        queue: Option<String>,
    },
    /// Met des tâches en pause
    Pause {
        #[arg(required = true)]
        task_ids: Vec<String>,
    },
    /// Remet en file des tâches en pause ou en échec
    Resume {
        #[arg(required = true)]
        task_ids: Vec<String>,
    },
    /// Annule des tâches
    Cancel {
        #[arg(required = true)]
        task_ids: Vec<String>,
        /// Supprimer aussi les fichiers partiels
        #[arg(long)]
        delete_files: bool,
    },
    /// Suit l'avancement des tâches (toutes celles en cours par défaut)
    Watch { task_ids: Vec<String> },
    /// Files de téléchargement
    Queue {
        #[command(subcommand)]
        action: QueueAction,
    },
    /// Moteur : démarrage, état, arrêt
    Engine {
        #[command(subcommand)]
        action: EngineAction,
    },
    /// Interroge un serveur : taille, plages acceptées, validateurs et nom du fichier
    Probe {
        /// Adresse HTTP ou HTTPS du fichier
        url: String,
    },
    /// Gestion d'une base de données du moteur
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Sans moteur : exécute une tâche avec la bibliothèque (bancs, dépannage)
    Direct {
        /// Base de données utilisée directement (jamais celle d'un moteur en marche)
        #[arg(long, default_value = "auro.db")]
        db: PathBuf,
        #[command(subcommand)]
        action: DirectAction,
    },
}

#[derive(Subcommand)]
enum QueueAction {
    /// Liste les files
    List,
    /// Crée une file
    Create {
        name: String,
        /// Tâches simultanées (3 par défaut)
        #[arg(long)]
        max_active: Option<u32>,
    },
    /// Modifie une file (nom ou identifiant)
    Update {
        queue: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        max_active: Option<u32>,
    },
}

#[derive(Subcommand)]
enum EngineAction {
    /// Démarre le moteur s'il ne tourne pas
    Start {
        /// Base du moteur (par défaut, dans le dossier de données de l'utilisateur)
        #[arg(long, value_name = "FICHIER")]
        db: Option<PathBuf>,
    },
    /// Indique si le moteur tourne, et l'état de ses tâches
    Status,
    /// Arrête le moteur ; les tâches en cours repartiront au prochain démarrage
    Stop,
}

#[derive(Subcommand)]
enum DbAction {
    /// Crée la base ou la met à jour vers le dernier schéma
    Migrate {
        /// Chemin du fichier de base de données
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum DirectAction {
    /// Télécharge un fichier (Ctrl-C met la tâche en pause)
    Get {
        url: String,
        #[arg(short = 'o', long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'c', long, value_parser = connections())]
        connections: Option<u32>,
    },
    /// Reprend une tâche en pause, en échec ou interrompue
    Resume {
        task_id: String,
        #[arg(short = 'c', long, value_parser = connections())]
        connections: Option<u32>,
    },
    /// Liste les tâches de la base
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let target = Target {
        endpoint: cli.endpoint.clone().unwrap_or_else(paths::endpoint),
        autostart: !cli.no_start,
        ca_cert: cli.ca_cert.clone(),
    };
    match cli.command {
        Command::Probe { url } => probe(&url, cli.ca_cert.as_deref()),
        Command::Db {
            action: DbAction::Migrate { path },
        } => {
            let conn = direct::open_db(&path)?;
            let version = auro_engine::store::current_version(&conn)?;
            println!("Base prête : {} (schéma version {version})", path.display());
            Ok(())
        }
        Command::Direct { db, action } => {
            let client = http_client(cli.ca_cert.as_deref())?;
            match action {
                DirectAction::Get {
                    url,
                    dir,
                    connections,
                } => direct::get(&db, &url, &dir, connections, client),
                DirectAction::Resume {
                    task_id,
                    connections,
                } => direct::resume(&db, &task_id, connections, client),
                DirectAction::List => direct::list(&db),
            }
        }
        command => runtime()?.block_on(engine_command(command, &target)),
    }
}

async fn engine_command(command: Command, target: &Target) -> Result<()> {
    match command {
        Command::Get {
            url,
            dir,
            connections,
            queue,
        } => {
            let opts = AddOptions {
                dir: &dir,
                connections,
                queue: queue.as_deref(),
                paused: false,
            };
            remote::get(target, &url, &opts).await
        }
        Command::Engine { action } => engine(action, target).await,
        command => {
            let mut rpc = client::connect(target).await?;
            match command {
                Command::Add {
                    urls,
                    dir,
                    connections,
                    queue,
                    paused,
                } => {
                    let opts = AddOptions {
                        dir: &dir,
                        connections,
                        queue: queue.as_deref(),
                        paused,
                    };
                    remote::add(&mut rpc, &urls, &opts).await
                }
                Command::List { states, queue } => {
                    remote::list(&mut rpc, &states, queue.as_deref()).await
                }
                Command::Pause { task_ids } => {
                    remote::act(&mut rpc, "task.pause", &task_ids, json!({})).await
                }
                Command::Resume { task_ids } => {
                    remote::act(&mut rpc, "task.resume", &task_ids, json!({})).await
                }
                Command::Cancel {
                    task_ids,
                    delete_files,
                } => {
                    let extra = json!({"deleteFiles": delete_files});
                    remote::act(&mut rpc, "task.cancel", &task_ids, extra).await
                }
                Command::Watch { task_ids } => remote::watch(&mut rpc, &task_ids).await,
                Command::Queue { action } => match action {
                    QueueAction::List => remote::queues(&mut rpc).await,
                    QueueAction::Create { name, max_active } => {
                        remote::queue_create(&mut rpc, &name, max_active).await
                    }
                    QueueAction::Update {
                        queue,
                        name,
                        max_active,
                    } => remote::queue_update(&mut rpc, &queue, name.as_deref(), max_active).await,
                },
                _ => unreachable!("commandes sans moteur traitées dans main"),
            }
        }
    }
}

async fn engine(action: EngineAction, target: &Target) -> Result<()> {
    let endpoint = &target.endpoint;
    match action {
        EngineAction::Start { db } => {
            if Rpc::connect(endpoint).await.is_ok() {
                println!("Le moteur tourne déjà sur {endpoint}.");
                return Ok(());
            }
            client::start_daemon(endpoint, db.as_deref(), target.ca_cert.as_deref())?;
            client::wait_for(endpoint).await?;
            println!("Moteur démarré sur {endpoint}.");
            Ok(())
        }
        EngineAction::Status => match Rpc::connect(endpoint).await {
            Ok(mut rpc) => remote::status(&mut rpc, endpoint).await,
            Err(_) => {
                println!("Aucun moteur sur {endpoint}.");
                Ok(())
            }
        },
        EngineAction::Stop => {
            let Ok(mut rpc) = Rpc::connect(endpoint).await else {
                println!("Aucun moteur sur {endpoint}.");
                return Ok(());
            };
            rpc.call("engine.shutdown", json!({"graceful": true}))
                .await?;
            // Le moteur ferme la connexion quand il a tout enregistré.
            let _ = rpc.notification().await;
            println!("Moteur arrêté.");
            Ok(())
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

fn http_client(ca: Option<&Path>) -> Result<Client> {
    Ok(match ca {
        None => Client::new()?,
        Some(path) => {
            let pem = std::fs::read(path)
                .with_context(|| format!("impossible de lire {}", path.display()))?;
            Client::with_extra_roots(&pem)?
        }
    })
}

fn probe(url: &str, ca: Option<&Path>) -> Result<()> {
    let runtime = runtime()?;
    let client = http_client(ca)?;
    let probe = match runtime.block_on(client.probe(url, &[])) {
        Ok(probe) => probe,
        Err(error) => {
            let code = error.error_code().unwrap_or("demande invalide");
            let retry = if error.retryable() {
                "nouvelle tentative possible"
            } else {
                "échec définitif"
            };
            bail!("{error} [{code}, {retry}]");
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
