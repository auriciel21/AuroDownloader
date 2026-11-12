//! Interface web distante (F-39, SPEC §7.19) : jetons, sessions des navigateurs, et
//! (étape 3a-2) l'écoute HTTP et WebSocket.
//!
//! Un jeton est un secret de 32 octets tirés de l'aléa du système, rendu une seule fois à
//! sa création ; la base n'en garde que l'empreinte SHA-256 (`store::remote`). De même pour
//! la valeur du cookie d'une session. Une connexion par jeton a au plus la portée
//! `control` (décision de l'utilisateur : `admin` reste au canal local), et passe par la
//! poignée restreinte du moteur ([`crate::scheduler::Engine::restricted`]).

use std::net::{IpAddr, SocketAddr};
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::rpc::Scope;
use crate::scheduler::{Engine, Event};
use crate::store::settings::{
    Flag, REMOTE_HOSTS, REMOTE_LISTEN, REMOTE_TLS_CERT, REMOTE_TLS_KEY, RemoteConfig,
};

#[cfg(test)]
mod fuzz;
pub mod page;
pub mod server;
#[cfg(test)]
mod server_tests;
pub mod tls;

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
    /// En TLS (toute écoute hors de la boucle locale).
    pub tls: bool,
    /// Le certificat servi.
    pub certificate: Option<CertificateInfo>,
    /// Adresses où ouvrir la page (`https://192.168.1.20:7380`…).
    pub urls: Vec<String>,
    /// Réglages `remote.hosts` et `remote.tlsCert` de l'écoute en place : un client sait
    /// ainsi quand elle a suivi un changement.
    pub hosts: Vec<String>,
    pub tls_cert: Option<String>,
}

/// Le certificat servi, tel que `remote.status` le montre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateInfo {
    /// SHA-256, à comparer avec celle que montre le navigateur.
    pub fingerprint: String,
    /// Fin de validité, en millisecondes Unix.
    pub expires_at: i64,
    /// Fabriqué par le moteur (sinon fourni).
    pub generated: bool,
    /// Noms et adresses qu'il porte.
    pub names: Vec<String>,
}

/// Une adresse où l'écoute peut s'ouvrir, avec le nom de son interface (« Wi-Fi »), pour
/// le choix de l'adresse d'écoute au bureau (étape 3d).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenChoice {
    pub interface: String,
    pub address: IpAddr,
}

/// Adresses des interfaces réseau où ouvrir l'écoute, hors boucle locale (le bureau propose
/// « cet ordinateur seulement » à part) : ni adresse locale au lien, ni interface de boucle
/// locale (`lo` de WSL porte aussi 10.255.255.254). IPv4 d'abord, puis par interface.
pub fn listen_choices() -> Vec<ListenChoice> {
    use network_interface::{NetworkInterface, NetworkInterfaceConfig};
    let Ok(nics) = NetworkInterface::show() else {
        return Vec::new();
    };
    let loopbacks: Vec<&str> = nics
        .iter()
        .filter(|n| n.addr.iter().any(|a| a.ip().is_loopback()))
        .map(|n| n.name.as_str())
        .collect();
    let mut choices: Vec<ListenChoice> = nics
        .iter()
        .filter(|n| !loopbacks.contains(&n.name.as_str()))
        .flat_map(|n| {
            n.addr.iter().map(|a| ListenChoice {
                interface: n.name.clone(),
                address: a.ip(),
            })
        })
        .filter(|c| listenable(&c.address))
        .collect();
    choices.sort_by(|a, b| {
        (a.address.is_ipv6(), &a.interface, a.address).cmp(&(
            b.address.is_ipv6(),
            &b.interface,
            b.address,
        ))
    });
    choices.dedup();
    choices
}

/// Une adresse proposée pour l'écoute : joignable depuis un autre appareil.
fn listenable(ip: &IpAddr) -> bool {
    crate::torrent::usable_addr(ip) && !ip.is_loopback() && !ip.is_multicast()
}

/// Délai maximal entre deux contrôles de l'échéance du certificat fabriqué.
const RENEW_CHECK: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Tient l'écoute de l'interface distante selon les réglages `remote.enabled`,
/// `remote.listen`, `remote.hosts`, `remote.tlsCert` et `remote.tlsKey`, pour toute la vie
/// du moteur : ouverte, refermée ou déplacée quand l'un d'eux change. Lancée par `aurod`
/// (pas par l'application Android). Hors de la boucle locale, en TLS (module [`tls`]) ; le
/// certificat fabriqué est refait à son échéance, l'écoute repartant avec lui.
pub async fn run(engine: Engine) {
    supervise(engine, false).await;
}

/// [`run`] ; `tls_everywhere` : TLS même sur la boucle locale (tests).
pub(crate) async fn supervise(engine: Engine, tls_everywhere: bool) {
    let mut events = engine.subscribe();
    // Réglages de l'écoute en cours, et la tâche qui la sert.
    let mut applied: Option<(bool, RemoteConfig)> = None;
    let mut serving: Option<tokio::task::JoinHandle<std::io::Result<()>>> = None;
    let mut renew_at: Option<i64> = None;
    // Le socket d'écoute, gardé tant que son adresse ne change pas.
    let mut bound: Option<Bound> = None;
    loop {
        let (Ok(enabled), Ok(config)) = (
            engine.flag(Flag::RemoteEnabled).await,
            engine.remote_config().await,
        ) else {
            break;
        };
        let due = renew_at.is_some_and(|at| crate::store::now_ms() >= at);
        let current = Some((enabled, config.clone()));
        if applied != current || due {
            if let Some(task) = serving.take() {
                task.abort();
                let _ = task.await;
            }
            let (state, task, renew) =
                open(&engine, enabled, &config, tls_everywhere, &mut bound).await;
            if let Some(error) = &state.error {
                eprintln!("aurod : interface distante : {error}");
            }
            engine.set_remote_state(state);
            (serving, renew_at, applied) = (task, renew, current);
        }
        // Jusqu'au prochain changement d'un réglage de l'écoute, ou à l'échéance du
        // certificat (contrôlée au moins chaque jour : le sommeil de tokio est borné).
        let wait = renew_at.map(|at| {
            let left = u64::try_from(at - crate::store::now_ms()).unwrap_or(0);
            std::time::Duration::from_millis(left).min(RENEW_CHECK)
        });
        // Créé une fois : les autres événements du moteur (avancement…) ne le repoussent pas.
        let deadline = sleep_or_forever(wait);
        tokio::pin!(deadline);
        let stopped = loop {
            let event = tokio::select! {
                event = events.recv() => event,
                () = &mut deadline => break false,
            };
            match event {
                Ok(Event::FlagChanged {
                    flag: Flag::RemoteEnabled,
                    ..
                })
                | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break false,
                Ok(Event::SettingChanged { key, .. })
                    if [REMOTE_LISTEN, REMOTE_HOSTS, REMOTE_TLS_CERT, REMOTE_TLS_KEY]
                        .contains(&key) =>
                {
                    break false;
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break true,
            }
        };
        if stopped {
            break;
        }
    }
    if let Some(task) = serving {
        task.abort();
        let _ = task.await;
    }
}

async fn sleep_or_forever(wait: Option<std::time::Duration>) {
    match wait {
        Some(wait) => tokio::time::sleep(wait).await,
        None => std::future::pending().await,
    }
}

type Serving = Option<tokio::task::JoinHandle<std::io::Result<()>>>;

/// Socket d'écoute ouvert sur une adresse. L'écoute qui repart sur la même adresse (noms,
/// certificat changés, certificat refait) en reprend une copie, sans jamais rendre le
/// port : entre la fermeture et une nouvelle ouverture, un autre programme, ou une connexion
/// sortante à qui le système le donnerait comme port local, pourrait le prendre (trouvé
/// sous Linux, test de la CLI en parallèle d'autres).
/// L'écouteur est partagé avec la tâche qui sert (pas de copie du socket : sous Windows,
/// une copie ne reçoit pas les connexions par l'exécuteur de tokio).
struct Bound {
    addr: SocketAddr,
    listener: std::sync::Arc<tokio::net::TcpListener>,
}

/// Un écouteur sur `listen` : celui gardé s'il est à cette adresse, sinon un nouveau
/// (l'ancien rendu d'abord).
async fn listener_for(
    bound: &mut Option<Bound>,
    listen: SocketAddr,
) -> std::io::Result<std::sync::Arc<tokio::net::TcpListener>> {
    if let Some(b) = bound.as_ref().filter(|b| b.addr == listen) {
        return Ok(std::sync::Arc::clone(&b.listener));
    }
    *bound = None;
    let listener = std::sync::Arc::new(tokio::net::TcpListener::bind(listen).await?);
    *bound = Some(Bound {
        addr: listen,
        listener: std::sync::Arc::clone(&listener),
    });
    Ok(listener)
}

/// Ouvre l'écoute voulue par les réglages ; rend son état, sa tâche et l'échéance de son
/// certificat fabriqué.
async fn open(
    engine: &Engine,
    enabled: bool,
    config: &RemoteConfig,
    tls_everywhere: bool,
    bound: &mut Option<Bound>,
) -> (RemoteState, Serving, Option<i64>) {
    let listen = config.listen;
    let mut state = RemoteState {
        enabled,
        listen: Some(listen.to_string()),
        tls: tls_everywhere || !listen.ip().is_loopback(),
        hosts: config.hosts.clone(),
        tls_cert: config.tls_files.as_ref().map(|(cert, _)| cert.clone()),
        ..RemoteState::default()
    };
    if !enabled {
        // Coupée : le port est rendu.
        *bound = None;
        return (state, None, None);
    }
    let listener = match listener_for(bound, listen).await {
        Ok(listener) => listener,
        Err(e) => {
            state.error = Some(format!("écoute impossible sur {listen} : {e}"));
            return (state, None, None);
        }
    };
    let port = listener.local_addr().map_or(listen.port(), |a| a.port());
    let mut renew_at = None;
    let tls = if state.tls {
        match load_tls(engine, config).await {
            Ok(tls) => {
                if tls.renewed {
                    eprintln!(
                        "aurod : interface distante : certificat fabriqué, empreinte SHA-256 {}",
                        tls.fingerprint
                    );
                }
                renew_at = tls.renew_at_ms();
                state.certificate = Some(CertificateInfo {
                    fingerprint: tls.fingerprint.clone(),
                    expires_at: tls.not_after_ms,
                    generated: tls.generated,
                    names: tls.names.clone(),
                });
                Some(tls.config)
            }
            Err(e) => {
                state.error = Some(format!("TLS : {e}"));
                *bound = None;
                return (state, None, None);
            }
        }
    } else {
        None
    };
    state.urls = urls(listen, port, state.tls, &config.hosts);
    state.listening = true;
    let listening = server::Listening {
        tls,
        names: tls::known_names(&config.hosts),
    };
    let task = tokio::spawn(server::serve(engine.clone(), listener, listening));
    (state, Some(task), renew_at)
}

/// Le certificat fourni par les réglages, sinon celui fabriqué à côté de la base (lu, ou
/// fait, hors de l'exécuteur : génération de clé et disque).
async fn load_tls(engine: &Engine, config: &RemoteConfig) -> Result<tls::Tls, String> {
    let config = config.clone();
    let dir = engine.data_dir().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let now = crate::store::now_ms();
        match &config.tls_files {
            Some((cert, key)) => tls::provided(Path::new(cert), Path::new(key), now),
            None => tls::generated(&dir, &tls::wanted_names(config.listen, &config.hosts), now),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Adresses où ouvrir la page : l'adresse d'écoute, ou pour une écoute sur toutes les
/// adresses celles des interfaces ; puis les noms de `remote.hosts`.
fn urls(listen: SocketAddr, port: u16, tls: bool, hosts: &[String]) -> Vec<String> {
    let scheme = if tls { "https" } else { "http" };
    let ips: Vec<IpAddr> = if listen.ip().is_unspecified() {
        tls::interface_addresses()
            .into_iter()
            .filter(|ip| listen.is_ipv6() || ip.is_ipv4())
            .collect()
    } else {
        vec![listen.ip()]
    };
    let mut urls: Vec<String> = ips
        .into_iter()
        .map(|ip| format!("{scheme}://{}", SocketAddr::new(ip, port)))
        .collect();
    for host in hosts {
        let host = match host.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, port).to_string(),
            Err(_) => format!("{host}:{port}"),
        };
        let url = format!("{scheme}://{host}");
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
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
