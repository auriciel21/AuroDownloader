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
    /// Langues des sous-titres proposés que le moteur sait télécharger (hors sous-titres
    /// automatiques ; formats retenus : `pick_subtitle`).
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
pub(crate) fn ytdlp_command(ytdlp: &Path, client: &crate::net::Client) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(ytdlp);
    // Proxy du moteur (F-07) : celui du système, rien à dire ; en direct, `--proxy ""`
    // (sans quoi yt-dlp prendrait celui du système) ; manuel, par l'environnement, où
    // l'identifiant ne se lit pas par les autres utilisateurs.
    let proxy = client.proxy();
    match crate::net::proxy::route(&proxy, client.is_public_only()) {
        crate::net::proxy::Route::System => {}
        crate::net::proxy::Route::Direct => {
            command.args(["--proxy", ""]);
        }
        crate::net::proxy::Route::Manual => {
            for (key, value) in
                crate::net::proxy::environment(&proxy, client.is_public_only()).unwrap_or_default()
            {
                command.env(key, value);
            }
        }
    }
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

/// Options d'une lecture de page par yt-dlp (`-J` : simulation, rien n'est écrit).
/// `--write-subs` : certains extracteurs (PeerTube) ne cherchent les sous-titres que s'ils
/// sont demandés ; sans lui, ils n'en rendent aucun (essai réel du 2026-11-09).
const READ_PAGE: [&str; 4] = ["-J", "--no-playlist", "--no-warnings", "--write-subs"];

/// Décrit la vidéo de `page_url` (`yt-dlp -J --no-playlist`).
pub async fn probe(
    ytdlp: &Path,
    client: &crate::net::Client,
    page_url: &str,
    timeout: Duration,
) -> Result<Probe, ExtractError> {
    let mut command = ytdlp_command(ytdlp, client);
    command.args(READ_PAGE).args(["--", page_url]);
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
    let mut subtitles: Vec<String> = parse_subtitles(json)
        .into_iter()
        .map(|s| s.language)
        .collect();
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
pub(crate) mod tests;

/// Les flux qu'un format retient (`yt-dlp -J -f <format>`), avec ce qu'il faut pour les
/// télécharger : adresse et en-têtes, valables quelques heures.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub title: String,
    pub duration_s: Option<f64>,
    pub streams: Vec<Resolved>,
    /// Sous-titres proposés que le moteur sait télécharger, un par langue.
    pub subtitles: Vec<Subtitle>,
}

/// Un sous-titre d'une langue (F-24), au format retenu par [`pick_subtitle`].
#[derive(Debug, Clone, PartialEq)]
pub struct Subtitle {
    pub language: String,
    /// `srt`, `vtt`… : l'extension de son fichier.
    pub ext: String,
    pub source: SubtitleSource,
}

/// D'où vient un sous-titre.
#[derive(Debug, Clone, PartialEq)]
pub enum SubtitleSource {
    /// Fichier à télécharger d'un seul tenant (`http`, `https`), avec ses en-têtes.
    Url {
        url: String,
        headers: Vec<(String, String)>,
    },
    /// Texte donné tel quel par yt-dlp.
    Data(String),
}

/// Formats de sous-titres retenus, par ordre de préférence : `.srt` d'abord (lu partout),
/// puis WebVTT, puis les autres formats que les lecteurs savent lire. Les formats propres à
/// un site (`json3`, `srv3`…) sont écartés. Pas de conversion (décision du 2026-11-09).
const SUBTITLE_FORMATS: [&str; 6] = ["srt", "vtt", "ass", "ssa", "ttml", "dfxp"];

/// Taille maximale d'un sous-titre donné en ligne par yt-dlp.
const MAX_SUBTITLE_DATA: usize = 8 << 20;

/// Le format retenu parmi ceux d'une langue : le premier de [`SUBTITLE_FORMATS`] qui se
/// télécharge d'un seul tenant (ni HLS ni DASH : le moteur ne passe pas par yt-dlp pour un
/// sous-titre), ou qui est donné en ligne.
fn pick_subtitle(
    language: &str,
    formats: &[Value],
    page_headers: &[(String, String)],
) -> Option<Subtitle> {
    SUBTITLE_FORMATS.iter().find_map(|wanted| {
        formats.iter().find_map(|f| {
            let ext = text(&f["ext"])?;
            if ext != *wanted {
                return None;
            }
            let source = if let Some(data) = f["data"].as_str() {
                if data.len() > MAX_SUBTITLE_DATA {
                    return None;
                }
                SubtitleSource::Data(data.to_owned())
            } else {
                let url = text(&f["url"])?;
                let direct = text(&f["protocol"])
                    .as_deref()
                    .is_none_or(|p| matches!(p, "http" | "https"))
                    && (url.starts_with("https://") || url.starts_with("http://"))
                    && f.get("fragments").is_none_or(Value::is_null);
                if !direct {
                    return None;
                }
                let headers = if f["http_headers"].is_object() {
                    headers_of(f)
                } else {
                    page_headers.to_vec()
                };
                SubtitleSource::Url { url, headers }
            };
            Some(Subtitle {
                language: language.to_owned(),
                ext,
                source,
            })
        })
    })
}

/// En-têtes `http_headers` d'un format ou de la page.
fn headers_of(v: &Value) -> Vec<(String, String)> {
    v["http_headers"]
        .as_object()
        .map(|h| {
            h.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// Les sous-titres de la page (hors automatiques) que le moteur sait télécharger, par
/// langue. Un code de langue étrange (hors lettres, chiffres, `-` et `_`) est écarté : il
/// entre dans un nom de fichier.
fn parse_subtitles(json: &Value) -> Vec<Subtitle> {
    let page_headers = headers_of(json);
    let Some(langs) = json["subtitles"].as_object() else {
        return Vec::new();
    };
    langs
        .iter()
        .filter(|(lang, _)| {
            !lang.is_empty()
                && lang.len() <= 20
                && lang
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .filter_map(|(lang, formats)| pick_subtitle(lang, formats.as_array()?, &page_headers))
        .collect()
}

/// Un flux retenu.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub format_id: String,
    pub ext: String,
    pub protocol: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub size: Option<u64>,
    pub has_video: bool,
    pub has_audio: bool,
    pub drm: bool,
    /// Téléchargé par le moteur (`http`, `https`, d'un seul tenant) ; sinon par yt-dlp
    /// (HLS, DASH à fragments).
    pub direct: bool,
}

/// Résout `format` pour `page_url`.
pub async fn resolve(
    ytdlp: &Path,
    client: &crate::net::Client,
    page_url: &str,
    format: &str,
    timeout: Duration,
) -> Result<Resolution, ExtractError> {
    let mut command = ytdlp_command(ytdlp, client);
    command.args(READ_PAGE).args(["-f", format, "--", page_url]);
    let output = run(command, timeout).await?;
    let json: Value = serde_json::from_slice(&output)
        .map_err(|_| ExtractError::Failed("réponse de yt-dlp illisible".into()))?;
    parse_resolution(&json)
}

pub(crate) fn parse_resolution(json: &Value) -> Result<Resolution, ExtractError> {
    if json["_type"].as_str() == Some("playlist") {
        return Err(ExtractError::Failed(
            "liste de lecture : seule une page de vidéo est prise en charge".into(),
        ));
    }
    let chosen: Vec<&Value> = match json["requested_formats"].as_array() {
        Some(list) if !list.is_empty() => list.iter().collect(),
        // Un seul format : décrit à la racine.
        _ => vec![json],
    };
    let streams = chosen
        .into_iter()
        .map(parse_resolved)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| ExtractError::Failed("flux sans adresse ni identifiant".into()))?;
    if streams.is_empty() {
        return Err(ExtractError::Failed("aucun flux retenu".into()));
    }
    Ok(Resolution {
        title: text(&json["title"]).unwrap_or_else(|| "vidéo".into()),
        duration_s: json["duration"].as_f64(),
        streams,
        subtitles: parse_subtitles(json),
    })
}

fn parse_resolved(f: &Value) -> Option<Resolved> {
    let format = parse_format(f)?;
    let url = text(&f["url"])?;
    let headers = headers_of(f);
    let direct = matches!(format.protocol.as_str(), "http" | "https")
        && f.get("fragments").is_none_or(Value::is_null);
    // Sans codec annoncé, un format est pris pour complet (vidéo et son).
    let unknown = format.vcodec.is_none() && format.acodec.is_none();
    Some(Resolved {
        has_video: format.vcodec.is_some() || unknown,
        has_audio: format.acodec.is_some() || unknown,
        format_id: format.format_id,
        ext: format.ext,
        protocol: format.protocol,
        url,
        headers,
        size: format.filesize.filter(|_| !format.filesize_approx),
        drm: format.drm,
        direct,
    })
}
