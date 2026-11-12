//! Bibliothèque native de l'application Android (SPEC §4.4, phase 4) : le moteur tourne
//! dans le processus de l'application, qui lui parle JSON-RPC comme tous les clients
//! (SPEC §6.2, appel direct dans le processus, sans authentification).
//!
//! L'application envoie un message par appel ([`Host::send`]) ; les réponses et les
//! notifications lui reviennent une par une, dans l'ordre ([`Host::start`],
//! `on_message`). [`Host`] ne dépend pas d'Android et se teste partout ; la liaison JNI
//! (`jni_glue`) n'est compilée que pour Android.

use std::path::Path;
use std::time::Duration;

use auro_engine::net::Client;
use auro_engine::rpc;
use auro_engine::scheduler::{Config, DISK_RECHECK, Engine, Planning};
use auro_engine::transfer::Options;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

#[cfg(target_os = "android")]
mod jni_glue;

/// Taille du tampon de la connexion dans le processus, dans chaque sens.
const PIPE: usize = 1 << 20;

/// Délai laissé au moteur pour enregistrer ses tâches à l'arrêt.
const SHUTDOWN: Duration = Duration::from_secs(10);

/// Moteur en marche dans le processus, avec sa connexion JSON-RPC.
pub struct Host {
    runtime: Option<Runtime>,
    engine: Engine,
    /// Messages de l'application, écrits sur la connexion dans l'ordre d'envoi.
    outgoing: mpsc::UnboundedSender<String>,
}

impl Host {
    /// Démarre le moteur sur la base `db` et ouvre sa connexion. `on_message` reçoit
    /// chaque message du moteur (une ligne JSON), depuis un fil du moteur.
    pub fn start(db: &Path, on_message: impl Fn(String) + Send + 'static) -> Result<Host, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("auro-moteur")
            .enable_all()
            .build()
            .map_err(|e| format!("moteur : {e}"))?;
        // Journaux (N-12) : dans logs/ à côté de la base, dans le dossier de l'application.
        let _ = auro_engine::logs::init(&auro_engine::logs::dir_for(db));
        let client = Client::new().map_err(|e| e.to_string())?;
        let config = Config {
            db_path: db.to_owned(),
            client,
            // Empreintes lues dans les fichiers voisins (F-11), selon le réglage, comme
            // pour `aurod`. Secrets : la mémoire, jusqu'au Keystore (SPEC §5.1).
            transfer: Options {
                neighbor_checksum: true,
                ..Options::default()
            },
            disk_recheck: DISK_RECHECK,
            // Rien ne part avant que l'application dise le réseau utilisable (SPEC §7.15) :
            // démarré par Android en arrière-plan (redémarrage de l'appareil), le processus
            // ne télécharge pas sans la protection d'un transfert ou d'un service.
            planning: Planning {
                network_usable: false,
                ..Planning::default()
            },
        };
        let engine = runtime
            .block_on(async { Engine::start(config) })
            .map_err(|e| e.to_string())?;
        runtime.spawn(auro_engine::logs::follow(engine.clone()));

        let (ours, theirs) = tokio::io::duplex(PIPE);
        let (read, write) = tokio::io::split(theirs);
        runtime.spawn(rpc::serve(engine.clone(), read, write));
        let (reader, mut writer) = tokio::io::split(ours);
        let (outgoing, mut queued) = mpsc::unbounded_channel::<String>();
        runtime.spawn(async move {
            while let Some(message) = queued.recv().await {
                let sent = async {
                    writer.write_all(message.as_bytes()).await?;
                    writer.write_all(b"\n").await
                };
                if sent.await.is_err() {
                    break;
                }
            }
        });
        runtime.spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                on_message(line);
            }
        });
        Ok(Host {
            runtime: Some(runtime),
            engine,
            outgoing,
        })
    }

    /// Envoie un message JSON-RPC au moteur. Un saut de ligne ne peut figurer en JSON que
    /// comme espacement, hors des chaînes : il devient une espace, le message reste une
    /// seule ligne.
    pub fn send(&self, message: &str) {
        let _ = self.outgoing.send(message.replace(['\r', '\n'], " "));
    }

    /// Arrête le moteur : ses tâches sont enregistrées, en pause, et reprendront au
    /// prochain démarrage (SPEC §7.4).
    pub fn stop(mut self) {
        if let Some(runtime) = self.runtime.take() {
            let engine = self.engine.clone();
            runtime.block_on(async {
                let _ = tokio::time::timeout(SHUTDOWN, engine.shutdown()).await;
            });
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests;
