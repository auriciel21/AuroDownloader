//! Banc de reprise des copies de sites (SPEC §7.18 ter, étape B1) : `aurod` copie plusieurs
//! sites à la fois et il est tué à un instant aléatoire (`TerminateProcess` sous Windows,
//! `SIGKILL` ailleurs), puis relancé, encore et encore ; un cycle sur cinq se termine par
//! un arrêt demandé.
//!
//! Le faux site a des pages, des images, un style qui en importe un autre et cite une
//! police, et un fichier coché servi par un autre hôte (par plages, comme sur un vrai
//! site) ; tout est bridé pour que les arrêts tombent dans chaque phase : exploration,
//! fichier coché, réécriture des liens (`post_processing`). Le rythme des copies est
//! raccourci (`aurod --site-spacing-ms`). Les copies s'ajoutent directement en base, moteur
//! arrêté.
//!
//! Une première copie, sans arrêt, sert de référence. Après chaque arrêt, le banc vérifie
//! qu'aucune copie n'est en échec ni annulée ; que la base n'annonce jamais plus que le
//! disque (élément fait : son fichier à son nom ; élément lu : son brut entier dans son
//! `.part`) ; et que chaque copie terminée est identique, fichier par fichier et octet par
//! octet, à la référence, sans fichier provisoire.
//!
//! Réglages, par variables d'environnement :
//! - `AURO_CRASH_CYCLES` : lancements d'`aurod` (défaut 6) ;
//! - `AURO_CRASH_TASKS` : copies gardées en cours (défaut 3) ;
//! - `AURO_CRASH_SECONDS` : durée visée d'une copie sans arrêt (défaut 3) ;
//! - `AURO_CRASH_SEED` : graine du hasard, affichée au départ pour rejouer un échec.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use auro_engine::store::site::{self, ItemState, NewSite, Settings};
use auro_engine::store::tasks::{self, TaskKind, TaskState, list_tasks};
use auro_engine::testing::{FileSite, Pace, Server, reply, serve_file_paced, serve_paced};
use serde_json::{Value, json};

/// Pages liées par la page de départ, images par page.
const PAGES: usize = 8;
const IMAGES: usize = 4;
/// Taille d'une image, et du fichier coché.
const IMAGE_LEN: usize = 16 * 1024;
const FILE_LEN: u64 = 1 << 20;

/// Générateur pseudo-aléatoire xorshift64* : reproductible à partir de sa graine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .map(|v| {
            v.parse()
                .unwrap_or_else(|_| panic!("{name} : nombre attendu, reçu {v:?}"))
        })
        .unwrap_or(default)
}

/// Connexion synchrone au point d'accès local.
#[cfg(windows)]
fn open(endpoint: &str) -> std::io::Result<(Box<dyn Read>, Box<dyn Write>)> {
    let pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(endpoint)?;
    Ok((Box::new(pipe.try_clone()?), Box::new(pipe)))
}

#[cfg(unix)]
fn open(endpoint: &str) -> std::io::Result<(Box<dyn Read>, Box<dyn Write>)> {
    let stream = std::os::unix::net::UnixStream::connect(endpoint)?;
    Ok((Box::new(stream.try_clone()?), Box::new(stream)))
}

/// Un appel du protocole, sur une connexion neuve ; `None` si le moteur ne répond pas.
fn call(endpoint: &str, method: &str, params: Value) -> Option<Value> {
    let (read, mut write) = open(endpoint).ok()?;
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    writeln!(write, "{request}").ok()?;
    let mut lines = BufReader::new(read).lines();
    loop {
        let message: Value = serde_json::from_str(&lines.next()?.ok()?).ok()?;
        if message["id"] == 1 {
            if let Some(error) = message.get("error") {
                panic!("{method} refusé : {error}");
            }
            return Some(message["result"].clone());
        }
    }
}

/// Un `aurod` du banc, tué s'il tourne encore quand le banc s'arrête (échec compris).
struct Aurod(Child);

impl std::ops::Deref for Aurod {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for Aurod {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for Aurod {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Une copie : chemin relatif (parties séparées par `/`) et contenu de chaque fichier.
type Tree = BTreeMap<String, Vec<u8>>;

fn tree(root: &Path) -> Tree {
    fn walk(root: &Path, dir: &Path, out: &mut Tree) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = Tree::new();
    walk(root, root, &mut out);
    out
}

/// Le faux site : `/` cite les pages, chaque page ses images et le style.
fn site_handler(r: &auro_engine::testing::Request, file_url: &str) -> auro_engine::testing::Reply {
    let html = |body: String| {
        reply(
            "200 OK",
            &[
                ("Content-Type", "text/html; charset=utf-8"),
                ("Content-Length", &body.len().to_string()),
            ],
            body.as_bytes(),
        )
    };
    let path = r.path.as_str();
    if path == "/" {
        let links: String = (0..PAGES)
            .map(|i| format!(r#"<a href="/p{i}.html">page {i}</a> "#))
            .collect();
        return html(format!(
            r#"<html><head><link rel="stylesheet" href="/s.css"></head><body>{links}<a href="{file_url}">fichier</a></body></html>"#
        ));
    }
    if let Some(i) = path
        .strip_prefix("/p")
        .and_then(|p| p.strip_suffix(".html"))
    {
        let images: String = (0..IMAGES)
            .map(|j| format!(r#"<img src="img/p{i}-{j}.png"> "#))
            .collect();
        return html(format!(
            r#"<html><head><link rel="stylesheet" href="s.css"></head><body><a href="/">accueil</a> {images}</body></html>"#
        ));
    }
    let (kind, body): (&str, Vec<u8>) = match path {
        "/s.css" => (
            "text/css",
            br#"@import "t.css"; body { background: url(img/fond.png) }"#.to_vec(),
        ),
        "/t.css" => (
            "text/css",
            br#"@font-face { src: url("/f.woff2") }"#.to_vec(),
        ),
        "/f.woff2" => ("font/woff2", vec![7; 4096]),
        _ if path.starts_with("/img/") => {
            let seed = path.bytes().fold(0u8, |a, b| a.wrapping_add(b));
            (
                "image/png",
                (0..IMAGE_LEN).map(|k| seed.wrapping_add(k as u8)).collect(),
            )
        }
        _ => return reply("404 Not Found", &[("Content-Length", "0")], b""),
    };
    reply(
        "200 OK",
        &[
            ("Content-Type", kind),
            ("Content-Length", &body.len().to_string()),
        ],
        &body,
    )
}

struct Bench {
    dir: tempfile::TempDir,
    endpoint: String,
    db: PathBuf,
    downloads: PathBuf,
    log: PathBuf,
    site: Server,
    file: Server,
    /// La copie de référence, faite sans arrêt.
    reference: Tree,
    added: u64,
    /// Arrêts tombés pendant une réécriture, ou pendant le fichier coché.
    in_links: u64,
    in_files: u64,
    verified: u64,
}

impl Bench {
    fn new(site_pace: Pace, file_pace: Pace) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("dl");
        std::fs::create_dir(&downloads).unwrap();
        let file = serve_file_paced(
            Arc::new(Mutex::new(FileSite::new(FILE_LEN))),
            Some(file_pace),
        );
        // L'adresse du fichier coché, connue une fois son serveur lancé.
        let file_url = file.url("/archive.bin");
        let site = serve_paced(move |r| site_handler(r, &file_url), Some(site_pace));
        let endpoint = if cfg!(windows) {
            format!(
                r"\\.\pipe\aurodownloader-banc-site-{}",
                auro_engine::store::new_id()
            )
        } else {
            dir.path().join("aurod.sock").to_string_lossy().into_owned()
        };
        Bench {
            db: dir.path().join("auro.db"),
            log: dir.path().join("aurod.log"),
            downloads,
            endpoint,
            site,
            file,
            reference: Tree::new(),
            added: 0,
            in_links: 0,
            in_files: 0,
            verified: 0,
            dir,
        }
    }

    /// Lance `aurod`, au rythme court, et attend qu'il réponde.
    fn start(&self) -> Aurod {
        let log = File::options()
            .create(true)
            .append(true)
            .open(&self.log)
            .unwrap();
        let mut child = Aurod(
            Command::new(env!("CARGO_BIN_EXE_aurod"))
                .arg("--db")
                .arg(&self.db)
                .args(["--endpoint", &self.endpoint, "--site-spacing-ms", "10"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(log)
                .spawn()
                .unwrap(),
        );
        let start = Instant::now();
        while call(&self.endpoint, "sync.snapshot", json!({})).is_none() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("aurod s'est arrêté ({status})\n{}", self.log_tail());
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "aurod ne répond pas\n{}",
                self.log_tail()
            );
            thread::sleep(Duration::from_millis(20));
        }
        child
    }

    /// Copies inachevées, moteur arrêté.
    fn unfinished_in_base(&self) -> usize {
        let conn = auro_engine::store::open(&self.db).unwrap();
        list_tasks(&conn)
            .unwrap()
            .iter()
            .filter(|t| t.state != TaskState::Completed)
            .count()
    }

    fn unfinished(&self) -> usize {
        let snapshot = call(&self.endpoint, "sync.snapshot", json!({})).expect("moteur");
        snapshot["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["state"] != "completed")
            .count()
    }

    fn unfinished_states(&self) -> String {
        let snapshot = call(&self.endpoint, "sync.snapshot", json!({})).expect("moteur");
        snapshot["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["state"] != "completed")
            .map(|t| format!("{} {} {}", t["state"], t["errorCode"], t["errorDetail"]))
            .collect::<Vec<_>>()
            .join(" ; ")
    }

    /// Ajoute des copies, moteur arrêté, jusqu'à en avoir `target` en cours.
    fn top_up(&mut self, target: usize) {
        let missing = target.saturating_sub(self.unfinished_in_base());
        let mut conn = auro_engine::store::open(&self.db).unwrap();
        let queue = tasks::ensure_queue(&conn, auro_engine::scheduler::DEFAULT_QUEUE).unwrap();
        let settings = Settings {
            depth: 1,
            max_pages: 200,
            max_bytes: 1 << 30,
            exclude: None,
            same_folder: false,
            ignore_robots: false,
        };
        let dest = self.downloads.to_string_lossy().into_owned();
        let page = self.site.url("/");
        let files = vec![self.file.url("/archive.bin")];
        for _ in 0..missing {
            self.added += 1;
            site::create_site_task(
                &mut conn,
                &NewSite {
                    queue_id: &queue,
                    page_url: &page,
                    display_name: "banc",
                    dest_dir: &dest,
                    speed_limit_bps: None,
                    max_connections: None,
                    idempotency_key: None,
                    settings: &settings,
                    files: &files,
                    public_only: false,
                },
                tasks::Start::default(),
            )
            .unwrap();
        }
    }

    fn shutdown(&self, child: &mut Aurod) {
        call(&self.endpoint, "engine.shutdown", json!({})).expect("engine.shutdown");
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "aurod : {status}\n{}", self.log_tail());
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "aurod ne s'arrête pas\n{}",
                self.log_tail()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn log_tail(&self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(8)..].join("\n")
    }

    /// Moteur arrêté : aucune copie en échec ni annulée ; la base n'annonce jamais plus
    /// que le disque ; chaque copie terminée est identique à la référence.
    fn check(&mut self) {
        let conn = auro_engine::store::open(&self.db).unwrap();
        let tasks = list_tasks(&conn).unwrap();
        self.verified = 0;
        for task in &tasks {
            assert_eq!(task.kind, TaskKind::Site, "tâche interne visible");
            assert!(
                !matches!(task.state, TaskState::Failed | TaskState::Cancelled),
                "copie {} : {:?} {:?} {:?}\n{}",
                task.id,
                task.state,
                task.error_code,
                task.error_detail,
                self.log_tail()
            );
            match site::load_job(&conn, &task.id).unwrap().phase {
                site::Phase::Links if task.state != TaskState::Completed => self.in_links += 1,
                site::Phase::Files => self.in_files += 1,
                _ => {}
            }
            let Some(name) = &task.file_name else {
                continue;
            };
            let root = self.downloads.join(name);
            for item in site::items(&conn, &task.id).unwrap() {
                let Some(path) = &item.path else { continue };
                let file = path.split('/').fold(root.clone(), |p, part| p.join(part));
                let mut part = file.as_os_str().to_os_string();
                part.push(".part");
                match item.state {
                    ItemState::Done => {
                        let len = std::fs::metadata(&file)
                            .unwrap_or_else(|e| panic!("{} fait, absent : {e}", file.display()))
                            .len();
                        assert_eq!(Some(len), item.size, "{} : taille", file.display());
                    }
                    ItemState::Fetched => {
                        let len = std::fs::metadata(&part)
                            .unwrap_or_else(|e| panic!("brut de {} absent : {e}", file.display()))
                            .len();
                        assert_eq!(Some(len), item.size, "brut de {}", file.display());
                    }
                    _ => {}
                }
            }
            if task.state == TaskState::Completed {
                let copy = tree(&root);
                if copy != self.reference {
                    let only: Vec<_> = copy
                        .keys()
                        .filter(|k| self.reference.get(*k) != copy.get(*k))
                        .chain(self.reference.keys().filter(|k| !copy.contains_key(*k)))
                        .collect();
                    panic!(
                        "{} : copie différente de la référence : {only:?}",
                        root.display()
                    );
                }
                self.verified += 1;
            }
        }
    }

    fn dir(&self) -> &Path {
        self.dir.path()
    }
}

#[test]
fn killed_engine_finishes_its_site_copies_without_corruption() {
    let cycles = env_f64("AURO_CRASH_CYCLES", 6.0) as u64;
    let tasks = env_f64("AURO_CRASH_TASKS", 3.0) as usize;
    let seconds = env_f64("AURO_CRASH_SECONDS", 3.0);
    let seed = std::env::var("AURO_CRASH_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64
                | 1
        });
    eprintln!(
        "banc de reprise des copies de sites : {cycles} cycles, {tasks} copies, environ \
         {seconds} s par copie, graine {seed}"
    );
    // Les images en un peu moins de la moitié du temps (deux à la fois), le fichier coché
    // en un peu moins de l'autre moitié.
    let chunk = 4 * 1024;
    let site_bytes = (PAGES * IMAGES * IMAGE_LEN) as f64;
    let site_pace = Pace {
        chunk,
        delay: Duration::from_secs_f64(chunk as f64 * 2.0 / (site_bytes / (seconds * 0.4))),
    };
    let file_pace = Pace {
        chunk: 16 * 1024,
        delay: Duration::from_secs_f64(16.0 * 1024.0 / (FILE_LEN as f64 / (seconds * 0.4) / 4.0)),
    };
    let mut bench = Bench::new(site_pace, file_pace);

    // Référence : une copie jusqu'au bout, sans arrêt.
    bench.top_up(1);
    let mut child = bench.start();
    let start = Instant::now();
    while bench.unfinished() > 0 {
        assert!(
            start.elapsed() < Duration::from_secs_f64(60.0 + seconds * 10.0),
            "copie de référence inachevée : {}\n{}",
            bench.unfinished_states(),
            bench.log_tail()
        );
        thread::sleep(Duration::from_millis(50));
    }
    bench.shutdown(&mut child);
    drop(child);
    {
        let conn = auro_engine::store::open(&bench.db).unwrap();
        let first = &list_tasks(&conn).unwrap()[0];
        bench.reference = tree(&bench.downloads.join(first.file_name.as_deref().unwrap()));
    }
    assert!(
        bench.reference.len() > PAGES * IMAGES,
        "référence trop petite : {:?}",
        bench.reference.keys().collect::<Vec<_>>()
    );
    assert!(
        bench.reference.keys().any(|k| k.ends_with("archive.bin")),
        "fichier coché absent de la référence"
    );
    bench.check();

    let mut rng = Rng(seed);
    let horizon_ms = (seconds * 1500.0) as u64;
    let (mut kills, mut shutdowns) = (0, 0);
    for cycle in 0..cycles {
        bench.top_up(tasks + 1);
        let mut child = bench.start();
        thread::sleep(Duration::from_millis(rng.below(horizon_ms)));
        if cycle % 5 == 4 {
            bench.shutdown(&mut child);
            shutdowns += 1;
        } else {
            let _ = child.kill();
            child.wait().unwrap();
            kills += 1;
        }
        drop(child);
        bench.check();
    }

    // Fin : tout est repris sans arrêt jusqu'au bout.
    let mut child = bench.start();
    let start = Instant::now();
    while bench.unfinished() > 0 {
        assert!(
            start.elapsed() < Duration::from_secs_f64(90.0 + seconds * 30.0),
            "copies inachevées : {}\n{}",
            bench.unfinished_states(),
            bench.log_tail()
        );
        thread::sleep(Duration::from_millis(100));
    }
    bench.shutdown(&mut child);
    drop(child);
    bench.check();
    let roots = std::fs::read_dir(&bench.downloads).unwrap().count() as u64;
    eprintln!(
        "{kills} arrêts brutaux, {shutdowns} arrêts demandés, {} copies ajoutées, {} arrêts \
         pendant un fichier coché, {} pendant une réécriture, {} copies terminées identiques à \
         la référence ({} fichiers chacune), {roots} dossiers (dossier {})",
        bench.added,
        bench.in_files,
        bench.in_links,
        bench.verified,
        bench.reference.len(),
        bench.dir().display()
    );
    assert_eq!(bench.verified, bench.added);
    assert_eq!(roots, bench.added, "un dossier par copie, rien d'autre");
}
