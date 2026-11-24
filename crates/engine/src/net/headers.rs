//! Lecture des en-têtes HTTP utiles à la préparation d'une tâche (SPEC §7.1), sans réseau.
//!
//! Chaque fonction refuse une valeur mal formée (`None`) plutôt que de deviner : une
//! taille ou une plage fausse ferait dériver les décalages d'octets.

use std::time::{Duration, SystemTime};

use percent_encoding::percent_decode;

/// Entier décimal sans signe ni espace ; `None` en cas de dépassement.
fn parse_u64(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Une taille ou une position au-delà d'`i64::MAX` ne s'enregistre pas en base : refusée
    // dès la lecture, comme une valeur illisible (audit S3).
    text.parse()
        .ok()
        .filter(|&n: &u64| i64::try_from(n).is_ok())
}

/// `Content-Length`.
pub fn content_length(value: &str) -> Option<u64> {
    parse_u64(value.trim())
}

/// `Content-Range` d'une réponse 206 ou 416 (RFC 9110 §14.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRange {
    /// Plage envoyée, bornes incluses ; `None` pour `bytes */taille` (réponse 416).
    pub range: Option<(u64, u64)>,
    /// Taille complète ; `None` pour `bytes début-fin/*`.
    pub total: Option<u64>,
}

/// Lit `bytes début-fin/taille`, `bytes début-fin/*` ou `bytes */taille`.
pub fn content_range(value: &str) -> Option<ContentRange> {
    let (unit, spec) = value.trim().split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, total) = spec.trim().split_once('/')?;
    let total = match total {
        "*" => None,
        t => Some(parse_u64(t)?),
    };
    let range = match range {
        "*" => {
            // Sans plage, la taille est obligatoire.
            total?;
            None
        }
        r => {
            let (first, last) = r.split_once('-')?;
            let (first, last) = (parse_u64(first)?, parse_u64(last)?);
            if first > last || total.is_some_and(|t| last >= t) {
                return None;
            }
            Some((first, last))
        }
    };
    Some(ContentRange { range, total })
}

/// `Accept-Ranges` : `Some(true)` si l'unité `bytes` est annoncée, `Some(false)` pour
/// `none`, `None` si l'en-tête ne dit rien d'utilisable.
pub fn accept_ranges(value: &str) -> Option<bool> {
    let mut units = value.split(',').map(str::trim).filter(|u| !u.is_empty());
    if units.clone().any(|u| u.eq_ignore_ascii_case("bytes")) {
        Some(true)
    } else if units.all(|u| u.eq_ignore_ascii_case("none")) && !value.trim().is_empty() {
        Some(false)
    } else {
        None
    }
}

/// `Content-Encoding` qui transforme le corps : la taille et les plages portent alors
/// sur les octets transformés, pas sur le fichier.
pub fn is_encoded(content_encoding: &str) -> bool {
    content_encoding
        .split(',')
        .map(str::trim)
        .any(|e| !e.is_empty() && !e.eq_ignore_ascii_case("identity"))
}

/// Un `ETag` fort (sans `W/`) : seul utilisable avec `If-Range` (RFC 9110 §13.1.5).
#[cfg(test)]
pub fn is_strong_etag(etag: &str) -> bool {
    !etag.trim_start().starts_with("W/")
}

/// Validateur à envoyer dans `If-Range` (SPEC §7.4) : l'`ETag` fort et bien formé (entre
/// guillemets), sinon la date `Last-Modified`, sinon l'`ETag` sans guillemets qu'on y met.
///
/// Certains serveurs envoient une `ETag` sans guillemets (`upload.wikimedia.org`, essai du
/// 2026-11-09) mais n'acceptent dans `If-Range` qu'une étiquette conforme : renvoyée telle
/// quelle, elle leur fait rendre le fichier entier (`200`), pris pour un fichier changé.
pub fn if_range_validator(etag: Option<&str>, last_modified: Option<&str>) -> Option<String> {
    let etag = etag.map(str::trim).filter(|e| !e.is_empty());
    let quoted = |e: &&str| e.len() >= 2 && e.starts_with('"') && e.ends_with('"');
    if let Some(e) = etag.filter(quoted) {
        return Some(e.to_owned());
    }
    if let Some(date) = last_modified.map(str::trim).filter(|d| !d.is_empty()) {
        return Some(date.to_owned());
    }
    etag.filter(|e| {
        !e.starts_with("W/")
            && e.bytes()
                .all(|b| b != b'"' && (b == 0x21 || (0x23..=0x7e).contains(&b) || b >= 0x80))
    })
    .map(|e| format!("\"{e}\""))
}

/// `Retry-After` : nombre de secondes ou date HTTP, ramené à un délai depuis `now`.
pub fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Some(seconds) = parse_u64(value) {
        return Some(Duration::from_secs(seconds));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(now).unwrap_or(Duration::ZERO))
}

/// Texte d'un en-tête : UTF-8 s'il est valide, sinon ISO-8859-1, l'ancien usage HTTP.
pub fn header_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

/// Nom de fichier proposé par `Content-Disposition` (RFC 6266), avant tout nettoyage :
/// `filename*` (encodé, RFC 8187) prime sur `filename`.
pub fn content_disposition_filename(value: &[u8]) -> Option<String> {
    let text = header_text(value);
    let mut plain = None;
    let mut extended = None;
    // Le premier élément est le type (`attachment`, `inline`) : ignoré.
    for param in split_params(&text).into_iter().skip(1) {
        let Some((name, raw)) = param.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.eq_ignore_ascii_case("filename*") {
            extended = extended.or_else(|| decode_ext_value(raw.trim()));
        } else if name.eq_ignore_ascii_case("filename") {
            plain = plain.or_else(|| Some(unquote(raw.trim())));
        }
    }
    extended.or(plain).filter(|name| !name.is_empty())
}

/// Découpe aux `;` qui ne sont pas dans une chaîne entre guillemets.
fn split_params(text: &str) -> Vec<&str> {
    let mut params = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, c) in text.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                params.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    params.push(&text[start..]);
    params
}

/// Chaîne entre guillemets (avec `\` d'échappement) ou jeton nu.
fn unquote(raw: &str) -> String {
    let Some(inner) = raw.strip_prefix('"') else {
        return raw.to_owned();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '"' => break,
            c => out.push(c),
        }
    }
    out
}

/// Valeur étendue `jeu'langue'octets-encodés` (RFC 8187) : UTF-8 ou ISO-8859-1.
fn decode_ext_value(raw: &str) -> Option<String> {
    let mut parts = raw.splitn(3, '\'');
    let (charset, _lang, encoded) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes: Vec<u8> = percent_decode(encoded.as_bytes()).collect();
    if charset.eq_ignore_ascii_case("utf-8") {
        String::from_utf8(bytes).ok()
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(bytes.iter().map(|&b| char::from(b)).collect())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_forms() {
        let cr = |range, total| Some(ContentRange { range, total });
        assert_eq!(
            content_range("bytes 0-0/5000"),
            cr(Some((0, 0)), Some(5000))
        );
        assert_eq!(content_range("Bytes 10-19/*"), cr(Some((10, 19)), None));
        assert_eq!(content_range("bytes */0"), cr(None, Some(0)));
        for bad in [
            "bytes */*",
            "bytes 5-4/10",
            "bytes 0-10/10",
            "bytes -1-3/10",
            "bytes 0-0",
            "items 0-0/10",
            "bytes 0-0/18446744073709551616",
            "bytes 0 - 0/10",
            "",
        ] {
            assert_eq!(content_range(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn content_length_is_strict() {
        assert_eq!(content_length(" 42 "), Some(42));
        assert_eq!(content_length("-1"), None);
        assert_eq!(content_length("4 2"), None);
        assert_eq!(content_length("+42"), None);
        // Audit S3 : au-delà d'`i64::MAX`, la base ne pourrait pas l'enregistrer.
        assert_eq!(
            content_length("9223372036854775807"),
            Some(9_223_372_036_854_775_807)
        );
        assert_eq!(content_length("9223372036854775808"), None);
        assert_eq!(content_range("bytes 0-0/18446744073709551615"), None);
    }

    #[test]
    fn accept_ranges_values() {
        assert_eq!(accept_ranges("bytes"), Some(true));
        assert_eq!(accept_ranges("none, BYTES"), Some(true));
        assert_eq!(accept_ranges("none"), Some(false));
        assert_eq!(accept_ranges("items"), None);
        assert_eq!(accept_ranges(""), None);
    }

    #[test]
    fn encodings_and_etags() {
        assert!(is_encoded("gzip"));
        assert!(is_encoded("identity, br"));
        assert!(!is_encoded("identity"));
        assert!(!is_encoded(""));
        assert!(is_strong_etag("\"abc\""));
        assert!(!is_strong_etag("W/\"abc\""));
    }

    #[test]
    fn the_if_range_validator_is_a_well_formed_etag_or_the_date() {
        let date = Some("Sat, 03 Oct 2020 17:29:25 GMT");
        // ETag conforme : elle d'abord.
        assert_eq!(
            if_range_validator(Some("\"v1\""), date).as_deref(),
            Some("\"v1\"")
        );
        // Faible : la date, sinon rien.
        assert_eq!(if_range_validator(Some("W/\"v1\""), date).as_deref(), date);
        assert_eq!(if_range_validator(Some("W/\"v1\""), None), None);
        // Sans guillemets (upload.wikimedia.org) : la date, sinon l'ETag mise entre
        // guillemets.
        assert_eq!(
            if_range_validator(Some("d087f893a1db4db2"), date).as_deref(),
            date
        );
        assert_eq!(
            if_range_validator(Some(" d087f893a1db4db2 "), None).as_deref(),
            Some("\"d087f893a1db4db2\"")
        );
        // Mal formée au point de ne pas pouvoir être citée : rien.
        assert_eq!(if_range_validator(Some("a\"b"), None), None);
        assert_eq!(if_range_validator(Some("a b"), None), None);
        assert_eq!(if_range_validator(Some(""), None), None);
        assert_eq!(if_range_validator(None, None), None);
    }

    #[test]
    fn retry_after_seconds_and_dates() {
        let now = httpdate::parse_http_date("Wed, 21 Oct 2026 07:28:00 GMT").unwrap();
        assert_eq!(retry_after("120", now), Some(Duration::from_secs(120)));
        assert_eq!(
            retry_after("Wed, 21 Oct 2026 07:30:00 GMT", now),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            retry_after("Wed, 21 Oct 2026 07:00:00 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(retry_after("demain", now), None);
    }

    #[test]
    fn disposition_filenames() {
        let name = |v: &str| content_disposition_filename(v.as_bytes());
        assert_eq!(
            name("attachment; filename=\"rapport annuel.pdf\""),
            Some("rapport annuel.pdf".into())
        );
        assert_eq!(name("attachment; filename=a.zip"), Some("a.zip".into()));
        assert_eq!(
            name("attachment; filename=\"a;b \\\"c\\\".txt\"; size=3"),
            Some("a;b \"c\".txt".into())
        );
        assert_eq!(
            name("attachment; filename=\"fallback.txt\"; filename*=UTF-8''b%C3%A9b%C3%A9.txt"),
            Some("bébé.txt".into())
        );
        assert_eq!(
            name("attachment; FILENAME*=iso-8859-1'fr'caf%E9.txt"),
            Some("café.txt".into())
        );
        // Jeu de caractères inconnu : repli sur `filename`.
        assert_eq!(
            name("attachment; filename*=koi8-r''%C1.txt; filename=x.txt"),
            Some("x.txt".into())
        );
        assert_eq!(name("inline"), None);
        assert_eq!(name("attachment; filename=\"\""), None);
    }

    #[test]
    fn non_utf8_header_is_read_as_latin1() {
        assert_eq!(
            content_disposition_filename(b"attachment; filename=\"caf\xe9.txt\""),
            Some("café.txt".into())
        );
    }
}
