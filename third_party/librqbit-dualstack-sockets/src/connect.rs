// Modifié par AuroDownloader (2026-11-09) : sous Windows, une connexion liée à une interface
// part de l'adresse de celle-ci. Voir third_party/README.md.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use socket2::SockRef;

use crate::{Error, bind_device::BindDevice};

#[derive(Clone, Copy, Debug, Default)]
pub struct ConnectOpts<'a> {
    pub source_port: Option<u16>,
    pub bind_device: Option<&'a BindDevice>,
}

pub async fn tcp_connect<'a>(
    addr: SocketAddr,
    opts: ConnectOpts<'a>,
) -> crate::Result<tokio::net::TcpStream> {
    let (sock, bind_addr) = if addr.is_ipv6() {
        (
            tokio::net::TcpSocket::new_v6().map_err(Error::SocketNew)?,
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, opts.source_port.unwrap_or(0))),
        )
    } else {
        (
            tokio::net::TcpSocket::new_v4().map_err(Error::SocketNew)?,
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, opts.source_port.unwrap_or(0))),
        )
    };
    let sref = SockRef::from(&sock);

    if let Some(bd) = opts.bind_device {
        bd.bind_sref(&sref, addr.is_ipv6())?;
    }

    // Windows : l'adresse de l'interface comme source, même sans port choisi ; sans adresse
    // de la famille du pair, pas de connexion (jamais par une autre interface).
    #[cfg(windows)]
    let bind_addr = match opts.bind_device {
        Some(bd) => {
            let ip = bd.source_for(&addr).ok_or_else(|| {
                Error::BindDeviceSetDeviceError(std::io::Error::from(
                    std::io::ErrorKind::AddrNotAvailable,
                ))
            })?;
            if bind_addr.port() == 0 {
                sref.bind(&SocketAddr::new(ip, 0).into())
                    .map_err(Error::Bind)?;
            }
            SocketAddr::new(ip, bind_addr.port())
        }
        None => bind_addr,
    };

    if bind_addr.port() > 0 {
        #[cfg(not(windows))]
        sref.set_reuse_port(true).map_err(Error::ReusePort)?;
        sref.set_reuse_address(true).map_err(Error::ReuseAddress)?;
        sref.bind(&bind_addr.into()).map_err(Error::Bind)?;
    }

    sock.connect(addr).await.map_err(Error::Connect)
}
