//! Plugin d'essai du prototype (F-41, étape 4a) : hébergeur fictif `fichiers.exemple`.
//!
//! Interface proposée (WebAssembly de base, sans WASI ni modèle de composants, pour que
//! wasmtime et wasmi la servent de même) :
//! - exports : `memory`, `auro_alloc(taille) -> adresse`, `auro_resolve(adresse, taille)
//!   -> (adresse << 32 | taille)` : JSON `{"url": …}` en entrée, `{"links": [{url, name,
//!   size}]}` ou `{"error": …}` en sortie ;
//! - imports du module `auro` : `fetch(adresse, taille) -> (adresse << 32 | taille)` (JSON
//!   `{"url": …}`, réponse `{"status", "url", "body"}` ou `{"error"}` écrite par l'hôte dans
//!   une zone qu'il obtient par `auro_alloc`), `log(adresse, taille)`.
//!
//! Plus des exports hostiles, pour éprouver le bac à sable : `auro_spin` (boucle sans fin),
//! `auro_hog` (mémoire jusqu'au refus), `auro_deep` (récursion), `auro_escape` (domaine non
//! déclaré).

use serde::Serialize;
use serde_json::{Value, json};

#[link(wasm_import_module = "auro")]
unsafe extern "C" {
    fn fetch(ptr: *const u8, len: usize) -> u64;
    fn log(ptr: *const u8, len: usize);
}

fn pack(ptr: *const u8, len: usize) -> u64 {
    ((ptr as u64) << 32) | len as u64
}

/// Rend à l'hôte un message JSON, dans une zone qu'il lit puis oublie (l'instance ne sert
/// qu'une fois).
fn hand_over(value: &Value) -> u64 {
    let bytes = serde_json::to_vec(value)
        .unwrap_or_default()
        .into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *const u8;
    pack(ptr, len)
}

/// Zone de `len` octets pour l'hôte (entrée de `auro_resolve`, réponse de `fetch`).
#[unsafe(no_mangle)]
pub extern "C" fn auro_alloc(len: usize) -> *mut u8 {
    let mut zone = Vec::<u8>::with_capacity(len.max(1));
    let ptr = zone.as_mut_ptr();
    std::mem::forget(zone);
    ptr
}

fn take(ptr: *mut u8, len: usize) -> Vec<u8> {
    // SAFETY : zone obtenue par `auro_alloc(len)`, remplie par l'hôte.
    unsafe { Vec::from_raw_parts(ptr, len, len.max(1)) }
}

fn say(text: &str) {
    // SAFETY : l'hôte lit `len` octets à `ptr` pendant l'appel.
    unsafe { log(text.as_ptr(), text.len()) }
}

struct Page {
    url: String,
    body: String,
}

fn get(url: &str) -> Result<Page, String> {
    let request = serde_json::to_vec(&json!({ "url": url })).map_err(|e| e.to_string())?;
    // SAFETY : l'hôte lit la requête pendant l'appel et rend une zone de `auro_alloc`.
    let packed = unsafe { fetch(request.as_ptr(), request.len()) };
    let answer = take(
        (packed >> 32) as usize as *mut u8,
        (packed & 0xffff_ffff) as usize,
    );
    let value: Value = serde_json::from_slice(&answer).map_err(|e| e.to_string())?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(error.to_owned());
    }
    Ok(Page {
        url: value["url"].as_str().unwrap_or(url).to_owned(),
        body: value["body"].as_str().unwrap_or_default().to_owned(),
    })
}

#[derive(Serialize)]
struct Link {
    url: String,
    name: String,
    size: Option<u64>,
}

/// Valeur d'un attribut dans une balise : `nom="valeur"`.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// Origine (`https://hôte`) d'une adresse.
fn origin(url: &str) -> &str {
    let after = url.find("://").map_or(0, |i| i + 3);
    let end = url[after..].find('/').map_or(url.len(), |i| i + after);
    &url[..end]
}

/// Les liens de téléchargement d'une page de l'hébergeur : `<a class="dl" href="/dl/…"
/// data-size="…">nom</a>`.
fn links(page: &Page) -> Vec<Link> {
    let mut found = Vec::new();
    let mut rest = page.body.as_str();
    while let Some(at) = rest.find("<a ") {
        rest = &rest[at..];
        let Some(close) = rest.find('>') else { break };
        let tag = &rest[..close];
        let after = &rest[close + 1..];
        if attribute(tag, "class") == Some("dl")
            && let Some(href) = attribute(tag, "href")
        {
            let url = if href.starts_with('/') {
                format!("{}{href}", origin(&page.url))
            } else {
                href.to_owned()
            };
            let name = after
                .split("</a>")
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned();
            let size = attribute(tag, "data-size").and_then(|s| s.parse().ok());
            found.push(Link { url, name, size });
        }
        rest = after;
    }
    found
}

fn resolve(input: &[u8]) -> Result<Vec<Link>, String> {
    let request: Value = serde_json::from_slice(input).map_err(|e| e.to_string())?;
    let url = request["url"].as_str().ok_or("url manquante")?;
    let page = get(url)?;
    let found = links(&page);
    say(&format!("{} liens trouvés", found.len()));
    Ok(found)
}

#[unsafe(no_mangle)]
pub extern "C" fn auro_resolve(ptr: *mut u8, len: usize) -> u64 {
    let input = take(ptr, len);
    match resolve(&input) {
        Ok(links) => hand_over(&json!({ "links": links })),
        Err(error) => hand_over(&json!({ "error": error })),
    }
}

// ---------- Exports hostiles (épreuves du bac à sable) ----------

/// Boucle sans fin : doit être arrêtée par la limite de calcul.
#[unsafe(no_mangle)]
pub extern "C" fn auro_spin() -> u32 {
    let mut n: u32 = 0;
    loop {
        n = std::hint::black_box(n.wrapping_add(1));
        if std::hint::black_box(n) == u32::MAX && std::hint::black_box(false) {
            return n;
        }
    }
}

/// Mémoire demandée par blocs de 16 Mio jusqu'au refus : doit être bornée par l'hôte.
#[unsafe(no_mangle)]
pub extern "C" fn auro_hog() -> u32 {
    let mut kept: Vec<Vec<u8>> = Vec::new();
    loop {
        let mut block = vec![0u8; 16 << 20];
        block[0] = 1;
        kept.push(std::hint::black_box(block));
        if kept.len() > 1000 {
            return kept.len() as u32;
        }
    }
}

#[inline(never)]
fn deep(n: u64) -> u64 {
    let local = std::hint::black_box([n; 16]);
    if n == 0 {
        return local[0];
    }
    deep(n - 1).wrapping_add(local[(n % 16) as usize])
}

/// Récursion de `n` niveaux : une profondeur démesurée doit finir sur la limite de pile.
#[unsafe(no_mangle)]
pub extern "C" fn auro_deep(n: u64) -> u64 {
    deep(std::hint::black_box(n))
}

/// Essaie un domaine non déclaré : 1 si l'hôte refuse, 0 s'il a servi la page.
#[unsafe(no_mangle)]
pub extern "C" fn auro_escape() -> u32 {
    match get("https://ailleurs.exemple/secret") {
        Err(_) => 1,
        Ok(_) => 0,
    }
}
