//! Banc de performance face à aria2 (SPEC §9.1 et N-01), ignoré par défaut :
//!
//! `AURO_ARIA2=chemin/vers/aria2c cargo test --release -p auro-cli --test perf_vs_aria2 \
//!  -- --ignored --nocapture`
//!
//! Chaque scénario télécharge le même fichier avec `auro` et avec `aria2c`, à nombre de
//! connexions égal, dans un dossier neuf, et mesure le temps écoulé et le temps processeur
//! du processus (utilisateur et système). Le résultat est un tableau : débit, rapport des
//! débits auro / aria2 (la porte de la phase 1 demande au moins 0,95), rapport des coûts
//! processeur par octet auro / aria2 (porte de N-01 : au plus 1,0), et part d'un cœur que
//! coûterait un débit de 1 Gbit/s (objectif indicatif de N-01 : moins de 15 %, sur la
//! machine de référence). Un « ⚠ » marque un rapport hors porte. Les portes ne
//! s'appliquent qu'à nombre de connexions égal, pas au mode automatique, et ne font échouer
//! le banc qu'avec `AURO_PERF_ENFORCE=1` : c'est le cas sous Linux en intégration
//! continue, où se mesure la porte (SPEC §9.1). Ailleurs, le tableau sert à décider.
//!
//! Scénarios : serveur local sans bridage (coût processeur) et bridé par connexion (gain
//! des connexions multiples), chacun en HTTP puis en HTTPS, et un serveur réel si
//! `AURO_PERF_REAL_URL` est donnée. Sans bridage, curl sert de référence (une connexion,
//! vers le disque ou sans rien écrire) : il sépare le coût du système (réseau, disque,
//! antivirus) de celui des outils, et rappelle qu'auro vide ses données sur disque
//! (règle 1), pas curl. En HTTPS, le serveur présente un certificat signé par une autorité
//! créée pour le banc, que chaque outil reçoit en option (sauf aria2 sous Windows, voir
//! `Tool::command`).
//!
//! Réglages : `AURO_PERF_SIZE` (octets, défaut 256 Mio), `AURO_PERF_RUNS` (mesures par
//! outil et scénario, défaut 3 ; la médiane est retenue), `AURO_PERF_REAL_URL`,
//! `AURO_PERF_LOGS` (dossier où garder la sortie d'erreurs de chaque lancement, pour
//! examiner l'avancement affiché par auro), `AURO_PERF_ENFORCE`.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use auro_engine::testing::{
    FileSite, Pace, Server, TestTls, content_range, serve_file_paced, serve_file_tls,
};

const MIB: u64 = 1024 * 1024;

/// Porte de débit de la phase 1 : auro au moins à 95 % d'aria2 (SPEC §9.1).
const GATE_SPEED: f64 = 0.95;
/// Porte de N-01 : coût processeur par octet au plus égal à celui d'aria2 (SPEC §3).
const GATE_CPU: f64 = 1.0;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map(|v| {
            v.parse()
                .unwrap_or_else(|_| panic!("{name} : entier attendu, reçu {v:?}"))
        })
        .unwrap_or(default)
}

/// Temps processeur d'un processus terminé mais pas encore libéré.
///
/// `GetProcessTimes` compte le temps par échantillonnage à chaque tic d'horloge (environ
/// 15,6 ms) : un processus qui s'endort et se réveille sans cesse pour ses entrées-sorties
/// y est mal compté, dans un sens comme dans l'autre. Les cycles de `QueryProcessCycleTime`
/// sont exacts ; ils sont convertis en secondes avec la fréquence du compteur de cycles,
/// étalonnée une fois.
#[cfg(windows)]
fn cpu_time(child: &Child) -> Duration {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::WindowsProgramming::QueryProcessCycleTime;

    let mut cycles = 0u64;
    // SAFETY : la poignée du processus reste valide tant que `child` existe ; `cycles` est
    // une variable locale.
    let ok = unsafe { QueryProcessCycleTime(child.as_raw_handle(), &mut cycles) };
    assert_ne!(ok, 0, "QueryProcessCycleTime a échoué");
    Duration::from_secs_f64(cycles as f64 / cycles_per_second())
}

/// Fréquence du compteur de cycles, mesurée sur ce fil en tournant 300 ms à vide. Un fil
/// interrompu pendant l'étalonnage donne une fréquence plus basse, donc des temps
/// processeur plus hauts : l'erreur va dans le sens prudent.
#[cfg(windows)]
fn cycles_per_second() -> f64 {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Threading::GetCurrentThread;
    use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;

    static RATE: OnceLock<f64> = OnceLock::new();
    *RATE.get_or_init(|| {
        let (mut before, mut after) = (0u64, 0u64);
        // SAFETY : pseudo-poignée du fil courant, variables locales.
        unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut before) };
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(300) {
            std::hint::spin_loop();
        }
        let elapsed = start.elapsed().as_secs_f64();
        // SAFETY : idem.
        unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut after) };
        (after - before) as f64 / elapsed
    })
}

/// Temps processeur cumulé des processus enfants déjà attendus (utilisateur et système).
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

    /// Temps processeur par octet, en secondes.
    fn cpu_per_byte(&self) -> f64 {
        self.cpu.as_secs_f64() / self.bytes as f64
    }

    /// Part d'un cœur que coûterait 1 Gbit/s (125 Mo/s) au même coût par octet.
    fn core_at_gbit(&self) -> f64 {
        self.cpu_per_byte() * 125e6 * 100.0
    }
}

#[derive(Debug, Clone, Copy)]
enum Tool {
    /// `None` : nombre de connexions automatique.
    Auro(Option<u32>),
    Aria2(u32),
    /// Référence : curl, une seule connexion, vers le disque (`true`) ou sans rien écrire.
    /// Elle sépare le coût du système (réseau, disque, antivirus) de celui des outils.
    Curl(bool),
}

impl Tool {
    fn label(self) -> String {
        match self {
            Tool::Auro(None) => "auro (auto)".into(),
            Tool::Auro(Some(n)) => format!("auro -c {n}"),
            Tool::Aria2(n) => format!("aria2c -x {n}"),
            Tool::Curl(true) => "curl (disque)".into(),
            Tool::Curl(false) => "curl (sans disque)".into(),
        }
    }

    /// `ca` : autorité du serveur HTTPS de test, en PEM.
    fn command(self, url: &str, dir: &Path, name: &str, ca: Option<&Path>) -> Command {
        match self {
            Tool::Auro(connections) => {
                let mut cmd = Command::new(env!("CARGO_BIN_EXE_auro"));
                cmd.arg("--db").arg(dir.join("auro.db"));
                if let Some(ca) = ca {
                    cmd.arg("--ca-cert").arg(ca);
                }
                cmd.args(["get", url, "-o"]).arg(dir);
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
                    .args(["--summary-interval=0", "--download-result=hide"]);
                match ca {
                    // La version Windows d'aria2 passe par la pile TLS du système, qui
                    // ignore `--ca-certificate` : sans vérification, ce qui ne coûte
                    // qu'une poignée de main par connexion et joue en faveur d'aria2.
                    Some(_) if cfg!(windows) => {
                        cmd.arg("--check-certificate=false");
                    }
                    Some(ca) => {
                        cmd.arg(format!("--ca-certificate={}", ca.display()));
                    }
                    None => {}
                }
                cmd.arg("-d").arg(dir).args(["-o", name, url]);
                cmd
            }
            Tool::Curl(to_disk) => {
                // Sous Windows, le curl livré avec le système, pas celui d'un autre outil.
                let curl = match std::env::var_os("SystemRoot") {
                    Some(root) if cfg!(windows) => {
                        Path::new(&root).join("System32").join("curl.exe")
                    }
                    _ => PathBuf::from("curl"),
                };
                let mut cmd = Command::new(curl);
                if let Some(ca) = ca {
                    cmd.arg("--cacert").arg(ca);
                    // Sous Windows (Schannel), la vérification de révocation échoue faute
                    // de liste de révocation pour l'autorité de test.
                    if cfg!(windows) {
                        cmd.arg("--ssl-no-revoke");
                    }
                }
                cmd.args(["-s", "-f", "-o"]);
                if to_disk {
                    cmd.arg(dir.join(name));
                } else {
                    cmd.arg(if cfg!(windows) { "NUL" } else { "/dev/null" })
                        .args(["-w", "%{size_download}"]);
                }
                cmd.arg(url);
                cmd
            }
        }
    }

    /// Un téléchargement mesuré, dans un dossier neuf ; renvoie la mesure et le fichier.
    fn measure(
        self,
        url: &str,
        name: &str,
        ca: Option<&Path>,
    ) -> (Measure, tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = self.command(url, dir.path(), name, ca);
        let discard = matches!(self, Tool::Curl(false));
        cmd.stdin(Stdio::null())
            .stdout(if discard {
                Stdio::piped()
            } else {
                Stdio::null()
            })
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
        let bytes = if discard {
            // curl écrit le nombre d'octets reçus (`-w`).
            let mut out = String::new();
            child
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut out)
                .unwrap();
            out.trim().parse().unwrap()
        } else {
            std::fs::metadata(&path)
                .unwrap_or_else(|e| panic!("{} : fichier absent : {e}", self.label()))
                .len()
        };
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

/// Un scénario : un serveur, les outils à comparer et l'autorité de son certificat.
struct Scenario<'a> {
    name: &'static str,
    url: &'a str,
    file: &'a str,
    tools: &'a [Tool],
    ca: Option<&'a Path>,
}

/// Mesure chaque outil `runs` fois, en alternant les outils pour répartir les variations
/// de la machine, et garde la médiane.
fn scenario(rows: &mut Vec<Row>, s: Scenario<'_>, runs: u64, check: &dyn Fn(&Path, u64) -> bool) {
    let mut samples: Vec<Vec<Measure>> = vec![Vec::new(); s.tools.len()];
    for _ in 0..runs {
        for (i, tool) in s.tools.iter().enumerate() {
            let (m, _dir, path) = tool.measure(s.url, s.file, s.ca);
            let checked = matches!(tool, Tool::Curl(false)) || check(&path, m.bytes);
            assert!(checked, "{} : contenu faux", tool.label());
            samples[i].push(m);
        }
    }
    for (tool, samples) in s.tools.iter().zip(samples) {
        let measure = median(samples);
        eprintln!(
            "  {:<32} {:<18} {:>8.1} Mio/s  {:>6.2} s  cpu {:>5.2} s",
            s.name,
            tool.label(),
            measure.mib_per_s(),
            measure.wall.as_secs_f64(),
            measure.cpu.as_secs_f64()
        );
        rows.push(Row {
            scenario: s.name,
            tool: *tool,
            measure,
        });
    }
}

/// Mesure d'aria2 qui sert de référence à `row` : même scénario, même nombre de
/// connexions, ou le plus rapide des aria2 pour le mode automatique.
fn reference<'a>(rows: &'a [Row], row: &Row) -> Option<&'a Measure> {
    rows.iter()
        .filter(|r| r.scenario == row.scenario)
        .filter(|r| match (row.tool, r.tool) {
            (Tool::Auro(Some(n)), Tool::Aria2(m)) => n == m,
            (Tool::Auro(None), Tool::Aria2(_)) => true,
            _ => false,
        })
        .map(|r| &r.measure)
        .max_by(|a, b| a.mib_per_s().total_cmp(&b.mib_per_s()))
}

/// Affiche le tableau ; renvoie les portes manquées à nombre de connexions égal.
fn report(rows: &[Row]) -> Vec<String> {
    let mark = |ok: bool| if ok { "" } else { " ⚠" };
    let mut missed = Vec::new();
    eprintln!();
    eprintln!(
        "| Scénario | Outil | Débit (Mio/s) | Débit auro / aria2 | Processeur par octet \
         auro / aria2 | Cœur à 1 Gbit/s |\n| --- | --- | --- | --- | --- | --- |"
    );
    for row in rows {
        let (speed, cpu) = match (row.tool, reference(rows, row)) {
            (Tool::Auro(connections), Some(aria2)) => {
                let speed = row.measure.mib_per_s() / aria2.mib_per_s();
                let cpu = row.measure.cpu_per_byte() / aria2.cpu_per_byte();
                let (speed_ok, cpu_ok) = (speed >= GATE_SPEED, cpu <= GATE_CPU);
                if connections.is_some() && !(speed_ok && cpu_ok) {
                    missed.push(format!(
                        "{}, {} : débit {speed:.2} (porte {GATE_SPEED}), processeur \
                         {cpu:.2} (porte {GATE_CPU})",
                        row.scenario,
                        row.tool.label()
                    ));
                }
                (
                    format!("{speed:.2}{}", mark(speed_ok)),
                    format!("{cpu:.2}{}", mark(cpu_ok)),
                )
            }
            _ => (String::new(), String::new()),
        };
        eprintln!(
            "| {} | {} | {:.1} | {speed} | {cpu} | {:.1} % |",
            row.scenario,
            row.tool.label(),
            row.measure.mib_per_s(),
            row.measure.core_at_gbit()
        );
    }
    missed
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

    // Autorité du serveur HTTPS, écrite dans un fichier que chaque outil lira.
    let tls = TestTls::new();
    let ca_dir = tempfile::tempdir().unwrap();
    let ca = ca_dir.path().join("autorite-de-test.pem");
    std::fs::write(&ca, &tls.ca_pem).unwrap();
    let site = || Arc::new(Mutex::new(FileSite::new(size)));
    let pace = Pace {
        chunk: 64 * 1024,
        delay: Duration::from_millis(8),
    };
    let free_tools = [
        Tool::Auro(Some(4)),
        Tool::Aria2(4),
        Tool::Curl(true),
        Tool::Curl(false),
    ];
    let paced_tools = [Tool::Auro(Some(16)), Tool::Auro(None), Tool::Aria2(16)];

    // 1. Sans bridage : le coût processeur domine. 2. Bridé à 8 Mio/s par connexion : le
    // gain vient des connexions multiples. Chacun en HTTP, puis en HTTPS.
    let servers: [(&'static str, Server, &[Tool], Option<&Path>); 4] = [
        (
            "local, sans bridage",
            serve_file_paced(site(), None),
            &free_tools,
            None,
        ),
        (
            "local HTTPS, sans bridage",
            serve_file_tls(site(), None, &tls),
            &free_tools,
            Some(&ca),
        ),
        (
            "local, 8 Mio/s par connexion",
            serve_file_paced(site(), Some(pace)),
            &paced_tools,
            None,
        ),
        (
            "local HTTPS, 8 Mio/s par connexion",
            serve_file_tls(site(), Some(pace), &tls),
            &paced_tools,
            Some(&ca),
        ),
    ];
    for (name, server, tools, ca) in &servers {
        let url = server.url("/banc.bin");
        let s = Scenario {
            name,
            url: &url,
            file: "banc.bin",
            tools,
            ca: *ca,
        };
        scenario(&mut rows, s, runs, &served);
    }

    // 3. Serveur réel, si demandé ; les outils doivent obtenir le même fichier.
    if let Ok(url) = std::env::var("AURO_PERF_REAL_URL") {
        let name = url.rsplit('/').next().unwrap_or("reel.bin").to_owned();
        let any_size = |_: &Path, bytes: u64| bytes > 0;
        let s = Scenario {
            name: "serveur réel",
            url: &url,
            file: &name,
            tools: &paced_tools,
            ca: None,
        };
        scenario(&mut rows, s, runs, &any_size);
    }

    let missed = report(&rows);
    if !missed.is_empty() {
        eprintln!("\nportes manquées :\n  {}", missed.join("\n  "));
        let enforce = std::env::var("AURO_PERF_ENFORCE").is_ok_and(|v| v == "1");
        assert!(!enforce, "portes de performance manquées (SPEC §9.1)");
    }
}
