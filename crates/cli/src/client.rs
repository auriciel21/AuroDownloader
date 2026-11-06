//! Client du protocole du moteur (SPEC §6) : connexion au point d'accès local, appels,
//! notifications, et démarrage d'`aurod` s'il ne tourne pas.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use auro_engine::launch;
use auro_engine::rpc::local::{self, Halves};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

/// Erreur renvoyée par le moteur : `code` est le `data.code` stable (SPEC §6.7).
#[derive(Debug, thiserror::Error)]
#[error("{message} [{code}]")]
pub struct EngineRefused {
    pub code: String,
    pub message: String,
}

/// Connexion au moteur.
pub struct Rpc {
    lines: Lines<BufReader<Box<dyn tokio::io::AsyncRead + Unpin + Send>>>,
    writer: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
    next_id: u64,
    /// Notifications reçues en attendant une réponse.
    pending: VecDeque<Value>,
}

impl Rpc {
    fn new((read, writer): Halves) -> Self {
        Rpc {
            lines: BufReader::new(read).lines(),
            writer,
            next_id: 1,
            pending: VecDeque::new(),
        }
    }

    /// Se connecte et ouvre la session (`engine.hello`).
    pub async fn connect(endpoint: &str) -> std::io::Result<Self> {
        let mut rpc = Rpc::new(local::connect(endpoint).await?);
        let hello = json!({
            "client": {"name": "auro-cli", "version": env!("CARGO_PKG_VERSION")},
            "protocol": {"min": "1.0", "max": "1.0"},
        });
        rpc.call("engine.hello", hello)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(rpc)
    }

    async fn read(&mut self) -> Result<Value> {
        let line = self
            .lines
            .next_line()
            .await?
            .context("le moteur a fermé la connexion")?;
        serde_json::from_str(&line).context("message illisible du moteur")
    }

    /// Appelle `method` ; une erreur du moteur devient un [`EngineRefused`].
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut bytes = request.to_string().into_bytes();
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await?;
        self.writer.flush().await?;
        loop {
            let message = self.read().await?;
            if message.get("id") != Some(&json!(id)) {
                self.pending.push_back(message);
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(EngineRefused {
                    code: error["data"]["code"]
                        .as_str()
                        .unwrap_or("INTERNAL_ERROR")
                        .to_owned(),
                    message: error["message"].as_str().unwrap_or("erreur").to_owned(),
                }
                .into());
            }
            return Ok(message["result"].clone());
        }
    }

    /// Prochaine notification : `(méthode, paramètres)`.
    pub async fn notification(&mut self) -> Result<(String, Value)> {
        loop {
            let message = match self.pending.pop_front() {
                Some(m) => m,
                None => self.read().await?,
            };
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                return Ok((method.to_owned(), message["params"].clone()));
            }
        }
    }
}

/// Comment joindre le moteur.
pub struct Target {
    pub endpoint: String,
    /// Démarrer `aurod` s'il ne répond pas.
    pub autostart: bool,
    /// Transmis à `aurod` s'il est démarré ici.
    pub ca_cert: Option<PathBuf>,
}

/// Connexion au moteur, après l'avoir démarré si besoin.
pub async fn connect(target: &Target) -> Result<Rpc> {
    if let Ok(rpc) = Rpc::connect(&target.endpoint).await {
        return Ok(rpc);
    }
    if !target.autostart {
        bail!(
            "aucun moteur sur {} ; le démarrer avec « auro engine start »",
            target.endpoint
        );
    }
    start_daemon(&target.endpoint, None, target.ca_cert.as_deref())?;
    wait_for(&target.endpoint).await
}

/// Attend que le moteur réponde, dix secondes au plus.
pub async fn wait_for(endpoint: &str) -> Result<Rpc> {
    let mut last = None;
    for _ in 0..100 {
        match Rpc::connect(endpoint).await {
            Ok(rpc) => return Ok(rpc),
            Err(e) => last = Some(e),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(last.map_or_else(
        || anyhow::anyhow!("le moteur ne répond pas"),
        anyhow::Error::from,
    ))
    .with_context(|| format!("le moteur ne répond pas sur {endpoint}"))
}

/// Lance `aurod` détaché (`auro_engine::launch`, commun avec l'application de bureau).
pub fn start_daemon(endpoint: &str, db: Option<&Path>, ca_cert: Option<&Path>) -> Result<()> {
    Ok(launch::start_daemon(endpoint, db, ca_cert)?)
}
