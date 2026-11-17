//! Commande de fin (F-34, SPEC §7.21), avec de vrais programmes du système : `cmd.exe`
//! sous Windows, `/bin/sh` ailleurs.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::time::timeout;

use crate::net::Client;
use crate::rules::Rule;
use crate::scheduler::{AddTask, Config, DISK_RECHECK, Engine, Event, Planning};
use crate::store::tasks::TaskState;
use crate::testing::{FileSite, Server, content, content_sha256, serve_file};

const WAIT: Duration = Duration::from_secs(30);
const LEN: u64 = 5000;

fn engine(dir: &Path) -> Engine {
    Engine::start(Config {
        db_path: dir.join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: crate::transfer::Options::default(),
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap()
}

fn site() -> Server {
    serve_file(Arc::new(Mutex::new(FileSite::new(LEN))))
}

/// Une commande du système : `script` pour `cmd /c` sous Windows ou `sh -c` ailleurs,
/// suivi d'arguments (sous Linux, `$1`, `$2`… dans le script).
fn shell(windows: &[&str], unix: &str, args: &[&str]) -> Value {
    if cfg!(windows) {
        let mut all = vec!["/c"];
        all.extend_from_slice(windows);
        json!({"program": r"C:\Windows\System32\cmd.exe", "args": all})
    } else {
        let mut all = vec!["-c", unix, "sh"];
        all.extend_from_slice(args);
        json!({"program": "/bin/sh", "args": all})
    }
}

/// Une seule règle, toujours le même identifiant : les tâches gardent leur règle quand sa
/// commande change.
async fn rule(engine: &Engine, command: Value) {
    let rule =
        Rule::from_json(&json!({"ruleId": "regle", "actions": {"command": command}})).unwrap();
    engine.set_rules(vec![rule]).await.unwrap();
}

fn add(server: &Server, dir: &Path, name: &str) -> AddTask {
    AddTask {
        url: server.url(name),
        dest_dir: Some(dir.to_str().unwrap().to_owned()),
        ..AddTask::default()
    }
}

/// États suivis par `task_id` jusqu'à `last`.
async fn states_until(
    events: &mut broadcast::Receiver<Event>,
    task_id: &str,
    last: &[TaskState],
) -> Vec<TaskState> {
    let mut seen = Vec::new();
    let found = timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(Event::StateChanged { task_id: t, to, .. }) if t == task_id => {
                    seen.push(to);
                    if last.contains(&to) {
                        return;
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("moteur arrêté"),
            }
        }
    })
    .await;
    assert!(found.is_ok(), "{task_id} : {seen:?}, jamais {last:?}");
    seen
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap().trim().to_owned()
}

/// La commande reçoit le fichier final (chemin, taille, empreinte, identifiant, en
/// arguments et dans l'environnement) ; la tâche passe par `post_processing`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_command_runs_on_the_final_file() {
    let dir = tempfile::tempdir().unwrap();
    let server = site();
    let engine = engine(dir.path());
    let out = dir.path().join("sortie.txt");
    let out_text = out.to_str().unwrap();
    let command = shell(
        &[
            "echo",
            "{taille}",
            "{sha256}",
            "{id}",
            "%AURO_SIZE%",
            "%AURO_NAME%",
            ">",
            out_text,
        ],
        r#"printf '%s %s %s %s %s' "$1" "$2" "$3" "$AURO_SIZE" "$AURO_NAME" > "$4"; cp "$5" "$5.copie""#,
        &["{taille}", "{sha256}", "{id}", out_text, "{chemin}"],
    );
    rule(&engine, command).await;
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server, dir.path(), "/f.bin"))
        .await
        .unwrap();
    let states = states_until(&mut events, &id, &[TaskState::Completed, TaskState::Failed]).await;
    assert_eq!(
        states,
        [
            TaskState::Active,
            TaskState::PostProcessing,
            TaskState::Completed
        ]
    );
    let words: Vec<String> = read(&out).split_whitespace().map(str::to_owned).collect();
    assert_eq!(
        words,
        [
            LEN.to_string(),
            content_sha256(LEN),
            id.clone(),
            LEN.to_string(),
            "f.bin".to_owned()
        ]
    );
    assert_eq!(
        std::fs::read(dir.path().join("f.bin")).unwrap(),
        content(LEN as usize)
    );
    #[cfg(not(windows))]
    assert!(dir.path().join("f.bin.copie").is_file());
    crate::testing::stop(&engine).await;
}

/// Un code de sortie non nul, puis un délai dépassé : `failed`, `POSTPROCESS_FAILED`, le
/// fichier gardé ; relancée une fois la règle corrigée, la commande seule repart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_command_fails_the_task_and_runs_again_on_resume() {
    let dir = tempfile::tempdir().unwrap();
    let server = site();
    let engine = engine(dir.path());
    rule(&engine, shell(&["exit", "3"], "exit 3", &[])).await;
    let mut events = engine.subscribe();
    let id = engine
        .add(add(&server, dir.path(), "/g.bin"))
        .await
        .unwrap();
    let states = states_until(&mut events, &id, &[TaskState::Completed, TaskState::Failed]).await;
    assert_eq!(states.last(), Some(&TaskState::Failed));
    let task = engine.get(&id).await.unwrap();
    assert_eq!(task.error_code.as_deref(), Some("POSTPROCESS_FAILED"));
    assert!(task.error_detail.unwrap().contains('3'));
    let file = dir.path().join("g.bin");
    assert_eq!(std::fs::read(&file).unwrap(), content(LEN as usize));

    // Délai dépassé : la commande est tuée, avec son arbre.
    let mut slow = shell(&["ping", "-n", "30", "127.0.0.1"], "sleep 30", &[]);
    slow["timeoutS"] = json!(1);
    rule(&engine, slow).await;
    let started = std::time::Instant::now();
    engine.resume(&id).await.unwrap();
    states_until(&mut events, &id, &[TaskState::Completed, TaskState::Failed]).await;
    assert!(started.elapsed() < Duration::from_secs(20));
    let task = engine.get(&id).await.unwrap();
    assert_eq!(task.state, TaskState::Failed);
    assert!(
        task.error_detail.unwrap().contains("délai"),
        "délai annoncé"
    );

    // Commande corrigée : la relance la lance de nouveau, sans rien retélécharger.
    let marker = dir.path().join("relance.txt");
    let marker_text = marker.to_str().unwrap();
    rule(
        &engine,
        shell(
            &["echo", "ok>", marker_text],
            r#"echo ok > "$1""#,
            &[marker_text],
        ),
    )
    .await;
    let requests = server.requests().len();
    engine.resume(&id).await.unwrap();
    let states = states_until(&mut events, &id, &[TaskState::Completed, TaskState::Failed]).await;
    assert_eq!(states.last(), Some(&TaskState::Completed));
    assert_eq!(read(&marker), "ok");
    assert_eq!(server.requests().len(), requests, "rien retéléchargé");
    assert_eq!(std::fs::read(&file).unwrap(), content(LEN as usize));

    // Règle retirée : plus de commande, la tâche suivante se termine seule.
    engine.set_rules(Vec::new()).await.unwrap();
    std::fs::remove_file(&marker).unwrap();
    let other = engine
        .add(add(&server, dir.path(), "/autre.bin"))
        .await
        .unwrap();
    let states = states_until(
        &mut events,
        &other,
        &[TaskState::Completed, TaskState::Failed],
    )
    .await;
    assert_eq!(states, [TaskState::Active, TaskState::Completed]);
    assert!(!marker.exists());
    crate::testing::stop(&engine).await;
}

/// Un arrêt du moteur pendant la commande la tue ; au démarrage suivant, la tâche,
/// toujours `post_processing`, la relance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_cut_by_a_stop_runs_again_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let server = site();
    let marker = dir.path().join("fini.txt");
    let marker_text = marker.to_str().unwrap().to_owned();
    // Lente la première fois (un fichier témoin manque), rapide ensuite.
    let first = dir.path().join("premiere.txt");
    let first_text = first.to_str().unwrap().to_owned();
    let command = shell(
        &[
            "if",
            "exist",
            &first_text,
            "(echo",
            "fini>",
            &format!("{marker_text})"),
            "else",
            "(echo",
            "x>",
            &first_text,
            "&",
            "ping",
            "-n",
            "60",
            "127.0.0.1)",
        ],
        r#"if [ -e "$1" ]; then echo fini > "$2"; else echo x > "$1"; sleep 60; fi"#,
        &[&first_text, &marker_text],
    );
    let id = {
        let engine = engine(dir.path());
        rule(&engine, command).await;
        let mut events = engine.subscribe();
        let id = engine
            .add(add(&server, dir.path(), "/h.bin"))
            .await
            .unwrap();
        states_until(&mut events, &id, &[TaskState::PostProcessing]).await;
        // La commande a démarré (fichier témoin écrit), puis le moteur s'arrête.
        timeout(WAIT, async {
            while !first.exists() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let started = std::time::Instant::now();
        crate::testing::stop(&engine).await;
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "arrêt sans attendre"
        );
        id
    };
    assert!(!marker.exists());
    let engine = engine(dir.path());
    let mut events = engine.subscribe();
    let task = engine.get(&id).await.unwrap();
    if task.state != TaskState::Completed {
        let states =
            states_until(&mut events, &id, &[TaskState::Completed, TaskState::Failed]).await;
        assert_eq!(states.last(), Some(&TaskState::Completed));
    }
    assert_eq!(read(&marker), "fini");
    crate::testing::stop(&engine).await;
}
