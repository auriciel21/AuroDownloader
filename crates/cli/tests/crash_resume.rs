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
//! Chaque lancement tire son nombre de connexions (automatique, 1, 2, 4 ou 8) : une reprise
//! avec plus de connexions que le départ redécoupe les segments en cours (F-02, SPEC §7.3).
//!
//! Porte de la phase 1 (SPEC §9.3) :
//! `AURO_CRASH_CYCLES=1000 AURO_CRASH_SIZE=1073741824 AURO_CRASH_SECONDS=8 \
//!  cargo test --release -p auro-cli --test crash_resume -- --nocapture`

use std::fs::File;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use auro_engine::store::{
    self,
    tasks::{self, TaskRow, TaskState},
};
use auro_engine::testing::{Checker, FileSite, Pace, serve_file_paced};

const MIB: u64 = 1024 * 1024;
/// Connexions de référence pour régler le débit ; chaque lancement en tire un nombre parmi
/// [`CONNECTION_CHOICES`] (`None` : automatique) : une reprise avec plus de connexions que
/// le départ coupe les segments en cours (redécoupage dynamique, F-02).
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
    log: PathBuf,
    url: String,
    checker: Checker,
}

impl Bench {
    fn new(url: String, size: u64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("dl");
        std::fs::create_dir(&downloads).unwrap();
        Bench {
            checker: Checker::new(dir.path().join("auro.db"), downloads, size),
            log: dir.path().join("auro.log"),
            _dir: dir,
            url,
        }
    }

    fn unfinished(&self) -> Option<TaskRow> {
        let conn = store::open(&self.checker.db).unwrap();
        tasks::list_tasks(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.state != TaskState::Completed)
    }

    /// Reprend la tâche inachevée, ou en commence une nouvelle.
    fn launch(&self, connections: Option<u32>) -> Child {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_auro"));
        cmd.args(["direct", "--db"]).arg(&self.checker.db);
        match self.unfinished() {
            Some(task) => cmd.args(["resume", &task.id]),
            None => cmd
                .args(["get", &self.url, "-o"])
                .arg(&self.checker.downloads),
        };
        if let Some(n) = connections {
            cmd.args(["-c", &n.to_string()]);
        }
        cmd.stdin(Stdio::null())
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

    fn check(&mut self) {
        let log = self.log_tail();
        self.checker.check(&|| log.clone());
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
    let per_connection = size as f64 / seconds / CONNECTIONS;
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
        let connections = CONNECTION_CHOICES[rng.below(CONNECTION_CHOICES.len() as u64) as usize];
        let mut child = bench.launch(connections);
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
        let mut child = bench.launch(None);
        match wait_or_kill(&mut child, None, timeout) {
            End::Exited(status) => assert!(status.success(), "{}", bench.log_tail()),
            End::Killed => unreachable!(),
        }
        bench.check();
    }
    assert!(bench.unfinished().is_none());
    eprintln!(
        "{kills} arrêts brutaux, {resumed_with_progress} reprises avec des octets déjà \
         enregistrés, {} arrêts sur un découpage redécoupé, {} arrêts entre le renommage \
         et l'enregistrement de la fin, {} fichiers terminés et vérifiés octet par octet",
        bench.checker.split_layouts,
        bench.checker.renamed_unrecorded,
        bench.checker.verified.len()
    );
    assert!(!bench.checker.verified.is_empty());
    bench.checker.assert_no_strays();
}
