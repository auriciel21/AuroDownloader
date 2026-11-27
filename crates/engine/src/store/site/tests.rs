use super::*;
use crate::store::{open_in_memory, tasks};

fn settings() -> Settings {
    Settings {
        depth: 2,
        max_pages: 200,
        max_bytes: 1 << 30,
        exclude: Some("(?i)logout".into()),
        same_folder: false,
        ignore_robots: false,
    }
}

fn create(conn: &mut Connection, files: &[String]) -> String {
    let queue = tasks::ensure_queue(conn, "default").unwrap();
    let s = settings();
    create_site_task(
        conn,
        &NewSite {
            queue_id: &queue,
            page_url: "https://exemple.org/doc/",
            display_name: "exemple.org",
            dest_dir: "/tmp/copies",
            speed_limit_bps: None,
            max_connections: Some(4),
            idempotency_key: None,
            settings: &s,
            files,
            public_only: false,
        },
        tasks::Start::default(),
    )
    .unwrap()
}

fn page<'a>(url: &'a str, depth: u8) -> NewItem<'a> {
    NewItem {
        url,
        kind: ItemKind::Page,
        depth: Some(depth),
    }
}

fn resource(url: &str) -> NewItem<'_> {
    NewItem {
        url,
        kind: ItemKind::Resource,
        depth: None,
    }
}

#[test]
fn a_copy_starts_with_its_page_then_the_ticked_files() {
    let mut conn = open_in_memory().unwrap();
    let files = vec![
        "https://exemple.org/a.zip".to_string(),
        "https://cdn.exemple.net/b.pdf".to_string(),
    ];
    let id = create(&mut conn, &files);
    let (kind, state, name): (String, String, String) = conn
        .query_row(
            "SELECT kind, state, display_name FROM tasks WHERE id = ?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (kind.as_str(), state.as_str(), name.as_str()),
        ("site", "queued", "exemple.org")
    );
    let job = load_job(&conn, &id).unwrap();
    assert_eq!(job.page_url, "https://exemple.org/doc/");
    assert_eq!(job.settings, settings());
    assert_eq!(
        (job.phase, job.stop, job.start_path),
        (Phase::Copy, None, None)
    );
    let items = items(&conn, &id).unwrap();
    let seen: Vec<_> = items
        .iter()
        .map(|i| (i.seq, i.url.as_str(), i.kind, i.depth, i.state))
        .collect();
    assert_eq!(
        seen,
        vec![
            (
                0,
                "https://exemple.org/doc/",
                ItemKind::Page,
                Some(0),
                ItemState::Queued
            ),
            (
                1,
                "https://exemple.org/a.zip",
                ItemKind::File,
                None,
                ItemState::Queued
            ),
            (
                2,
                "https://cdn.exemple.net/b.pdf",
                ItemKind::File,
                None,
                ItemState::Queued
            ),
        ]
    );
}

#[test]
fn a_page_read_queues_its_links_once_in_the_same_transaction() {
    let mut conn = open_in_memory().unwrap();
    let id = create(&mut conn, &[]);
    let found = [
        page("https://exemple.org/doc/a.html", 1),
        resource("https://exemple.org/logo.png"),
        page("https://exemple.org/doc/a.html", 1),
        page("https://exemple.org/doc/", 1),
    ];
    let added = fetched(
        &mut conn,
        &id,
        0,
        Some("https://exemple.org/doc/"),
        1200,
        Some("utf-8"),
        &found,
    )
    .unwrap();
    assert_eq!(
        added,
        Added {
            added: 2,
            full: false
        }
    );
    let items = items(&conn, &id).unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].state, ItemState::Fetched);
    assert_eq!(items[0].size, Some(1200));
    assert_eq!(items[0].charset.as_deref(), Some("utf-8"));
    assert_eq!(items[1].seq, 1);
    assert_eq!(items[2].url, "https://exemple.org/logo.png");
    done(&conn, &id, 2, None, 300).unwrap();
    give_up(&conn, &id, 1, ItemState::Failed, Some("HTTP_STATUS")).unwrap();
    let c = counts(&conn, &id).unwrap();
    assert_eq!(
        c,
        Counts {
            pages: 1,
            resources: 1,
            files: 0,
            failed: 1,
            online: 0,
            skipped: 0,
            queued: 0,
            bytes: 1500,
        }
    );
    // Pas une page web : laissé en ligne, compté hors des échecs.
    give_up(&conn, &id, 1, ItemState::Failed, Some(LEFT_ONLINE)).unwrap();
    let c = counts(&conn, &id).unwrap();
    assert_eq!((c.failed, c.online), (0, 1));
    give_up(&conn, &id, 1, ItemState::Failed, Some("HTTP_STATUS")).unwrap();
    assert_eq!(
        item(&conn, &id, 1).unwrap().error.as_deref(),
        Some("HTTP_STATUS")
    );
}

#[test]
fn items_stop_at_the_limit_and_say_so() {
    let mut conn = open_in_memory().unwrap();
    let id = create(&mut conn, &[]);
    let urls: Vec<String> = (0..MAX_ITEMS + 5)
        .map(|i| format!("https://exemple.org/i/{i}.png"))
        .collect();
    let found: Vec<NewItem<'_>> = urls.iter().map(|u| resource(u)).collect();
    let tx = conn.transaction().unwrap();
    let added = add_items(&tx, &id, &found).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        added,
        Added {
            added: MAX_ITEMS - 1,
            full: true
        }
    );
    // Une adresse déjà connue n'est pas un refus.
    let again = add_items(&conn, &id, &[resource(&urls[3])]).unwrap();
    assert_eq!(
        again,
        Added {
            added: 0,
            full: false
        }
    );
    let refused = add_items(&conn, &id, &[resource("https://exemple.org/neuf.png")]).unwrap();
    assert_eq!(
        refused,
        Added {
            added: 0,
            full: true
        }
    );
}

#[test]
fn a_path_is_recorded_with_the_release_of_its_claim() {
    let mut conn = open_in_memory().unwrap();
    let id = create(&mut conn, &[]);
    crate::store::claims::insert(
        &conn,
        "/tmp/copies/exemple.org",
        "index.html",
        crate::store::claims::ClaimKind::Part,
        &id,
    )
    .unwrap();
    set_path(&mut conn, &id, 0, "exemple.org/doc/index.html").unwrap();
    assert!(crate::store::claims::all(&conn).unwrap().is_empty());
    assert_eq!(
        item(&conn, &id, 0).unwrap().path.as_deref(),
        Some("exemple.org/doc/index.html")
    );
}

#[test]
fn a_limit_skips_what_waits_but_keeps_the_ticked_files() {
    let mut conn = open_in_memory().unwrap();
    let id = create(&mut conn, &["https://exemple.org/a.zip".to_string()]);
    add_items(&conn, &id, &[page("https://exemple.org/doc/b.html", 1)]).unwrap();
    add_items(&conn, &id, &[resource("https://exemple.org/logo.png")]).unwrap();
    // Limite de pages : les pages seulement ; les ressources des pages lues restent.
    assert_eq!(skip_queued(&conn, &id, true).unwrap(), 2);
    assert_eq!(skip_queued(&conn, &id, false).unwrap(), 1);
    let states: Vec<_> = items(&conn, &id).unwrap().iter().map(|i| i.state).collect();
    assert_eq!(
        states,
        [
            ItemState::Skipped,
            ItemState::Queued,
            ItemState::Skipped,
            ItemState::Skipped
        ]
    );
    set_stop(&conn, &id, "maxBytes").unwrap();
    set_stop(&conn, &id, "finished").unwrap();
    assert_eq!(
        load_job(&conn, &id).unwrap().stop.as_deref(),
        Some("maxBytes")
    );
}

#[test]
fn a_restart_starts_again_with_the_same_files_and_no_children() {
    let mut conn = open_in_memory().unwrap();
    let id = create(&mut conn, &["https://exemple.org/a.zip".to_string()]);
    let queue = tasks::ensure_queue(&conn, "default").unwrap();
    let (child, _) = tasks::create_task(
        &mut conn,
        &tasks::NewTask {
            queue_id: &queue,
            url: "https://exemple.org/a.zip",
            display_name: "a.zip",
            dest_dir: "/tmp/copies",
            max_connections: None,
            speed_limit_bps: None,
            idempotency_key: None,
            requested_name: Some("a.zip"),
            checksum: None,
            headers: &[],
            parent_id: Some(&id),
            public_only: false,
        },
    )
    .unwrap();
    set_child(&conn, &id, 1, &child).unwrap();
    assert_eq!(children(&conn, &id).unwrap(), std::slice::from_ref(&child));
    fetched(
        &mut conn,
        &id,
        0,
        None,
        10,
        None,
        &[page("https://exemple.org/doc/b.html", 1)],
    )
    .unwrap();
    set_start(
        &conn,
        &id,
        "https://exemple.org/doc/",
        "exemple.org/doc/index.html",
    )
    .unwrap();
    set_phase(&conn, &id, Phase::Links).unwrap();
    set_stop(&conn, &id, "finished").unwrap();

    reset(&mut conn, &id).unwrap();
    let job = load_job(&conn, &id).unwrap();
    assert_eq!(
        (job.phase, job.stop, job.start_url),
        (Phase::Copy, None, None)
    );
    let items = items(&conn, &id).unwrap();
    let seen: Vec<_> = items
        .iter()
        .map(|i| (i.url.as_str(), i.kind, i.state, i.child_id.as_deref()))
        .collect();
    assert_eq!(
        seen,
        vec![
            (
                "https://exemple.org/doc/",
                ItemKind::Page,
                ItemState::Queued,
                None
            ),
            (
                "https://exemple.org/a.zip",
                ItemKind::File,
                ItemState::Queued,
                None
            ),
        ]
    );
    assert!(children(&conn, &id).unwrap().is_empty());
}

#[test]
fn removing_the_task_removes_its_copy_rows() {
    let mut conn = open_in_memory().unwrap();
    let id = create(&mut conn, &["https://exemple.org/a.zip".to_string()]);
    conn.execute("DELETE FROM tasks WHERE id = ?1", [&id])
        .unwrap();
    let left: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM site_jobs) + (SELECT COUNT(*) FROM site_items)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
}
