//! Commandes qui passent par le moteur (`aurod`) et son protocole (SPEC §6).

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::client::{EngineRefused, Rpc, Target};
use crate::display::{Line, Status, size};

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

/// Identifiant d'une file donnée par son nom ou son identifiant.
async fn queue_id(rpc: &mut Rpc, queue: &str) -> Result<String> {
    let list = rpc.call("queue.list", json!({})).await?;
    list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|q| q["name"] == queue || q["queueId"] == queue)
        .and_then(|q| q["queueId"].as_str().map(str::to_owned))
        .with_context(|| format!("file inconnue : {queue}"))
}

pub struct AddOptions<'a> {
    pub dir: &'a Path,
    pub connections: Option<u32>,
    pub queue: Option<&'a str>,
    pub paused: bool,
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
    if let Some(queue) = opts.queue {
        params["queueId"] = json!(queue_id(rpc, queue).await?);
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
    let mut rpc = crate::client::connect(target).await?;
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
            Some("disk_full") => bail!("disque plein : tâche en pause"),
            other => bail!("tâche en pause ({})", other.unwrap_or("?")),
        },
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

/// `task.pause`, `task.resume` ou `task.cancel` sur plusieurs tâches ; échoue si l'une
/// au moins est refusée, après avoir appliqué les autres.
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

pub async fn queues(rpc: &mut Rpc) -> Result<()> {
    let list = rpc.call("queue.list", json!({})).await?;
    for q in list["items"].as_array().into_iter().flatten() {
        println!(
            "{}  {:<20} {} tâche(s) à la fois",
            q["queueId"].as_str().unwrap_or_default(),
            q["name"].as_str().unwrap_or_default(),
            q["maxActive"]
        );
    }
    Ok(())
}

pub async fn queue_create(rpc: &mut Rpc, name: &str, max_active: Option<u32>) -> Result<()> {
    let mut params = json!({"name": name});
    if let Some(m) = max_active {
        params["maxActive"] = json!(m);
    }
    let q = rpc.call("queue.create", params).await?;
    println!("{}  {}", q["queueId"].as_str().unwrap_or_default(), name);
    Ok(())
}

pub async fn queue_update(
    rpc: &mut Rpc,
    queue: &str,
    name: Option<&str>,
    max_active: Option<u32>,
) -> Result<()> {
    let mut params = json!({"queueId": queue_id(rpc, queue).await?});
    if let Some(n) = name {
        params["name"] = json!(n);
    }
    if let Some(m) = max_active {
        params["maxActive"] = json!(m);
    }
    let q = rpc.call("queue.update", params).await?;
    println!(
        "{}  {}  {} tâche(s) à la fois",
        q["queueId"].as_str().unwrap_or_default(),
        q["name"].as_str().unwrap_or_default(),
        q["maxActive"]
    );
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
