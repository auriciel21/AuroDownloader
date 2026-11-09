//! Banc de reprise des torrents (SPEC §7.16, phase 5, étape 3c) : `aurod` télécharge
//! plusieurs torrents à la fois depuis un pair local bridé (`testing::swarm`), et il est
//! tué à un instant aléatoire (`TerminateProcess` sous Windows, `SIGKILL` ailleurs), puis
//! relancé, encore et encore ; un cycle sur cinq se termine par un arrêt demandé.
//!
//! Écritures perdues : après un arrêt sur deux, le banc efface une plage d'un `.part` au
//! hasard, comme une coupure de courant qui perd des écritures pas encore vidées sur disque.
//! Le moteur ne se fie à aucun état enregistré : la reprise revérifie les morceaux, et le
//! fichier final doit être juste.
//!
//! Après chaque arrêt, le banc vérifie l'état des tâches (aucune en échec ni annulée) et
//! compare chaque fichier terminé octet par octet ; un torrent terminé ne laisse aucun
//! `.part`.
//!
//! Réglages, par variables d'environnement :
//! - `AURO_CRASH_CYCLES` : lancements d'`aurod` (défaut 6) ;
//! - `AURO_CRASH_TASKS` : torrents gardés en cours (défaut 3) ;
//! - `AURO_CRASH_SIZE` : taille de chaque torrent en octets (défaut 2 Mio) ;
//! - `AURO_CRASH_SECONDS` : durée visée d'un téléchargement sans arrêt (défaut 3) ;
//! - `AURO_CRASH_SEED` : graine du hasard, affichée au départ pour rejouer un échec.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use auro_engine::store::tasks::{TaskKind, TaskState, list_tasks};
use auro_engine::store::torrents::load_files;
use auro_engine::testing::content_range;
use auro_engine::testing::swarm::Seeder;
use base64::Engine as _;
use serde_json::{Value, json};

const MIB: u64 = 1024 * 1024;

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

/// Un torrent ajouté : son nom, et ses fichiers (chemin, taille, décalage de leur contenu).
struct Expected {
    task_id: String,
    single: bool,
    files: Vec<(String, usize, u64)>,
}

/// Un `aurod` du banc, tué s'il tourne encore quand le banc s'arrête (échec compris) :
/// sinon il survit au test et garde ouverte la sortie de celui qui l'a lancé.
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
    runtime: tokio::runtime::Runtime,
    seeder: Seeder,
    expected: Vec<Expected>,
    size: u64,
    /// Plages effacées dans un `.part` (écritures perdues simulées).
    damaged: u64,
    /// Arrêts tombés entre le renommage d'un fichier et l'enregistrement de la fin.
    renamed_unrecorded: u64,
    verified: u64,
}

impl Bench {
    fn new(size: u64, upload_bps: u32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("dl");
        std::fs::create_dir(&downloads).unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let seeder = runtime.block_on(Seeder::start(&dir.path().join("seed"), Some(upload_bps)));
        let endpoint = if cfg!(windows) {
            format!(
                r"\\.\pipe\aurodownloader-banc-torrent-{}",
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
            runtime,
            seeder,
            expected: Vec::new(),
            size,
            damaged: 0,
            renamed_unrecorded: 0,
            verified: 0,
            dir,
        }
    }

    /// Lance `aurod`, branché sur le seul pair local, et attend qu'il réponde.
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
                .args(["--torrent-peers", &self.seeder.addr.to_string()])
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
        // Pas de partage : le banc n'a pas de pair pour le recevoir.
        call(
            &self.endpoint,
            "settings.set",
            json!({"values": {"torrent.ratioLimit": 0}}),
        )
        .expect("settings.set");
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

    /// Les torrents inachevés, pour dire où ils en sont.
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
                    t["pauseReason"]
                )
            })
            .collect::<Vec<_>>()
            .join(" ; ")
    }

    /// Ajoute des torrents jusqu'à en avoir `target` en cours : un fichier, ou plusieurs
    /// (fichiers vides compris), d'environ `size` octets en tout.
    fn top_up(&mut self, target: usize, rng: &mut Rng) {
        let dest = self.downloads.to_str().unwrap().to_owned();
        for _ in self.unfinished()..target {
            let k = self.expected.len();
            let single = rng.below(3) == 0;
            let count = if single { 1 } else { 1 + rng.below(4) as usize };
            let mut files = Vec::new();
            for i in 0..count {
                let len = if !single && rng.below(5) == 0 {
                    0
                } else {
                    (self.size / count as u64 / 2 + rng.below(self.size / count as u64)) as usize
                };
                let rel = if single {
                    String::new()
                } else if i % 2 == 1 {
                    format!("sous/f{i}.bin")
                } else {
                    format!("f{i}.bin")
                };
                let offset = (k as u64) * 1_000_003 + (i as u64) * 7919;
                files.push((rel, len, offset));
            }
            // Au moins un octet : un torrent vide n'a pas de morceau.
            if files.iter().all(|f| f.1 == 0) {
                files[0].1 = 1;
            }
            let name = if single {
                format!("t{k}.bin")
            } else {
                format!("T{k}")
            };
            let contents: Vec<(&str, Vec<u8>)> = files
                .iter()
                .map(|(rel, len, offset)| (rel.as_str(), content_range(*offset, *len)))
                .collect();
            let metainfo = self.runtime.block_on(self.seeder.share(&name, &contents));
            let encoded = base64::engine::general_purpose::STANDARD.encode(&metainfo);
            let added = call(
                &self.endpoint,
                "task.add",
                json!({"sources": [{"torrent": encoded}], "destDir": dest}),
            )
            .expect("task.add");
            self.expected.push(Expected {
                task_id: added["taskId"].as_str().unwrap().to_owned(),
                single,
                files,
            });
        }
    }

    /// Écritures perdues : une plage d'un `.part` au hasard, mise à zéro.
    fn lose_writes(&mut self, rng: &mut Rng) {
        let parts: Vec<PathBuf> = walk(&self.downloads)
            .into_iter()
            .filter(|p| p.to_string_lossy().ends_with(".part"))
            .collect();
        if parts.is_empty() {
            return;
        }
        let part = &parts[rng.below(parts.len() as u64) as usize];
        let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(part) else {
            return;
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len == 0 {
            return;
        }
        let at = rng.below(len);
        let span = (1 + rng.below(64 * 1024)).min(len - at);
        file.seek(SeekFrom::Start(at)).unwrap();
        file.write_all(&vec![0u8; span as usize]).unwrap();
        self.damaged += 1;
    }

    /// Arrêt demandé : `aurod` doit tout enregistrer et se terminer de lui-même.
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

    /// Moteur arrêté : aucune tâche en échec ni annulée ; chaque torrent terminé a tous ses
    /// fichiers, justes octet par octet, et aucun `.part`.
    fn check(&mut self) {
        let conn = auro_engine::store::open(&self.db).unwrap();
        let tasks = list_tasks(&conn).unwrap();
        self.verified = 0;
        for task in &tasks {
            assert_eq!(task.kind, TaskKind::Torrent);
            assert!(
                !matches!(task.state, TaskState::Failed | TaskState::Cancelled),
                "tâche {} : {:?} {:?} {:?}\n{}",
                task.id,
                task.state,
                task.error_code,
                task.error_detail,
                self.log_tail()
            );
            let expected = self
                .expected
                .iter()
                .find(|e| e.task_id == task.id)
                .expect("tâche connue du banc");
            let Some(top) = &task.file_name else {
                continue;
            };
            let top = self.downloads.join(top);
            let paths: Vec<(PathBuf, usize, u64)> = expected
                .files
                .iter()
                .map(|(rel, len, offset)| {
                    let path = if expected.single {
                        top.clone()
                    } else {
                        rel.split('/').fold(top.clone(), |p, c| p.join(c))
                    };
                    (path, *len, *offset)
                })
                .collect();
            if task.state == TaskState::Completed {
                for (path, len, offset) in &paths {
                    let found =
                        std::fs::read(path).unwrap_or_else(|e| panic!("{} : {e}", path.display()));
                    assert!(
                        found == content_range(*offset, *len),
                        "{} : contenu faux ({} octets, {} attendus)",
                        path.display(),
                        found.len(),
                        len
                    );
                    let part = PathBuf::from(format!("{}.part", path.display()));
                    assert!(!part.exists(), "{} reste", part.display());
                }
                assert_eq!(
                    load_files(&conn, &task.id).unwrap().len(),
                    expected.files.len()
                );
                self.verified += 1;
            } else if paths.iter().any(|(p, _, _)| p.is_file()) {
                self.renamed_unrecorded += 1;
            }
        }
    }

    fn dir(&self) -> &Path {
        self.dir.path()
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn killed_engine_finishes_its_torrents_without_corruption() {
    let cycles = env_f64("AURO_CRASH_CYCLES", 6.0) as u64;
    let tasks = env_f64("AURO_CRASH_TASKS", 3.0) as usize;
    let size = env_f64("AURO_CRASH_SIZE", (2 * MIB) as f64) as u64;
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
        "banc de reprise des torrents : {cycles} cycles, {tasks} torrents d'environ {size} \
         octets, {seconds} s, graine {seed}"
    );

    // Le pair local envoie tous les torrents en cours dans le temps visé.
    let upload = (size as f64 * tasks as f64 / seconds).min(u32::MAX as f64) as u32;
    let mut bench = Bench::new(size, upload.max(64 * 1024));
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
        if rng.below(2) == 0 {
            bench.lose_writes(&mut rng);
        }
    }

    // Fin : tout est repris sans arrêt jusqu'au bout.
    let mut child = bench.start();
    let start = Instant::now();
    while bench.unfinished() > 0 {
        assert!(
            start.elapsed() < Duration::from_secs_f64(60.0 + seconds * 20.0),
            "torrents inachevés : {}\n{}",
            bench.unfinished_states(),
            bench.log_tail()
        );
        thread::sleep(Duration::from_millis(100));
    }
    bench.shutdown(&mut child);
    bench.check();
    eprintln!(
        "{kills} arrêts brutaux, {shutdowns} arrêts demandés, {} torrents ajoutés, {} plages \
         effacées dans un .part, {} arrêts entre un renommage et l'enregistrement de la fin, \
         {} torrents terminés et vérifiés octet par octet (dossier {})",
        bench.expected.len(),
        bench.damaged,
        bench.renamed_unrecorded,
        bench.verified,
        bench.dir().display()
    );
    assert_eq!(bench.verified, bench.expected.len() as u64);
}
