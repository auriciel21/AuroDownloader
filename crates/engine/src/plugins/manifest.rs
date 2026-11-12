//! Manifeste d'un plugin (SPEC §7.20) : la section personnalisée `auro-plugin` du fichier
//! `.wasm`, en JSON. Lu ici sans le moteur WebAssembly (en-tête et sections seulement),
//! puis validé : identifiant, textes, domaines.

use serde_json::Value;

/// Nom de la section personnalisée qui porte le manifeste.
pub const SECTION: &str = "auro-plugin";
/// Version de l'interface d'un plugin que ce moteur sait servir.
pub const ABI: u64 = 1;
/// Taille d'un fichier de plugin, au plus.
pub const MAX_FILE: usize = 8 << 20;
/// Domaines de `sites` ou de `fetch`, au plus.
pub const MAX_DOMAINS: usize = 16;
/// Longueur d'un texte du manifeste (nom, version, auteur, description), au plus.
pub const MAX_TEXT: usize = 200;

/// Manifeste validé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    /// Domaines des pages prises en charge (un domaine vaut aussi pour ses sous-domaines).
    pub sites: Vec<String>,
    /// Domaines que le plugin peut faire demander au moteur.
    pub fetch: Vec<String>,
}

/// Un entier LEB128 non signé de 32 bits au plus, à `*at` ; avance `at`.
fn leb_u32(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let mut value: u32 = 0;
    for shift in (0..35).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        let part = u32::from(byte & 0x7f);
        if shift == 28 && part > 0x0f {
            return None;
        }
        value |= part << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Le contenu de la section personnalisée `name`, si le fichier en a une seule.
fn custom_section<'a>(bytes: &'a [u8], name: &str) -> Result<&'a [u8], String> {
    if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
        return Err("ce n'est pas un fichier WebAssembly".into());
    }
    if bytes[4..8] != [1, 0, 0, 0] {
        return Err("version de WebAssembly non prise en charge (module de base 1 attendu)".into());
    }
    let mut at = 8;
    let mut found = None;
    while at < bytes.len() {
        let id = bytes[at];
        at += 1;
        let size = leb_u32(bytes, &mut at).ok_or("section illisible")? as usize;
        let end = at.checked_add(size).filter(|end| *end <= bytes.len());
        let end = end.ok_or("section qui dépasse la fin du fichier")?;
        if id == 0 {
            let mut inner = at;
            let len =
                leb_u32(&bytes[..end], &mut inner).ok_or("nom de section illisible")? as usize;
            let name_end = inner
                .checked_add(len)
                .filter(|e| *e <= end)
                .ok_or("nom de section qui dépasse sa section")?;
            if &bytes[inner..name_end] == name.as_bytes() {
                if found.is_some() {
                    return Err(format!("plusieurs sections {name}"));
                }
                found = Some(&bytes[name_end..end]);
            }
        }
        at = end;
    }
    found.ok_or_else(|| format!("pas de manifeste (section {name})"))
}

/// Un nom de domaine du manifeste : minuscules, au moins deux étiquettes, pas une adresse
/// IP, pas de joker ni de point final.
pub fn valid_domain(name: &str) -> bool {
    if name.is_empty() || name.len() > 253 || name.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    let labels: Vec<&str> = name.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        // La dernière étiquette n'est pas un nombre (forme partielle d'une adresse IPv4).
        && !labels[labels.len() - 1].bytes().all(|b| b.is_ascii_digit())
}

/// `host` est-il `domain` ou l'un de ses sous-domaines ?
pub fn domain_covers(domain: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn text(value: &Value, key: &str, required: bool) -> Result<Option<String>, String> {
    match value.get(key) {
        None | Some(Value::Null) if !required => Ok(None),
        None | Some(Value::Null) => Err(format!("manifeste : {key} manquant")),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() && required {
                return Err(format!("manifeste : {key} vide"));
            }
            if s.chars().count() > MAX_TEXT || s.chars().any(char::is_control) {
                return Err(format!(
                    "manifeste : {key} de {MAX_TEXT} caractères au plus, sans caractère de contrôle"
                ));
            }
            Ok((!s.is_empty()).then(|| s.to_owned()))
        }
        Some(_) => Err(format!("manifeste : {key} doit être un texte")),
    }
}

fn domains(value: &Value, key: &str) -> Result<Vec<String>, String> {
    let list = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("manifeste : {key} doit être une liste de domaines"))?;
    if list.is_empty() || list.len() > MAX_DOMAINS {
        return Err(format!("manifeste : {key} de 1 à {MAX_DOMAINS} domaines"));
    }
    let mut out: Vec<String> = Vec::new();
    for item in list {
        let name = item
            .as_str()
            .ok_or_else(|| format!("manifeste : {key} : texte attendu"))?;
        if !valid_domain(name) {
            return Err(format!(
                "manifeste : {key} : nom de domaine invalide : {name:?}"
            ));
        }
        if !out.iter().any(|d| d == name) {
            out.push(name.to_owned());
        }
    }
    Ok(out)
}

/// Le manifeste d'un fichier de plugin, lu et validé.
pub fn read(bytes: &[u8]) -> Result<Manifest, String> {
    if bytes.len() > MAX_FILE {
        return Err(format!(
            "fichier de plugin de {} Mio au plus",
            MAX_FILE >> 20
        ));
    }
    let raw = custom_section(bytes, SECTION)?;
    let value: Value =
        serde_json::from_slice(raw).map_err(|_| "manifeste : JSON illisible".to_owned())?;
    if !value.is_object() {
        return Err("manifeste : objet JSON attendu".into());
    }
    match value.get("abi").and_then(Value::as_u64) {
        Some(ABI) => {}
        Some(other) => {
            return Err(format!(
                "interface {other} non prise en charge (version {ABI})"
            ));
        }
        None => return Err("manifeste : abi manquant".into()),
    }
    let id = text(&value, "id", true)?.unwrap_or_default();
    let id_ok = (3..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && !id.starts_with(['.', '-'])
        && !id.ends_with(['.', '-']);
    if !id_ok {
        return Err("manifeste : id de 3 à 64 caractères a-z, 0-9, « . » et « - »".into());
    }
    Ok(Manifest {
        id,
        name: text(&value, "name", true)?.unwrap_or_default(),
        version: text(&value, "version", true)?.unwrap_or_default(),
        author: text(&value, "author", false)?,
        description: text(&value, "description", false)?,
        sites: domains(&value, "sites")?,
        fetch: domains(&value, "fetch")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un module vide avec ces sections personnalisées.
    fn module(sections: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = b"\0asm\x01\0\0\0".to_vec();
        for (name, payload) in sections {
            let mut body = vec![name.len() as u8];
            body.extend_from_slice(name.as_bytes());
            body.extend_from_slice(payload);
            out.push(0);
            let mut size = body.len();
            loop {
                let byte = (size & 0x7f) as u8;
                size >>= 7;
                out.push(if size > 0 { byte | 0x80 } else { byte });
                if size == 0 {
                    break;
                }
            }
            out.extend(body);
        }
        out
    }

    const GOOD: &str = r#"{"abi": 1, "id": "org.exemple.fichiers", "name": "Fichiers",
        "version": "1.0", "author": "Moi", "sites": ["fichiers.exemple"],
        "fetch": ["fichiers.exemple", "cdn.fichiers.exemple", "fichiers.exemple"]}"#;

    #[test]
    fn a_manifest_is_read_from_its_section() {
        let bytes = module(&[("autre", b"x"), (SECTION, GOOD.as_bytes())]);
        let m = read(&bytes).unwrap();
        assert_eq!(m.id, "org.exemple.fichiers");
        assert_eq!(m.author.as_deref(), Some("Moi"));
        assert_eq!(m.description, None);
        // Doublons retirés.
        assert_eq!(m.fetch, ["fichiers.exemple", "cdn.fichiers.exemple"]);
    }

    #[test]
    fn bad_files_and_manifests_are_refused() {
        assert!(read(b"pas du wasm").is_err());
        assert!(read(&module(&[])).unwrap_err().contains("pas de manifeste"));
        let twice = module(&[(SECTION, GOOD.as_bytes()), (SECTION, GOOD.as_bytes())]);
        assert!(read(&twice).unwrap_err().contains("plusieurs"));
        let mut cut = module(&[(SECTION, GOOD.as_bytes())]);
        cut.truncate(cut.len() - 5);
        assert!(read(&cut).is_err());
        for bad in [
            r#"{"abi": 2, "id": "a.b.c", "name": "n", "version": "1", "sites": ["a.b"], "fetch": ["a.b"]}"#,
            r#"{"abi": 1, "id": "A.B", "name": "n", "version": "1", "sites": ["a.b"], "fetch": ["a.b"]}"#,
            r#"{"abi": 1, "id": "a.b.c", "name": "", "version": "1", "sites": ["a.b"], "fetch": ["a.b"]}"#,
            r#"{"abi": 1, "id": "a.b.c", "name": "n", "version": "1", "sites": [], "fetch": ["a.b"]}"#,
            r#"{"abi": 1, "id": "a.b.c", "name": "n", "version": "1", "sites": ["*.a.b"], "fetch": ["a.b"]}"#,
            r#"{"abi": 1, "id": "a.b.c", "name": "n", "version": "1", "sites": ["a.b"], "fetch": ["127.0.0.1"]}"#,
            r#"{"abi": 1, "id": "a.b.c", "name": "n", "version": "1", "sites": ["a.b"], "fetch": ["localhost"]}"#,
            r#"{"abi": 1, "id": "a.b.c", "name": "n\u0007", "version": "1", "sites": ["a.b"], "fetch": ["a.b"]}"#,
            r#"[1]"#,
        ] {
            assert!(
                read(&module(&[(SECTION, bad.as_bytes())])).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn domains_cover_their_subdomains_only() {
        assert!(domain_covers("exemple.org", "exemple.org"));
        assert!(domain_covers("exemple.org", "dl.EXEMPLE.org."));
        assert!(!domain_covers("exemple.org", "autreexemple.org"));
        assert!(!domain_covers("exemple.org", "exemple.org.evil.com"));
        assert!(valid_domain("cdn-1.exemple.org"));
        for bad in [
            "exemple", "-a.org", "a..org", "1.2.3.4", "a.123", "A.org", "a.org.",
        ] {
            assert!(!valid_domain(bad), "{bad}");
        }
    }
}
