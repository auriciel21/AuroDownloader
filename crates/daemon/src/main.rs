//! `aurod` : moteur d'AuroDownloader en processus autonome (SPEC §4.6).
//!
//! Il exécute les téléchargements et sert le protocole JSON-RPC (SPEC §6) sur le point
//! d'accès local de l'utilisateur : canal nommé sous Windows, socket Unix ailleurs. Les
//! interfaces (bureau, CLI) s'y connectent ; les téléchargements continuent sans elles.
//! Il s'arrête proprement sur Ctrl-C, sur SIGTERM (Unix) ou par `engine.shutdown`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use auro_engine::media;
use auro_engine::net::Client;
use auro_engine::paths;
use auro_engine::power;
use auro_engine::rpc::local;
use auro_engine::scheduler::{Config, DISK_RECHECK, Engine, Planning};
use auro_engine::secrets::Vault;
use auro_engine::torrent;
use auro_engine::transfer::Options;
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "aurod",
    version,
    about = "AuroDownloader : moteur de téléchargement",
    disable_help_flag = true,
    disable_version_flag = true
)]
struct Args {
    /// Affiche l'aide
    #[arg(short = 'h', long = "help", action = clap::ArgAction::Help)]
    help: Option<bool>,

    /// Affiche la version
    #[arg(short = 'V', long = "version", action = clap::ArgAction::Version)]
    version: Option<bool>,

    /// Base du moteur (par défaut, dans le dossier de données de l'utilisateur)
    #[arg(long, value_name = "FICHIER")]
    db: Option<PathBuf>,

    /// Point d'accès local : canal nommé (Windows) ou chemin du socket (Unix)
    #[arg(long, value_name = "NOM")]
    endpoint: Option<String>,

    /// Autorités de certification à ajouter à celles du système (fichier PEM). La
    /// validation des certificats reste stricte.
    #[arg(long, value_name = "FICHIER")]
    ca_cert: Option<PathBuf>,

    /// Essais : l'action de fin d'une planification (veille, extinction) est écrite dans
    /// le journal au lieu d'être exécutée
    #[arg(long)]
    simulate_power: bool,

    /// Essais : les torrents passent par ces seuls pairs (adresse:port, séparées par des
    /// virgules), sans DHT ni connexion entrante (banc de reprise des torrents)
    #[arg(long, value_name = "PAIRS", value_delimiter = ',')]
    torrent_peers: Vec<std::net::SocketAddr>,

    /// Essais : ce yt-dlp, pris tel quel, sans installation ni mise à jour (banc de reprise
    /// des vidéos) ; avec --media-ffmpeg et --media-ffprobe
    #[arg(long, value_name = "FICHIER", requires_all = ["media_ffmpeg", "media_ffprobe"])]
    media_ytdlp: Option<PathBuf>,

    /// Essais : ce ffmpeg, pris tel quel
    #[arg(long, value_name = "FICHIER", requires = "media_ytdlp")]
    media_ffmpeg: Option<PathBuf>,

    /// Essais : ce ffprobe, pris tel quel
    #[arg(long, value_name = "FICHIER", requires = "media_ytdlp")]
    media_ffprobe: Option<PathBuf>,

    /// Essais : temps entre deux lectures d'une copie de site sur un même hôte, en
    /// millisecondes, au lieu de 250 (banc de reprise des copies de sites)
    #[arg(long, value_name = "MS")]
    site_spacing_ms: Option<u64>,

    /// Désinstallation : efface du coffre du système les secrets de la base (cookies, mots
    /// de passe des sites et du proxy), sans toucher à ceux d'une autre base, puis sort ;
    /// moteur arrêté
    #[arg(long)]
    erase_secrets: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    // Le point d'accès effectif (`--endpoint`, sinon `AURO_ENDPOINT`, sinon le défaut),
    // comparé au seul vrai défaut (audit R3).
    let effective = args.endpoint.clone().unwrap_or_else(paths::endpoint);
    check_test_setup(&effective, args.db.as_deref(), &paths::default_endpoint())?;
    let db = match args.db {
        Some(db) => db,
        None => paths::database().context("dossier de données introuvable ; préciser --db")?,
    };
    // Désinstallation (« Supprimer aussi les données », audit C4) : rien d'autre.
    if args.erase_secrets {
        let erased = Vault::system()
            .erase_base(&db)
            .with_context(|| format!("secrets de la base {}", db.display()))?;
        eprintln!("aurod : {erased} secrets effacés du coffre");
        return Ok(());
    }
    if let Some(dir) = db.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("impossible de créer {}", dir.display()))?;
    }
    // Journaux (N-12) : dans logs/ à côté de la base, avant le démarrage du moteur (la
    // reprise du démarrage y figure). Sans eux, le moteur tourne quand même.
    if let Err(e) = auro_engine::logs::init(&auro_engine::logs::dir_for(&db)) {
        eprintln!("aurod : journaux indisponibles : {e}");
    }
    let endpoint = args.endpoint.unwrap_or_else(paths::endpoint);
    let client = match &args.ca_cert {
        None => Client::new()?,
        Some(path) => {
            let pem = std::fs::read(path)
                .with_context(|| format!("impossible de lire {}", path.display()))?;
            Client::with_extra_roots(&pem)?
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let planning = Planning {
        power: if args.simulate_power {
            power::simulate
        } else {
            power::system
        },
        ..Planning::default()
    };
    // Essais seulement : un essaim local (banc de reprise des torrents).
    let torrent = if args.torrent_peers.is_empty() {
        torrent::Options::default()
    } else {
        torrent::Options::local(args.torrent_peers.clone())
    };
    // Essais seulement : des outils de la vidéo imposés (banc de reprise des vidéos).
    let media = match (args.media_ytdlp, args.media_ffmpeg, args.media_ffprobe) {
        (Some(ytdlp), Some(ffmpeg), Some(ffprobe)) => media::Options {
            sources: media::tools::Sources {
                local_ytdlp: Some(ytdlp),
                local_ffmpeg: Some((ffmpeg, ffprobe)),
                ..media::tools::Sources::default()
            },
            dir: None,
        },
        _ => media::Options::default(),
    };
    // Essais seulement : le rythme des copies de sites (banc de reprise des copies).
    let site_spacing = args
        .site_spacing_ms
        .map_or(auro_engine::linkscan::crawl::SPACING, Duration::from_millis);
    runtime.block_on(run(
        db,
        endpoint,
        client,
        planning,
        (torrent, media),
        site_spacing,
    ))
}

/// Un point d'accès autre que celui par défaut sert aux essais : sans `--db`, il ouvrirait
/// pourtant la base par défaut, celle du moteur réel (incident du 2026-11-08). Refusé avant
/// toute ouverture de base, qu'il vienne de `--endpoint` ou d'`AURO_ENDPOINT` (audit R3).
fn check_test_setup(endpoint: &str, db: Option<&Path>, default: &str) -> Result<()> {
    if db.is_none() && endpoint != default {
        anyhow::bail!(
            "point d'accès {endpoint} sans --db : un point d'accès autre que celui par défaut \
             (--endpoint ou AURO_ENDPOINT) ne sert jamais la base par défaut, celle du moteur \
             réel ; préciser --db"
        );
    }
    Ok(())
}

async fn run(
    db: PathBuf,
    endpoint: String,
    client: Client,
    planning: Planning,
    (torrent, media): (torrent::Options, media::Options),
    site_spacing: Duration,
) -> Result<()> {
    let engine = Engine::start(Config {
        db_path: db.clone(),
        client,
        // Cookies et jetons des tâches : le coffre du système (SPEC §5.1). Empreintes lues
        // dans les fichiers voisins (F-11), selon le réglage du moteur.
        transfer: Options {
            vault: Vault::system(),
            neighbor_checksum: true,
            torrent,
            media,
            site_spacing,
            ..Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning,
    })
    .with_context(|| format!("impossible de démarrer le moteur sur {}", db.display()))?;
    // Niveau des journaux suivi (réglage `logs.level`), changements d'état notés.
    tokio::spawn(auro_engine::logs::follow(engine.clone()));
    let mut listening = tokio::spawn({
        let (engine, endpoint) = (engine.clone(), endpoint.clone());
        async move { local::listen(engine, &endpoint).await }
    });
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        base = %db.display(),
        point = %endpoint,
        "moteur démarré"
    );
    // Interface distante (SPEC §7.19) : ouverte seulement si `remote.enabled` ; suit ses
    // réglages jusqu'à l'arrêt du moteur.
    tokio::spawn(auro_engine::remote::run(engine.clone()));

    tokio::select! {
        served = &mut listening => {
            // Le point d'accès n'a pas pu s'ouvrir (déjà occupé…) : rien ne tourne encore.
            engine.shutdown().await;
            served?.with_context(|| format!("point d'accès {endpoint}"))?;
            return Ok(());
        }
        _ = stop_signal() => {
            tracing::info!("arrêt demandé, enregistrement des tâches en cours");
            engine.shutdown().await;
        }
        _ = engine.stopped() => {}
    }
    listening.await??;
    tracing::info!("moteur arrêté");
    Ok(())
}

/// Ctrl-C, ou SIGTERM sous Unix (arrêt par systemd).
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(term) => term,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
#[path = "../../cli/src/man_pages.rs"]
mod man_pages;

#[cfg(test)]
mod tests {
    use super::*;

    /// Page de manuel d'`aurod` (étape 9a) ; son installation par le `.deb` est vérifiée par
    /// le test d'`auro`.
    #[test]
    fn man_pages_are_up_to_date() {
        use clap::CommandFactory;
        man_pages::check("aurod", Args::command());
    }

    #[test]
    fn a_test_endpoint_never_opens_the_default_database() {
        let default = r"\.\pipe\aurodownloader-alice";
        assert!(check_test_setup(r"\.\pipe\essai", None, default).is_err());
        assert!(check_test_setup(r"\.\pipe\essai", Some(Path::new("e.db")), default).is_ok());
        assert!(check_test_setup(default, None, default).is_ok());
    }

    /// Audit R3 : `AURO_ENDPOINT` (navigateur d'essai) rend un point d'accès effectif autre
    /// que le défaut ; sans `--db`, refusé comme `--endpoint`.
    #[test]
    fn auro_endpoint_without_a_database_is_refused() {
        let default = paths::default_endpoint();
        let from_variable = r"\\.\pipe\auro-essai-navigateur".to_owned();
        assert_ne!(from_variable, default);
        assert!(check_test_setup(&from_variable, None, &default).is_err());
        assert!(check_test_setup(&from_variable, Some(Path::new("e.db")), &default).is_ok());
    }
}
