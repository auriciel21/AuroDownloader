//! Banc de performance face à aria2 (SPEC §9.1 et N-01), ignoré par défaut :
//!
//! `AURO_ARIA2=chemin/vers/aria2c cargo test --release -p auro-cli --test perf_vs_aria2 \
//!  -- --ignored --nocapture`
//!
//! Chaque scénario télécharge le même fichier avec `auro` et avec `aria2c`, à nombre de
//! connexions égal, dans un dossier neuf, et mesure le temps écoulé et le temps processeur
//! du processus (utilisateur et système). Le résultat est un tableau : débit, rapport
//! auro / aria2 (la porte de la phase 1 demande au moins 0,95), et part d'un cœur que
//! coûterait un débit de 1 Gbit/s (N-01 : moins de 15 %). Rien n'est imposé : les mesures
//! dépendent de la machine, le tableau sert à décider.
//!
//! Scénarios : serveur local sans bridage (coût processeur), serveur local bridé par
//! connexion (gain des connexions multiples), et un serveur réel si `AURO_PERF_REAL_URL`
//! est donnée.
//!
//! Réglages : `AURO_PERF_SIZE` (octets, défaut 256 Mio), `AURO_PERF_RUNS` (mesures par
//! outil et scénario, défaut 3 ; la médiane est retenue), `AURO_PERF_REAL_URL`,
//! `AURO_PERF_LOGS` (dossier où garder la sortie d'erreurs de chaque lancement, pour
//! examiner l'avancement affiché par auro).

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use auro_engine::testing::{FileSite, Pace, content_range, serve_file_paced};

const MIB: u64 = 1024 * 1024;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map(|v| {
            v.parse()
                .unwrap_or_else(|_| panic!("{name} : entier attendu, reçu {v:?}"))
        })
        .unwrap_or(default)
}

/// Temps processeur (utilisateur et système) d'un processus terminé mais pas encore
/// libéré.
#[cfg(windows)]
fn cpu_time(child: &Child) -> Duration {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;

    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY : la poignée du processus reste valide tant que `child` existe ; les quatre
    // pointeurs désignent des variables locales.
    let ok = unsafe {
        GetProcessTimes(
            child.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    assert_ne!(ok, 0, "GetProcessTimes a échoué");
    // Unités de 100 ns.
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

/// Temps processeur cumulé des processus enfants déjà attendus.
#[cfg(unix)]
fn children_cpu() -> Duration {
    // SAFETY : `usage` est une variable locale que getrusage remplit.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        assert_eq!(libc::getrusage(libc::RUSAGE_CHILDREN, &mut usage), 0);
        usage
    };
    let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    tv(usage.ru_utime) + tv(usage.ru_stime)
}

#[derive(Debug, Clone, Copy)]
struct Measure {
    wall: Duration,
    cpu: Duration,
    bytes: u64,
}

impl Measure {
    fn mib_per_s(&self) -> f64 {
        self.bytes as f64 / MIB as f64 / self.wall.as_secs_f64()
    }

    /// Part d'un cœur que coûterait 1 Gbit/s (125 Mo/s) au même coût par octet.
    fn core_at_gbit(&self) -> f64 {
        self.cpu.as_secs_f64() / self.bytes as f64 * 125e6 * 100.0
    }
}

#[derive(Debug, Clone, Copy)]
enum Tool {
    /// `None` : nombre de connexions automatique.
    Auro(Option<u32>),
    Aria2(u32),
}

impl Tool {
    fn label(self) -> String {
        match self {
            Tool::Auro(None) => "auro (auto)".into(),
            Tool::Auro(Some(n)) => format!("auro -c {n}"),
            Tool::Aria2(n) => format!("aria2c -x {n}"),
        }
    }

    fn command(self, url: &str, dir: &Path, name: &str) -> Command {
        match self {
            Tool::Auro(connections) => {
                let mut cmd = Command::new(env!("CARGO_BIN_EXE_auro"));
                cmd.arg("--db")
                    .arg(dir.join("auro.db"))
                    .args(["get", url, "-o"])
                    .arg(dir);
                if let Some(n) = connections {
                    cmd.args(["-c", &n.to_string()]);
                }
                cmd
            }
            Tool::Aria2(n) => {
                let aria2 = std::env::var("AURO_ARIA2").unwrap_or_else(|_| "aria2c".into());
                let mut cmd = Command::new(aria2);
                // Même découpage minimal que le moteur (1 Mio), pas de préallocation (le
                // moteur n'en fait pas), pas d'écrasement ni de renommage.
                cmd.args(["-x", &n.to_string(), "-s", &n.to_string(), "-k", "1M"])
                    .args(["--file-allocation=none", "--allow-overwrite=true"])
                    .args(["--auto-file-renaming=false", "--console-log-level=warn"])
                    .args(["--summary-interval=0", "--download-result=hide"])
                    .arg("-d")
                    .arg(dir)
                    .args(["-o", name, url]);
                cmd
            }
        }
    }

    /// Un téléchargement mesuré, dans un dossier neuf ; renvoie la mesure et le fichier.
    fn measure(self, url: &str, name: &str) -> (Measure, tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = self.command(url, dir.path(), name);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr_for(self));
        #[cfg(unix)]
        let before = children_cpu();
        let start = Instant::now();
        let mut child = cmd
            .spawn()
            .unwrap_or_else(|e| panic!("{} : {e}", self.label()));
        let status = child.wait().unwrap();
        let wall = start.elapsed();
        assert!(status.success(), "{} a échoué : {status}", self.label());
        #[cfg(windows)]
        let cpu = cpu_time(&child);
        #[cfg(unix)]
        let cpu = children_cpu() - before;
        let path = dir.path().join(name);
        let bytes = std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("{} : fichier absent : {e}", self.label()))
            .len();
        (Measure { wall, cpu, bytes }, dir, path)
    }
}

/// Sortie d'erreurs d'un lancement : gardée dans `AURO_PERF_LOGS` si ce dossier est donné.
fn stderr_for(tool: Tool) -> Stdio {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static RUN: AtomicUsize = AtomicUsize::new(0);
    match std::env::var_os("AURO_PERF_LOGS") {
        Some(dir) => {
            let n = RUN.fetch_add(1, Ordering::SeqCst);
            let name = format!("{n:03}-{}.log", tool.label().replace(' ', "_"));
            File::create(Path::new(&dir).join(name)).unwrap().into()
        }
        None => Stdio::null(),
    }
}

/// Le fichier est-il le contenu servi par le serveur de test ?
fn same_as_served(path: &Path, len: u64) -> bool {
    let mut f = File::open(path).unwrap();
    let mut buf = vec![0; MIB as usize];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(MIB) as usize;
        if f.read_exact(&mut buf[..n]).is_err() || buf[..n] != content_range(done, n)[..] {
            return false;
        }
        done += n as u64;
    }
    true
}

fn median(mut values: Vec<Measure>) -> Measure {
    values.sort_by_key(|m| m.wall);
    values[values.len() / 2]
}

struct Row {
    scenario: &'static str,
    tool: Tool,
    measure: Measure,
}

/// Mesure chaque outil `runs` fois, en alternant les outils pour répartir les variations
/// de la machine, et garde la médiane.
fn scenario(
    rows: &mut Vec<Row>,
    name: &'static str,
    url: &str,
    file: &str,
    tools: &[Tool],
    runs: u64,
    check: &dyn Fn(&Path, u64) -> bool,
) {
    let mut samples: Vec<Vec<Measure>> = vec![Vec::new(); tools.len()];
    for _ in 0..runs {
        for (i, tool) in tools.iter().enumerate() {
            let (m, _dir, path) = tool.measure(url, file);
            assert!(check(&path, m.bytes), "{} : contenu faux", tool.label());
            samples[i].push(m);
        }
    }
    for (tool, s) in tools.iter().zip(samples) {
        let measure = median(s);
        eprintln!(
            "  {name:<26} {:<14} {:>8.1} Mio/s  {:>6.2} s  cpu {:>6.2} s",
            tool.label(),
            measure.mib_per_s(),
            measure.wall.as_secs_f64(),
            measure.cpu.as_secs_f64()
        );
        rows.push(Row {
            scenario: name,
            tool: *tool,
            measure,
        });
    }
}

fn report(rows: &[Row]) {
    eprintln!();
    eprintln!(
        "| Scénario | Outil | Débit (Mio/s) | auro / aria2 | Cœur à 1 Gbit/s |\n| --- | --- | --- | --- | --- |"
    );
    for row in rows {
        // Référence : aria2 dans le même scénario, au même nombre de connexions, ou le
        // plus rapide des aria2 pour le mode automatique.
        let reference = rows
            .iter()
            .filter(|r| r.scenario == row.scenario)
            .filter_map(|r| match (row.tool, r.tool) {
                (Tool::Auro(Some(n)), Tool::Aria2(m)) if n == m => Some(r.measure.mib_per_s()),
                (Tool::Auro(None), Tool::Aria2(_)) => Some(r.measure.mib_per_s()),
                _ => None,
            })
            .fold(None, |best: Option<f64>, v| {
                Some(best.map_or(v, |b| b.max(v)))
            });
        let ratio = match (row.tool, reference) {
            (Tool::Auro(_), Some(reference)) => {
                let ratio = row.measure.mib_per_s() / reference;
                let verdict = if ratio >= 0.95 { "" } else { " ⚠" };
                format!("{ratio:.2}{verdict}")
            }
            _ => "".into(),
        };
        eprintln!(
            "| {} | {} | {:.1} | {} | {:.1} % |",
            row.scenario,
            row.tool.label(),
            row.measure.mib_per_s(),
            ratio,
            row.measure.core_at_gbit()
        );
    }
}

#[test]
#[ignore = "banc de performance : demande aria2c et le mode release"]
fn throughput_and_cpu_against_aria2() {
    if cfg!(debug_assertions) {
        eprintln!("attention : mesures en mode debug, à refaire avec --release");
    }
    let size = env_u64("AURO_PERF_SIZE", 256 * MIB);
    let runs = env_u64("AURO_PERF_RUNS", 3).max(1);
    let served = |path: &Path, bytes: u64| bytes == size && same_as_served(path, size);
    let mut rows = Vec::new();

    eprintln!("banc de performance : {size} octets, {runs} mesures par outil (médiane)");

    // 1. Sans bridage : le coût processeur domine.
    let free = serve_file_paced(Arc::new(Mutex::new(FileSite::new(size))), None);
    scenario(
        &mut rows,
        "local, sans bridage",
        &free.url("/banc.bin"),
        "banc.bin",
        &[Tool::Auro(Some(4)), Tool::Aria2(4)],
        runs,
        &served,
    );

    // 2. Bridé à 8 Mio/s par connexion : le gain vient des connexions multiples.
    let pace = Pace {
        chunk: 64 * 1024,
        delay: Duration::from_millis(8),
    };
    let paced = serve_file_paced(Arc::new(Mutex::new(FileSite::new(size))), Some(pace));
    scenario(
        &mut rows,
        "local, 8 Mio/s par connexion",
        &paced.url("/banc.bin"),
        "banc.bin",
        &[Tool::Auro(Some(16)), Tool::Auro(None), Tool::Aria2(16)],
        runs,
        &served,
    );

    // 3. Serveur réel, si demandé ; les deux outils doivent obtenir le même fichier.
    if let Ok(url) = std::env::var("AURO_PERF_REAL_URL") {
        let name = url.rsplit('/').next().unwrap_or("reel.bin").to_owned();
        let any_size = |_: &Path, bytes: u64| bytes > 0;
        scenario(
            &mut rows,
            "serveur réel",
            &url,
            &name,
            &[Tool::Auro(Some(16)), Tool::Auro(None), Tool::Aria2(16)],
            runs,
            &any_size,
        );
    }

    report(&rows);
}
