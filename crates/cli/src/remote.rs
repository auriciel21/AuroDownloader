//! Commandes qui passent par le moteur (`aurod`) et son protocole (SPEC §6).

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::display::{self, Limit, Line, Status, size};
use auro_cli::client::{EngineRefused, Rpc, Target};
use auro_engine::io::checksum::Checksum;
use auro_engine::io::{BoundedReadError, read_bounded};

/// Dossier de destination en chemin absolu, qui doit exister.
pub(crate) fn dest_dir(dir: &Path) -> Result<String> {
    let dir = std::path::absolute(dir)?;
    if !dir.is_dir() {
        bail!("le dossier {} n'existe pas", dir.display());
    }
    Ok(dir
        .to_str()
        .context("le chemin du dossier n'est pas en UTF-8")?
        .to_owned())
}

/// `destDir` d'un ajout : le dossier donné ; aucun avec `--auto`, et le moteur prend son
/// dossier des téléchargements, où les règles de rangement s'appliquent (SPEC §7.21).
fn set_dest(params: &mut Value, dir: Option<&Path>) -> Result<()> {
    if let Some(dir) = dir {
        params["destDir"] = json!(dest_dir(dir)?);
    }
    Ok(())
}

/// Une file, par son nom ou son identifiant ; sans `queue`, la file principale.
async fn find_queue(rpc: &mut Rpc, queue: Option<&str>) -> Result<Value> {
    let list = rpc.call("queue.list", json!({})).await?;
    let items = list["items"].as_array().cloned().unwrap_or_default();
    let found = match queue {
        Some(q) => items
            .into_iter()
            .find(|i| i["name"] == q || i["queueId"] == q),
        None => {
            let main = items.iter().find(|i| i["name"] == "default").cloned();
            main.or_else(|| items.into_iter().next())
        }
    };
    found.with_context(|| format!("file inconnue : {}", queue.unwrap_or("principale")))
}

/// Identifiant d'une file donnée par son nom ou son identifiant.
async fn queue_id(rpc: &mut Rpc, queue: &str) -> Result<String> {
    find_queue(rpc, Some(queue)).await?["queueId"]
        .as_str()
        .map(str::to_owned)
        .context("file sans identifiant")
}

pub struct AddOptions<'a> {
    /// Dossier de destination ; `None` (`--auto`) : celui des téléchargements du moteur,
    /// où les règles de rangement s'appliquent (SPEC §7.21).
    pub dir: Option<&'a Path>,
    pub connections: Option<u32>,
    pub queue: Option<&'a str>,
    /// Limite de débit de la tâche, en octets par seconde.
    pub limit: Option<u64>,
    pub paused: bool,
    /// En-têtes HTTP de la tâche.
    pub headers: &'a [(String, String)],
    /// Empreinte attendue du fichier (F-11).
    pub checksum: Option<&'a Checksum>,
    /// Miroirs du même fichier (SPEC §7.7).
    pub mirrors: &'a [String],
    /// Fichiers d'un Metalink à ajouter (rangs de `auro metalink`) ; tous sinon.
    pub select: Option<&'a [u32]>,
    /// L'adresse telle quelle, même si un plugin prend son site en charge (SPEC §7.20).
    pub no_plugin: bool,
}

/// Taille maximale d'un fichier Metalink lu par la CLI : celle du moteur (SPEC §7.7,
/// audit C29).
const MAX_METALINK_FILE: u64 = auro_engine::metalink::MAX_LEN as u64;

/// Fichier Metalink désigné par un argument (F-27, SPEC §7.7) : un fichier local `.meta4` ou
/// `.metalink` (envoyé en base64), ou une adresse HTTP(S) dont le chemin finit ainsi (lue
/// par le moteur). `None` pour tout autre argument.
fn metalink_of(arg: &str) -> Result<Option<Value>> {
    let named = |text: &str| {
        let lower = text.to_ascii_lowercase();
        lower.ends_with(".meta4") || lower.ends_with(".metalink")
    };
    if let Some((scheme, _)) = arg.split_once("://") {
        let web = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
        let path = arg.split(['?', '#']).next().unwrap_or(arg);
        return Ok((web && named(path)).then(|| json!({"url": arg})));
    }
    let path = Path::new(arg);
    if !named(arg) || !path.is_file() {
        return Ok(None);
    }
    use base64::Engine as _;
    let bytes = read_chosen(path, arg, MAX_METALINK_FILE, "fichier Metalink")?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(Some(json!({"data": encoded})))
}

/// Fichier local nommé par l'argument `arg`, `limit` octets au plus (`io::read_bounded`,
/// audit C31) ; `what` le nomme dans le refus.
fn read_chosen(path: &Path, arg: &str, limit: u64, what: &str) -> Result<Vec<u8>> {
    match read_bounded(path, limit) {
        Ok(bytes) => Ok(bytes),
        Err(BoundedReadError::TooLarge { len, .. }) => {
            bail!("{arg} : {what} trop gros ({})", size(len))
        }
        Err(BoundedReadError::Io(e)) => Err(e).with_context(|| format!("lecture de {arg}")),
    }
}

/// Fichiers d'un Metalink ajoutés : identifiant et chemin de chaque tâche, et nombre de
/// fichiers refusés (déjà signalés un par un).
struct MetalinkAdded {
    tasks: Vec<(String, String)>,
    refused: usize,
}

/// Erreur de fin d'une commande qui a ajouté le reste d'un Metalink (code de sortie 1,
/// audit C12).
fn refused_files(refused: usize) -> anyhow::Error {
    anyhow::anyhow!("{refused} fichier(s) du Metalink refusé(s) ; les autres sont ajoutés")
}

/// Ajoute les fichiers d'un Metalink (`metalink.add`) : une tâche par fichier. Un fichier
/// refusé est signalé, les autres restent.
async fn add_metalink(
    rpc: &mut Rpc,
    mut params: Value,
    opts: &AddOptions<'_>,
) -> Result<MetalinkAdded> {
    if opts.checksum.is_some() || !opts.mirrors.is_empty() || !opts.headers.is_empty() {
        bail!(
            "--checksum, --mirror et --header ne valent pas pour un Metalink : il donne lui-même \
             les adresses et l'empreinte de chaque fichier"
        );
    }
    set_dest(&mut params, opts.dir)?;
    params["startPaused"] = json!(opts.paused);
    if let Some(select) = opts.select {
        params["select"] = json!(select);
    }
    if let Some(c) = opts.connections {
        params["maxConnections"] = json!(c);
    }
    if let Some(l) = opts.limit {
        params["speedLimitBps"] = json!(l);
    }
    if let Some(queue) = opts.queue {
        params["queueId"] = json!(queue_id(rpc, queue).await?);
    }
    let added = rpc.call("metalink.add", params).await?;
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    let errors = added["errors"].as_array().cloned().unwrap_or_default();
    for e in &errors {
        eprintln!(
            "refusé : {}  {} [{}]",
            text(&e["path"]),
            text(&e["message"]),
            text(&e["code"])
        );
    }
    let tasks: Vec<(String, String)> = added["tasks"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|t| (text(&t["taskId"]), text(&t["path"])))
                .collect()
        })
        .unwrap_or_default();
    if tasks.is_empty() {
        bail!("aucun fichier du Metalink n'a été ajouté");
    }
    Ok(MetalinkAdded {
        tasks,
        refused: errors.len(),
    })
}

/// Contenu d'un Metalink (`auro metalink`) : rang, chemin, taille, empreinte et sources de
/// chaque fichier, puis ceux écartés et pourquoi.
pub async fn metalink(rpc: &mut Rpc, arg: &str) -> Result<()> {
    let params = match metalink_of(arg)? {
        Some(params) => params,
        // Une adresse sans extension connue : le moteur dira si c'est un Metalink.
        None if arg.contains("://") => json!({"url": arg}),
        None => bail!("{arg} : ni fichier .meta4 ou .metalink, ni adresse"),
    };
    let doc = rpc.call("metalink.read", params).await?;
    println!("Metalink {}", doc["format"].as_str().unwrap_or("?"));
    for f in doc["files"].as_array().into_iter().flatten() {
        let length = f["size"]
            .as_u64()
            .map_or_else(|| "taille inconnue".to_owned(), size);
        let sum = f["checksum"]["algo"].as_str().unwrap_or("aucune empreinte");
        let urls = f["urls"].as_array().map_or(0, Vec::len);
        let dropped = f["droppedUrls"].as_u64().unwrap_or(0);
        let sources = match (urls, dropped) {
            (1, 0) => "1 source".to_owned(),
            (n, 0) => format!("{n} sources"),
            (n, d) => format!("{n} sources ({d} laissées de côté)"),
        };
        println!(
            "{:>3}  {}  {length}, {sum}, {sources}",
            f["index"],
            f["path"].as_str().unwrap_or_default()
        );
    }
    for i in doc["ignored"].as_array().into_iter().flatten() {
        println!(
            "écarté : {}  {}",
            i["name"].as_str().unwrap_or_default(),
            i["reason"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

/// Taille maximale d'un fichier `.torrent` lu par la CLI : celle du moteur (audit C29).
const MAX_TORRENT_FILE: u64 = auro_engine::torrent::MAX_TORRENT_FILE as u64;

/// Source d'un ajout : un fichier `.torrent` local (envoyé en base64, SPEC §6.5), ou une
/// adresse (HTTP, HTTPS ou lien magnet).
fn source_of(arg: &str) -> Result<Value> {
    let path = Path::new(arg);
    let is_torrent = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("torrent"));
    if is_torrent && !arg.contains("://") && path.is_file() {
        use base64::Engine as _;
        let bytes = read_chosen(path, arg, MAX_TORRENT_FILE, "fichier .torrent")?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        return Ok(json!({"torrent": encoded}));
    }
    Ok(json!({"url": arg}))
}

async fn add_one(rpc: &mut Rpc, url: &str, opts: &AddOptions<'_>) -> Result<String> {
    let mut params = json!({
        "sources": [source_of(url)?],
        "startPaused": opts.paused,
    });
    set_dest(&mut params, opts.dir)?;
    if !opts.mirrors.is_empty() {
        let sources = params["sources"].as_array_mut().expect("liste");
        sources.extend(opts.mirrors.iter().map(|m| json!({"url": m})));
    }
    if let Some(c) = opts.connections {
        params["maxConnections"] = json!(c);
    }
    if let Some(l) = opts.limit {
        params["speedLimitBps"] = json!(l);
    }
    if let Some(c) = opts.checksum {
        params["checksum"] = checksum_json(c);
    }
    if opts.no_plugin {
        params["plugin"] = json!(false);
    }
    if let Some(queue) = opts.queue {
        params["queueId"] = json!(queue_id(rpc, queue).await?);
    }
    if !opts.headers.is_empty() {
        let headers: serde_json::Map<String, Value> = opts
            .headers
            .iter()
            .map(|(n, v)| (n.clone(), json!(v)))
            .collect();
        params["headers"] = Value::Object(headers);
    }
    add_task(rpc, params).await
}

/// `task.add` ; l'identifiant de la tâche ajoutée.
async fn add_task(rpc: &mut Rpc, params: Value) -> Result<String> {
    let added = rpc.call("task.add", params).await.map_err(tools_hint)?;
    added["taskId"]
        .as_str()
        .map(str::to_owned)
        .context("réponse du moteur sans taskId")
}

/// Un refus `TOOLS_MISSING` (vidéo, SPEC §7.17) dit comment installer les outils.
fn tools_hint(e: anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<EngineRefused>() {
        Some(r) if r.code == "TOOLS_MISSING" => {
            e.context("outils de la vidéo absents : auro video tools --install")
        }
        Some(r) if r.code == "PLUGIN_CHOICE" => e.context(
            "plusieurs liens sur cette page : auro plugin resolve <page>, puis auro plugin add \
             <page> --select <rangs>",
        ),
        _ => e,
    }
}

pub async fn add(rpc: &mut Rpc, urls: &[String], opts: &AddOptions<'_>) -> Result<()> {
    let mut refused = 0;
    for url in urls {
        // Un Metalink : une tâche par fichier (SPEC §7.7).
        if let Some(source) = metalink_of(url)? {
            let added = add_metalink(rpc, source, opts).await?;
            for (id, path) in added.tasks {
                println!("{id}  {path}");
            }
            refused += added.refused;
            continue;
        }
        if opts.select.is_some() {
            bail!(
                "--select : seulement pour un fichier Metalink ({})",
                shown(url)
            );
        }
        let id = add_one(rpc, url, opts).await?;
        println!("{id}  {}", shown(url));
    }
    if refused > 0 {
        return Err(refused_files(refused));
    }
    Ok(())
}

/// Une vidéo à ajouter (`auro video add`, `auro video get` ; SPEC §7.17).
pub struct VideoOptions<'a> {
    /// `None` (`--auto`) : le dossier des téléchargements du moteur, rangé par les règles.
    pub dir: Option<&'a Path>,
    /// Sélecteur de formats de yt-dlp ; celui du moteur par défaut.
    pub format: Option<&'a str>,
    pub container: Option<&'a str>,
    /// Langues des sous-titres.
    pub subtitles: &'a [String],
    pub queue: Option<&'a str>,
    pub limit: Option<u64>,
    pub paused: bool,
}

async fn add_video(rpc: &mut Rpc, page: &str, opts: &VideoOptions<'_>) -> Result<String> {
    let mut media = json!({"pageUrl": page});
    if let Some(f) = opts.format {
        media["format"] = json!(f);
    }
    if let Some(c) = opts.container {
        media["container"] = json!(c);
    }
    if !opts.subtitles.is_empty() {
        media["subtitles"] = json!(opts.subtitles);
    }
    let mut params = json!({
        "media": media,
        "startPaused": opts.paused,
    });
    set_dest(&mut params, opts.dir)?;
    if let Some(l) = opts.limit {
        params["speedLimitBps"] = json!(l);
    }
    if let Some(queue) = opts.queue {
        params["queueId"] = json!(queue_id(rpc, queue).await?);
    }
    add_task(rpc, params).await
}

pub async fn video_add(rpc: &mut Rpc, pages: &[String], opts: &VideoOptions<'_>) -> Result<()> {
    for page in pages {
        let id = add_video(rpc, page, opts).await?;
        println!("{id}  {}", shown(page));
    }
    Ok(())
}

/// Ce que `auro get` ou `auro video get` ajoute avant de le suivre.
pub enum NewTask<'a> {
    File(&'a str, &'a AddOptions<'a>),
    Video(&'a str, &'a VideoOptions<'a>),
}

/// Ajoute une tâche et la suit jusqu'à la fin ; Ctrl-C la met en pause.
pub async fn get(target: &Target, new: NewTask<'_>) -> Result<()> {
    let mut rpc = auro_cli::client::connect(target).await?;
    // Abonné avant l'ajout : aucun événement de la tâche ne peut manquer. Seule la tâche
    // ajoutée est suivie.
    let mut follow = Follow::start(&mut rpc, Some(HashSet::new())).await?;
    let mut refused = 0;
    let ids = match new {
        // Un Metalink : ses fichiers, suivis ensemble (SPEC §7.7).
        NewTask::File(url, opts) => match metalink_of(url)? {
            Some(source) => {
                let added = add_metalink(&mut rpc, source, opts).await?;
                refused = added.refused;
                added
                    .tasks
                    .into_iter()
                    .map(|(id, path)| {
                        println!("Tâche {id}  {path}");
                        id
                    })
                    .collect()
            }
            None if opts.select.is_some() => {
                bail!(
                    "--select : seulement pour un fichier Metalink ({})",
                    shown(url)
                )
            }
            None => {
                let id = add_one(&mut rpc, url, opts).await?;
                println!("Tâche {id}");
                vec![id]
            }
        },
        NewTask::Video(page, opts) => {
            let id = add_video(&mut rpc, page, opts).await?;
            println!("Tâche {id}");
            vec![id]
        }
    };
    follow.targets.extend(ids.iter().cloned());
    follow.refresh(&mut rpc).await?;
    follow.run(&mut rpc, true).await?;
    // Le bilan de chaque tâche ; la première erreur après tous les bilans.
    let mut first_error = None;
    for id in &ids {
        let task = rpc.call("task.get", json!({"taskId": id})).await?;
        if let Err(e) = report(&task) {
            if ids.len() > 1 {
                eprintln!("{id} : {e:#}");
            }
            first_error.get_or_insert(e);
        }
    }
    // Codes de sortie (audit C12) : une tâche en échec, puis des fichiers refusés (1) ;
    // sinon un Ctrl-C (130).
    if let Some(e) = first_error {
        return Err(e);
    }
    if refused > 0 {
        return Err(refused_files(refused));
    }
    if follow.interrupted {
        return Err(Interrupted.into());
    }
    Ok(())
}

/// Bilan d'une tâche suivie jusqu'à son arrêt.
fn report(task: &Value) -> Result<()> {
    let id = task["taskId"].as_str().unwrap_or_default();
    let detail = |t: &Value| {
        let code = t["errorCode"].as_str().unwrap_or("?");
        let text = t["errorDetail"].as_str().unwrap_or("");
        format!("{text} [{code}]")
    };
    let dir = Path::new(task["destDir"].as_str().unwrap_or_default());
    let name = task["fileName"].as_str().unwrap_or_default();
    match task["state"].as_str().unwrap_or_default() {
        "completed" => {
            println!("Terminé : {}", dir.join(name).display());
            Ok(())
        }
        // Torrent reçu en entier, qui partage jusqu'au ratio (SPEC §7.16).
        "seeding" => {
            println!(
                "Terminé : {}\nPartage en cours jusqu'au ratio réglé (auro config \
                 torrent.ratioLimit) ; pour l'arrêter : auro pause {id}",
                dir.join(name).display()
            );
            Ok(())
        }
        "paused" => match task["pauseReason"].as_str() {
            Some("user") => {
                println!("En pause. Pour reprendre : auro resume {id}");
                Ok(())
            }
            Some("file_changed") => bail!("le fichier distant a changé : tâche en pause"),
            Some("disk_full") => bail!(
                "tâche en pause, faute de place ; elle repartira d'elle-même quand la place \
                 reviendra : {}",
                detail(task)
            ),
            other => bail!("tâche en pause ({})", other.unwrap_or("?")),
        },
        // Empreintes de morceaux d'un Metalink (F-12) : rien à corriger à la main ; une
        // relance redonne trois tours aux seuls morceaux faux.
        "failed" if task["errorCode"] == "CHECKSUM_MISMATCH" && task.get("pieces").is_some() => {
            bail!(
                "le fichier reçu ne correspond pas aux empreintes du Metalink ; il reste sous \
                 son nom provisoire (.part). Pour réessayer les morceaux faux : auro resume \
                 {id} ; fichier à retélécharger : auro restart {id} --delete-files. {}",
                detail(task)
            )
        }
        "failed" if task["errorCode"] == "CHECKSUM_MISMATCH" => bail!(
            "le fichier reçu n'a pas l'empreinte attendue ; il reste sous son nom provisoire \
             (.part). Empreinte à corriger : auro checksum {id} … puis auro resume {id} ; \
             fichier à retélécharger : auro restart {id} --delete-files. {}",
            detail(task)
        ),
        // SFTP (F-08) : la clé du serveur attend la décision de l'utilisateur.
        "failed" if task["errorCode"] == "HOST_KEY_UNKNOWN" => bail!(
            "{} ; empreinte à vérifier auprès de l'administrateur du serveur, puis : auro \
             hostkey accept <serveur> <empreinte> (la tâche repartira seule) ; voir auro \
             hostkey list",
            detail(task)
        ),
        "failed" if task["errorCode"] == "HOST_KEY_CHANGED" => bail!(
            "{} ; si l'administrateur du serveur confirme ce changement : auro hostkey accept \
             <serveur> <empreinte> (voir auro hostkey list)",
            detail(task)
        ),
        "failed" if task["errorCode"] == "TOOLS_MISSING" => {
            bail!("outils de la vidéo absents : auro video tools --install, puis auro resume {id}")
        }
        "failed" => bail!("échec : {}", detail(task)),
        "cancelled" => bail!("tâche annulée"),
        other => bail!("la tâche s'est arrêtée dans l'état {other}"),
    }
}

pub async fn list(rpc: &mut Rpc, states: &[String], queue: Option<&str>) -> Result<()> {
    let mut params = json!({});
    if !states.is_empty() {
        params["states"] = json!(states);
    }
    if let Some(queue) = queue {
        params["queueId"] = json!(queue_id(rpc, queue).await?);
    }
    let list = rpc.call("task.list", params).await?;
    for t in list["items"].as_array().into_iter().flatten() {
        let done = t["doneBytes"].as_u64().unwrap_or(0);
        let amount = match t["totalBytes"].as_u64() {
            Some(total) => format!("{} / {}", size(done), size(total)),
            None => size(done),
        };
        let note = t["pauseReason"]
            .as_str()
            .or(t["errorCode"].as_str())
            .map(|n| format!(" ({n})"))
            .unwrap_or_default();
        println!(
            "{}  {:<26} {:>22}  {}{}",
            t["taskId"].as_str().unwrap_or_default(),
            format!("{}{note}", t["state"].as_str().unwrap_or_default()),
            amount,
            kind_tag(t),
            t["displayName"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

/// Genre d'une tâche devant son nom : rien pour un fichier HTTP.
fn kind_tag(task: &Value) -> &'static str {
    match task["kind"].as_str() {
        Some("torrent") => "[torrent] ",
        Some("media") => "[vidéo] ",
        _ => "",
    }
}

/// `task.pause`, `task.resume`, `task.cancel`, `task.remove` ou `task.restart` sur plusieurs tâches ;
/// échoue si l'une au moins est refusée, après avoir appliqué les autres.
pub async fn act(rpc: &mut Rpc, method: &str, ids: &[String], extra: Value) -> Result<()> {
    let mut params = json!({"taskIds": ids});
    if let (Some(p), Some(e)) = (params.as_object_mut(), extra.as_object()) {
        p.extend(e.clone());
    }
    let result = rpc.call(method, params).await?;
    for id in result["taskIds"].as_array().into_iter().flatten() {
        println!("{}  ok", id.as_str().unwrap_or_default());
    }
    let errors = result["errors"].as_array().cloned().unwrap_or_default();
    for e in &errors {
        eprintln!(
            "{}  {} [{}]",
            e["taskId"].as_str().unwrap_or_default(),
            e["message"].as_str().unwrap_or_default(),
            e["code"].as_str().unwrap_or_default()
        );
    }
    if !errors.is_empty() {
        bail!("{} tâche(s) refusée(s)", errors.len());
    }
    Ok(())
}

/// Suit des tâches (toutes celles en cours si `ids` est vide) jusqu'à leur arrêt ;
/// Ctrl-C arrête de suivre sans toucher aux tâches.
pub async fn watch(rpc: &mut Rpc, ids: &[String]) -> Result<()> {
    let only = (!ids.is_empty()).then(|| ids.iter().cloned().collect());
    let mut follow = Follow::start(rpc, only).await?;
    // Un identifiant inconnu ne s'arrêterait jamais (audit C9).
    let unknown: Vec<&str> = ids
        .iter()
        .filter(|id| !follow.tasks.contains_key(*id))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        bail!("tâche inconnue : {}", unknown.join(", "));
    }
    if follow.targets.is_empty() {
        println!("Aucune tâche en cours.");
        return Ok(());
    }
    follow.run(rpc, false).await?;
    if follow.interrupted {
        return Err(Interrupted.into());
    }
    Ok(())
}

/// Une file, sur une ligne.
fn queue_line(q: &Value) -> String {
    format!(
        "{}  {:<20} {} tâche(s) à la fois, limite {}",
        q["queueId"].as_str().unwrap_or_default(),
        q["name"].as_str().unwrap_or_default(),
        q["maxActive"],
        display::limit(q["speedLimitBps"].as_u64())
    )
}

pub async fn queues(rpc: &mut Rpc) -> Result<()> {
    let list = rpc.call("queue.list", json!({})).await?;
    for q in list["items"].as_array().into_iter().flatten() {
        println!("{}", queue_line(q));
    }
    Ok(())
}

pub async fn queue_create(
    rpc: &mut Rpc,
    name: &str,
    max_active: Option<u32>,
    limit: Option<u64>,
) -> Result<()> {
    let mut params = json!({"name": name});
    if let Some(m) = max_active {
        params["maxActive"] = json!(m);
    }
    if let Some(l) = limit {
        params["speedLimitBps"] = json!(l);
    }
    let q = rpc.call("queue.create", params).await?;
    println!("{}", queue_line(&q));
    Ok(())
}

/// Modification d'une file ; `None` : inchangé.
pub struct QueueEdit<'a> {
    pub name: Option<&'a str>,
    pub max_active: Option<u32>,
    pub limit: Option<Limit>,
}

pub async fn queue_update(rpc: &mut Rpc, queue: &str, edit: &QueueEdit<'_>) -> Result<()> {
    let mut params = json!({"queueId": queue_id(rpc, queue).await?});
    if let Some(n) = edit.name {
        params["name"] = json!(n);
    }
    if let Some(m) = edit.max_active {
        params["maxActive"] = json!(m);
    }
    if let Some(Limit(l)) = edit.limit {
        params["speedLimitBps"] = json!(l);
    }
    let q = rpc.call("queue.update", params).await?;
    println!("{}", queue_line(&q));
    Ok(())
}

/// Affiche le dossier des téléchargements du moteur ; avec `path`, le change d'abord ;
/// avec `reset`, revient à celui du système.
pub async fn download_dir(rpc: &mut Rpc, path: Option<&Path>, reset: bool) -> Result<()> {
    let settings = match (path, reset) {
        (Some(p), _) => {
            let values = json!({"downloads.dir": dest_dir(p)?});
            rpc.call("settings.set", json!({"values": values})).await?
        }
        (None, true) => {
            let values = json!({"downloads.dir": null});
            rpc.call("settings.set", json!({"values": values})).await?
        }
        (None, false) => rpc.call("settings.get", json!({})).await?,
    };
    match settings["values"]["downloads.dir"].as_str() {
        Some(dir) => println!("Dossier des téléchargements : {dir}"),
        None => println!("Aucun dossier des téléchargements : préciser -o à chaque ajout."),
    }
    Ok(())
}

/// Ce dont `auro limit` lit ou change la limite.
pub enum LimitOf<'a> {
    Global,
    /// Limite d'envoi globale des torrents (SPEC §7.16).
    Upload,
    Task(&'a str),
    Queue(&'a str),
}

/// Affiche une limite de débit ; avec `rate`, la change d'abord (`Limit(None)` la retire).
pub async fn limit(rpc: &mut Rpc, of: LimitOf<'_>, rate: Option<Limit>) -> Result<()> {
    let (what, bps) = match of {
        LimitOf::Global => {
            let limits = match rate {
                Some(Limit(l)) => rpc.call("limits.set", json!({"downloadBps": l})).await?,
                None => rpc.call("limits.get", json!({})).await?,
            };
            ("globale".to_owned(), limits["downloadBps"].as_u64())
        }
        LimitOf::Upload => {
            let limits = match rate {
                Some(Limit(l)) => rpc.call("limits.set", json!({"uploadBps": l})).await?,
                None => rpc.call("limits.get", json!({})).await?,
            };
            (
                "d'envoi des torrents".to_owned(),
                limits["uploadBps"].as_u64(),
            )
        }
        LimitOf::Task(id) => {
            let task = match rate {
                Some(Limit(l)) => {
                    let params = json!({"taskId": id, "speedLimitBps": l});
                    rpc.call("task.update", params).await?
                }
                None => rpc.call("task.get", json!({"taskId": id})).await?,
            };
            let name = task["displayName"].as_str().unwrap_or(id);
            (
                format!("de la tâche {name}"),
                task["speedLimitBps"].as_u64(),
            )
        }
        LimitOf::Queue(queue) => {
            let found = find_queue(rpc, Some(queue)).await?;
            let q = match rate {
                Some(Limit(l)) => {
                    let params = json!({"queueId": found["queueId"], "speedLimitBps": l});
                    rpc.call("queue.update", params).await?
                }
                None => found,
            };
            let name = q["name"].as_str().unwrap_or(queue);
            (format!("de la file {name}"), q["speedLimitBps"].as_u64())
        }
    };
    println!("Limite {what} : {}", display::limit(bps));
    Ok(())
}

const DAY_NAMES: [&str; 7] = ["lun", "mar", "mer", "jeu", "ven", "sam", "dim"];

fn days_text(days: &[Value]) -> String {
    let days: Vec<usize> = days
        .iter()
        .filter_map(Value::as_u64)
        .map(|d| d as usize)
        .collect();
    let all_following = days.windows(2).all(|w| w[1] == w[0] + 1);
    match (days.first(), days.last()) {
        (Some(&a), Some(&b)) if days.len() > 2 && all_following => {
            format!("{}-{}", DAY_NAMES[a - 1], DAY_NAMES[b - 1])
        }
        _ => days
            .iter()
            .map(|&d| DAY_NAMES[d - 1])
            .collect::<Vec<_>>()
            .join(","),
    }
}

fn end_text(action: &str) -> &'static str {
    match action {
        "sleep" => "mise en veille",
        "shutdown" => "extinction",
        _ => "rien",
    }
}

/// Durée jusqu'à `at` (millisecondes Unix), arrondie à la minute.
fn until(at: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let minutes = ((at - now).max(0) + 59_999) / 60_000;
    match (minutes / 1440, minutes % 1440 / 60, minutes % 60) {
        (0, 0, m) => format!("{m} min"),
        (0, h, m) => format!("{h} h {m:02} min"),
        (d, h, _) => format!("{d} j {h} h"),
    }
}

fn schedule_line(s: &Value, queues: &[Value]) -> String {
    let windows: Vec<String> = s["rules"]["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| {
            format!(
                "{} {}-{}",
                days_text(w["days"].as_array().map_or(&[][..], Vec::as_slice)),
                w["start"].as_str().unwrap_or("?"),
                w["end"].as_str().unwrap_or("?"),
            )
        })
        .collect();
    let state = match (s["enabled"].as_bool(), s["open"].as_bool()) {
        (Some(false), _) => "désactivée".to_owned(),
        (_, Some(open)) => {
            let (now, then) = if open {
                ("ouverte", "ferme")
            } else {
                ("fermée", "ouvre")
            };
            match s["nextChangeAt"].as_i64() {
                Some(at) => format!("{now}, {then} dans {}", until(at)),
                None => now.to_owned(),
            }
        }
        _ => "?".to_owned(),
    };
    let used: Vec<&str> = queues
        .iter()
        .filter(|q| q["scheduleId"] == s["scheduleId"])
        .filter_map(|q| q["name"].as_str())
        .collect();
    format!(
        "{}  {}  [{}]  fin : {}  files : {}",
        s["name"].as_str().unwrap_or("?"),
        windows.join(" ; "),
        state,
        end_text(s["rules"]["onDone"].as_str().unwrap_or("none")),
        if used.is_empty() {
            "aucune".to_owned()
        } else {
            used.join(", ")
        }
    )
}

pub async fn schedules(rpc: &mut Rpc) -> Result<()> {
    let list = rpc.call("schedule.list", json!({})).await?;
    let queues = rpc.call("queue.list", json!({})).await?;
    let queues = queues["items"].as_array().cloned().unwrap_or_default();
    let items = list["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        println!("Aucune planification : toutes les files téléchargent à toute heure.");
    }
    for s in &items {
        println!("{}", schedule_line(s, &queues));
    }
    Ok(())
}

/// Une plage d'une planification.
pub struct WindowSpec<'a> {
    pub days: &'a [u8],
    pub from: &'a str,
    pub to: &'a str,
    pub then: &'a str,
}

/// Donne à une file une seule plage, en remplaçant sa planification.
pub async fn schedule_set(rpc: &mut Rpc, queue: Option<&str>, spec: &WindowSpec<'_>) -> Result<()> {
    let q = find_queue(rpc, queue).await?;
    let queue_id = q["queueId"]
        .as_str()
        .context("file sans identifiant")?
        .to_owned();
    let name = match q["name"].as_str() {
        Some("default") | None => "File principale".to_owned(),
        Some(n) => n.to_owned(),
    };
    let mut params = json!({
        "name": name,
        "enabled": true,
        "rules": {
            "windows": [{"days": spec.days, "start": spec.from, "end": spec.to}],
            "onDone": spec.then,
        },
    });
    if let Some(id) = q["scheduleId"].as_str() {
        params["scheduleId"] = json!(id);
    }
    let s = rpc.call("schedule.set", params).await?;
    if q["scheduleId"] != s["scheduleId"] {
        let params = json!({"queueId": queue_id, "scheduleId": s["scheduleId"]});
        rpc.call("queue.update", params).await?;
    }
    let queues = rpc.call("queue.list", json!({})).await?;
    let queues = queues["items"].as_array().cloned().unwrap_or_default();
    println!("{}", schedule_line(&s, &queues));
    Ok(())
}

/// Retire la planification d'une file, et la supprime si plus aucune file ne s'en sert.
pub async fn schedule_clear(rpc: &mut Rpc, queue: Option<&str>) -> Result<()> {
    let q = find_queue(rpc, queue).await?;
    let Some(id) = q["scheduleId"].as_str().map(str::to_owned) else {
        println!("Cette file n'a pas de planification.");
        return Ok(());
    };
    let params = json!({"queueId": q["queueId"], "scheduleId": null});
    rpc.call("queue.update", params).await?;
    let queues = rpc.call("queue.list", json!({})).await?;
    let still_used = queues["items"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|q| q["scheduleId"] == id.as_str());
    if !still_used {
        rpc.call("schedule.delete", json!({"scheduleId": id}))
            .await?;
    }
    println!("Planification retirée : la file télécharge à toute heure.");
    Ok(())
}

pub async fn cancel_end_action(rpc: &mut Rpc) -> Result<()> {
    let r = rpc.call("engine.cancelEndAction", json!({})).await?;
    if r["cancelled"] == true {
        println!("Action de fin annulée.");
    } else {
        println!("Aucune action de fin n'était annoncée.");
    }
    Ok(())
}

pub async fn status(rpc: &mut Rpc, endpoint: &str) -> Result<()> {
    let snapshot = rpc.call("sync.snapshot", json!({})).await?;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for t in snapshot["tasks"].as_array().into_iter().flatten() {
        *counts
            .entry(t["state"].as_str().unwrap_or("?").to_owned())
            .or_default() += 1;
    }
    println!("Moteur actif sur {endpoint} (révision {})", snapshot["rev"]);
    if counts.is_empty() {
        println!("Aucune tâche.");
    }
    for (state, n) in counts {
        println!("  {state:<12} {n}");
    }
    Ok(())
}

/// Suivi arrêté par Ctrl-C : la commande sort avec le code 130 (audit C12), sans autre
/// message que ceux du suivi.
#[derive(Debug)]
pub struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("interrompu")
    }
}

impl std::error::Error for Interrupted {}

/// Une adresse telle qu'affichée : sans nom ni mot de passe (`ftp://moi:secret@…`, audit
/// C11) ; une adresse illisible reste telle quelle, le moteur la refusera.
pub fn shown(url: &str) -> String {
    match auro_engine::net::ftp::split_credentials(url) {
        Ok((clean, Some(_))) => clean,
        _ => url.to_owned(),
    }
}

/// Une tâche suivie.
struct Followed {
    name: String,
    state: String,
    line: Option<Line>,
    /// Octets lus par la vérification de l'empreinte (`verifying`).
    checked: Option<u64>,
    /// `http`, `torrent` ou `media`.
    kind: String,
}

/// Suivi de tâches par les notifications : abonnement, puis instantané, puis
/// notifications de révision plus récente (SPEC §6.10).
struct Follow {
    /// Tâches à suivre ; `None` à la création : toutes celles qui tournent.
    targets: HashSet<String>,
    all: bool,
    rev: u64,
    tasks: BTreeMap<String, Followed>,
    status: Status,
    /// Ctrl-C reçu pendant le suivi.
    interrupted: bool,
}

/// Tâche qui n'a pas fini de recevoir : la fusion d'une vidéo (`post_processing`) en fait
/// partie, le partage d'un torrent reçu (`seeding`) non.
fn running(state: &str) -> bool {
    matches!(
        state,
        "queued" | "active" | "retry_wait" | "verifying" | "post_processing"
    )
}

impl Follow {
    async fn start(rpc: &mut Rpc, only: Option<HashSet<String>>) -> Result<Follow> {
        rpc.call(
            "events.subscribe",
            // `queues` : avertissements du moteur (sous-titres manquants, SPEC §7.17).
            json!({"topics": ["tasks", "progress", "queues"], "progressIntervalMs": 500}),
        )
        .await?;
        let mut follow = Follow {
            all: only.is_none(),
            targets: only.unwrap_or_default(),
            rev: 0,
            tasks: BTreeMap::new(),
            status: Status::new(),
            interrupted: false,
        };
        follow.refresh(rpc).await?;
        Ok(follow)
    }

    /// Repart d'un instantané complet ; une tâche qui n'y est plus a été retirée.
    async fn refresh(&mut self, rpc: &mut Rpc) -> Result<()> {
        let snapshot = rpc.call("sync.snapshot", json!({})).await?;
        self.rev = snapshot["rev"].as_u64().unwrap_or(0);
        let present: HashSet<&str> = snapshot["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t["taskId"].as_str())
            .collect();
        let gone: Vec<String> = self
            .tasks
            .keys()
            .filter(|id| !present.contains(id.as_str()))
            .cloned()
            .collect();
        for id in gone {
            self.forget(&id);
        }
        for t in snapshot["tasks"].as_array().into_iter().flatten() {
            let id = t["taskId"].as_str().unwrap_or_default().to_owned();
            let state = t["state"].as_str().unwrap_or_default().to_owned();
            if self.all && running(&state) {
                self.targets.insert(id.clone());
            }
            let name = t["displayName"].as_str().unwrap_or_default().to_owned();
            let kind = t["kind"].as_str().unwrap_or("http").to_owned();
            let entry = self.tasks.entry(id).or_insert(Followed {
                name: name.clone(),
                state: state.clone(),
                line: None,
                checked: None,
                kind,
            });
            entry.name = name;
            entry.state = state;
        }
        Ok(())
    }

    /// Une tâche retirée : plus suivie, et dit si elle l'était (audit C9).
    fn forget(&mut self, id: &str) {
        let Some(task) = self.tasks.remove(id) else {
            return;
        };
        if self.targets.remove(id) {
            self.render();
            self.status.finish();
            eprintln!("{} : retirée", task.name);
        }
    }

    fn done(&self) -> bool {
        self.targets
            .iter()
            .all(|id| self.tasks.get(id).is_some_and(|t| !running(&t.state)))
    }

    fn render(&mut self) {
        let lines: Vec<String> = self
            .tasks
            .iter()
            .filter(|(id, t)| self.targets.contains(*id) && running(&t.state))
            .map(|(_, t)| {
                let name: String = t.name.chars().take(32).collect();
                match (&t.line, t.state.as_str()) {
                    (_, "queued") => format!("{name:<32}  en file"),
                    (_, "retry_wait") => format!("{name:<32}  nouvelle tentative bientôt"),
                    (_, "post_processing") => format!("{name:<32}  assemblage des flux (ffmpeg)"),
                    (line, "verifying") => {
                        let total = line.as_ref().and_then(|l| l.total).filter(|&n| n > 0);
                        match (t.checked, total) {
                            (Some(read), Some(total)) => format!(
                                "{name:<32}  vérification de l'empreinte {} %",
                                read.min(total) * 100 / total
                            ),
                            _ => format!("{name:<32}  vérification de l'empreinte"),
                        }
                    }
                    (Some(line), _) => format!("{name:<32}  {}", line.render()),
                    (None, state) => format!("{name:<32}  {state}"),
                }
            })
            .collect();
        self.status.show(&lines);
    }

    /// Jusqu'à l'arrêt de toutes les tâches suivies. Avec `pause_on_interrupt`, Ctrl-C
    /// les met en pause et attend que ce soit fait ; sinon il arrête seulement le suivi.
    async fn run(&mut self, rpc: &mut Rpc, pause_on_interrupt: bool) -> Result<()> {
        let mut interrupted = false;
        while !self.done() {
            let event = tokio::select! {
                n = rpc.notification() => Some(n?),
                _ = tokio::signal::ctrl_c() => None,
            };
            let Some((method, params)) = event else {
                self.interrupted = true;
                if !pause_on_interrupt || interrupted {
                    break;
                }
                interrupted = true;
                let ids: Vec<&String> = self.targets.iter().collect();
                eprintln!("\nMise en pause… (Ctrl-C de nouveau pour quitter sans attendre)");
                if let Err(e) = rpc.call("task.pause", json!({"taskIds": ids})).await
                    && e.downcast_ref::<EngineRefused>().is_none()
                {
                    return Err(e);
                }
                continue;
            };
            let rev = params["rev"].as_u64().unwrap_or(0);
            match method.as_str() {
                "task.added" if rev > self.rev => {
                    let t = &params["task"];
                    let id = t["taskId"].as_str().unwrap_or_default().to_owned();
                    if self.all {
                        self.targets.insert(id.clone());
                    }
                    self.tasks.insert(
                        id,
                        Followed {
                            name: t["displayName"].as_str().unwrap_or_default().to_owned(),
                            state: t["state"].as_str().unwrap_or_default().to_owned(),
                            line: None,
                            checked: None,
                            kind: t["kind"].as_str().unwrap_or("http").to_owned(),
                        },
                    );
                }
                "task.stateChanged" if rev > self.rev => {
                    let id = params["taskId"].as_str().unwrap_or_default();
                    let to = params["to"].as_str().unwrap_or_default().to_owned();
                    if let Some(t) = self.tasks.get_mut(id) {
                        t.state = to.clone();
                        if !running(&to) && self.targets.contains(id) {
                            let name = t.name.clone();
                            self.render();
                            self.status.finish();
                            eprintln!("{name} : {to}");
                        }
                    }
                }
                "task.removed" if rev > self.rev => {
                    let id = params["taskId"].as_str().unwrap_or_default().to_owned();
                    self.forget(&id);
                }
                // Préparation : le nom du fichier, ou le titre d'une vidéo.
                "task.updated" if rev > self.rev => {
                    let id = params["taskId"].as_str().unwrap_or_default();
                    if let (Some(t), Some(name)) =
                        (self.tasks.get_mut(id), params["displayName"].as_str())
                    {
                        t.name = name.to_owned();
                    }
                }
                "task.progress" => {
                    for item in params["items"].as_array().into_iter().flatten() {
                        let id = item["taskId"].as_str().unwrap_or_default();
                        if let Some(t) = self.tasks.get_mut(id) {
                            t.checked = item["verifiedBytes"].as_u64();
                            t.line = Some(Line {
                                done: item["doneBytes"].as_u64().unwrap_or(0),
                                total: item["totalBytes"].as_u64(),
                                bps: item["bps"].as_u64().unwrap_or(0),
                                connections: item["connections"].as_u64().unwrap_or(0),
                                eta: item["etaS"].as_u64(),
                                peers: t.kind == "torrent",
                                upload_bps: item["uploadBps"].as_u64(),
                            });
                        }
                    }
                }
                // Notifications perdues : on repart d'un état complet.
                "engine.warning" if params["code"] == "EVENTS_LOST" => {
                    self.refresh(rpc).await?;
                }
                // Avertissement sur une tâche suivie (sous-titres manquants, SPEC §7.17).
                "engine.warning"
                    if params["taskId"]
                        .as_str()
                        .is_some_and(|id| self.targets.contains(id)) =>
                {
                    self.render();
                    self.status.finish();
                    eprintln!(
                        "Attention : {}",
                        params["message"].as_str().unwrap_or_default()
                    );
                }
                _ => {}
            }
            self.render();
        }
        self.status.finish();
        // Une vidéo terminée sans certains sous-titres l'apprend juste après sa fin
        // (`engine.warning`, SPEC §7.17) : un court délai pour le dire.
        let videos = self
            .targets
            .iter()
            .any(|id| self.tasks.get(id).is_some_and(|t| t.kind == "media"));
        if videos && !interrupted {
            let deadline = std::time::Duration::from_millis(500);
            while let Ok(Ok((method, params))) =
                tokio::time::timeout(deadline, rpc.notification()).await
            {
                if method == "engine.warning"
                    && params["taskId"]
                        .as_str()
                        .is_some_and(|id| self.targets.contains(id))
                {
                    eprintln!(
                        "Attention : {}",
                        params["message"].as_str().unwrap_or_default()
                    );
                }
            }
        }
        Ok(())
    }
}

/// Déplace ou renomme le fichier d'une tâche (SPEC §7.12).
pub async fn relocate(
    rpc: &mut Rpc,
    id: &str,
    dir: Option<&Path>,
    name: Option<&str>,
) -> Result<()> {
    if dir.is_none() && name.is_none() {
        bail!("préciser le nouveau dossier (-o), le nouveau nom (-n), ou les deux");
    }
    let mut params = json!({"taskId": id});
    if let Some(dir) = dir {
        params["destDir"] = json!(dest_dir(dir)?);
    }
    if let Some(name) = name {
        params["fileName"] = json!(name);
    }
    let task = rpc.call("task.update", params).await?;
    let place = Path::new(task["destDir"].as_str().unwrap_or_default())
        .join(task["displayName"].as_str().unwrap_or_default());
    if task["moving"].as_bool() == Some(true) {
        println!(
            "Copie vers l'autre disque en cours ; à la fin, la tâche sera à {}",
            dest_dir(dir.unwrap_or(Path::new(".")))?
        );
    } else {
        println!("Tâche {id} : {}", place.display());
    }
    Ok(())
}

/// Remplace l'adresse d'une tâche, par exemple un lien expiré (SPEC §7.13).
pub async fn refresh(rpc: &mut Rpc, id: &str, url: &str) -> Result<()> {
    let task = rpc
        .call("task.update", json!({"taskId": id, "url": url}))
        .await?;
    let name = task["displayName"].as_str().unwrap_or(id);
    match task["state"].as_str() {
        Some("failed" | "paused") => {
            println!("Adresse de {name} remplacée. Pour reprendre : auro resume {id}")
        }
        _ => println!("Adresse de {name} remplacée."),
    }
    Ok(())
}

/// Ajoute un miroir à une tâche (`task.addSource`, SPEC §7.7). Une tâche préparée le voit
/// vérifié tout de suite : un autre fichier est refusé ici.
pub async fn add_mirror(rpc: &mut Rpc, id: &str, url: &str) -> Result<()> {
    rpc.call("task.addSource", json!({"taskId": id, "url": url}))
        .await?;
    println!("Miroir ajouté à {id}.");
    Ok(())
}

/// Sources d'une tâche : rang, état, débit, raison d'un échec ou d'un refus.
pub async fn sources(rpc: &mut Rpc, id: &str) -> Result<()> {
    let task = rpc
        .call("task.get", json!({"taskId": id, "include": ["sources"]}))
        .await?;
    for s in task["sources"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let rank = s["rank"].as_u64().unwrap_or(0);
        let role = if rank == 0 { "référence" } else { "miroir" };
        let state = match s["state"].as_str().unwrap_or("?") {
            "unknown" => "à vérifier",
            "ok" => "bonne",
            "slow" => "lente",
            "failed" => "écartée",
            other => other,
        };
        let mut line = format!(
            "{rank:>2}  {role:<9}  {state:<10}  {}",
            s["url"].as_str().unwrap_or("")
        );
        if let Some(bps) = s["avgBps"].as_u64() {
            line.push_str(&format!("  {}/s", display::size(bps)));
        }
        if let Some(error) = s["lastError"].as_str() {
            line.push_str(&format!("  ({error})"));
        }
        println!("{line}");
    }
    Ok(())
}

/// Historique (F-35), page par page : un tableau, ou du CSV (RFC 4180) avec les dates en
/// heure locale, les tailles en octets et les états du protocole.
pub async fn history(rpc: &mut Rpc, query: Option<&str>, csv: bool) -> Result<()> {
    // L'heure locale, d'après SQLite (modificateur `localtime`), comme le moteur.
    let clock = rusqlite::Connection::open_in_memory()?;
    let local = |ms: i64| -> String {
        clock
            .query_row(
                "SELECT strftime('%Y-%m-%d %H:%M:%S', ?1 / 1000, 'unixepoch', 'localtime')",
                [ms],
                |r| r.get(0),
            )
            .unwrap_or_default()
    };
    if csv {
        println!("date,etat,taille,nom,site,chemin,sha256");
    }
    let (mut cursor, mut count): (Option<String>, usize) = (None, 0);
    loop {
        let mut params = json!({"limit": 500});
        if let Some(q) = query {
            params["query"] = json!(q);
        }
        if let Some(c) = &cursor {
            params["cursor"] = json!(c);
        }
        let page = rpc.call("history.list", params).await?;
        for h in page["items"].as_array().into_iter().flatten() {
            count += 1;
            let text = |key: &str| h[key].as_str().unwrap_or_default().to_owned();
            let when = local(h["finishedAt"].as_i64().unwrap_or(0));
            let total = h["totalBytes"].as_u64();
            if csv {
                let fields = [
                    when,
                    text("finalState"),
                    total.map(|t| t.to_string()).unwrap_or_default(),
                    text("displayName"),
                    text("sourceHost"),
                    text("finalPath"),
                    text("sha256"),
                ];
                let line: Vec<String> = fields.iter().map(|f| display::csv_field(f)).collect();
                println!("{}", line.join(","));
            } else {
                println!(
                    "{when}  {:<10} {:>10}  {}  ({})",
                    text("finalState"),
                    total.map(size).unwrap_or_else(|| "?".into()),
                    text("displayName"),
                    text("sourceHost")
                );
            }
        }
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    if count == 0 && !csv {
        println!("Historique vide.");
    }
    Ok(())
}

/// Réglages du proxy, lisibles ; jamais le mot de passe.
fn print_proxy(view: &Value) {
    let mode = view["mode"].as_str().unwrap_or_default();
    match mode {
        "system" => println!("Proxy : celui du système."),
        "none" => println!("Proxy : aucun (toujours en direct)."),
        _ => {
            println!("Proxy : {}", view["url"].as_str().unwrap_or("?"));
            let exceptions: Vec<&str> = view["exceptions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if !exceptions.is_empty() {
                println!("En direct : {}", exceptions.join(", "));
            }
            if let Some(user) = view["username"].as_str() {
                let password = if view["hasPassword"] == true {
                    "mot de passe au coffre du système"
                } else {
                    "sans mot de passe"
                };
                println!("Identifiant : {user} ({password})");
            }
        }
    }
    println!("Les torrents ne passent jamais par le proxy.");
}

/// `auro proxy` : les réglages du proxy.
pub async fn proxy_show(rpc: &mut Rpc) -> Result<()> {
    print_proxy(&rpc.call("proxy.get", json!({})).await?);
    Ok(())
}

/// `auro proxy system|none|manual` : change le proxy, puis l'affiche.
pub async fn proxy_set(rpc: &mut Rpc, params: Value) -> Result<()> {
    print_proxy(&rpc.call("proxy.set", params).await?);
    Ok(())
}

/// Types de fichier des règles (ceux de la page) ; une autre valeur de `--type` est une
/// extension.
pub const RULE_CATEGORIES: [&str; 7] = [
    "compressed",
    "documents",
    "images",
    "music",
    "programs",
    "video",
    "other",
];

/// Ce qu'on change d'une règle existante.
pub enum RuleEdit {
    Remove,
    Enable(bool),
    /// Nouveau rang, à partir de 1.
    MoveTo(usize),
}

fn texts(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn print_rules(rules: &Value) {
    let list = rules["rules"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if list.is_empty() {
        println!("Aucune règle de rangement.");
        return;
    }
    for (i, rule) in list.iter().enumerate() {
        let name = rule["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .unwrap_or("(sans nom)");
        let state = if rule["enabled"] == true {
            ""
        } else {
            "  [désactivée]"
        };
        let id = rule["ruleId"].as_str().unwrap_or_default();
        println!("{}. {name}{state}  ({id})", i + 1);
        let m = &rule["match"];
        let mut when = Vec::new();
        let types: Vec<&str> = texts(&m["categories"])
            .into_iter()
            .chain(texts(&m["extensions"]))
            .collect();
        if !types.is_empty() {
            when.push(format!("type {}", types.join(", ")));
        }
        let domains = texts(&m["domains"]);
        if !domains.is_empty() {
            when.push(format!("domaine {}", domains.join(", ")));
        }
        if let Some(r) = m["urlRegex"].as_str() {
            when.push(format!("adresse ~ {r}"));
        }
        if let Some(r) = m["nameRegex"].as_str() {
            when.push(format!("nom ~ {r}"));
        }
        if when.is_empty() {
            println!("   si : toute tâche");
        } else {
            println!("   si : {}", when.join(" ; "));
        }
        let a = &rule["actions"];
        let mut then = Vec::new();
        if let Some(f) = a["folder"].as_str() {
            then.push(format!("dossier {f}"));
        }
        if let Some(r) = a["rename"].as_str() {
            then.push(format!("nom {r}"));
        }
        if let Some(c) = a["command"].as_object() {
            let args = texts(c.get("args").unwrap_or(&Value::Null));
            let timeout = c
                .get("timeoutS")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let mut line = format!("commande {}", c["program"].as_str().unwrap_or("?"));
            for arg in args {
                line.push(' ');
                line.push_str(arg);
            }
            then.push(format!("{line} (délai {timeout} s)"));
        } else if a["hasCommand"] == true {
            then.push("une commande de fin".into());
        }
        if then.is_empty() {
            println!("   alors : rien (la première règle qui correspond l'emporte)");
        } else {
            println!("   alors : {}", then.join(" ; "));
        }
    }
}

/// `auro rules` : les règles de rangement, dans leur ordre.
pub async fn rules_show(rpc: &mut Rpc) -> Result<()> {
    print_rules(&rpc.call("rules.list", json!({})).await?);
    Ok(())
}

/// Règles telles que `rules.set` les reprend.
async fn current_rules(rpc: &mut Rpc) -> Result<Vec<Value>> {
    let listed = rpc.call("rules.list", json!({})).await?;
    Ok(listed["rules"].as_array().cloned().unwrap_or_default())
}

/// `auro rules add` : ajoute une règle, à la fin ou au rang `at` (à partir de 1).
pub async fn rules_add(rpc: &mut Rpc, rule: Value, at: Option<usize>) -> Result<()> {
    let mut rules = current_rules(rpc).await?;
    let at = at.map_or(rules.len(), |at| at.saturating_sub(1).min(rules.len()));
    rules.insert(at, rule);
    print_rules(&rpc.call("rules.set", json!({"rules": rules})).await?);
    Ok(())
}

/// Une règle désignée par son rang (à partir de 1), son identifiant ou son nom.
fn find_rule(rules: &[Value], key: &str) -> Result<usize> {
    if let Ok(rank) = key.parse::<usize>()
        && (1..=rules.len()).contains(&rank)
    {
        return Ok(rank - 1);
    }
    let found: Vec<usize> = rules
        .iter()
        .enumerate()
        .filter(|(_, r)| r["ruleId"] == key || r["name"] == key)
        .map(|(i, _)| i)
        .collect();
    match found.as_slice() {
        [i] => Ok(*i),
        [] => bail!("aucune règle {key:?} (rang, identifiant ou nom ; voir auro rules)"),
        _ => bail!("plusieurs règles s'appellent {key:?} : donnez son rang ou son identifiant"),
    }
}

/// `auro rules remove|enable|disable|move` : change une règle, puis les affiche.
pub async fn rules_edit(rpc: &mut Rpc, key: &str, edit: RuleEdit) -> Result<()> {
    let mut rules = current_rules(rpc).await?;
    let i = find_rule(&rules, key)?;
    match edit {
        RuleEdit::Remove => {
            rules.remove(i);
        }
        RuleEdit::Enable(on) => rules[i]["enabled"] = json!(on),
        RuleEdit::MoveTo(rank) => {
            let rule = rules.remove(i);
            let at = rank.saturating_sub(1).min(rules.len());
            rules.insert(at, rule);
        }
    }
    print_rules(&rpc.call("rules.set", json!({"rules": rules})).await?);
    Ok(())
}

/// Lignes de journal montrées avant l'enregistrement (les dernières).
const DIAGNOSTIC_SHOWN: usize = 20;

/// `auro diagnostic` (N-12) : montre ce que contient le diagnostic, sans adresses ni
/// identifiants ; avec `save`, l'écrit dans une archive `.zip`.
pub async fn diagnostic(rpc: &mut Rpc, save: Option<&Path>) -> Result<()> {
    let found = rpc.call("engine.diagnostic", json!({})).await?;
    println!("Rapport :");
    println!("{}", serde_json::to_string_pretty(&found["report"])?);
    let logs = found["logs"].as_array().cloned().unwrap_or_default();
    println!();
    println!(
        "Journaux : {} ligne(s), nettoyées (adresses, hôtes, chemins, nom de l'utilisateur et jetons retirés).",
        logs.len()
    );
    if !logs.is_empty() {
        println!("Les {} dernières :", DIAGNOSTIC_SHOWN.min(logs.len()));
        for line in logs
            .iter()
            .skip(logs.len().saturating_sub(DIAGNOSTIC_SHOWN))
        {
            println!("  {}", line.as_str().unwrap_or_default());
        }
    }
    match save {
        Some(path) => {
            auro_engine::logs::write_archive(path, &found)
                .with_context(|| format!("archive {}", path.display()))?;
            println!();
            println!(
                "Enregistré : {} (rapport.json, journaux.jsonl).",
                path.display()
            );
        }
        None => {
            println!();
            println!("Rien n'est enregistré : relancer avec --save diagnostic.zip pour l'archive.");
        }
    }
    Ok(())
}

/// Efface tout l'historique ; les tâches et les fichiers restent.
pub async fn clear_history(rpc: &mut Rpc) -> Result<()> {
    let result = rpc.call("history.clear", json!({})).await?;
    println!(
        "{} entrée(s) d'historique effacée(s).",
        result["removed"].as_u64().unwrap_or(0)
    );
    Ok(())
}

/// Empreinte au format du protocole.
fn checksum_json(c: &Checksum) -> Value {
    json!({"algo": c.algo().as_sql(), "value": c.value()})
}

/// Donne, change (`Some`) ou retire (`None`) l'empreinte attendue d'une tâche (F-11).
pub async fn set_checksum(rpc: &mut Rpc, id: &str, checksum: Option<&Checksum>) -> Result<()> {
    let value = checksum.map_or(Value::Null, checksum_json);
    let task = rpc
        .call("task.update", json!({"taskId": id, "checksum": value}))
        .await?;
    let name = task["displayName"].as_str().unwrap_or(id);
    match (checksum, task["state"].as_str()) {
        (None, _) => println!("Empreinte de {name} retirée."),
        (Some(c), Some("failed" | "paused")) => println!(
            "Empreinte {} de {name} enregistrée. Pour vérifier le fichier reçu : auro resume {id}",
            c.algo().label()
        ),
        (Some(c), _) => println!("Empreinte {} de {name} enregistrée.", c.algo().label()),
    }
    Ok(())
}

/// Identifiants enregistrés, sans mots de passe (SPEC §7.14).
pub async fn logins(rpc: &mut Rpc) -> Result<()> {
    let list = rpc.call("login.list", json!({})).await?;
    let items = list["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        println!("Aucun identifiant de site.");
    }
    for item in items {
        let plain = if item["allowPlain"] == true {
            "  (FTP sans chiffrement autorisé)"
        } else {
            ""
        };
        let key = item["keyPath"]
            .as_str()
            .map(|k| format!("  (clé privée {k})"))
            .unwrap_or_default();
        println!(
            "{:<40} {}{key}{plain}",
            item["site"].as_str().unwrap_or_default(),
            item["username"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

/// Identifiants déjà enregistrés pour ce site (même forme normalisée), s'il y en a.
async fn login_of(rpc: &mut Rpc, site: &str) -> Result<Option<Value>> {
    let wanted = auro_engine::net::auth::Site::parse(site)
        .map_err(|e| anyhow::anyhow!(e))?
        .text();
    let list = rpc.call("login.list", json!({})).await?;
    Ok(list["items"]
        .as_array()
        .and_then(|items| items.iter().find(|i| i["site"] == wanted.as_str()))
        .cloned())
}

/// Enregistre ou modifie les identifiants d'un site ; `password` absent : celui déjà
/// enregistré reste. `allow_plain` : FTP sans chiffrement permis (F-08) ; absent : inchangé.
/// Ce que `auro login set` change, en plus du site et du nom.
pub struct LoginChange {
    /// `None` : celui déjà enregistré reste.
    pub password: Option<String>,
    /// FTP sans chiffrement permis (F-08) ; `None` : inchangé.
    pub allow_plain: Option<bool>,
    /// SFTP : fichier de clé privée ; `Some("")` la retire ; `None` : inchangée.
    pub key_path: Option<String>,
}

pub async fn login_set(
    rpc: &mut Rpc,
    site: &str,
    username: &str,
    change: LoginChange,
) -> Result<()> {
    let mut params = json!({"site": site, "username": username});
    if let Some(existing) = login_of(rpc, site).await? {
        params["loginId"] = existing["loginId"].clone();
    }
    if let Some(password) = change.password {
        params["password"] = json!(password);
    }
    if let Some(allow) = change.allow_plain {
        params["allowPlain"] = json!(allow);
    }
    if let Some(key) = change.key_path {
        params["keyPath"] = json!(key);
    }
    let saved = rpc.call("login.set", params).await?;
    let (site, username) = (
        saved["site"].as_str().unwrap_or(site),
        saved["username"].as_str().unwrap_or(username),
    );
    match saved["keyPath"].as_str() {
        Some(key) => println!(
            "Identifiants de {site} enregistrés ({username}, clé privée {key}) ; la phrase de \
             passe est dans le coffre du système."
        ),
        None => println!(
            "Identifiants de {site} enregistrés ({username}) ; le mot de passe est dans le \
             coffre du système."
        ),
    }
    if saved["allowPlain"] == true {
        println!(
            "FTP sans chiffrement autorisé pour ce site : le mot de passe peut y passer en clair."
        );
    }
    Ok(())
}

/// `hôte` ou `hôte:port` (22 par défaut), `[adresse IPv6]:port` compris.
fn server_of(text: &str) -> Result<(String, u16)> {
    let text = text.trim();
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .with_context(|| format!("serveur illisible : {text}"))?;
        (host, after.strip_prefix(':'))
    } else if text.matches(':').count() == 1 {
        let (host, port) = text.split_once(':').expect("un deux-points");
        (host, Some(port))
    } else {
        (text, None)
    };
    let port = match port {
        Some(p) => p
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .with_context(|| format!("port illisible : {p}"))?,
        None => 22,
    };
    if host.is_empty() {
        bail!("serveur illisible : {text}");
    }
    Ok((host.to_ascii_lowercase(), port))
}

/// Clés des serveurs SSH : acceptées, puis en attente d'une décision.
pub async fn host_keys(rpc: &mut Rpc) -> Result<()> {
    let keys = rpc.call("hostkey.list", json!({})).await?;
    let items = keys["items"].as_array().cloned().unwrap_or_default();
    let pending = keys["pending"].as_array().cloned().unwrap_or_default();
    if items.is_empty() && pending.is_empty() {
        println!("Aucune clé de serveur SSH.");
    }
    let server = |k: &Value| {
        let host = k["host"].as_str().unwrap_or_default();
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        format!("{host}:{}", k["port"])
    };
    for k in &items {
        println!(
            "{:<32} {} {}",
            server(k),
            k["algorithm"].as_str().unwrap_or_default(),
            k["fingerprint"].as_str().unwrap_or_default()
        );
    }
    for k in &pending {
        let why = if k["changed"] == true {
            "CLÉ CHANGÉE (attaque possible)"
        } else {
            "inconnue"
        };
        println!(
            "{:<32} {} {}  en attente : {why}",
            server(k),
            k["algorithm"].as_str().unwrap_or_default(),
            k["fingerprint"].as_str().unwrap_or_default()
        );
    }
    if !pending.is_empty() {
        println!(
            "Vérifier l'empreinte auprès de l'administrateur du serveur, puis : auro hostkey \
             accept <serveur> <empreinte>"
        );
    }
    Ok(())
}

/// Accepte la clé en attente d'un serveur ; les tâches qu'elle bloquait repartent.
pub async fn host_key_accept(rpc: &mut Rpc, server: &str, fingerprint: &str) -> Result<()> {
    let (host, port) = server_of(server)?;
    let done = rpc
        .call(
            "hostkey.accept",
            json!({"host": host, "port": port, "fingerprint": fingerprint.trim()}),
        )
        .await?;
    let resumed = done["resumed"].as_array().map_or(0, Vec::len);
    println!("Clé de {host}:{port} acceptée ; {resumed} tâche(s) relancée(s).");
    Ok(())
}

/// Oublie la clé acceptée d'un serveur.
pub async fn host_key_remove(rpc: &mut Rpc, server: &str) -> Result<()> {
    let (host, port) = server_of(server)?;
    let done = rpc
        .call("hostkey.remove", json!({"host": host, "port": port}))
        .await?;
    if done["removed"] != true {
        bail!("aucune clé acceptée pour {host}:{port}");
    }
    println!("Clé de {host}:{port} oubliée : la prochaine connexion la montrera de nouveau.");
    Ok(())
}

/// Retire les identifiants d'un site.
pub async fn login_remove(rpc: &mut Rpc, site: &str) -> Result<()> {
    let Some(existing) = login_of(rpc, site).await? else {
        bail!("aucun identifiant enregistré pour {site}");
    };
    rpc.call("login.delete", json!({"loginId": existing["loginId"]}))
        .await?;
    println!(
        "Identifiants de {} retirés.",
        existing["site"].as_str().unwrap_or(site)
    );
    Ok(())
}

/// Fichiers d'un torrent (`task.get` avec `files`) ; avec `select`, les rangs à garder
/// (`task.setFiles`, SPEC §7.16), les autres n'étant pas téléchargés.
pub async fn files(rpc: &mut Rpc, id: &str, select: Option<&[u32]>) -> Result<()> {
    if let Some(selected) = select {
        let task = rpc
            .call("task.setFiles", json!({"taskId": id, "selected": selected}))
            .await?;
        println!(
            "{} fichier(s) choisi(s), {} à télécharger",
            selected.len(),
            task["totalBytes"]
                .as_u64()
                .map_or_else(|| "taille inconnue".to_owned(), size)
        );
    }
    let task = rpc
        .call("task.get", json!({"taskId": id, "include": ["files"]}))
        .await?;
    let files = task["files"].as_array().cloned().unwrap_or_default();
    if files.is_empty() {
        match task["kind"].as_str() {
            Some("torrent") => println!("Fichiers pas encore connus (métadonnées attendues)."),
            _ => println!("Tâche d'un seul fichier : {}", task["displayName"]),
        }
        return Ok(());
    }
    for f in &files {
        let length = f["length"].as_u64().unwrap_or(0);
        let done = f["doneBytes"].as_u64().unwrap_or(0);
        let mark = if f["selected"].as_bool() == Some(true) {
            "x"
        } else {
            " "
        };
        println!(
            "[{mark}] {:>4}  {:>21}  {}",
            f["index"],
            format!("{} / {}", size(done.min(length)), size(length)),
            f["path"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

/// Une valeur de réglage tapée : booléen (`true`, `oui`, `on`…), `none` ou `null` (retirer
/// la valeur), nombre, sinon texte. Le moteur vérifie le type attendu par chaque réglage.
pub fn setting_value(text: &str) -> Value {
    match text.to_lowercase().as_str() {
        "true" | "oui" | "on" | "yes" => return json!(true),
        "false" | "non" | "off" | "no" => return json!(false),
        "none" | "null" | "aucun" | "aucune" => return Value::Null,
        _ => {}
    }
    if let Ok(n) = text.parse::<u64>() {
        return json!(n);
    }
    match text.parse::<f64>() {
        Ok(f) if f.is_finite() => json!(f),
        _ => json!(text),
    }
}

/// Réglages du moteur (`settings.get`) ; avec une valeur, change d'abord le réglage
/// (`settings.set`).
pub async fn config(rpc: &mut Rpc, key: Option<&str>, value: Option<&str>) -> Result<()> {
    let settings = match (key, value) {
        (Some(k), Some(v)) => {
            let mut values = serde_json::Map::new();
            values.insert(k.to_owned(), setting_value(v));
            rpc.call("settings.set", json!({"values": values})).await?
        }
        _ => rpc.call("settings.get", json!({})).await?,
    };
    let values = settings["values"].as_object().cloned().unwrap_or_default();
    let shown = |v: &Value| match v {
        Value::Null => "aucun".to_owned(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    match key {
        Some(k) => {
            let v = values
                .get(k)
                .with_context(|| format!("réglage inconnu : {k}"))?;
            println!("{k} = {}", shown(v));
        }
        None => {
            for (k, v) in &values {
                println!("{k:<28} {}", shown(v));
            }
        }
    }
    Ok(())
}

/// Crée un jeton de l'interface distante (SPEC §7.19) et l'affiche, une seule fois.
pub async fn token_create(rpc: &mut Rpc, name: &str, scope: &str) -> Result<()> {
    let created = rpc
        .call("token.create", json!({"name": name, "scope": scope}))
        .await?;
    println!(
        "Jeton « {} » créé (portée {}, identifiant {}).",
        created["name"].as_str().unwrap_or(name),
        created["scope"].as_str().unwrap_or(scope),
        created["id"].as_str().unwrap_or_default()
    );
    println!("{}", created["token"].as_str().unwrap_or_default());
    println!(
        "Il ne sera plus jamais montré : le garder en lieu sûr. `auro token revoke` le retire."
    );
    Ok(())
}

/// Les jetons, sans leur valeur.
pub async fn tokens(rpc: &mut Rpc) -> Result<()> {
    let list = rpc.call("token.list", json!({})).await?;
    let items = list["tokens"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        println!("Aucun jeton.");
        return Ok(());
    }
    println!(
        "{:<36}  {:<8}  {:<16}  {:<16}  nom",
        "identifiant", "portée", "créé", "dernier usage"
    );
    for t in items {
        println!(
            "{:<36}  {:<8}  {:<16}  {:<16}  {}",
            t["id"].as_str().unwrap_or_default(),
            t["scope"].as_str().unwrap_or_default(),
            local_time(t["createdAt"].as_i64()),
            local_time(t["lastUsedAt"].as_i64()),
            t["name"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

/// Révoque un jeton, désigné par son identifiant ou par son nom s'il est seul à le porter.
pub async fn token_revoke(rpc: &mut Rpc, which: &str) -> Result<()> {
    let list = rpc.call("token.list", json!({})).await?;
    let items = list["tokens"].as_array().cloned().unwrap_or_default();
    let by_id: Vec<&Value> = items.iter().filter(|t| t["id"] == which).collect();
    let by_name: Vec<&Value> = items.iter().filter(|t| t["name"] == which).collect();
    let token = match (by_id.as_slice(), by_name.as_slice()) {
        ([one], _) | ([], [one]) => *one,
        ([], []) => bail!("aucun jeton {which}"),
        _ => bail!("plusieurs jetons s'appellent « {which} » : donner l'identifiant"),
    };
    rpc.call("token.revoke", json!({"tokenId": token["id"]}))
        .await?;
    println!(
        "Jeton « {} » révoqué : ses sessions et ses connexions sont fermées.",
        token["name"].as_str().unwrap_or(which)
    );
    Ok(())
}

/// État de l'interface distante (SPEC §7.19) : réglages et écoute.
pub async fn remote_status(rpc: &mut Rpc) -> Result<()> {
    let state = rpc.call("remote.status", json!({})).await?;
    let values = rpc.call("settings.get", json!({})).await?["values"].clone();
    let listen = values["remote.listen"].as_str().unwrap_or_default();
    let texts = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    match (
        state["enabled"].as_bool().unwrap_or(false),
        state["listening"].as_bool().unwrap_or(false),
    ) {
        (false, _) => println!("Interface distante : coupée (écoute prévue sur {listen})."),
        (true, true) => {
            println!("Interface distante : ouverte sur {listen}. Adresses de la page :");
            for url in texts(&state["urls"]) {
                println!("  {url}");
            }
        }
        (true, false) => println!(
            "Interface distante : activée mais fermée ({}).",
            state["error"].as_str().unwrap_or("pas encore ouverte")
        ),
    }
    let cert = &state["certificate"];
    if let Some(fingerprint) = cert["fingerprint"].as_str() {
        let origin = if cert["generated"].as_bool().unwrap_or(false) {
            "fabriqué par le moteur (auto-signé ; le navigateur demande confirmation)"
        } else {
            "fourni"
        };
        println!(
            "Certificat {origin}, valable jusqu'au {}.",
            local_time(cert["expiresAt"].as_i64())
        );
        println!("Empreinte SHA-256, à comparer dans le navigateur :\n  {fingerprint}");
        println!("Noms : {}", texts(&cert["names"]).join(", "));
    } else if state["tls"].as_bool().unwrap_or(false) {
        println!("En TLS (hors de la boucle locale).");
    }
    let hosts = texts(&values["remote.hosts"]);
    if !hosts.is_empty() {
        println!("Noms ajoutés : {}", hosts.join(", "));
    }
    if let (Some(c), Some(k)) = (
        values["remote.tlsCert"].as_str(),
        values["remote.tlsKey"].as_str(),
    ) {
        println!("Certificat fourni : {c} (clé : {k})");
    }
    let private = values["remote.allowPrivate"].as_bool().unwrap_or(false);
    println!(
        "Réseau interne pour les jetons : {}.",
        if private { "permis" } else { "refusé" }
    );
    Ok(())
}

/// Une date du moteur (millisecondes Unix) à l'heure locale, `AAAA-MM-JJ HH:MM`.
fn local_time(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return "jamais".to_owned();
    };
    rusqlite::Connection::open_in_memory()
        .and_then(|clock| {
            clock.query_row(
                "SELECT strftime('%Y-%m-%d %H:%M', ?1 / 1000, 'unixepoch', 'localtime')",
                [ms],
                |r| r.get(0),
            )
        })
        .unwrap_or_default()
}

/// Ce que `remote_set` change, en plus de l'activation.
#[derive(Debug, Default)]
pub struct RemoteChange<'a> {
    pub enabled: Option<bool>,
    pub listen: Option<&'a str>,
    pub allow_private: Option<bool>,
    /// Noms ajoutés (`remote.hosts`) ; vide : aucun.
    pub hosts: Option<Vec<String>>,
    /// Certificat et clé fournis (chemins), ou `Some(None)` pour celui du moteur.
    pub tls_files: Option<Option<(String, String)>>,
}

/// Change les réglages de l'interface distante, puis attend que l'écoute les suive.
pub async fn remote_set(rpc: &mut Rpc, change: RemoteChange<'_>) -> Result<()> {
    let mut values = serde_json::Map::new();
    if let Some(listen) = change.listen {
        values.insert("remote.listen".into(), json!(listen));
    }
    if let Some(on) = change.allow_private {
        values.insert("remote.allowPrivate".into(), json!(on));
    }
    if let Some(hosts) = &change.hosts {
        values.insert("remote.hosts".into(), json!(hosts));
    }
    if let Some(files) = change.tls_files {
        // Chemins absolus : le moteur ne tourne pas forcément dans ce dossier.
        let (cert, key) = match files {
            Some((cert, key)) => (
                json!(std::path::absolute(cert)?.to_string_lossy()),
                json!(std::path::absolute(key)?.to_string_lossy()),
            ),
            None => (Value::Null, Value::Null),
        };
        values.insert("remote.tlsCert".into(), cert);
        values.insert("remote.tlsKey".into(), key);
    }
    if let Some(on) = change.enabled {
        values.insert("remote.enabled".into(), json!(on));
    }
    rpc.call("settings.set", json!({"values": values})).await?;
    // L'écoute suit les réglages dans le moteur, un instant après : attendre qu'elle
    // corresponde à ceux qui sont enregistrés.
    let values = rpc.call("settings.get", json!({})).await?["values"].clone();
    let wanted = values["remote.enabled"].as_bool().unwrap_or(false);
    for _ in 0..50 {
        let state = rpc.call("remote.status", json!({})).await?;
        let listening = state["listening"].as_bool().unwrap_or(false);
        let applied = state["enabled"].as_bool() == Some(wanted)
            && state["listen"] == values["remote.listen"]
            && state["hosts"] == values["remote.hosts"]
            && state["tlsCert"] == values["remote.tlsCert"];
        let settled = if wanted {
            listening || state["error"].is_string()
        } else {
            !listening
        };
        if applied && settled {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    remote_status(rpc).await
}

// ---------- Plugins d'hébergeurs (F-41, SPEC §7.20) ----------

/// Domaines d'une liste JSON, séparés par des virgules.
fn domain_list(v: &Value) -> String {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// Montre un plugin à installer (manifeste, permissions, empreinte) ; l'installe avec
/// `yes`, l'accord de l'utilisateur donné sur ce qu'il vient de voir.
pub async fn plugin_install(rpc: &mut Rpc, file: &Path, yes: bool) -> Result<()> {
    use base64::Engine as _;
    let size = std::fs::metadata(file)
        .with_context(|| format!("{} illisible", file.display()))?
        .len();
    if size > 8 << 20 {
        bail!("fichier de plugin de 8 Mio au plus");
    }
    let bytes = std::fs::read(file).with_context(|| format!("{} illisible", file.display()))?;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let shown = rpc.call("plugin.inspect", json!({ "data": data })).await?;
    let id = shown["pluginId"].as_str().unwrap_or_default();
    println!(
        "Plugin « {} » {} ({id})",
        shown["name"].as_str().unwrap_or_default(),
        shown["version"].as_str().unwrap_or_default()
    );
    if let Some(author) = shown["author"].as_str() {
        println!("Auteur : {author}");
    }
    if let Some(description) = shown["description"].as_str() {
        println!("{description}");
    }
    println!("Pages prises en charge : {}", domain_list(&shown["sites"]));
    println!(
        "Domaines qu'il peut faire demander : {}",
        domain_list(&shown["fetch"])
    );
    println!(
        "Empreinte SHA-256 : {}",
        shown["digest"].as_str().unwrap_or_default()
    );
    // Nouvelle version d'un plugin installé : ce qu'elle demande de plus.
    let installed = rpc.call("plugin.list", json!({})).await?;
    if let Some(old) = installed["plugins"]
        .as_array()
        .and_then(|list| list.iter().find(|p| p["pluginId"] == id))
    {
        let had = |key: &str, d: &str| {
            old[key]
                .as_array()
                .is_some_and(|list| list.iter().any(|x| x == d))
        };
        let added: Vec<String> = ["sites", "fetch"]
            .iter()
            .flat_map(|key| {
                shown[*key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|d| !had(key, d))
                    .map(|d| d.to_owned())
                    .collect::<Vec<_>>()
            })
            .collect();
        println!(
            "Remplace la version {} installée.{}",
            old["version"].as_str().unwrap_or_default(),
            if added.is_empty() {
                String::new()
            } else {
                format!(" Domaines nouveaux : {}", added.join(", "))
            }
        );
    }
    if !yes {
        println!("Rien n'est installé : relancer avec --yes pour accepter ces permissions.");
        return Ok(());
    }
    let row = rpc
        .call(
            "plugin.install",
            json!({"data": data, "digest": shown["digest"]}),
        )
        .await?;
    println!(
        "Installé et activé : {}.",
        row["name"].as_str().unwrap_or(id)
    );
    Ok(())
}

/// Les plugins installés.
pub async fn plugins(rpc: &mut Rpc) -> Result<()> {
    let list = rpc.call("plugin.list", json!({})).await?;
    let items = list["plugins"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        println!("Aucun plugin.");
        return Ok(());
    }
    for p in items {
        println!(
            "{} {} ({}){} — pages : {} ; demandes : {}",
            p["name"].as_str().unwrap_or_default(),
            p["version"].as_str().unwrap_or_default(),
            p["pluginId"].as_str().unwrap_or_default(),
            if p["enabled"] == true {
                ""
            } else {
                ", désactivé"
            },
            domain_list(&p["sites"]),
            domain_list(&p["fetch"]),
        );
    }
    Ok(())
}

pub async fn plugin_remove(rpc: &mut Rpc, id: &str) -> Result<()> {
    rpc.call("plugin.remove", json!({ "pluginId": id })).await?;
    println!("Plugin {id} retiré, avec son fichier.");
    Ok(())
}

pub async fn plugin_enable(rpc: &mut Rpc, id: &str, enabled: bool) -> Result<()> {
    rpc.call("plugin.enable", json!({"pluginId": id, "enabled": enabled}))
        .await?;
    println!(
        "Plugin {id} {}.",
        if enabled { "activé" } else { "désactivé" }
    );
    Ok(())
}

/// Lance le plugin d'une page et montre ses liens, numérotés pour `--select`.
async fn plugin_links(rpc: &mut Rpc, page: &str) -> Result<Value> {
    let found = rpc.call("plugin.resolve", json!({ "url": page })).await?;
    let links = found["links"].as_array().cloned().unwrap_or_default();
    println!(
        "{} : {} lien(s) par « {} »",
        page,
        links.len(),
        found["pluginName"].as_str().unwrap_or_default()
    );
    for (i, link) in links.iter().enumerate() {
        let size = link["size"]
            .as_u64()
            .map_or_else(String::new, |n| format!(" ({})", size(n)));
        println!(
            "  {i}. {}{size}  {}",
            link["name"].as_str().unwrap_or("(sans nom)"),
            link["url"].as_str().unwrap_or_default()
        );
    }
    Ok(found)
}

pub async fn plugin_resolve(rpc: &mut Rpc, page: &str) -> Result<()> {
    plugin_links(rpc, page).await.map(|_| ())
}

/// Ajoute des liens choisis (tous sinon) d'une page, par son plugin.
pub async fn plugin_add(
    rpc: &mut Rpc,
    page: &str,
    select: Option<&[u32]>,
    dir: &Path,
    paused: bool,
) -> Result<()> {
    let found = plugin_links(rpc, page).await?;
    let count = found["links"].as_array().map_or(0, Vec::len);
    let picked: Vec<u64> = match select {
        Some(list) => list.iter().map(|&i| u64::from(i)).collect(),
        None => (0..count as u64).collect(),
    };
    if picked.is_empty() {
        bail!("aucun lien à ajouter");
    }
    let added = rpc
        .call(
            "plugin.add",
            json!({
                "resolutionId": found["resolutionId"],
                "links": picked,
                "destDir": dest_dir(dir)?,
                "startPaused": paused,
            }),
        )
        .await?;
    for task in added["tasks"].as_array().into_iter().flatten() {
        println!(
            "Ajouté : lien {} → tâche {}",
            task["index"],
            task["taskId"].as_str().unwrap_or_default()
        );
    }
    for error in added["errors"].as_array().into_iter().flatten() {
        println!(
            "Refusé : lien {} : {}",
            error["index"],
            error["message"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}
