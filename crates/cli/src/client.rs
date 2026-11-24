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

/// Ouverture d'une session : nom du client et portée gardée (SPEC §6.3, §6.9).
pub struct Session {
    pub name: &'static str,
    /// `None` : tous les droits du canal local.
    pub scope: Option<&'static str>,
}

impl Session {
    pub const CLI: Session = Session {
        name: "auro-cli",
        scope: None,
    };
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

    /// Se connecte et ouvre la session (`engine.hello`) au nom de la CLI, avec tous les
    /// droits du canal local.
    pub async fn connect(endpoint: &str) -> std::io::Result<Self> {
        Self::open(endpoint, &Session::CLI).await
    }

    /// Se connecte et ouvre la session décrite par `session`.
    pub async fn open(endpoint: &str, session: &Session) -> std::io::Result<Self> {
        let mut rpc = Rpc::new(local::connect(endpoint).await?);
        let mut hello = json!({
            "client": {"name": session.name, "version": env!("CARGO_PKG_VERSION")},
            "protocol": {"min": "1.0", "max": "1.0"},
        });
        if let Some(scope) = session.scope {
            hello["scope"] = json!(scope);
        }
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
            // Une erreur sans identifiant (`id` nul) : le moteur n'a pas pu lire la requête,
            // qui n'aura jamais de réponse ; une seule requête attend à la fois (audit C13).
            let unread = message.get("id").is_some_and(Value::is_null);
            if message.get("id") != Some(&json!(id)) && !unread {
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
            if unread {
                self.pending.push_back(message);
                continue;
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
    // Démarré ici, `aurod` prendrait la base par défaut, celle du moteur réel : seulement
    // pour le point d'accès par défaut (incident du 2026-11-08), jamais pour celui
    // d'`AURO_ENDPOINT` (audit R3).
    if target.endpoint != auro_engine::paths::default_endpoint() {
        bail!(
            "aucun moteur sur {0} ; un point d'accès d'essai se démarre avec sa base : « auro \
             --endpoint {0} engine start --db <base> »",
            target.endpoint
        );
    }
    start_daemon(&target.endpoint, None, target.ca_cert.as_deref())?;
    wait_for(&target.endpoint).await
}

/// Un `aurod` démarré sur un point d'accès d'essai exige sa base : sans elle, il refuse de
/// démarrer, et l'attendre ne mènerait qu'à « ne répond pas » au bout de 10 s (audit C7).
pub fn check_start(endpoint: &str, db: Option<&Path>) -> Result<()> {
    if db.is_none() && endpoint != auro_engine::paths::default_endpoint() {
        bail!(
            "un point d'accès d'essai se démarre avec sa base : « auro --endpoint {endpoint} \
             engine start --db <base> »"
        );
    }
    Ok(())
}

/// Attend que le moteur réponde, dix secondes au plus.
pub async fn wait_for(endpoint: &str) -> Result<Rpc> {
    wait_for_session(endpoint, &Session::CLI).await
}

/// Attend que le moteur réponde, dix secondes au plus, et ouvre la session `session`.
pub async fn wait_for_session(endpoint: &str, session: &Session) -> Result<Rpc> {
    let mut last = None;
    for _ in 0..100 {
        match Rpc::open(endpoint, session).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt;

    /// Audit C13 : une requête que le moteur n'a pas pu lire reçoit une erreur à `id` nul ;
    /// l'appel la rend au lieu d'attendre pour toujours.
    #[tokio::test]
    async fn an_error_without_id_ends_the_call() {
        let (client, server) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client);
        let mut rpc = Rpc::new((Box::new(read), Box::new(write)));
        let (server_read, mut server_write) = tokio::io::split(server);
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            let _request = lines.next_line().await.unwrap();
            let refusal = json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32600, "message": "requête illisible",
                          "data": {"code": "INVALID_REQUEST"}},
            });
            server_write
                .write_all(format!("{refusal}\n").as_bytes())
                .await
                .unwrap();
            // Connexion gardée ouverte : sans le correctif, l'appel attendrait.
            std::future::pending::<()>().await;
        });
        let answer = tokio::time::timeout(Duration::from_secs(5), rpc.call("task.list", json!({})))
            .await
            .expect("l'appel ne doit pas attendre pour toujours");
        let refused = answer.unwrap_err();
        let refused = refused.downcast_ref::<EngineRefused>().unwrap();
        assert_eq!(refused.code, "INVALID_REQUEST");
    }
}
