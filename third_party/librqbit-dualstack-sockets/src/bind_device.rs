// Modifié par AuroDownloader (2026-11-09) : liaison à une interface sous Windows, que la
// version 0.7.0 refusait (`BindDeviceNotSupported`). Voir third_party/README.md.

#[cfg(test)]
pub(crate) mod tests;

use crate::Error;
#[cfg(windows)]
use std::net::{IpAddr, SocketAddr};
use std::{ffi::CString, num::NonZeroU32, str::FromStr};

#[derive(Debug, Clone)]
pub struct BindDevice {
    #[allow(unused)]
    index: NonZeroU32,
    #[allow(unused)]
    name: CString,
    /// Windows : adresses de l'interface (hors adresses locales au lien), auxquelles les
    /// sockets se lient. `IP_UNICAST_IF` ne choisit que l'interface de sortie : liée à
    /// l'adresse, une socket ne reçoit que ce qui arrive sur l'interface, et le modèle
    /// d'hôte strict de Windows n'envoie que par elle.
    #[cfg(windows)]
    addrs: Vec<IpAddr>,
}

impl BindDevice {
    #[cfg(not(windows))]
    pub fn new_from_name(name: &str) -> crate::Result<Self> {
        let name = CString::new(name).map_err(|_| Error::BindDeviceInvalid)?;

        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        let index = NonZeroU32::new(index)
            .ok_or_else(|| Error::BindDeviceInvalidError(std::io::Error::last_os_error()))?;
        Ok(Self { index, name })
    }

    /// Windows : l'interface est désignée par son nom affiché (« Wi-Fi », celui d'un VPN).
    /// Sans adresse utilisable (VPN déconnecté), elle est refusée.
    #[cfg(windows)]
    pub fn new_from_name(name: &str) -> crate::Result<Self> {
        use network_interface::{NetworkInterface, NetworkInterfaceConfig};

        let cname = CString::new(name).map_err(|_| Error::BindDeviceInvalid)?;
        let nics = NetworkInterface::show()
            .map_err(|e| Error::BindDeviceInvalidError(std::io::Error::other(e)))?;
        let nic = nics.iter().find(|n| n.name == name).ok_or_else(|| {
            Error::BindDeviceInvalidError(std::io::Error::from(std::io::ErrorKind::NotFound))
        })?;
        let index = NonZeroU32::new(nic.index).ok_or(Error::BindDeviceInvalid)?;
        let addrs: Vec<IpAddr> = nic
            .addr
            .iter()
            .map(|a| a.ip())
            .filter(|ip| usable(ip))
            .collect();
        if addrs.is_empty() {
            return Err(Error::BindDeviceInvalidError(std::io::Error::from(
                std::io::ErrorKind::AddrNotAvailable,
            )));
        }
        Ok(Self {
            index,
            name: cname,
            addrs,
        })
    }

    pub fn index(&self) -> NonZeroU32 {
        self.index
    }

    pub fn name(&self) -> &str {
        // We constructed from a string so this can't fail
        unsafe { std::str::from_utf8_unchecked(self.name.to_bytes()) }
    }

    /// Windows : l'adresse de l'interface où lier une socket demandée sur `addr`. Une
    /// adresse non précisée (`0.0.0.0`, `[::]`) devient celle de l'interface, IPv4 d'abord
    /// (une socket « double pile » n'est alors plus qu'IPv4) ; le port est gardé. Une
    /// adresse précise reste telle quelle. Sans adresse de la bonne famille : `None`.
    #[cfg(windows)]
    pub fn local_addr_for(&self, addr: SocketAddr) -> Option<SocketAddr> {
        if !addr.ip().is_unspecified() {
            return Some(addr);
        }
        let v4 = self.addrs.iter().find(|ip| ip.is_ipv4());
        let v6 = self.addrs.iter().find(|ip| ip.is_ipv6());
        let ip = if addr.is_ipv4() { v4 } else { v4.or(v6) }?;
        Some(SocketAddr::new(*ip, addr.port()))
    }

    /// Windows : l'adresse de l'interface pour joindre `peer` (même famille).
    #[cfg(windows)]
    pub fn source_for(&self, peer: &SocketAddr) -> Option<IpAddr> {
        self.addrs
            .iter()
            .find(|ip| ip.is_ipv4() == peer.is_ipv4())
            .copied()
    }

    /// Windows : la première adresse IPv4 de l'interface, sinon IPv6 (client HTTP des
    /// trackers).
    #[cfg(windows)]
    pub fn preferred_addr(&self) -> Option<IpAddr> {
        let v4 = self.addrs.iter().find(|ip| ip.is_ipv4());
        v4.or(self.addrs.first()).copied()
    }

    #[cfg(target_os = "macos")]
    pub fn bind_sref(&self, sref: &socket2::Socket, is_v6: bool) -> crate::Result<()> {
        if is_v6 {
            sref.bind_device_by_index_v6(Some(self.index))
                .map_err(Error::BindDeviceSetDeviceError)
        } else {
            sref.bind_device_by_index_v4(Some(self.index))
                .map_err(Error::BindDeviceSetDeviceError)
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub fn bind_sref(&self, sref: &socket2::Socket, _is_v6: bool) -> crate::Result<()> {
        let name = self.name.as_bytes_with_nul();
        sref.bind_device(Some(name))
            .map_err(Error::BindDeviceSetDeviceError)
    }

    /// Windows : interface de sortie par `IP_UNICAST_IF` (index en ordre réseau) et, pour
    /// une socket IPv6, `IPV6_UNICAST_IF` (ordre de l'hôte) ; une socket IPv6 double pile
    /// envoie aussi en IPv4 et reçoit les deux. Les appelants lient ensuite la socket à
    /// l'adresse de l'interface ([`BindDevice::local_addr_for`]).
    #[cfg(windows)]
    pub fn bind_sref(&self, sref: &socket2::Socket, is_v6: bool) -> crate::Result<()> {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{
            IP_UNICAST_IF, IPPROTO_IP, IPPROTO_IPV6, IPV6_UNICAST_IF, SOCKET, WSAGetLastError,
            setsockopt,
        };

        let socket = sref.as_raw_socket() as SOCKET;
        let set = |level: i32, option: i32, value: u32| -> crate::Result<()> {
            // SAFETY : `value` vit pendant l'appel, dont la taille est passée.
            let done = unsafe {
                setsockopt(
                    socket,
                    level,
                    option,
                    (&value as *const u32).cast(),
                    std::mem::size_of::<u32>() as i32,
                )
            };
            if done == 0 {
                Ok(())
            } else {
                let code = unsafe { WSAGetLastError() };
                Err(Error::BindDeviceSetDeviceError(
                    std::io::Error::from_raw_os_error(code),
                ))
            }
        };
        let index = self.index.get();
        if is_v6 {
            set(IPPROTO_IPV6, IPV6_UNICAST_IF, index)?;
            // Double pile : le trafic IPv4 d'une socket IPv6 suit `IP_UNICAST_IF`. Une socket
            // seulement IPv6 le refuse, ce qui ne change rien pour elle.
            let _ = set(IPPROTO_IP, IP_UNICAST_IF, index.to_be());
            Ok(())
        } else {
            set(IPPROTO_IP, IP_UNICAST_IF, index.to_be())
        }
    }
}

/// Une adresse où l'interface est joignable : pas une adresse locale au lien (APIPA
/// 169.254/16, fe80::/10), que Windows donne à une carte sans configuration.
#[cfg(windows)]
fn usable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_link_local() && !v4.is_unspecified(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80 && !v6.is_unspecified(),
    }
}

impl FromStr for BindDevice {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new_from_name(s)
    }
}
