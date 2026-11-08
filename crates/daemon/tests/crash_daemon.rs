//! Banc de reprise du moteur autonome (SPEC §9.1, phase 2) : `aurod` exécute plusieurs
//! tâches à la fois, servies en local à débit bridé, et il est tué à un instant
//! aléatoire (`TerminateProcess` sous Windows, `SIGKILL` ailleurs), puis relancé, encore
//! et encore. À chaque lancement, le banc ajoute des tâches par le protocole pour en
//! garder toujours plusieurs en cours ; un cycle sur cinq se termine par un arrêt
//! demandé (`engine.shutdown`) au lieu d'un arrêt brutal. Une tâche sur deux porte
//! l'empreinte du fichier (F-11) : des arrêts tombent pendant son calcul (`verifying`).
//!
//! Après chaque arrêt, le banc vérifie les règles de la SPEC §5.6 sur la base et les
//! `.part` (`testing::Checker`), et compare chaque fichier terminé octet par octet.
//!
//! Réglages, par variables d'environnement :
//! - `AURO_CRASH_CYCLES` : lancements d'`aurod` (défaut 8) ;
//! - `AURO_CRASH_TASKS` : tâches gardées en cours (défaut 4 ; trois tournent à la fois) ;
//! - `AURO_CRASH_SIZE` : taille de chaque fichier en octets (défaut 4 Mio) ;
//! - `AURO_CRASH_SECONDS` : durée visée d'un téléchargement sans arrêt (défaut 3) ;
//! - `AURO_CRASH_SEED` : graine du hasard, affichée au départ pour rejouer un échec.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use auro_engine::testing::{Checker, FileSite, Pace, content_sha256, serve_file_paced};
use serde_json::{Value, json};

const MIB: u64 = 1024 * 1024;
const CONNECTIONS: f64 = 4.0;
const CONNECTION_CHOICES: [Option<u32>; 5] = [None, Some(1), Some(2), Some(4), Some(8)];

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
fn open(endpoint: &str) -> std::io::Result<(Box<dyn std::io::Read>, Box<dyn Write>)> {
    let pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(endpoint)?;
    Ok((Box::new(pipe.try_clone()?), Box::new(pipe)))
}

#[cfg(unix)]
fn open(endpoint: &str) -> std::io::Result<(Box<dyn std::io::Read>, Box<dyn Write>)> {
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

struct Bench {
    dir: tempfile::TempDir,
    endpoint: String,
    log: PathBuf,
    base_url: String,
    checker: Checker,
    added: u64,
    /// Empreinte du fichier servi.
    sha256: String,
}

impl Bench {
    fn new(base_url: String, size: u64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("dl");
        std::fs::create_dir(&downloads).unwrap();
        let endpoint = if cfg!(windows) {
            format!(
                r"\\.\pipe\aurodownloader-banc-{}",
                auro_engine::store::new_id()
            )
        } else {
            dir.path().join("aurod.sock").to_string_lossy().into_owned()
        };
        Bench {
            checker: Checker::new(dir.path().join("auro.db"), downloads, size),
            log: dir.path().join("aurod.log"),
            endpoint,
            base_url,
            dir,
            added: 0,
            sha256: content_sha256(size),
        }
    }

    /// Lance `aurod` et attend qu'il réponde.
    fn start(&self) -> Child {
        let log = File::options()
            .create(true)
            .append(true)
            .open(&self.log)
            .unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_aurod"))
            .arg("--db")
            .arg(&self.checker.db)
            .args(["--endpoint", &self.endpoint])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
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

    /// Ajoute des tâches jusqu'à en avoir `target` en cours.
    fn top_up(&mut self, target: usize, rng: &mut Rng) {
        let dest = self.checker.downloads.to_str().unwrap().to_owned();
        for _ in self.unfinished()..target {
            self.added += 1;
            let url = format!("{}/f{}.bin", self.base_url, self.added);
            let mut params = json!({"sources": [{"url": url}], "destDir": dest});
            // Une tâche sur deux a son empreinte (F-11) : des arrêts tombent pendant son
            // calcul.
            if self.added % 2 == 0 {
                params["checksum"] = json!({"algo": "sha256", "value": self.sha256});
            }
            if let Some(n) = CONNECTION_CHOICES[rng.below(CONNECTION_CHOICES.len() as u64) as usize]
            {
                params["maxConnections"] = json!(n);
            }
            call(&self.endpoint, "task.add", params).expect("task.add");
        }
    }

    /// Arrêt demandé : `aurod` doit tout enregistrer et se terminer de lui-même.
    fn shutdown(&self, child: &mut Child) {
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

    fn check(&mut self) {
        let log = self.log_tail();
        self.checker.check(&|| log.clone());
    }

    fn dir(&self) -> &Path {
        self.dir.path()
    }
}

#[test]
fn killed_engine_resumes_all_its_tasks_without_corruption() {
    let cycles = env_f64("AURO_CRASH_CYCLES", 8.0) as u64;
    let tasks = env_f64("AURO_CRASH_TASKS", 4.0) as usize;
    let size = env_f64("AURO_CRASH_SIZE", (4 * MIB) as f64) as u64;
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
        "banc de reprise d'aurod : {cycles} cycles, {tasks} tâches de {size} octets, \
         {seconds} s, graine {seed}"
    );

    let chunk = 16 * 1024;
    let per_connection = size as f64 / seconds / CONNECTIONS;
    let pace = Pace {
        chunk,
        delay: Duration::from_secs_f64(chunk as f64 / per_connection.max(1.0)),
    };
    let server = serve_file_paced(Arc::new(Mutex::new(FileSite::new(size))), Some(pace));
    let mut bench = Bench::new(server.url(""), size);
    let mut rng = Rng(seed);
    let horizon_ms = (seconds * 1000.0) as u64;
    let (mut kills, mut shutdowns) = (0, 0);

    for cycle in 0..cycles {
        let mut child = bench.start();
        bench.top_up(tasks, &mut rng);
        thread::sleep(Duration::from_millis(rng.below(horizon_ms)));
        if cycle % 5 == 4 {
            bench.shutdown(&mut child);
            shutdowns += 1;
        } else {
            // Déjà terminé si une erreur l'a arrêté : le contrôle le dira.
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
            start.elapsed() < Duration::from_secs_f64(60.0 + seconds * 20.0),
            "tâches inachevées\n{}",
            bench.log_tail()
        );
        thread::sleep(Duration::from_millis(100));
    }
    bench.shutdown(&mut child);
    bench.check();
    eprintln!(
        "{kills} arrêts brutaux, {shutdowns} arrêts demandés, {} tâches ajoutées, \
         {} arrêts sur un découpage redécoupé, {} arrêts entre le renommage et \
         l'enregistrement de la fin, {} arrêts pendant le calcul de l'empreinte, {} fichiers \
         terminés et vérifiés octet par octet (dossier {})",
        bench.added,
        bench.checker.split_layouts,
        bench.checker.renamed_unrecorded,
        bench.checker.checking,
        bench.checker.verified.len(),
        bench.dir().display()
    );
    assert_eq!(bench.checker.verified.len() as u64, bench.added);
}
