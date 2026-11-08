//! Commandes qui passent par le moteur (`aurod`) et son protocole (SPEC §6).

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::display::{self, Limit, Line, Status, size};
use auro_cli::client::{EngineRefused, Rpc, Target};
use auro_engine::io::checksum::Checksum;

/// Dossier de destination en chemin absolu, qui doit exister.
fn dest_dir(dir: &Path) -> Result<String> {
    let dir = std::path::absolute(dir)?;
    if !dir.is_dir() {
        bail!("le dossier {} n'existe pas", dir.display());
    }
    Ok(dir
        .to_str()
        .context("le chemin du dossier n'est pas en UTF-8")?
        .to_owned())
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
    pub dir: &'a Path,
    pub connections: Option<u32>,
    pub queue: Option<&'a str>,
    /// Limite de débit de la tâche, en octets par seconde.
    pub limit: Option<u64>,
    pub paused: bool,
    /// En-têtes HTTP de la tâche.
    pub headers: &'a [(String, String)],
    /// Empreinte attendue du fichier (F-11).
    pub checksum: Option<&'a Checksum>,
}

async fn add_one(rpc: &mut Rpc, url: &str, opts: &AddOptions<'_>) -> Result<String> {
    let mut params = json!({
        "sources": [{"url": url}],
        "destDir": dest_dir(opts.dir)?,
        "startPaused": opts.paused,
    });
    if let Some(c) = opts.connections {
        params["maxConnections"] = json!(c);
    }
    if let Some(l) = opts.limit {
        params["speedLimitBps"] = json!(l);
    }
    if let Some(c) = opts.checksum {
        params["checksum"] = checksum_json(c);
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
    let added = rpc.call("task.add", params).await?;
    added["taskId"]
        .as_str()
        .map(str::to_owned)
        .context("réponse du moteur sans taskId")
}

pub async fn add(rpc: &mut Rpc, urls: &[String], opts: &AddOptions<'_>) -> Result<()> {
    for url in urls {
        let id = add_one(rpc, url, opts).await?;
        println!("{id}  {url}");
    }
    Ok(())
}

/// Ajoute une tâche et la suit jusqu'à la fin ; Ctrl-C la met en pause.
pub async fn get(target: &Target, url: &str, opts: &AddOptions<'_>) -> Result<()> {
    let mut rpc = auro_cli::client::connect(target).await?;
    // Abonné avant l'ajout : aucun événement de la tâche ne peut manquer. Seule la tâche
    // ajoutée est suivie.
    let mut follow = Follow::start(&mut rpc, Some(HashSet::new())).await?;
    let id = add_one(&mut rpc, url, opts).await?;
    println!("Tâche {id}");
    follow.targets.insert(id.clone());
    follow.refresh(&mut rpc).await?;
    follow.run(&mut rpc, true).await?;
    let task = rpc.call("task.get", json!({"taskId": id})).await?;
    report(&task)
}

/// Bilan d'une tâche suivie jusqu'à son arrêt.
fn report(task: &Value) -> Result<()> {
    let id = task["taskId"].as_str().unwrap_or_default();
    let detail = |t: &Value| {
        let code = t["errorCode"].as_str().unwrap_or("?");
        let text = t["errorDetail"].as_str().unwrap_or("");
        format!("{text} [{code}]")
    };
    match task["state"].as_str().unwrap_or_default() {
        "completed" => {
            let dir = Path::new(task["destDir"].as_str().unwrap_or_default());
            let name = task["fileName"].as_str().unwrap_or_default();
            println!("Terminé : {}", dir.join(name).display());
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
        "failed" if task["errorCode"] == "CHECKSUM_MISMATCH" => bail!(
            "le fichier reçu n'a pas l'empreinte attendue ; il reste sous son nom provisoire \
             (.part). Empreinte à corriger : auro checksum {id} … puis auro resume {id} ; \
             fichier à retélécharger : auro restart {id} --delete-files. {}",
            detail(task)
        ),
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
            "{}  {:<26} {:>22}  {}",
            t["taskId"].as_str().unwrap_or_default(),
            format!("{}{note}", t["state"].as_str().unwrap_or_default()),
            amount,
            t["displayName"].as_str().unwrap_or_default()
        );
    }
    Ok(())
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
    if follow.targets.is_empty() {
        println!("Aucune tâche en cours.");
        return Ok(());
    }
    follow.run(rpc, false).await
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

/// Une tâche suivie.
struct Followed {
    name: String,
    state: String,
    line: Option<Line>,
    /// Octets lus par la vérification de l'empreinte (`verifying`).
    checked: Option<u64>,
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
}

fn running(state: &str) -> bool {
    matches!(state, "queued" | "active" | "retry_wait" | "verifying")
}

impl Follow {
    async fn start(rpc: &mut Rpc, only: Option<HashSet<String>>) -> Result<Follow> {
        rpc.call(
            "events.subscribe",
            json!({"topics": ["tasks", "progress"], "progressIntervalMs": 500}),
        )
        .await?;
        let mut follow = Follow {
            all: only.is_none(),
            targets: only.unwrap_or_default(),
            rev: 0,
            tasks: BTreeMap::new(),
            status: Status::new(),
        };
        follow.refresh(rpc).await?;
        Ok(follow)
    }

    /// Repart d'un instantané complet.
    async fn refresh(&mut self, rpc: &mut Rpc) -> Result<()> {
        let snapshot = rpc.call("sync.snapshot", json!({})).await?;
        self.rev = snapshot["rev"].as_u64().unwrap_or(0);
        for t in snapshot["tasks"].as_array().into_iter().flatten() {
            let id = t["taskId"].as_str().unwrap_or_default().to_owned();
            let state = t["state"].as_str().unwrap_or_default().to_owned();
            if self.all && running(&state) {
                self.targets.insert(id.clone());
            }
            let name = t["displayName"].as_str().unwrap_or_default().to_owned();
            let entry = self.tasks.entry(id).or_insert(Followed {
                name: name.clone(),
                state: state.clone(),
                line: None,
                checked: None,
            });
            entry.name = name;
            entry.state = state;
        }
        Ok(())
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
                if !pause_on_interrupt || interrupted {
                    break;
                }
                interrupted = true;
                let ids: Vec<&String> = self.targets.iter().collect();
                eprintln!("\nMise en pause… (Ctrl-C de nouveau pour quitter sans attendre)");
                if let Err(e) = rpc.call("task.pause", json!({"taskIds": ids})).await {
                    if e.downcast_ref::<EngineRefused>().is_none() {
                        return Err(e);
                    }
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
                            });
                        }
                    }
                }
                // Notifications perdues : on repart d'un état complet.
                "engine.warning" if params["code"] == "EVENTS_LOST" => {
                    self.refresh(rpc).await?;
                }
                _ => {}
            }
            self.render();
        }
        self.status.finish();
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
        println!(
            "{:<40} {}",
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
/// enregistré reste.
pub async fn login_set(
    rpc: &mut Rpc,
    site: &str,
    username: &str,
    password: Option<String>,
) -> Result<()> {
    let mut params = json!({"site": site, "username": username});
    if let Some(existing) = login_of(rpc, site).await? {
        params["loginId"] = existing["loginId"].clone();
    }
    if let Some(password) = password {
        params["password"] = json!(password);
    }
    let saved = rpc.call("login.set", params).await?;
    println!(
        "Identifiants de {} enregistrés ({}) ; le mot de passe est dans le coffre du système.",
        saved["site"].as_str().unwrap_or(site),
        saved["username"].as_str().unwrap_or(username)
    );
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
