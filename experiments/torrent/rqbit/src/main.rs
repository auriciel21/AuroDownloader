//! Prototype torrent avec librqbit (phase 5, étape 2 ; experiments/torrent/README.md).
//!
//! `auro-torrent-rqbit <fichier.torrent | lien magnet> <dossier> <état> <envoi max en octets/s>`
//!
//! Télécharge le torrent dans `<dossier>`, avec la reprise rapide et l'état de la session
//! dans `<état>` (un second lancement reprend où le premier s'est arrêté), l'envoi limité,
//! sans port d'écoute entrant ni découverte sur le réseau local. Une ligne par seconde
//! (`t=… octets=… recu=… envoye=… pairs=…`), puis s'arrête dès la fin du téléchargement,
//! sans continuer à partager.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use librqbit::dht::DhtPersistenceConfig;
use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, DhtSessionConfig, Session, SessionOptions,
    SessionPersistenceConfig,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, torrent, dir, state, upload] = args.as_slice() else {
        bail!("usage : auro-torrent-rqbit <fichier.torrent> <dossier> <état> <envoi max o/s>");
    };
    let upload: u32 = upload.parse().context("envoi max")?;
    let state = PathBuf::from(state);
    std::fs::create_dir_all(&state)?;
    std::fs::create_dir_all(dir)?;

    let started = Instant::now();
    let session = Session::new_with_opts(
        PathBuf::from(dir),
        SessionOptions {
            dht: Some(DhtSessionConfig {
                persistence: Some(DhtPersistenceConfig {
                    config_filename: Some(state.join("dht.json")),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            fastresume: true,
            persistence: Some(SessionPersistenceConfig::Json {
                folder: Some(state.join("session")),
            }),
            listen: None,
            disable_local_service_discovery: true,
            ratelimits: LimitsConfig {
                upload_bps: NonZeroU32::new(upload),
                download_bps: None,
            },
            client_name_and_version: Some("auro-prototype 0.1".into()),
            ..Default::default()
        },
    )
    .await?;

    // Un lien magnet, ou le chemin d'un fichier .torrent.
    let add = if torrent.starts_with("magnet:") {
        AddTorrent::from_url(torrent.as_str())
    } else {
        AddTorrent::from_bytes(std::fs::read(torrent).context("lecture du .torrent")?)
    };
    let handle = session
        .add_torrent(
            add,
            Some(AddTorrentOptions {
                output_folder: Some(dir.clone()),
                ..Default::default()
            }),
        )
        .await?
        .into_handle()
        .context("pas de torrent")?;
    eprintln!("ajouté en {:.1} s", started.elapsed().as_secs_f64());

    // Octets déjà présents : relevés à la fin de la vérification de départ.
    let mut resumed_at: Option<u64> = None;
    let mut first_byte: Option<f64> = None;
    loop {
        let s = handle.stats();
        let t = started.elapsed().as_secs_f64();
        let peers = s
            .live
            .as_ref()
            .map(|l| l.snapshot.peer_stats.live)
            .unwrap_or(0);
        if resumed_at.is_none() && s.live.is_some() {
            resumed_at = Some(s.progress_bytes);
        }
        if first_byte.is_none() && resumed_at.is_some_and(|r| s.progress_bytes > r) {
            first_byte = Some(t);
        }
        println!(
            "t={t:.0} etat={} octets={} total={} recu={} envoye={} pairs={peers}",
            s.state,
            s.progress_bytes,
            s.total_bytes,
            s.live.as_ref().map(|l| l.download_speed.mbps).unwrap_or(0.0),
            s.uploaded_bytes,
        );
        if let Some(e) = &s.error {
            bail!("erreur du torrent : {e}");
        }
        if s.finished {
            println!(
                "FIN t={t:.1} repris_a={} premier_octet={:.1} envoye={}",
                resumed_at.unwrap_or(0),
                first_byte.unwrap_or(0.0),
                s.uploaded_bytes
            );
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    // Arrêt sans partager : on retire le torrent de la session, fichiers gardés.
    session.delete(handle.id().into(), false).await?;
    session.stop().await;
    Ok(())
}
