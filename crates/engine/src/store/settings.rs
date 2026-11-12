//! Réglages du moteur (table `settings`, SPEC §5.2) : une valeur JSON par clé.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::{StoreError, now_ms};

/// Limite de débit descendant globale, en octets par seconde ; `null` : aucune (F-15).
pub const DOWNLOAD_LIMIT: &str = "limits.downloadBps";

/// Dossier des téléchargements ajoutés sans `destDir` ; `null` : le dossier
/// « Téléchargements » du système.
pub const DOWNLOAD_DIR: &str = "downloads.dir";

/// Réserver la place de chaque fichier au départ (F-10, SPEC §7.6 bis) ; vrai par défaut.
pub const PREALLOCATE: &str = "downloads.preallocate";

/// Chercher l'empreinte d'un fichier dans ses fichiers voisins (`.sha256`, `SHA256SUMS`…) quand
/// l'utilisateur n'en a pas donné (F-11, SPEC §7.9) ; vrai par défaut.
pub const NEIGHBOR_CHECKSUM: &str = "downloads.neighborChecksum";

/// Limite de débit montant globale (torrents, SPEC §7.16), en octets par seconde ; `null` :
/// aucune.
pub const UPLOAD_LIMIT: &str = "limits.uploadBps";

/// Ratio de partage d'un torrent terminé (SPEC §7.16) : `0` pour ne pas partager, `null`
/// pour partager sans limite ; 1,0 tant que rien n'est enregistré (décision de
/// l'utilisateur du 2026-11-09).
pub const TORRENT_RATIO: &str = "torrent.ratioLimit";

/// Ratio de partage par défaut.
pub const DEFAULT_RATIO: f64 = 1.0;

/// Demander à la box d'ouvrir le port d'écoute des torrents (UPnP) ; faux par défaut
/// (décision de l'utilisateur du 2026-11-09).
pub const TORRENT_UPNP: &str = "torrent.upnp";

/// Interface réseau des torrents (VPN, SPEC §7.16 et 8.1) : son nom (« wg0 », nom affiché
/// sous Windows) ; `null` : aucune, les routes du système. Sans elle, les torrents attendent
/// (décision de l'utilisateur du 2026-11-09).
pub const TORRENT_INTERFACE: &str = "torrent.interface";

/// Mettre yt-dlp à jour de lui-même, au plus une fois par jour (F-25, SPEC §7.17) ; vrai
/// par défaut.
pub const MEDIA_AUTO_UPDATE: &str = "media.autoUpdate";

/// Interface distante (SPEC §7.19) : écoute HTTP et WebSocket active ; faux par défaut.
pub const REMOTE_ENABLED: &str = "remote.enabled";

/// Interface distante (SPEC §7.19) : les connexions à jeton peuvent joindre le réseau interne
/// (adresses privées, boucle locale) ; faux par défaut (décision de l'utilisateur du
/// 2026-11-11).
pub const REMOTE_ALLOW_PRIVATE: &str = "remote.allowPrivate";

/// Interface distante (SPEC §7.19) : adresse et port d'écoute ; [`REMOTE_DEFAULT_LISTEN`]
/// par défaut.
pub const REMOTE_LISTEN: &str = "remote.listen";

/// Écoute par défaut de l'interface distante : la boucle locale seulement.
pub const REMOTE_DEFAULT_LISTEN: &str = "127.0.0.1:7380";

/// Adresse et port d'écoute de l'interface distante.
pub fn remote_listen(conn: &Connection) -> Result<std::net::SocketAddr, StoreError> {
    let text = match get(conn, REMOTE_LISTEN)? {
        None | Some(Value::Null) => REMOTE_DEFAULT_LISTEN.to_owned(),
        Some(Value::String(text)) => text,
        Some(v) => {
            return Err(StoreError::BadValue(format!(
                "réglage {REMOTE_LISTEN} : {v} n'est pas une adresse"
            )));
        }
    };
    text.parse().map_err(|_| {
        StoreError::BadValue(format!(
            "réglage {REMOTE_LISTEN} : {text} n'est pas une adresse"
        ))
    })
}

/// Change l'écoute de l'interface distante ; `None` revient à celle par défaut.
pub fn set_remote_listen(
    conn: &Connection,
    addr: Option<std::net::SocketAddr>,
) -> Result<(), StoreError> {
    set(
        conn,
        REMOTE_LISTEN,
        &addr.map_or(Value::Null, |a| Value::from(a.to_string())),
    )
}

/// Interface distante (SPEC §7.19) : noms ajoutés par l'utilisateur, acceptés dans `Host`
/// et portés par le certificat fabriqué (nom DNS ou adresse IP) ; aucun par défaut.
pub const REMOTE_HOSTS: &str = "remote.hosts";

/// Noms ajoutés à `remote.hosts`, au plus.
pub const REMOTE_HOSTS_MAX: usize = 32;

/// Interface distante (SPEC §7.19, étape 3b) : certificat fourni par l'utilisateur (fichier
/// PEM, chemin absolu) ; `null` : celui fabriqué par le moteur.
pub const REMOTE_TLS_CERT: &str = "remote.tlsCert";

/// Clé privée du certificat fourni (fichier PEM, chemin absolu), avec
/// [`REMOTE_TLS_CERT`] ; jamais lue ailleurs que dans ce fichier, jamais copiée en base.
pub const REMOTE_TLS_KEY: &str = "remote.tlsKey";

/// Réglages de l'écoute de l'interface distante, hors `remote.enabled`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConfig {
    pub listen: std::net::SocketAddr,
    /// `remote.hosts`, en minuscules.
    pub hosts: Vec<String>,
    /// Certificat et clé fournis (`remote.tlsCert`, `remote.tlsKey`), ou aucun.
    pub tls_files: Option<(String, String)>,
}

/// Un nom de `remote.hosts` : une adresse IP, ou un nom DNS (lettres, chiffres, tirets,
/// points ; 253 caractères au plus, 63 par partie).
pub fn valid_host_name(name: &str) -> bool {
    if name.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let name = name.strip_suffix('.').unwrap_or(name);
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Réglages de l'écoute de l'interface distante.
pub fn remote_config(conn: &Connection) -> Result<RemoteConfig, StoreError> {
    let hosts = match get(conn, REMOTE_HOSTS)? {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v.as_str() {
                Some(name) if valid_host_name(name) => Ok(name.to_ascii_lowercase()),
                _ => Err(StoreError::BadValue(format!(
                    "réglage {REMOTE_HOSTS} : {v} n'est pas un nom"
                ))),
            })
            .collect::<Result<_, _>>()?,
        Some(v) => {
            return Err(StoreError::BadValue(format!(
                "réglage {REMOTE_HOSTS} : {v} n'est pas une liste"
            )));
        }
    };
    let path = |key: &str| match get(conn, key)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(path)) => Ok(Some(path)),
        Some(v) => Err(StoreError::BadValue(format!(
            "réglage {key} : {v} n'est pas un chemin"
        ))),
    };
    let tls_files = match (path(REMOTE_TLS_CERT)?, path(REMOTE_TLS_KEY)?) {
        (Some(cert), Some(key)) => Some((cert, key)),
        _ => None,
    };
    Ok(RemoteConfig {
        listen: remote_listen(conn)?,
        hosts,
        tls_files,
    })
}

/// Remplace les noms ajoutés de l'interface distante (vérifiés par l'appelant).
pub fn set_remote_hosts(conn: &Connection, hosts: &[String]) -> Result<(), StoreError> {
    let value = if hosts.is_empty() {
        Value::Null
    } else {
        Value::from(
            hosts
                .iter()
                .map(|h| h.to_ascii_lowercase())
                .collect::<Vec<_>>(),
        )
    };
    set(conn, REMOTE_HOSTS, &value)
}

/// Certificat et clé fournis, ou `None` pour celui fabriqué par le moteur ; les deux
/// ensemble, dans une transaction.
pub fn set_remote_tls(
    conn: &mut Connection,
    files: Option<(&str, &str)>,
) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    let (cert, key) = files.map_or((Value::Null, Value::Null), |(c, k)| {
        (Value::from(c), Value::from(k))
    });
    set(&tx, REMOTE_TLS_CERT, &cert)?;
    set(&tx, REMOTE_TLS_KEY, &key)?;
    tx.commit()?;
    Ok(())
}

/// Longueur maximale du nom d'une interface (caractères).
pub const INTERFACE_NAME_MAX: usize = 256;

/// Espace de noms des secrets de cette base dans le coffre du système (`secrets`), tiré
/// au hasard au premier démarrage. Interne : `settings.get` ne le montre pas.
pub const VAULT_NAMESPACE: &str = "vault.namespace";

/// Espace de noms des secrets de cette base, créé s'il n'existe pas encore.
pub fn vault_namespace(conn: &Connection) -> Result<String, StoreError> {
    match get(conn, VAULT_NAMESPACE)? {
        Some(Value::String(ns)) if !ns.is_empty() => Ok(ns),
        None => {
            let ns = uuid::Uuid::now_v7().simple().to_string();
            set(conn, VAULT_NAMESPACE, &Value::from(ns.as_str()))?;
            Ok(ns)
        }
        Some(v) => Err(StoreError::BadValue(format!(
            "réglage {VAULT_NAMESPACE} : {v} n'est pas un espace de noms"
        ))),
    }
}

/// Dossier des téléchargements enregistré ; `None` : aucun (celui du système).
pub fn download_dir(conn: &Connection) -> Result<Option<String>, StoreError> {
    match get(conn, DOWNLOAD_DIR)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(dir)) => Ok(Some(dir)),
        Some(v) => Err(StoreError::BadValue(format!(
            "réglage {DOWNLOAD_DIR} : {v} n'est pas un dossier"
        ))),
    }
}

/// Enregistre le dossier des téléchargements ; `None` : celui du système.
pub fn set_download_dir(conn: &Connection, dir: Option<&str>) -> Result<(), StoreError> {
    set(conn, DOWNLOAD_DIR, &dir.map_or(Value::Null, Value::from))
}

/// Réglages oui ou non du moteur ; leur valeur par défaut : [`Flag::default_on`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flag {
    /// Réserver la place de chaque fichier au départ (F-10, SPEC §7.6 bis).
    Preallocate,
    /// Chercher l'empreinte d'un fichier dans ses fichiers voisins (F-11, SPEC §7.9).
    NeighborChecksum,
    /// UPnP pour le port d'écoute des torrents (SPEC §7.16) ; faux par défaut.
    TorrentUpnp,
    /// Mise à jour automatique de yt-dlp (F-25, SPEC §7.17) ; vrai par défaut, désactivable
    /// comme toute vérification de mise à jour (SPEC §8.2).
    MediaAutoUpdate,
    /// Interface distante (SPEC §7.19) : les connexions à jeton peuvent joindre le réseau
    /// interne ; faux par défaut.
    RemoteAllowPrivate,
    /// Interface distante (SPEC §7.19) : écoute active ; faux par défaut.
    RemoteEnabled,
}

impl Flag {
    pub const ALL: [Flag; 6] = [
        Flag::Preallocate,
        Flag::NeighborChecksum,
        Flag::TorrentUpnp,
        Flag::MediaAutoUpdate,
        Flag::RemoteAllowPrivate,
        Flag::RemoteEnabled,
    ];

    /// Clé du réglage, dans la base et dans le protocole.
    pub fn key(self) -> &'static str {
        match self {
            Flag::Preallocate => PREALLOCATE,
            Flag::NeighborChecksum => NEIGHBOR_CHECKSUM,
            Flag::TorrentUpnp => TORRENT_UPNP,
            Flag::MediaAutoUpdate => MEDIA_AUTO_UPDATE,
            Flag::RemoteAllowPrivate => REMOTE_ALLOW_PRIVATE,
            Flag::RemoteEnabled => REMOTE_ENABLED,
        }
    }

    /// Valeur tant que rien n'est enregistré.
    pub fn default_on(self) -> bool {
        !matches!(
            self,
            Flag::TorrentUpnp | Flag::RemoteAllowPrivate | Flag::RemoteEnabled
        )
    }

    pub fn from_key(key: &str) -> Option<Flag> {
        Flag::ALL.into_iter().find(|f| f.key() == key)
    }
}

/// Valeur d'un réglage oui ou non ; celle par défaut tant que rien n'est enregistré.
pub fn flag(conn: &Connection, flag: Flag) -> Result<bool, StoreError> {
    let key = flag.key();
    match get(conn, key)? {
        None | Some(Value::Null) => Ok(flag.default_on()),
        Some(Value::Bool(on)) => Ok(on),
        Some(v) => Err(StoreError::BadValue(format!(
            "réglage {key} : {v} n'est pas un booléen"
        ))),
    }
}

pub fn set_flag(conn: &Connection, flag: Flag, on: bool) -> Result<(), StoreError> {
    set(conn, flag.key(), &Value::Bool(on))
}
/// Valeur d'un réglage ; `None` s'il n'a jamais été enregistré.
pub fn get(conn: &Connection, key: &str) -> Result<Option<Value>, StoreError> {
    let text: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    text.map(|t| {
        serde_json::from_str(&t)
            .map_err(|e| StoreError::BadValue(format!("réglage {key} illisible : {e}")))
    })
    .transpose()
}

/// Enregistre un réglage.
pub fn set(conn: &Connection, key: &str, value: &Value) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO settings (key, value_json, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (key) DO UPDATE SET value_json = ?2, updated_at = ?3",
        params![key, value.to_string(), now_ms()],
    )?;
    Ok(())
}

/// Limite de débit globale ; `None` : aucune.
pub fn download_limit(conn: &Connection) -> Result<Option<u64>, StoreError> {
    match get(conn, DOWNLOAD_LIMIT)? {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n > 0 => Ok(Some(n)),
            _ => Err(StoreError::BadValue(format!(
                "réglage {DOWNLOAD_LIMIT} : {v} n'est pas un débit"
            ))),
        },
    }
}

/// Enregistre la limite de débit globale ; `None` : aucune.
pub fn set_download_limit(conn: &Connection, limit: Option<u64>) -> Result<(), StoreError> {
    set(
        conn,
        DOWNLOAD_LIMIT,
        &limit.map_or(Value::Null, Value::from),
    )
}

/// Limite de débit montant globale ; `None` : aucune.
pub fn upload_limit(conn: &Connection) -> Result<Option<u64>, StoreError> {
    match get(conn, UPLOAD_LIMIT)? {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n > 0 => Ok(Some(n)),
            _ => Err(StoreError::BadValue(format!(
                "réglage {UPLOAD_LIMIT} : {v} n'est pas un débit"
            ))),
        },
    }
}

pub fn set_upload_limit(conn: &Connection, limit: Option<u64>) -> Result<(), StoreError> {
    set(conn, UPLOAD_LIMIT, &limit.map_or(Value::Null, Value::from))
}

/// Ratio de partage des torrents ; `None` : sans limite. [`DEFAULT_RATIO`] tant que rien
/// n'est enregistré.
pub fn torrent_ratio(conn: &Connection) -> Result<Option<f64>, StoreError> {
    match get(conn, TORRENT_RATIO)? {
        None => Ok(Some(DEFAULT_RATIO)),
        Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64() {
            Some(r) if r.is_finite() && r >= 0.0 => Ok(Some(r)),
            _ => Err(StoreError::BadValue(format!(
                "réglage {TORRENT_RATIO} : {v} n'est pas un ratio"
            ))),
        },
    }
}

pub fn set_torrent_ratio(conn: &Connection, ratio: Option<f64>) -> Result<(), StoreError> {
    set(conn, TORRENT_RATIO, &ratio.map_or(Value::Null, Value::from))
}

/// Proxy (F-07, phase 7) : mode, adresse, exceptions, identifiant ; le mot de passe est
/// au coffre du système, la base n'en garde que la référence (`proxy.secret`).
pub const PROXY_MODE: &str = "proxy.mode";
pub const PROXY_URL: &str = "proxy.url";
pub const PROXY_EXCEPTIONS: &str = "proxy.exceptions";
pub const PROXY_USERNAME: &str = "proxy.username";
pub const PROXY_SECRET: &str = "proxy.secret";

/// Réglages du proxy, et la référence de son mot de passe au coffre.
pub fn proxy(
    conn: &Connection,
) -> Result<(crate::net::proxy::Settings, Option<String>), StoreError> {
    use crate::net::proxy::{Mode, Settings};
    let text = |key: &str| -> Result<Option<String>, StoreError> {
        Ok(match get(conn, key)? {
            Some(Value::String(s)) => Some(s),
            _ => None,
        })
    };
    let mode = match text(PROXY_MODE)? {
        None => Mode::System,
        Some(m) => Mode::parse(&m)
            .ok_or_else(|| StoreError::BadValue(format!("réglage {PROXY_MODE} : {m}")))?,
    };
    let exceptions = match get(conn, PROXY_EXCEPTIONS)? {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    let settings = Settings {
        mode,
        url: text(PROXY_URL)?,
        exceptions,
        username: text(PROXY_USERNAME)?,
    };
    Ok((settings, text(PROXY_SECRET)?))
}

/// Enregistre les réglages du proxy et la référence de son mot de passe (`None` : aucun).
pub fn set_proxy(
    conn: &Connection,
    settings: &crate::net::proxy::Settings,
    secret: Option<&str>,
) -> Result<(), StoreError> {
    let tx = conn.unchecked_transaction()?;
    set(&tx, PROXY_MODE, &Value::from(settings.mode.as_str()))?;
    set(
        &tx,
        PROXY_URL,
        &settings.url.clone().map_or(Value::Null, Value::from),
    )?;
    set(
        &tx,
        PROXY_EXCEPTIONS,
        &Value::from(settings.exceptions.clone()),
    )?;
    set(
        &tx,
        PROXY_USERNAME,
        &settings.username.clone().map_or(Value::Null, Value::from),
    )?;
    set(&tx, PROXY_SECRET, &secret.map_or(Value::Null, Value::from))?;
    tx.commit()?;
    Ok(())
}

/// Niveau des journaux (N-12, phase 7) : `error`, `warn`, `info` (par défaut) ou `debug`.
pub const LOGS_LEVEL: &str = "logs.level";

pub fn logs_level(conn: &Connection) -> Result<crate::logs::Level, StoreError> {
    match get(conn, LOGS_LEVEL)? {
        None | Some(Value::Null) => Ok(crate::logs::Level::Info),
        Some(Value::String(text)) => crate::logs::Level::parse(&text).ok_or_else(|| {
            StoreError::BadValue(format!("réglage {LOGS_LEVEL} : {text} n'est pas un niveau"))
        }),
        Some(v) => Err(StoreError::BadValue(format!(
            "réglage {LOGS_LEVEL} : {v} n'est pas un niveau"
        ))),
    }
}

pub fn set_logs_level(conn: &Connection, level: crate::logs::Level) -> Result<(), StoreError> {
    set(conn, LOGS_LEVEL, &Value::from(level.as_str()))
}

/// Interface réseau des torrents ; `None` : aucune.
pub fn torrent_interface(conn: &Connection) -> Result<Option<String>, StoreError> {
    match get(conn, TORRENT_INTERFACE)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) if valid_interface_name(&name) => Ok(Some(name)),
        Some(v) => Err(StoreError::BadValue(format!(
            "réglage {TORRENT_INTERFACE} : {v} n'est pas un nom d'interface"
        ))),
    }
}

pub fn set_torrent_interface(conn: &Connection, name: Option<&str>) -> Result<(), StoreError> {
    if let Some(name) = name.filter(|n| !valid_interface_name(n)) {
        return Err(StoreError::BadValue(format!(
            "réglage {TORRENT_INTERFACE} : {name:?} n'est pas un nom d'interface"
        )));
    }
    set(
        conn,
        TORRENT_INTERFACE,
        &name.map_or(Value::Null, Value::from),
    )
}

/// Un nom d'interface : non vide, sans caractère de contrôle, [`INTERFACE_NAME_MAX`]
/// caractères au plus.
pub fn valid_interface_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= INTERFACE_NAME_MAX
        && !name.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_in_memory;

    #[test]
    fn the_download_limit_is_kept_and_removed() {
        let conn = open_in_memory().unwrap();
        assert_eq!(download_limit(&conn).unwrap(), None);
        set_download_limit(&conn, Some(2_000_000)).unwrap();
        assert_eq!(download_limit(&conn).unwrap(), Some(2_000_000));
        set_download_limit(&conn, None).unwrap();
        assert_eq!(download_limit(&conn).unwrap(), None);
        assert_eq!(get(&conn, DOWNLOAD_LIMIT).unwrap(), Some(Value::Null));
    }

    #[test]
    fn the_download_dir_is_kept_and_reset() {
        let conn = open_in_memory().unwrap();
        assert_eq!(download_dir(&conn).unwrap(), None);
        set_download_dir(&conn, Some("/srv/dl")).unwrap();
        assert_eq!(download_dir(&conn).unwrap().as_deref(), Some("/srv/dl"));
        set_download_dir(&conn, None).unwrap();
        assert_eq!(download_dir(&conn).unwrap(), None);
        set(&conn, DOWNLOAD_DIR, &Value::from(3)).unwrap();
        assert!(download_dir(&conn).is_err());
    }

    /// Noms ajoutés et certificat fourni de l'interface distante (SPEC §7.19, étape 3b).
    #[test]
    fn remote_hosts_and_certificate_files_are_kept_together() {
        let mut conn = open_in_memory().unwrap();
        let config = remote_config(&conn).unwrap();
        assert!(config.hosts.is_empty() && config.tls_files.is_none());
        assert_eq!(config.listen.to_string(), REMOTE_DEFAULT_LISTEN);
        set_remote_hosts(&conn, &["NAS.maison".to_owned(), "192.168.1.20".to_owned()]).unwrap();
        set_remote_tls(&mut conn, Some(("/etc/c.pem", "/etc/k.pem"))).unwrap();
        let config = remote_config(&conn).unwrap();
        assert_eq!(config.hosts, ["nas.maison", "192.168.1.20"]);
        assert_eq!(
            config.tls_files,
            Some(("/etc/c.pem".to_owned(), "/etc/k.pem".to_owned()))
        );
        set_remote_hosts(&conn, &[]).unwrap();
        set_remote_tls(&mut conn, None).unwrap();
        let config = remote_config(&conn).unwrap();
        assert!(config.hosts.is_empty() && config.tls_files.is_none());

        for good in [
            "nas",
            "nas.local",
            "a-b.example.org.",
            "10.0.0.1",
            "fe80::1",
        ] {
            assert!(valid_host_name(good), "{good}");
        }
        let long_label = "a".repeat(64);
        for bad in [
            "",
            ".",
            "-nas",
            "nas-",
            "a..b",
            "nas:443",
            "n as",
            "*.example",
            &long_label,
        ] {
            assert!(!valid_host_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_damaged_setting_is_reported() {
        let conn = open_in_memory().unwrap();
        set(&conn, DOWNLOAD_LIMIT, &Value::from("vite")).unwrap();
        assert!(matches!(
            download_limit(&conn),
            Err(StoreError::BadValue(_))
        ));
    }
}
