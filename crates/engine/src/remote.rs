//! Interface web distante (F-39, SPEC §7.19) : jetons, sessions des navigateurs, et
//! (étape 3a-2) l'écoute HTTP et WebSocket.
//!
//! Un jeton est un secret de 32 octets tirés de l'aléa du système, rendu une seule fois à
//! sa création ; la base n'en garde que l'empreinte SHA-256 (`store::remote`). De même pour
//! la valeur du cookie d'une session. Une connexion par jeton a au plus la portée
//! `control` (décision de l'utilisateur : `admin` reste au canal local), et passe par la
//! poignée restreinte du moteur ([`crate::scheduler::Engine::restricted`]).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::rpc::Scope;
use crate::scheduler::{Engine, Event};
use crate::store::settings::{Flag, REMOTE_LISTEN};

pub mod server;
#[cfg(test)]
mod server_tests;

/// Préfixe d'un jeton : le reconnaître dans un fichier ou un message (outils de recherche
/// de secrets), et refuser tôt ce qui n'en est pas un.
pub const TOKEN_PREFIX: &str = "auro_";

/// Octets aléatoires d'un jeton ou d'une session.
const SECRET_BYTES: usize = 32;

/// Durée d'une session de navigateur (SPEC §7.19) : 30 jours.
pub const SESSION_TTL_MS: i64 = 30 * 24 * 3600 * 1000;

/// Un nouveau secret : `auro_` suivi de 32 octets aléatoires en base64url (43 caractères).
pub fn new_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    aws_lc_rs::rand::fill(&mut bytes).expect("aléa du système indisponible");
    format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// Empreinte SHA-256 d'un secret, seule gardée en base.
pub fn digest(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

/// A-t-il la forme d'un secret ? (Rien n'est cherché en base sinon.)
pub fn looks_like_secret(text: &str) -> bool {
    text.len() == TOKEN_PREFIX.len() + 43
        && text.starts_with(TOKEN_PREFIX)
        && text[TOKEN_PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Portée d'un jeton : `read`, `add` ou `control`, jamais `admin`.
pub fn token_scope(text: &str) -> Option<Scope> {
    Scope::parse(text).filter(|s| *s != Scope::Admin)
}

/// État de l'écoute, pour `remote.status`, la CLI et le bureau.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteState {
    /// Réglage `remote.enabled`.
    pub enabled: bool,
    /// Réglage `remote.listen`.
    pub listen: Option<String>,
    /// L'écoute est ouverte.
    pub listening: bool,
    /// Pourquoi elle ne l'est pas, alors qu'elle devrait.
    pub error: Option<String>,
}

/// Tient l'écoute de l'interface distante selon les réglages `remote.enabled` et
/// `remote.listen`, pour toute la vie du moteur : ouverte, refermée ou déplacée à chaque
/// changement. Lancée par `aurod` (pas par l'application Android). Hors de la boucle
/// locale, TLS est exigé : refusé jusqu'à l'étape 3b.
pub async fn run(engine: Engine) {
    let mut events = engine.subscribe();
    loop {
        let (Ok(enabled), Ok(listen)) = (
            engine.flag(Flag::RemoteEnabled).await,
            engine.remote_listen().await,
        ) else {
            return;
        };
        let mut state = RemoteState {
            enabled,
            listen: Some(listen.to_string()),
            ..RemoteState::default()
        };
        let mut serving = None;
        if enabled {
            if !listen.ip().is_loopback() {
                state.error = Some(format!(
                    "{listen} : hors de la boucle locale, TLS est exigé (pas encore pris en \
                     charge) ; écouter sur 127.0.0.1"
                ));
            } else {
                match tokio::net::TcpListener::bind(listen).await {
                    Ok(listener) => {
                        state.listening = true;
                        serving = Some(tokio::spawn(server::serve(engine.clone(), listener)));
                    }
                    Err(e) => state.error = Some(format!("écoute impossible sur {listen} : {e}")),
                }
            }
        }
        if let Some(error) = &state.error {
            eprintln!("aurod : interface distante : {error}");
        }
        engine.set_remote_state(state);
        let stopped = loop {
            match events.recv().await {
                Ok(Event::FlagChanged {
                    flag: Flag::RemoteEnabled,
                    ..
                })
                | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break false,
                Ok(Event::SettingChanged { key, .. }) if key == REMOTE_LISTEN => break false,
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break true,
            }
        };
        if let Some(task) = serving {
            task.abort();
            let _ = task.await;
        }
        if stopped {
            return;
        }
    }
}

/// Opérations sur les jetons et les sessions, faites par la boucle du planificateur,
/// seule à écrire en base.
#[derive(Debug)]
pub enum TokenOp {
    Create {
        name: String,
        scope: String,
        digest: [u8; 32],
    },
    List,
    Revoke(String),
    /// Le jeton de cette empreinte ; son usage est noté.
    UseToken([u8; 32]),
    OpenSession {
        digest: [u8; 32],
        token_id: String,
        expires_at: i64,
    },
    /// Le jeton d'une session encore valable ; son usage est noté.
    UseSession([u8; 32]),
    CloseSession([u8; 32]),
}

/// Réponse à une [`TokenOp`].
#[derive(Debug)]
pub enum TokenAnswer {
    Token(crate::store::remote::TokenRow),
    Tokens(Vec<crate::store::remote::TokenRow>),
    Found(Option<crate::store::remote::TokenRow>),
    Done(bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_random_recognisable_and_only_their_digest_is_kept() {
        let (a, b) = (new_secret(), new_secret());
        assert_ne!(a, b);
        assert!(looks_like_secret(&a) && looks_like_secret(&b), "{a}");
        assert_eq!(digest(&a), digest(&a));
        assert_ne!(digest(&a), digest(&b));
        for bad in [
            "",
            "auro_",
            "auro_court",
            &a[1..],
            &format!("{a}x"),
            &a.replace('_', "+"),
        ] {
            assert!(!looks_like_secret(bad), "{bad}");
        }
        assert_eq!(token_scope("control"), Some(Scope::Control));
        assert_eq!(token_scope("read"), Some(Scope::Read));
        assert_eq!(token_scope("admin"), None);
        assert_eq!(token_scope("tout"), None);
    }
}
