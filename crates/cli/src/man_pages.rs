//! Pages de manuel (phase 7, étape 9a) : `docs/man/<commande>.1`, une par commande et
//! sous-commande (`auro-engine-start.1`, à la manière de git), faites depuis les définitions
//! de clap par clap_mangen. Inclus par les tests d'`auro` et d'`aurod` : un test échoue si une
//! page n'est plus celle que donnent les commandes. Ni section VERSION ni version dans
//! l'en-tête : une nouvelle version ne change pas les pages.
//!
//! Régénérer : `AURO_UPDATE_MAN=1 cargo test -p auro-cli -p auro-daemon man_pages`.

use std::path::{Path, PathBuf};

/// `docs/man` : les deux crates sont à deux niveaux sous la racine du dépôt.
pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/man")
}

/// Pages d'une commande et de ses sous-commandes visibles, triées par nom de fichier.
pub fn render(cmd: clap::Command) -> Vec<(String, Vec<u8>)> {
    fn walk(cmd: clap::Command, out: &mut Vec<(String, Vec<u8>)>) {
        for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()).cloned() {
            walk(sub, out);
        }
        let has_args = cmd.get_arguments().any(|a| !a.is_hide_set());
        let has_subs = cmd.get_subcommands().any(|s| !s.is_hide_set());
        let has_after = cmd.get_after_help().is_some() || cmd.get_after_long_help().is_some();
        let man = clap_mangen::Man::new(cmd)
            .source("AuroDownloader")
            .manual("AuroDownloader");
        let mut page = Vec::new();
        man.render_title(&mut page).unwrap();
        man.render_name_section(&mut page).unwrap();
        man.render_synopsis_section(&mut page).unwrap();
        man.render_description_section(&mut page).unwrap();
        if has_args {
            man.render_options_section(&mut page).unwrap();
        }
        if has_subs {
            man.render_subcommands_section(&mut page).unwrap();
        }
        if has_after {
            man.render_extra_section(&mut page).unwrap();
        }
        out.push((man.get_filename(), page));
    }
    let mut cmd = cmd.disable_help_subcommand(true);
    cmd.build();
    let mut out = Vec::new();
    walk(cmd, &mut out);
    out.sort();
    out
}

/// Pages de `docs/man` d'une commande (`auro.1`, `auro-….1`).
pub fn existing(prefix: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| {
            n == &format!("{prefix}.1")
                || (n.starts_with(&format!("{prefix}-")) && n.ends_with(".1"))
        })
        .collect();
    names.sort();
    names
}

/// Vérifie les pages d'une commande ; avec `AURO_UPDATE_MAN`, les réécrit (et retire celles
/// d'une sous-commande disparue).
pub fn check(prefix: &str, cmd: clap::Command) {
    let pages = render(cmd);
    let dir = dir();
    let known = |name: &str| pages.iter().any(|(n, _)| n == name);
    if std::env::var_os("AURO_UPDATE_MAN").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        for stale in existing(prefix).into_iter().filter(|n| !known(n)) {
            std::fs::remove_file(dir.join(stale)).unwrap();
        }
        for (name, page) in &pages {
            std::fs::write(dir.join(name), page).unwrap();
        }
        return;
    }
    let mut faults = Vec::new();
    for (name, page) in &pages {
        match std::fs::read(dir.join(name)) {
            Ok(bytes) if bytes == *page => {}
            Ok(_) => faults.push(format!("{name} : différente des commandes")),
            Err(_) => faults.push(format!("{name} : absente")),
        }
    }
    for stale in existing(prefix).into_iter().filter(|n| !known(n)) {
        faults.push(format!("{stale} : commande disparue"));
    }
    assert!(
        faults.is_empty(),
        "pages de manuel pas à jour (AURO_UPDATE_MAN=1 cargo test -p auro-cli -p auro-daemon man_pages) :\n{}",
        faults.join("\n")
    );
}
