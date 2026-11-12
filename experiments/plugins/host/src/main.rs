//! Hôte du prototype des plugins (F-41, étape 4a) : charge le plugin d'essai avec wasmtime
//! ou wasmi (`--features`), et mesure : compilation, premier appel, appels suivants (une
//! instance neuve à chaque fois), épreuves du bac à sable, mémoire de pointe du processus.
//! Sans moteur : binaire de référence, pour mesurer ce que chacun ajoute.
//!
//! `host <sample.wasm>` ; `AURO_RUNS` (20 par défaut).

#![cfg_attr(
    not(any(feature = "wasmtime", feature = "wasmi")),
    allow(dead_code, unused_imports)
)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[cfg(feature = "wasmi")]
mod rt_wasmi;
#[cfg(feature = "wasmtime")]
mod rt_wasmtime;

#[cfg(feature = "wasmi")]
use rt_wasmi as rt;
#[cfg(feature = "wasmtime")]
use rt_wasmtime as rt;

/// Limites d'une instance : calcul (unités de « fuel »), mémoire linéaire, pile.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub fuel: u64,
    pub memory: usize,
    pub stack: usize,
}

pub const LIMITS: Limits = Limits {
    fuel: 1_000_000_000,
    memory: 64 << 20,
    stack: 512 << 10,
};

/// Réponses d'une page, au plus.
const MAX_BODY: usize = 8 << 20;

/// Ce que l'hôte offre au plugin : `fetch` vers les domaines déclarés (pages servies ici,
/// sans réseau), `log`.
#[derive(Debug, Default)]
pub struct Host {
    pub declared: Vec<String>,
    pub pages: HashMap<String, String>,
    pub fetches: u32,
    pub refused: u32,
    pub logs: Vec<String>,
}

fn host_of(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&rest[..end])
}

impl Host {
    pub fn fetch(&mut self, request: &[u8]) -> Vec<u8> {
        self.fetches += 1;
        let answer = match serde_json::from_slice::<Value>(request) {
            Ok(request) => {
                let url = request["url"].as_str().unwrap_or_default();
                let allowed = host_of(url).is_some_and(|h| {
                    self.declared
                        .iter()
                        .any(|d| h == d || h.ends_with(&format!(".{d}")))
                });
                if !allowed {
                    self.refused += 1;
                    json!({"error": "domaine non déclaré"})
                } else {
                    match self.pages.get(url) {
                        Some(body) if body.len() <= MAX_BODY => {
                            json!({"status": 200, "url": url, "body": body})
                        }
                        Some(_) => json!({"error": "page trop grande"}),
                        None => json!({"error": "introuvable"}),
                    }
                }
            }
            Err(_) => json!({"error": "requête illisible"}),
        };
        serde_json::to_vec(&answer).unwrap_or_default()
    }

    pub fn log(&mut self, text: &[u8]) {
        if self.logs.len() < 100 {
            self.logs
                .push(String::from_utf8_lossy(&text[..text.len().min(1000)]).into_owned());
        }
    }
}

const PAGE: &str = "https://fichiers.exemple/page";

/// Une page d'hébergeur d'environ `size` octets : texte, liens ordinaires et, un bloc sur
/// dix, un lien de téléchargement. Rend la page et le nombre de liens à trouver.
fn fixture(size: usize) -> (String, usize) {
    let mut page = String::from("<!doctype html><html><body>\n");
    let mut i = 0;
    let mut wanted = 0;
    while page.len() < size {
        page.push_str(
            "<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod \
                       tempor incididunt ut labore et dolore magna aliqua.</p>\n",
        );
        page.push_str(&format!("<a href=\"/autre/{i}\">page {i}</a>\n"));
        if i % 10 == 0 {
            page.push_str(&format!(
                "<a class=\"dl\" href=\"/dl/{i}\" data-size=\"{}\">fichier-{i}.iso</a>\n",
                1000 + i
            ));
            wanted += 1;
        }
        i += 1;
    }
    page.push_str("</body></html>\n");
    (page, wanted)
}

fn new_host(page: &str) -> Host {
    Host {
        declared: vec!["fichiers.exemple".into()],
        pages: HashMap::from([(PAGE.to_owned(), page.to_owned())]),
        ..Host::default()
    }
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// Mémoire de pointe du processus (octets).
#[cfg(windows)]
fn peak_memory() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY : structure de la bonne taille, processus courant.
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    counters.PeakWorkingSetSize as u64
}

#[cfg(not(windows))]
fn peak_memory() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        })
        .map_or(0, |kib| kib * 1024)
}

fn mib(bytes: u64) -> String {
    format!("{:.1} Mio", bytes as f64 / (1 << 20) as f64)
}

#[cfg(not(any(feature = "wasmtime", feature = "wasmi")))]
fn main() {
    let (page, wanted) = fixture(1 << 20);
    let mut host = new_host(&page);
    let answer = host.fetch(br#"{"url": "https://fichiers.exemple/page"}"#);
    println!(
        "référence sans moteur : page {} octets, {wanted} liens, réponse {} octets, pointe {}",
        page.len(),
        answer.len(),
        mib(peak_memory())
    );
}

#[cfg(any(feature = "wasmtime", feature = "wasmi"))]
fn main() {
    let path = std::env::args().nth(1).expect("chemin du plugin .wasm");
    let wasm = std::fs::read(&path).expect("plugin illisible");
    let runs: usize = std::env::var("AURO_RUNS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(20);
    let (page, wanted) = fixture(1 << 20);
    println!("moteur : {}", rt::NAME);
    println!(
        "plugin : {} octets ; page : {} octets, {wanted} liens",
        wasm.len(),
        page.len()
    );

    let start = Instant::now();
    let runtime = rt::Runtime::new(&wasm, &LIMITS).expect("compilation");
    println!("compilation : {:?}", start.elapsed());

    let input = serde_json::to_vec(&json!({"url": PAGE})).unwrap();
    let mut times = Vec::new();
    for run in 0..=runs {
        let start = Instant::now();
        let (result, host, fuel) = runtime.resolve(new_host(&page), &input, &LIMITS);
        let elapsed = start.elapsed();
        let output = result.expect("résolution");
        let value: Value = serde_json::from_slice(&output).expect("sortie JSON");
        let found = value["links"].as_array().map_or(0, Vec::len);
        assert_eq!(found, wanted, "{value}");
        if run == 0 {
            println!(
                "premier appel : {elapsed:?} ({found} liens, {fuel} unités de calcul, journal {:?})",
                host.logs
            );
        } else {
            times.push(elapsed);
        }
    }
    println!(
        "appels suivants (instance neuve, médiane de {runs}) : {:?}",
        median(times)
    );

    // Épreuves du bac à sable.
    let start = Instant::now();
    let (result, _, fuel) = runtime.call_i32(new_host(&page), "auro_spin", &LIMITS);
    println!(
        "boucle sans fin : {:?} après {:?} ({fuel} unités)",
        result.map_err(short),
        start.elapsed()
    );
    let start = Instant::now();
    let (result, _, _) = runtime.call_i32(new_host(&page), "auro_hog", &LIMITS);
    println!(
        "mémoire démesurée : {:?} après {:?}",
        result.map_err(short),
        start.elapsed()
    );
    for depth in [1_000u64, 5_000, 100_000_000] {
        let start = Instant::now();
        let (result, _, _) = runtime.call_deep(new_host(&page), depth, &LIMITS);
        println!(
            "récursion de {depth} : {:?} après {:?}",
            result.map_err(short),
            start.elapsed()
        );
    }
    let (result, host, _) = runtime.call_i32(new_host(&page), "auro_escape", &LIMITS);
    println!(
        "domaine non déclaré : {:?} (1 = refusé), refus de l'hôte : {}",
        result.map_err(short),
        host.refused
    );
    // Le moteur sert encore après les épreuves.
    let (result, _, _) = runtime.resolve(new_host(&page), &input, &LIMITS);
    assert!(result.is_ok(), "après les épreuves : {result:?}");
    println!("après les épreuves : résolution correcte");
    println!("mémoire de pointe du processus : {}", mib(peak_memory()));
}

#[cfg(any(feature = "wasmtime", feature = "wasmi"))]
fn short(e: String) -> String {
    e.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect()
}
