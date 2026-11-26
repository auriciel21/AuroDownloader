//! Pages d'une copie de site (SPEC §7.18 ter) : les liens d'une page, avec leur rôle
//! (page à suivre, ressource d'affichage, feuille de style, autre), et la réécriture de la
//! page pour la navigation hors ligne.
//!
//! Lecteur de balises sur les octets (une page n'est pas toujours en UTF-8) : seules les
//! balises ouvrantes comptent, avec la place de chaque attribut ; le contenu de `<script>`
//! est sauté, celui de `<style>` lu comme un style. La réécriture ne change que la valeur
//! des attributs de liens et les adresses des styles, retire `<base href>` et les
//! `integrity` des liens rendus locaux, et ajoute au besoin `<meta charset>` ; le reste de
//! la page est gardé octet pour octet.

use std::collections::HashSet;
use std::ops::Range;

use reqwest::Url;

use super::css;

/// Ce qu'un lien désigne.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Lien de navigation : `href` de `a` et `area`, `src` de `frame` et `iframe`.
    Navigate,
    /// Ressource d'affichage : image, script, police, icône.
    Resource,
    /// Feuille de style.
    Style,
    /// Autre (vidéo, son, objet, `link` d'une autre sorte) : réécrit seulement s'il est
    /// copié (fichier coché).
    Other,
}

/// L'usage d'un attribut relevé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Use {
    Url(Role),
    Srcset,
    Style,
    Base,
    Integrity,
}

/// Un attribut relevé.
#[derive(Debug, Clone)]
struct Attr {
    /// Rang de sa balise dans la page.
    tag: usize,
    /// L'attribut entier, de son nom à la fin de sa valeur (guillemet compris).
    full: Range<usize>,
    /// Sa valeur brute (entre les guillemets).
    value: Range<usize>,
    quoted: bool,
    /// Sa valeur, entités lues.
    text: String,
    usage: Use,
}

/// Une page lue.
#[derive(Debug, Default)]
pub struct Scan {
    attrs: Vec<Attr>,
    /// Contenu des blocs `<style>`.
    blocks: Vec<Range<usize>>,
    /// Juste après `<head …>`.
    head_end: Option<usize>,
    /// La page dit son encodage (`<meta charset>`, `http-equiv`, déclaration XML).
    declares_charset: bool,
}

/// Une balise ouvrante lue : nom en minuscules, attributs (nom en minuscules, attribut
/// entier, valeur brute, entre guillemets), et sa fin.
struct RawTag {
    name: String,
    attrs: Vec<(String, Range<usize>, Range<usize>, bool)>,
    end: usize,
}

fn find_ignore_case(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
        .map(|p| from + p)
}

/// La balise ouvrante qui commence à `lt` (`<` suivi d'une lettre).
fn read_tag(bytes: &[u8], lt: usize) -> RawTag {
    let mut i = lt + 1;
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':'))
    {
        i += 1;
    }
    let name = String::from_utf8_lossy(&bytes[lt + 1..i]).to_ascii_lowercase();
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
                return RawTag {
                    name,
                    attrs,
                    end: i,
                };
            }
            Some(b'>') => {
                return RawTag {
                    name,
                    attrs,
                    end: i + 1,
                };
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
            i += 1;
            continue;
        }
        let key = String::from_utf8_lossy(&bytes[key_start..i]).to_ascii_lowercase();
        let mut j = i;
        while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        if bytes.get(j) != Some(&b'=') {
            attrs.push((key, key_start..i, i..i, false));
            continue;
        }
        i = j + 1;
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        match bytes.get(i) {
            Some(&q @ (b'"' | b'\'')) => {
                let start = i + 1;
                let end = bytes[start..]
                    .iter()
                    .position(|&b| b == q)
                    .map_or(bytes.len(), |e| start + e);
                i = (end + 1).min(bytes.len());
                attrs.push((key, key_start..i, start..end, true));
            }
            _ => {
                let start = i;
                while bytes
                    .get(i)
                    .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>')
                {
                    i += 1;
                }
                attrs.push((key, key_start..i, start..i, false));
            }
        }
    }
}

/// Entités HTML d'une valeur : celles de XML, `&nbsp;` et les références numériques.
fn decode_entities(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    if !text.contains('&') {
        return text.into_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest: &str = &text;
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
                "nbsp" => Some('\u{a0}'),
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

fn value_of<'a>(
    bytes: &'a [u8],
    attrs: &[(String, Range<usize>, Range<usize>, bool)],
    name: &str,
) -> Option<&'a [u8]> {
    attrs
        .iter()
        .find(|(k, ..)| k == name)
        .map(|(_, _, v, _)| &bytes[v.clone()])
}

fn lower(bytes: Option<&[u8]>) -> String {
    bytes.map_or_else(String::new, |b| {
        String::from_utf8_lossy(b).to_ascii_lowercase()
    })
}

/// Le rôle d'une adresse de `link`, d'après `rel` et `as`.
fn link_role(rel: &str, as_: &str) -> Role {
    let rels: Vec<&str> = rel.split_ascii_whitespace().collect();
    if rels.contains(&"stylesheet") {
        Role::Style
    } else if rels.iter().any(|r| {
        matches!(
            *r,
            "icon" | "apple-touch-icon" | "apple-touch-icon-precomposed" | "mask-icon"
        )
    }) || rels.contains(&"modulepreload")
    {
        Role::Resource
    } else if rels.contains(&"preload") {
        match as_ {
            "style" => Role::Style,
            "script" | "font" | "image" => Role::Resource,
            _ => Role::Other,
        }
    } else {
        Role::Other
    }
}

/// L'usage d'un attribut d'une balise, s'il porte un lien (SPEC §7.18 ter).
fn usage(tag: &str, attr: &str, kind: &str, rel: &str, as_: &str) -> Option<Use> {
    use Role::{Navigate, Other, Resource};
    let role = match (tag, attr) {
        (_, "style") => return Some(Use::Style),
        ("base", "href") => return Some(Use::Base),
        ("link" | "script", "integrity") => return Some(Use::Integrity),
        ("img" | "source", "srcset") => return Some(Use::Srcset),
        ("a" | "area", "href") | ("frame" | "iframe", "src") => Navigate,
        ("img", "src" | "data-src")
        | ("video", "poster")
        | ("script", "src")
        | ("body" | "table" | "td" | "th" | "tr", "background")
        | ("image" | "use", "href" | "xlink:href") => Resource,
        ("input", "src") if kind == "image" => Resource,
        ("input", "src") => Other,
        ("link", "href") => link_role(rel, as_),
        ("video" | "audio" | "source" | "track" | "embed", "src") | ("object", "data") => Other,
        _ => return None,
    };
    Some(Use::Url(role))
}

/// Lit une page.
pub fn scan(bytes: &[u8]) -> Scan {
    let mut scan = Scan::default();
    let mut at = 0;
    let mut tag_index = 0;
    if bytes.starts_with(b"<?xml") {
        let line = bytes.iter().position(|&b| b == b'>').unwrap_or(bytes.len());
        scan.declares_charset |= find_ignore_case(&bytes[..line], 0, b"encoding").is_some();
    }
    while let Some(lt) = bytes
        .get(at..)
        .and_then(|rest| rest.iter().position(|&b| b == b'<'))
        .map(|p| at + p)
    {
        if bytes[lt..].starts_with(b"<!--") {
            at = find_ignore_case(bytes, lt + 4, b"-->").map_or(bytes.len(), |i| i + 3);
            continue;
        }
        if !bytes.get(lt + 1).is_some_and(u8::is_ascii_alphabetic) {
            at = lt + 1;
            continue;
        }
        let tag = read_tag(bytes, lt);
        at = tag.end;
        let index = tag_index;
        tag_index += 1;
        let kind = lower(value_of(bytes, &tag.attrs, "type"));
        let rel = lower(value_of(bytes, &tag.attrs, "rel"));
        let as_ = lower(value_of(bytes, &tag.attrs, "as"));
        for (name, full, value, quoted) in &tag.attrs {
            if let Some(usage) = usage(&tag.name, name, &kind, &rel, &as_) {
                scan.attrs.push(Attr {
                    tag: index,
                    full: full.clone(),
                    value: value.clone(),
                    quoted: *quoted,
                    text: decode_entities(&bytes[value.clone()]),
                    usage,
                });
            }
        }
        match tag.name.as_str() {
            "head" if scan.head_end.is_none() => scan.head_end = Some(tag.end),
            "meta" => {
                let equiv = lower(value_of(bytes, &tag.attrs, "http-equiv"));
                let content = lower(value_of(bytes, &tag.attrs, "content"));
                scan.declares_charset |= value_of(bytes, &tag.attrs, "charset").is_some()
                    || (equiv == "content-type" && content.contains("charset"));
            }
            // Contenu brut : un script est sauté, un style lu comme un style.
            "script" | "style" => {
                let close: &[u8] = if tag.name == "script" {
                    b"</script"
                } else {
                    b"</style"
                };
                let end = find_ignore_case(bytes, at, close).unwrap_or(bytes.len());
                if tag.name == "style" {
                    scan.blocks.push(at..end);
                }
                at = end;
            }
            _ => {}
        }
    }
    scan
}

/// Une adresse à suivre ou à copier : absolue, HTTP(S), sans ancre. `None` pour une
/// adresse vide, une ancre seule, `data:`, `javascript:`, `mailto:`…
pub fn target(base: &Url, raw: &str) -> Option<Url> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('#') {
        return None;
    }
    let mut url = base.join(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

/// L'adresse de la page après `<base href>` (la première).
fn base_of(scan: &Scan, page: &Url) -> Url {
    scan.attrs
        .iter()
        .find(|a| a.usage == Use::Base)
        .and_then(|a| page.join(a.text.trim()).ok())
        .unwrap_or_else(|| page.clone())
}

/// Les liens d'une page lue à l'adresse `page`, sans doublon, dans l'ordre, avec leur
/// rôle ; ceux des styles (`style`, blocs) sont des ressources, ou des styles pour un
/// `@import`.
pub fn links(bytes: &[u8], scan: &Scan, page: &Url) -> Vec<(Url, Role)> {
    let base = base_of(scan, page);
    let mut seen: HashSet<(String, Role)> = HashSet::new();
    let mut out = Vec::new();
    let mut push = |url: Option<Url>, role: Role| {
        if let Some(url) = url
            && seen.insert((url.to_string(), role))
        {
            out.push((url, role));
        }
    };
    let css_role = |import: bool| if import { Role::Style } else { Role::Resource };
    // Dans l'ordre de la page : attributs et blocs mêlés.
    let mut events: Vec<(usize, Either<'_>)> = scan
        .attrs
        .iter()
        .map(|a| (a.value.start, Either::Attr(a)))
        .chain(
            scan.blocks
                .iter()
                .map(|b| (b.start, Either::Block(b.clone()))),
        )
        .collect();
    events.sort_by_key(|(at, _)| *at);
    for (_, event) in events {
        match event {
            Either::Attr(a) => match a.usage {
                Use::Url(role) => push(target(&base, &a.text), role),
                Use::Srcset => {
                    for (url, _) in srcset(&a.text) {
                        push(target(&base, url), Role::Resource);
                    }
                }
                Use::Style => {
                    for r in css::refs(a.text.as_bytes()) {
                        push(target(&base, &r.url), css_role(r.import));
                    }
                }
                Use::Base | Use::Integrity => {}
            },
            Either::Block(range) => {
                for r in css::refs(&bytes[range]) {
                    push(target(&base, &r.url), css_role(r.import));
                }
            }
        }
    }
    out
}

enum Either<'a> {
    Attr(&'a Attr),
    Block(Range<usize>),
}

/// Les candidats d'un `srcset` : adresse et descripteur (`2x`, `640w`, ou rien).
fn srcset(text: &str) -> Vec<(&str, &str)> {
    text.split(',')
        .filter_map(|candidate| {
            let candidate = candidate.trim();
            let (url, descriptor) = match candidate.find(char::is_whitespace) {
                Some(space) => (&candidate[..space], candidate[space..].trim()),
                None => (candidate, ""),
            };
            (!url.is_empty()).then_some((url, descriptor))
        })
        .collect()
}

/// Une valeur d'attribut, échappée pour des guillemets doubles.
fn attr_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// La page peut-elle être réécrite ? Pas en UTF-16 ni en UTF-32 (marque d'ordre des
/// octets, ou octets nuls au début).
pub fn rewritable(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    !(head.starts_with(&[0xFE, 0xFF]) || head.starts_with(&[0xFF, 0xFE]) || head.contains(&0))
}

/// Une adresse réécrite : `local` rend le chemin relatif d'une adresse copiée (sans
/// ancre), `None` pour une adresse laissée en ligne, écrite alors absolue. L'ancre est
/// gardée.
pub(crate) fn rewritten(
    base: &Url,
    raw: &str,
    local: &mut dyn FnMut(&Url) -> Option<String>,
) -> Option<(String, bool)> {
    let url = target(base, raw)?;
    let fragment = base
        .join(raw.trim())
        .ok()
        .and_then(|u| u.fragment().map(|f| format!("#{f}")))
        .unwrap_or_default();
    Some(match local(&url) {
        Some(path) => (format!("{path}{fragment}"), true),
        None => (format!("{url}{fragment}"), false),
    })
}

/// La page réécrite pour la navigation hors ligne. `page` : son adresse (après
/// redirections) ; `local` : le chemin relatif depuis la page d'une adresse copiée ;
/// `charset` : l'encodage annoncé par l'en-tête `Content-Type`, ajouté en
/// `<meta charset>` si la page ne dit pas le sien.
pub fn rewrite(
    bytes: &[u8],
    page: &Url,
    charset: Option<&str>,
    local: &mut dyn FnMut(&Url) -> Option<String>,
) -> Vec<u8> {
    let scan = scan(bytes);
    let base = base_of(&scan, page);
    let mut edits: Vec<(Range<usize>, Vec<u8>)> = Vec::new();
    let mut local_tags: HashSet<usize> = HashSet::new();
    for a in &scan.attrs {
        let new = match a.usage {
            Use::Url(_) => match rewritten(&base, &a.text, local) {
                Some((text, is_local)) => {
                    if is_local {
                        local_tags.insert(a.tag);
                    }
                    Some(text)
                }
                None => None,
            },
            Use::Srcset => {
                let candidates: Vec<String> = srcset(&a.text)
                    .into_iter()
                    .map(|(url, descriptor)| {
                        let url = match rewritten(&base, url, local) {
                            Some((text, is_local)) => {
                                if is_local {
                                    local_tags.insert(a.tag);
                                }
                                text
                            }
                            None => url.to_owned(),
                        };
                        // Une virgule dans une adresse en ligne couperait le candidat.
                        let url = url.replace(',', "%2C");
                        if descriptor.is_empty() {
                            url
                        } else {
                            format!("{url} {descriptor}")
                        }
                    })
                    .collect();
                Some(candidates.join(", "))
            }
            Use::Style if css::refs(a.text.as_bytes()).is_empty() => None,
            Use::Style => {
                let css = css::rewrite(a.text.as_bytes(), &mut |url, _| {
                    rewritten(&base, url, local).map(|(text, _)| text)
                });
                Some(String::from_utf8_lossy(&css).into_owned())
            }
            Use::Base => {
                edits.push((a.full.clone(), Vec::new()));
                None
            }
            Use::Integrity => None,
        };
        if let Some(new) = new {
            edits.push((a.value.clone(), attr_value(&new, a.quoted)));
        }
    }
    // `integrity` d'un lien rendu local : le navigateur refuserait le fichier.
    for a in &scan.attrs {
        if a.usage == Use::Integrity && local_tags.contains(&a.tag) {
            edits.push((a.full.clone(), Vec::new()));
        }
    }
    for block in &scan.blocks {
        let css = css::rewrite(&bytes[block.clone()], &mut |url, _| {
            rewritten(&base, url, local).map(|(text, _)| text)
        });
        edits.push((block.clone(), css));
    }
    if let (Some(charset), false, Some(at)) = (charset, scan.declares_charset, scan.head_end) {
        let clean: String = charset
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
            .collect();
        if !clean.is_empty() {
            edits.push((at..at, format!("<meta charset=\"{clean}\">").into_bytes()));
        }
    }
    edits.sort_by_key(|(r, _)| (r.start, r.end));
    let mut out = Vec::with_capacity(bytes.len() + 256);
    let mut at = 0;
    for (range, new) in edits {
        if range.start < at {
            continue;
        }
        out.extend_from_slice(&bytes[at..range.start]);
        out.extend_from_slice(&new);
        at = range.end;
    }
    out.extend_from_slice(&bytes[at..]);
    out
}

/// Une valeur d'attribut à mettre à la place de la valeur brute : entre ses guillemets
/// d'origine (sans eux), ou entre guillemets doubles pour une valeur qui n'en avait pas.
fn attr_value(text: &str, quoted: bool) -> Vec<u8> {
    let full = attr_text(text);
    if quoted {
        // Les guillemets d'origine restent : seul le texte change. Une apostrophe est
        // échappée aussi, pour une valeur entre apostrophes.
        full[1..full.len() - 1].replace('\'', "&#39;").into_bytes()
    } else {
        full.into_bytes()
    }
}

#[cfg(test)]
mod tests;
