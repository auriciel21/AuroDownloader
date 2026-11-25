//! Analyse de liens (F-29, F-31, SPEC §7.18, phase 6, étape 2) : les liens d'une page HTML
//! ou d'un texte (texte collé, liste importée), avec un nom, une extension et une sorte
//! (fichier, page, lien magnet) pour chacun.
//!
//! Décisions de l'utilisateur (2026-11-11) : la page est lue par le moteur, HTML seul, sans
//! JavaScript ni exploration des pages liées ; une liste importée se lit comme un texte
//! (toute adresse trouvée) ; les filtres sont appliqués par les clients. Le HTML est
//! parcouru par un petit lecteur de balises, sans arbre : seuls comptent les attributs qui
//! portent des liens et les adresses écrites en clair.

use std::collections::HashSet;

use reqwest::Url;

use crate::net::filename::{NameRules, name_from_url, sanitize};

/// Taille maximale d'une page ou d'un texte analysés.
pub const MAX_INPUT: usize = 4 << 20;
/// Liens rendus au plus par une analyse.
pub const MAX_LINKS: usize = 10_000;

/// Extensions des pages web : un lien vers l'une d'elles n'est pas un fichier à télécharger
/// (montré seulement sur demande). Aussi celles du presse-papiers du bureau ; recopiées dans
/// `ui/src/model.js`, comparées par un test (audit C28).
pub const PAGE_EXTENSIONS: [&str; 17] = [
    "htm", "html", "xhtml", "shtml", "php", "php3", "php5", "asp", "aspx", "jsp", "jspx", "cgi",
    "pl", "cfm", "do", "action", "py",
];

/// Sorte d'un lien.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// Une extension, hors pages web : un fichier à télécharger.
    File,
    /// Une page web, ou une adresse sans extension.
    Page,
    /// Un lien magnet (torrent).
    Magnet,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::File => "file",
            LinkKind::Page => "page",
            LinkKind::Magnet => "magnet",
        }
    }
}

/// Un lien trouvé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Adresse absolue, sans ancre (`#…`).
    pub url: String,
    /// Nom de fichier tiré de l'adresse, nettoyé (F-09) ; celui du lien magnet (`dn`).
    pub name: String,
    /// Extension en minuscules, sans le point.
    pub ext: Option<String>,
    pub kind: LinkKind,
}

/// Les liens d'une page HTML, dans l'ordre où ils apparaissent : attributs des balises qui
/// portent un lien, puis adresses écrites en clair (scripts, texte). `base` est l'adresse
/// de la page après ses redirections ; une balise `<base href>` la remplace.
pub fn from_html(html: &str, base: &Url) -> Vec<Link> {
    let mut found = Found::default();
    let mut base = base.clone();
    let mut based = false;
    for tag in Tags::new(html) {
        for (attr, value) in &tag.attrs {
            let value = decode_entities(value);
            if tag.name == "base" && attr == "href" && !based {
                if let Ok(url) = base.join(value.trim()) {
                    base = url;
                    based = true;
                }
                continue;
            }
            if !carries_link(&tag.name, attr) {
                continue;
            }
            if attr == "srcset" {
                // « a.jpg 1x, b.jpg 2x » : chaque adresse, sans son descripteur.
                for candidate in value.split(',') {
                    if let Some(first) = candidate.split_whitespace().next() {
                        found.add_relative(&base, first);
                    }
                }
            } else {
                found.add_relative(&base, &value);
            }
        }
    }
    // Le texte hors des balises (scripts compris) : les adresses des autres attributs
    // (feuilles de style, adresses de partage…) n'y sont pas.
    for candidate in plain_urls(&decode_entities(&outside_tags(html).replace("\\/", "/"))) {
        found.add_absolute(&candidate);
    }
    found.links
}

/// Le texte d'un document sans ses balises ni ses commentaires, chacun remplacé par une
/// espace.
fn outside_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt..];
        let end = if let Some(comment) = after.strip_prefix("<!--") {
            comment.find("-->").map(|i| 4 + i + 3)
        } else if after[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '!')
        {
            after.find('>').map(|i| i + 1)
        } else {
            out.push('<');
            rest = &after[1..];
            continue;
        };
        out.push(' ');
        match end {
            Some(end) => rest = &after[end..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Les adresses d'un texte (texte collé, liste importée, F-31) : HTTP(S) et liens magnet
/// écrits en clair, quel que soit leur entourage (colonnes d'un CSV comprises).
pub fn from_text(text: &str) -> Vec<Link> {
    let mut found = Found::default();
    for candidate in plain_urls(text) {
        found.add_absolute(&candidate);
    }
    found.links
}

/// Les attributs qui portent un lien vers une ressource, par balise.
fn carries_link(tag: &str, attr: &str) -> bool {
    matches!(
        (tag, attr),
        ("a" | "area", "href")
            | ("img", "src" | "srcset" | "data-src")
            | ("video", "src" | "poster")
            | ("audio" | "track" | "embed" | "iframe" | "frame", "src")
            | ("source", "src" | "srcset")
            | ("object", "data")
    )
}

/// Liens retenus, sans doublon, dans l'ordre.
#[derive(Default)]
struct Found {
    links: Vec<Link>,
    seen: HashSet<String>,
}

impl Found {
    fn add_relative(&mut self, base: &Url, value: &str) {
        let value = value.trim();
        if value.is_empty() || value.starts_with('#') {
            return;
        }
        if is_magnet(value) {
            self.add_absolute(value);
            return;
        }
        if let Ok(url) = base.join(value) {
            self.add_url(url);
        }
    }

    fn add_absolute(&mut self, value: &str) {
        if is_magnet(value) {
            self.add_magnet(value);
        } else if let Ok(url) = Url::parse(value) {
            self.add_url(url);
        }
    }

    fn add_url(&mut self, mut url: Url) {
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return;
        }
        url.set_fragment(None);
        let text = url.to_string();
        if self.links.len() >= MAX_LINKS || !self.seen.insert(text.clone()) {
            return;
        }
        let raw = name_from_url(&url).unwrap_or_default();
        let name = if raw.trim().is_empty() {
            sanitize(url.host_str().unwrap_or_default(), NameRules::native())
        } else {
            sanitize(&raw, NameRules::native())
        };
        let ext = extension(&raw);
        let kind = match &ext {
            Some(e) if !PAGE_EXTENSIONS.contains(&e.as_str()) => LinkKind::File,
            _ => LinkKind::Page,
        };
        self.links.push(Link {
            url: text,
            name,
            ext,
            kind,
        });
    }

    fn add_magnet(&mut self, value: &str) {
        let Ok(url) = Url::parse(value) else {
            return;
        };
        // Un lien magnet sans empreinte n'identifie rien.
        if !url.query_pairs().any(|(k, v)| k == "xt" && !v.is_empty()) {
            return;
        }
        let text = url.to_string();
        if self.links.len() >= MAX_LINKS || !self.seen.insert(text.clone()) {
            return;
        }
        let shown = url
            .query_pairs()
            .find(|(k, _)| k == "dn")
            .map(|(_, v)| v.into_owned())
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "magnet".to_owned());
        self.links.push(Link {
            url: text,
            name: sanitize(&shown, NameRules::native()),
            ext: None,
            kind: LinkKind::Magnet,
        });
    }
}

fn is_magnet(text: &str) -> bool {
    text.get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("magnet:?"))
}

/// Extension d'un nom : après le dernier point, 1 à 10 lettres ou chiffres, en minuscules.
fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.len() > 10 {
        return None;
    }
    ext.chars()
        .all(|c| c.is_ascii_alphanumeric())
        .then(|| ext.to_ascii_lowercase())
}

/// Adresses écrites en clair : `http://`, `https://` ou `magnet:?`, jusqu'à un espace, un
/// guillemet, un chevron, une virgule ou un point-virgule (séparateurs d'un CSV) ; la
/// ponctuation finale (point, deux-points, parenthèse…) n'en fait pas partie.
fn plain_urls(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut at = 0;
    while at < lower.len() {
        let next = ["http://", "https://", "magnet:?"]
            .iter()
            .filter_map(|p| lower[at..].find(p).map(|i| at + i))
            .min();
        let Some(start) = next else {
            break;
        };
        let rest = &text[start..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace()
                    || matches!(
                        c,
                        '"' | '\'' | '<' | '>' | '`' | '\\' | '{' | '}' | '|' | '^' | ',' | ';'
                    )
            })
            .unwrap_or(rest.len());
        let mut candidate = &rest[..end];
        // Ponctuation de la phrase, ou parenthèse fermante sans ouvrante.
        loop {
            let trimmed = candidate.trim_end_matches(['.', ',', ';', ':', '!', '?', ']', '*']);
            let trimmed = if trimmed.ends_with(')') && !trimmed.contains('(') {
                &trimmed[..trimmed.len() - 1]
            } else {
                trimmed
            };
            if trimmed.len() == candidate.len() {
                break;
            }
            candidate = trimmed;
        }
        if candidate.len() > 8 {
            found.push(candidate.to_owned());
        }
        at = start + end.max(1);
        if found.len() >= MAX_LINKS * 2 {
            break;
        }
    }
    found
}

/// Une balise ouvrante : son nom en minuscules et ses attributs (noms en minuscules,
/// valeurs brutes).
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
}

/// Les balises ouvrantes d'un document, commentaires sautés.
struct Tags<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Tags<'a> {
    fn new(text: &'a str) -> Self {
        Tags { text, at: 0 }
    }
}

impl Iterator for Tags<'_> {
    type Item = Tag;

    fn next(&mut self) -> Option<Tag> {
        let bytes = self.text.as_bytes();
        loop {
            let lt = self.at + self.text.get(self.at..)?.find('<')?;
            if self.text[lt..].starts_with("<!--") {
                self.at = match self.text[lt + 4..].find("-->") {
                    Some(i) => lt + 4 + i + 3,
                    None => self.text.len(),
                };
                continue;
            }
            let mut i = lt + 1;
            if !bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
                self.at = i;
                continue;
            }
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
            {
                i += 1;
            }
            let name = self.text[lt + 1..i].to_ascii_lowercase();
            let mut attrs = Vec::new();
            loop {
                while bytes
                    .get(i)
                    .is_some_and(|b| b.is_ascii_whitespace() || *b == b'/')
                {
                    i += 1;
                }
                match bytes.get(i) {
                    None => {
                        self.at = i;
                        return Some(Tag { name, attrs });
                    }
                    Some(b'>') => {
                        self.at = i + 1;
                        return Some(Tag { name, attrs });
                    }
                    _ => {}
                }
                let key_start = i;
                while bytes
                    .get(i)
                    .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'=' | b'>' | b'/'))
                {
                    i += 1;
                }
                if i == key_start {
                    // Un `/` ou `=` isolé : sauté.
                    i += 1;
                    continue;
                }
                let key = self.text[key_start..i].to_ascii_lowercase();
                while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                    i += 1;
                }
                if bytes.get(i) != Some(&b'=') {
                    attrs.push((key, String::new()));
                    continue;
                }
                i += 1;
                while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                    i += 1;
                }
                let value = match bytes.get(i) {
                    Some(&q @ (b'"' | b'\'')) => {
                        let start = i + 1;
                        let end = self.text[start..]
                            .find(q as char)
                            .map_or(self.text.len(), |e| start + e);
                        i = (end + 1).min(self.text.len());
                        &self.text[start..end]
                    }
                    _ => {
                        let start = i;
                        while bytes
                            .get(i)
                            .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>')
                        {
                            i += 1;
                        }
                        &self.text[start..i]
                    }
                };
                attrs.push((key, value.to_owned()));
            }
        }
    }
}

/// Entités HTML d'une valeur : les cinq de XML, `&nbsp;` et les références numériques ;
/// les autres restent telles quelles.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let decoded = after.find(';').filter(|&semi| semi <= 10).and_then(|semi| {
            let entity = &after[..semi];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => entity.strip_prefix('#').and_then(|n| {
                    let code = match n.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => n.parse().ok(),
                    };
                    code.and_then(char::from_u32)
                }),
            };
            c.map(|c| (c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests;
