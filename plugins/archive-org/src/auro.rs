//! Interface entre un plugin et AuroDownloader, version 1 (SPEC §7.20). Module à recopier
//! dans un autre plugin : il ne dépend que de `serde_json`.
//!
//! - Le plugin exporte `memory`, `auro_alloc(taille) -> adresse` et `auro_resolve(adresse,
//!   taille) -> (adresse << 32) | taille` : JSON `{"url": page}` en entrée ; `{"links":
//!   [{url, name?, size?, referer?}]}` ou `{"error": texte}` en sortie.
//! - Il ne peut importer que `auro.fetch` (requête JSON `{url, method?, headers?, form?}`,
//!   réponse `{status, url, headers: {content-type}, body}` ou `{error}`, écrite par l'hôte
//!   dans une zone obtenue par `auro_alloc`) et `auro.log` (un texte montré à l'utilisateur).
//! - Le moteur ne sert que les domaines déclarés par `fetch` dans le manifeste, sur le
//!   réseau public ; il garde les cookies du site, que le plugin ne voit jamais.
//!
//! Chaque résolution a sa propre instance, détruite ensuite : rien n'est jamais libéré ici.

use serde_json::{Value, json};

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "auro")]
unsafe extern "C" {
    #[link_name = "fetch"]
    fn host_fetch(ptr: i32, len: i32) -> i64;
    #[link_name = "log"]
    fn host_log(ptr: i32, len: i32);
}

/// Un lien rendu au moteur, qui le télécharge lui-même.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    /// Nom proposé pour le fichier (le moteur le nettoie).
    pub name: Option<String>,
    /// Taille annoncée, en octets.
    pub size: Option<u64>,
    /// `Referer` à envoyer avec le téléchargement.
    pub referer: Option<String>,
}

/// Réponse d'une requête faite par le moteur pour le plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Adresse finale, après les redirections.
    pub url: String,
    pub content_type: Option<String>,
    /// Corps, en texte (un octet illisible devient « � »).
    pub body: String,
}

/// Lit la réponse JSON de `auro.fetch`.
pub fn response_of(answer: &Value) -> Result<Response, String> {
    if let Some(error) = answer.get("error") {
        return Err(error.as_str().unwrap_or("requête refusée").to_owned());
    }
    Ok(Response {
        status: answer["status"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok())
            .ok_or("réponse sans statut")?,
        url: answer["url"].as_str().unwrap_or_default().to_owned(),
        content_type: answer["headers"]["content-type"]
            .as_str()
            .map(str::to_owned),
        body: answer["body"].as_str().unwrap_or_default().to_owned(),
    })
}

/// Sortie de `auro_resolve`.
pub fn output(result: Result<Vec<Link>, String>) -> Value {
    match result {
        Ok(links) => json!({
            "links": links.iter().map(|l| json!({
                "url": l.url,
                "name": l.name,
                "size": l.size,
                "referer": l.referer,
            })).collect::<Vec<_>>(),
        }),
        Err(error) => json!({ "error": error }),
    }
}

/// Zone de `len` octets demandée par l'hôte (entrée, réponses de `fetch`).
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn auro_alloc(len: i32) -> i32 {
    let mut zone = Vec::<u8>::with_capacity(len.max(1) as usize);
    let ptr = zone.as_mut_ptr();
    std::mem::forget(zone);
    ptr as i32
}

/// Copie d'une zone de la mémoire, remplie par l'hôte.
#[cfg(target_arch = "wasm32")]
fn read(ptr: i32, len: i32) -> Vec<u8> {
    // SAFETY : `ptr` vient de `auro_alloc(len)`, et l'hôte y a écrit `len` octets.
    unsafe { std::slice::from_raw_parts(ptr as usize as *const u8, len.max(0) as usize).to_vec() }
}

/// Une zone `(adresse << 32) | taille` rendue par l'hôte.
#[cfg(target_arch = "wasm32")]
fn unpack(packed: i64) -> Vec<u8> {
    let packed = packed as u64;
    read(
        (packed >> 32) as u32 as i32,
        (packed & 0xffff_ffff) as u32 as i32,
    )
}

/// Rend un message JSON à l'hôte, dans une zone qu'il lit (jamais libérée).
#[cfg(target_arch = "wasm32")]
fn hand_over(value: &Value) -> i64 {
    let bytes = serde_json::to_vec(value).unwrap_or_default().leak();
    ((bytes.as_ptr() as usize as i64) << 32) | bytes.len() as i64
}

/// Une ligne du journal du plugin, montrée à l'utilisateur dans « Ajouter ».
pub fn log(text: &str) {
    #[cfg(target_arch = "wasm32")]
    // SAFETY : l'hôte lit `len` octets à `ptr` pendant l'appel.
    unsafe {
        host_log(text.as_ptr() as usize as i32, text.len() as i32)
    }
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("auro.log : {text}");
}

/// Une requête GET par le moteur (domaines déclarés seulement).
#[cfg(target_arch = "wasm32")]
pub fn get(url: &str, accept: &str) -> Result<Response, String> {
    let request = serde_json::to_vec(&json!({
        "url": url,
        "headers": {"Accept": accept},
    }))
    .unwrap_or_default();
    // SAFETY : l'hôte lit la requête pendant l'appel, puis écrit sa réponse dans une zone
    // obtenue par `auro_alloc`.
    let packed = unsafe { host_fetch(request.as_ptr() as usize as i32, request.len() as i32) };
    let answer: Value =
        serde_json::from_slice(&unpack(packed)).map_err(|_| "réponse illisible du moteur")?;
    response_of(&answer)
}

/// Corps de `auro_resolve` : lit `{"url": …}`, appelle `resolve`, rend sa sortie.
#[cfg(target_arch = "wasm32")]
pub fn run(ptr: i32, len: i32, resolve: impl FnOnce(&str) -> Result<Vec<Link>, String>) -> i64 {
    let input: Value = serde_json::from_slice(&read(ptr, len)).unwrap_or_default();
    let result = match input["url"].as_str() {
        Some(page) => resolve(page),
        None => Err("entrée sans adresse de page".to_owned()),
    };
    hand_over(&output(result))
}
