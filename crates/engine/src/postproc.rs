//! Commande de fin d'une tâche (F-34, SPEC §7.21).
//!
//! La règle retenue à la préparation (`tasks.rule_id`) peut porter une commande : un
//! programme et ses arguments, lancés sans shell quand le fichier est à son nom. La tâche
//! l'attend en `post_processing`, état durable : un arrêt du moteur pendant la commande la
//! relance au démarrage. Un code de sortie non nul ou un délai dépassé met la tâche en
//! `failed`, code `POSTPROCESS_FAILED`, le fichier gardé ; la relancer relance la commande.
//!
//! Rien de la commande ne va aux journaux (ni arguments, ni sorties) : seulement son
//! issue, avec l'identifiant de la tâche.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::watch;

use crate::rules::{COMMAND_VARIABLES, Command, substitute};
use crate::store::StoreError;
use crate::store::tasks::{Change, TaskState, change_state, complete_task, load_task};
use crate::transfer::{Outcome, TransferError};

/// Code d'échec d'une commande de fin (SPEC §6.8).
pub const FAILED: &str = "POSTPROCESS_FAILED";

/// La commande de fin d'une tâche : celle de sa règle, si la règle existe encore et est
/// activée.
pub fn pending(conn: &Connection, task_id: &str) -> Result<Option<Command>, StoreError> {
    let Some(rule_id) = crate::store::rules::task_rule(conn, task_id)? else {
        return Ok(None);
    };
    Ok(crate::store::rules::get(conn, &rule_id)?
        .filter(|r| r.enabled)
        .and_then(|r| r.actions.command))
}

/// Fin du téléchargement d'une tâche, fichier à son nom : terminée, ou en
/// `post_processing` si sa règle a une commande, que l'exécution lance ensuite
/// ([`run`]). Rend vrai dans ce second cas.
pub fn finish(
    conn: &mut Connection,
    task_id: &str,
    final_path: &Path,
    total: u64,
) -> Result<bool, StoreError> {
    if pending(conn, task_id)?.is_none() {
        complete_task(conn, task_id, &final_path.to_string_lossy(), total)?;
        return Ok(false);
    }
    enter(conn, task_id, total)?;
    Ok(true)
}

/// Passe la tâche en `post_processing` (déjà là pour une vidéo fusionnée), avec sa taille.
pub fn enter(conn: &mut Connection, task_id: &str, total: u64) -> Result<(), StoreError> {
    if load_task(conn, task_id)?.state != TaskState::PostProcessing {
        change_state(conn, task_id, &Change::to(TaskState::PostProcessing))?;
    }
    crate::store::tasks::set_finished_size(conn, task_id, total)
}

/// Ce que la commande a donné.
#[derive(Debug)]
pub enum Ran {
    /// Code de sortie nul (ou plus de commande à lancer).
    Done,
    /// Échec enregistré : tâche `failed`, `POSTPROCESS_FAILED`.
    Failed(Outcome),
    /// Arrêt du moteur : commande tuée, tâche toujours `post_processing`, relancée au
    /// démarrage.
    Stopped,
}

/// Chemin final d'une tâche : fichier, ou dossier d'un torrent à plusieurs fichiers.
pub fn final_path(conn: &Connection, task_id: &str) -> Result<PathBuf, StoreError> {
    let task = load_task(conn, task_id)?;
    Ok(Path::new(&task.dest_dir).join(task.file_name.as_deref().unwrap_or_default()))
}

/// Lance la commande de fin d'une tâche en `post_processing` et l'attend, dans la limite
/// de son délai ; `stop` (arrêt du moteur) la tue. N'enregistre que l'échec : la suite
/// d'une commande réussie (terminée, ou partage d'un torrent) revient à l'appelant.
pub async fn run(
    conn: &mut Connection,
    task_id: &str,
    stop: &mut watch::Receiver<bool>,
) -> Result<Ran, TransferError> {
    let Some(command) = pending(conn, task_id)? else {
        // Règle retirée ou désactivée depuis : plus rien à lancer.
        return Ok(Ran::Done);
    };
    let task = load_task(conn, task_id)?;
    let path = final_path(conn, task_id)?;
    let size = task.total_bytes.unwrap_or(task.done_bytes);
    let wants_sha256 = command.args.iter().any(|a| a.contains("{sha256}"));
    let sha256 = match (&task.checksum, wants_sha256) {
        // Empreinte SHA-256 vérifiée avant le renommage (F-11) : déjà connue.
        (Some(c), _) if c.algo() == crate::io::checksum::Algo::Sha256 => Some(c.value().to_owned()),
        (_, true) if path.is_file() => match hash(&path, stop).await {
            Some(Ok(digest)) => Some(digest),
            Some(Err(e)) => return fail(conn, task_id, format!("empreinte du fichier : {e}")),
            None => return Ok(Ran::Stopped),
        },
        _ => None,
    };
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let value = |var: &str| -> Option<String> {
        Some(match var {
            "chemin" | "path" => path.to_string_lossy().into_owned(),
            "dossier" | "dir" => dir.to_string_lossy().into_owned(),
            "nom" | "name" => name.clone(),
            "taille" | "size" => size.to_string(),
            "sha256" => sha256.clone().unwrap_or_default(),
            "id" => task_id.to_owned(),
            _ => return None,
        })
    };
    debug_assert!(COMMAND_VARIABLES.iter().all(|v| value(v).is_some()));
    let args = match command
        .args
        .iter()
        .map(|a| substitute(a, value))
        .collect::<Option<Vec<_>>>()
    {
        Some(args) => args,
        None => return fail(conn, task_id, "variable inconnue dans un argument".into()),
    };

    let mut process = tokio::process::Command::new(&command.program);
    process
        .args(&args)
        .current_dir(if dir.is_dir() { &dir } else { Path::new(".") })
        .env("AURO_PATH", &path)
        .env("AURO_DIR", &dir)
        .env("AURO_NAME", &name)
        .env("AURO_SIZE", size.to_string())
        .env("AURO_TASK_ID", task_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match &sha256 {
        Some(digest) => process.env("AURO_SHA256", digest),
        None => process.env_remove("AURO_SHA256"),
    };
    #[cfg(windows)]
    // CREATE_NO_WINDOW : pas de fenêtre de console.
    process.creation_flags(0x0800_0000);
    crate::media::process::prepare(&mut process);
    let (mut child, tree) = match crate::media::process::spawn(&mut process) {
        Ok(spawned) => spawned,
        Err(e) => {
            return fail(
                conn,
                task_id,
                format!("lancement de la commande impossible : {e}"),
            );
        }
    };
    tracing::info!(tache = %task_id, "commande de fin lancée");
    let limit = Duration::from_secs(u64::from(command.timeout_s));
    let ended = tokio::select! {
        status = child.wait() => Some(status),
        () = tokio::time::sleep(limit) => None,
        () = stopped(stop) => {
            drop(tree);
            let _ = child.wait().await;
            tracing::info!(tache = %task_id, "commande de fin arrêtée avec le moteur");
            return Ok(Ran::Stopped);
        }
    };
    // L'arbre entier meurt avec le garde, même si le premier processus a fini.
    drop(tree);
    match ended {
        Some(Ok(status)) if status.success() => {
            tracing::info!(tache = %task_id, "commande de fin réussie");
            Ok(Ran::Done)
        }
        Some(Ok(status)) => {
            let detail = match status.code() {
                Some(code) => format!("la commande a rendu le code {code}"),
                None => "la commande a été arrêtée par un signal".into(),
            };
            fail(conn, task_id, detail)
        }
        Some(Err(e)) => fail(conn, task_id, format!("attente de la commande : {e}")),
        None => {
            let _ = child.wait().await;
            fail(
                conn,
                task_id,
                format!(
                    "délai de {} s dépassé : commande arrêtée",
                    command.timeout_s
                ),
            )
        }
    }
}

/// Termine une tâche dont la commande a réussi (pas un torrent qui partage ensuite).
pub fn complete(conn: &mut Connection, task_id: &str) -> Result<Outcome, TransferError> {
    let task = load_task(conn, task_id)?;
    let path = final_path(conn, task_id)?;
    complete_task(
        conn,
        task_id,
        &path.to_string_lossy(),
        task.total_bytes.unwrap_or(task.done_bytes),
    )?;
    Ok(Outcome::Completed(path))
}

fn fail(conn: &mut Connection, task_id: &str, detail: String) -> Result<Ran, TransferError> {
    tracing::warn!(tache = %task_id, code = FAILED, "commande de fin en échec");
    let change = Change {
        error: Some((FAILED, &detail)),
        ..Change::to(TaskState::Failed)
    };
    change_state(conn, task_id, &change)?;
    Ok(Ran::Failed(Outcome::Failed {
        code: FAILED,
        detail,
    }))
}

/// Attend une demande d'arrêt ; jamais si l'émetteur a disparu.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    if stop.wait_for(|s| *s).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// SHA-256 du fichier, hors de la boucle ; `None` si l'arrêt est demandé avant la fin.
async fn hash(path: &Path, stop: &mut watch::Receiver<bool>) -> Option<std::io::Result<String>> {
    let cancel = Arc::new(AtomicBool::new(false));
    let (path, flag) = (path.to_path_buf(), Arc::clone(&cancel));
    let mut work = tokio::task::spawn_blocking(move || {
        crate::io::checksum::hash_file(&path, crate::io::checksum::Algo::Sha256, |_| {
            !flag.load(Ordering::Relaxed)
        })
    });
    tokio::select! {
        done = &mut work => match done {
            Ok(Ok(Some(digest))) => Some(Ok(digest)),
            Ok(Ok(None)) => None,
            Ok(Err(e)) => Some(Err(e)),
            Err(e) => Some(Err(std::io::Error::other(e))),
        },
        () = stopped(stop) => {
            cancel.store(true, Ordering::Relaxed);
            let _ = work.await;
            None
        }
    }
}

#[cfg(test)]
mod tests;
