//! `auro` : ligne de commande d'AuroDownloader.
//!
//! En phase 1, la CLI appelle le moteur comme une bibliothèque (SPEC §9.3). Commandes :
//! `get` télécharge un fichier, `resume` reprend une tâche interrompue, `list` montre les
//! tâches, `probe` prépare une tâche sans rien télécharger (SPEC §7.1), `db migrate` crée
//! ou met à jour la base.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use auro_engine::net::Client;
use auro_engine::store::tasks::{self, NewTask, TaskState};
use auro_engine::transfer::{self, Options, Outcome, Progress};
use clap::{Parser, Subcommand};
use rusqlite::Connection;
use tokio::sync::watch;

#[derive(Parser)]
#[command(name = "auro", version, about = "AuroDownloader : ligne de commande")]
struct Cli {
    /// Base de données du moteur
    #[arg(long, global = true, default_value = "auro.db")]
    db: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Télécharge un fichier (Ctrl-C met la tâche en pause)
    Get {
        /// Adresse HTTP ou HTTPS du fichier
        url: String,
        /// Dossier de destination
        #[arg(short = 'o', long, default_value = ".")]
        dir: PathBuf,
        /// Nombre de connexions (1 à 32)
        #[arg(short = 'c', long, value_parser = clap::value_parser!(u32).range(1..=32))]
        connections: Option<u32>,
    },
    /// Reprend une tâche en pause, en échec ou interrompue
    Resume {
        /// Identifiant de la tâche (voir `auro list`)
        task_id: String,
        /// Nombre de connexions (1 à 32)
        #[arg(short = 'c', long, value_parser = clap::value_parser!(u32).range(1..=32))]
        connections: Option<u32>,
    },
    /// Liste les tâches
    List,
    /// Interroge un serveur : taille, plages acceptées, validateurs et nom du fichier
    Probe {
        /// Adresse HTTP ou HTTPS du fichier
        url: String,
    },
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
            let conn = open_db(&path)?;
            let version = auro_engine::store::current_version(&conn)?;
            println!("Base prête : {} (schéma version {version})", path.display());
        }
        Command::Probe { url } => probe(&url)?,
        Command::Get {
            url,
            dir,
            connections,
        } => get(&cli.db, &url, &dir, connections)?,
        Command::Resume {
            task_id,
            connections,
        } => resume(&cli.db, &task_id, connections)?,
        Command::List => list(&cli.db)?,
    }
    Ok(())
}

fn open_db(path: &Path) -> Result<Connection> {
    auro_engine::store::open(path)
        .with_context(|| format!("impossible d'ouvrir la base {}", path.display()))
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

fn get(db: &Path, url: &str, dir: &Path, connections: Option<u32>) -> Result<()> {
    auro_engine::net::check_url(url)?;
    let dir = std::path::absolute(dir)?;
    if !dir.is_dir() {
        bail!("le dossier {} n'existe pas", dir.display());
    }
    let dir_text = dir
        .to_str()
        .context("le chemin du dossier n'est pas en UTF-8")?;
    let mut conn = open_db(db)?;
    let queue = tasks::ensure_queue(&conn, "default")?;
    let display_name = transfer::display_name_for(url);
    let (task_id, _) = tasks::create_task(
        &mut conn,
        &NewTask {
            queue_id: &queue,
            url,
            display_name: &display_name,
            dest_dir: dir_text,
            max_connections: connections,
        },
    )?;
    println!("Tâche {task_id}");
    execute(&mut conn, &task_id, connections)
}

fn resume(db: &Path, task_id: &str, connections: Option<u32>) -> Result<()> {
    let mut conn = open_db(db)?;
    let task = tasks::load_task(&conn, task_id)?;
    match task.state {
        TaskState::Paused if task.pause_reason.as_deref() == Some("file_changed") => bail!(
            "le fichier distant a changé depuis le début du téléchargement ; repartir de \
             zéro n'est pas encore possible (F-13)"
        ),
        TaskState::Paused | TaskState::Failed => transfer::requeue(&mut conn, task_id)?,
        _ => {}
    }
    execute(&mut conn, task_id, connections)
}

fn execute(conn: &mut Connection, task_id: &str, connections: Option<u32>) -> Result<()> {
    let runtime = runtime()?;
    let client = Client::new()?;
    // Sans -c : le nombre enregistré dans la tâche, sinon automatique (SPEC §7.3).
    let opts = Options {
        connections,
        ..Options::default()
    };
    let (stop_tx, stop) = watch::channel(false);
    let mut display = Display::new();
    let outcome = runtime.block_on(async {
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                let _ = stop_tx.send(true);
            }
        });
        transfer::run(conn, &client, task_id, &opts, stop, &mut |p| {
            display.show(p)
        })
        .await
    })?;
    eprintln!();
    match outcome {
        Outcome::Completed(path) => println!("Terminé : {}", path.display()),
        Outcome::Paused => println!("En pause. Pour reprendre : auro resume {task_id}"),
        Outcome::FileChanged => {
            bail!("le fichier distant a changé : tâche en pause [FILE_CHANGED]")
        }
        Outcome::DiskFull => bail!("disque plein : tâche en pause [DISK_FULL]"),
        Outcome::RetryLater { code, detail, .. } => {
            bail!("{detail} [{code}] ; nouvelle tentative possible : auro resume {task_id}")
        }
        Outcome::Failed { code, detail } => bail!("{detail} [{code}]"),
    }
    Ok(())
}

/// Ligne d'avancement, réécrite sur place.
struct Display {
    last: Option<(Instant, u64)>,
    speed: f64,
}

impl Display {
    fn new() -> Self {
        Display {
            last: None,
            speed: 0.0,
        }
    }

    fn show(&mut self, p: Progress) {
        let now = Instant::now();
        if let Some((at, bytes)) = self.last {
            let elapsed = now.duration_since(at).as_secs_f64();
            if elapsed > 0.0 {
                let instant = p.received.saturating_sub(bytes) as f64 / elapsed;
                self.speed = if self.speed == 0.0 {
                    instant
                } else {
                    0.7 * self.speed + 0.3 * instant
                };
            }
        }
        self.last = Some((now, p.received));
        let done = match p.total {
            Some(total) if total > 0 => format!(
                "{:5.1} %  {} / {}",
                p.received as f64 * 100.0 / total as f64,
                size(p.received),
                size(total)
            ),
            _ => size(p.received),
        };
        eprint!(
            "\r{done}  {}/s  {} connexion(s)        ",
            size(self.speed as u64),
            p.connections
        );
        let _ = std::io::stderr().flush();
    }
}

fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Kio", "Mio", "Gio", "Tio"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} o")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn list(db: &Path) -> Result<()> {
    let conn = open_db(db)?;
    for t in tasks::list_tasks(&conn)? {
        let amount = match t.total_bytes {
            Some(total) => format!("{} / {}", size(t.done_bytes), size(total)),
            None => size(t.done_bytes),
        };
        let note = t
            .pause_reason
            .as_deref()
            .or(t.error_code.as_deref())
            .map(|n| format!(" ({n})"))
            .unwrap_or_default();
        println!(
            "{}  {:<26} {:>22}  {}",
            t.id,
            format!("{}{note}", t.state.as_sql()),
            amount,
            t.display_name
        );
    }
    Ok(())
}

fn probe(url: &str) -> Result<()> {
    let runtime = runtime()?;
    let client = Client::new()?;
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
