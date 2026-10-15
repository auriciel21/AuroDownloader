//! Banc de reprise sous arrêt brutal (SPEC §9.1) : le binaire `auro` télécharge un
//! fichier servi en local, débit bridé, et il est tué à un instant aléatoire
//! (`TerminateProcess` sous Windows, `SIGKILL` ailleurs), puis relancé, encore et encore.
//!
//! Après chaque arrêt, le banc relit la base et le `.part` et vérifie les règles de la
//! SPEC §5.6 : les octets que la base déclare écrits sont dans le `.part`, à leur place
//! (règle 1) ; les segments couvrent le fichier (règle 2) ; `done_bytes` est leur somme
//! (règle 3) ; une tâche terminée a son historique et plus de segments (règle 4). Chaque
//! fichier terminé est comparé octet par octet au contenu servi.
//!
//! Ce banc tue le processus, pas la machine : le cache du système survit. Il éprouve la
//! logique de reprise du moteur, pas l'ordre des écritures face à une coupure de courant.
//!
//! Réglages, par variables d'environnement :
//! - `AURO_CRASH_CYCLES` : lancements du binaire (défaut 15) ;
//! - `AURO_CRASH_SIZE` : taille du fichier en octets (défaut 8 Mio) ;
//! - `AURO_CRASH_SECONDS` : durée visée d'un téléchargement sans arrêt (défaut 3) ;
//! - `AURO_CRASH_SEED` : graine du hasard, affichée au départ pour rejouer un échec.
//!
//! Porte de la phase 1 (SPEC §9.3) :
//! `AURO_CRASH_CYCLES=1000 AURO_CRASH_SIZE=1073741824 AURO_CRASH_SECONDS=8 \
//!  cargo test --release -p auro-cli --test crash_resume -- --nocapture`

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use auro_engine::io::part_path;
use auro_engine::store::{
    self, segments,
    tasks::{self, TaskRow, TaskState},
};
use auro_engine::testing::{FileSite, Pace, content_range, serve_file_paced};

const MIB: u64 = 1024 * 1024;
const CONNECTIONS: &str = "4";

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

enum End {
    Exited(ExitStatus),
    Killed,
}

/// Attend la fin de `child`, ou le tue à `kill_at`.
fn wait_or_kill(child: &mut Child, kill_at: Option<Duration>, timeout: Duration) -> End {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return End::Exited(status);
        }
        let elapsed = start.elapsed();
        if kill_at.is_some_and(|at| elapsed >= at) {
            return match child.kill() {
                Ok(()) => {
                    child.wait().unwrap();
                    End::Killed
                }
                // Terminé entre-temps.
                Err(_) => End::Exited(child.wait().unwrap()),
            };
        }
        if elapsed > timeout {
            let _ = child.kill();
            panic!("auro ne s'est pas terminé en {timeout:?}");
        }
        thread::sleep(Duration::from_millis(2));
    }
}

struct Bench {
    _dir: tempfile::TempDir,
    db: PathBuf,
    downloads: PathBuf,
    log: PathBuf,
    url: String,
    size: u64,
    /// Tâches terminées dont le fichier a été comparé puis supprimé.
    verified: HashSet<String>,
}

impl Bench {
    fn new(url: String, size: u64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("dl");
        std::fs::create_dir(&downloads).unwrap();
        Bench {
            db: dir.path().join("auro.db"),
            log: dir.path().join("auro.log"),
            downloads,
            _dir: dir,
            url,
            size,
            verified: HashSet::new(),
        }
    }

    fn tasks(&self) -> Vec<TaskRow> {
        let conn = store::open(&self.db).unwrap();
        tasks::list_tasks(&conn).unwrap()
    }

    fn unfinished(&self) -> Option<TaskRow> {
        self.tasks()
            .into_iter()
            .find(|t| t.state != TaskState::Completed)
    }

    /// Reprend la tâche inachevée, ou en commence une nouvelle.
    fn launch(&self) -> Child {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_auro"));
        cmd.arg("--db").arg(&self.db);
        match self.unfinished() {
            Some(task) => cmd.args(["resume", &task.id]),
            None => cmd.args(["get", &self.url, "-o"]).arg(&self.downloads),
        };
        cmd.args(["-c", CONNECTIONS])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(&self.log).unwrap())
            .spawn()
            .unwrap()
    }

    fn log_tail(&self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(5)..].join("\n")
    }

    /// Compare `len` octets de `file` à partir de `offset` avec le contenu servi.
    fn same_as_served(file: &Path, offset: u64, len: u64) -> Result<(), String> {
        let mut f = File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
        std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(offset)).unwrap();
        let mut buf = vec![0; MIB as usize];
        let mut done = 0;
        while done < len {
            let n = (len - done).min(MIB) as usize;
            f.read_exact(&mut buf[..n])
                .map_err(|e| format!("lecture à {} : {e}", offset + done))?;
            if buf[..n] != content_range(offset + done, n)[..] {
                return Err(format!(
                    "octets différents entre {} et {}",
                    offset + done,
                    offset + done + n as u64
                ));
            }
            done += n as u64;
        }
        Ok(())
    }

    /// Règles de la SPEC §5.6 après un arrêt ; compare et supprime les fichiers terminés.
    fn check(&mut self) {
        let conn = store::open(&self.db).unwrap();
        let broken = segments::find_coverage_errors(&conn).unwrap();
        assert!(broken.is_empty(), "règle 2 : {broken:?}");

        for task in tasks::list_tasks(&conn).unwrap() {
            let segs = segments::load_segments(&conn, &task.id).unwrap();
            match task.state {
                TaskState::Completed => {
                    assert!(segs.is_empty(), "règle 4 : segments restants");
                    let history: i64 = conn
                        .query_row(
                            "SELECT COUNT(*) FROM history WHERE task_id = ?1",
                            [&task.id],
                            |r| r.get(0),
                        )
                        .unwrap();
                    assert_eq!(history, 1, "règle 4 : historique");
                    if self.verified.contains(&task.id) {
                        continue;
                    }
                    let name = task.file_name.as_deref().expect("nom du fichier");
                    let path = self.downloads.join(name);
                    let len = std::fs::metadata(&path)
                        .unwrap_or_else(|e| panic!("fichier terminé absent : {e}"))
                        .len();
                    assert_eq!(len, self.size, "taille du fichier final");
                    assert_eq!(task.done_bytes, self.size);
                    if let Err(e) = Self::same_as_served(&path, 0, len) {
                        panic!("fichier final corrompu : {e}");
                    }
                    assert!(!part_path(&path).exists(), ".part restant");
                    std::fs::remove_file(&path).unwrap();
                    self.verified.insert(task.id.clone());
                }
                TaskState::Queued | TaskState::Active => {
                    let sum: u64 = segs.iter().map(|s| s.written).sum();
                    assert_eq!(task.done_bytes, sum, "règle 3");
                    let Some(name) = task.file_name.as_deref() else {
                        assert!(segs.is_empty(), "segments sans préparation");
                        continue;
                    };
                    let part = part_path(&self.downloads.join(name));
                    for s in segs.iter().filter(|s| s.written > 0) {
                        if let Err(e) = Self::same_as_served(&part, s.start, s.written) {
                            panic!(
                                "règle 1 : segment {} ({}-{}, {} octets annoncés) : {e}",
                                s.idx, s.start, s.end, s.written
                            );
                        }
                    }
                }
                other => panic!(
                    "état inattendu {other:?} : {:?} {:?}\n{}",
                    task.error_code,
                    task.error_detail,
                    self.log_tail()
                ),
            }
        }
    }
}

#[test]
fn killed_downloads_resume_without_corruption() {
    let cycles = env_f64("AURO_CRASH_CYCLES", 15.0) as u64;
    let size = env_f64("AURO_CRASH_SIZE", (8 * MIB) as f64) as u64;
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
    eprintln!("banc de reprise : {cycles} cycles, {size} octets, {seconds} s, graine {seed}");

    // Débit par connexion tel qu'un téléchargement complet dure `seconds`.
    let chunk = 16 * 1024;
    let per_connection = size as f64 / seconds / CONNECTIONS.parse::<f64>().unwrap();
    let pace = Pace {
        chunk,
        delay: Duration::from_secs_f64(chunk as f64 / per_connection.max(1.0)),
    };
    let server = serve_file_paced(Arc::new(Mutex::new(FileSite::new(size))), Some(pace));
    let mut bench = Bench::new(server.url("/banc.bin"), size);
    let mut rng = Rng(seed);
    let horizon_ms = (seconds * 1000.0) as u64;
    let timeout = Duration::from_secs_f64(30.0 + seconds * 10.0);
    let (mut kills, mut resumed_with_progress) = (0, 0);

    for cycle in 0..cycles {
        if bench.unfinished().is_some_and(|t| t.done_bytes > 0) {
            resumed_with_progress += 1;
        }
        let mut child = bench.launch();
        // Un lancement sur dix va jusqu'au bout.
        let kill_at = (cycle % 10 != 9).then(|| Duration::from_millis(rng.below(horizon_ms)));
        match wait_or_kill(&mut child, kill_at, timeout) {
            End::Killed => kills += 1,
            End::Exited(status) => assert!(
                status.success(),
                "cycle {cycle} : auro a échoué ({status})\n{}",
                bench.log_tail()
            ),
        }
        bench.check();
    }

    // Fin : la tâche en cours est reprise sans arrêt jusqu'au bout.
    if bench.unfinished().is_some() {
        let mut child = bench.launch();
        match wait_or_kill(&mut child, None, timeout) {
            End::Exited(status) => assert!(status.success(), "{}", bench.log_tail()),
            End::Killed => unreachable!(),
        }
        bench.check();
    }
    assert!(bench.unfinished().is_none());
    eprintln!(
        "{kills} arrêts brutaux, {resumed_with_progress} reprises avec des octets déjà \
         enregistrés, {} fichiers terminés et vérifiés octet par octet",
        bench.verified.len()
    );
    assert!(!bench.verified.is_empty());
}
