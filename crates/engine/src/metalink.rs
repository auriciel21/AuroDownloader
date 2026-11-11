//! Fichiers Metalink (F-27, SPEC §7.7, phase 6, étape 1d) : la description d'un ou de
//! plusieurs fichiers, chacun avec ses adresses et son empreinte. Formats lus : Metalink 4
//! (`.meta4`, RFC 5854) et l'ancien Metalink 3 (`.metalink`). Chaque fichier décrit devient
//! une tâche HTTP : la première adresse est la référence, les suivantes des miroirs.
//!
//! Décisions de l'utilisateur (2026-11-11) : une tâche par fichier ; adresses triées par la
//! priorité du fichier, le pays ignoré ; seules les adresses HTTP et HTTPS servent (torrents
//! des `metaurl` ignorés) ; l'empreinte la plus forte du fichier entier devient celle de la
//! tâche, les empreintes par morceau et les signatures sont ignorées en v1.
//!
//! Les noms viennent d'un tiers : chaque composante est nettoyée (SPEC §8.1), un chemin
//! absolu ou qui remonte (`..`) écarte le fichier. Le XML est lu sans DTD ni entité autre
//! que les cinq prédéfinies et les références de caractère.

use std::borrow::Cow;
use std::collections::HashSet;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::io::checksum::{Algo, Checksum};
use crate::net::filename::{NameRules, sanitize};
use crate::store::sources::MAX_SOURCES;

/// Taille maximale d'un fichier Metalink lu (local ou distant).
pub const MAX_LEN: usize = 4 << 20;
/// Fichiers décrits au plus ; les suivants sont écartés.
pub const MAX_FILES: usize = 256;
/// Profondeur maximale d'un chemin (dossiers et nom).
const MAX_DEPTH: usize = 16;
/// Adresses lues au plus par fichier, avant le tri : un garde-fou.
const MAX_URLS_READ: usize = 1024;

const NS_V4: &str = "urn:ietf:params:xml:ns:metalink";
const NS_V3: &str = "http://www.metalinker.org/";

/// Format du document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// RFC 5854, `.meta4`.
    V4,
    /// Metalink 3.0, `.metalink`.
    V3,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::V4 => "4",
            Format::V3 => "3",
        }
    }
}

/// Un document lu : les fichiers retenus et ceux écartés, avec la raison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metalink {
    pub format: Format,
    pub files: Vec<MetaFile>,
    pub ignored: Vec<Ignored>,
}

/// Un fichier décrit, prêt à devenir une tâche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaFile {
    /// Nom du fichier, nettoyé.
    pub name: String,
    /// Dossiers sous le dossier de destination, nettoyés ; vide le plus souvent.
    pub dir: Vec<String>,
    /// Taille annoncée, si le document la donne.
    pub size: Option<u64>,
    /// Empreinte la plus forte du fichier entier : SHA-256, sinon SHA-1, sinon MD5.
    pub checksum: Option<Checksum>,
    /// Adresses HTTP(S), de la meilleure priorité à la moins bonne, 16 au plus : la
    /// première est la référence.
    pub urls: Vec<String>,
    /// Adresses laissées de côté : au-delà de 16, ou d'un autre protocole.
    pub dropped_urls: usize,
}

impl MetaFile {
    /// Chemin affiché : dossiers et nom, séparés par `/`.
    pub fn path(&self) -> String {
        let mut parts = self.dir.clone();
        parts.push(self.name.clone());
        parts.join("/")
    }
}

/// Un fichier écarté du document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ignored {
    /// Nom tel que le document le donne (à n'afficher que comme texte).
    pub name: String,
    pub reason: String,
}

/// Document refusé en entier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MetalinkError {
    #[error("fichier Metalink trop gros (plus de {} Mio)", MAX_LEN >> 20)]
    TooLarge,
    #[error("fichier Metalink illisible : {0}")]
    Xml(String),
    #[error("ce n'est pas un fichier Metalink (version 4 ou 3)")]
    NotMetalink,
    #[error("aucun fichier utilisable dans ce Metalink{0}")]
    NoUsableFile(String),
}

/// Ce que le document dit d'un fichier, avant les contrôles.
#[derive(Default)]
struct RawFile {
    name: Option<String>,
    size: Option<String>,
    /// Empreintes du fichier entier : type et valeur.
    hashes: Vec<(String, String)>,
    /// Adresses et leur rang de tri (plus petit d'abord).
    urls: Vec<(String, u64)>,
}

/// Lit un document Metalink 4 ou 3.
pub fn parse(bytes: &[u8]) -> Result<Metalink, MetalinkError> {
    if bytes.len() > MAX_LEN {
        return Err(MetalinkError::TooLarge);
    }
    let (format, raw) = read_xml(bytes)?;
    let mut files = Vec::new();
    let mut ignored = Vec::new();
    let mut taken: Vec<Vec<String>> = Vec::new();
    for (i, raw) in raw.into_iter().enumerate() {
        let shown = raw.name.clone().unwrap_or_default();
        if i >= MAX_FILES {
            ignored.push(Ignored {
                name: shown,
                reason: format!("plus de {MAX_FILES} fichiers dans le Metalink"),
            });
            continue;
        }
        match check_file(format, raw, &taken) {
            Ok(file) => {
                taken.push(fold_path(&file));
                files.push(file);
            }
            Err(reason) => ignored.push(Ignored {
                name: shown,
                reason,
            }),
        }
    }
    if files.is_empty() {
        let why = ignored
            .first()
            .map(|i| format!(" ({})", i.reason))
            .unwrap_or_default();
        return Err(MetalinkError::NoUsableFile(why));
    }
    Ok(Metalink {
        format,
        files,
        ignored,
    })
}

/// Chemin comparé aux autres : la casse ne distingue pas deux noms sous Windows et Android
/// (comme pour les torrents, SPEC §7.16).
fn fold_path(file: &MetaFile) -> Vec<String> {
    file.dir
        .iter()
        .chain(std::iter::once(&file.name))
        .map(|c| {
            if cfg!(any(windows, target_os = "android")) {
                c.to_lowercase()
            } else {
                c.clone()
            }
        })
        .collect()
}

fn check_file(format: Format, raw: RawFile, taken: &[Vec<String>]) -> Result<MetaFile, String> {
    let name = raw.name.ok_or("fichier sans nom")?;
    let (dir, name) = clean_path(&name)?;
    let size = match raw.size {
        None => None,
        Some(text) => Some(
            text.trim()
                .parse::<u64>()
                .map_err(|_| "taille illisible".to_owned())?,
        ),
    };
    let checksum = best_checksum(format, &raw.hashes)?;
    // Tri stable : à priorité égale, l'ordre du document.
    let mut urls = raw.urls;
    urls.sort_by_key(|(_, rank)| *rank);
    let total = urls.len();
    let mut seen = HashSet::new();
    let mut kept: Vec<String> = urls
        .into_iter()
        .map(|(url, _)| url)
        .filter(|url| crate::net::check_url(url).is_ok() && seen.insert(url.clone()))
        .collect();
    if kept.is_empty() {
        return Err("aucune adresse HTTP ou HTTPS".into());
    }
    kept.truncate(MAX_SOURCES);
    let file = MetaFile {
        name,
        dir,
        size,
        checksum,
        dropped_urls: total - kept.len(),
        urls: kept,
    };
    let path = fold_path(&file);
    if taken
        .iter()
        .any(|t| t.starts_with(&path) || path.starts_with(t))
    {
        return Err(
            "même chemin qu'un autre fichier du Metalink, une fois les noms nettoyés".into(),
        );
    }
    Ok(file)
}

/// Dossiers et nom d'un chemin relatif (`/` sépare les dossiers, RFC 5854 §4.2.6) : un
/// chemin absolu, vide ou qui remonte est refusé ; chaque composante est nettoyée.
fn clean_path(raw: &str) -> Result<(Vec<String>, String), String> {
    let refused = || "nom refusé : chemin absolu, vide ou qui remonte (..)".to_owned();
    // `C:…` : un chemin Windows absolu ; un `:` ailleurs est seulement nettoyé.
    if raw.starts_with(['/', '\\']) || is_drive(raw) {
        return Err(refused());
    }
    let parts: Vec<&str> = raw.split(['/', '\\']).collect();
    if parts.len() > MAX_DEPTH {
        return Err(format!("chemin trop profond (plus de {MAX_DEPTH} niveaux)"));
    }
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return Err(refused());
    }
    let mut clean: Vec<String> = parts
        .iter()
        .map(|p| sanitize(p, NameRules::native()))
        .collect();
    let name = clean.pop().ok_or_else(refused)?;
    Ok((clean, name))
}

/// `C:` en tête : un lecteur Windows.
fn is_drive(raw: &str) -> bool {
    let b = raw.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// L'empreinte la plus forte prise en charge ; une valeur mal formée écarte le fichier.
fn best_checksum(format: Format, hashes: &[(String, String)]) -> Result<Option<Checksum>, String> {
    let mut best: Option<Checksum> = None;
    for (kind, value) in hashes {
        let algo = match kind.trim().to_ascii_lowercase().replace('-', "").as_str() {
            "sha256" => Algo::Sha256,
            "sha1" => Algo::Sha1,
            "md5" => Algo::Md5,
            // SHA-512 et les autres : pas encore vérifiés par le moteur.
            _ => continue,
        };
        let sum = Checksum::new(algo, value.trim()).map_err(|_| {
            format!(
                "empreinte {} illisible (Metalink {})",
                algo.as_sql(),
                format.as_str()
            )
        })?;
        if best
            .as_ref()
            .is_none_or(|b| strength(algo) > strength(b.algo()))
        {
            best = Some(sum);
        }
    }
    Ok(best)
}

fn strength(algo: Algo) -> u8 {
    match algo {
        Algo::Sha256 => 3,
        Algo::Sha1 => 2,
        Algo::Md5 => 1,
    }
}

/// Lit le XML : le format (d'après l'élément racine) et les fichiers bruts.
fn read_xml(bytes: &[u8]) -> Result<(Format, Vec<RawFile>), MetalinkError> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let xml = |e: quick_xml::Error| MetalinkError::Xml(e.to_string());
    let mut buf = Vec::new();
    // Éléments ouverts, par nom local.
    let mut stack: Vec<String> = Vec::new();
    let mut format = None;
    let mut files: Vec<RawFile> = Vec::new();
    let mut current: Option<RawFile> = None;
    // Texte de l'élément ouvert, et ce qu'il deviendra à sa fermeture.
    let mut text = String::new();
    let mut pending: Option<Pending> = None;
    loop {
        let event = reader.read_event_into(&mut buf).map_err(xml)?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                let local = local_name(e);
                if stack.is_empty() {
                    if format.is_some() {
                        return Err(MetalinkError::Xml("plusieurs éléments racine".into()));
                    }
                    format = Some(root_format(e, &local)?);
                } else {
                    let format = format.expect("racine lue");
                    open(format, e, &local, &stack, &mut current, &mut pending)?;
                }
                text.clear();
                if empty {
                    close(&local, &mut current, &mut files, &mut pending, &text);
                } else {
                    stack.push(local);
                }
            }
            Event::End(_) => {
                let Some(local) = stack.pop() else {
                    return Err(MetalinkError::Xml("élément fermé sans être ouvert".into()));
                };
                close(&local, &mut current, &mut files, &mut pending, &text);
                text.clear();
            }
            Event::Text(t) => {
                if pending.is_some() {
                    let part = t
                        .xml_content(XmlVersion::Implicit1_0)
                        .map_err(|e| MetalinkError::Xml(e.to_string()))?;
                    text.push_str(&part);
                }
            }
            Event::CData(t) => {
                if pending.is_some() {
                    let part = t.decode().map_err(|e| MetalinkError::Xml(e.to_string()))?;
                    text.push_str(&part);
                }
            }
            Event::GeneralRef(r) => {
                if pending.is_some() {
                    text.push_str(&resolve_ref(&r)?);
                }
            }
            // Une DTD pourrait déclarer des entités : refusée.
            Event::DocType(_) => {
                return Err(MetalinkError::Xml("déclaration DOCTYPE refusée".into()));
            }
            Event::Eof => break,
            _ => {}
        }
        if files.len() > MAX_FILES + 1 {
            // Le reste ne serait qu'écarté : inutile de le lire.
            break;
        }
        buf.clear();
    }
    let Some(format) = format else {
        return Err(MetalinkError::NotMetalink);
    };
    Ok((format, files))
}

/// Ce que devient le texte de l'élément ouvert.
enum Pending {
    Size,
    Hash(String),
    Url(u64),
}

fn local_name(e: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn attr(e: &BytesStart<'_>, name: &str) -> Result<Option<String>, MetalinkError> {
    let found = e
        .try_get_attribute(name)
        .map_err(|err| MetalinkError::Xml(err.to_string()))?;
    match found {
        None => Ok(None),
        Some(a) => Ok(Some(
            a.normalized_value(XmlVersion::Implicit1_0)
                .map_err(|err| MetalinkError::Xml(err.to_string()))?
                .into_owned(),
        )),
    }
}

/// Format d'après la racine : `metalink` dans l'espace de noms de la version 4, ou de la
/// version 3 (ou `version="3…"` sans espace de noms).
fn root_format(e: &BytesStart<'_>, local: &str) -> Result<Format, MetalinkError> {
    if local != "metalink" {
        return Err(MetalinkError::NotMetalink);
    }
    let mut namespaces = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|err| MetalinkError::Xml(err.to_string()))?;
        let key = a.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            namespaces.push(String::from_utf8_lossy(&a.value).into_owned());
        }
    }
    if namespaces.iter().any(|n| n == NS_V4) {
        return Ok(Format::V4);
    }
    let version = attr(e, "version")?;
    if namespaces.iter().any(|n| n == NS_V3) || version.is_some_and(|v| v.starts_with('3')) {
        return Ok(Format::V3);
    }
    Err(MetalinkError::NotMetalink)
}

/// Ouverture d'un élément sous la racine.
fn open(
    format: Format,
    e: &BytesStart<'_>,
    local: &str,
    stack: &[String],
    current: &mut Option<RawFile>,
    pending: &mut Option<Pending>,
) -> Result<(), MetalinkError> {
    *pending = None;
    let parent = stack.last().map(String::as_str);
    if local == "file" {
        if current.is_some() {
            return Err(MetalinkError::Xml("élément file imbriqué".into()));
        }
        *current = Some(RawFile {
            name: attr(e, "name")?,
            ..RawFile::default()
        });
        return Ok(());
    }
    let Some(file) = current.as_mut() else {
        return Ok(());
    };
    match local {
        "size" if parent == Some("file") => *pending = Some(Pending::Size),
        // Une empreinte de morceau (`pieces`) n'est pas celle du fichier.
        "hash" if parent != Some("pieces") => {
            *pending = Some(Pending::Hash(attr(e, "type")?.unwrap_or_default()));
        }
        // Metalink 3 : `type="bittorrent"` désigne un `.torrent`, servi en HTTP lui aussi.
        "url"
            if format == Format::V3
                && attr(e, "type")?.is_some_and(|t| {
                    !matches!(t.trim().to_ascii_lowercase().as_str(), "http" | "https")
                }) => {}
        "url" if file.urls.len() < MAX_URLS_READ => {
            let rank = match format {
                // 1 est la meilleure ; sans priorité, après les autres.
                Format::V4 => attr(e, "priority")?
                    .and_then(|p| p.trim().parse::<u64>().ok())
                    .unwrap_or(u64::MAX),
                // `preference` de 0 à 100, la plus haute d'abord.
                Format::V3 => attr(e, "preference")?
                    .and_then(|p| p.trim().parse::<u64>().ok())
                    .map_or(u64::MAX, |p| 100u64.saturating_sub(p.min(100))),
            };
            *pending = Some(Pending::Url(rank));
        }
        _ => {}
    }
    Ok(())
}

/// Fermeture d'un élément : son texte va où `open` l'a dit.
fn close(
    local: &str,
    current: &mut Option<RawFile>,
    files: &mut Vec<RawFile>,
    pending: &mut Option<Pending>,
    text: &str,
) {
    if local == "file" {
        if let Some(file) = current.take() {
            files.push(file);
        }
        *pending = None;
        return;
    }
    let (Some(file), Some(what)) = (current.as_mut(), pending.take()) else {
        return;
    };
    let value = text.trim().to_owned();
    match what {
        Pending::Size => file.size = Some(value),
        Pending::Hash(kind) => file.hashes.push((kind, value)),
        Pending::Url(rank) => {
            if !value.is_empty() {
                file.urls.push((value, rank));
            }
        }
    }
}

/// `&amp;` et les autres entités prédéfinies, ou une référence de caractère (`&#38;`).
fn resolve_ref(r: &quick_xml::events::BytesRef<'_>) -> Result<Cow<'static, str>, MetalinkError> {
    if let Some(c) = r
        .resolve_char_ref()
        .map_err(|e| MetalinkError::Xml(e.to_string()))?
    {
        return Ok(Cow::Owned(c.to_string()));
    }
    let name = r.decode().map_err(|e| MetalinkError::Xml(e.to_string()))?;
    quick_xml::escape::resolve_predefined_entity(&name)
        .map(Cow::Borrowed)
        .ok_or_else(|| MetalinkError::Xml(format!("entité inconnue &{name};")))
}

#[cfg(test)]
mod tests;
