//! Audit R3 : `aurod` lancé avec `AURO_ENDPOINT` (navigateur d'essai, relais) et sans
//! `--db` refuse de démarrer, au lieu d'ouvrir la base par défaut, celle du moteur réel.
//!
//! Le dossier des données est redirigé vers un dossier temporaire : même si le garde
//! manquait, le test n'ouvrirait jamais la vraie base de l'utilisateur.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn auro_endpoint_without_a_database_refuses_to_start() {
    let data = tempfile::tempdir().unwrap();
    let endpoint = if cfg!(windows) {
        format!(r"\\.\pipe\auro-essai-r3-{}", std::process::id())
    } else {
        data.path()
            .join("essai.sock")
            .to_string_lossy()
            .into_owned()
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_aurod"))
        .env("AURO_ENDPOINT", &endpoint)
        .env("LOCALAPPDATA", data.path())
        .env("XDG_DATA_HOME", data.path())
        .env("HOME", data.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(20) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let Some(status) = status else {
        // Il a démarré : arrêté par son propre identifiant, jamais par son nom.
        let _ = child.kill();
        let _ = child.wait();
        panic!("aurod a démarré sans --db sur le point d'accès {endpoint}");
    };
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains("AURO_ENDPOINT"), "{stderr}");
    let left: Vec<_> = walk(data.path());
    assert!(
        !left.iter().any(|p| p.ends_with("auro.db")),
        "aucune base créée : {left:?}"
    );
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}
