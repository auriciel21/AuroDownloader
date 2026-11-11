//! TLS de l'interface distante (SPEC §7.19, étape 3b).
//!
//! Hors de la boucle locale, l'écoute exige TLS. Le certificat est soit fourni par
//! l'utilisateur (réglages `remote.tlsCert` et `remote.tlsKey`, fichiers PEM lus à chaque
//! ouverture de l'écoute), soit fabriqué par le moteur (décisions de l'utilisateur du
//! 2026-11-11) :
//! - auto-signé, ECDSA P-256, 824 jours (les iPhone refusent plus de 825 jours), usage
//!   « serveur » ; noms : `localhost`, le nom de la machine (et `<nom>.local`), les noms de
//!   `remote.hosts`, la boucle locale, et l'adresse d'écoute, ou pour une écoute sur toutes
//!   les adresses celles des interfaces ;
//! - gardé à côté de la base : `remote-cert.pem`, et `remote-key.pem` lisible par
//!   l'utilisateur seul (`0600`, ou sous Windows un descripteur qui n'autorise que lui) ;
//! - refait seul 30 jours avant son expiration, ou quand un de ses noms manque (adresse
//!   d'écoute changée, nouvelle adresse d'interface) ; l'empreinte change alors.

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};

use crate::store::settings::valid_host_name;

/// Certificat fabriqué, à côté de la base.
pub const CERT_FILE: &str = "remote-cert.pem";
/// Sa clé privée, lisible par l'utilisateur seul.
pub const KEY_FILE: &str = "remote-key.pem";
/// Durée d'un certificat fabriqué : 824 jours (825 au plus pour les iPhone).
pub(crate) const VALIDITY_S: i64 = 824 * 24 * 3600;
/// Début de validité avancé d'une heure : une horloge un peu en retard l'accepte.
pub(crate) const BACKDATE_S: i64 = 3600;
/// Un certificat fabriqué se refait 30 jours avant son expiration.
pub const RENEW_BEFORE_MS: i64 = 30 * 24 * 3600 * 1000;

/// Un certificat prêt à servir.
#[derive(Clone)]
pub struct Tls {
    pub config: Arc<ServerConfig>,
    /// SHA-256 du certificat (DER), en hexadécimal par paires : ce que les navigateurs
    /// montrent, à comparer.
    pub fingerprint: String,
    /// Fin de validité, en millisecondes Unix.
    pub not_after_ms: i64,
    /// Noms et adresses portés par le certificat.
    pub names: Vec<String>,
    /// Fabriqué par le moteur (sinon fourni).
    pub generated: bool,
    /// Fabriqué à l'instant.
    pub renewed: bool,
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tls")
            .field("fingerprint", &self.fingerprint)
            .field("not_after_ms", &self.not_after_ms)
            .field("names", &self.names)
            .field("generated", &self.generated)
            .field("renewed", &self.renewed)
            .finish_non_exhaustive()
    }
}

impl Tls {
    /// Quand refaire ce certificat : 30 jours avant son expiration s'il est fabriqué ;
    /// jamais pour un certificat fourni.
    pub fn renew_at_ms(&self) -> Option<i64> {
        self.generated.then(|| self.not_after_ms - RENEW_BEFORE_MS)
    }
}

/// Noms qu'un certificat fabriqué doit porter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names {
    pub dns: Vec<String>,
    pub ips: Vec<IpAddr>,
}

/// Noms qui désignent ce serveur dans `Host` (en plus des adresses IP) : `localhost`, le
/// nom de la machine et `<nom>.local`, et les noms DNS de `remote.hosts`.
pub fn known_names(hosts: &[String]) -> Vec<String> {
    let mut names = vec!["localhost".to_owned()];
    names.extend(machine_names());
    for host in hosts {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host.parse::<IpAddr>().is_err() && !names.contains(&host) {
            names.push(host);
        }
    }
    names
}

/// Noms et adresses du certificat fabriqué pour une écoute sur `listen`.
pub fn wanted_names(listen: SocketAddr, hosts: &[String]) -> Names {
    let mut ips = vec![
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ];
    ips.extend(hosts.iter().filter_map(|h| h.parse::<IpAddr>().ok()));
    if listen.ip().is_unspecified() {
        ips.extend(interface_addresses());
    } else {
        ips.push(listen.ip());
    }
    let mut seen = Vec::new();
    ips.retain(|ip| {
        let new = !seen.contains(ip);
        seen.push(*ip);
        new
    });
    Names {
        dns: known_names(hosts),
        ips,
    }
}

/// Adresses des interfaces de la machine, hors boucle locale et adresses locales au lien.
pub fn interface_addresses() -> Vec<IpAddr> {
    use network_interface::{NetworkInterface, NetworkInterfaceConfig};
    let Ok(nics) = NetworkInterface::show() else {
        return Vec::new();
    };
    let mut ips: Vec<IpAddr> = nics
        .iter()
        .flat_map(|n| n.addr.iter().map(|a| a.ip()))
        .filter(|ip| crate::torrent::usable_addr(ip) && !ip.is_loopback() && !ip.is_multicast())
        .collect();
    ips.sort();
    ips.dedup();
    ips
}

/// Le nom de la machine (en minuscules) et, s'il n'a pas de point, `<nom>.local` (mDNS) ;
/// rien si le système n'en donne pas d'utilisable.
fn machine_names() -> Vec<String> {
    let Some(name) = host_name() else {
        return Vec::new();
    };
    let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
    if name.is_empty()
        || name == "localhost"
        || name.parse::<IpAddr>().is_ok()
        || !valid_host_name(&name)
    {
        return Vec::new();
    }
    let mut names = vec![name.clone()];
    if !name.contains('.') {
        names.push(format!("{name}.local"));
    }
    names
}

#[cfg(windows)]
fn host_name() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::{
        ComputerNameDnsHostname, GetComputerNameExW,
    };
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    // SAFETY : `buf` fait `len` caractères ; `len` reçoit la longueur écrite.
    let ok = unsafe { GetComputerNameExW(ComputerNameDnsHostname, buf.as_mut_ptr(), &mut len) };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

#[cfg(unix)]
fn host_name() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY : `buf` fait la longueur donnée ; le nom est terminé par un zéro s'il tient.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if ok != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0)?;
    String::from_utf8(buf[..end].to_vec()).ok()
}

#[cfg(not(any(windows, unix)))]
fn host_name() -> Option<String> {
    None
}

/// Le certificat fabriqué de `dir`, refait s'il manque, s'il est illisible, s'il expire
/// dans moins de 30 jours ou s'il ne porte pas tous les `names`.
pub fn generated(dir: &Path, names: &Names, now_ms: i64) -> Result<Tls, String> {
    let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
    if cert_path.exists() || key_path.exists() {
        match read_pair(&cert_path, &key_path, true) {
            Ok(tls) if tls.not_after_ms - now_ms > RENEW_BEFORE_MS && covers(&tls, names) => {
                return Ok(tls);
            }
            Ok(_) => {}
            Err(e) => eprintln!("aurod : interface distante : certificat fabriqué refait ({e})"),
        }
    }
    let (cert_pem, key_pem) = generate(names, now_ms)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{} : {e}", dir.display()))?;
    // La clé d'abord : un arrêt entre les deux laisse une paire qui ne va pas ensemble,
    // refaite au prochain démarrage.
    write_file(&key_path, key_pem.as_bytes(), true)?;
    write_file(&cert_path, cert_pem.as_bytes(), false)?;
    let mut tls = read_pair(&cert_path, &key_path, true)?;
    tls.renewed = true;
    Ok(tls)
}

/// Le certificat fourni par l'utilisateur : chaîne et clé PEM. Refusé s'il a expiré, ou si
/// la clé ne va pas avec lui.
pub fn provided(cert: &Path, key: &Path, now_ms: i64) -> Result<Tls, String> {
    let tls = read_pair(cert, key, false)?;
    if tls.not_after_ms <= now_ms {
        return Err(format!(
            "{} : certificat expiré (fin de validité dépassée)",
            cert.display()
        ));
    }
    Ok(tls)
}

/// SHA-256 d'un certificat, en hexadécimal majuscule par paires (`AB:CD:…`).
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn read_pair(cert: &Path, key: &Path, generated: bool) -> Result<Tls, String> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .and_then(|certs| certs.collect())
        .map_err(|e| format!("{} : certificat illisible : {e}", cert.display()))?;
    let Some(leaf) = chain.first() else {
        return Err(format!("{} : aucun certificat", cert.display()));
    };
    let parsed = parse(leaf).map_err(|e| format!("{} : {e}", cert.display()))?;
    let fingerprint = fingerprint(leaf);
    // Jamais le contenu de la clé dans un message.
    let key_der = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| format!("{} : clé illisible : {e}", key.display()))?;
    let config = server_config(chain, key_der)?;
    Ok(Tls {
        config,
        fingerprint,
        not_after_ms: parsed.not_after_ms,
        names: parsed
            .dns
            .iter()
            .cloned()
            .chain(parsed.ips.iter().map(IpAddr::to_string))
            .collect(),
        generated,
        renewed: false,
    })
}

fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<ServerConfig>, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS : {e}"))?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| match e {
            rustls::Error::InconsistentKeys(_) => "la clé ne va pas avec le certificat".to_owned(),
            e => format!("certificat ou clé refusés : {e}"),
        })?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// Ce qui compte d'un certificat : sa fin de validité et ses noms.
struct Parsed {
    not_after_ms: i64,
    dns: Vec<String>,
    ips: Vec<IpAddr>,
}

fn parse(der: &[u8]) -> Result<Parsed, String> {
    use x509_parser::extensions::GeneralName;
    let (_, cert) = x509_parser::parse_x509_certificate(der)
        .map_err(|e| format!("certificat illisible : {e}"))?;
    let mut parsed = Parsed {
        not_after_ms: cert.validity().not_after.timestamp().saturating_mul(1000),
        dns: Vec::new(),
        ips: Vec::new(),
    };
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        for name in &san.value.general_names {
            match name {
                GeneralName::DNSName(dns) => parsed.dns.push(dns.to_ascii_lowercase()),
                GeneralName::IPAddress(bytes) => {
                    if let Ok(v4) = <[u8; 4]>::try_from(*bytes) {
                        parsed.ips.push(IpAddr::from(v4));
                    } else if let Ok(v6) = <[u8; 16]>::try_from(*bytes) {
                        parsed.ips.push(IpAddr::from(v6));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(parsed)
}

/// Le certificat porte-t-il tous ces noms ?
fn covers(tls: &Tls, names: &Names) -> bool {
    names.dns.iter().all(|d| tls.names.contains(d))
        && names
            .ips
            .iter()
            .all(|ip| tls.names.contains(&ip.to_string()))
}

fn generate(names: &Names, now_ms: i64) -> Result<(String, String), String> {
    use rcgen::{
        CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        KeyUsagePurpose, SerialNumber,
    };
    let fail = |e: rcgen::Error| format!("fabrication du certificat : {e}");
    let key = KeyPair::generate().map_err(fail)?;
    let sans: Vec<String> = names
        .dns
        .iter()
        .cloned()
        .chain(names.ips.iter().map(IpAddr::to_string))
        .collect();
    let mut params = CertificateParams::new(sans).map_err(fail)?;
    let mut subject = DistinguishedName::new();
    subject.push(DnType::CommonName, "AuroDownloader");
    params.distinguished_name = subject;
    let start = now_ms / 1000 - BACKDATE_S;
    let time = |s: i64| {
        time::OffsetDateTime::from_unix_timestamp(s)
            .map_err(|e| format!("fabrication du certificat : date : {e}"))
    };
    params.not_before = time(start)?;
    params.not_after = time(start + VALIDITY_S)?;
    let mut serial = [0u8; 16];
    aws_lc_rs::rand::fill(&mut serial).map_err(|_| "aléa du système indisponible".to_owned())?;
    // Entier positif et non nul (RFC 5280, 4.1.2.2).
    serial[0] = (serial[0] & 0x7f) | 0x01;
    params.serial_number = Some(SerialNumber::from_slice(&serial));
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let cert = params.self_signed(&key).map_err(fail)?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// Écrit un fichier en entier sous un nom provisoire, vidé sur disque, puis renommé ;
/// `private` : lisible par l'utilisateur seul dès sa création.
fn write_file(path: &Path, bytes: &[u8], private: bool) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{} : {e}", path.display());
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp_name);
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(fail(e)),
        _ => {}
    }
    let mut file = if private {
        create_private(&tmp).map_err(fail)?
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(fail)?
    };
    file.write_all(bytes).map_err(fail)?;
    file.sync_all().map_err(fail)?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(fail)?;
    crate::io::sync_parent_dir(path).map_err(|e| format!("{} : {e}", path.display()))?;
    Ok(())
}

#[cfg(windows)]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    crate::winsec::create_owner_only(path)
}

#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(any(windows, unix)))]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 24 * 3600 * 1000;

    fn names(ips: &[&str]) -> Names {
        Names {
            dns: vec!["localhost".to_owned(), "nas.maison".to_owned()],
            ips: ips.iter().map(|ip| ip.parse().unwrap()).collect(),
        }
    }

    /// Fabriqué une fois, puis repris tel quel (même empreinte) ; refait quand un nom
    /// manque, quand l'expiration approche, ou quand la clé ne va plus avec lui.
    #[test]
    fn the_generated_certificate_is_kept_until_it_must_change() {
        let dir = tempfile::tempdir().unwrap();
        let now = crate::store::now_ms();
        let wanted = names(&["127.0.0.1", "192.168.1.20"]);
        let first = generated(dir.path(), &wanted, now).unwrap();
        assert!(first.renewed && first.generated);
        assert_eq!(first.fingerprint.len(), 32 * 3 - 1);
        for name in ["localhost", "nas.maison", "127.0.0.1", "192.168.1.20"] {
            assert!(
                first.names.contains(&name.to_owned()),
                "{name} : {:?}",
                first.names
            );
        }
        // 824 jours, au plus 825 (iPhone).
        let days = (first.not_after_ms - now) / DAY_MS;
        assert!((823..=824).contains(&days), "{days}");
        assert_eq!(
            first.renew_at_ms(),
            Some(first.not_after_ms - RENEW_BEFORE_MS)
        );

        let again = generated(dir.path(), &wanted, now + DAY_MS).unwrap();
        assert!(!again.renewed);
        assert_eq!(again.fingerprint, first.fingerprint);
        // Moins de noms : rien à refaire.
        let fewer = generated(dir.path(), &names(&["127.0.0.1"]), now).unwrap();
        assert_eq!(fewer.fingerprint, first.fingerprint);

        // Une adresse de plus (écoute déplacée) : refait.
        let moved = generated(dir.path(), &names(&["127.0.0.1", "10.0.0.5"]), now).unwrap();
        assert!(moved.renewed);
        assert_ne!(moved.fingerprint, first.fingerprint);
        assert!(moved.names.contains(&"10.0.0.5".to_owned()));

        // 30 jours avant l'expiration : refait.
        let late = moved.not_after_ms - RENEW_BEFORE_MS + 1;
        let renewed = generated(dir.path(), &names(&["10.0.0.5"]), late).unwrap();
        assert!(renewed.renewed);
        assert_ne!(renewed.fingerprint, moved.fingerprint);

        // Clé d'une autre paire (arrêt entre les deux écritures) : refait.
        let other = tempfile::tempdir().unwrap();
        generated(other.path(), &wanted, now).unwrap();
        std::fs::copy(other.path().join(KEY_FILE), dir.path().join(KEY_FILE)).unwrap();
        let repaired = generated(dir.path(), &names(&["10.0.0.5"]), late).unwrap();
        assert!(repaired.renewed);
        // Fichier abîmé : refait aussi.
        std::fs::write(dir.path().join(CERT_FILE), "pas un certificat").unwrap();
        assert!(generated(dir.path(), &wanted, now).unwrap().renewed);
        assert!(!dir.path().join("remote-key.pem.tmp").exists());
    }

    /// La clé fabriquée n'est lisible que par l'utilisateur.
    #[cfg(unix)]
    #[test]
    fn the_generated_key_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        generated(dir.path(), &names(&[]), crate::store::now_ms()).unwrap();
        let mode = std::fs::metadata(dir.path().join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }

    /// Sous Windows, le descripteur de la clé n'a qu'une entrée, pour l'utilisateur, sans
    /// droit hérité du dossier.
    #[cfg(windows)]
    #[test]
    fn the_generated_key_is_readable_by_its_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        generated(dir.path(), &names(&[]), crate::store::now_ms()).unwrap();
        let key = dir.path().join(KEY_FILE);
        let out = std::process::Command::new("icacls")
            .arg(&key)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        // Une entrée par ligne : `DOMAINE\nom:(droits)`.
        let entries: Vec<&str> = text.lines().filter(|l| l.contains(":(")).collect();
        assert_eq!(entries.len(), 1, "{text}");
        assert!(!text.contains("(I)"), "aucun droit hérité : {text}");
        let user = std::env::var("USERNAME").unwrap().to_ascii_lowercase();
        assert!(entries[0].to_ascii_lowercase().contains(&user), "{text}");
    }

    /// Un certificat fourni : repris avec son empreinte ; refusé s'il a expiré, si sa clé
    /// ne va pas avec lui, ou s'il manque.
    #[test]
    fn a_provided_certificate_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let mine = tempfile::tempdir().unwrap();
        let now = crate::store::now_ms();
        let made = generated(mine.path(), &names(&[]), now).unwrap();
        let (cert, key) = (mine.path().join(CERT_FILE), mine.path().join(KEY_FILE));
        let tls = provided(&cert, &key, now).unwrap();
        assert!(!tls.generated && tls.renew_at_ms().is_none());
        assert_eq!(tls.fingerprint, made.fingerprint);
        let der = CertificateDer::pem_file_iter(&cert)
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(tls.fingerprint, fingerprint(&der));

        let expired = provided(&cert, &key, made.not_after_ms + 1).unwrap_err();
        assert!(expired.contains("expiré"), "{expired}");
        generated(dir.path(), &names(&[]), now).unwrap();
        let mismatch = provided(&cert, &dir.path().join(KEY_FILE), now).unwrap_err();
        assert!(mismatch.contains("ne va pas"), "{mismatch}");
        let missing = provided(&dir.path().join("absent.pem"), &key, now).unwrap_err();
        assert!(missing.contains("absent.pem"), "{missing}");
        // Le message d'une clé illisible ne cite jamais son contenu.
        std::fs::write(dir.path().join("k.pem"), "SECRET-QUI-NE-DOIT-PAS-SORTIR").unwrap();
        let unreadable = provided(&cert, &dir.path().join("k.pem"), now).unwrap_err();
        assert!(!unreadable.contains("SECRET"), "{unreadable}");
    }

    #[test]
    fn names_cover_the_machine_the_listen_address_and_the_added_hosts() {
        let hosts = vec!["NAS.maison".to_owned(), "192.168.1.9".to_owned()];
        let specific = wanted_names("10.1.2.3:7380".parse().unwrap(), &hosts);
        assert_eq!(specific.dns[0], "localhost");
        assert!(specific.dns.contains(&"nas.maison".to_owned()));
        assert!(!specific.dns.iter().any(|d| d.parse::<IpAddr>().is_ok()));
        for ip in ["127.0.0.1", "::1", "192.168.1.9", "10.1.2.3"] {
            assert!(specific.ips.contains(&ip.parse().unwrap()), "{ip}");
        }
        let all = wanted_names("0.0.0.0:7380".parse().unwrap(), &[]);
        assert!(!all.ips.iter().any(IpAddr::is_unspecified));
        let mut sorted = all.ips.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.ips.len(), "sans doublon");
        for name in known_names(&[]) {
            assert!(valid_host_name(&name), "{name}");
        }
    }
}
