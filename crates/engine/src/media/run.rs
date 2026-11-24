//! Exécution d'une tâche vidéo (SPEC §7.17, étape 4c) : résolution des flux par yt-dlp,
//! téléchargement de chaque flux, puis fusion (`post_processing`).
//!
//! Un flux direct se télécharge comme une tâche HTTP interne (`tasks.parent_id`), par
//! `transfer::run` lui-même : règle 1, segments, limites et reprise vérifiée, sans rien de
//! propre à la vidéo. Un flux fragmenté (HLS, DASH à fragments) est confié à yt-dlp, qui
//! reprend ses fichiers partiels ; le moteur ne vide son fichier sur disque qu'à la fin.
//!
//! Chaque exécution refait la résolution : les adresses des flux expirent. Une adresse
//! refusée (`HTTP_STATUS`) la fait refaire, au plus [`MAX_REFRESH`] fois.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _};
use tokio::sync::watch;

use super::extract::{self, ExtractError, Resolution, Resolved, Subtitle, SubtitleSource};
use super::tools::{Status, Tools};
use crate::claims;
use crate::io::{part_path, sync_parent_dir};
use crate::net::Client;
use crate::net::filename::{NameRules, sanitize};
use crate::store::claims::ClaimKind;
use crate::store::headers::{Stored, StoredHeader};
use crate::store::media::{self, StreamRow};
use crate::store::now_ms;
use crate::store::tasks::{
    self, Change, NewTask, PauseReason, TaskRow, TaskState, change_state, load_task, reserved_names,
};
use crate::transfer::{self, MAX_FAILURES, Outcome, Progress, TransferError, retry_delay};

/// Résolutions refaites dans une exécution pour une adresse refusée.
pub const MAX_REFRESH: u32 = 3;

/// Délai de la fusion par ffmpeg (copie des flux, sans réencodage).
const MERGE_TIMEOUT: Duration = Duration::from_secs(3600);

/// Écart toléré entre la durée annoncée et celle du fichier produit.
const DURATION_SLACK_S: f64 = 2.0;

/// Ce dont une exécution a besoin.
pub struct Context<'a> {
    pub client: &'a Client,
    pub opts: &'a transfer::Options,
    pub tools: Option<&'a Arc<Tools>>,
}

/// Fin d'une exécution, avant son enregistrement.
enum End {
    Completed(PathBuf),
    Stopped,
    DiskFull(String),
    RetryLater { code: &'static str, detail: String },
    Failed { code: &'static str, detail: String },
}

fn failed(code: &'static str, detail: impl Into<String>) -> End {
    End::Failed {
        code,
        detail: detail.into(),
    }
}

/// Exécute la tâche vidéo `task_id` jusqu'à sa fin ou son arrêt, et enregistre son issue.
pub async fn run(
    conn: &mut Connection,
    ctx: &Context<'_>,
    task_id: &str,
    mut stop: watch::Receiver<bool>,
    cause: &crate::transfer::StopCause,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Outcome, TransferError> {
    let task = load_task(conn, task_id)?;
    match task.state {
        TaskState::Queued | TaskState::RetryWait => {
            change_state(conn, task_id, &Change::to(TaskState::Active))?;
        }
        TaskState::Active | TaskState::PostProcessing => {}
        other => {
            return Err(TransferError::NotRunnable {
                task_id: task_id.to_owned(),
                state: other.as_sql(),
            });
        }
    }
    // yt-dlp tenu jusqu'à la fin de l'exécution (audit V8), puis l'état des outils lu.
    let _held = match ctx.tools {
        Some(tools) => Some(tools.hold().await),
        None => None,
    };
    let status = ctx.tools.map(|t| t.status()).unwrap_or_default();
    // Pas d'empreinte cherchée à côté des adresses des flux et des sous-titres : des
    // requêtes inutiles vers les serveurs de vidéos (étape 4e).
    let opts = transfer::Options {
        neighbor_checksum: false,
        ..ctx.opts.clone()
    };
    let ctx = &Context {
        client: ctx.client,
        opts: &opts,
        tools: ctx.tools,
    };
    let end = if !status.ready() {
        failed(
            "TOOLS_MISSING",
            "yt-dlp, ffmpeg ou ffprobe n'est pas installé (media.installTools)",
        )
    } else if task.state == TaskState::PostProcessing {
        // Arrêt pendant la fusion : elle recommence.
        finish(conn, &status, task_id, None).await?
    } else {
        download(conn, ctx, &status, task_id, &mut stop, on_progress).await?
    };
    record(conn, task_id, end, cause)
}

/// Enregistre l'issue, comme `transfer::run` pour une tâche HTTP.
fn record(
    conn: &mut Connection,
    task_id: &str,
    end: End,
    cause: &crate::transfer::StopCause,
) -> Result<Outcome, TransferError> {
    match end {
        End::Completed(path) => Ok(Outcome::Completed(path)),
        End::Stopped => {
            let change = Change {
                pause_reason: Some(cause.get()),
                ..Change::to(TaskState::Paused)
            };
            change_state(conn, task_id, &change)?;
            Ok(Outcome::Paused)
        }
        End::DiskFull(detail) => {
            let change = Change {
                pause_reason: Some(PauseReason::DiskFull),
                error: Some(("DISK_FULL", &detail)),
                ..Change::to(TaskState::Paused)
            };
            change_state(conn, task_id, &change)?;
            Ok(Outcome::DiskFull)
        }
        End::RetryLater { code, detail } => {
            let task = load_task(conn, task_id)?;
            if task.retry_count + 1 >= MAX_FAILURES {
                return record(conn, task_id, End::Failed { code, detail }, cause);
            }
            let delay = retry_delay(task.retry_count, None, Duration::ZERO);
            let at_ms =
                now_ms().saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX));
            let change = Change {
                error: Some((code, &detail)),
                next_retry_at: Some(at_ms),
                ..Change::to(TaskState::RetryWait)
            };
            change_state(conn, task_id, &change)?;
            Ok(Outcome::RetryLater {
                code,
                detail,
                at_ms,
            })
        }
        End::Failed { code, detail } => {
            let change = Change {
                error: Some((code, &detail)),
                ..Change::to(TaskState::Failed)
            };
            change_state(conn, task_id, &change)?;
            Ok(Outcome::Failed { code, detail })
        }
    }
}

/// Arrêt demandé : valeur vraie, ou plus personne pour le demander.
fn stopping(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow() || stop.has_changed().is_err()
}

async fn wait_stop(stop: &mut watch::Receiver<bool>) {
    loop {
        if stopping(stop) {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

/// Résolution, interrompue par un arrêt (`Ok(None)`).
async fn resolve(
    status: &Status,
    client: &Client,
    page_url: &str,
    format: &str,
    stop: &mut watch::Receiver<bool>,
) -> Result<Option<Resolution>, ExtractError> {
    let ytdlp = &status.ytdlp.as_ref().expect("outils prêts").path;
    tokio::select! {
        r = extract::resolve(ytdlp, client, page_url, format, extract::PROBE_TIMEOUT) => r.map(Some),
        _ = wait_stop(stop) => Ok(None),
    }
}

/// Conteneur de la vidéo finale : celui demandé ; sinon `mp4` si tous les flux sont
/// `mp4` ou `m4a`, `webm` s'ils sont tous `webm`, `mkv` pour un mélange. Un flux seul
/// garde le sien.
fn container_for(requested: Option<&str>, streams: &[Resolved]) -> String {
    if let Some(c) = requested {
        return c.to_owned();
    }
    if let [one] = streams {
        return one.ext.clone();
    }
    if streams
        .iter()
        .all(|s| matches!(s.ext.as_str(), "mp4" | "m4a"))
    {
        "mp4".into()
    } else if streams.iter().all(|s| s.ext == "webm") {
        "webm".into()
    } else {
        "mkv".into()
    }
}

/// Identifiant de format réduit à des caractères sûrs dans un nom de fichier.
fn safe_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect()
}

/// Lignes des flux de la vidéo nommée `final_stem`, puis des sous-titres demandés que la
/// page propose (F-24) : à côté de la vidéo, sous `<nom>.<langue>.<ext>`. Les autres
/// manqueront, signalés à la fin.
fn stream_rows(resolution: &Resolution, subtitles: &[String], final_stem: &str) -> Vec<StreamRow> {
    let mut rows: Vec<StreamRow> = resolution
        .streams
        .iter()
        .enumerate()
        .map(|(i, s)| StreamRow {
            position: i as u32,
            format_id: s.format_id.clone(),
            protocol: s.protocol.clone(),
            ext: s.ext.clone(),
            has_video: s.has_video,
            has_audio: s.has_audio,
            file_name: format!("{final_stem}.f{}.{}", safe_id(&s.format_id), s.ext),
            size: s.size,
            child_id: None,
            done: false,
            language: None,
        })
        .collect();
    for lang in subtitles {
        if let Some(sub) = resolution.subtitles.iter().find(|s| &s.language == lang) {
            rows.push(StreamRow {
                position: rows.len() as u32,
                format_id: format!("sub-{}", sub.ext),
                protocol: match &sub.source {
                    SubtitleSource::Url { url, .. } if url.starts_with("http://") => "http",
                    SubtitleSource::Url { .. } => "https",
                    SubtitleSource::Data(_) => "data",
                }
                .into(),
                ext: sub.ext.clone(),
                has_video: false,
                has_audio: false,
                file_name: format!("{final_stem}.{lang}.{}", sub.ext),
                size: None,
                child_id: None,
                done: false,
                language: Some(lang.clone()),
            });
        }
    }
    rows
}

/// Résolution, puis chaque flux, puis la fusion.
async fn download(
    conn: &mut Connection,
    ctx: &Context<'_>,
    status: &Status,
    task_id: &str,
    stop: &mut watch::Receiver<bool>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<End, TransferError> {
    let mut task = load_task(conn, task_id)?;
    let job = media::load_job(conn, task_id)?;
    let mut streams = media::load_streams(conn, task_id)?;
    // Retéléchargement : la préparation est oubliée (nom), les flux aussi.
    if task.file_name.is_none() && !streams.is_empty() {
        media::forget_streams(conn, task_id)?;
        streams.clear();
    }
    let selector = if streams.is_empty() {
        job.format.clone()
    } else {
        // Les mêmes flux qu'à la première résolution.
        selector_of(&streams)
    };
    // Vidéo ajoutée par une connexion à jeton : yt-dlp résout le nom de la page lui-même,
    // vérifié ici à chaque exécution (SPEC §7.19).
    if ctx.client.is_public_only() {
        let checked = match crate::net::parse_url(&job.page_url) {
            Ok(url) => crate::net::reach::check_public(&url).await,
            Err(e) => Err(e),
        };
        if let Err(e) = checked {
            return Ok(failed(
                e.error_code().unwrap_or("ADDRESS_FORBIDDEN"),
                e.to_string(),
            ));
        }
    }
    let mut resolution = match resolve(status, ctx.client, &job.page_url, &selector, stop).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(End::Stopped),
        // Tous les flux sont faits (arrêt avant la fusion) : la page n'est plus utile, la
        // vidéo se finit, ses sous-titres non obtenus annoncés manquants (audit V15 : elle
        // échouait, complète, en `EXTRACTOR_FAILED`).
        Err(_) if !streams.is_empty() && streams.iter().all(|s| s.done || s.is_subtitle()) => {
            return finish(conn, status, task_id, None).await;
        }
        Err(e) => return Ok(failed("EXTRACTOR_FAILED", e.to_string())),
    };
    media::set_duration(conn, task_id, resolution.duration_s)?;
    if resolution.streams.iter().any(|s| s.drm) {
        // SPEC §8.3 : jamais de contournement d'une protection.
        return Ok(failed(
            "EXTRACTOR_FAILED",
            "format protégé par DRM : refusé",
        ));
    }
    if streams.is_empty() {
        let container = container_for(job.container.as_deref(), &resolution.streams);
        let stem = sanitize(&resolution.title, NameRules::native());
        let titled = format!("{stem}.{container}");
        // Règles de rangement (SPEC §7.21) : comparées à la page et au nom tiré du titre.
        let host = crate::net::parse_url(&job.page_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned));
        let subject = crate::rules::Subject {
            url: &job.page_url,
            host: host.as_deref(),
            name: &titled,
        };
        let placement = crate::store::rules::place_task(conn, &task, &subject, true)?;
        let dest_dir = match &placement.dir {
            Some(dir) => {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    return Ok(failed("DISK_IO", e.to_string()));
                }
                dir.to_string_lossy().into_owned()
            }
            None => task.dest_dir.clone(),
        };
        let wanted = placement.name.clone().unwrap_or(titled);
        let mut reserved = reserved_names(conn, &dest_dir, task_id)?;
        // Le nom final est pris sur le disque (`.part`), comme pour une tâche HTTP ; la
        // réservation, notée d'abord, part avec `save_resolution` (SPEC §5.6, règle 7).
        // Les noms qui en dérivent (flux, sous-titres) doivent être libres eux aussi : un
        // fichier déjà là sous l'un d'eux serait pris pour un flux fait, fusionné puis
        // effacé (audit V4). Sinon, le nom suivant.
        let dir = Path::new(&dest_dir);
        let mut tries = 0;
        let (final_name, rows) = loop {
            let final_name =
                match claims::claim(conn, dir, &wanted, &reserved, ClaimKind::Part, task_id) {
                    Ok(name) => name,
                    Err(claims::ClaimError::Store(e)) => return Err(e.into()),
                    Err(claims::ClaimError::Disk(e)) => {
                        return Ok(failed("DISK_IO", e.to_string()));
                    }
                };
            let final_stem = final_name
                .strip_suffix(&format!(".{container}"))
                .unwrap_or(&final_name)
                .to_owned();
            let rows = stream_rows(&resolution, &job.subtitles, &final_stem);
            let taken = rows.iter().any(|r| {
                let path = dir.join(&r.file_name);
                path.exists() || part_path(&path).exists()
            });
            tries += 1;
            if !taken {
                break (final_name, rows);
            }
            // Défait : le `.part` vide, puis la réservation.
            if let Err(e) = std::fs::remove_file(part_path(&dir.join(&final_name))) {
                return Ok(failed("DISK_IO", e.to_string()));
            }
            crate::store::claims::delete(conn, &dir.to_string_lossy(), &final_name)?;
            if tries >= 100 {
                return Ok(failed(
                    "DISK_IO",
                    "aucun nom libre pour la vidéo et ses flux",
                ));
            }
            reserved.push(final_name);
        };
        media::save_resolution(
            conn,
            task_id,
            &resolution.title,
            &final_name,
            &container,
            &rows,
            &crate::store::rules::Placed::of(&placement, &dest_dir),
        )?;
        streams = media::load_streams(conn, task_id)?;
        task = load_task(conn, task_id)?;
    } else if !same_streams(&streams, &resolution) {
        return Ok(failed(
            "EXTRACTOR_FAILED",
            "la page ne propose plus les mêmes flux",
        ));
    }

    let mut done_bytes: u64 = streams
        .iter()
        .filter(|s| s.done && !s.is_subtitle())
        .filter_map(|s| s.size)
        .sum();
    let mut refreshes = 0;
    // Les flux d'abord (positions 0 à n-1, comme `resolution.streams`), les sous-titres
    // ensuite.
    for index in 0..streams.len() {
        if streams[index].done || streams[index].is_subtitle() {
            continue;
        }
        loop {
            let stream = &streams[index];
            let resolved = &resolution.streams[index];
            let total: Option<u64> = streams
                .iter()
                .filter(|s| !s.is_subtitle())
                .map(|s| s.size)
                .sum();
            let step = if resolved.direct {
                direct(
                    conn,
                    ctx,
                    &task,
                    stream,
                    (resolved.url.as_str(), resolved.headers.as_slice()),
                    stop,
                    done_bytes,
                    total,
                    on_progress,
                )
                .await?
            } else {
                fragmented(
                    conn,
                    status,
                    ctx.client,
                    &task,
                    stream,
                    &job.page_url,
                    stop,
                    done_bytes,
                    total,
                    on_progress,
                )
                .await?
            };
            match step {
                Step::Done(size) => {
                    done_bytes += size;
                    media::save_progress(conn, task_id, done_bytes, None)?;
                    break;
                }
                Step::Refresh if refreshes < MAX_REFRESH => {
                    refreshes += 1;
                    resolution = match resolve(
                        status,
                        ctx.client,
                        &job.page_url,
                        &selector_of(&streams),
                        stop,
                    )
                    .await
                    {
                        Ok(Some(r)) if same_streams(&streams, &r) => r,
                        Ok(Some(_)) => {
                            return Ok(failed(
                                "EXTRACTOR_FAILED",
                                "la page ne propose plus les mêmes flux",
                            ));
                        }
                        Ok(None) => return Ok(End::Stopped),
                        Err(e) => return Ok(failed("EXTRACTOR_FAILED", e.to_string())),
                    };
                }
                Step::Refresh => {
                    return Ok(failed(
                        "HTTP_STATUS",
                        "adresse du flux toujours refusée après une nouvelle résolution",
                    ));
                }
                Step::End(end) => {
                    media::save_progress(conn, task_id, done_bytes, None)?;
                    return Ok(end);
                }
            }
        }
        streams = media::load_streams(conn, task_id)?;
    }

    if let Some(end) = subtitles(
        conn,
        ctx,
        status,
        &task,
        &job.page_url,
        &mut resolution,
        &mut refreshes,
        stop,
    )
    .await?
    {
        return Ok(end);
    }

    change_state(conn, task_id, &Change::to(TaskState::PostProcessing))?;
    // Annoncé par le planificateur.
    on_progress(Progress {
        received: done_bytes,
        total: Some(done_bytes),
        connections: 0,
        segments: Arc::from([]),
        verified: None,
        uploaded: None,
        upload_bps: None,
        seeding: false,
        merging: true,
    });
    finish(conn, status, task_id, resolution.duration_s).await
}

/// Sélecteur des flux retenus (sans les sous-titres).
fn selector_of(streams: &[StreamRow]) -> String {
    streams
        .iter()
        .filter(|s| !s.is_subtitle())
        .map(|s| s.format_id.as_str())
        .collect::<Vec<_>>()
        .join("+")
}

/// La résolution rend les flux retenus, dans le même ordre (sans les sous-titres).
fn same_streams(streams: &[StreamRow], resolution: &Resolution) -> bool {
    let media: Vec<&StreamRow> = streams.iter().filter(|s| !s.is_subtitle()).collect();
    media.len() == resolution.streams.len()
        && media
            .iter()
            .zip(&resolution.streams)
            .all(|(a, b)| a.format_id == b.format_id)
}

/// Un échec définitif d'un sous-titre : la vidéo se termine sans lui (décision du
/// 2026-11-09), quelle qu'en soit la cause côté site ou réseau (refus, adresse interdite à
/// une tâche à jeton, TLS, plages, redirections) ; avant, seuls quatre codes l'étaient, et
/// les autres mettaient en échec une vidéo aux flux complets, à chaque essai (audit V5).
/// Seuls le disque et le moteur lui-même arrêtent la vidéo : ils la toucheraient aussi.
fn subtitle_given_up(code: &str) -> bool {
    !matches!(
        code,
        "DISK_IO" | "DEST_PERMISSION" | "INTERNAL" | "SECRET_UNAVAILABLE"
    )
}

/// Les sous-titres de la vidéo (F-24), une fois ses flux complets. Un sous-titre que la
/// page ne propose plus, ou que le site refuse (après une nouvelle résolution, comme un
/// flux), est laissé : la vidéo se termine sans lui, et le planificateur le signale
/// (`SUBTITLES_MISSING`). Une coupure passagère, un arrêt ou un disque plein arrêtent la
/// vidéo comme pour un flux (`Some`).
#[allow(clippy::too_many_arguments)]
async fn subtitles(
    conn: &mut Connection,
    ctx: &Context<'_>,
    status: &Status,
    task: &TaskRow,
    page_url: &str,
    resolution: &mut Resolution,
    refreshes: &mut u32,
    stop: &mut watch::Receiver<bool>,
) -> Result<Option<End>, TransferError> {
    let all = media::load_streams(conn, &task.id)?;
    let selector = selector_of(&all);
    let rows: Vec<StreamRow> = all
        .into_iter()
        .filter(|s| s.is_subtitle() && !s.done)
        .collect();
    // Même langue et même format qu'à la première résolution : le nom du fichier porte
    // l'extension.
    let offered = |resolution: &Resolution, row: &StreamRow| {
        resolution
            .subtitles
            .iter()
            .find(|s| row.language.as_deref() == Some(s.language.as_str()) && s.ext == row.ext)
            .cloned()
    };
    for row in &rows {
        while let Some(sub) = offered(resolution, row) {
            let step = match &sub.source {
                SubtitleSource::Data(text) => write_subtitle(conn, task, row, &sub, text)?,
                SubtitleSource::Url { url, headers } => {
                    direct(
                        conn,
                        ctx,
                        task,
                        row,
                        (url.as_str(), headers.as_slice()),
                        stop,
                        0,
                        None,
                        &mut |_| {},
                    )
                    .await?
                }
            };
            match step {
                Step::Done(_) => break,
                Step::Refresh if *refreshes < MAX_REFRESH => {
                    *refreshes += 1;
                    match resolve(status, ctx.client, page_url, &selector, stop).await {
                        Ok(Some(r)) => *resolution = r,
                        Ok(None) => return Ok(Some(End::Stopped)),
                        // La page ne se lit plus : la vidéo est complète, elle se termine.
                        Err(_) => break,
                    }
                }
                Step::Refresh => break,
                Step::End(End::Failed { code, .. }) if subtitle_given_up(code) => break,
                Step::End(end) => return Ok(Some(end)),
            }
        }
    }
    Ok(None)
}

/// Un sous-titre donné en ligne par yt-dlp : écrit sous un nom libre, vidé sur disque,
/// puis renommé, comme un fichier téléchargé.
fn write_subtitle(
    conn: &Connection,
    task: &TaskRow,
    row: &StreamRow,
    sub: &Subtitle,
    text: &str,
) -> Result<Step, TransferError> {
    let dir = Path::new(&task.dest_dir);
    let reserved = reserved_names(conn, &task.dest_dir, &task.id)?;
    // Réservation notée d'abord, effacée par `stream_done` (SPEC §5.6, règle 7).
    let claimed = match claims::claim(
        conn,
        dir,
        &row.file_name,
        &reserved,
        ClaimKind::File,
        &task.id,
    ) {
        Ok(name) => Ok(name),
        Err(claims::ClaimError::Store(e)) => return Err(e.into()),
        Err(claims::ClaimError::Disk(e)) => Err(e),
    };
    let written = claimed.and_then(|name| {
        let path = dir.join(&name);
        let part = part_path(&path);
        std::fs::write(&part, text.as_bytes())?;
        let size = sync_file(&part)?;
        crate::io::mark_from_internet(&part);
        std::fs::rename(&part, &path)?;
        sync_parent_dir(&path).map_err(std::io::Error::other)?;
        Ok((name, size))
    });
    match written {
        Ok((name, size)) => {
            media::stream_done(conn, &task.id, row.position, &name, size)?;
            Ok(Step::Done(size))
        }
        Err(e) => Ok(Step::End(failed(
            "DISK_IO",
            format!("sous-titre {} : {e}", sub.language),
        ))),
    }
}

/// Issue d'un flux.
enum Step {
    /// Complet et vidé sur disque ; sa taille.
    Done(u64),
    /// Adresse refusée : refaire la résolution.
    Refresh,
    End(End),
}

/// En-têtes de yt-dlp recevables par une tâche HTTP (ceux que le moteur fixe lui-même
/// sont écartés), les secrets au coffre (SPEC §5.1).
fn store_headers(
    headers: &[(String, String)],
    vault: &crate::secrets::Vault,
) -> Result<Vec<StoredHeader>, String> {
    let mut kept: Vec<(String, String)> = Vec::new();
    for (name, value) in headers {
        if kept.len() >= crate::net::MAX_TASK_HEADERS
            || kept.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            continue;
        }
        let pair = vec![(name.clone(), value.clone())];
        if crate::net::check_task_headers(&pair).is_ok() {
            kept.extend(pair);
        }
    }
    let mut stored = Vec::with_capacity(kept.len());
    for (name, value) in kept {
        if crate::net::is_secret_header(&name) {
            match vault.put(&value) {
                Ok(id) => stored.push((name, Stored::Secret(id))),
                Err(e) => {
                    vault.forget(
                        stored
                            .into_iter()
                            .filter_map(|(_, s)| match s {
                                Stored::Secret(id) => Some(id),
                                Stored::Plain(_) => None,
                            })
                            .collect(),
                    );
                    return Err(e.to_string());
                }
            }
        } else {
            stored.push((name, Stored::Plain(value)));
        }
    }
    Ok(stored)
}

/// Un flux direct, ou un sous-titre, par sa tâche HTTP interne ; `(url, headers)` : son
/// adresse fraîche et les en-têtes de yt-dlp.
#[allow(clippy::too_many_arguments)]
async fn direct(
    conn: &mut Connection,
    ctx: &Context<'_>,
    task: &TaskRow,
    stream: &StreamRow,
    (url, headers): (&str, &[(String, String)]),
    stop: &mut watch::Receiver<bool>,
    base: u64,
    total: Option<u64>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Step, TransferError> {
    let child_id =
        match stream
            .child_id
            .clone()
            .or(media::child_named(conn, &task.id, &stream.file_name)?)
        {
            Some(id) => {
                // Adresse fraîche à chaque exécution ; ses validateurs restent, pour `If-Range`.
                tasks::replace_url(conn, &id, url, None, None)?;
                id
            }
            None => {
                let stored = match store_headers(headers, &ctx.opts.vault) {
                    Ok(stored) => stored,
                    Err(e) => {
                        return Ok(Step::End(End::RetryLater {
                            code: "SECRET_UNAVAILABLE",
                            detail: e,
                        }));
                    }
                };
                let (id, _) = tasks::create_task(
                    conn,
                    &NewTask {
                        queue_id: &task.queue_id,
                        url,
                        display_name: &stream.file_name,
                        dest_dir: &task.dest_dir,
                        max_connections: task.max_connections,
                        speed_limit_bps: None,
                        idempotency_key: None,
                        requested_name: Some(&stream.file_name),
                        checksum: None,
                        headers: &stored,
                        parent_id: Some(&task.id),
                        public_only: task.public_only,
                    },
                )?;
                id
            }
        };
    media::set_child(conn, &task.id, stream.position, &child_id)?;
    let mut restarted = false;
    loop {
        let child = load_task(conn, &child_id)?;
        match child.state {
            TaskState::Completed => {
                // Arrêt entre la fin du flux et son enregistrement.
                let name = child.file_name.clone().unwrap_or_default();
                let size = child.total_bytes.unwrap_or(child.done_bytes);
                media::stream_done(conn, &task.id, stream.position, &name, size)?;
                return Ok(Step::Done(size));
            }
            TaskState::Paused | TaskState::Failed => {
                change_state(conn, &child_id, &Change::to(TaskState::Queued))?;
            }
            _ => {}
        }
        let mut forward = |p: Progress| {
            let current = p.total.or(stream.size).unwrap_or(0);
            let others: Option<u64> = total.map(|t| t.saturating_sub(stream.size.unwrap_or(0)));
            on_progress(Progress {
                received: base + p.received,
                total: others.map(|o| o + current).or(p.total.map(|t| base + t)),
                connections: p.connections,
                segments: Arc::from([]),
                verified: None,
                uploaded: None,
                upload_bps: None,
                seeding: false,
                merging: false,
            });
        };
        let outcome = transfer::run(
            conn,
            ctx.client,
            &child_id,
            ctx.opts,
            stop.clone(),
            &mut forward,
        )
        .await?;
        return Ok(match outcome {
            Outcome::Completed(path) => {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                media::stream_done(conn, &task.id, stream.position, &name, size)?;
                Step::Done(size)
            }
            Outcome::Paused => Step::End(End::Stopped),
            Outcome::DiskFull => Step::End(End::DiskFull(
                "plus assez de place pour un flux de la vidéo".into(),
            )),
            Outcome::FileChanged if !restarted => {
                // Le flux a changé derrière sa nouvelle adresse : il repart de zéro.
                restarted = true;
                if let Some(name) = &load_task(conn, &child_id)?.file_name {
                    let _ = std::fs::remove_file(part_path(&Path::new(&task.dest_dir).join(name)));
                }
                tasks::restart_task(conn, &child_id)?;
                continue;
            }
            Outcome::FileChanged => Step::End(failed(
                "FILE_CHANGED",
                "un flux de la vidéo change à chaque essai",
            )),
            Outcome::RetryLater { code, detail, .. } => Step::End(End::RetryLater { code, detail }),
            Outcome::Failed {
                code: "HTTP_STATUS",
                ..
            } => Step::Refresh,
            Outcome::Failed { code, detail } => Step::End(End::Failed { code, detail }),
            Outcome::Seeding | Outcome::PostProcessing => {
                Step::End(failed("INTERNAL", "issue inattendue pour un flux de vidéo"))
            }
        });
    }
}

/// Un flux fragmenté, par yt-dlp : il écrit `<flux>.part` et le renomme à la fin ; le
/// moteur vide ensuite le fichier sur disque.
#[allow(clippy::too_many_arguments)]
async fn fragmented(
    conn: &mut Connection,
    status: &Status,
    client: &Client,
    task: &TaskRow,
    stream: &StreamRow,
    page_url: &str,
    stop: &mut watch::Receiver<bool>,
    base: u64,
    total: Option<u64>,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Step, TransferError> {
    let path = Path::new(&task.dest_dir).join(&stream.file_name);
    if !path.exists() {
        let ytdlp = &status.ytdlp.as_ref().expect("outils prêts").path;
        let ffmpeg = &status.ffmpeg.as_ref().expect("outils prêts").path;
        let mut command = extract::ytdlp_command(ytdlp, client);
        // `-o` est un modèle de yt-dlp : `%` s'y double.
        let template = path.to_string_lossy().replace('%', "%%");
        command.args([
            "-f",
            &stream.format_id,
            "--no-playlist",
            "--newline",
            "--no-mtime",
        ]);
        command.args(["--no-warnings", "--continue"]);
        // Limite de débit (SPEC §7.6) : yt-dlp ne passe pas par les seaux du moteur ; il
        // reçoit la plus petite de la tâche, de sa file et globale (étape 4d).
        if let Some(bps) = rate_limit(conn, task)? {
            command.args(["--limit-rate", &bps.to_string()]);
        }
        command.args([
            "--progress-template",
            "download:AURO %(progress.downloaded_bytes)s %(progress.total_bytes)s \
             %(progress.total_bytes_estimate)s",
        ]);
        command.arg("--ffmpeg-location").arg(ffmpeg);
        command.arg("-o").arg(&template);
        command.args(["--", page_url]);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let (mut child, tree) = match super::process::spawn(&mut command) {
            Ok(spawned) => spawned,
            Err(e) => return Ok(Step::End(failed("TOOLS_MISSING", e.to_string()))),
        };
        let mut stdout = tokio::io::BufReader::new(child.stdout.take().expect("sortie")).lines();
        let mut stderr = child.stderr.take().expect("sortie d'erreur");
        let errors = tokio::spawn(async move {
            let mut text = Vec::new();
            let _ = (&mut stderr).take(1 << 20).read_to_end(&mut text).await;
            text
        });
        loop {
            tokio::select! {
                line = stdout.next_line() => match line {
                    Ok(Some(line)) => {
                        if let Some(p) = parse_progress(&line) {
                            let current = p.1.or(stream.size);
                            let others: Option<u64> =
                                total.map(|t| t.saturating_sub(stream.size.unwrap_or(0)));
                            on_progress(Progress {
                                received: base + p.0,
                                total: match (others, current) {
                                    (Some(o), Some(c)) => Some(o + c),
                                    _ => None,
                                },
                                connections: 1,
                                segments: Arc::from([]),
                                verified: None,
                                uploaded: None,
                                upload_bps: None,
                                seeding: false,
                                merging: false,
                            });
                        }
                    }
                    _ => break,
                },
                _ = wait_stop(stop) => return Ok(Step::End(End::Stopped)),
            }
        }
        let exit = tokio::select! {
            exit = super::process::wait(&mut child, tree) => exit,
            _ = wait_stop(stop) => return Ok(Step::End(End::Stopped)),
        };
        let stderr = errors.await.unwrap_or_default();
        match exit {
            Ok(code) if code.success() => {}
            Ok(_) => {
                return Ok(Step::End(failed(
                    "EXTRACTOR_FAILED",
                    extract::error_message(&stderr),
                )));
            }
            Err(e) => return Ok(Step::End(failed("EXTRACTOR_FAILED", e.to_string()))),
        }
        if !path.exists() {
            return Ok(Step::End(failed(
                "EXTRACTOR_FAILED",
                "yt-dlp a fini sans écrire le flux",
            )));
        }
    }
    // Règle 1, à la fin du flux : le fichier est vidé sur disque avant d'être enregistré.
    let synced = tokio::task::spawn_blocking({
        let path = path.clone();
        move || -> std::io::Result<u64> {
            let len = sync_file(&path)?;
            sync_parent_dir(&path).map_err(std::io::Error::other)?;
            Ok(len)
        }
    })
    .await?;
    let size = match synced {
        Ok(size) => size,
        Err(e) => return Ok(Step::End(failed("DISK_IO", e.to_string()))),
    };
    // Le `.part` réservé à la création du nom, que yt-dlp a remplacé.
    let _ = std::fs::remove_file(part_path(&path));
    media::stream_done(conn, &task.id, stream.position, &stream.file_name, size)?;
    Ok(Step::Done(size))
}

/// Plus petite limite de débit de la tâche, de sa file et globale, s'il y en a une.
fn rate_limit(conn: &Connection, task: &TaskRow) -> Result<Option<u64>, TransferError> {
    let queue = tasks::list_queues(conn)?
        .into_iter()
        .find(|q| q.id == task.queue_id)
        .and_then(|q| q.speed_limit_bps);
    let global = crate::store::settings::download_limit(conn)?;
    Ok([task.speed_limit_bps, queue, global]
        .into_iter()
        .flatten()
        .min())
}

/// Ligne d'avancement de yt-dlp (`AURO <reçus> <total> <estimation>`, `NA` si inconnu) :
/// octets reçus et taille, exacte ou estimée.
fn parse_progress(line: &str) -> Option<(u64, Option<u64>)> {
    let mut parts = line.strip_prefix("AURO ")?.split_whitespace();
    let number = |s: Option<&str>| s.and_then(|s| s.parse::<f64>().ok()).map(|v| v as u64);
    let received = number(parts.next())?;
    let total = number(parts.next());
    let estimate = number(parts.next());
    Some((received, total.or(estimate)))
}

/// Fusion (F-23) et contrôle, puis fin de la tâche.
async fn finish(
    conn: &mut Connection,
    status: &Status,
    task_id: &str,
    duration_s: Option<f64>,
) -> Result<End, TransferError> {
    // Fusion relancée après un arrêt : la durée notée à la résolution, pour que ffprobe la
    // contrôle encore (audit V14).
    let duration_s = match duration_s {
        Some(d) => Some(d),
        None => media::duration(conn, task_id)?,
    };
    let task = load_task(conn, task_id)?;
    let job = media::load_job(conn, task_id)?;
    // Les sous-titres restent à côté de la vidéo : ni fusionnés, ni effacés.
    let streams: Vec<StreamRow> = media::load_streams(conn, task_id)?
        .into_iter()
        .filter(|s| !s.is_subtitle())
        .collect();
    let Some(final_name) = task.file_name.clone() else {
        return Ok(failed("INTERNAL", "vidéo sans nom final"));
    };
    if streams.is_empty() || streams.iter().any(|s| !s.done) {
        return Ok(failed("INTERNAL", "fusion d'une vidéo aux flux incomplets"));
    }
    let dir = PathBuf::from(&task.dest_dir);
    let final_path = dir.join(&final_name);
    let part = part_path(&final_path);
    let inputs: Vec<PathBuf> = streams.iter().map(|s| dir.join(&s.file_name)).collect();
    let container = job.container.unwrap_or_else(|| "mkv".into());
    let ffmpeg = &status.ffmpeg.as_ref().expect("outils prêts").path;
    let ffprobe = status.ffprobe.as_ref().expect("outils prêts");

    // Fin interrompue par un arrêt brutal, après le renommage du fichier final et avant
    // l'enregistrement de la fin (porte de la phase 5, banc des vidéos sous Windows) : la
    // fusion est faite, il ne reste qu'à effacer les flux restants et à l'enregistrer.
    if already_merged(&inputs, &final_path) {
        let synced = tokio::task::spawn_blocking({
            let (final_path, part) = (final_path.clone(), part.clone());
            move || -> std::io::Result<u64> {
                let len = sync_file(&final_path)?;
                let _ = std::fs::remove_file(&part);
                Ok(len)
            }
        })
        .await?;
        let size = match synced {
            Ok(size) => size,
            Err(e) => return Ok(failed("DISK_IO", e.to_string())),
        };
        for input in &inputs {
            let _ = std::fs::remove_file(input);
        }
        crate::postproc::finish(conn, task_id, &final_path, size)?;
        return Ok(End::Completed(final_path));
    }
    let single_as_is = matches!(&streams[..], [one] if one.ext == container);
    if !single_as_is {
        let _ = std::fs::remove_file(&part);
        if let Err(e) = merge(ffmpeg, &inputs, &container, &part).await {
            return Ok(failed("MERGE_FAILED", e));
        }
    }
    let produced = if single_as_is { &inputs[0] } else { &part };
    if let Err(e) = check(ffprobe, produced, duration_s).await {
        return Ok(failed("MERGE_FAILED", e));
    }
    let synced = tokio::task::spawn_blocking({
        let (produced, final_path, part) = (produced.clone(), final_path.clone(), part.clone());
        move || -> std::io::Result<u64> {
            let len = sync_file(&produced)?;
            crate::io::mark_from_internet(&produced);
            std::fs::rename(&produced, &final_path)?;
            // Le `.part` réservé au nom final, s'il reste (flux seul gardé tel quel).
            let _ = std::fs::remove_file(&part);
            sync_parent_dir(&final_path).map_err(std::io::Error::other)?;
            Ok(len)
        }
    })
    .await?;
    let size = match synced {
        Ok(size) => size,
        Err(e) => return Ok(failed("DISK_IO", e.to_string())),
    };
    if !single_as_is {
        for input in &inputs {
            let _ = std::fs::remove_file(input);
        }
    }
    crate::postproc::finish(conn, task_id, &final_path, size)?;
    Ok(End::Completed(final_path))
}

/// La fusion d'une vidéo est-elle déjà faite ? Les flux ne s'effacent qu'une fois le
/// fichier final à son nom (`finish`) : un flux manquant à côté du fichier final veut dire
/// qu'un arrêt est tombé entre ce renommage et l'enregistrement de la fin. Le nom final est
/// réservé à la tâche dès sa préparation : un fichier à ce nom est le sien.
fn already_merged(inputs: &[PathBuf], final_path: &Path) -> bool {
    final_path.is_file() && inputs.iter().any(|input| !input.exists())
}

/// Vide un fichier sur disque et rend sa taille. Ouvert en écriture, sans le tronquer :
/// Windows refuse de vider un fichier ouvert en lecture seule.
fn sync_file(path: &Path) -> std::io::Result<u64> {
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.sync_all()?;
    Ok(file.metadata()?.len())
}

/// Nom de format de ffmpeg pour un conteneur (la sortie est un `.part`).
fn muxer(container: &str) -> &str {
    match container {
        "mkv" => "matroska",
        "m4a" => "ipod",
        other => other,
    }
}

/// `ffmpeg -i <flux>… -map … -c copy -f <format> <sortie>`, sans réencodage.
async fn merge(
    ffmpeg: &Path,
    inputs: &[PathBuf],
    container: &str,
    output: &Path,
) -> Result<(), String> {
    let mut command = tokio::process::Command::new(ffmpeg);
    command.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"]);
    for input in inputs {
        command.arg("-i").arg(input);
    }
    for i in 0..inputs.len() {
        command.args(["-map", &i.to_string()]);
    }
    command
        .args(["-c", "copy", "-f", muxer(container)])
        .arg(output);
    run_tool(command, MERGE_TIMEOUT).await.map(|_| ())
}

/// `ffprobe` lit le fichier produit ; sa durée doit être celle annoncée, à
/// [`DURATION_SLACK_S`] près.
async fn check(ffprobe: &Path, file: &Path, expected_s: Option<f64>) -> Result<(), String> {
    let mut command = tokio::process::Command::new(ffprobe);
    command.args([
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
    ]);
    command.arg(file);
    let out = run_tool(command, Duration::from_secs(120)).await?;
    let duration: f64 = String::from_utf8_lossy(&out)
        .trim()
        .parse()
        .map_err(|_| "ffprobe ne lit pas la durée du fichier produit".to_owned())?;
    match expected_s {
        Some(expected) if (duration - expected).abs() > DURATION_SLACK_S => Err(format!(
            "durée du fichier produit : {duration:.1} s au lieu de {expected:.1} s"
        )),
        _ => Ok(()),
    }
}

/// Lance ffmpeg ou ffprobe (arbre tué au bout du délai) ; sa sortie, ou son message.
async fn run_tool(
    mut command: tokio::process::Command,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    super::process::prepare(&mut command);
    let tool = Path::new(command.as_std().get_program())
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "l'outil".to_owned());
    let (mut child, tree) = super::process::spawn(&mut command).map_err(|e| e.to_string())?;
    let (Some(mut out), Some(mut err)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(format!("{tool} : sorties absentes"));
    };
    let work = async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut out = (&mut out).take(16 << 20);
        let mut err = (&mut err).take(1 << 20);
        let (read_out, read_err) =
            tokio::join!(out.read_to_end(&mut stdout), err.read_to_end(&mut stderr));
        read_out.and(read_err)?;
        let status = super::process::wait(&mut child, tree).await?;
        Ok::<_, std::io::Error>(std::process::Output {
            status,
            stdout,
            stderr,
        })
    };
    let output = tokio::time::timeout(timeout, work)
        .await
        .map_err(|_| "délai dépassé".to_owned())?
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        // Sans message, l'outil et son code de sortie : le message de repli de
        // `error_message` nomme yt-dlp.
        if String::from_utf8_lossy(&output.stderr).trim().is_empty() {
            return Err(format!("{tool} a échoué sans message ({})", output.status));
        }
        return Err(extract::error_message(&output.stderr));
    }
    Ok(output.stdout)
}

/// Fichiers d'une vidéo à effacer (annulation, retrait) : ceux de ses flux et leurs
/// `.part`, le `.part` du nom final ; avec `finals`, le fichier final aussi. Les
/// sous-titres d'une vidéo terminée sont des fichiers finaux : effacés avec `finals`
/// seulement ; ceux d'une vidéo inachevée partent avec ses flux.
pub fn delete_files(conn: &Connection, task: &TaskRow, finals: bool) -> std::io::Result<()> {
    let dir = Path::new(&task.dest_dir);
    let remove = |path: PathBuf| match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    };
    let keep_subtitles = !finals && task.state == TaskState::Completed;
    let streams = media::load_streams(conn, &task.id).map_err(std::io::Error::other)?;
    let mut kept: Vec<&str> = Vec::new();
    for s in &streams {
        if keep_subtitles && s.is_subtitle() {
            kept.extend(s.child_id.as_deref());
            continue;
        }
        // Un flux direct a sa tâche interne, effacée plus bas sous son nom réel (qui peut
        // porter un numéro) : le nom prévu peut être celui d'un autre fichier (audit V4).
        if s.child_id.is_some() {
            continue;
        }
        let path = dir.join(&s.file_name);
        remove(part_path(&path))?;
        remove(path)?;
        // Flux fragmenté, écrit par yt-dlp : son état (`.ytdl`) et ses fragments
        // (`.part-FragN`) restaient à l'annulation (audit V13).
        remove(dir.join(format!("{}.ytdl", s.file_name)))?;
        let fragments = format!("{}.part-Frag", s.file_name);
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&fragments) {
                    remove(entry.path())?;
                }
            }
        }
    }
    for child in media::children(conn, &task.id).map_err(std::io::Error::other)? {
        if kept.contains(&child.as_str()) {
            continue;
        }
        if let Ok(row) = load_task(conn, &child)
            && let Some(name) = row.file_name
        {
            let path = dir.join(name);
            remove(part_path(&path))?;
            remove(path)?;
        }
    }
    if let Some(name) = &task.file_name {
        let path = dir.join(name);
        remove(part_path(&path))?;
        if finals {
            remove(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
