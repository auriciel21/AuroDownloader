//! Adresses joignables par une tâche venue d'une connexion à jeton (SPEC §7.19, SSRF).
//!
//! Un client distant ne doit pas faire du moteur un relais vers le réseau interne de sa
//! machine (NAS, box, services sur la boucle locale). Pour ses tâches, le moteur refuse
//! les adresses de boucle locale, privées, de lien local, partagées (CGNAT), non
//! spécifiées et de multidiffusion. La vérification se fait sur les adresses obtenues par
//! la résolution DNS, à chaque connexion et à chaque redirection (un nom qui changerait de
//! réponse entre deux requêtes est vérifié à chacune), et sur les adresses écrites telles
//! quelles dans l'URL, que reqwest ne fait pas résoudre.

use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use reqwest::Url;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Cause d'un refus, posée dans la chaîne d'erreurs de reqwest (résolveur, redirection)
/// pour être reconnue par `NetError::from_reqwest`.
#[derive(Debug)]
pub(crate) struct Forbidden;

impl fmt::Display for Forbidden {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("adresse du réseau interne refusée")
    }
}

impl Error for Forbidden {}

/// Une adresse du réseau public : ni boucle locale, ni privée, ni lien local, ni partagée
/// (CGNAT), ni non spécifiée, ni multidiffusion, ni réservée. Une adresse IPv4 écrite en
/// IPv6 (`::ffff:a.b.c.d`, NAT64 `64:ff9b::/96`) est jugée sur son adresse IPv4.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let s = v6.segments();
            // NAT64 (RFC 6052) : l'adresse IPv4 est dans les 32 derniers bits.
            if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0; 4] {
                let o = v6.octets();
                return is_public_v4(Ipv4Addr::new(o[12], o[13], o[14], o[15]));
            }
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                // Ancienne forme compatible IPv4 (`::a.b.c.d`), dépréciée.
                || s[..6] == [0; 6]
                // Unique locale fc00::/7, lien local fe80::/10, site local fec0::/10.
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xffc0) == 0xfec0
                // Documentation 2001:db8::/32.
                || (s[0] == 0x2001 && s[1] == 0x0db8))
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
        // « Ce réseau » 0.0.0.0/8.
        || a == 0
        // Espace partagé des opérateurs (CGNAT) 100.64.0.0/10.
        || (a == 100 && (64..128).contains(&b))
        // Réservées au protocole 192.0.0.0/24, bancs d'essai 198.18.0.0/15, futures
        // 240.0.0.0/4.
        || (a == 192 && b == 0 && ip.octets()[2] == 0)
        || (a == 198 && (18..20).contains(&b))
        || a >= 240)
}

/// L'hôte de `url` est-il une adresse écrite telle quelle, hors du réseau public ? Un nom
/// d'hôte passe ici : il est vérifié après sa résolution, par [`PublicResolver`].
pub fn literal_forbidden(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return !is_public(ip);
    }
    // `localhost` et ses sous-domaines désignent la machine elle-même (RFC 6761),
    // quelle que soit la réponse du résolveur.
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    name == "localhost" || name.ends_with(".localhost")
}

/// Vérifie une adresse joignable par un programme externe (yt-dlp), qui résout les noms
/// lui-même : l'hôte, écrit tel quel ou résolu ici, ne doit désigner que le réseau
/// public. Un nom dont la réponse changerait ensuite échappe à ce contrôle (limite connue,
/// SPEC §7.19).
pub async fn check_public(url: &Url) -> Result<(), super::NetError> {
    if literal_forbidden(url) {
        return Err(super::NetError::AddressForbidden);
    }
    let Some(host) = url.host_str() else {
        return Err(super::NetError::InvalidUrl("adresse sans hôte".into()));
    };
    if host.starts_with('[') || host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let found: Vec<SocketAddr> = tokio::net::lookup_host((host, 0))
        .await
        .map_err(|_| super::NetError::BadResponse("nom d'hôte introuvable"))?
        .collect();
    if found.is_empty() || found.iter().any(|a| !is_public(a.ip())) {
        return Err(super::NetError::AddressForbidden);
    }
    Ok(())
}

/// Résolveur des clients restreints : celui du système, dont les réponses hors du réseau
/// public sont écartées ; aucune réponse publique : la connexion est refusée.
#[derive(Debug, Default)]
pub(crate) struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let found: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let public: Vec<SocketAddr> = found
                .into_iter()
                .filter(|addr| is_public(addr.ip()))
                .collect();
            if public.is_empty() {
                return Err(Box::new(Forbidden) as Box<dyn Error + Send + Sync>);
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_addresses_are_not_public() {
        for ip in [
            "127.0.0.1",
            "127.8.9.10",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "0.0.0.0",
            "0.1.2.3",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.0.8",
            "198.18.0.1",
            "240.0.0.1",
            "::",
            "::1",
            "::127.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:192.168.0.1",
            "64:ff9b::a00:1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.1",
            "172.32.0.1",
            "151.101.1.1",
            "2a00:1450:4007::1",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn literal_hosts_are_checked_and_names_left_to_the_resolver() {
        for url in [
            "http://127.0.0.1/f",
            "http://[::1]:8080/f",
            "https://192.168.1.10/f",
            "http://[::ffff:10.0.0.1]/f",
            "http://localhost/f",
            "http://LOCALHOST./f",
            "http://box.localhost/f",
            // Formes abrégées ou décimales : reqwest (url) les normalise en IPv4.
            "http://2130706433/f",
            "http://127.1/f",
            "http://0x7f.0.0.1/f",
        ] {
            assert!(literal_forbidden(&Url::parse(url).unwrap()), "{url}");
        }
        for url in [
            "https://deb.debian.org/f",
            "http://8.8.8.8/f",
            "http://[2a00:1450:4007::1]/f",
            "http://localhost.example/f",
        ] {
            assert!(!literal_forbidden(&Url::parse(url).unwrap()), "{url}");
        }
    }

    #[tokio::test]
    async fn the_resolver_refuses_names_that_only_point_inside() {
        let refused = PublicResolver
            .resolve("localhost".parse().unwrap())
            .await
            .err()
            .expect("localhost refusé");
        assert!(refused.is::<Forbidden>());
    }
}
