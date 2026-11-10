//! Commandes de la vidéo (`auro video`, SPEC §7.17) qui ne font pas de tâche : outils
//! (yt-dlp, ffmpeg) et description d'une page. L'ajout passe par `remote`.

use anyhow::Result;
use serde_json::{Value, json};

use crate::display::{duration, size};
use auro_cli::client::Rpc;

/// Un outil, sur une ligne : version et origine, ou absent.
fn tool_line(name: &str, tool: &Value) -> String {
    match tool["version"].as_str() {
        Some(version) => {
            let origin = match tool["origin"].as_str() {
                Some("system") => "du système",
                _ => "téléchargé",
            };
            format!("{name:<8} {version} ({origin})")
        }
        None => format!("{name:<8} absent"),
    }
}

fn print_tools(status: &Value) {
    println!("{}", tool_line("yt-dlp", &status["ytdlp"]));
    println!("{}", tool_line("ffmpeg", &status["ffmpeg"]));
    if let Some(error) = status["error"].as_str() {
        println!("Dernière installation en échec : {error}");
    }
    if status["ready"].as_bool() == Some(true) {
        println!("Outils prêts.");
    } else if status["installing"].as_bool() != Some(true) {
        println!("Outils absents : auro video tools --install");
    }
}

/// État des outils (`media.tools`) ; avec `install`, les installe d'abord
/// (`media.installTools`) et attend la fin (notification `media.tools`). La commande vaut
/// accord de l'utilisateur : rien ne s'installe sans elle (SPEC §7.17).
pub async fn tools(rpc: &mut Rpc, install: bool, force: bool) -> Result<()> {
    if !install {
        let status = rpc.call("media.tools", json!({})).await?;
        print_tools(&status);
        return Ok(());
    }
    // Abonné avant de lancer : la fin ne peut pas passer inaperçue.
    rpc.call("events.subscribe", json!({"topics": ["queues"]}))
        .await?;
    let mut status = rpc
        .call("media.installTools", json!({"force": force}))
        .await?;
    if status["installing"].as_bool() == Some(true) {
        eprintln!(
            "Installation de yt-dlp et ffmpeg depuis leurs publications officielles \
             (signature et empreintes vérifiées)…"
        );
        loop {
            let (method, params) = rpc.notification().await?;
            if method == "media.tools" && params["installing"].as_bool() != Some(true) {
                status = params;
                break;
            }
        }
    }
    print_tools(&status);
    match status["error"].as_str() {
        Some(error) => anyhow::bail!("installation en échec : {error}"),
        None => Ok(()),
    }
}

/// Une taille de format : exacte, ou estimée (`~`).
fn format_size(f: &Value) -> String {
    match f["filesize"].as_u64() {
        Some(bytes) if f["filesizeApprox"].as_bool() == Some(true) => {
            format!("~{}", size(bytes))
        }
        Some(bytes) => size(bytes),
        None => "?".into(),
    }
}

/// Ce que contient un format : image, son, ou les deux.
fn content(f: &Value) -> &'static str {
    match (f["vcodec"].is_string(), f["acodec"].is_string()) {
        (true, true) => "image+son",
        (true, false) => "image",
        (false, true) => "son",
        // Codecs inconnus : yt-dlp ne sait pas ; le moteur le prend pour complet.
        (false, false) => "?",
    }
}

/// Décrit une page de vidéo (`media.probe`) : titre, durée, sous-titres, formats.
pub async fn info(rpc: &mut Rpc, page: &str) -> Result<()> {
    let probe = rpc
        .call("media.probe", json!({"pageUrl": page}))
        .await
        .map_err(
            |e| match e.downcast_ref::<auro_cli::client::EngineRefused>() {
                Some(r) if r.code == "TOOLS_MISSING" => {
                    e.context("outils de la vidéo absents : auro video tools --install")
                }
                _ => e,
            },
        )?;
    println!("Titre       : {}", probe["title"].as_str().unwrap_or("?"));
    if let Some(s) = probe["durationS"].as_f64() {
        println!("Durée       : {}", duration(s.round() as u64));
    }
    if let Some(x) = probe["extractor"].as_str() {
        println!("Site        : {x}");
    }
    let subtitles: Vec<&str> = probe["subtitles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    println!(
        "Sous-titres : {}",
        if subtitles.is_empty() {
            "aucun".to_owned()
        } else {
            subtitles.join(", ")
        }
    );
    println!();
    println!(
        "{:<16} {:<5} {:<10} {:<10} {:>10}  {:<12} remarque",
        "format", "ext", "contenu", "image", "taille", "protocole"
    );
    for f in probe["formats"].as_array().into_iter().flatten() {
        let picture = match (f["width"].as_u64(), f["height"].as_u64()) {
            (Some(w), Some(h)) => format!("{w}x{h}"),
            (None, Some(h)) => format!("{h}p"),
            _ => String::new(),
        };
        let mut note = f["note"].as_str().unwrap_or_default().to_owned();
        if f["drm"].as_bool() == Some(true) {
            note = format!("protégé (DRM), refusé {note}");
        }
        println!(
            "{:<16} {:<5} {:<10} {:<10} {:>10}  {:<12} {}",
            f["formatId"].as_str().unwrap_or("?"),
            f["ext"].as_str().unwrap_or("?"),
            content(f),
            picture,
            format_size(f),
            f["protocol"].as_str().unwrap_or("?"),
            note.trim()
        );
    }
    println!();
    println!("Télécharger : auro video get <page> [-f <format>[+<format>]] [--subs fr,en]");
    Ok(())
}
