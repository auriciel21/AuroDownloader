//! Client SOCKS5 minimal (RFC 1928, identifiant RFC 1929) des connexions que le moteur ouvre
//! lui-même : commande et données FTP (F-08, SPEC §7.22). Les connexions HTTP passent par
//! celui de reqwest.

use std::io::{Error, ErrorKind};
use std::net::{IpAddr, SocketAddr};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Destination demandée au proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Un nom, que le proxy résout (`socks5h://`).
    Name(String, u16),
    /// Une adresse résolue par le moteur (`socks5://`, réseau public vérifié).
    Addr(SocketAddr),
}

/// Refus du proxy lui-même (méthode, identifiant) : définitif ([`super::dial`]).
fn refused(text: &str) -> Error {
    Error::new(
        ErrorKind::PermissionDenied,
        format!("proxy SOCKS5 : {text}"),
    )
}

/// Réponse qui n'est pas du SOCKS5 : définitif aussi.
fn garbled() -> Error {
    Error::new(ErrorKind::InvalidData, "proxy SOCKS5 : réponse illisible")
}

/// Demande au proxy déjà joint par `stream` une connexion vers `target`. `login` :
/// identifiant et mot de passe, proposés au proxy s'il en veut.
pub async fn handshake(
    mut stream: TcpStream,
    target: &Target,
    login: Option<(&str, &str)>,
) -> std::io::Result<TcpStream> {
    // Méthodes proposées : sans identification, et avec si on en a un.
    let greeting: &[u8] = if login.is_some() {
        &[5, 2, 0, 2]
    } else {
        &[5, 1, 0]
    };
    stream.write_all(greeting).await?;
    let mut chosen = [0u8; 2];
    stream.read_exact(&mut chosen).await?;
    if chosen[0] != 5 {
        return Err(garbled());
    }
    match (chosen[1], login) {
        (0, _) => {}
        (2, Some((user, pass))) => {
            let (user, pass) = (user.as_bytes(), pass.as_bytes());
            if user.len() > 255 || pass.len() > 255 {
                return Err(refused("identifiant trop long"));
            }
            let mut message = vec![1, user.len() as u8];
            message.extend_from_slice(user);
            message.push(pass.len() as u8);
            message.extend_from_slice(pass);
            stream.write_all(&message).await?;
            let mut status = [0u8; 2];
            stream.read_exact(&mut status).await?;
            // RFC 1929 : version 1 de la sous-négociation, puis 0 pour « accepté ».
            if status[0] != 1 {
                return Err(garbled());
            }
            if status[1] != 0 {
                return Err(refused("identifiant refusé"));
            }
        }
        _ => return Err(refused("aucune méthode d'identification acceptée")),
    }
    let mut request = vec![5, 1, 0];
    let port = match target {
        Target::Name(name, port) => {
            let name = name.as_bytes();
            if name.is_empty() || name.len() > 255 {
                return Err(refused("nom d'hôte trop long"));
            }
            request.push(3);
            request.push(name.len() as u8);
            request.extend_from_slice(name);
            *port
        }
        Target::Addr(addr) => {
            match addr.ip() {
                IpAddr::V4(ip) => {
                    request.push(1);
                    request.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    request.push(4);
                    request.extend_from_slice(&ip.octets());
                }
            }
            addr.port()
        }
    };
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await?;
    if reply[0] != 5 {
        return Err(garbled());
    }
    if reply[1] != 0 {
        let kind = match reply[1] {
            2 => ErrorKind::PermissionDenied,
            3 | 4 => ErrorKind::HostUnreachable,
            5 => ErrorKind::ConnectionRefused,
            6 => ErrorKind::TimedOut,
            _ => ErrorKind::Other,
        };
        return Err(Error::new(
            kind,
            format!("proxy SOCKS5 : connexion refusée (code {})", reply[1]),
        ));
    }
    // Adresse liée par le proxy, lue et ignorée.
    let skip = match reply[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            usize::from(len[0])
        }
        _ => return Err(garbled()),
    };
    let mut bound = vec![0u8; skip + 2];
    stream.read_exact(&mut bound).await?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::proxies::socks5_proxy;

    async fn echo_server() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        addr
    }

    async fn round_trip(stream: TcpStream) -> Vec<u8> {
        let mut stream = stream;
        stream.write_all(b"bonjour").await.unwrap();
        let mut back = [0u8; 7];
        stream.read_exact(&mut back).await.unwrap();
        back.to_vec()
    }

    #[tokio::test]
    async fn connects_by_address_and_by_name() {
        let server = echo_server().await;
        let proxy = socks5_proxy(None);
        let s = TcpStream::connect(proxy.addr).await.unwrap();
        let s = handshake(s, &Target::Addr(server), None).await.unwrap();
        assert_eq!(round_trip(s).await, b"bonjour");
        let s = TcpStream::connect(proxy.addr).await.unwrap();
        let named = Target::Name("localhost".into(), server.port());
        let s = handshake(s, &named, None).await.unwrap();
        assert_eq!(round_trip(s).await, b"bonjour");
        let seen = proxy.seen();
        assert_eq!(seen[0].target, server.to_string());
        assert!(!seen[0].by_name);
        assert_eq!(seen[1].target, format!("localhost:{}", server.port()));
        assert!(seen[1].by_name);
    }

    #[tokio::test]
    async fn gives_the_login_and_reports_a_refusal() {
        let server = echo_server().await;
        let proxy = socks5_proxy(Some(("moi", "secret")));
        let s = TcpStream::connect(proxy.addr).await.unwrap();
        let s = handshake(s, &Target::Addr(server), Some(("moi", "secret")))
            .await
            .unwrap();
        assert_eq!(round_trip(s).await, b"bonjour");
        assert_eq!(proxy.seen()[0].credentials.as_deref(), Some("moi"));

        let s = TcpStream::connect(proxy.addr).await.unwrap();
        let wrong = handshake(s, &Target::Addr(server), Some(("moi", "faux"))).await;
        assert!(wrong.unwrap_err().to_string().contains("refusé"));
        let s = TcpStream::connect(proxy.addr).await.unwrap();
        let none = handshake(s, &Target::Addr(server), None).await;
        assert!(none.is_err());
    }
}
