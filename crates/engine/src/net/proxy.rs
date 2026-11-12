//! Mandataire (proxy) des connexions HTTP(S) du moteur (F-07, phase 7, étape 3).
//!
//! Trois modes : celui du système (par défaut ; réglages Internet sous Windows, variables
//! d'environnement ailleurs), aucun, ou manuel (`http://`, `https://`, `socks5://`,
//! `socks5h://`, exceptions, identifiant ; le mot de passe est au coffre du système).
//! Les torrents ne passent jamais par lui.
//!
//! Connexions limitées au réseau public (jetons distants, plugins, SPEC §7.19) : un proxy
//! HTTP résoudrait les noms lui-même, hors du contrôle du moteur ; elles sortent donc en
//! direct, sauf par SOCKS5, que le moteur interroge avec une adresse IP qu'il a résolue
//! et vérifiée lui-même (`socks5://`, résolution locale par le résolveur filtrant).

use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::{ClientBuilder, NoProxy, Proxy, Url};

/// Exceptions d'un proxy manuel, au plus.
pub const MAX_EXCEPTIONS: usize = 64;
/// Longueur d'un identifiant, d'une exception ou d'une adresse de proxy, au plus.
const MAX_TEXT: usize = 256;

/// Mode du proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Celui du système (par défaut).
    #[default]
    System,
    /// Aucun : toujours en direct.
    None,
    /// Celui des réglages.
    Manual,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Mode> {
        match text {
            "system" => Some(Mode::System),
            "none" => Some(Mode::None),
            "manual" => Some(Mode::Manual),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::System => "system",
            Mode::None => "none",
            Mode::Manual => "manual",
        }
    }
}

/// Réglages du proxy, sans le mot de passe.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    pub mode: Mode,
    /// Adresse du proxy manuel : `http://hôte:port`, `https://…`, `socks5://…`,
    /// `socks5h://…`, sans identifiant ni chemin.
    pub url: Option<String>,
    /// Hôtes joints en direct (`localhost`, `.lan`, `192.168.0.0/16`…).
    pub exceptions: Vec<String>,
    pub username: Option<String>,
}

/// Réglages appliqués aux clients : ceux de la base, et le mot de passe lu au coffre.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Effective {
    pub settings: Settings,
    pub password: Option<String>,
}

impl std::fmt::Debug for Effective {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Effective")
            .field("settings", &self.settings)
            .field("password", &self.password.as_ref().map(|_| "…"))
            .finish()
    }
}

/// Réglages partagés par toutes les copies d'un client ; leur génération change à chaque
/// réglage, et les clients se refont alors à leur prochaine requête.
#[derive(Debug, Default)]
pub struct Shared {
    current: RwLock<Effective>,
    generation: AtomicU64,
}

impl Shared {
    pub fn get(&self) -> Effective {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set(&self, effective: Effective) {
        *self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = effective;
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

/// Vérifie des réglages : en manuel, une adresse `http`, `https`, `socks5` ou `socks5h`,
/// avec un hôte et un port, sans identifiant (donné à part), chemin ni paramètres.
pub fn validate(settings: &Settings) -> Result<(), String> {
    if settings.mode == Mode::Manual {
        let Some(text) = settings.url.as_deref() else {
            return Err("proxy manuel : adresse attendue (socks5://hôte:1080…)".into());
        };
        let url = Url::parse(text).map_err(|_| format!("adresse de proxy illisible : {text}"))?;
        if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") {
            return Err("proxy : http, https, socks5 ou socks5h".into());
        }
        if url.host_str().is_none_or(str::is_empty) || url.port_or_known_default().is_none() {
            return Err("proxy : hôte et port attendus".into());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("proxy : l'identifiant se donne à part, pas dans l'adresse".into());
        }
        if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
            return Err("proxy : ni chemin ni paramètres dans l'adresse".into());
        }
        if text.len() > MAX_TEXT {
            return Err(format!("proxy : adresse de {MAX_TEXT} caractères au plus"));
        }
    }
    if settings.exceptions.len() > MAX_EXCEPTIONS {
        return Err(format!("proxy : {MAX_EXCEPTIONS} exceptions au plus"));
    }
    let bad = |s: &str| {
        s.is_empty()
            || s.len() > MAX_TEXT
            || s.chars()
                .any(|c| c.is_control() || c == ',' || c.is_whitespace())
    };
    if let Some(e) = settings.exceptions.iter().find(|e| bad(e)) {
        return Err(format!("proxy : exception invalide : {e:?}"));
    }
    if let Some(user) = &settings.username
        && (user.is_empty() || user.len() > MAX_TEXT || user.chars().any(char::is_control))
    {
        return Err("proxy : identifiant de 1 à 256 caractères, sans caractère de contrôle".into());
    }
    Ok(())
}

/// Le proxy manuel est-il un SOCKS5 ?
fn is_socks(url: &str) -> bool {
    url.starts_with("socks5://") || url.starts_with("socks5h://")
}

/// Adresse complète du proxy manuel : identifiant et mot de passe dans l'adresse pour
/// SOCKS5 ; `public_only` force la résolution locale (`socks5://`).
fn manual_url(effective: &Effective, public_only: bool) -> Option<Url> {
    let text = effective.settings.url.as_deref()?;
    let mut url = Url::parse(text).ok()?;
    if public_only && url.scheme() == "socks5h" {
        url = Url::parse(&text.replacen("socks5h://", "socks5://", 1)).ok()?;
    }
    if is_socks(url.as_str())
        && let Some(user) = &effective.settings.username
    {
        let _ = url.set_username(user);
        let _ = url.set_password(effective.password.as_deref());
    }
    Some(url)
}

/// Le proxy suivi par une connexion : `None` en direct.
pub fn route(effective: &Effective, public_only: bool) -> Route {
    match effective.settings.mode {
        // Le proxy du système (souvent HTTP) résoudrait les noms lui-même.
        Mode::System if public_only => Route::Direct,
        Mode::System => Route::System,
        Mode::None => Route::Direct,
        Mode::Manual => match effective.settings.url.as_deref() {
            Some(url) if !public_only || is_socks(url) => Route::Manual,
            _ => Route::Direct,
        },
    }
}

/// Chemin d'une connexion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Direct,
    System,
    Manual,
}

/// Applique le proxy à un client en construction. Un client restreint au réseau public
/// (`public_only`) a déjà son résolveur filtrant.
pub fn apply(
    builder: ClientBuilder,
    effective: &Effective,
    public_only: bool,
) -> Result<ClientBuilder, String> {
    match route(effective, public_only) {
        // reqwest suit de lui-même le proxy du système.
        Route::System => Ok(builder),
        Route::Direct => Ok(builder.no_proxy()),
        Route::Manual => {
            let url = manual_url(effective, public_only).ok_or("adresse de proxy illisible")?;
            let socks = is_socks(url.as_str());
            let mut proxy = Proxy::all(url.as_str()).map_err(|e| e.to_string())?;
            if !effective.settings.exceptions.is_empty() {
                proxy = proxy.no_proxy(NoProxy::from_string(
                    &effective.settings.exceptions.join(","),
                ));
            }
            if !socks && let Some(user) = &effective.settings.username {
                proxy = proxy.basic_auth(user, effective.password.as_deref().unwrap_or_default());
            }
            Ok(builder.no_proxy().proxy(proxy))
        }
    }
}

/// Variables d'environnement d'un programme externe (yt-dlp) pour suivre le même proxy :
/// `None` laisse les siennes (proxy du système) ; sinon, les variables à poser (vides :
/// en direct). Passées par l'environnement, jamais en argument : les arguments d'un
/// processus se lisent par les autres utilisateurs de la machine.
pub fn environment(
    effective: &Effective,
    public_only: bool,
) -> Option<Vec<(&'static str, String)>> {
    let keys = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ];
    match route(effective, public_only) {
        Route::System => None,
        Route::Direct => Some(keys.iter().map(|k| (*k, String::new())).collect()),
        Route::Manual => {
            let mut url = manual_url(effective, public_only)?;
            if !is_socks(url.as_str())
                && let Some(user) = &effective.settings.username
            {
                let _ = url.set_username(user);
                let _ = url.set_password(effective.password.as_deref());
            }
            let text = url.as_str().trim_end_matches('/').to_owned();
            let mut vars: Vec<(&'static str, String)> =
                keys.iter().map(|k| (*k, text.clone())).collect();
            let exceptions = effective.settings.exceptions.join(",");
            vars.push(("NO_PROXY", exceptions.clone()));
            vars.push(("no_proxy", exceptions));
            Some(vars)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual(url: &str) -> Effective {
        Effective {
            settings: Settings {
                mode: Mode::Manual,
                url: Some(url.into()),
                exceptions: vec!["localhost".into(), ".lan".into()],
                username: Some("moi".into()),
            },
            password: Some("p@ss w".into()),
        }
    }

    #[test]
    fn settings_are_checked() {
        assert!(validate(&Settings::default()).is_ok());
        for url in [
            "http://p.lan:3128",
            "https://p.lan:443",
            "socks5://10.0.0.1:1080",
            "socks5h://p:1080/",
        ] {
            let s = Settings {
                mode: Mode::Manual,
                url: Some(url.into()),
                ..Settings::default()
            };
            assert!(validate(&s).is_ok(), "{url}");
        }
        for url in [
            "ftp://p:21",
            "socks4://p:1080",
            "http://moi:x@p:3128",
            "http://p:3128/chemin",
            "pas une adresse",
            "http://:3128",
        ] {
            let s = Settings {
                mode: Mode::Manual,
                url: Some(url.into()),
                ..Settings::default()
            };
            assert!(validate(&s).is_err(), "{url}");
        }
        let none = Settings {
            mode: Mode::Manual,
            ..Settings::default()
        };
        assert!(validate(&none).is_err());
        let bad = Settings {
            exceptions: vec!["a b".into()],
            ..Settings::default()
        };
        assert!(validate(&bad).is_err());
        for mode in ["system", "none", "manual"] {
            assert_eq!(Mode::parse(mode).unwrap().as_str(), mode);
        }
        // Le mot de passe ne se montre pas.
        assert!(!format!("{:?}", manual("http://p:1")).contains("p@ss"));
    }

    #[test]
    fn public_connections_never_go_through_an_http_proxy() {
        let http = manual("http://p.lan:3128");
        assert_eq!(route(&http, false), Route::Manual);
        assert_eq!(route(&http, true), Route::Direct);
        let socks = manual("socks5h://p.lan:1080");
        assert_eq!(route(&socks, true), Route::Manual);
        // Résolution locale forcée, identifiant dans l'adresse.
        let url = manual_url(&socks, true).unwrap();
        assert_eq!(url.scheme(), "socks5");
        assert_eq!(url.username(), "moi");
        let system = Effective::default();
        assert_eq!(route(&system, false), Route::System);
        assert_eq!(route(&system, true), Route::Direct);
    }

    #[test]
    fn external_programs_get_the_same_proxy_by_their_environment() {
        assert!(environment(&Effective::default(), false).is_none());
        let direct = environment(&Effective::default(), true).unwrap();
        assert!(direct.iter().all(|(_, v)| v.is_empty()));
        let vars = environment(&manual("http://p.lan:3128"), false).unwrap();
        let https = &vars.iter().find(|(k, _)| *k == "HTTPS_PROXY").unwrap().1;
        assert_eq!(https, "http://moi:p%40ss%20w@p.lan:3128");
        let no = &vars.iter().find(|(k, _)| *k == "NO_PROXY").unwrap().1;
        assert_eq!(no, "localhost,.lan");
    }
}
