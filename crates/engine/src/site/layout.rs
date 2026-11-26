//! Rangement d'une copie de site sur le disque (SPEC §7.18 ter) : de l'adresse au chemin
//! dans le dossier de la copie, et d'un fichier copié à un autre.
//!
//! Un dossier par hôte (port compris : `exemple.org+8080`), puis les dossiers et le nom de
//! l'adresse, décodés et nettoyés comme un nom de fichier (F-09) ; `index.html` pour un
//! chemin qui finit par `/` ; les paramètres derrière un `@` ; l'extension du type ajoutée
//! (`.html`, `.css`) ; chaque partie de 100 caractères au plus, 16 dossiers au plus.

use percent_encoding::percent_decode_str;
use reqwest::Url;
use sha2::{Digest, Sha256};

use crate::net::filename::{NameRules, sanitize};

/// Longueur maximale d'une partie du chemin, en caractères.
pub const MAX_PART_CHARS: usize = 100;
/// Et en octets UTF-8 : la place du `.part` et d'un « (n) » reste sous 255.
const MAX_PART_BYTES: usize = 200;
/// Dossiers au plus sous celui de l'hôte (les plus profonds réunis en un seul).
pub const MAX_DIRS: usize = 16;

/// Ce que contient le fichier, pour son extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// Page HTML : `.html` si elle n'a ni `.html` ni `.htm`.
    Html,
    /// Feuille de style : `.css` si elle ne l'a pas.
    Css,
    /// Gardé tel quel.
    Other,
}

/// Suffixes des fichiers provisoires d'une copie : un nom d'élément ne finit jamais par
/// l'un d'eux (un `_` y est ajouté), pour qu'aucun élément ne prenne le fichier provisoire
/// d'un autre.
pub const TEMP_SUFFIXES: [&str; 2] = [".part", ".rewrite"];

fn decoded(segment: &str) -> String {
    percent_decode_str(segment).decode_utf8_lossy().into_owned()
}

/// Une partie raccourcie à [`MAX_PART_CHARS`], unique par 8 caractères de son empreinte,
/// extension courte gardée.
fn shortened(part: &str) -> String {
    if part.chars().count() <= MAX_PART_CHARS && part.len() <= MAX_PART_BYTES {
        return part.to_owned();
    }
    let digest = Sha256::digest(part.as_bytes());
    let tag: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
    let ext = part
        .rfind('.')
        .map(|dot| &part[dot..])
        .filter(|e| {
            e.len() > 1 && e.len() <= 11 && e[1..].bytes().all(|b| b.is_ascii_alphanumeric())
        })
        .unwrap_or("");
    // Le nom raccourci tient en caractères et en octets (le `.part` compris).
    let (max_chars, max_bytes) = (
        MAX_PART_CHARS - 9 - ext.len(),
        MAX_PART_BYTES - 9 - ext.len(),
    );
    let mut stem = String::new();
    for c in part.chars().take(max_chars) {
        if stem.len() + c.len_utf8() > max_bytes {
            break;
        }
        stem.push(c);
    }
    format!("{}-{tag}{ext}", stem.trim_end())
}

/// Raccourci d'abord (l'empreinte distingue deux noms longs qui ne diffèrent qu'à la fin,
/// que le nettoyage tronquerait pareil), puis nettoyé.
fn clean(raw: &str) -> String {
    let cleaned = sanitize(&shortened(raw), NameRules::native());
    // Un nettoyage qui allonge (nom réservé de Windows) reste dans la limite.
    shortened(&cleaned)
}

fn has_ext(name: &str, exts: &[&str]) -> bool {
    let lower = name.to_lowercase();
    exts.iter().any(|e| lower.ends_with(e))
}

/// Le chemin d'une adresse dans le dossier de la copie, en parties : l'hôte, les dossiers,
/// le nom. `url` est absolue, en HTTP(S).
pub fn parts_of(url: &Url, content: Content) -> Vec<String> {
    let host = url.host_str().unwrap_or("hote");
    let host = match url.port() {
        Some(port) => format!("{host}+{port}"),
        None => host.to_owned(),
    };
    let mut parts = vec![clean(&host)];
    let segments: Vec<&str> = url
        .path_segments()
        .map(Iterator::collect)
        .unwrap_or_default();
    let (dirs, last) = match segments.split_last() {
        Some((last, dirs)) => (dirs, *last),
        None => (&[][..], ""),
    };
    let mut dirs: Vec<String> = dirs
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| clean(&decoded(s)))
        .collect();
    if dirs.len() > MAX_DIRS {
        let deep = dirs.split_off(MAX_DIRS - 1).join("_");
        dirs.push(shortened(&deep));
    }
    parts.extend(dirs);
    let mut name = if last.is_empty() {
        "index.html".to_owned()
    } else {
        decoded(last)
    };
    if let Some(query) = url.query().filter(|q| !q.is_empty()) {
        name = format!("{name}@{}", decoded(query));
    }
    match content {
        Content::Html if !has_ext(&name, &[".html", ".htm"]) => name.push_str(".html"),
        Content::Css if !has_ext(&name, &[".css"]) => name.push_str(".css"),
        _ => {}
    }
    let mut name = clean(&name);
    if has_ext(&name, &TEMP_SUFFIXES) {
        name.push('_');
    }
    parts.push(name);
    parts
}

/// Caractères gardés tels quels dans un chemin relatif écrit dans une page ; les autres
/// sont encodés (`%20`, UTF-8 encodé). `,` l'est aussi (séparateur de `srcset`), `:` aussi
/// (une première partie `a:b` se lirait comme un schéma).
fn keep(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'.' | b'_' | b'~' | b'@' | b'=' | b'+' | b'!' | b'$'
        )
}

fn encode_part(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    for &b in part.as_bytes() {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Le chemin relatif, encodé pour une adresse, qui mène du fichier `from` au fichier `to`
/// (chemins de la copie, parties séparées par `/`).
pub fn relative(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').collect();
    let to: Vec<&str> = to.split('/').collect();
    let from_dirs = &from[..from.len().saturating_sub(1)];
    let (to_dirs, to_name) = match to.split_last() {
        Some((name, dirs)) => (dirs, *name),
        None => (&[][..], ""),
    };
    let common = from_dirs
        .iter()
        .zip(to_dirs)
        .take_while(|(a, b)| a == b)
        .count();
    let mut out: Vec<String> = vec!["..".to_owned(); from_dirs.len() - common];
    out.extend(to_dirs[common..].iter().map(|p| encode_part(p)));
    out.push(encode_part(to_name));
    out.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn path(url: &str, content: Content) -> String {
        parts_of(&Url::parse(url).unwrap(), content).join("/")
    }

    #[test]
    fn an_address_becomes_a_path_under_its_host() {
        assert_eq!(
            path("https://exemple.org/", Content::Html),
            "exemple.org/index.html"
        );
        assert_eq!(
            path("https://exemple.org/doc/", Content::Html),
            "exemple.org/doc/index.html"
        );
        assert_eq!(
            path("https://exemple.org/doc/a.html", Content::Html),
            "exemple.org/doc/a.html"
        );
        assert_eq!(
            path("https://exemple.org/doc/a.HTM", Content::Html),
            "exemple.org/doc/a.HTM"
        );
        assert_eq!(
            path("https://exemple.org/a-propos", Content::Html),
            "exemple.org/a-propos.html"
        );
        assert_eq!(
            path("http://exemple.org:8080/x.png", Content::Other),
            "exemple.org+8080/x.png"
        );
        assert_eq!(
            path("https://cdn.exemple.net/css/site", Content::Css),
            "cdn.exemple.net/css/site.css"
        );
        assert_eq!(
            path(
                "https://exemple.org/b%C3%A9b%C3%A9/%C3%A9t%C3%A9.png",
                Content::Other
            ),
            "exemple.org/bébé/été.png"
        );
        assert_eq!(
            path("https://exemple.org//a//b.png", Content::Other),
            "exemple.org/a/b.png"
        );
    }

    #[test]
    fn parameters_follow_an_at_sign_and_the_type_comes_last() {
        assert_eq!(
            path("https://exemple.org/page.php?id=3", Content::Html),
            "exemple.org/page.php@id=3.html"
        );
        assert_eq!(
            path("https://exemple.org/style.css?v=2", Content::Css),
            "exemple.org/style.css@v=2.css"
        );
        assert_eq!(
            path("https://exemple.org/logo.png?v=2", Content::Other),
            "exemple.org/logo.png@v=2"
        );
        assert_eq!(
            path("https://exemple.org/?p=1", Content::Html),
            "exemple.org/index.html@p=1.html"
        );
    }

    #[test]
    fn names_are_cleaned_and_never_end_like_a_temporary_file() {
        let windows = cfg!(windows);
        let q = path("https://exemple.org/a%3Fb%2Ac.png", Content::Other);
        assert_eq!(
            q,
            if windows {
                "exemple.org/a_b_c.png"
            } else {
                "exemple.org/a?b*c.png"
            }
        );
        assert_eq!(
            path("https://exemple.org/%2E%2E/x.png", Content::Other),
            "exemple.org/x.png"
        );
        assert_eq!(
            path("https://exemple.org/a%2Fb.png", Content::Other),
            "exemple.org/a_b.png"
        );
        assert_eq!(
            path("https://exemple.org/x.part", Content::Other),
            "exemple.org/x.part_"
        );
        assert_eq!(
            path("https://exemple.org/x.REWRITE", Content::Other),
            "exemple.org/x.REWRITE_"
        );
        if windows {
            assert_eq!(
                path("https://exemple.org/con.png", Content::Other),
                "exemple.org/_con.png"
            );
        }
    }

    #[test]
    fn long_parts_and_deep_paths_are_bounded() {
        let long = "a".repeat(300);
        let p = path(
            &format!("https://exemple.org/{long}/{long}.png"),
            Content::Other,
        );
        let parts: Vec<&str> = p.split('/').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[1].chars().count() <= MAX_PART_CHARS);
        assert!(parts[2].ends_with(".png") && parts[2].chars().count() <= MAX_PART_CHARS);
        // Deux noms longs qui diffèrent à la fin restent deux noms.
        let other = path(
            &format!("https://exemple.org/{long}/{long}b.png"),
            Content::Other,
        );
        assert_ne!(p, other);
        let deep: String = (0..30).map(|i| format!("d{i}/")).collect();
        let p = path(&format!("https://exemple.org/{deep}x.png"), Content::Other);
        let parts: Vec<&str> = p.split('/').collect();
        assert_eq!(parts.len(), 1 + MAX_DIRS + 1);
        assert_eq!(
            parts[MAX_DIRS],
            "d15_d16_d17_d18_d19_d20_d21_d22_d23_d24_d25_d26_d27_d28_d29"
        );
    }

    #[test]
    fn relative_paths_climb_then_descend_and_are_encoded() {
        assert_eq!(relative("h/doc/index.html", "h/doc/a.html"), "a.html");
        assert_eq!(
            relative("h/doc/index.html", "h/img/a b.png"),
            "../img/a%20b.png"
        );
        assert_eq!(
            relative("h/index.html", "cdn/css/s.css"),
            "../cdn/css/s.css"
        );
        assert_eq!(relative("h/a/b/c.html", "h/a.html"), "../../a.html");
        assert_eq!(relative("h/index.html", "h/index.html"), "index.html");
        assert_eq!(
            relative("h/x.html", "h/page.php@id=3.html"),
            "page.php@id=3.html"
        );
        assert_eq!(
            relative("h/x.html", "h/a#b?c,d:e%f.png"),
            "a%23b%3Fc%2Cd%3Ae%25f.png"
        );
        assert_eq!(relative("h/x.html", "h/été.png"), "%C3%A9t%C3%A9.png");
    }

    proptest! {
        /// Quelle que soit l'adresse : des parties propres, non vides, jamais `..`, et le
        /// premier dossier est l'hôte.
        #[test]
        fn any_address_gives_safe_parts(path in "(/[a-zA-Z0-9%._~!$&'()*+,;=:@ -]{0,40}){0,20}", query in proptest::option::of("[a-z=&%0-9]{0,30}")) {
            let mut text = format!("https://exemple.org{path}");
            if let Some(q) = query { text.push('?'); text.push_str(&q); }
            let Ok(url) = Url::parse(&text) else { return Ok(()); };
            for content in [Content::Html, Content::Css, Content::Other] {
                let parts = parts_of(&url, content);
                prop_assert_eq!(parts[0].as_str(), "exemple.org");
                prop_assert!(parts.len() <= MAX_DIRS + 2);
                for p in &parts {
                    prop_assert!(!p.is_empty() && p != "." && p != "..");
                    prop_assert!(!p.contains('/') && !p.contains('\\'));
                    prop_assert!(p.chars().count() <= MAX_PART_CHARS);
                    prop_assert_eq!(sanitize(p, NameRules::native()), p.clone());
                }
                let name = parts.last().unwrap();
                prop_assert!(!has_ext(name, &TEMP_SUFFIXES));
            }
        }
    }
}
