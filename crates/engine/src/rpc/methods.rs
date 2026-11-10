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
use crate::schedule::Rules;
use crate::scheduler::{
    AddTask, EngineError, Include, LoginChange, LoginView, MediaRequest, QueueChange, TaskDetails,
    TaskUpdate,
};
use crate::store::history::{Cursor, HistoryFilter, HistoryRow};
use crate::store::settings::{self, DOWNLOAD_DIR, Flag, TORRENT_INTERFACE, TORRENT_RATIO};
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
    if let Some(need) = scope::required(method, &params)
        && !ctx.scope.allows(need)
    {
        return Err(RpcError::unauthorized(format!(
            "{method} demande la portée {}, cette connexion a la portée {}",
            need.as_str(),
            ctx.scope.as_str()
        )));
    }
    let p = Params::new(params)?;
    match method {
        "engine.hello" => hello(ctx, &p),
        "engine.shutdown" => {
            ctx.shutdown_after = true;
            Ok(json!({}))
        }
        "task.add" => task_add(ctx, &p).await,
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
            // Vérifiée avant : une planification inconnue ne laisse pas de file créée.
            if let Some(id) = &schedule_id
                && !ctx
                    .engine
                    .schedules()
                    .await?
                    .iter()
                    .any(|v| &v.schedule.id == id)
            {
                return Err(EngineError::ScheduleNotFound(id.clone()).into());
            }
            let id = ctx.engine.create_queue(&name, max_active, limit).await?;
            if let Some(schedule_id) = schedule_id {
                let change = QueueChange {
                    schedule_id: Some(Some(schedule_id)),
                    ..QueueChange::default()
                };
                ctx.engine.update_queue(&id, change).await?;
            }
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
            };
            Ok(login_json(&ctx.engine.set_login(change).await?))
        }
        "login.delete" => {
            ctx.engine.delete_login(&p.required_str("loginId")?).await?;
            Ok(json!({}))
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
                    && Flag::from_key(k).is_none()
            }) {
                let known: Vec<_> = Flag::ALL.iter().map(|f| f.key()).collect();
                return Err(RpcError::invalid_params(format!(
                    "réglage inconnu : {key} (pris en charge : {DOWNLOAD_DIR}, \
                     {TORRENT_RATIO}, {TORRENT_INTERFACE}, {})",
                    known.join(", ")
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
            // Tous vérifiés avant d'en changer un.
            let mut flags = Vec::new();
            for flag in Flag::ALL {
                match values.get(flag.key()) {
                    None => {}
                    Some(Value::Bool(on)) => flags.push((flag, *on)),
                    Some(_) => return Err(Params::wrong(flag.key(), "booléen")),
                }
            }
            if let Some(value) = values.get(DOWNLOAD_DIR)
                && !matches!(value, Value::Null | Value::String(_))
            {
                return Err(Params::wrong(DOWNLOAD_DIR, "texte ou null"));
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
            Ok(json!({
                "rev": s.rev,
                "tasks": s.tasks.iter().map(task_json).collect::<Vec<_>>(),
                "queues": s.queues.iter().map(queue_json).collect::<Vec<_>>(),
                "limits": {"downloadBps": s.download_limit, "uploadBps": s.upload_limit},
                "settings": settings_json(
                    s.download_dir,
                    &s.flags,
                    s.torrent_ratio,
                    s.torrent_interface,
                ),
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
        // Vidéo (SPEC §7.17) : Windows et Linux seulement.
        "capabilities": if crate::media::SUPPORTED {
            json!(["torrent", "media"])
        } else {
            json!(["torrent"])
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
    Ok(json!({"values": settings_json(dir, &flags, ratio, interface)}))
}

/// Valeurs des réglages, telles que `settings.get` et `sync.snapshot` les donnent.
fn settings_json(
    download_dir: Option<String>,
    flags: &[(Flag, bool)],
    ratio: Value,
    interface: Option<String>,
) -> Value {
    let mut values = json!({
        DOWNLOAD_DIR: download_dir,
        TORRENT_RATIO: ratio,
        TORRENT_INTERFACE: interface,
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
fn login_json(l: &LoginView) -> Value {
    json!({"loginId": l.id, "site": l.site, "username": l.username})
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
            torrent,
            media,
            mirrors,
        })
        .await?;
    Ok(json!({"taskId": task_id}))
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
    let details = ctx
        .engine
        .details(&p.required_str("taskId")?, include)
        .await?;
    Ok(details_json(&details))
}

fn details_json(d: &TaskDetails) -> Value {
    let mut task = task_json(&d.task);
    if let Some(segments) = &d.segments {
        task["segments"] = segments
            .iter()
            .map(|(s, receiving)| {
                json!({
                    "idx": s.idx,
                    "start": s.start,
                    "end": s.end,
                    // Vidés sur disque, ou reçus par la connexion en cours, le plus grand.
                    "doneBytes": receiving.map_or(s.written, |r| r.max(s.written)),
                    "receiving": receiving.is_some(),
                })
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
                })
            })
            .collect();
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
