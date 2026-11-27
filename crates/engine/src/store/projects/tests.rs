use super::*;
use crate::store::open_in_memory;

fn spec(name: &str) -> ProjectSpec {
    ProjectSpec {
        name: name.into(),
        page_url: "https://exemple.org/doc/".into(),
        settings: Settings {
            depth: 2,
            max_pages: 200,
            max_bytes: 1 << 30,
            exclude: Some("(?i)logout".into()),
            same_folder: true,
            ignore_robots: false,
        },
        filters: Filters {
            extensions: vec!["pdf".into(), "zip".into()],
            regex: Some("rapport".into()),
            min_size: Some(1024),
            max_size: None,
        },
        copy_site: false,
        keep_tree: true,
        dest_dir: Some("/tmp/projets".into()),
        queue_id: None,
        schedule: Some(Schedule {
            minute: 9 * 60 + 30,
            days: 0b001_1111,
        }),
    }
}

#[test]
fn a_project_is_saved_read_and_replaced_keeping_its_task() {
    let conn = open_in_memory().unwrap();
    let id = create(&conn, &spec("Docs"), Some(5_000), false).unwrap();
    let saved = get(&conn, &id).unwrap().unwrap();
    assert_eq!(saved.spec, spec("Docs"));
    assert_eq!(saved.next_run_at, Some(5_000));
    assert_eq!((saved.task_id, saved.last_run_at), (None, None));
    assert!(
        name_taken(&conn, "docs", None).unwrap(),
        "sans tenir compte de la casse"
    );
    assert!(!name_taken(&conn, "Docs", Some(&id)).unwrap(), "lui-même");

    // Une tâche (la clé étrangère l'exige), son lancement, puis un remplacement.
    let queue = crate::store::tasks::ensure_queue(&conn, "default").unwrap();
    crate::store::tests::insert_task(&conn, "t1", &queue, "completed").unwrap();
    mark_run(&conn, &id, "t1", 7_000).unwrap();
    let mut other = spec("Docs");
    other.copy_site = true;
    other.filters = Filters::default();
    other.schedule = None;
    assert!(replace(&conn, &id, &other, None).unwrap());
    let saved = get(&conn, &id).unwrap().unwrap();
    assert_eq!(saved.spec, other);
    assert_eq!(saved.task_id.as_deref(), Some("t1"));
    assert_eq!(saved.last_run_at, Some(7_000));
    assert!(!replace(&conn, "absent", &other, None).unwrap());

    // La tâche retirée : le projet reste, sans tâche.
    conn.execute("DELETE FROM tasks WHERE id = 't1'", [])
        .unwrap();
    assert_eq!(get(&conn, &id).unwrap().unwrap().task_id, None);
    assert!(delete(&conn, &id).unwrap());
    assert!(!delete(&conn, &id).unwrap());
}

#[test]
fn due_projects_come_in_order_and_a_one_time_schedule_is_cleared() {
    let conn = open_in_memory().unwrap();
    let late = create(&conn, &spec("B"), Some(2_000), false).unwrap();
    let early = create(&conn, &spec("A"), Some(1_000), true).unwrap();
    let _manual = create(
        &conn,
        &ProjectSpec {
            schedule: None,
            ..spec("C")
        },
        None,
        false,
    )
    .unwrap();
    let due_ids =
        |now| -> Vec<String> { due(&conn, now).unwrap().into_iter().map(|p| p.id).collect() };
    assert!(due_ids(999).is_empty());
    assert_eq!(due_ids(2_000), [early.clone(), late.clone()]);
    assert_eq!(next_due(&conn).unwrap(), Some(1_000));
    assert!(get(&conn, &early).unwrap().unwrap().public_only);

    mark_skipped(&conn, &late, 2_100).unwrap();
    assert_eq!(
        get(&conn, &late).unwrap().unwrap().last_skipped_at,
        Some(2_100)
    );
    set_next_run(&conn, &late, Some(9_000)).unwrap();
    set_next_run(&conn, &early, None).unwrap();
    let early = get(&conn, &early).unwrap().unwrap();
    assert_eq!((early.next_run_at, early.spec.schedule), (None, None));
    assert_eq!(due_ids(5_000), Vec::<String>::new());
    assert_eq!(next_due(&conn).unwrap(), Some(9_000));
    let names: Vec<String> = list(&conn)
        .unwrap()
        .into_iter()
        .map(|p| p.spec.name)
        .collect();
    assert_eq!(names, ["A", "B", "C"]);
}
