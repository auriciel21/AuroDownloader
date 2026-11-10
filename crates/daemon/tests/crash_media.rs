//! Banc de reprise des vidéos (SPEC §7.17, phase 5, étape 4d) : `aurod` télécharge
//! plusieurs vidéos à la fois et il est tué à un instant aléatoire (`TerminateProcess` sous
//! Windows, `SIGKILL` ailleurs), puis relancé, encore et encore ; un cycle sur cinq se
//! termine par un arrêt demandé.
//!
//! Chaque vidéo a deux flux : un flux fragmenté, que le faux yt-dlp « télécharge » en une
//! à deux secondes, et un flux direct (son), servi en local et bridé, que le moteur
//! télécharge par sa tâche HTTP interne. Le faux ffmpeg attend une seconde avant de
//! fusionner : des arrêts tombent aussi pendant la fusion (`post_processing`). Les outils
//! sont imposés par `aurod --media-ytdlp --media-ffmpeg --media-ffprobe`.
//!
//! Après chaque arrêt, le banc vérifie qu'aucune vidéo n'est en échec ni annulée, que
//! chaque vidéo terminée est juste octet par octet (flux fragmenté puis son, bout à bout),
//! et qu'elle ne laisse ni flux ni `.part`.
//!
//! Réglages, par variables d'environnement :
//! - `AURO_CRASH_CYCLES` : lancements d'`aurod` (défaut 6) ;
//! - `AURO_CRASH_TASKS` : vidéos gardées en cours (défaut 3) ;
//! - `AURO_CRASH_SIZE` : taille du flux direct en octets (défaut 1 Mio) ;
//! - `AURO_CRASH_SECONDS` : durée visée d'un flux direct sans arrêt (défaut 3) ;
//! - `AURO_CRASH_SEED` : graine du hasard, affichée au départ pour rejouer un échec.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use auro_engine::store::media::load_streams;
use auro_engine::store::tasks::{TaskKind, TaskState, list_tasks};
use auro_engine::testing::tools::{fake_ffmpeg_slow, fake_ytdlp_media};
use auro_engine::testing::{FileSite, Pace, Server, content_range, serve_file_paced};
use serde_json::{Value, json};

const MIB: u64 = 1024 * 1024;

/// Contenu du flux fragmenté (écrit par le faux yt-dlp).
const FRAGMENTED_LEN: usize = 200 * 1024;
const FRAGMENTED_OFFSET: u64 = 900_000_000;

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

struct Bench {
    dir: tempfile::TempDir,
    endpoint: String,
    db: PathBuf,
    downloads: PathBuf,
    log: PathBuf,
    _server: Server,
    ytdlp: PathBuf,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    /// Contenu attendu de chaque vidéo terminée.
    expected: Vec<u8>,
    added: u64,
    /// Arrêts tombés pendant une fusion.
    in_merge: u64,
    verified: u64,
}

impl Bench {
    fn new(size: usize, pace: Pace) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("dl");
        std::fs::create_dir(&downloads).unwrap();
        // Plages et ETag : le flux direct reprend là où il en était, comme sur un vrai site.
        let audio = content_range(0, size);
        let server = serve_file_paced(Arc::new(Mutex::new(FileSite::new(size as u64))), Some(pace));
        let fragment = content_range(FRAGMENTED_OFFSET, FRAGMENTED_LEN);
        let answer = json!({
            "title": "Banc", "duration": 6.0,
            "requested_formats": [
                {"format_id": "hls-1", "ext": "mp4", "vcodec": "avc1", "acodec": "none",
                 "protocol": "m3u8_native", "url": server.url("/v.m3u8")},
                {"format_id": "a1", "ext": "m4a", "vcodec": "none", "acodec": "mp4a",
                 "protocol": "http", "url": server.url("/a.m4a")}
            ]
        })
        .to_string();
        let fake = dir.path().join("faux");
        let ytdlp = fake_ytdlp_media(&fake, &answer, &fragment, 1);
        let (ffmpeg, ffprobe) = fake_ffmpeg_slow(&fake, "6.0", 1);
        let mut expected = fragment;
        expected.extend(audio);
        let endpoint = if cfg!(windows) {
            format!(
                r"\\.\pipe\aurodownloader-banc-video-{}",
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
            _server: server,
            ytdlp,
            ffmpeg,
            ffprobe,
            expected,
            added: 0,
            in_merge: 0,
            verified: 0,
            dir,
        }
    }

    /// Lance `aurod` avec les faux outils, et attend qu'il réponde.
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
                .args(["--endpoint", &self.endpoint])
                .arg("--media-ytdlp")
                .arg(&self.ytdlp)
                .arg("--media-ffmpeg")
                .arg(&self.ffmpeg)
                .arg("--media-ffprobe")
                .arg(&self.ffprobe)
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
            .map(|t| {
                format!(
                    "{} {} {}/{} {} {}",
                    t["fileName"],
                    t["state"],
                    t["doneBytes"],
                    t["totalBytes"],
                    t["errorCode"],
                    t["errorDetail"]
                )
            })
            .collect::<Vec<_>>()
            .join(" ; ")
    }

    /// Ajoute des vidéos jusqu'à en avoir `target` en cours.
    fn top_up(&mut self, target: usize) {
        let dest = self.downloads.to_str().unwrap().to_owned();
        for _ in self.unfinished()..target {
            self.added += 1;
            call(
                &self.endpoint,
                "task.add",
                json!({
                    "media": {"pageUrl": format!("https://exemple.org/v{}", self.added)},
                    "destDir": dest,
                }),
            )
            .expect("task.add");
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

    /// Moteur arrêté : aucune vidéo en échec ni annulée ; chaque vidéo terminée est juste
    /// et ne laisse ni flux ni `.part` ; les tâches internes ne se voient pas.
    fn check(&mut self) {
        let conn = auro_engine::store::open(&self.db).unwrap();
        let tasks = list_tasks(&conn).unwrap();
        self.verified = 0;
        let mut finals = Vec::new();
        for task in &tasks {
            assert_eq!(task.kind, TaskKind::Media, "tâche interne visible");
            assert!(
                !matches!(task.state, TaskState::Failed | TaskState::Cancelled),
                "vidéo {} : {:?} {:?} {:?}\n{}",
                task.id,
                task.state,
                task.error_code,
                task.error_detail,
                self.log_tail()
            );
            if task.state == TaskState::PostProcessing {
                self.in_merge += 1;
            }
            if task.state != TaskState::Completed {
                continue;
            }
            let name = task.file_name.clone().expect("nom final");
            let path = self.downloads.join(&name);
            let found = std::fs::read(&path).unwrap_or_else(|e| panic!("{} : {e}", path.display()));
            assert!(
                found == self.expected,
                "{} : contenu faux ({} octets, {} attendus)",
                path.display(),
                found.len(),
                self.expected.len()
            );
            for stream in load_streams(&conn, &task.id).unwrap() {
                let file = self.downloads.join(&stream.file_name);
                assert!(!file.exists(), "{} reste", file.display());
            }
            finals.push(name);
            self.verified += 1;
        }
        // Aucun `.part` à côté d'une vidéo terminée.
        for name in finals {
            let part = self.downloads.join(format!("{name}.part"));
            assert!(!part.exists(), "{} reste", part.display());
        }
    }

    fn dir(&self) -> &Path {
        self.dir.path()
    }
}

#[test]
fn killed_engine_finishes_its_videos_without_corruption() {
    let cycles = env_f64("AURO_CRASH_CYCLES", 6.0) as u64;
    let tasks = env_f64("AURO_CRASH_TASKS", 3.0) as usize;
    let size = env_f64("AURO_CRASH_SIZE", MIB as f64) as usize;
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
        "banc de reprise des vidéos : {cycles} cycles, {tasks} vidéos, flux direct de {size} \
         octets en {seconds} s, graine {seed}"
    );
    // Débit par connexion : le flux direct dure à peu près `seconds` sur deux connexions.
    let chunk = 16 * 1024;
    let per_second = (size as f64 / seconds / 2.0).max(chunk as f64);
    let pace = Pace {
        chunk,
        delay: Duration::from_secs_f64(chunk as f64 / per_second),
    };
    let mut bench = Bench::new(size, pace);
    let mut rng = Rng(seed);
    // Assez long pour qu'une vidéo finisse parfois dans le cycle (flux fragmenté, flux
    // direct, fusion), et que des arrêts tombent dans chaque étape.
    let horizon_ms = ((seconds + 4.0) * 1000.0) as u64;
    let (mut kills, mut shutdowns) = (0, 0);

    for cycle in 0..cycles {
        let mut child = bench.start();
        bench.top_up(tasks);
        thread::sleep(Duration::from_millis(rng.below(horizon_ms)));
        if cycle % 5 == 4 {
            bench.shutdown(&mut child);
            shutdowns += 1;
        } else {
            let _ = child.kill();
            child.wait().unwrap();
            kills += 1;
        }
        bench.check();
    }

    // Fin : tout est repris sans arrêt jusqu'au bout.
    let mut child = bench.start();
    let start = Instant::now();
    while bench.unfinished() > 0 {
        assert!(
            start.elapsed() < Duration::from_secs_f64(90.0 + seconds * 20.0),
            "vidéos inachevées : {}\n{}",
            bench.unfinished_states(),
            bench.log_tail()
        );
        thread::sleep(Duration::from_millis(100));
    }
    bench.shutdown(&mut child);
    bench.check();
    // Plus rien que les vidéos finales dans le dossier.
    let left: Vec<String> = std::fs::read_dir(&bench.downloads)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with("Banc"))
        .collect();
    let finals = std::fs::read_dir(&bench.downloads).unwrap().count();
    eprintln!(
        "{kills} arrêts brutaux, {shutdowns} arrêts demandés, {} vidéos ajoutées, {} arrêts \
         pendant une fusion, {} vidéos terminées et vérifiées octet par octet, {finals} \
         fichiers dans le dossier (dossier {})",
        bench.added,
        bench.in_merge,
        bench.verified,
        bench.dir().display()
    );
    assert!(left.is_empty(), "restent : {left:?}");
    assert_eq!(bench.verified, bench.added);
    if finals as u64 != bench.added {
        // Les noms en trop commencent aussi par « Banc » : tout le dossier, pour les voir.
        let mut all: Vec<String> = std::fs::read_dir(&bench.downloads)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        all.sort();
        panic!(
            "ni flux ni .part restant : {finals} fichiers pour {} vidéos\n{all:#?}",
            bench.added
        );
    }
}
