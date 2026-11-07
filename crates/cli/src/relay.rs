//! Relais de l'extension navigateur (SPEC §4.5) : hôte de messagerie native.
//!
//! Le navigateur lance `auro-relay` et lui parle par son entrée et sa sortie standard :
//! chaque message est un objet JSON en UTF-8, précédé de sa longueur sur 4 octets, dans
//! l'ordre des octets de la machine. Le relais transmet au moteur, sur le point d'accès
//! local, en se limitant à la portée `add` (SPEC §6.9) ; il démarre `aurod` s'il ne tourne
//! pas.
//!
//! Messages de l'extension (champ `id` facultatif, renvoyé tel quel dans la réponse) :
//! - `{"type": "ping"}` : `{"ok": true, "version": …}`, sans toucher au moteur ;
//! - `{"type": "add", "url": …, "requestId": …, "paused": false, "headers": {…}}` : ajoute
//!   un téléchargement dans le dossier du moteur ; `requestId` sert de clé d'idempotence
//!   (un nouvel essai ne crée pas de doublon) ; `headers`, les en-têtes de la page
//!   (`Referer`, `User-Agent`, `Cookie`), transmis à `task.add`. Réponse
//!   `{"ok": true, "taskId": …}`.
//!
//! Un refus répond `{"ok": false, "code": …, "message": …}` : `code` est celui du moteur
//! (SPEC §6.7), ou `ENGINE_UNAVAILABLE`, `BAD_MESSAGE`, `UNKNOWN_TYPE`.

use std::io;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::client::{self, EngineRefused, Rpc, Session};

/// Nom de l'hôte déclaré aux navigateurs.
pub const HOST_NAME: &str = "org.aurodownloader.relay";

/// Plus long message accepté de l'extension. Les siens font quelques centaines d'octets ;
/// le navigateur n'en enverrait pas plus de 64 Mio.
pub const MAX_INCOMING: usize = 1 << 20;

/// Plus long message que le navigateur accepte de l'hôte (Chrome, Firefox).
pub const MAX_OUTGOING: usize = 1 << 20;

/// Session du relais : la portée `add` seulement.
const SESSION: Session = Session {
    name: "auro-relay",
    scope: Some("add"),
};

/// Lit un message ; `None` quand le navigateur ferme l'entrée.
pub async fn read_message<R: AsyncRead + Unpin>(input: &mut R) -> io::Result<Option<Value>> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_ne_bytes(len) as usize;
    if len > MAX_INCOMING {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("message de {len} octets, au plus {MAX_INCOMING}"),
        ));
    }
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Écrit un message.
pub async fn write_message<W: AsyncWrite + Unpin>(
    output: &mut W,
    message: &Value,
) -> io::Result<()> {
    let bytes = message.to_string().into_bytes();
    if bytes.len() > MAX_OUTGOING {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "réponse trop longue pour le navigateur",
        ));
    }
    output
        .write_all(&(bytes.len() as u32).to_ne_bytes())
        .await?;
    output.write_all(&bytes).await?;
    output.flush().await
}

/// Le relais : une connexion au moteur, ouverte au premier ajout et gardée ensuite.
pub struct Relay {
    endpoint: String,
    rpc: Option<Rpc>,
}

fn refusal(code: &str, message: impl Into<String>) -> Value {
    json!({"ok": false, "code": code, "message": message.into()})
}

impl Relay {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Relay {
            endpoint: endpoint.into(),
            rpc: None,
        }
    }

    /// Connexion au moteur, démarré si besoin.
    async fn engine(&mut self) -> anyhow::Result<&mut Rpc> {
        if self.rpc.is_none() {
            let rpc = match Rpc::open(&self.endpoint, &SESSION).await {
                Ok(rpc) => rpc,
                Err(_) => {
                    client::start_daemon(&self.endpoint, None, None)?;
                    client::wait_for_session(&self.endpoint, &SESSION).await?
                }
            };
            self.rpc = Some(rpc);
        }
        Ok(self.rpc.as_mut().expect("connexion ouverte ci-dessus"))
    }

    /// Appelle le moteur ; si la connexion gardée est tombée (moteur redémarré), en
    /// rouvre une et réessaie une fois. Un refus du moteur n'est pas réessayé.
    async fn call(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let first = self.engine().await?.call(method, params.clone()).await;
        match first {
            Err(e) if e.downcast_ref::<EngineRefused>().is_none() => {
                self.rpc = None;
                self.engine().await?.call(method, params).await
            }
            other => other,
        }
    }

    /// Réponse à un message de l'extension.
    pub async fn handle(&mut self, message: &Value) -> Value {
        let mut reply = match message.get("type").and_then(Value::as_str) {
            Some("ping") => json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}),
            Some("add") => self.add(message).await,
            Some(other) => refusal("UNKNOWN_TYPE", format!("type de message inconnu : {other}")),
            None => refusal("BAD_MESSAGE", "type manquant"),
        };
        if let Some(id) = message.get("id") {
            reply["id"] = id.clone();
        }
        reply
    }

    async fn add(&mut self, message: &Value) -> Value {
        let Some(url) = message.get("url").and_then(Value::as_str) else {
            return refusal("BAD_MESSAGE", "url manquante");
        };
        let mut params = json!({
            "sources": [{"url": url}],
            "startPaused": message.get("paused").and_then(Value::as_bool).unwrap_or(false),
        });
        if let Some(key) = message.get("requestId").and_then(Value::as_str) {
            params["idempotencyKey"] = json!(key);
        }
        // En-têtes de la page (`Referer`, `User-Agent`, `Cookie`) : le moteur les vérifie,
        // et range les secrets dans le coffre du système.
        match message.get("headers") {
            None | Some(Value::Null) => {}
            Some(headers @ Value::Object(_)) => params["headers"] = headers.clone(),
            Some(_) => return refusal("BAD_MESSAGE", "headers : un objet est attendu"),
        }
        match self.call("task.add", params).await {
            Ok(result) => json!({"ok": true, "taskId": result["taskId"]}),
            Err(e) => match e.downcast::<EngineRefused>() {
                Ok(refused) => refusal(&refused.code, refused.message),
                Err(e) => refusal("ENGINE_UNAVAILABLE", format!("{e:#}")),
            },
        }
    }
}

/// Sert l'extension jusqu'à ce que le navigateur ferme l'entrée. Un message illisible
/// arrête le relais : le flux n'est plus fiable.
pub async fn serve<R, W>(mut input: R, mut output: W, endpoint: &str) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut relay = Relay::new(endpoint);
    while let Some(message) = read_message(&mut input).await? {
        let reply = relay.handle(&message).await;
        write_message(&mut output, &reply).await?;
    }
    Ok(())
}
