//! Nom du fichier final (F-09) et défense contre les noms piégés (SPEC §8.1).
//!
//! Le nom vient de `Content-Disposition` ou de l'URL : il est fourni par le serveur et ne
//! doit jamais sortir du dossier de destination ni viser un nom réservé du système.
//! [`sanitize`] renvoie toujours un seul composant de chemin, non vide, différent de `.`
//! et `..`, assez court pour que le `.part` tienne aussi.

use percent_encoding::percent_decode_str;
use reqwest::Url;

/// Nom utilisé quand le serveur n'en donne aucun d'utilisable.
pub const FALLBACK_NAME: &str = "download";

/// Longueur maximale en octets UTF-8 : 255 (limite courante des systèmes de fichiers)
/// moins les 5 octets de `.part`. En UTF-16 (Windows), le nom est alors au plus aussi
/// long qu'en UTF-8.
pub const MAX_NAME_BYTES: usize = 250;

/// Une extension plus longue n'est pas préservée quand il faut raccourcir le nom.
const MAX_EXTENSION_BYTES: usize = 16;

/// Règles de nommage du système de destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRules {
    /// Windows, et les cartes mémoire FAT ou exFAT d'Android : caractères `<>:"|?*`
    /// interdits, noms réservés (`CON`, `COM1`…), pas de point ni d'espace final.
    Windows,
    /// Linux : seul `/` est interdit par le système ; `\` est aussi remplacé.
    Unix,
}

impl NameRules {
    /// Règles du système qui exécute le moteur.
    pub fn native() -> Self {
        if cfg!(any(windows, target_os = "android")) {
            NameRules::Windows
        } else {
            NameRules::Unix
        }
    }
}

/// Caractères invisibles qui changent l'ordre d'affichage : `fichier` + U+202E + `fdp.exe`
/// s'afficherait `fichierexe.pdf`. Supprimés.
fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}')
}

fn is_forbidden(c: char, rules: NameRules) -> bool {
    c == '/'
        || c == '\\'
        || c.is_control()
        || (rules == NameRules::Windows && matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
}

/// Noms de périphériques de Windows, réservés quelle que soit l'extension.
fn is_reserved_on_windows(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end();
    let upper = stem.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let mut chars = upper.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: String = chars.collect();
    (prefix == "COM" || prefix == "LPT")
        && matches!(
            rest.as_str(),
            "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
}

/// Points et espaces de tête retirés (fichier caché, `..`) ; de fin aussi sous Windows,
/// qui les ignore.
fn trim(name: &str, rules: NameRules) -> &str {
    let name = name.trim_start_matches(|c: char| c == '.' || c.is_whitespace());
    match rules {
        NameRules::Windows => name.trim_end_matches(|c: char| c == '.' || c.is_whitespace()),
        NameRules::Unix => name.trim_end(),
    }
}

/// Coupe `text` à au plus `max` octets, sur une frontière de caractère.
fn truncate_at(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Raccourcit `name` à [`MAX_NAME_BYTES`], en gardant son extension si elle est courte.
fn shorten(name: &str, rules: NameRules) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_owned();
    }
    if let Some(dot) = name.rfind('.') {
        let ext = &name[dot..];
        if dot > 0 && ext.len() > 1 && ext.len() <= MAX_EXTENSION_BYTES {
            let stem = truncate_at(&name[..dot], MAX_NAME_BYTES - ext.len());
            let stem = trim(stem, rules);
            if !stem.is_empty() {
                return format!("{stem}{ext}");
            }
        }
    }
    trim(truncate_at(name, MAX_NAME_BYTES), rules).to_owned()
}

/// Nom sûr pour un fichier du dossier de destination, tiré de `raw` (F-09, SPEC §8.1).
pub fn sanitize(raw: &str, rules: NameRules) -> String {
    let replaced: String = raw
        .chars()
        .filter(|&c| !is_invisible(c))
        .map(|c| if is_forbidden(c, rules) { '_' } else { c })
        .collect();
    let mut name = trim(&replaced, rules).to_owned();
    if rules == NameRules::Windows && is_reserved_on_windows(&name) {
        name.insert(0, '_');
    }
    let name = shorten(&name, rules);
    if name.is_empty() {
        FALLBACK_NAME.to_owned()
    } else {
        name
    }
}

/// Dernier segment non vide du chemin de l'URL, décodé : `…/b%C3%A9b%C3%A9.iso` donne
/// `bébé.iso`. Pas encore nettoyé.
pub fn name_from_url(url: &Url) -> Option<String> {
    let segment = url.path_segments()?.rev().find(|s| !s.is_empty())?;
    Some(percent_decode_str(segment).decode_utf8_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::{Component, Path};

    const BOTH: [NameRules; 2] = [NameRules::Windows, NameRules::Unix];

    #[test]
    fn ordinary_names_are_kept() {
        for rules in BOTH {
            assert_eq!(sanitize("rapport 2026.pdf", rules), "rapport 2026.pdf");
            assert_eq!(sanitize("bébé.iso", rules), "bébé.iso");
        }
    }

    #[test]
    fn path_traversal_stays_in_the_folder() {
        for rules in BOTH {
            assert_eq!(sanitize("../../etc/passwd", rules), "_.._etc_passwd");
            assert_eq!(
                sanitize("..\\..\\Windows\\win.ini", rules),
                "_.._Windows_win.ini"
            );
            assert_eq!(sanitize("/etc/passwd", rules), "_etc_passwd");
            assert_eq!(sanitize("..", rules), FALLBACK_NAME);
            assert_eq!(sanitize(" . ", rules), FALLBACK_NAME);
            assert_eq!(sanitize("", rules), FALLBACK_NAME);
        }
    }

    #[test]
    fn hidden_files_are_not_created() {
        assert_eq!(sanitize(".bashrc", NameRules::Unix), "bashrc");
    }

    #[test]
    fn windows_specific_rules() {
        let w = NameRules::Windows;
        assert_eq!(sanitize("a<b>c:d\"e|f?g*h.txt", w), "a_b_c_d_e_f_g_h.txt");
        assert_eq!(sanitize("a<b>c:d.txt", NameRules::Unix), "a<b>c:d.txt");
        assert_eq!(sanitize("CON", w), "_CON");
        assert_eq!(sanitize("con.txt", w), "_con.txt");
        assert_eq!(sanitize("Com1.tar.gz", w), "_Com1.tar.gz");
        assert_eq!(sanitize("LPT²", w), "_LPT²");
        assert_eq!(sanitize("CONSOLE.txt", w), "CONSOLE.txt");
        assert_eq!(sanitize("COM10", w), "COM10");
        assert_eq!(sanitize("CON", NameRules::Unix), "CON");
        assert_eq!(sanitize("fin. . ", w), "fin");
    }

    #[test]
    fn control_and_bidi_characters_are_neutralised() {
        for rules in BOTH {
            assert_eq!(sanitize("a\nb\0c.txt", rules), "a_b_c.txt");
            assert_eq!(sanitize("facture\u{202E}fdp.exe", rules), "facturefdp.exe");
        }
    }

    #[test]
    fn long_names_keep_their_extension() {
        let long = format!("{}.tar.gz", "é".repeat(200));
        let name = sanitize(&long, NameRules::Unix);
        assert!(name.len() <= MAX_NAME_BYTES);
        assert!(name.ends_with("é.gz"), "{name}");
        let no_ext = "x".repeat(300);
        assert_eq!(sanitize(&no_ext, NameRules::Unix).len(), MAX_NAME_BYTES);
    }

    #[test]
    fn url_names() {
        let name = |u: &str| name_from_url(&Url::parse(u).unwrap());
        assert_eq!(
            name("https://h/a/b%C3%A9b%C3%A9.iso?x=1"),
            Some("bébé.iso".into())
        );
        assert_eq!(name("https://h/dossier/"), Some("dossier".into()));
        assert_eq!(name("https://h/"), None);
        assert_eq!(
            name("https://h/a/%2E%2E%2Fx"),
            Some("../x".into()),
            "nettoyé plus tard"
        );
    }

    proptest! {
        /// Quel que soit le nom reçu : un seul composant normal, dans la limite de
        /// longueur, et un second nettoyage ne change plus rien.
        #[test]
        fn sanitized_names_are_single_safe_components(raw in any::<String>()) {
            for rules in BOTH {
                let name = sanitize(&raw, rules);
                prop_assert!(!name.is_empty() && name.len() <= MAX_NAME_BYTES);
                prop_assert!(!name.contains(['/', '\\']));
                // Le découpage en composants dépend du système qui exécute le test.
                if rules == NameRules::native() {
                    let mut parts = Path::new(&name).components();
                    prop_assert!(matches!(parts.next(), Some(Component::Normal(_))), "{name:?}");
                    prop_assert!(parts.next().is_none(), "{name:?}");
                }
                if rules == NameRules::Windows {
                    prop_assert!(!is_reserved_on_windows(&name), "{name:?}");
                    prop_assert!(!name.ends_with(['.', ' ']), "{name:?}");
                }
                prop_assert_eq!(sanitize(&name, rules), name.clone());
            }
        }
    }
}
