//! Extraction (SPEC §7.17) : yt-dlp lit une page et décrit sa vidéo (`yt-dlp -J`). Le
//! moteur ne garde de sa réponse que ce que le protocole montre (`media.probe`).
//!
//! yt-dlp est lancé sans interpréteur de commandes, avec l'adresse après `--` (elle ne peut
//! pas passer pour une option), sans la configuration de l'utilisateur (`--ignore-config`) ni
//! cache, sans fenêtre de console sous Windows, et tué s'il dépasse son délai ou si
//! l'appelant abandonne.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;

/// Délai d'une extraction.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(90);

/// Taille maximale de la réponse de yt-dlp : un garde-fou.
const MAX_OUTPUT: usize = 64 << 20;

/// Une vidéo décrite par yt-dlp.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub title: String,
    pub duration_s: Option<f64>,
    pub extractor: Option<String>,
    pub thumbnail: Option<String>,
    pub formats: Vec<Format>,
    /// Langues des sous-titres proposés (hors sous-titres automatiques).
    pub subtitles: Vec<String>,
}

/// Un format proposé.
#[derive(Debug, Clone, PartialEq)]
pub struct Format {
    pub format_id: String,
    pub ext: String,
    /// Codec vidéo ; `None` : pas de vidéo (audio seul).
    pub vcodec: Option<String>,
    /// Codec audio ; `None` : pas d'audio (vidéo seule).
    pub acodec: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub fps: Option<f64>,
    /// Débit total, en kbit/s.
    pub tbr: Option<f64>,
    /// Taille en octets, exacte ou estimée ([`Format::filesize_approx`]).
    pub filesize: Option<u64>,
    pub filesize_approx: bool,
    /// `https`, `m3u8_native`, `http_dash_segments`…
    pub protocol: String,
    /// Protégé par DRM (ou peut-être) : jamais téléchargé (SPEC §8.3).
    pub drm: bool,
    pub note: Option<String>,
}

/// Échec d'une extraction.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExtractError {
    /// yt-dlp a échoué : son dernier message, sans adresse.
    #[error("{0}")]
    Failed(String),
    #[error("yt-dlp n'a pas répondu dans le délai")]
    Timeout,
    #[error("yt-dlp n'a pas pu être lancé : {0}")]
    Launch(String),
}

/// Commande yt-dlp avec les options communes à tous ses usages.
pub(crate) fn ytdlp_command(ytdlp: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(ytdlp);
    command
        .args(["--ignore-config", "--no-cache-dir", "--no-color"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    super::process::prepare(&mut command);
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW : pas de fenêtre de console.
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Décrit la vidéo de `page_url` (`yt-dlp -J --no-playlist`).
pub async fn probe(ytdlp: &Path, page_url: &str, timeout: Duration) -> Result<Probe, ExtractError> {
    let mut command = ytdlp_command(ytdlp);
    command.args(["-J", "--no-playlist", "--no-warnings", "--", page_url]);
    let output = run(command, timeout).await?;
    let json: Value = serde_json::from_slice(&output)
        .map_err(|_| ExtractError::Failed("réponse de yt-dlp illisible".into()))?;
    parse_probe(&json)
}

/// Lance la commande et rend sa sortie ; un code de retour non nul rend le dernier message
/// d'erreur de yt-dlp.
async fn run(
    mut command: tokio::process::Command,
    timeout: Duration,
) -> Result<Vec<u8>, ExtractError> {
    // L'arbre de yt-dlp meurt avec `_tree`, à la fin, sur un délai dépassé ou si
    // l'appelant abandonne.
    let (child, _tree) =
        super::process::spawn(&mut command).map_err(|e| ExtractError::Launch(e.to_string()))?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| ExtractError::Timeout)?
        .map_err(|e| ExtractError::Launch(e.to_string()))?;
    if !output.status.success() {
        return Err(ExtractError::Failed(error_message(&output.stderr)));
    }
    if output.stdout.len() > MAX_OUTPUT {
        return Err(ExtractError::Failed("réponse de yt-dlp trop longue".into()));
    }
    Ok(output.stdout)
}

/// Le dernier message `ERROR:` de yt-dlp (sinon sa dernière ligne), sans adresse (SPEC
/// §8.2), raccourci.
pub(crate) fn error_message(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let line = lines
        .iter()
        .rev()
        .find(|l| l.starts_with("ERROR:"))
        .or(lines.last())
        .copied()
        .unwrap_or("yt-dlp a échoué sans message");
    let cleaned: Vec<String> = line
        .trim_start_matches("ERROR:")
        .split_whitespace()
        .map(|word| {
            let bare = word.trim_start_matches(['(', '[', '<', '"', '\'']);
            if bare.starts_with("http://") || bare.starts_with("https://") {
                "<adresse>".to_owned()
            } else {
                word.to_owned()
            }
        })
        .collect();
    let mut message = cleaned.join(" ");
    if message.chars().count() > 300 {
        message = message.chars().take(300).collect::<String>() + "…";
    }
    message
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Un codec ; `none` (absent) devient `None`.
fn codec(v: &Value) -> Option<String> {
    text(v).filter(|c| c != "none")
}

fn parse_probe(json: &Value) -> Result<Probe, ExtractError> {
    if json["_type"].as_str() == Some("playlist") {
        return Err(ExtractError::Failed(
            "liste de lecture : seule une page de vidéo est prise en charge".into(),
        ));
    }
    let formats: Vec<Format> = match json["formats"].as_array() {
        Some(list) => list.iter().filter_map(parse_format).collect(),
        // Une vidéo à format unique : yt-dlp le décrit à la racine.
        None => parse_format(json).into_iter().collect(),
    };
    if formats.is_empty() {
        return Err(ExtractError::Failed("aucun format trouvé".into()));
    }
    let mut subtitles: Vec<String> = json["subtitles"]
        .as_object()
        .map(|langs| langs.keys().cloned().collect())
        .unwrap_or_default();
    subtitles.sort();
    Ok(Probe {
        title: text(&json["title"]).unwrap_or_else(|| "vidéo".into()),
        duration_s: json["duration"].as_f64(),
        extractor: text(&json["extractor_key"]).or_else(|| text(&json["extractor"])),
        thumbnail: text(&json["thumbnail"]),
        formats,
        subtitles,
    })
}

fn parse_format(f: &Value) -> Option<Format> {
    let format_id = text(&f["format_id"])?;
    let (filesize, filesize_approx) = match f["filesize"].as_u64() {
        Some(size) => (Some(size), false),
        None => (f["filesize_approx"].as_f64().map(|s| s as u64), true),
    };
    Some(Format {
        format_id,
        ext: text(&f["ext"]).unwrap_or_default(),
        vcodec: codec(&f["vcodec"]),
        acodec: codec(&f["acodec"]),
        width: f["width"].as_u64(),
        height: f["height"].as_u64(),
        fps: f["fps"].as_f64(),
        tbr: f["tbr"].as_f64(),
        filesize,
        filesize_approx: filesize.is_some() && filesize_approx,
        protocol: text(&f["protocol"]).unwrap_or_else(|| "https".into()),
        // `has_drm` : vrai, faux, ou « maybe ».
        drm: matches!(&f["has_drm"], Value::Bool(true) | Value::String(_)),
        note: text(&f["format_note"]),
    })
}

#[cfg(test)]
mod tests;
