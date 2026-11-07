//! Mode direct : la CLI exécute elle-même une tâche avec la bibliothèque du moteur, sans
//! `aurod`. Il sert aux bancs (reprise sous arrêt brutal, performance), qui mesurent le
//! transfert seul, et au dépannage. Ne pas le mêler à un `aurod` qui utilise la même
//! base : deux planificateurs se disputeraient les tâches.

use std::path::Path;

use anyhow::{Context, Result, bail};
use auro_engine::net::Client;
use auro_engine::store::tasks::{self, NewTask, PauseReason, TaskState};
use auro_engine::transfer::{self, Options, Outcome, Progress};
use rusqlite::Connection;
use tokio::sync::watch;

use crate::display::{Line, Speed, Status, size};

pub fn open_db(path: &Path) -> Result<Connection> {
    auro_engine::store::open(path)
        .with_context(|| format!("impossible d'ouvrir la base {}", path.display()))
}

pub fn get(
    db: &Path,
    url: &str,
    dir: &Path,
    connections: Option<u32>,
    client: Client,
) -> Result<()> {
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
            speed_limit_bps: None,
        },
    )?;
    println!("Tâche {task_id}");
    execute(&mut conn, &task_id, connections, client)
}

pub fn resume(db: &Path, task_id: &str, connections: Option<u32>, client: Client) -> Result<()> {
    let mut conn = open_db(db)?;
    let task = tasks::load_task(&conn, task_id)?;
    match task.state {
        TaskState::Paused
            if task.pause_reason.as_deref() == Some(PauseReason::FileChanged.as_sql()) =>
        {
            bail!(
                "le fichier distant a changé depuis le début du téléchargement ; repartir de \
                 zéro n'est pas encore possible (F-13)"
            )
        }
        TaskState::Paused | TaskState::Failed => transfer::requeue(&mut conn, task_id)?,
        _ => {}
    }
    execute(&mut conn, task_id, connections, client)
}

fn execute(
    conn: &mut Connection,
    task_id: &str,
    connections: Option<u32>,
    client: Client,
) -> Result<()> {
    let runtime = crate::runtime()?;
    // Sans -c : le nombre enregistré dans la tâche, sinon automatique (SPEC §7.3).
    let opts = Options {
        connections,
        ..Options::default()
    };
    let (stop_tx, stop) = watch::channel(false);
    let (mut status, mut speed) = (Status::new(), Speed::new());
    let mut show = |p: Progress| {
        let bps = speed.update(p.received);
        let line = Line {
            done: p.received,
            total: p.total,
            bps,
            connections: p.connections as u64,
            eta: None,
        };
        status.show(&[line.render()]);
    };
    let outcome = runtime.block_on(async {
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                let _ = stop_tx.send(true);
            }
        });
        transfer::run(conn, &client, task_id, &opts, stop, &mut show).await
    })?;
    match outcome {
        Outcome::Completed(path) => println!("Terminé : {}", path.display()),
        Outcome::Paused => println!("En pause. Pour reprendre : auro direct resume {task_id}"),
        Outcome::FileChanged => {
            bail!("le fichier distant a changé : tâche en pause [FILE_CHANGED]")
        }
        Outcome::DiskFull => bail!("disque plein : tâche en pause [DISK_FULL]"),
        Outcome::RetryLater { code, detail, .. } => {
            bail!("{detail} [{code}] ; nouvelle tentative possible : auro direct resume {task_id}")
        }
        Outcome::Failed { code, detail } => bail!("{detail} [{code}]"),
    }
    Ok(())
}

pub fn list(db: &Path) -> Result<()> {
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
