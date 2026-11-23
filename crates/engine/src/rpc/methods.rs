//! Méthodes du protocole (SPEC §6.4) : lecture des paramètres nommés, appel du moteur,
//! résultat en JSON.

use std::time::Duration;

use base64::Engine as _;
use serde_json::{Map, Value, json};

use super::scope::{self, Scope};
use super::session::{Ctx, Topics};
use super::{
    PROTOCOL_VERSIONS, RpcError, end_action_json, negotiate, queue_json, schedule_json, task_json,
};
use crate::io::checksum::Checksum;
use crate::metalink::Metalink;
use crate::schedule::Rules;
use crate::scheduler::{
    AddTask, EngineError, Include, LoginChange, LoginView, MediaRequest, MetalinkSource,
    QueueChange, TaskDetails, TaskUpdate,
};
use crate::store::history::{Cursor, HistoryFilter, HistoryRow};
use crate::store::settings::{
    self, DOWNLOAD_DIR, Flag, REMOTE_HOSTS, REMOTE_LISTEN, REMOTE_TLS_CERT, REMOTE_TLS_KEY,
    TORRENT_INTERFACE, TORRENT_RATIO,
};
use crate::store::tasks::{TaskFilter, TaskState};
use crate::transfer::MAX_CONNECTIONS;

/// Intervalle par défaut des notifications `task.progress` (SPEC §6.5).
const DEFAULT_PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// Longueur maximale d'une clé d'idempotence, en octets.
const MAX_IDEMPOTENCY_KEY: usize = 256;

/// Paramètres nommés d'un appel : toujours un objet (SPEC §6.1).
struct Params(Map<String, Value>);

impl Params {
    fn new(params: Value) -> Result<Self, RpcError> {
        match params {
            Value::Null => Ok(Params(Map::new())),
            Value::Object(map) => Ok(Params(map)),
            _ => Err(RpcError::invalid_params("params doit être un objet")),
        }
    }

    /// Valeur présente et non nulle.
    fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name).filter(|v| !v.is_null())
    }

    fn wrong(name: &str, what: &str) -> RpcError {
        RpcError::invalid_params(format!("{name} : {what} attendu"))
    }

    fn str(&self, name: &str) -> Result<Option<String>, RpcError> {
        self.get(name)
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| Self::wrong(name, "texte"))
            })
            .transpose()
    }

    fn required_str(&self, name: &str) -> Result<String, RpcError> {
        self.str(name)?
            .ok_or_else(|| RpcError::invalid_params(format!("{name} manquant")))
    }

    /// Entier signé (date en millisecondes Unix).
    fn i64(&self, name: &str) -> Result<Option<i64>, RpcError> {
        self.get(name)
            .map(|v| v.as_i64().ok_or_else(|| Self::wrong(name, "entier")))
            .transpose()
    }

    fn u32(&self, name: &str) -> Result<Option<u32>, RpcError> {
        self.get(name)
            .map(|v| {
                v.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| Self::wrong(name, "entier positif"))
            })
            .transpose()
    }

    /// Limite de débit : absente, inchangée (`None`) ; `null`, retirée (`Some(None)`) ;
    /// sinon un entier strictement positif, en octets par seconde.
    fn limit(&self, name: &str) -> Result<Option<Option<u64>>, RpcError> {
        match self.0.get(name) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(v) => match v.as_u64() {
                Some(n) if n > 0 => Ok(Some(Some(n))),
                _ => Err(Self::wrong(name, "entier strictement positif ou null")),
            },
        }
    }

    /// Empreinte attendue (F-11) : `{"algo": "sha256" | "sha1" | "md5", "value": hex}` ;
    /// absente `None`, `null` `Some(None)` (retirée).
    fn checksum(&self, name: &str) -> Result<Option<Option<Checksum>>, RpcError> {
        let object = match self.0.get(name) {
            None => return Ok(None),
            Some(Value::Null) => return Ok(Some(None)),
            Some(Value::Object(object)) => object,
            Some(_) => return Err(Self::wrong(name, "objet {algo, value} ou null")),
        };
        let field = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params(format!("{name}.{key} manquant")))
        };
        Checksum::parse(field("algo")?, field("value")?)
            .map(|c| Some(Some(c)))
            .map_err(|e| RpcError::invalid_params(format!("{name} : {e}")))
    }

    /// Texte facultatif qu'on peut effacer : absent `None`, `null` `Some(None)`.
    fn nullable_str(&self, name: &str) -> Result<Option<Option<String>>, RpcError> {
        match self.0.get(name) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(Value::String(s)) => Ok(Some(Some(s.clone()))),
            Some(_) => Err(Self::wrong(name, "texte ou null")),
        }
    }

    /// Booléen ; `default` s'il est absent.
    fn bool_or(&self, name: &str, default: bool) -> Result<bool, RpcError> {
        self.get(name)
            .map(|v| v.as_bool().ok_or_else(|| Self::wrong(name, "booléen")))
            .transpose()
            .map(|b| b.unwrap_or(default))
    }

    fn bool(&self, name: &str) -> Result<bool, RpcError> {
        self.bool_or(name, false)
    }

    fn strings(&self, name: &str) -> Result<Option<Vec<String>>, RpcError> {
        self.get(name)
            .map(|v| {
                v.as_array()
                    .and_then(|items| {
                        items
                            .iter()
                            .map(|i| i.as_str().map(str::to_owned))
                            .collect::<Option<Vec<_>>>()
                    })
                    .ok_or_else(|| Self::wrong(name, "tableau de textes"))
            })
            .transpose()
    }

    /// Objet dont chaque valeur est un texte (`headers`), dans l'ordre reçu.
    fn string_map(&self, name: &str) -> Result<Vec<(String, String)>, RpcError> {
        let Some(value) = self.get(name) else {
            return Ok(Vec::new());
        };
        value
            .as_object()
            .and_then(|map| {
                map.iter()
                    .map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or_else(|| Self::wrong(name, "objet de textes"))
    }

    fn task_ids(&self) -> Result<Vec<String>, RpcError> {
        let ids = self
            .strings("taskIds")?
            .ok_or_else(|| RpcError::invalid_params("taskIds manquant"))?;
        if ids.is_empty() {
            return Err(RpcError::invalid_params("taskIds vide"));
        }
        Ok(ids)
    }

    /// Paramètres prévus par la SPEC mais pas encore pris en charge : refusés plutôt
    /// qu'ignorés en silence.
    fn refuse(&self, names: &[&str]) -> Result<(), RpcError> {
        match names.iter().find(|n| self.get(n).is_some()) {
            Some(name) => Err(RpcError::capability_missing(format!(
                "{name} n'est pas encore pris en charge par ce moteur"
            ))),
            None => Ok(()),
        }
    }
}

/// Appelle `method` ; les méthodes inconnues répondent « méthode inconnue ».
pub(super) async fn call(ctx: &mut Ctx, method: &str, params: Value) -> Result<Value, RpcError> {
    match scope::required(method, &params) {
        Some(need) if !ctx.scope.allows(need) => {
            return Err(RpcError::unauthorized(format!(
                "{method} demande la portée {}, cette connexion a la portée {}",
                need.as_str(),
                ctx.scope.as_str()
            )));
        }
        Some(_) => {}
        // Seul `engine.hello` est permis à tous ; une méthode sans portée, même servie par
        // oubli, n'est jamais exécutée (audit R5).
        None if method == "engine.hello" => {}
        None => return Err(RpcError::method_not_found(method)),
    }
    let p = Params::new(params)?;
    match method {
        "engine.hello" => hello(ctx, &p),
        "engine.shutdown" => {
            ctx.shutdown_after = true;
            Ok(json!({}))
        }
        "task.add" => task_add(ctx, &p).await,
        // Fichiers Metalink (F-27, SPEC §7.7, depuis le 2026-11-11).
        "metalink.read" => metalink_read(ctx, &p).await,
        "metalink.add" => metalink_add(ctx, &p).await,
        // Plugins d'hébergeurs (F-41, SPEC §7.20, depuis le 2026-11-12).
        "plugin.inspect" => {
            let inspected = ctx.engine.inspect_plugin(plugin_data(&p)?).await?;
            Ok(inspected_json(&inspected))
        }
        "plugin.install" => {
            let digest = p.required_str("digest")?;
            let row = ctx.engine.install_plugin(plugin_data(&p)?, &digest).await?;
            Ok(plugin_json(&row))
        }
        "plugin.list" => {
            let rows = ctx.engine.plugins().await?;
            Ok(json!({"plugins": rows.iter().map(plugin_json).collect::<Vec<_>>()}))
        }
        "plugin.remove" => {
            ctx.engine
                .remove_plugin(&p.required_str("pluginId")?)
                .await?;
            Ok(json!({}))
        }
        "plugin.enable" => {
            let Some(Value::Bool(enabled)) = p.get("enabled") else {
                return Err(Params::wrong("enabled", "booléen"));
            };
            let row = ctx
                .engine
                .enable_plugin(&p.required_str("pluginId")?, *enabled)
                .await?;
            Ok(plugin_json(&row))
        }
        "plugin.match" => {
            let found = ctx.engine.plugin_for(&p.required_str("url")?).await?;
            let plugin = found.map(|row| json!({"pluginId": row.id, "name": row.name}));
            Ok(json!({ "plugin": plugin }))
        }
        "plugin.resolve" => {
            let resolution = ctx
                .engine
                .resolve_with_plugin(&p.required_str("url")?)
                .await?;
            Ok(resolution_json(&resolution))
        }
        "plugin.add" => plugin_add(ctx, &p).await,
        // Analyse de liens (F-29, F-31, SPEC §7.18, depuis le 2026-11-11).
        "task.addBatch" => task_add_batch(ctx, &p).await,
        "linkscan.run" => linkscan_run(ctx, &p).await,
        "linkscan.probe" => linkscan_probe(ctx, &p).await,
        "task.get" => task_get(ctx, &p).await,
        "task.list" => task_list(ctx, &p).await,
        "task.update" => task_update(ctx, &p).await,
        "task.pause" => for_each(ctx, &p, Action::Pause).await,
        "task.resume" => for_each(ctx, &p, Action::Resume).await,
        "task.cancel" => {
            let delete_files = p.bool("deleteFiles")?;
            for_each(ctx, &p, Action::Cancel { delete_files }).await
        }
        "task.remove" => {
            let delete_files = p.bool("deleteFiles")?;
            let mut result = for_each(ctx, &p, Action::Remove { delete_files }).await?;
            // SPEC §6.4 : le nombre de tâches retirées.
            result["removed"] = json!(result["taskIds"].as_array().map_or(0, Vec::len));
            Ok(result)
        }
        // Retéléchargement (depuis le 2026-11-07) ; `deleteFiles` : le fichier final d'une
        // tâche terminée part aussi.
        "task.restart" => {
            let delete_files = p.bool("deleteFiles")?;
            for_each(ctx, &p, Action::Restart { delete_files }).await
        }
        // Fichiers d'un torrent à télécharger (SPEC §7.16, depuis le 2026-11-09) : `selected`,
        // les rangs choisis ; les autres ne le sont plus.
        // Miroir ajouté à une tâche HTTP (SPEC §7.7, phase 6) ; `rank` n'est pas pris en
        // charge : le miroir se range après les autres.
        "task.addSource" => {
            if p.get("rank").is_some() {
                return Err(RpcError::capability_missing(
                    "task.addSource : rank pas encore pris en charge",
                ));
            }
            let task_id = p.required_str("taskId")?;
            let url = p.required_str("url")?;
            let source_id = ctx.engine.add_source(&task_id, &url).await?;
            Ok(json!({"sourceId": source_id}))
        }
        "task.setFiles" => {
            let task_id = p.required_str("taskId")?;
            let selected = p
                .get("selected")
                .and_then(Value::as_array)
                .ok_or_else(|| Params::wrong("selected", "liste de rangs"))?
                .iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|i| u32::try_from(i).ok())
                        .ok_or_else(|| Params::wrong("selected", "liste de rangs"))
                })
                .collect::<Result<Vec<u32>, _>>()?;
            let task = ctx.engine.set_files(&task_id, selected).await?;
            Ok(task_json(&task))
        }
        "history.list" => history_list(ctx, &p).await,
        "history.clear" => {
            let before = p.i64("before")?;
            let removed = ctx.engine.clear_history(before).await?;
            Ok(json!({"removed": removed}))
        }
        "queue.list" => {
            let queues = ctx.engine.queues().await?;
            Ok(json!({"items": queues.iter().map(queue_json).collect::<Vec<_>>()}))
        }
        "queue.create" => {
            let name = p.required_str("name")?;
            let max_active = p.u32("maxActive")?.unwrap_or(3);
            let limit = p.limit("speedLimitBps")?.flatten();
            let schedule_id = p.nullable_str("scheduleId")?.flatten();
            // Une seule commande : la planification est vérifiée et attachée avec la création
            // (audit R14 : supprimée entre deux commandes, elle laissait une file sans elle).
            let id = ctx
                .engine
                .create_queue_with(&name, max_active, limit, schedule_id)
                .await?;
            queue_by_id(ctx, &id).await
        }
        "queue.update" => {
            let id = p.required_str("queueId")?;
            let change = QueueChange {
                name: p.str("name")?,
                max_active: p.u32("maxActive")?,
                speed_limit_bps: p.limit("speedLimitBps")?,
                schedule_id: p.nullable_str("scheduleId")?,
            };
            let queue = ctx.engine.update_queue(&id, change).await?;
            Ok(queue_json(&queue))
        }
        // Identifiants de sites (SPEC §7.14) : le mot de passe entre, il ne ressort jamais.
        "login.list" => {
            let items = ctx.engine.logins().await?;
            Ok(json!({"items": items.iter().map(login_json).collect::<Vec<_>>()}))
        }
        "login.set" => {
            let change = LoginChange {
                id: p.str("loginId")?,
                site: p.required_str("site")?,
                username: p.required_str("username")?,
                password: p.str("password")?,
                allow_plain: p
                    .get("allowPlain")
                    .map(|_| p.bool("allowPlain"))
                    .transpose()?,
                key_path: p.str("keyPath")?,
            };
            Ok(login_json(&ctx.engine.set_login(change).await?))
        }
        "login.delete" => {
            ctx.engine.delete_login(&p.required_str("loginId")?).await?;
            Ok(json!({}))
        }
        // Clés des serveurs SSH (SFTP, SPEC §7.22).
        "hostkey.list" => {
            let (accepted, pending) = ctx.engine.host_keys().await?;
            Ok(json!({
                "items": accepted.iter().map(|k| json!({
                    "host": k.host,
                    "port": k.port,
                    "algorithm": k.algorithm,
                    "fingerprint": k.fingerprint,
                    "acceptedAt": k.accepted_at,
                })).collect::<Vec<_>>(),
                "pending": pending.iter().map(|k| json!({
                    "host": k.host,
                    "port": k.port,
                    "algorithm": k.algorithm,
                    "fingerprint": k.fingerprint,
                    "changed": k.changed,
                    "seenAt": k.seen_at,
                })).collect::<Vec<_>>(),
            }))
        }
        "hostkey.accept" => {
            let (host, port) = host_and_port(&p)?;
            let fingerprint = p.required_str("fingerprint")?;
            let resumed = ctx
                .engine
                .accept_host_key(&host, port, &fingerprint)
                .await?;
            Ok(json!({"resumed": resumed}))
        }
        "hostkey.remove" => {
            let (host, port) = host_and_port(&p)?;
            let removed = ctx.engine.remove_host_key(&host, port).await?;
            Ok(json!({"removed": removed}))
        }
        // Jetons de l'interface distante (SPEC §7.19) ; le jeton n'est rendu qu'ici.
        "token.create" => {
            let name = p.required_str("name")?;
            let scope = p.str("scope")?.unwrap_or_else(|| "control".to_owned());
            let (row, token) = ctx.engine.create_token(&name, &scope).await?;
            let mut view = token_json(&row);
            view["token"] = json!(token);
            Ok(view)
        }
        "token.list" => {
            let rows = ctx.engine.tokens().await?;
            Ok(json!({"tokens": rows.iter().map(token_json).collect::<Vec<_>>()}))
        }
        // État de l'écoute de l'interface distante (SPEC §7.19).
        "remote.status" => {
            let state = ctx.engine.remote_state();
            let certificate = state.certificate.as_ref().map(|c| {
                json!({
                    "fingerprint": c.fingerprint,
                    "expiresAt": c.expires_at,
                    "generated": c.generated,
                    "names": c.names,
                })
            });
            Ok(json!({
                "enabled": state.enabled,
                "listen": state.listen,
                "listening": state.listening,
                "error": state.error,
                "tls": state.tls,
                "certificate": certificate,
                "urls": state.urls,
                "hosts": state.hosts,
                "tlsCert": state.tls_cert,
                // Adresses où l'écoute peut s'ouvrir (étape 3d, choix du bureau).
                "addresses": crate::remote::listen_choices().iter().map(|c| json!({
                    "interface": c.interface,
                    "address": c.address.to_string(),
                })).collect::<Vec<_>>(),
            }))
        }
        "token.revoke" => {
            let revoked = ctx.engine.revoke_token(&p.required_str("tokenId")?).await?;
            Ok(json!({"revoked": revoked}))
        }
        // Vidéo d'une page (SPEC §7.17).
        "media.probe" => {
            let page = p.required_str("pageUrl")?;
            Ok(probe_json(&ctx.engine.probe_media(&page).await?))
        }
        // Outils de la vidéo (SPEC §7.17).
        // Interfaces réseau où lier les torrents (SPEC §7.16, étape 5b).
        "torrent.interfaces" => Ok(json!({"items": crate::torrent::system_interfaces()})),
        "media.tools" => Ok(tools_json(&ctx.engine.media_tools()?)),
        "media.installTools" => {
            let force = match p.get("force") {
                None => false,
                Some(Value::Bool(force)) => *force,
                Some(_) => return Err(Params::wrong("force", "booléen")),
            };
            Ok(tools_json(&ctx.engine.install_media_tools(force).await?))
        }
        "settings.get" => settings(ctx).await,
        "settings.set" => {
            let values = p
                .get("values")
                .and_then(Value::as_object)
                .ok_or_else(|| RpcError::invalid_params("values : objet attendu"))?;
            if let Some(key) = values.keys().find(|k| {
                *k != DOWNLOAD_DIR
                    && *k != TORRENT_RATIO
                    && *k != TORRENT_INTERFACE
                    && *k != settings::LOGS_LEVEL
                    && *k != REMOTE_LISTEN
                    && *k != REMOTE_HOSTS
                    && *k != REMOTE_TLS_CERT
                    && *k != REMOTE_TLS_KEY
                    && Flag::from_key(k).is_none()
            }) {
                let known: Vec<_> = Flag::ALL.iter().map(|f| f.key()).collect();
                return Err(RpcError::invalid_params(format!(
                    "réglage inconnu : {key} (pris en charge : {DOWNLOAD_DIR}, \
                     {TORRENT_RATIO}, {TORRENT_INTERFACE}, {}, {REMOTE_LISTEN}, {REMOTE_HOSTS}, \
                     {REMOTE_TLS_CERT}, {REMOTE_TLS_KEY}, {})",
                    settings::LOGS_LEVEL,
                    known.join(", ")
                )));
            }
            // Noms ajoutés de l'interface distante (SPEC §7.19) : liste de noms, ou null.
            let hosts = match values.get(REMOTE_HOSTS) {
                None => None,
                Some(Value::Null) => Some(Vec::new()),
                Some(Value::Array(items)) => Some(
                    items
                        .iter()
                        .map(|v| v.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| Params::wrong(REMOTE_HOSTS, "liste de noms ou null"))?,
                ),
                Some(_) => return Err(Params::wrong(REMOTE_HOSTS, "liste de noms ou null")),
            };
            // Certificat fourni (SPEC §7.19, étape 3b) : les deux chemins ensemble, ou les
            // deux à null pour celui que fabrique le moteur.
            let tls_files = match (values.get(REMOTE_TLS_CERT), values.get(REMOTE_TLS_KEY)) {
                (None, None) => None,
                (Some(Value::Null), Some(Value::Null)) => Some(None),
                (Some(Value::String(cert)), Some(Value::String(key))) => {
                    Some(Some((cert.clone(), key.clone())))
                }
                _ => {
                    return Err(RpcError::invalid_params(format!(
                        "{REMOTE_TLS_CERT} et {REMOTE_TLS_KEY} : deux chemins, ou deux null, \
                         donnés ensemble"
                    )));
                }
            };
            // Vérifiés ici aussi, avant tout changement (le moteur les revérifie).
            if let Some(hosts) = &hosts
                && (hosts.len() > settings::REMOTE_HOSTS_MAX
                    || !hosts.iter().all(|h| settings::valid_host_name(h)))
            {
                return Err(Params::wrong(
                    REMOTE_HOSTS,
                    "noms DNS ou adresses IP, 32 au plus",
                ));
            }
            if let Some(Some((cert, key))) = &tls_files
                && !(std::path::Path::new(cert).is_absolute()
                    && std::path::Path::new(key).is_absolute())
            {
                return Err(RpcError::invalid_params(format!(
                    "{REMOTE_TLS_CERT}, {REMOTE_TLS_KEY} : chemins absolus attendus"
                )));
            }
            // Ratio de partage des torrents (SPEC §7.16) : un nombre positif ou nul, ou null
            // pour sans limite.
            let ratio = match values.get(TORRENT_RATIO) {
                None => None,
                Some(Value::Null) => Some(None),
                Some(v) => match v.as_f64() {
                    Some(r) if r.is_finite() && r >= 0.0 => Some(Some(r)),
                    _ => {
                        return Err(Params::wrong(
                            TORRENT_RATIO,
                            "nombre positif ou nul, ou null",
                        ));
                    }
                },
            };
            // Interface réseau des torrents (SPEC §7.16) : un nom, ou null pour aucune.
            let interface = match values.get(TORRENT_INTERFACE) {
                None => None,
                Some(Value::Null) => Some(None),
                Some(Value::String(name)) if settings::valid_interface_name(name) => {
                    Some(Some(name.clone()))
                }
                Some(_) => {
                    return Err(Params::wrong(
                        TORRENT_INTERFACE,
                        "nom d'interface (1 à 256 caractères, sans caractère de contrôle) ou null",
                    ));
                }
            };
            // Niveau des journaux (N-12) : error, warn, info ou debug.
            let logs_level = match values.get(settings::LOGS_LEVEL) {
                None => None,
                Some(Value::String(text)) => match crate::logs::Level::parse(text) {
                    Some(level) => Some(level),
                    None => {
                        return Err(Params::wrong(
                            settings::LOGS_LEVEL,
                            "error, warn, info ou debug",
                        ));
                    }
                },
                Some(_) => {
                    return Err(Params::wrong(
                        settings::LOGS_LEVEL,
                        "error, warn, info ou debug",
                    ));
                }
            };
            // Écoute de l'interface distante (SPEC §7.19) : adresse:port, ou null pour celle par
            // défaut.
            let listen = match values.get(REMOTE_LISTEN) {
                None => None,
                Some(Value::Null) => Some(None),
                Some(Value::String(text)) => match text.parse::<std::net::SocketAddr>() {
                    Ok(addr) => Some(Some(addr)),
                    Err(_) => {
                        return Err(Params::wrong(
                            REMOTE_LISTEN,
                            "adresse et port (127.0.0.1:7380, [::1]:7380) ou null",
                        ));
                    }
                },
                Some(_) => return Err(Params::wrong(REMOTE_LISTEN, "texte ou null")),
            };
            // Tous vérifiés avant d'en changer un.
            let mut flags = Vec::new();
            for flag in Flag::ALL {
                match values.get(flag.key()) {
                    None => {}
                    Some(Value::Bool(on)) => flags.push((flag, *on)),
                    Some(_) => return Err(Params::wrong(flag.key(), "booléen")),
                }
            }
            match values.get(DOWNLOAD_DIR) {
                None | Some(Value::Null) => {}
                // Le dossier aussi, comme le moteur le vérifiera : un dossier refusé ne laisse
                // pas les autres réglages changés (audit R6).
                Some(Value::String(dir)) => crate::scheduler::check_dest_dir(dir)?,
                Some(_) => return Err(Params::wrong(DOWNLOAD_DIR, "texte ou null")),
            }
            for (flag, on) in flags {
                ctx.engine.set_flag(flag, on).await?;
            }
            if let Some(ratio) = ratio {
                ctx.engine.set_torrent_ratio(ratio).await?;
            }
            if let Some(interface) = interface {
                ctx.engine.set_torrent_interface(interface).await?;
            }
            if let Some(level) = logs_level {
                ctx.engine.set_logs_level(level).await?;
            }
            if let Some(listen) = listen {
                ctx.engine.set_remote_listen(listen).await?;
            }
            if let Some(hosts) = hosts {
                ctx.engine.set_remote_hosts(hosts).await?;
            }
            if let Some(files) = tls_files {
                ctx.engine.set_remote_tls(files).await?;
            }
            if let Some(value) = values.get(DOWNLOAD_DIR) {
                let dir = match value {
                    Value::Null => None,
                    Value::String(dir) => Some(dir.clone()),
                    _ => return Err(Params::wrong(DOWNLOAD_DIR, "texte ou null")),
                };
                ctx.engine.set_download_dir(dir).await?;
            }
            settings(ctx).await
        }
        "limits.get" => limits(ctx).await,
        "limits.set" => {
            // Tout vérifié avant de changer quoi que ce soit.
            let download = p.limit("downloadBps")?;
            // Débit montant : torrents (SPEC §7.16, depuis le 2026-11-09).
            let upload = p.limit("uploadBps")?;
            if let Some(limit) = download {
                ctx.engine.set_download_limit(limit).await?;
            }
            if let Some(limit) = upload {
                ctx.engine.set_upload_limit(limit).await?;
            }
            limits(ctx).await
        }
        "events.subscribe" => subscribe(ctx, &p),
        "events.unsubscribe" => {
            ctx.stop_events();
            Ok(json!({}))
        }
        "sync.snapshot" => {
            let s = ctx.engine.snapshot().await?;
            let mut settings = settings_json(
                s.download_dir,
                &s.flags,
                s.torrent_ratio,
                s.torrent_interface,
                s.logs_level,
            );
            if ctx.engine.is_restricted() {
                hide_remote_settings(&mut settings);
            }
            Ok(json!({
                "rev": s.rev,
                "tasks": s.tasks.iter().map(task_json).collect::<Vec<_>>(),
                "queues": s.queues.iter().map(queue_json).collect::<Vec<_>>(),
                "limits": {"downloadBps": s.download_limit, "uploadBps": s.upload_limit},
                "settings": settings,
                "networkUsable": s.network_usable,
                "networkMetered": s.network_metered,
                "torrentInterfaceUp": s.torrent_interface_up,
                "schedules": s.schedules.iter().map(schedule_json).collect::<Vec<_>>(),
                "endAction": end_action_json(s.end_action.as_ref()),
            }))
        }
        "schedule.list" => {
            let items = ctx.engine.schedules().await?;
            Ok(json!({"items": items.iter().map(schedule_json).collect::<Vec<_>>()}))
        }
        "schedule.set" => {
            let rules = p
                .get("rules")
                .ok_or_else(|| RpcError::invalid_params("rules manquant"))
                .and_then(|v| Rules::from_json(v).map_err(RpcError::invalid_params))?;
            let view = ctx
                .engine
                .set_schedule(
                    p.str("scheduleId")?,
                    p.required_str("name")?,
                    p.bool_or("enabled", true)?,
                    rules,
                )
                .await?;
            Ok(schedule_json(&view))
        }
        "schedule.delete" => {
            ctx.engine
                .delete_schedule(&p.required_str("scheduleId")?)
                .await?;
            Ok(json!({}))
        }
        // Proxy (F-07, phase 7) : jamais le mot de passe en sortie.
        "proxy.get" => Ok(proxy_json(&ctx.engine.proxy().await?)),
        "proxy.set" => {
            use crate::net::proxy::{Mode, Settings};
            let mode = p.required_str("mode")?;
            let mode = Mode::parse(&mode)
                .ok_or_else(|| Params::wrong("mode", "system, none ou manual"))?;
            let password = match p.0.get("password") {
                None => None,
                Some(Value::Null) => Some(None),
                Some(Value::String(s)) => Some(Some(s.clone())),
                Some(_) => return Err(Params::wrong("password", "texte ou null")),
            };
            let change = crate::scheduler::ProxyChange {
                settings: Settings {
                    mode,
                    url: p.str("url")?,
                    exceptions: p.strings("exceptions")?.unwrap_or_default(),
                    username: p.str("username")?,
                },
                password,
            };
            Ok(proxy_json(&ctx.engine.set_proxy(change).await?))
        }
        // Règles de rangement et commande de fin (F-32, F-34, SPEC §7.21) : une connexion à
        // jeton les lit sans leurs commandes, et ne les écrit jamais (portée `admin`).
        "rules.list" => {
            let commands = !ctx.engine.is_restricted();
            Ok(rules_json(&ctx.engine.rules().await?, commands))
        }
        "rules.set" => {
            let list = match p.get("rules") {
                Some(Value::Array(list)) => list,
                _ => return Err(Params::wrong("rules", "liste de règles")),
            };
            let rules = list
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    crate::rules::Rule::from_json(r)
                        .map_err(|e| RpcError::invalid_params(format!("rules[{i}] : {e}")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rules_json(&ctx.engine.set_rules(rules).await?, true))
        }
        // Diagnostic (N-12, phase 7) : état sans adresses ni identifiants, journaux nettoyés.
        "engine.diagnostic" => diagnostic(ctx, &p).await,
        "engine.cancelEndAction" => {
            let cancelled = ctx.engine.cancel_end_action().await?;
            Ok(json!({"cancelled": cancelled}))
        }
        // Réseau utilisable ou non (SPEC §7.15, depuis le 2026-11-09) : dit par l'application.
        "engine.setNetwork" => {
            let usable = match p.get("usable") {
                Some(Value::Bool(usable)) => *usable,
                _ => return Err(Params::wrong("usable", "booléen")),
            };
            // Réseau facturé au volume : pas de partage de torrents (SPEC §7.16).
            let metered = p.bool("metered")?;
            ctx.engine.set_network(usable, metered).await?;
            Ok(json!({"usable": usable, "metered": metered}))
        }
        _ => Err(RpcError::method_not_found(method)),
    }
}

/// Le proxy tel que le protocole le montre : jamais le mot de passe.
/// Règles de rangement (`rules.list`, `rules.set`) ; `commands` : faux pour une connexion à
/// jeton, qui ne voit que la présence d'une commande.
fn rules_json(rules: &[crate::rules::Rule], commands: bool) -> Value {
    json!({"rules": rules.iter().map(|r| r.to_json(commands)).collect::<Vec<_>>()})
}

fn proxy_json(view: &crate::scheduler::ProxyView) -> Value {
    json!({
        "mode": view.settings.mode.as_str(),
        "url": view.settings.url,
        "exceptions": view.settings.exceptions,
        "username": view.settings.username,
        "hasPassword": view.has_password,
    })
}

/// Journaux d'un diagnostic, au plus (les plus récents).
const DIAGNOSTIC_LOGS: usize = 2 << 20;

/// `engine.diagnostic` (N-12) : de quoi comprendre un problème sans rien révéler de
/// l'utilisateur. Ni adresse, ni nom de fichier, ni chemin, ni identifiant : des comptes,
/// des états, des codes ; les journaux nettoyés (`logs::scrub`). Le client le montre avant
/// de l'enregistrer.
async fn diagnostic(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    use std::collections::BTreeMap;
    let max_bytes = p
        .u32("maxLogBytes")?
        .map_or(DIAGNOSTIC_LOGS, |n| (n as usize).min(DIAGNOSTIC_LOGS));
    let engine = &ctx.engine;
    let snapshot = engine.snapshot().await?;
    let count = |items: Vec<&str>| {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for item in items {
            *counts.entry(item.to_owned()).or_default() += 1;
        }
        json!(counts)
    };
    let tasks = &snapshot.tasks;
    let defined = |set: bool| if set { json!("<défini>") } else { Value::Null };
    let mut flags = Map::new();
    for (flag, on) in &snapshot.flags {
        flags.insert(flag.key().to_owned(), json!(on));
    }
    let remote = engine.remote_state();
    let download_dir = engine.download_dir().await?;
    let masks = crate::logs::Masks::of_machine(Some(engine.data_dir()), download_dir.as_deref());
    // L'erreur des outils est un texte libre (chemins, adresses) : nettoyée comme les
    // journaux (audit R9).
    let tools = engine.media_tools().ok().map(|status| {
        let mut json = tools_json(&status);
        if let Some(error) = &status.error {
            json["error"] = json!(crate::logs::scrub_text(error, &masks));
        }
        json
    });
    let plugins = engine.plugins().await?;
    let report = json!({
        "generatedAt": crate::store::now_ms(),
        "engine": {
            "version": env!("CARGO_PKG_VERSION"),
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "protocol": PROTOCOL_VERSIONS.iter().map(|(a, b)| format!("{a}.{b}")).collect::<Vec<_>>(),
        },
        "settings": {
            "downloads.dir": defined(snapshot.download_dir.is_some()),
            "torrent.ratioLimit": snapshot.torrent_ratio,
            "torrent.interface": defined(snapshot.torrent_interface.is_some()),
            "logs.level": snapshot.logs_level.as_str(),
            "flags": flags,
        },
        "limits": {
            "downloadBps": snapshot.download_limit.is_some(),
            "uploadBps": snapshot.upload_limit.is_some(),
        },
        "network": {
            "usable": snapshot.network_usable,
            "metered": snapshot.network_metered,
            "torrentInterfaceUp": snapshot.torrent_interface_up,
        },
        "tasks": {
            "count": tasks.len(),
            "byState": count(tasks.iter().map(|t| t.state.as_sql()).collect()),
            "byKind": count(tasks.iter().map(|t| t.kind.as_sql()).collect()),
            "errorCodes": count(tasks.iter().filter_map(|t| t.error_code.as_deref()).collect()),
            "pauseReasons": count(tasks.iter().filter_map(|t| t.pause_reason.as_deref()).collect()),
            "publicOnly": tasks.iter().filter(|t| t.public_only).count(),
            "moving": tasks.iter().filter(|t| t.moving).count(),
        },
        "queues": snapshot.queues.len(),
        "schedules": snapshot.schedules.len(),
        "endActionPending": snapshot.end_action.is_some(),
        "plugins": {
            "installed": plugins.len(),
            "enabled": plugins.iter().filter(|p| p.enabled).count(),
        },
        "remote": {
            "enabled": remote.enabled,
            "listening": remote.listening,
            "tls": remote.tls,
            "error": remote.error.is_some(),
            "generatedCertificate": remote.certificate.as_ref().map(|c| c.generated),
            "hosts": remote.hosts.len(),
        },
        "media": tools,
    });
    let dir = crate::logs::installed_dir()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| engine.data_dir().join(crate::logs::DIR));
    let logs = crate::logs::collect(&dir, &masks, max_bytes);
    Ok(json!({"report": report, "logs": logs}))
}

fn hello(ctx: &mut Ctx, p: &Params) -> Result<Value, RpcError> {
    let protocol = p
        .get("protocol")
        .and_then(Value::as_object)
        .ok_or_else(|| RpcError::invalid_params("protocol manquant"))?;
    let bound = |name: &str| {
        protocol
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params(format!("protocol.{name} manquant")))
    };
    let Some((major, minor)) = negotiate(bound("min")?, bound("max")?) else {
        // Aucune version commune : erreur, puis fermeture (SPEC §6.3).
        ctx.close_after = true;
        let ours: Vec<String> = PROTOCOL_VERSIONS
            .iter()
            .map(|(a, b)| format!("{a}.{b}"))
            .collect();
        return Err(RpcError::new(
            -32002,
            "PROTOCOL_UNSUPPORTED",
            format!("versions du protocole de ce moteur : {}", ours.join(", ")),
        ));
    };
    // Portée demandée : seulement plus étroite que celle de la connexion (SPEC §6.9).
    if let Some(text) = p.str("scope")? {
        let wanted = Scope::parse(&text).ok_or_else(|| {
            RpcError::invalid_params("scope : read, add, control ou admin attendu")
        })?;
        if !ctx.scope.allows(wanted) {
            return Err(RpcError::unauthorized(format!(
                "portée {text} refusée : cette connexion a la portée {}",
                ctx.scope.as_str()
            )));
        }
        ctx.scope = wanted;
        // Plus le droit de lire : l'abonnement en cours s'arrête.
        if !wanted.allows(Scope::Read) {
            ctx.stop_events();
        }
    }
    Ok(json!({
        "engine": {"version": env!("CARGO_PKG_VERSION"), "platform": std::env::consts::OS},
        "protocol": format!("{major}.{minor}"),
        // Vidéo (SPEC §7.17) : Windows et Linux seulement. Analyse de liens (SPEC §7.18) et
        // Metalink : depuis le 2026-11-11.
        "capabilities": if crate::media::SUPPORTED {
            json!(["torrent", "media", "linkscan", "metalink"])
        } else {
            json!(["torrent", "linkscan", "metalink"])
        },
        "rev": ctx.engine.rev(),
        "scope": ctx.scope.as_str(),
    }))
}

/// Réglages du moteur (`settings.get`) : tous ceux qu'il prend en charge, avec la valeur
/// retenue (pour `downloads.dir`, le dossier du système si aucun n'est enregistré ; pour
/// les réglages oui ou non, vrai).
async fn settings(ctx: &Ctx) -> Result<Value, RpcError> {
    let dir = ctx.engine.download_dir().await?;
    let mut flags = Vec::new();
    for flag in Flag::ALL {
        flags.push((flag, ctx.engine.flag(flag).await?));
    }
    let ratio = ctx.engine.torrent_ratio().await?;
    let ratio = ratio.map_or(Value::Null, Value::from);
    let interface = ctx.engine.torrent_interface().await?;
    let level = ctx.engine.logs_level().await?;
    let mut values = settings_json(dir, &flags, ratio, interface, level);
    if ctx.engine.is_restricted() {
        hide_remote_settings(&mut values);
        return Ok(json!({"values": values}));
    }
    let remote = ctx.engine.remote_config().await?;
    values[REMOTE_LISTEN] = json!(remote.listen.to_string());
    values[REMOTE_HOSTS] = json!(remote.hosts);
    let (cert, key) = remote.tls_files.unzip();
    values[REMOTE_TLS_CERT] = json!(cert);
    values[REMOTE_TLS_KEY] = json!(key);
    Ok(json!({"values": values}))
}

/// Les réglages de l'écoute (`remote.*` : chemins du certificat et de sa clé, noms,
/// adresse) ne regardent que le canal local (SPEC §7.19) : une connexion à jeton ne les
/// voit ni dans `settings.get`, ni dans `sync.snapshot`, ni dans `settings.updated`
/// (audit R1).
pub(crate) fn is_remote_setting(key: &str) -> bool {
    key.starts_with("remote.")
}

fn hide_remote_settings(values: &mut Value) {
    if let Some(map) = values.as_object_mut() {
        map.retain(|key, _| !is_remote_setting(key));
    }
}

/// Valeurs des réglages, telles que `settings.get` et `sync.snapshot` les donnent.
fn settings_json(
    download_dir: Option<String>,
    flags: &[(Flag, bool)],
    ratio: Value,
    interface: Option<String>,
    logs_level: crate::logs::Level,
) -> Value {
    let mut values = json!({
        DOWNLOAD_DIR: download_dir,
        TORRENT_RATIO: ratio,
        TORRENT_INTERFACE: interface,
        settings::LOGS_LEVEL: logs_level.as_str(),
    });
    for (flag, on) in flags {
        values[flag.key()] = json!(on);
    }
    values
}

/// Vidéo décrite par yt-dlp (`media.probe`).
fn probe_json(probe: &crate::media::extract::Probe) -> Value {
    let formats: Vec<Value> = probe
        .formats
        .iter()
        .map(|f| {
            json!({
                "formatId": f.format_id,
                "ext": f.ext,
                "vcodec": f.vcodec,
                "acodec": f.acodec,
                "width": f.width,
                "height": f.height,
                "fps": f.fps,
                "tbr": f.tbr,
                "filesize": f.filesize,
                "filesizeApprox": f.filesize_approx,
                "protocol": f.protocol,
                "drm": f.drm,
                "note": f.note,
            })
        })
        .collect();
    json!({
        "title": probe.title,
        "durationS": probe.duration_s,
        "extractor": probe.extractor,
        "thumbnail": probe.thumbnail,
        "formats": formats,
        "subtitles": probe.subtitles,
    })
}

/// État des outils de la vidéo (`media.tools`, notification `media.tools`).
pub(crate) fn tools_json(status: &crate::media::tools::Status) -> Value {
    let tool = |t: &Option<crate::media::tools::Tool>| {
        t.as_ref()
            .map(|t| json!({"version": t.version, "origin": t.origin.as_str()}))
    };
    json!({
        "ytdlp": tool(&status.ytdlp),
        "ffmpeg": tool(&status.ffmpeg),
        "ready": status.ready(),
        "installing": status.installing,
        "error": status.error,
    })
}

/// Limites globales (`limits.get`) : réception, et envoi (torrents).
async fn limits(ctx: &Ctx) -> Result<Value, RpcError> {
    let download = ctx.engine.download_limit().await?;
    let upload = ctx.engine.upload_limit().await?;
    Ok(json!({"downloadBps": download, "uploadBps": upload}))
}

/// Identifiants d'un site tels que le protocole les montre, sans mot de passe.
fn token_json(t: &crate::store::remote::TokenRow) -> Value {
    json!({
        "id": t.id,
        "name": t.name,
        "scope": t.scope,
        "createdAt": t.created_at,
        "lastUsedAt": t.last_used_at,
    })
}

/// `host` et `port` (1 à 65535) d'une clé SSH.
fn host_and_port(p: &Params) -> Result<(String, u16), RpcError> {
    let host = p.required_str("host")?;
    let port = p
        .u32("port")?
        .and_then(|n| u16::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| Params::wrong("port", "port de 1 à 65535"))?;
    Ok((host, port))
}

fn login_json(l: &LoginView) -> Value {
    json!({
        "loginId": l.id,
        "site": l.site,
        "username": l.username,
        "allowPlain": l.allow_plain,
        "keyPath": l.key_path,
    })
}

/// `task.update` : limite de débit, empreinte (F-11), nouvelle adresse (SPEC §7.13), ou
/// nouveau dossier et nouveau nom (SPEC §7.12). Chacun se change à part : chaque
/// opération est vérifiée en entier avant de changer quoi que ce soit, pas leur
/// combinaison.
async fn task_update(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    p.refuse(&["maxConnections"])?;
    let id = p.required_str("taskId")?;
    let url = p.str("url")?;
    let dest_dir = p.str("destDir")?;
    let file_name = p.str("fileName")?;
    let speed_limit_bps = p.limit("speedLimitBps")?;
    let checksum = p.checksum("checksum")?;
    let relocate = dest_dir.is_some() || file_name.is_some();
    let several = [
        url.is_some(),
        relocate,
        speed_limit_bps.is_some(),
        checksum.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if several > 1 {
        return Err(RpcError::invalid_params(
            "url, destDir et fileName, speedLimitBps, et checksum se changent par des appels \
             séparés",
        ));
    }
    let task = if let Some(url) = url {
        ctx.engine.set_url(&id, &url).await?
    } else if relocate {
        ctx.engine.relocate(&id, dest_dir, file_name).await?
    } else {
        let update = TaskUpdate {
            speed_limit_bps,
            checksum,
        };
        ctx.engine.update_task(&id, update).await?
    };
    Ok(task_json(&task))
}

async fn task_add(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    if !matches!(p.get("plugin"), None | Some(Value::Null | Value::Bool(_))) {
        return Err(Params::wrong("plugin", "booléen"));
    }
    let idempotency_key = p.str("idempotencyKey")?;
    if idempotency_key
        .as_ref()
        .is_some_and(|k| k.is_empty() || k.len() > MAX_IDEMPOTENCY_KEY)
    {
        return Err(RpcError::invalid_params(format!(
            "idempotencyKey : de 1 à {MAX_IDEMPOTENCY_KEY} octets"
        )));
    }
    // Vidéo d'une page (SPEC §7.17) : `media` à la place de `sources`.
    let media = match p.get("media") {
        None | Some(Value::Null) => None,
        Some(Value::Object(m)) => {
            if p.get("sources").is_some() {
                return Err(RpcError::invalid_params("media ou sources, pas les deux"));
            }
            Some(media_request(m)?)
        }
        Some(_) => return Err(Params::wrong("media", "objet")),
    };
    let no_sources = Vec::new();
    let sources = match &media {
        Some(_) => &no_sources,
        None => p
            .get("sources")
            .and_then(Value::as_array)
            .ok_or_else(|| RpcError::invalid_params("sources manquant"))?,
    };
    // Une adresse (`url`, lien magnet compris), ou un fichier `.torrent` en base64
    // (`torrent`, SPEC §7.16).
    let (url, torrent) = match sources.as_slice() {
        [] if media.is_some() => ("", None),
        [one] => match (one.get("url"), one.get("torrent")) {
            (Some(url), None) => (
                url.as_str()
                    .ok_or_else(|| RpcError::invalid_params("sources[0].url : texte attendu"))?,
                None,
            ),
            (None, Some(torrent)) => {
                let text = torrent.as_str().ok_or_else(|| {
                    RpcError::invalid_params("sources[0].torrent : texte base64 attendu")
                })?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .map_err(|_| {
                        RpcError::invalid_params("sources[0].torrent : base64 invalide")
                    })?;
                ("", Some(bytes))
            }
            (Some(_), Some(_)) => {
                return Err(RpcError::invalid_params(
                    "sources[0] : url ou torrent, pas les deux",
                ));
            }
            (None, None) => {
                return Err(RpcError::invalid_params("sources[0].url manquant"));
            }
        },
        [] => return Err(RpcError::invalid_params("sources vide")),
        // Plusieurs sources (SPEC §7.7, phase 6) : la référence, puis ses miroirs, toutes
        // des adresses.
        [first, ..] => (
            first
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("sources[0].url : texte attendu"))?,
            None,
        ),
    };
    let mirrors: Vec<String> = sources
        .iter()
        .enumerate()
        .skip(1)
        .map(
            |(i, s)| match (s.get("url").and_then(Value::as_str), s.get("torrent")) {
                (Some(url), None) => Ok(url.to_owned()),
                _ => Err(RpcError::invalid_params(format!(
                    "sources[{i}].url : une adresse attendue (un .torrent n'a qu'une source)"
                ))),
            },
        )
        .collect::<Result<_, _>>()?;
    // Absent : le dossier des téléchargements du moteur (`settings.get`).
    let dest_dir = p.str("destDir")?;
    let max_connections = p.u32("maxConnections")?;
    if max_connections.is_some_and(|c| c == 0 || c > MAX_CONNECTIONS) {
        return Err(RpcError::invalid_params(format!(
            "maxConnections : de 1 à {MAX_CONNECTIONS}"
        )));
    }
    let task_id = ctx
        .engine
        .add(AddTask {
            url: url.to_owned(),
            dest_dir,
            queue_id: p.str("queueId")?,
            max_connections,
            speed_limit_bps: p.limit("speedLimitBps")?.flatten(),
            start_paused: p.bool("startPaused")?,
            idempotency_key,
            headers: p.string_map("headers")?,
            file_name: p.str("fileName")?,
            checksum: p.checksum("checksum")?.flatten(),
            // Seulement d.un Metalink (`metalink.add`, F-12).
            pieces: None,
            torrent,
            media,
            mirrors,
            sub_dir: sub_dir_param(p)?,
            // Posés par la poignée restreinte d'une connexion à jeton (SPEC §7.19).
            remote: false,
            public_only: false,
            plugin: None,
            // `plugin: false` : l'adresse telle quelle, même pour un site d'un plugin
            // (SPEC §7.20).
            no_plugin: matches!(p.get("plugin"), Some(Value::Bool(false))),
        })
        .await?;
    Ok(json!({"taskId": task_id}))
}

/// Profondeur d'un `subDir`, au plus.
const MAX_SUB_DIR_DEPTH: usize = 8;

/// `subDir` (SPEC §6.5, étape 3c) : sous-dossier créé sous la destination, parties séparées
/// par `/` (chacune un nom de fichier valide, vérifié par le moteur) ; absent ou vide : aucun.
fn sub_dir_param(p: &Params) -> Result<Vec<String>, RpcError> {
    let Some(text) = p.str("subDir")? else {
        return Ok(Vec::new());
    };
    let parts: Vec<String> = text
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    if parts.len() > MAX_SUB_DIR_DEPTH || parts.iter().any(|part| part == "." || part == "..") {
        return Err(Params::wrong(
            "subDir",
            "sous-dossier relatif, parties séparées par / (8 au plus, sans . ni ..)",
        ));
    }
    Ok(parts)
}

/// Éléments d'un `task.addBatch`, au plus.
const MAX_BATCH: usize = 1000;
/// Adresses d'un `linkscan.probe`, au plus.
const MAX_PROBES: usize = 50;

/// `task.addBatch` (SPEC §6.5, analyse de liens) : chaque élément est un `task.add`, ajouté
/// ou refusé pour lui-même (décision de l'utilisateur du 2026-11-11) ; la réponse donne les
/// tâches créées et les refus, avec le code qu'aurait eu `task.add`.
async fn task_add_batch(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    let items = p
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| Params::wrong("items", "liste de paramètres de task.add"))?;
    if items.is_empty() || items.len() > MAX_BATCH {
        return Err(RpcError::invalid_params(format!(
            "items : de 1 à {MAX_BATCH} éléments"
        )));
    }
    let (mut added, mut errors) = (Vec::new(), Vec::new());
    for (index, item) in items.iter().enumerate() {
        let result = match Params::new(item.clone()) {
            Ok(params) => task_add(ctx, &params).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(task) => added.push(json!({"index": index, "taskId": task["taskId"]})),
            Err(e) => {
                errors.push(json!({"index": index, "code": e.data_code, "message": e.message}))
            }
        }
    }
    Ok(json!({"tasks": added, "errors": errors}))
}

/// `linkscan.run` (F-29, F-31, SPEC §7.18) : les liens d'un texte (`text`) ou d'une page
/// (`pageUrl`, lue par le moteur).
async fn linkscan_run(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    let (links, page) = match (p.str("text")?, p.str("pageUrl")?) {
        (Some(text), None) => {
            if text.len() > crate::linkscan::MAX_INPUT {
                return Err(RpcError::invalid_params(format!(
                    "text : {} Mio au plus",
                    crate::linkscan::MAX_INPUT >> 20
                )));
            }
            (crate::linkscan::from_text(&text), None)
        }
        (None, Some(url)) => {
            let (page, links) = ctx.engine.scan_page(&url).await?;
            (links, Some(page))
        }
        _ => return Err(RpcError::invalid_params("text ou pageUrl, l'un des deux")),
    };
    let links: Vec<Value> = links
        .iter()
        .map(|l| json!({"url": l.url, "name": l.name, "ext": l.ext, "kind": l.kind.as_str()}))
        .collect();
    let mut result = json!({"links": links});
    if let Some(page) = page {
        result["pageUrl"] = json!(page);
    }
    Ok(result)
}

/// `linkscan.probe` (SPEC §7.18) : taille et nom de chaque fichier (50 adresses au plus par
/// appel), ou la raison de l'échec.
async fn linkscan_probe(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    let urls: Vec<String> = p
        .get("urls")
        .and_then(Value::as_array)
        .ok_or_else(|| Params::wrong("urls", "liste d'adresses"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| Params::wrong("urls", "liste d'adresses"))
        })
        .collect::<Result<_, _>>()?;
    if urls.is_empty() || urls.len() > MAX_PROBES {
        return Err(RpcError::invalid_params(format!(
            "urls : de 1 à {MAX_PROBES} adresses"
        )));
    }
    let results = ctx.engine.probe_links(urls.clone()).await;
    let items: Vec<Value> = urls
        .iter()
        .zip(results)
        .map(|(url, result)| match result {
            // `kind` : ce que dit le serveur (page web ou fichier), `null` s'il ne dit rien.
            Ok(probe) => json!({"url": url, "size": probe.total_bytes, "name": probe.file_name,
                "kind": probe.is_page().map(|page| if page { "page" } else { "file" })}),
            Err(e) => {
                let e = RpcError::from(match e {
                    e @ (crate::net::NetError::InvalidUrl(_)
                    | crate::net::NetError::UnsupportedScheme(_)) => EngineError::Url(e),
                    e => EngineError::SourceUnreachable(e),
                });
                json!({"url": url, "size": null, "name": null,
                       "error": {"code": e.data_code, "message": e.message}})
            }
        })
        .collect();
    Ok(json!({"items": items}))
}

/// Fichier Metalink d'un appel (`metalink.read`, `metalink.add`, SPEC §7.7) : `data`
/// (contenu en base64) ou `url` (lue par le moteur).
fn metalink_source(p: &Params) -> Result<MetalinkSource, RpcError> {
    match (p.str("data")?, p.str("url")?) {
        (Some(data), None) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| RpcError::invalid_params("data : base64 invalide"))?;
            Ok(MetalinkSource::Data(bytes))
        }
        (None, Some(url)) => Ok(MetalinkSource::Url(url)),
        _ => Err(RpcError::invalid_params("data ou url, l'un des deux")),
    }
}

/// `metalink.read` : les fichiers décrits, ceux écartés, et le contenu lu à une adresse.
async fn metalink_read(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    let source = metalink_source(p)?;
    let from_url = matches!(source, MetalinkSource::Url(_));
    let (doc, bytes) = ctx.engine.read_metalink(source).await?;
    let mut result = metalink_json(&doc);
    // Le même document pour l'ajout qui suit, sans le relire à l'adresse.
    if from_url {
        result["data"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
    }
    Ok(result)
}

fn metalink_json(doc: &Metalink) -> Value {
    json!({
        "format": doc.format.as_str(),
        "files": doc.files.iter().enumerate().map(|(index, f)| json!({
            "index": index,
            "name": f.name,
            "path": f.path(),
            "size": f.size,
            "checksum": f.checksum.as_ref().map(|c| json!({"algo": c.algo().as_sql(), "value": c.value()})),
            "urls": f.urls,
            "droppedUrls": f.dropped_urls,
        })).collect::<Vec<_>>(),
        "ignored": doc.ignored.iter().map(|i| json!({"name": i.name, "reason": i.reason})).collect::<Vec<_>>(),
    })
}

/// `metalink.add` : une tâche HTTP par fichier choisi (`select`, tous sinon), avec ses
/// miroirs et son empreinte (SPEC §7.7). Un refus n'arrête pas les autres fichiers : il est
/// rendu dans `errors`, avec le code d'erreur qu'aurait eu `task.add`.
async fn metalink_add(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    let (doc, _) = ctx.engine.read_metalink(metalink_source(p)?).await?;
    let select: Vec<usize> = match p.get("select") {
        None | Some(Value::Null) => (0..doc.files.len()).collect(),
        Some(Value::Array(list)) => list
            .iter()
            .map(|v| {
                v.as_u64()
                    .and_then(|i| usize::try_from(i).ok())
                    .filter(|&i| i < doc.files.len())
                    .ok_or_else(|| Params::wrong("select", "liste de rangs de fichiers"))
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(Params::wrong("select", "liste de rangs de fichiers")),
    };
    let mut select = select;
    select.sort_unstable();
    select.dedup();
    if select.is_empty() {
        return Err(RpcError::invalid_params("select : au moins un fichier"));
    }
    let idempotency_key = p.str("idempotencyKey")?;
    if idempotency_key
        .as_ref()
        .is_some_and(|k| k.is_empty() || k.len() > MAX_IDEMPOTENCY_KEY)
    {
        return Err(RpcError::invalid_params(format!(
            "idempotencyKey : de 1 à {MAX_IDEMPOTENCY_KEY} octets"
        )));
    }
    let max_connections = p.u32("maxConnections")?;
    if max_connections.is_some_and(|c| c == 0 || c > MAX_CONNECTIONS) {
        return Err(RpcError::invalid_params(format!(
            "maxConnections : de 1 à {MAX_CONNECTIONS}"
        )));
    }
    let prefix = sub_dir_param(p)?;
    let base = AddTask {
        dest_dir: p.str("destDir")?,
        queue_id: p.str("queueId")?,
        max_connections,
        speed_limit_bps: p.limit("speedLimitBps")?.flatten(),
        start_paused: p.bool("startPaused")?,
        ..AddTask::default()
    };
    let (mut added, mut errors) = (Vec::new(), Vec::new());
    for index in select {
        let file = &doc.files[index];
        let task = AddTask {
            url: file.urls[0].clone(),
            mirrors: file.urls[1..].to_vec(),
            file_name: Some(file.name.clone()),
            sub_dir: prefix.iter().chain(&file.dir).cloned().collect(),
            checksum: file.checksum.clone(),
            pieces: file.pieces.clone(),
            // Une clé par fichier : un ajout répété rend les mêmes tâches.
            idempotency_key: idempotency_key.as_ref().map(|k| format!("{k}#{index}")),
            ..base.clone()
        };
        match ctx.engine.add(task).await {
            Ok(task_id) => {
                added.push(json!({"index": index, "path": file.path(), "taskId": task_id}))
            }
            Err(e) => {
                let e = RpcError::from(e);
                errors.push(json!({"index": index, "path": file.path(), "code": e.data_code, "message": e.message}));
            }
        }
    }
    Ok(json!({"tasks": added, "errors": errors}))
}

/// `data` d'un fichier de plugin (SPEC §7.20) : contenu en base64, 8 Mio au plus.
fn plugin_data(p: &Params) -> Result<Vec<u8>, RpcError> {
    let text = p.required_str("data")?;
    if text.len() > crate::plugins::manifest::MAX_FILE / 3 * 4 + 4 {
        return Err(RpcError::invalid_params(format!(
            "data : fichier de plugin de {} Mio au plus",
            crate::plugins::manifest::MAX_FILE >> 20
        )));
    }
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| RpcError::invalid_params("data : base64 invalide"))
}

fn manifest_json(m: &crate::plugins::Manifest) -> Value {
    json!({
        "pluginId": m.id,
        "name": m.name,
        "version": m.version,
        "author": m.author,
        "description": m.description,
        "sites": m.sites,
        "fetch": m.fetch,
    })
}

fn inspected_json(i: &crate::plugins::Inspected) -> Value {
    let mut view = manifest_json(&i.manifest);
    view["digest"] = json!(i.digest);
    view["size"] = json!(i.size);
    view
}

fn plugin_json(row: &crate::store::plugins::PluginRow) -> Value {
    json!({
        "pluginId": row.id,
        "name": row.name,
        "version": row.version,
        "author": row.author,
        "description": row.description,
        "sites": row.sites,
        "fetch": row.fetch,
        "digest": row.digest,
        "enabled": row.enabled,
        "installedAt": row.installed_at,
    })
}

/// Une résolution : jamais ses cookies.
fn resolution_json(r: &crate::plugins::Resolution) -> Value {
    json!({
        "resolutionId": r.id,
        "pluginId": r.plugin_id,
        "pluginName": r.plugin_name,
        "page": r.page,
        "links": r.links.iter().map(|l| json!({
            "url": l.url,
            "name": l.name,
            "size": l.size,
        })).collect::<Vec<_>>(),
        "logs": r.logs,
    })
}

/// `plugin.add` (SPEC §7.20) : les liens choisis (`links`, rangs) d'une résolution gardée ;
/// dossier, sous-dossier, file et pause comme `task.add`. Chaque lien est ajouté ou refusé
/// pour lui-même.
async fn plugin_add(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    let resolution = p.required_str("resolutionId")?;
    let picked: Vec<usize> = p
        .get("links")
        .and_then(Value::as_array)
        .ok_or_else(|| Params::wrong("links", "liste de rangs de liens"))?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|i| usize::try_from(i).ok())
                .ok_or_else(|| Params::wrong("links", "liste de rangs de liens"))
        })
        .collect::<Result<_, _>>()?;
    let base = AddTask {
        dest_dir: p.str("destDir")?,
        queue_id: p.str("queueId")?,
        speed_limit_bps: p.limit("speedLimitBps")?.flatten(),
        start_paused: p.bool("startPaused")?,
        sub_dir: sub_dir_param(p)?,
        ..AddTask::default()
    };
    let results = ctx
        .engine
        .add_from_plugin(&resolution, &picked, base)
        .await?;
    let (mut added, mut errors) = (Vec::new(), Vec::new());
    for (index, result) in picked.iter().zip(results) {
        match result {
            Ok(task_id) => added.push(json!({"index": index, "taskId": task_id})),
            Err(e) => {
                let e = RpcError::from(e);
                errors.push(json!({"index": index, "code": e.data_code, "message": e.message}));
            }
        }
    }
    Ok(json!({"tasks": added, "errors": errors}))
}

/// `media` de `task.add` (SPEC §7.17) : `pageUrl`, et `format`, `container`, `subtitles`,
/// `title` facultatifs.
fn media_request(m: &serde_json::Map<String, Value>) -> Result<MediaRequest, RpcError> {
    let text = |key: &str| -> Result<Option<String>, RpcError> {
        match m.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(RpcError::invalid_params(format!(
                "media.{key} : texte attendu"
            ))),
        }
    };
    let page_url =
        text("pageUrl")?.ok_or_else(|| RpcError::invalid_params("media.pageUrl manquant"))?;
    let subtitles = match m.get("subtitles") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) => list
            .iter()
            .map(|v| {
                v.as_str().map(str::to_owned).ok_or_else(|| {
                    RpcError::invalid_params("media.subtitles : codes de langue attendus")
                })
            })
            .collect::<Result<_, _>>()?,
        Some(_) => {
            return Err(RpcError::invalid_params(
                "media.subtitles : liste de codes de langue attendue",
            ));
        }
    };
    Ok(MediaRequest {
        page_url,
        format: text("format")?,
        container: text("container")?,
        subtitles,
        title: text("title")?,
    })
}

/// `task.get` : la tâche, et sur demande (`include`) ses segments et ses sources.
async fn task_get(ctx: &mut Ctx, p: &Params) -> Result<Value, RpcError> {
    let mut include = Include::default();
    for item in p.strings("include")?.unwrap_or_default() {
        match item.as_str() {
            "segments" => include.segments = true,
            "sources" => include.sources = true,
            // Fichiers d'un torrent (SPEC §7.16, depuis le 2026-11-09).
            "files" => include.files = true,
            other => {
                return Err(RpcError::invalid_params(format!(
                    "include : valeur inconnue {other}"
                )));
            }
        }
    }
    let task_id = p.required_str("taskId")?;
    let details = ctx.engine.details(&task_id, include).await?;
    let mut task = details_json(&details);
    // Tâche venue d'un plugin (SPEC §7.20, depuis le 2026-11-12) : sa page et le plugin,
    // dont le nom est null s'il a été retiré depuis.
    if let Some((page, plugin_id)) = ctx.engine.plugin_origin(&task_id).await? {
        let name = ctx
            .engine
            .plugins()
            .await?
            .into_iter()
            .find(|row| row.id == plugin_id)
            .map(|row| row.name);
        task["plugin"] = json!({"page": page, "pluginId": plugin_id, "name": name});
    }
    Ok(task)
}

fn details_json(d: &TaskDetails) -> Value {
    let mut task = task_json(&d.task);
    if let Some(segments) = &d.segments {
        task["segments"] = segments
            .iter()
            .map(|(s, receiving)| {
                let mut segment = json!({
                    "idx": s.idx,
                    "start": s.start,
                    "end": s.end,
                    // Vidés sur disque, ou reçus par la connexion en cours, le plus grand.
                    "doneBytes": receiving.as_ref().map_or(s.written, |r| r.received.max(s.written)),
                    "receiving": receiving.is_some(),
                });
                // Source qui l'envoie, s'il est en cours de réception (SPEC §7.7, depuis
                // le 2026-11-11).
                if let Some(r) = receiving {
                    segment["sourceId"] = json!(r.source_id.as_ref());
                }
                segment
            })
            .collect();
    }
    if let Some(sources) = &d.sources {
        task["sources"] = sources
            .iter()
            .map(|s| {
                // Miroirs (SPEC §7.7, phase 6) : rang, état, débit, échecs et raison.
                json!({
                    "sourceId": s.id,
                    "url": s.url,
                    "acceptsRanges": s.accepts_ranges,
                    "rank": s.rank,
                    "state": s.state.as_sql(),
                    "avgBps": s.avg_bps,
                    "failCount": s.fail_count,
                    "lastError": s.last_error,
                    // Morceaux faux envoyés (F-12, depuis le 2026-11-17).
                    "badPieces": s.bad_pieces,
                })
            })
            .collect();
    }
    // Empreintes de morceaux d'un Metalink (F-12, SPEC §7.9, depuis le 2026-11-17) :
    // nombre, longueur, algorithme, morceaux retéléchargés et tours depuis la relance.
    if let Some(p) = &d.pieces {
        task["pieces"] = json!({
            "count": p.pieces.count(),
            "length": p.pieces.length(),
            "algo": p.pieces.algo().as_sql(),
            "redone": p.redone,
            "rounds": p.rounds,
        });
    }
    // Fichiers d'un torrent (SPEC §7.16) ; `doneBytes` n'est qu'une indication.
    if let Some(files) = &d.files {
        task["files"] = files
            .iter()
            .map(|f| {
                json!({
                    "index": f.index,
                    "path": f.path,
                    "length": f.length,
                    "selected": f.selected,
                    "doneBytes": f.done_bytes,
                })
            })
            .collect();
    }
    task
}

/// `history.list` (F-35) : entrées les plus récentes d'abord, filtrées par `query` (nom,
/// chemin ou hôte) et `before` ; `limit` et `cursor` pour lire page par page.
async fn history_list(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    let cursor = p
        .str("cursor")?
        .map(|c| Cursor::parse(&c).ok_or_else(|| RpcError::invalid_params("cursor invalide")))
        .transpose()?;
    let limit = p.u32("limit")?;
    if limit == Some(0) {
        return Err(RpcError::invalid_params("limit : au moins 1"));
    }
    let filter = HistoryFilter {
        query: p.str("query")?,
        before: p.i64("before")?,
        cursor,
        limit,
    };
    let rows = ctx.engine.history(filter).await?;
    let next = match (limit, rows.last()) {
        (Some(l), Some(last)) if rows.len() == l as usize => Some(
            Cursor {
                finished_at: last.finished_at,
                id: last.id.clone(),
            }
            .text(),
        ),
        _ => None,
    };
    Ok(json!({
        "items": rows.iter().map(history_json).collect::<Vec<_>>(),
        "nextCursor": next,
    }))
}

fn history_json(h: &HistoryRow) -> Value {
    json!({
        "historyId": h.id,
        "taskId": h.task_id,
        "displayName": h.display_name,
        "finalPath": h.final_path,
        "totalBytes": h.total_bytes,
        "sourceHost": h.source_host,
        "finalState": h.final_state,
        "sha256": h.sha256,
        "finishedAt": h.finished_at,
    })
}

async fn task_list(ctx: &Ctx, p: &Params) -> Result<Value, RpcError> {
    let states = p
        .strings("states")?
        .unwrap_or_default()
        .iter()
        .map(|s| {
            TaskState::from_sql(s)
                .ok_or_else(|| RpcError::invalid_params(format!("état inconnu : {s}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let limit = p.u32("limit")?;
    // Comme `history.list` : une page vide n'a pas de sens (audit R17).
    if limit == Some(0) {
        return Err(RpcError::invalid_params("limit : au moins 1"));
    }
    let filter = TaskFilter {
        queue_id: p.str("queueId")?,
        states,
        after: p.str("cursor")?,
        limit,
    };
    let (tasks, rev) = ctx.engine.list(filter).await?;
    let next = match (limit, tasks.last()) {
        (Some(l), Some(last)) if tasks.len() == l as usize => Some(last.id.clone()),
        _ => None,
    };
    Ok(json!({
        "items": tasks.iter().map(task_json).collect::<Vec<_>>(),
        "nextCursor": next,
        "rev": rev,
    }))
}

#[derive(Clone, Copy)]
enum Action {
    Pause,
    Resume,
    Cancel { delete_files: bool },
    Remove { delete_files: bool },
    Restart { delete_files: bool },
}

/// Applique `action` à chaque tâche de `taskIds`. Les tâches refusées n'empêchent pas
/// les autres : elles figurent dans `errors`, avec leur erreur.
async fn for_each(ctx: &Ctx, p: &Params, action: Action) -> Result<Value, RpcError> {
    let (mut done, mut errors) = (Vec::new(), Vec::new());
    for id in p.task_ids()? {
        let result: Result<(), EngineError> = match action {
            Action::Pause => ctx.engine.pause(&id).await,
            Action::Resume => ctx.engine.resume(&id).await,
            Action::Cancel { delete_files } => ctx.engine.cancel(&id, delete_files).await,
            Action::Remove { delete_files } => ctx.engine.remove(&id, delete_files).await,
            Action::Restart { delete_files } => ctx.engine.restart(&id, delete_files).await,
        };
        match result {
            Ok(()) => done.push(id),
            Err(e) => {
                let e = RpcError::from(e);
                errors.push(json!({
                    "taskId": id,
                    "code": e.data_code,
                    "message": e.message,
                }));
            }
        }
    }
    Ok(json!({"taskIds": done, "errors": errors}))
}

async fn queue_by_id(ctx: &Ctx, queue_id: &str) -> Result<Value, RpcError> {
    let queues = ctx.engine.queues().await?;
    queues
        .iter()
        .find(|q| q.id == queue_id)
        .map(queue_json)
        .ok_or_else(|| RpcError::internal("file créée introuvable"))
}

fn subscribe(ctx: &mut Ctx, p: &Params) -> Result<Value, RpcError> {
    let names = p
        .strings("topics")?
        .ok_or_else(|| RpcError::invalid_params("topics manquant"))?;
    let mut topics = Topics::default();
    for name in &names {
        match name.as_str() {
            "tasks" => topics.tasks = true,
            "queues" => topics.queues = true,
            "progress" => topics.progress = true,
            other => {
                return Err(RpcError::invalid_params(format!(
                    "sujet inconnu : {other} (tasks, queues, progress)"
                )));
            }
        }
    }
    let interval = p
        .u32("progressIntervalMs")?
        .map_or(DEFAULT_PROGRESS_INTERVAL, |ms| {
            Duration::from_millis(u64::from(ms.max(50)))
        });
    ctx.start_events(topics, interval);
    Ok(json!({
        "topics": names,
        "progressIntervalMs": interval.as_millis() as u64,
        "rev": ctx.engine.rev(),
    }))
}
