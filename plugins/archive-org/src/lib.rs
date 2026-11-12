//! Plugin d'exemple d'AuroDownloader (SPEC §7.20, étape 4d) : les fichiers d'un élément
//! d'Internet Archive.
//!
//! Une page `https://archive.org/details/<élément>` (ou `/download/<élément>`) donne les
//! fichiers d'origine de l'élément, lus par l'interface publique de métadonnées
//! (`https://archive.org/metadata/<élément>`), avec leur taille ; une adresse
//! `/download/<élément>/<fichier>` donne ce seul fichier. Le moteur télécharge ensuite
//! `https://archive.org/download/<élément>/<fichier>` lui-même.
//!
//! Le manifeste (`manifest.json`) est rangé dans la section personnalisée `auro-plugin` du
//! fichier `.wasm` par la compilation même : rien à ajouter après.

pub mod auro;

use auro::{Link, Response};
use serde_json::Value;

/// Manifeste du plugin, dans sa section personnalisée.
#[cfg(target_arch = "wasm32")]
#[unsafe(link_section = "auro-plugin")]
#[used]
static MANIFEST: [u8; include_bytes!("../manifest.json").len()] =
    *include_bytes!("../manifest.json");

/// Point d'entrée appelé par le moteur.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn auro_resolve(ptr: i32, len: i32) -> i64 {
    auro::run(ptr, len, |page| {
        resolve(page, |url| auro::get(url, "application/json"))
    })
}

/// Liens rendus au plus (ceux du moteur).
const MAX_LINKS: usize = 1000;

/// Ce que désigne une adresse d'archive.org.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    /// Identifiant de l'élément.
    pub item: String,
    /// Fichier visé (`/download/<élément>/<fichier>`), sinon tous ceux de l'élément.
    pub file: Option<String>,
}

/// Décodage des `%XX` d'un chemin ; `None` s'il est mal formé ou n'est pas de l'UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Encodage d'un chemin de fichier pour une adresse : chaque segment, `/` gardés.
fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Un identifiant d'élément : lettres, chiffres, `.`, `-`, `_`, 100 au plus.
fn valid_item(item: &str) -> bool {
    !item.is_empty()
        && item.len() <= 100
        && !item.starts_with('.')
        && item
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// L'élément (et le fichier) d'une adresse d'archive.org.
pub fn target(page: &str) -> Result<Target, String> {
    let rest = page
        .strip_prefix("https://")
        .or_else(|| page.strip_prefix("http://"))
        .ok_or("adresse http(s) attendue")?;
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    if host != "archive.org" && !host.ends_with(".archive.org") {
        return Err("adresse d'archive.org attendue".into());
    }
    let mut parts = path.splitn(3, '/');
    let kind = parts.next().unwrap_or_default();
    let item = parts.next().unwrap_or_default();
    let file = parts.next().filter(|f| !f.is_empty() && !f.ends_with('/'));
    if !matches!(kind, "details" | "download") || !valid_item(item) {
        return Err("page d'un élément attendue : https://archive.org/details/<élément>".into());
    }
    let file = match (kind, file) {
        ("download", Some(file)) => {
            Some(percent_decode(file).ok_or("chemin de fichier mal encodé")?)
        }
        _ => None,
    };
    Ok(Target {
        item: item.to_owned(),
        file,
    })
}

/// Un fichier de l'élément à proposer : d'origine, ni métadonnées, ni vignette, ni
/// `.torrent` de l'élément, ni réservé.
fn offered(file: &Value) -> bool {
    file["source"] == "original"
        && !matches!(
            file["format"].as_str(),
            Some("Metadata" | "Item Tile" | "Archive BitTorrent")
        )
        && file["private"] != "true"
}

/// Les liens des fichiers d'un élément, d'après ses métadonnées.
pub fn links_of(target: &Target, page: &str, metadata: &Value) -> Result<Vec<Link>, String> {
    let files = metadata["files"].as_array().filter(|f| !f.is_empty());
    let Some(files) = files else {
        return Err(if metadata.get("is_dark").is_some() {
            format!("élément {} retiré ou réservé", target.item)
        } else {
            format!("élément {} introuvable", target.item)
        });
    };
    let mut links = Vec::new();
    for file in files {
        let Some(name) = file["name"].as_str() else {
            continue;
        };
        let wanted = match &target.file {
            Some(only) => name == only,
            None => offered(file),
        };
        if !wanted {
            continue;
        }
        links.push(Link {
            url: format!(
                "https://archive.org/download/{}/{}",
                target.item,
                percent_encode_path(name)
            ),
            name: name.rsplit('/').next().map(str::to_owned),
            size: file["size"].as_str().and_then(|s| s.parse().ok()),
            referer: Some(page.to_owned()),
        });
    }
    if let Some(only) = &target.file
        && links.is_empty()
    {
        return Err(format!(
            "{only} : fichier absent de l'élément {}",
            target.item
        ));
    }
    if links.len() > MAX_LINKS {
        auro::log(&format!(
            "{} fichiers : seuls les {MAX_LINKS} premiers sont proposés",
            links.len()
        ));
        links.truncate(MAX_LINKS);
    }
    Ok(links)
}

/// Résout une page : `fetch` fait une requête GET par le moteur.
pub fn resolve(
    page: &str,
    fetch: impl FnOnce(&str) -> Result<Response, String>,
) -> Result<Vec<Link>, String> {
    let target = target(page)?;
    let api = format!("https://archive.org/metadata/{}", target.item);
    auro::log(&format!("lecture des métadonnées de {}", target.item));
    let response = fetch(&api)?;
    if response.status != 200 {
        return Err(format!(
            "métadonnées de {} : réponse {}",
            target.item, response.status
        ));
    }
    let metadata: Value = serde_json::from_str(&response.body)
        .map_err(|_| format!("métadonnées de {} illisibles", target.item))?;
    let links = links_of(&target, page, &metadata)?;
    auro::log(&format!("{} fichier(s) proposé(s)", links.len()));
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn item_pages_and_file_addresses_are_recognised() {
        assert_eq!(
            target("https://archive.org/details/night_of_the_living_dead?x=1#y").unwrap(),
            Target {
                item: "night_of_the_living_dead".into(),
                file: None
            }
        );
        assert_eq!(
            target("http://www.archive.org/details/Item-1.0/page/n5")
                .unwrap()
                .item,
            "Item-1.0"
        );
        assert_eq!(
            target("https://archive.org/download/lot/dossier/f%C3%A9e%20bleue.mp4").unwrap(),
            Target {
                item: "lot".into(),
                file: Some("dossier/fée bleue.mp4".into())
            }
        );
        assert_eq!(
            target("https://archive.org/download/lot/").unwrap().file,
            None
        );
        for bad in [
            "ftp://archive.org/details/a",
            "https://archive.org.evil.example/details/a",
            "https://evilarchive.org/details/a",
            "https://archive.org/",
            "https://archive.org/search?q=a",
            "https://archive.org/details/",
            "https://archive.org/details/a%2Fb",
            "https://archive.org/details/..",
            "https://archive.org/download/a/%zz",
        ] {
            assert!(target(bad).is_err(), "{bad}");
        }
    }

    fn metadata() -> Value {
        json!({
            "files": [
                {"name": "film.mp4", "source": "original", "format": "MPEG4", "size": "1234"},
                {"name": "film.ogv", "source": "derivative", "format": "Ogg Video", "size": "99"},
                {"name": "lot_meta.xml", "source": "original", "format": "Metadata"},
                {"name": "__ia_thumb.jpg", "source": "original", "format": "Item Tile"},
                {"name": "lot_archive.torrent", "source": "original", "format": "Archive BitTorrent"},
                {"name": "secret.pdf", "source": "original", "format": "PDF", "private": "true"},
                {"name": "sous dossier/notes #1.txt", "source": "original", "format": "Text"},
            ]
        })
    }

    #[test]
    fn original_files_are_offered_with_their_size() {
        let page = "https://archive.org/details/lot";
        let links = links_of(&target(page).unwrap(), page, &metadata()).unwrap();
        assert_eq!(
            links,
            [
                Link {
                    url: "https://archive.org/download/lot/film.mp4".into(),
                    name: Some("film.mp4".into()),
                    size: Some(1234),
                    referer: Some(page.into()),
                },
                Link {
                    url: "https://archive.org/download/lot/sous%20dossier/notes%20%231.txt".into(),
                    name: Some("notes #1.txt".into()),
                    size: None,
                    referer: Some(page.into()),
                },
            ]
        );
        // Un fichier désigné : lui seul, même dérivé.
        let one = "https://archive.org/download/lot/film.ogv";
        let links = links_of(&target(one).unwrap(), one, &metadata()).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].size, Some(99));
        let absent = "https://archive.org/download/lot/autre.mp4";
        assert!(links_of(&target(absent).unwrap(), absent, &metadata()).is_err());
    }

    #[test]
    fn missing_or_dark_items_and_bad_answers_are_errors() {
        let page = "https://archive.org/details/absent";
        let t = target(page).unwrap();
        assert!(
            links_of(&t, page, &json!({}))
                .unwrap_err()
                .contains("introuvable")
        );
        assert!(
            links_of(&t, page, &json!({"is_dark": true}))
                .unwrap_err()
                .contains("retiré")
        );
        let answer = |status, body: &str| {
            let body = body.to_owned();
            move |url: &str| {
                assert_eq!(url, "https://archive.org/metadata/absent");
                Ok(Response {
                    status,
                    url: url.to_owned(),
                    content_type: Some("application/json".into()),
                    body,
                })
            }
        };
        assert!(resolve(page, answer(503, "")).unwrap_err().contains("503"));
        assert!(
            resolve(page, answer(200, "<html>"))
                .unwrap_err()
                .contains("illisibles")
        );
        assert!(resolve(page, |_| Err("domaine non déclaré".into())).is_err());
        let found = resolve(page, answer(200, &metadata().to_string())).unwrap();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn the_output_follows_the_interface() {
        let out = auro::output(Ok(vec![Link {
            url: "https://archive.org/download/a/b".into(),
            name: None,
            size: Some(3),
            referer: None,
        }]));
        assert_eq!(out["links"][0]["url"], "https://archive.org/download/a/b");
        assert_eq!(out["links"][0]["size"], 3);
        assert_eq!(auro::output(Err("non".into())), json!({"error": "non"}));
        let answer =
            json!({"status": 200, "url": "u", "headers": {"content-type": null}, "body": "b"});
        assert_eq!(auro::response_of(&answer).unwrap().body, "b");
        assert!(auro::response_of(&json!({"error": "refusé"})).is_err());
    }
}
