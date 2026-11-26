//! Styles d'une copie de site (SPEC §7.18 ter) : les adresses `url(…)` et `@import` d'une
//! feuille de style, d'un bloc `<style>` ou d'un attribut `style`, et leur réécriture.
//!
//! Un petit lecteur de jetons, sans arbre : commentaires et chaînes sautés, `url(…)` lu
//! entier (guillemets ou non), `@import` suivi d'une chaîne ou d'un `url(…)`. Seul le jeton
//! d'une adresse change à la réécriture ; le reste est gardé octet pour octet.

use std::ops::Range;

/// Une adresse d'un style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssRef {
    /// Le jeton entier : `url(…)`, ou la chaîne d'un `@import`.
    pub span: Range<usize>,
    /// L'adresse, échappements CSS lus.
    pub url: String,
    /// Feuille importée (`@import`) : un style, pas une ressource.
    pub import: bool,
    /// Le jeton est une chaîne (`@import "a.css"`), pas un `url(…)`.
    string: bool,
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b >= 0x80
}

fn starts_with_ignore_case(bytes: &[u8], at: usize, word: &[u8]) -> bool {
    bytes
        .get(at..at + word.len())
        .is_some_and(|s| s.eq_ignore_ascii_case(word))
}

fn skip_spaces(css: &[u8], mut i: usize) -> usize {
    while css.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// Une chaîne qui commence à `start` (son guillemet) : sa fin (après le guillemet fermant,
/// ou la fin de ligne d'une chaîne non fermée) et son contenu brut.
fn string_at(css: &[u8], start: usize) -> (usize, Range<usize>) {
    let quote = css[start];
    let mut i = start + 1;
    while let Some(&b) = css.get(i) {
        match b {
            b'\\' => i += 2,
            b'\n' | b'\r' | b'\x0c' => return (i, start + 1..i),
            _ if b == quote => return (i + 1, start + 1..i),
            _ => i += 1,
        }
    }
    let end = css.len();
    (end, start + 1..end.max(start + 1))
}

/// Les échappements CSS d'un texte : `\` suivi de 1 à 6 chiffres hexadécimaux (et d'une
/// espace facultative), ou d'un autre caractère gardé tel quel ; `\` en fin de ligne
/// disparaît.
fn unescape(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    if !text.contains('\\') {
        return text.into_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 && chars.peek().is_some_and(char::is_ascii_hexdigit) {
            hex.push(chars.next().unwrap_or('0'));
        }
        if hex.is_empty() {
            match chars.next() {
                Some('\n') | None => {}
                Some(other) => out.push(other),
            }
            continue;
        }
        if chars.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            chars.next();
        }
        let code = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
        out.push(
            char::from_u32(code)
                .filter(|&c| c != '\0')
                .unwrap_or('\u{FFFD}'),
        );
    }
    out
}

/// `url(` commence à `start` : la fin du jeton (après `)`) et l'adresse, ou `None` si ce
/// n'est pas un `url(…)` bien formé.
fn url_at(css: &[u8], start: usize) -> Option<(usize, String)> {
    let mut i = skip_spaces(css, start + 4);
    let url = match css.get(i)? {
        b'"' | b'\'' => {
            let (end, content) = string_at(css, i);
            i = skip_spaces(css, end);
            unescape(&css[content])
        }
        _ => {
            let from = i;
            while let Some(&b) = css.get(i) {
                match b {
                    b')' => break,
                    b'\\' => {
                        // `\66 ` : jusqu'à 6 chiffres hexadécimaux et une espace.
                        i += 1;
                        let digits = css[i..]
                            .iter()
                            .take(6)
                            .take_while(|b| b.is_ascii_hexdigit())
                            .count();
                        if digits == 0 {
                            i += 1;
                        } else {
                            i += digits;
                            if css.get(i).is_some_and(u8::is_ascii_whitespace) {
                                i += 1;
                            }
                        }
                    }
                    b'"' | b'\'' | b'(' => return None,
                    _ if b.is_ascii_whitespace() => break,
                    _ => i += 1,
                }
            }
            let raw = &css[from..i.min(css.len())];
            i = skip_spaces(css, i);
            unescape(raw)
        }
    };
    (css.get(i) == Some(&b')')).then(|| (i + 1, url))
}

/// Les adresses d'un style, dans l'ordre.
pub fn refs(css: &[u8]) -> Vec<CssRef> {
    let mut found = Vec::new();
    let mut i = 0;
    let mut import = false;
    while i < css.len() {
        let b = css[i];
        if b == b'/' && css.get(i + 1) == Some(&b'*') {
            i = match find(css, i + 2, b"*/") {
                Some(end) => end + 2,
                None => css.len(),
            };
            continue;
        }
        if b == b'"' || b == b'\'' {
            let (end, content) = string_at(css, i);
            if std::mem::take(&mut import) {
                found.push(CssRef {
                    span: i..end,
                    url: unescape(&css[content]),
                    import: true,
                    string: true,
                });
            }
            i = end;
            continue;
        }
        if b == b'@'
            && starts_with_ignore_case(css, i + 1, b"import")
            && !css.get(i + 7).copied().is_some_and(is_ident)
        {
            import = true;
            i += 7;
            continue;
        }
        if (b == b'u' || b == b'U')
            && starts_with_ignore_case(css, i, b"url(")
            && (i == 0 || !is_ident(css[i - 1]))
        {
            if let Some((end, url)) = url_at(css, i) {
                found.push(CssRef {
                    span: i..end,
                    url,
                    import: std::mem::take(&mut import),
                    string: false,
                });
                i = end;
                continue;
            }
            i += 4;
            continue;
        }
        if matches!(b, b';' | b'{' | b'}') {
            import = false;
        }
        i += 1;
    }
    found
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// Une adresse dans une chaîne CSS entre guillemets doubles.
fn quoted(url: &str) -> String {
    let mut out = String::with_capacity(url.len() + 2);
    out.push('"');
    for c in url.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\a "),
            '\r' => out.push_str("\\d "),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Le style avec chaque adresse que `map` remplace (`map(adresse, import)` rend la
/// nouvelle, ou `None` pour la garder) ; le reste est gardé octet pour octet.
pub fn rewrite(css: &[u8], map: &mut dyn FnMut(&str, bool) -> Option<String>) -> Vec<u8> {
    let mut out = Vec::with_capacity(css.len() + 64);
    let mut at = 0;
    for r in refs(css) {
        let Some(new) = map(&r.url, r.import) else {
            continue;
        };
        out.extend_from_slice(&css[at..r.span.start]);
        let token = if r.string {
            quoted(&new)
        } else {
            format!("url({})", quoted(&new))
        };
        out.extend_from_slice(token.as_bytes());
        at = r.span.end;
    }
    out.extend_from_slice(&css[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn urls(css: &str) -> Vec<(String, bool)> {
        refs(css.as_bytes())
            .into_iter()
            .map(|r| (r.url, r.import))
            .collect()
    }

    #[test]
    fn urls_and_imports_are_found_in_every_form() {
        let css = r#"
            @import "base.css";
            @import url('print.css') print;
            @IMPORT url(ecran.css);
            /* url(commentaire.png) */
            body { background: URL( fond.png ) no-repeat; }
            .a { background-image: url("a\"b.png"), url('c d.png'); }
            @font-face { src: url(police.woff2) format("woff2"), url(police.woff); }
            .b { content: "url(pas-une-adresse.png)"; }
            .c { mask: url(#masque); }
            .d { background: myurl(x.png); }
            .e { background: url(\66 ichier.png); }
        "#;
        assert_eq!(
            urls(css),
            vec![
                ("base.css".into(), true),
                ("print.css".into(), true),
                ("ecran.css".into(), true),
                ("fond.png".into(), false),
                ("a\"b.png".into(), false),
                ("c d.png".into(), false),
                ("police.woff2".into(), false),
                ("police.woff".into(), false),
                ("#masque".into(), false),
                ("fichier.png".into(), false),
            ]
        );
    }

    #[test]
    fn a_malformed_url_is_left_alone() {
        assert!(urls("a { b: url(x y.png) }").is_empty());
        assert!(urls("a { b: url(x\"y) }").is_empty());
        assert!(urls("a { b: url(").is_empty());
        assert_eq!(urls("a { b: url('x.png'"), vec![]);
        // Une chaîne non fermée finit à sa ligne.
        assert_eq!(
            urls("a { content: \"x\n} b { c: url(y.png) }"),
            vec![("y.png".into(), false)]
        );
    }

    #[test]
    fn only_the_tokens_of_the_mapped_urls_change() {
        let css = "@import 'a.css';\nb { background: url( b.png ) } c { d: url(\"c.png\") }";
        let out = rewrite(css.as_bytes(), &mut |url, import| match url {
            "a.css" => {
                assert!(import);
                Some("../x/a.css".into())
            }
            "b.png" => Some("https://exemple.org/b.png?a=\"1\"".into()),
            _ => None,
        });
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "@import \"../x/a.css\";\nb { background: url(\"https://exemple.org/b.png?a=\\\"1\\\"\") } c { d: url(\"c.png\") }"
        );
    }

    #[test]
    fn non_utf8_bytes_outside_the_tokens_are_kept() {
        let css = b"/* caf\xe9 */ a { b: url(x.png) } /* \xff */".to_vec();
        let out = rewrite(&css, &mut |_, _| Some("y.png".into()));
        let start = css.windows(4).position(|w| w == b"url(").unwrap();
        let end = start + "url(x.png)".len();
        let expected = [&css[..start], b"url(\"y.png\")", &css[end..]].concat();
        assert_eq!(out, expected);
    }

    proptest! {
        /// Quel que soit le style : la lecture ne panique pas, chaque jeton est dans le
        /// texte, dans l'ordre, sans chevauchement ; une réécriture qui ne change rien
        /// rend le texte tel quel.
        #[test]
        fn any_style_is_read_safely(css in proptest::collection::vec(any::<u8>(), 0..400)) {
            let found = refs(&css);
            let mut last = 0;
            for r in &found {
                prop_assert!(r.span.start >= last && r.span.end <= css.len());
                prop_assert!(r.span.start < r.span.end);
                last = r.span.end;
            }
            prop_assert_eq!(rewrite(&css, &mut |_, _| None), css);
        }

        /// Réécrire avec la même adresse rend un style qui se relit pareil.
        #[test]
        fn a_rewritten_style_reads_back_the_same_urls(
            parts in proptest::collection::vec("[a-z]{1,8}(\\.png|\\.css)?", 1..6),
        ) {
            let css: String = parts.iter().map(|p| format!("a {{ b: url('{p}') }} ")).collect();
            let out = rewrite(css.as_bytes(), &mut |u, _| Some(format!("x/{u}")));
            let back: Vec<String> = refs(&out).into_iter().map(|r| r.url).collect();
            let expected: Vec<String> = parts.iter().map(|p| format!("x/{p}")).collect();
            prop_assert_eq!(back, expected);
        }
    }
}
