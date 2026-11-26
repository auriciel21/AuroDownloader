//! `auro links` (F-29, F-31, SPEC §7.18) : les liens d'une page, d'un texte ou d'une liste
//! importée, filtrés par la CLI (sorte, extension, expression régulière, taille), avec leur
//! taille lue par le moteur ; `--add` les ajoute ensemble (`task.addBatch`). Avec `--depth`,
//! le moteur explore le site de lien en lien (F-42, SPEC §7.18 bis).

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::{Value, json};

use crate::display::size;
use auro_cli::client::Rpc;
use auro_engine::io::{BoundedReadError, read_bounded};

/// Taille maximale d'un texte ou d'une liste envoyés au moteur : la sienne (SPEC §7.18,
/// audit C29).
const MAX_TEXT: u64 = auro_engine::linkscan::MAX_INPUT as u64;
/// Liens dont la taille est demandée, au plus (décision de l'utilisateur du 2026-11-11).
const MAX_SIZED: usize = 500;
/// Adresses par appel à `linkscan.probe`.
const PROBE_CHUNK: usize = 50;
/// Éléments par appel à `task.addBatch`.
const BATCH_CHUNK: usize = 1000;

/// Ce qui est gardé des liens trouvés.
#[derive(Default, Clone)]
pub struct Filters {
    /// Extensions gardées, en minuscules ; toutes si vide.
    pub extensions: Vec<String>,
    /// Expression que l'adresse doit contenir.
    pub regex: Option<Regex>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Pages et liens sans extension aussi (masqués par défaut).
    pub all: bool,
}

impl Filters {
    /// Un lien gardé, avant les tailles : sorte, extension, expression.
    fn keeps(&self, link: &Value) -> bool {
        let kind = link["kind"].as_str().unwrap_or_default();
        if !self.all && kind == "page" {
            return false;
        }
        if !self.extensions.is_empty() {
            let ext = link["ext"].as_str().unwrap_or_default();
            if !self.extensions.iter().any(|e| e == ext) {
                return false;
            }
        }
        let url = link["url"].as_str().unwrap_or_default();
        self.regex.as_ref().is_none_or(|r| r.is_match(url))
    }

    /// La taille, si un filtre la demande : inconnue, le lien n'est pas gardé.
    fn keeps_size(&self, size: Option<u64>) -> bool {
        if self.min_size.is_none() && self.max_size.is_none() {
            return true;
        }
        size.is_some_and(|s| {
            self.min_size.is_none_or(|m| s >= m) && self.max_size.is_none_or(|m| s <= m)
        })
    }
}

/// Exploration d'un site (F-42, SPEC §7.18 bis).
#[derive(Debug, Clone)]
pub struct Crawl {
    /// Niveau le plus profond lu (1 à 5).
    pub depth: u8,
    pub max_pages: Option<u32>,
    /// Pages à ne pas ouvrir ; vide : aucune.
    pub exclude: String,
    pub same_folder: bool,
    pub ignore_robots: bool,
    /// Garder l'arborescence du site (`sitePath`) à l'ajout.
    pub tree: bool,
}

/// Ce que fait `auro links`.
pub struct LinksOptions<'a> {
    /// Adresse d'une page, chemin d'un fichier texte ou CSV, ou « - » (entrée standard).
    pub source: &'a str,
    pub filters: Filters,
    /// Explorer le site à partir de la page.
    pub crawl: Option<Crawl>,
    /// Ne pas demander les tailles (aucune requête vers les fichiers).
    pub no_sizes: bool,
    /// Ajouter les liens gardés, dans ce dossier.
    pub add: Option<&'a Path>,
    pub paused: bool,
}

/// Paramètres de `linkscan.run` pour une source.
fn scan_params(source: &str) -> Result<Value> {
    if source.contains("://") {
        return Ok(json!({"pageUrl": source}));
    }
    let bytes = if source == "-" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(MAX_TEXT + 1)
            .read_to_end(&mut bytes)
            .context("lecture de l'entrée standard")?;
        bytes
    } else {
        match read_bounded(Path::new(source), MAX_TEXT) {
            Ok(bytes) => bytes,
            Err(BoundedReadError::TooLarge { len, .. }) => {
                bail!("{source} : liste trop grosse ({})", size(len))
            }
            Err(BoundedReadError::Io(e)) => {
                return Err(e)
                    .with_context(|| format!("{source} : ni adresse, ni fichier lisible"));
            }
        }
    };
    if bytes.len() as u64 > MAX_TEXT {
        bail!("entrée trop grosse (plus de {})", size(MAX_TEXT));
    }
    Ok(json!({"text": String::from_utf8_lossy(&bytes)}))
}

/// Paramètres de `linkscan.crawl`.
fn crawl_params(page: &str, crawl: &Crawl) -> Value {
    let mut params = json!({
        "pageUrl": page,
        "depth": crawl.depth,
        "exclude": crawl.exclude,
        "sameFolder": crawl.same_folder,
        "ignoreRobots": crawl.ignore_robots,
    });
    if let Some(n) = crawl.max_pages {
        params["maxPages"] = json!(n);
    }
    params
}

/// Pourquoi une exploration finie s'est arrêtée, en clair ; rien si elle a tout lu.
fn stop_reason(stop: &str) -> Option<&'static str> {
    match stop {
        "maxPages" => Some("limite de pages atteinte (--max-pages)"),
        "maxBytes" => Some("256 Mio de pages lus"),
        "maxLinks" => Some("10 000 liens trouvés"),
        _ => None,
    }
}

/// Explore le site à partir de `page` ; l'avancement sur la sortie d'erreur ; Ctrl-C arrête
/// l'exploration et garde ce qui a été trouvé.
async fn crawl_links(rpc: &mut Rpc, page: &str, crawl: &Crawl) -> Result<Vec<Value>> {
    let started = rpc
        .call("linkscan.crawl", crawl_params(page, crawl))
        .await?;
    let id = started["crawlId"].as_str().unwrap_or_default().to_owned();
    loop {
        let next = tokio::select! {
            note = rpc.notification() => Some(note?),
            _ = tokio::signal::ctrl_c() => None,
        };
        let Some((method, p)) = next else {
            eprintln!();
            eprintln!("arrêt demandé : l'exploration s'arrête, ce qui a été trouvé est gardé");
            rpc.call("linkscan.cancel", json!({"crawlId": id})).await?;
            break;
        };
        if method != "linkscan.progress" || p["crawlId"] != json!(id) {
            continue;
        }
        eprint!(
            "\r{} page(s) lue(s), {} en attente, {} lien(s)   ",
            p["pagesRead"], p["pagesQueued"], p["links"]
        );
        if p["state"] != "running" {
            eprintln!();
            break;
        }
    }
    let result = rpc
        .call("linkscan.crawlResult", json!({"crawlId": id}))
        .await?;
    if result["state"] == "failed" {
        bail!(
            "exploration impossible : {} [{}]",
            result["error"]["message"].as_str().unwrap_or_default(),
            result["error"]["code"].as_str().unwrap_or_default()
        );
    }
    if let Some(why) = result["stop"].as_str().and_then(stop_reason) {
        eprintln!("exploration arrêtée : {why}");
    }
    Ok(result["links"].as_array().cloned().unwrap_or_default())
}

pub async fn links(rpc: &mut Rpc, opts: LinksOptions<'_>) -> Result<()> {
    let mut found = match &opts.crawl {
        Some(crawl) if opts.source.contains("://") => crawl_links(rpc, opts.source, crawl).await?,
        Some(_) => bail!("--depth explore un site : il faut l'adresse d'une page"),
        None => {
            let scan = rpc.call("linkscan.run", scan_params(opts.source)?).await?;
            scan["links"].as_array().cloned().unwrap_or_default()
        }
    };
    // Tailles (500 au plus, liens magnet exclus) : celles des liens gardés, puis celles des
    // « pages » masquées, que le serveur dira peut-être des fichiers (SHA256SUMS…).
    let mut sizes: HashMap<String, u64> = HashMap::new();
    if !opts.no_sizes {
        let wide = Filters {
            all: true,
            ..opts.filters.clone()
        };
        let probed = |l: &&Value| l["kind"] != "magnet";
        let first = found
            .iter()
            .filter(probed)
            .filter(|l| opts.filters.keeps(l));
        let then = found
            .iter()
            .filter(probed)
            .filter(|l| !opts.filters.keeps(l) && l["kind"] == "page" && wide.keeps(l));
        let urls: Vec<String> = first
            .chain(then)
            .filter_map(|l| l["url"].as_str().map(str::to_owned))
            .take(MAX_SIZED)
            .collect();
        let mut files: HashSet<String> = HashSet::new();
        for chunk in urls.chunks(PROBE_CHUNK) {
            let probed = rpc.call("linkscan.probe", json!({"urls": chunk})).await?;
            for item in probed["items"].as_array().into_iter().flatten() {
                let Some(url) = item["url"].as_str() else {
                    continue;
                };
                if let Some(s) = item["size"].as_u64() {
                    sizes.insert(url.to_owned(), s);
                }
                if item["kind"] == "file" {
                    files.insert(url.to_owned());
                }
            }
        }
        // Une « page » que le serveur décrit comme un fichier en devient un ; jamais l'inverse.
        for l in &mut found {
            if l["kind"] == "page" && l["url"].as_str().is_some_and(|u| files.contains(u)) {
                l["kind"] = json!("file");
            }
        }
    }
    let mut kept: Vec<Value> = found
        .iter()
        .filter(|l| opts.filters.keeps(l))
        .cloned()
        .collect();
    let size_of = |l: &Value| l["url"].as_str().and_then(|u| sizes.get(u)).copied();
    kept.retain(|l| opts.filters.keeps_size(size_of(l)));
    for (i, l) in kept.iter().enumerate() {
        let shown = size_of(l).map_or_else(|| "?".to_owned(), size);
        println!(
            "{i:>4}  {shown:>10}  {}  {}",
            l["name"].as_str().unwrap_or_default(),
            l["url"].as_str().unwrap_or_default()
        );
    }
    let hidden = found.len() - kept.len();
    match hidden {
        0 => println!("{} lien(s)", kept.len()),
        n => println!(
            "{} lien(s) gardé(s), {n} écarté(s) par les filtres",
            kept.len()
        ),
    }
    let Some(dir) = opts.add else {
        return Ok(());
    };
    if kept.is_empty() {
        bail!("aucun lien à ajouter");
    }
    let dest = crate::remote::dest_dir(dir)?;
    let tree = opts.crawl.as_ref().is_some_and(|c| c.tree);
    let (mut added, mut refused) = (0, 0);
    for chunk in kept.chunks(BATCH_CHUNK) {
        let items: Vec<Value> = chunk
            .iter()
            .map(|l| {
                let mut item = json!({"sources": [{"url": l["url"]}], "destDir": dest,
                                      "startPaused": opts.paused});
                // Arborescence du site (SPEC §7.18 bis) : hôte, puis dossiers.
                if let Some(path) = l["sitePath"].as_str().filter(|p| tree && !p.is_empty()) {
                    item["subDir"] = json!(path);
                }
                item
            })
            .collect();
        let result = rpc.call("task.addBatch", json!({"items": items})).await?;
        for t in result["tasks"].as_array().into_iter().flatten() {
            let index = t["index"].as_u64().unwrap_or(0) as usize;
            println!(
                "{}  {}",
                t["taskId"].as_str().unwrap_or_default(),
                chunk[index]["url"].as_str().unwrap_or_default()
            );
            added += 1;
        }
        for e in result["errors"].as_array().into_iter().flatten() {
            let index = e["index"].as_u64().unwrap_or(0) as usize;
            eprintln!(
                "refusé : {}  {} [{}]",
                chunk[index]["url"].as_str().unwrap_or_default(),
                e["message"].as_str().unwrap_or_default(),
                e["code"].as_str().unwrap_or_default()
            );
            refused += 1;
        }
    }
    if refused > 0 {
        bail!("{added} ajouté(s), {refused} refusé(s)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str, ext: Option<&str>, kind: &str) -> Value {
        json!({"url": url, "name": "x", "ext": ext, "kind": kind})
    }

    #[test]
    fn filters_by_kind_extension_expression_and_size() {
        let iso = link("https://a.example/debian-13.iso", Some("iso"), "file");
        let page = link("https://a.example/index.html", Some("html"), "page");
        let magnet = link("magnet:?xt=urn:btih:abc", None, "magnet");
        let zip = link("https://b.example/src.zip", Some("zip"), "file");
        let plain = Filters::default();
        assert!(plain.keeps(&iso) && plain.keeps(&magnet) && !plain.keeps(&page));
        let all = Filters {
            all: true,
            ..Filters::default()
        };
        assert!(all.keeps(&page));
        let ext = Filters {
            extensions: vec!["iso".into(), "7z".into()],
            ..Filters::default()
        };
        assert!(ext.keeps(&iso) && !ext.keeps(&zip) && !ext.keeps(&magnet));
        let regex = Filters {
            regex: Some(Regex::new(r"debian-\d+").unwrap()),
            ..Filters::default()
        };
        assert!(regex.keeps(&iso) && !regex.keeps(&zip));
        // Taille : sans filtre, tout passe ; avec, une taille inconnue n'est pas gardée.
        assert!(plain.keeps_size(None));
        let sized = Filters {
            min_size: Some(10),
            max_size: Some(20),
            ..Filters::default()
        };
        assert!(sized.keeps_size(Some(10)) && sized.keeps_size(Some(20)));
        assert!(
            !sized.keeps_size(Some(9)) && !sized.keeps_size(Some(21)) && !sized.keeps_size(None)
        );
    }

    #[test]
    fn crawl_parameters_follow_the_options() {
        let crawl = Crawl {
            depth: 2,
            max_pages: None,
            exclude: String::new(),
            same_folder: true,
            ignore_robots: false,
            tree: false,
        };
        assert_eq!(
            crawl_params("https://a.example/", &crawl),
            json!({"pageUrl": "https://a.example/", "depth": 2, "exclude": "",
                   "sameFolder": true, "ignoreRobots": false})
        );
        let limited = Crawl {
            max_pages: Some(30),
            ..crawl
        };
        assert_eq!(crawl_params("https://a.example/", &limited)["maxPages"], 30);
        assert_eq!(stop_reason("finished"), None);
        assert!(stop_reason("maxPages").is_some());
    }

    #[test]
    fn a_source_is_a_page_a_file_or_the_standard_input() {
        assert_eq!(
            scan_params("https://a.example/liste.html").unwrap(),
            json!({"pageUrl": "https://a.example/liste.html"})
        );
        let dir = tempfile::tempdir().unwrap();
        let list = dir.path().join("liste.csv");
        std::fs::write(&list, "url;nom\nhttps://a.example/f.iso;f\n").unwrap();
        let params = scan_params(list.to_str().unwrap()).unwrap();
        assert!(
            params["text"]
                .as_str()
                .unwrap()
                .contains("https://a.example/f.iso")
        );
        assert!(scan_params(dir.path().join("absent.txt").to_str().unwrap()).is_err());
    }
}
