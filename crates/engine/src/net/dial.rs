//! Connexions TCP que le moteur ouvre lui-même (FTP, SFTP : F-08, SPEC §7.22) : en direct
//! ou par un proxy SOCKS5 manuel, et réseau public seulement pour les tâches à jeton
//! (SPEC §7.19). Les connexions HTTP passent par reqwest ([`super::proxy::apply`]).

use std::net::{IpAddr, SocketAddr};

use reqwest::Url;
use tokio::net::TcpStream;

use super::socks::{self, Target as SocksTarget};
use super::{NetError, proxy, reach};

/// Chemin réseau des connexions vers un serveur.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Route {
    Direct {
        /// Réseau public seulement : adresses résolues et vérifiées ici.
        public_only: bool,
    },
    Socks {
        proxy: String,
        login: Option<(String, String)>,
        /// Résolution locale (`socks5://`) ; sinon le proxy résout (`socks5h://`).
        local: bool,
        public_only: bool,
    },
}

impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Route::Direct { public_only } => write!(f, "direct:{public_only}"),
            Route::Socks {
                proxy,
                login,
                local,
                public_only,
            } => write!(
                f,
                "socks:{proxy}:{}:{local}:{public_only}",
                login.as_ref().map_or("", |(u, _)| u.as_str())
            ),
        }
    }
}

/// L'hôte est-il une exception du proxy manuel (joint en direct) ? `*`, un nom (et ses
/// sous-domaines), `.domaine`, une adresse ou un réseau `adresse/longueur`.
pub(crate) fn excepted(host: &str, exceptions: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let ip: Option<IpAddr> = host.parse().ok();
    exceptions.iter().any(|e| {
        let e = e.trim().trim_end_matches('.').to_ascii_lowercase();
        if e == "*" {
            return true;
        }
        if let Some((net, bits)) = e.split_once('/') {
            let (Ok(net), Ok(bits), Some(ip)) = (net.parse::<IpAddr>(), bits.parse::<u32>(), ip)
            else {
                return false;
            };
            return in_network(ip, net, bits);
        }
        let bare = e.trim_start_matches('.');
        host == bare || host.ends_with(&format!(".{bare}"))
    })
}

pub(crate) fn in_network(ip: IpAddr, net: IpAddr, bits: u32) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) if bits <= 32 => {
            let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
            u32::from(a) & mask == u32::from(n) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(n)) if bits <= 128 => {
            let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

/// Le chemin des connexions FTP et SFTP vers `host` : SOCKS5 du proxy manuel, sinon en direct
/// (proxy HTTP ou du système : décision de l'utilisateur du 2026-11-17).
pub(crate) fn route_for(effective: &proxy::Effective, public_only: bool, host: &str) -> Route {
    let direct = Route::Direct { public_only };
    if proxy::route(effective, public_only) != proxy::Route::Manual {
        return direct;
    }
    let Some(url) = effective
        .settings
        .url
        .as_deref()
        .and_then(|u| Url::parse(u).ok())
    else {
        return direct;
    };
    if !matches!(url.scheme(), "socks5" | "socks5h")
        || excepted(host, &effective.settings.exceptions)
    {
        return direct;
    }
    let (Some(proxy_host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return direct;
    };
    Route::Socks {
        proxy: format!(
            "{}:{port}",
            proxy_host.trim_start_matches('[').trim_end_matches(']')
        ),
        login: effective
            .settings
            .username
            .clone()
            .map(|u| (u, effective.password.clone().unwrap_or_default())),
        local: public_only || url.scheme() == "socks5",
        public_only,
    }
}

pub(crate) fn net_error(error: std::io::Error, connect: bool) -> NetError {
    if error.kind() == std::io::ErrorKind::TimedOut {
        return NetError::Socket {
            connect,
            text: "délai dépassé".into(),
        };
    }
    NetError::Socket {
        connect,
        text: error.to_string(),
    }
}

/// Adresses de `host` ; avec `public_only`, celles du réseau public seulement.
pub(crate) async fn resolve(
    host: &str,
    port: u16,
    public_only: bool,
) -> Result<Vec<SocketAddr>, NetError> {
    let found: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| net_error(e, true))?
            .collect(),
    };
    if public_only {
        let name = host.trim_end_matches('.').to_ascii_lowercase();
        if name == "localhost" || name.ends_with(".localhost") {
            return Err(NetError::AddressForbidden);
        }
        let public: Vec<_> = found
            .into_iter()
            .filter(|a| reach::is_public(a.ip()))
            .collect();
        if public.is_empty() {
            return Err(NetError::AddressForbidden);
        }
        return Ok(public);
    }
    if found.is_empty() {
        return Err(NetError::BadResponse("nom d'hôte introuvable"));
    }
    Ok(found)
}

pub(crate) async fn connect_any(addrs: &[SocketAddr]) -> Result<TcpStream, NetError> {
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(net_error(
        last.unwrap_or_else(|| std::io::Error::other("aucune adresse")),
        true,
    ))
}

/// Ouvre une connexion TCP vers `host:port` par `route`. `peer` : adresse déjà joignable
/// (connexion de données en direct, celle de la connexion de commande).
pub(crate) async fn dial(
    route: &Route,
    host: &str,
    port: u16,
    peer: Option<SocketAddr>,
) -> Result<TcpStream, NetError> {
    match route {
        Route::Direct { public_only } => match peer {
            Some(addr) => {
                if *public_only && !reach::is_public(addr.ip()) {
                    return Err(NetError::AddressForbidden);
                }
                connect_any(&[addr]).await
            }
            None => connect_any(&resolve(host, port, *public_only).await?).await,
        },
        Route::Socks {
            proxy,
            login,
            local,
            public_only,
        } => {
            let target = if *local {
                let addrs = resolve(host, port, *public_only).await?;
                SocksTarget::Addr(addrs[0])
            } else {
                SocksTarget::Name(host.to_owned(), port)
            };
            let stream = TcpStream::connect(proxy.as_str())
                .await
                .map_err(|e| NetError::Proxy(format!("proxy SOCKS5 injoignable : {e}")))?;
            let login = login.as_ref().map(|(u, p)| (u.as_str(), p.as_str()));
            socks::handshake(stream, &target, login)
                .await
                .map_err(|e| net_error(e, true))
        }
    }
}

/// Réseau public seulement ?
pub(crate) fn route_public(route: &Route) -> bool {
    match route {
        Route::Direct { public_only } | Route::Socks { public_only, .. } => *public_only,
    }
}
