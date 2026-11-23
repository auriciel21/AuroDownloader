//! Journaux du moteur (N-12, phase 7, étape 2) : un événement par ligne, en JSON (date,
//! niveau, module, message, champs), dans `logs/` à côté de la base. Rotation par taille :
//! [`KEEP`] fichiers de [`MAX_FILE`] au plus. Niveau `info` par défaut, réglage
//! `logs.level` appliqué sans redémarrer ; la variable [`ENV`] le remplace.
//!
//! Ce qui s'y écrit ne nomme jamais une adresse complète ni un en-tête (SPEC 8.2) : un
//! hôte au plus. Les bibliothèques (librqbit…) n'y écrivent rien : leurs messages peuvent
//! porter des adresses de pairs. L'export de diagnostic ([`scrub`]) retire en plus les
//! hôtes, les chemins et le nom de l'utilisateur.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use serde_json::Value;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::reload;

/// Dossier des journaux, à côté de la base.
pub const DIR: &str = "logs";
/// Fichier courant ; les anciens sont `aurod.1.jsonl`, `aurod.2.jsonl`…
pub const FILE: &str = "aurod.jsonl";
/// Taille d'un fichier de journal, au plus.
pub const MAX_FILE: u64 = 5 << 20;
/// Fichiers gardés, le courant compris.
pub const KEEP: usize = 5;
/// Variable d'environnement qui impose le niveau (`error`, `warn`, `info`, `debug`).
pub const ENV: &str = "AURO_LOG";

/// Modules dont les événements vont au journal ; les autres crates n'y écrivent rien.
const OURS: [&str; 4] = ["auro_engine", "aurod", "auro_android", "auro_cli"];

/// Niveau de détail des journaux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::Error, Level::Warn, Level::Info, Level::Debug];

    pub fn parse(text: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.as_str() == text)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
        }
    }

    fn filter(self) -> LevelFilter {
        match self {
            Level::Error => LevelFilter::ERROR,
            Level::Warn => LevelFilter::WARN,
            Level::Info => LevelFilter::INFO,
            Level::Debug => LevelFilter::DEBUG,
        }
    }
}

/// Le dossier des journaux d'une base.
pub fn dir_for(db: &Path) -> PathBuf {
    std::path::absolute(db)
        .ok()
        .and_then(|db| db.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join(DIR)
}

/// Chemin du `n`-ième fichier (0 : le courant).
fn file_at(dir: &Path, n: usize) -> PathBuf {
    if n == 0 {
        dir.join(FILE)
    } else {
        dir.join(format!("aurod.{n}.jsonl"))
    }
}

/// Les fichiers de journal présents, du plus ancien au plus récent.
pub fn files(dir: &Path) -> Vec<PathBuf> {
    (0..KEEP)
        .rev()
        .map(|n| file_at(dir, n))
        .filter(|p| p.is_file())
        .collect()
}

struct RotatingState {
    dir: PathBuf,
    file: File,
    size: u64,
    max: u64,
    keep: usize,
    /// Rotation ratée : pas de nouvel essai avant cet instant.
    retry_at: Option<std::time::Instant>,
}

/// Délai avant de retenter une rotation ratée.
const ROTATE_RETRY: std::time::Duration = std::time::Duration::from_secs(60);

impl RotatingState {
    /// Le fichier courant devient `aurod.1.jsonl`, et ainsi de suite ; le plus ancien part.
    fn rotate(&mut self) -> io::Result<()> {
        let _ = std::fs::remove_file(file_at(&self.dir, self.keep - 1));
        for n in (0..self.keep - 1).rev() {
            let from = file_at(&self.dir, n);
            if from.exists() {
                std::fs::rename(&from, file_at(&self.dir, n + 1))?;
            }
        }
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file_at(&self.dir, 0))?;
        self.size = 0;
        Ok(())
    }
}

/// Fichier de journal à rotation par taille. Chaque événement est écrit d'un seul tenant.
pub struct Rotating {
    state: Mutex<RotatingState>,
}

impl Rotating {
    pub fn open(dir: &Path, max: u64, keep: usize) -> io::Result<Rotating> {
        std::fs::create_dir_all(dir)?;
        let path = file_at(dir, 0);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Rotating {
            state: Mutex::new(RotatingState {
                dir: dir.to_path_buf(),
                file,
                size,
                max,
                keep: keep.max(1),
                retry_at: None,
            }),
        })
    }

    fn write_event(&self, buf: &[u8]) -> io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let due = state
            .retry_at
            .is_none_or(|at| std::time::Instant::now() >= at);
        if state.size > 0 && state.size + buf.len() as u64 > state.max && due {
            // Rotation impossible (fichier tenu par un autre programme, dossier en lecture
            // seule) : l'événement s'écrit quand même dans le fichier courant, et la rotation
            // se retente plus tard (audit R16 : tous les suivants étaient perdus).
            state.retry_at = match state.rotate() {
                Ok(()) => None,
                Err(_) => Some(std::time::Instant::now() + ROTATE_RETRY),
            };
        }
        state.file.write_all(buf)?;
        state.size += buf.len() as u64;
        Ok(())
    }
}

/// Écrivain d'un événement, remis par [`Rotating`] : ce qu'il reçoit, en un ou plusieurs
/// morceaux, s'écrit d'un seul tenant quand il est rendu (jamais coupé par une rotation).
pub struct EventWriter<'a> {
    log: &'a Rotating,
    buf: Vec<u8>,
}

impl Write for EventWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for EventWriter<'_> {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            let _ = self.log.write_event(&self.buf);
        }
    }
}

impl<'a> MakeWriter<'a> for Rotating {
    type Writer = EventWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        EventWriter {
            log: self,
            buf: Vec::new(),
        }
    }
}

/// Filtre d'un niveau : nos modules à ce niveau, rien des autres.
fn targets(level: Level) -> Targets {
    OURS.iter()
        .fold(Targets::new(), |t, target| {
            t.with_target(*target, level.filter())
        })
        .with_default(LevelFilter::OFF)
}

type Handle = reload::Handle<Targets, tracing_subscriber::Registry>;

struct Installed {
    handle: Handle,
    dir: PathBuf,
    /// Niveau imposé par [`ENV`] : le réglage ne le change pas.
    forced: bool,
}

static INSTALLED: OnceLock<Installed> = OnceLock::new();

/// Installe les journaux du processus dans `dir`, au niveau `info` (ou celui de [`ENV`]).
/// Une seule fois par processus ; une deuxième installation ne change rien.
pub fn init(dir: &Path) -> io::Result<()> {
    if INSTALLED.get().is_some() {
        return Ok(());
    }
    let forced = std::env::var(ENV).ok().and_then(|v| Level::parse(v.trim()));
    let (filter, handle) = reload::Layer::new(targets(forced.unwrap_or(Level::Info)));
    let writer = Rotating::open(dir, MAX_FILE, KEEP)?;
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_target(true)
        .with_writer(writer);
    tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init()
        .map_err(io::Error::other)?;
    let _ = INSTALLED.set(Installed {
        handle,
        dir: dir.to_path_buf(),
        forced: forced.is_some(),
    });
    Ok(())
}

/// Applique le niveau du réglage `logs.level`, sauf si [`ENV`] l'impose ; sans effet si
/// les journaux ne sont pas installés (tests, CLI).
pub fn set_level(level: Level) {
    if let Some(installed) = INSTALLED.get()
        && !installed.forced
    {
        let _ = installed.handle.reload(targets(level));
    }
}

/// Dossier des journaux installés, s'il y en a.
pub fn installed_dir() -> Option<&'static Path> {
    INSTALLED.get().map(|i| i.dir.as_path())
}

/// Suit le réglage `logs.level` du moteur, jusqu'à son arrêt.
pub async fn follow(engine: crate::scheduler::Engine) {
    use crate::scheduler::Event;
    let mut events = engine.subscribe();
    if let Ok(level) = engine.logs_level().await {
        set_level(level);
    }
    loop {
        match events.recv().await {
            Ok(Event::SettingChanged { key, value, .. })
                if key == crate::store::settings::LOGS_LEVEL =>
            {
                if let Some(level) = value.as_str().and_then(Level::parse) {
                    set_level(level);
                    tracing::info!(niveau = level.as_str(), "niveau des journaux changé");
                }
            }
            Ok(Event::StateChanged {
                task_id,
                from,
                to,
                error_code,
                pause_reason,
                ..
            }) => {
                tracing::info!(
                    tache = %task_id,
                    de = from.as_sql(),
                    vers = to.as_sql(),
                    erreur = error_code.as_deref(),
                    pause = pause_reason.as_deref(),
                    "état d'une tâche"
                );
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(lost)) => {
                tracing::warn!(perdus = lost, "événements du moteur perdus par le journal");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Ce que le diagnostic masque : chemins (dossier personnel, données, téléchargements)
/// et nom de l'utilisateur, remplacés par un nom générique.
#[derive(Debug, Default, Clone)]
pub struct Masks {
    pairs: Vec<(String, &'static str)>,
}

impl Masks {
    /// Les chemins et le nom de cette machine.
    pub fn of_machine(data_dir: Option<&Path>, download_dir: Option<&str>) -> Masks {
        let mut masks = Masks::default();
        if let Some(dir) = download_dir {
            masks.add(dir, "<téléchargements>");
        }
        if let Some(dir) = data_dir {
            masks.add(&dir.to_string_lossy(), "<données>");
        }
        for var in ["USERPROFILE", "HOME"] {
            if let Ok(home) = std::env::var(var) {
                masks.add(&home, "<dossier personnel>");
            }
        }
        for var in ["USERNAME", "USER"] {
            if let Ok(user) = std::env::var(var) {
                masks.add(&user, "<utilisateur>");
            }
        }
        masks
    }

    /// Masque `text` (au moins 3 caractères, pour ne pas masquer n'importe quoi).
    pub fn add(&mut self, text: &str, name: &'static str) {
        let text = text.trim_end_matches(['/', '\\']);
        if text.chars().count() >= 3 && !self.pairs.iter().any(|(t, _)| t == text) {
            self.pairs.push((text.to_owned(), name));
            // Les plus longs d'abord : un chemin avant le nom qu'il contient.
            self.pairs.sort_by_key(|(t, _)| std::cmp::Reverse(t.len()));
        }
    }

    fn apply(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (from, to) in &self.pairs {
            // Les chemins Windows, tels quels et avec leurs barres doublées par JSON.
            for variant in [
                from.clone(),
                from.replace('\\', "\\\\"),
                from.replace('\\', "/"),
            ] {
                if variant.len() >= 3 {
                    out = replace_ignoring_case(&out, &variant, to);
                }
            }
        }
        out
    }
}

fn replace_ignoring_case(text: &str, from: &str, to: &str) -> String {
    let lower = text.to_lowercase();
    let needle = from.to_lowercase();
    // Repli simple si la mise en minuscules change les longueurs (rare hors ASCII).
    if lower.len() != text.len() || needle.len() != from.len() {
        return text.replace(from, to);
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find(&needle) {
        out.push_str(&text[at..at + found]);
        out.push_str(to);
        at += found + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

/// Protocoles d'adresse remplacés par leur seul nom (proxys, WebSocket et fichiers
/// locaux depuis l'audit R10).
const SCHEMES: [&str; 11] = [
    "https://",
    "http://",
    "ftps://",
    "ftp://",
    "sftp://",
    "magnet:",
    "socks5h://",
    "socks5://",
    "wss://",
    "ws://",
    "file://",
];

/// Nettoie un texte pour le diagnostic : adresses réduites à leur protocole, jetons de
/// l'interface distante retirés, chemins et nom de l'utilisateur masqués.
pub fn scrub_text(text: &str, masks: &Masks) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while !rest.is_empty() {
        for scheme in SCHEMES {
            if rest.len() >= scheme.len()
                && rest.is_char_boundary(scheme.len())
                && rest[..scheme.len()].eq_ignore_ascii_case(scheme)
            {
                let end = rest
                    .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')'))
                    .unwrap_or(rest.len());
                out.push_str(scheme);
                out.push('…');
                rest = &rest[end..];
                continue 'scan;
            }
        }
        if rest.starts_with(crate::remote::TOKEN_PREFIX)
            && rest.len() >= crate::remote::TOKEN_PREFIX.len() + 43
            && rest.is_char_boundary(crate::remote::TOKEN_PREFIX.len() + 43)
            && crate::remote::looks_like_secret(&rest[..crate::remote::TOKEN_PREFIX.len() + 43])
        {
            out.push_str("auro_…");
            rest = &rest[crate::remote::TOKEN_PREFIX.len() + 43..];
            continue;
        }
        let c = rest.chars().next().expect("non vide");
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    cut_paths(&masks.apply(&out))
}

/// Noms donnés aux dossiers masqués (voir [`Masks::of_machine`]).
const PLACEHOLDERS: [&str; 4] = [
    "<téléchargements>",
    "<données>",
    "<dossier personnel>",
    "<utilisateur>",
];

/// Coupe les chemins : ce qui suit un dossier masqué (`<données>\f.iso` devient
/// `<données>\…`), et tout autre chemin absolu (`C:\…`, `/home/…`) devient `<chemin>`. Un
/// chemin finit à un blanc ou à un guillemet.
fn cut_paths(text: &str) -> String {
    let end_of = |s: &str| {
        s.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | ')'))
            .unwrap_or(s.len())
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut boundary = true;
    'scan: while let Some(c) = rest.chars().next() {
        for name in PLACEHOLDERS {
            if let Some(after) = rest.strip_prefix(name)
                && after.starts_with(['\\', '/'])
            {
                out.push_str(name);
                out.push_str(&after[..1]);
                out.push('…');
                rest = &after[end_of(after)..];
                boundary = false;
                continue 'scan;
            }
        }
        let bytes = rest.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/');
        let unix = c == '/' && rest[1..end_of(rest).max(1)].contains('/');
        // Chemins réseau et étendus de Windows : `\\serveur\partage`, `\\?\C:\…` (audit R10).
        let unc = rest.starts_with("\\\\");
        if boundary && (drive || unix || unc) {
            out.push_str("<chemin>");
            rest = &rest[end_of(rest)..];
            boundary = false;
            continue;
        }
        boundary = c.is_whitespace() || matches!(c, '"' | '\'' | '(' | '=');
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Nettoie une ligne de journal : chaque texte nettoyé, les hôtes (`host`, `hote`)
/// retirés. Une ligne illisible est nettoyée comme un texte.
pub fn scrub(line: &str, masks: &Masks) -> String {
    fn walk(value: &mut Value, masks: &Masks) {
        match value {
            Value::String(s) => *s = scrub_text(s, masks),
            Value::Array(items) => items.iter_mut().for_each(|v| walk(v, masks)),
            Value::Object(map) => {
                for (key, v) in map.iter_mut() {
                    if matches!(key.as_str(), "host" | "hote" | "hôte") {
                        *v = Value::from("<hôte>");
                    } else {
                        walk(v, masks);
                    }
                }
            }
            _ => {}
        }
    }
    match serde_json::from_str::<Value>(line) {
        Ok(mut value) => {
            walk(&mut value, masks);
            value.to_string()
        }
        Err(_) => scrub_text(line, masks),
    }
}

/// Les dernières lignes des journaux de `dir` (au plus `max_bytes`), nettoyées.
pub fn collect(dir: &Path, masks: &Masks, max_bytes: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for path in files(dir) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            lines.extend(text.lines().map(str::to_owned));
        }
    }
    let mut kept = Vec::new();
    let mut size = 0;
    for line in lines.into_iter().rev() {
        size += line.len() + 1;
        if size > max_bytes {
            break;
        }
        kept.push(scrub(&line, masks));
    }
    kept.reverse();
    kept
}

/// Écrit le diagnostic (résultat d'`engine.diagnostic`) dans une archive : `rapport.json`
/// et `journaux.jsonl`. Un fichier déjà là n'est pas écrasé.
pub fn write_archive(path: &Path, diagnostic: &Value) -> io::Result<()> {
    use zip::write::SimpleFileOptions;
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("rapport.json", options)
        .map_err(io::Error::other)?;
    let report = serde_json::to_string_pretty(&diagnostic["report"]).map_err(io::Error::other)?;
    zip.write_all(report.as_bytes())?;
    zip.start_file("journaux.jsonl", options)
        .map_err(io::Error::other)?;
    for line in diagnostic["logs"].as_array().into_iter().flatten() {
        if let Some(line) = line.as_str() {
            zip.write_all(line.as_bytes())?;
            zip.write_all(b"\n")?;
        }
    }
    zip.finish().map_err(io::Error::other)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diagnostic_archive_holds_the_report_and_the_logs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("diagnostic.zip");
        let diagnostic = serde_json::json!({"report": {"tasks": {"count": 2}}, "logs": ["{\"n\":1}", "{\"n\":2}"]});
        write_archive(&path, &diagnostic).unwrap();
        // Jamais par-dessus un fichier existant.
        assert!(write_archive(&path, &diagnostic).is_err());
        let mut zip = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let mut read = |name: &str| {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut zip.by_name(name).unwrap(), &mut text).unwrap();
            text
        };
        let report: Value = serde_json::from_str(&read("rapport.json")).unwrap();
        assert_eq!(report["tasks"]["count"], 2);
        assert_eq!(read("journaux.jsonl"), "{\"n\":1}\n{\"n\":2}\n");
    }

    #[test]
    fn files_rotate_by_size_and_keep_a_few() {
        let dir = tempfile::tempdir().unwrap();
        let log = Rotating::open(dir.path(), 100, 3).unwrap();
        for i in 0..20 {
            writeln!(
                log.make_writer(),
                "{{\"n\":{i},\"remplissage\":\"0123456789\"}}"
            )
            .unwrap();
        }
        let found = files(dir.path());
        assert_eq!(found.len(), 3, "{found:?}");
        for path in &found {
            let len = std::fs::metadata(path).unwrap().len();
            assert!(len <= 100, "{path:?} : {len}");
            // Chaque ligne entière, lisible.
            for line in std::fs::read_to_string(path).unwrap().lines() {
                serde_json::from_str::<Value>(line).unwrap();
            }
        }
        // Le plus récent est le dernier.
        let last = std::fs::read_to_string(found.last().unwrap()).unwrap();
        assert!(last.contains("\"n\":19"), "{last}");
        // Reprise après un redémarrage : la taille du fichier courant est relue.
        let again = Rotating::open(dir.path(), 100, 3).unwrap();
        let size = again.state.lock().unwrap().size;
        assert_eq!(
            size,
            std::fs::metadata(dir.path().join(FILE)).unwrap().len()
        );
    }

    /// Audit R16 : une rotation impossible (ici, un dossier à la place du fichier suivant)
    /// ne fait perdre aucun événement : ils restent dans le fichier courant.
    #[test]
    fn a_failed_rotation_keeps_the_events() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(file_at(dir.path(), 1)).unwrap();
        let log = Rotating::open(dir.path(), 100, 2).unwrap();
        for i in 0..10 {
            writeln!(
                log.make_writer(),
                "{{\"n\":{i},\"remplissage\":\"0123456789\"}}"
            )
            .unwrap();
        }
        let current = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        for i in 0..10 {
            assert!(
                current.contains(&format!("\"n\":{i},")),
                "{i} perdu : {current}"
            );
        }
    }

    #[test]
    fn levels_are_read_and_written() {
        for level in Level::ALL {
            assert_eq!(Level::parse(level.as_str()), Some(level));
        }
        assert_eq!(Level::parse("trace"), None);
        assert_eq!(Level::parse("INFO"), None);
    }

    #[test]
    fn the_diagnostic_removes_addresses_tokens_hosts_and_paths() {
        let mut masks = Masks::default();
        masks.add(r"C:\Users\alice", "<dossier personnel>");
        masks.add("alice", "<utilisateur>");
        masks.add("/home/alice/Téléchargements", "<téléchargements>");
        let token = format!("auro_{}", "A".repeat(43));
        let line = serde_json::json!({
            "timestamp": "2026-11-12T10:00:00Z",
            "level": "WARN",
            "fields": {
                "message": format!("échec de https://exemple.org/f.iso?jeton=1 et magnet:?xt=urn:btih:abc ; jeton {token}"),
                "host": "exemple.org",
                "chemin": r"C:\Users\alice\Downloads\f.iso",
                "dossier": "/home/alice/Téléchargements/ISO",
                "qui": "ALICE",
            }
        })
        .to_string();
        let scrubbed = scrub(&line, &masks);
        for leak in [
            "exemple.org",
            "jeton=1",
            "btih",
            &token,
            "alice",
            "ALICE",
            "Users\\\\alice",
        ] {
            assert!(!scrubbed.contains(leak), "{leak} dans {scrubbed}");
        }
        for kept in [
            "https://…",
            "magnet:…",
            "auro_…",
            "<hôte>",
            "<dossier personnel>",
            "<téléchargements>",
            "<utilisateur>",
            "WARN",
        ] {
            assert!(scrubbed.contains(kept), "{kept} absent de {scrubbed}");
        }
        // Une ligne qui n'est pas du JSON est nettoyée comme un texte.
        assert_eq!(scrub("voir http://a.b/c", &masks), "voir http://…");
        // Chemins : coupés après un dossier connu, remplacés sinon ; le reste du texte
        // reste lisible.
        for (text, expected) in [
            (
                r"C:\Users\alice\Downloads\rapport.pdf",
                r"<dossier personnel>\…",
            ),
            (
                r"écrit dans D:\ISO\debian.iso : accès refusé",
                "écrit dans <chemin> : accès refusé",
            ),
            (
                "lu /srv/partage/film.mkv puis rien",
                "lu <chemin> puis rien",
            ),
            ("dossier=/var/lib/x", "dossier=<chemin>"),
            ("ratio 3/4 et http://h/x", "ratio 3/4 et http://…"),
            // Audit R10 : chemins réseau et étendus de Windows, proxys, WebSocket, fichiers.
            (r"lu \\nas\partage\film.mkv puis", "lu <chemin> puis"),
            (r"ouvert \\?\C:\très\long\f.iso", "ouvert <chemin>"),
            (
                "proxy socks5://moi:secret@p:1080 refusé",
                "proxy socks5://… refusé",
            ),
            (
                "par socks5h://p:1080 et wss://h/ws",
                "par socks5h://… et wss://…",
            ),
            ("ws://h:7380/ws coupé", "ws://… coupé"),
            ("voir file:///home/x/f.iso", "voir file://…"),
        ] {
            assert_eq!(scrub_text(text, &masks), expected, "{text}");
        }
    }

    #[test]
    fn collected_lines_are_the_last_ones_and_scrubbed() {
        let dir = tempfile::tempdir().unwrap();
        let log = Rotating::open(dir.path(), 200, 3).unwrap();
        for i in 0..10 {
            writeln!(
                log.make_writer(),
                "{{\"n\":{i},\"url\":\"https://x.org/{i}\"}}"
            )
            .unwrap();
        }
        let lines = collect(dir.path(), &Masks::default(), 120);
        assert!(!lines.is_empty() && lines.len() < 10, "{lines:?}");
        assert!(lines.last().unwrap().contains("\"n\":9"));
        assert!(lines.iter().all(|l| !l.contains("x.org")), "{lines:?}");
    }
}
