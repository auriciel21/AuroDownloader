//! Copie d'un site (SPEC §7.18 ter) sur un faux site local, par le planificateur.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::time::timeout;

use crate::net::Client;
use crate::scheduler::{AddTask, Config, DISK_RECHECK, Engine, Planning, SiteRequest};
use crate::store::site::{self as store_site, ItemState};
use crate::store::tasks::TaskState;
use crate::testing::{Request, Server, reply, serve};
use crate::transfer::Options;

const WAIT: Duration = Duration::from_secs(60);
const HTML: &str = "text/html; charset=utf-8";

fn engine(dir: &Path) -> Engine {
    Engine::start(Config {
        db_path: dir.join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options {
            site_spacing: Duration::from_millis(5),
            ..Options::default()
        },
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap()
}

type Pages = HashMap<String, (String, Vec<u8>)>;

/// Un faux site : `{base}` vaut l'adresse du serveur (`127.0.0.1`), `{other}` la même vue
/// sous un autre nom d'hôte (`localhost`). Un chemin absent répond `404`.
struct Site {
    server: Server,
}

impl Site {
    fn new(pages: &[(&str, &str, &[u8])]) -> Site {
        Self::with(pages, |_| None)
    }

    fn with(
        pages: &[(&str, &str, &[u8])],
        special: impl Fn(&Request) -> Option<crate::testing::Reply> + Send + Sync + 'static,
    ) -> Site {
        let base = Arc::new(OnceLock::<String>::new());
        let table: Arc<Mutex<Pages>> = Arc::default();
        let (b, t) = (Arc::clone(&base), Arc::clone(&table));
        let server = serve(move |r| {
            if let Some(answer) = special(r) {
                return answer;
            }
            let base = b.get().cloned().unwrap_or_default();
            let other = base.replace("127.0.0.1", "localhost");
            match t.lock().unwrap().get(&r.path) {
                Some((kind, body)) => {
                    let body = if kind.starts_with("text/") {
                        String::from_utf8_lossy(body)
                            .replace("{base}", &base)
                            .replace("{other}", &other)
                            .into_bytes()
                    } else {
                        body.clone()
                    };
                    reply(
                        "200 OK",
                        &[
                            ("Content-Type", kind.as_str()),
                            ("Content-Length", &body.len().to_string()),
                        ],
                        &body,
                    )
                }
                None => reply("404 Not Found", &[("Content-Length", "0")], b""),
            }
        });
        base.set(server.base.clone()).unwrap();
        for (path, kind, body) in pages {
            table
                .lock()
                .unwrap()
                .insert((*path).to_owned(), ((*kind).to_owned(), body.to_vec()));
        }
        Site { server }
    }

    fn url(&self, path: &str) -> String {
        self.server.url(path)
    }

    /// Dossier de l'hôte du site dans la copie (`127.0.0.1+port`), et celui de l'autre nom.
    fn hosts(&self) -> (String, String) {
        let url = reqwest::Url::parse(&self.server.base).unwrap();
        let port = url.port().unwrap();
        (format!("127.0.0.1+{port}"), format!("localhost+{port}"))
    }

    fn asked(&self, path: &str) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|r| r.path == path)
            .count()
    }
}

fn request(site: &Site, depth: u8, files: &[&str]) -> SiteRequest {
    SiteRequest {
        page_url: site.url("/"),
        depth: Some(depth),
        files: files.iter().map(|f| site.url(f)).collect(),
        ..SiteRequest::default()
    }
}

async fn add(engine: &Engine, dest: &Path, site: SiteRequest) -> String {
    engine
        .add(AddTask {
            site: Some(site),
            dest_dir: Some(dest.to_string_lossy().into_owned()),
            ..AddTask::default()
        })
        .await
        .unwrap()
}

/// Attend un état terminal (ou celui donné) de la tâche.
async fn wait(engine: &Engine, id: &str, states: &[TaskState]) -> crate::store::tasks::TaskRow {
    timeout(WAIT, async {
        loop {
            let task = engine.get(id).await.unwrap();
            if states.contains(&task.state) {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("copie trop longue")
}

async fn finished(engine: &Engine, id: &str) -> crate::store::tasks::TaskRow {
    wait(engine, id, &[TaskState::Completed, TaskState::Failed]).await
}

fn read(path: &Path) -> String {
    String::from_utf8(std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap()
}

/// Valeurs des attributs `href`, `src` et `srcset` (adresses seules) d'une page copiée.
fn attribute_links(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for key in [" href=\"", " src=\"", " srcset=\""] {
        let mut rest = html;
        while let Some(at) = rest.find(key) {
            rest = &rest[at + key.len()..];
            let end = rest.find('"').unwrap_or(rest.len());
            let value = &rest[..end];
            if key == " srcset=\"" {
                out.extend(
                    value
                        .split(',')
                        .filter_map(|c| c.split_whitespace().next())
                        .map(str::to_owned),
                );
            } else {
                out.push(value.to_owned());
            }
        }
    }
    out
}

/// Les liens locaux des pages et styles copiés : chacun mène à un fichier de la copie, et
/// il ne reste aucun fichier provisoire.
fn assert_local_links_lead_somewhere(root: &Path) {
    let mut checked = 0;
    for path in walk(root) {
        let name = path.to_string_lossy().to_string();
        assert!(
            !name.ends_with(".part") && !name.ends_with(".rewrite"),
            "fichier provisoire resté : {name}"
        );
        let bytes = std::fs::read(&path).unwrap();
        let urls: Vec<String> = if name.ends_with(".css") {
            super::css::refs(&bytes)
                .into_iter()
                .map(|r| r.url)
                .collect()
        } else if name.ends_with(".html") {
            attribute_links(&String::from_utf8_lossy(&bytes))
        } else {
            continue;
        };
        let base = reqwest::Url::from_file_path(&path).unwrap();
        for url in urls {
            if url.contains("://") || url.starts_with('#') || url.starts_with("mailto:") {
                continue;
            }
            let target = base.join(&url).unwrap();
            let file = target.to_file_path().unwrap();
            assert!(file.is_file(), "{name} : lien local sans fichier : {url}");
            checked += 1;
        }
    }
    assert!(checked > 0, "aucun lien local vérifié");
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage";

fn full_site() -> Site {
    let index = br#"<!doctype html><html><head><title>Accueil</title>
<link rel="stylesheet" href="/style.css" integrity="sha384-x">
<script src="app.js"></script></head><body>
<a href="a.html#haut">A</a> <a href="/b/">B</a> <a href="/private/p.html">prive</a>
<img src="/img/logo.png" srcset="/img/logo.png 1x, /img/absent.png 2x">
<img src="{other}/cdn/pic.png">
<a href="/files/doc.pdf">pdf</a> <a href="/files/autre.zip">zip</a>
<a href="{other}/page.html">ailleurs</a> <a href="mailto:x@y">m</a>
</body></html>"#;
    Site::new(&[
        ("/", HTML, index),
        (
            "/a.html",
            HTML,
            br#"<html><head></head><body><a href="/">accueil</a> <a href="/loin.html">loin</a></body></html>"#,
        ),
        (
            "/b/",
            HTML,
            br#"<html><body><img src="../img/logo.png"><a href="/a.html">a</a></body></html>"#,
        ),
        ("/loin.html", HTML, b"<html>niveau 2</html>"),
        ("/private/p.html", HTML, b"<html>interdit</html>"),
        (
            "/style.css",
            "text/css",
            br#"@import "more.css"; body { background: url(img/bg.png) } @font-face { src: url("/fonts/f.woff2") }"#,
        ),
        ("/more.css", "text/css", b"p { color: red }"),
        ("/app.js", "application/javascript", b"console.log('x')"),
        ("/img/logo.png", "image/png", PNG),
        ("/img/bg.png", "image/png", PNG),
        ("/fonts/f.woff2", "font/woff2", b"wOF2"),
        ("/cdn/pic.png", "image/png", PNG),
        ("/files/doc.pdf", "application/pdf", b"%PDF-1.7 document"),
        ("/files/autre.zip", "application/zip", b"PK"),
        ("/robots.txt", "text/plain", b"User-agent: *\nDisallow: /private/\n"),
    ])
}

#[tokio::test]
async fn a_site_is_copied_with_its_resources_and_its_links_rewritten() {
    let site = full_site();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("copies");
    std::fs::create_dir(&dest).unwrap();
    let engine = engine(dir.path());
    let id = add(&engine, &dest, request(&site, 1, &["/files/doc.pdf"])).await;
    let task = finished(&engine, &id).await;
    assert_eq!(task.state, TaskState::Completed, "{:?}", task.error_detail);
    let (main, other) = site.hosts();
    let root = dest.join(task.file_name.as_deref().unwrap());
    assert_eq!(task.file_name.as_deref(), Some(main.as_str()));

    let index = read(&root.join(&main).join("index.html"));
    assert!(index.contains(r#"<a href="a.html#haut">"#), "{index}");
    assert!(index.contains(r#"<a href="b/index.html">"#), "{index}");
    assert!(
        index.contains(r#"<link rel="stylesheet" href="style.css" >"#),
        "{index}"
    );
    assert!(index.contains(r#"<script src="app.js">"#), "{index}");
    assert!(
        index.contains(&format!(
            r#"srcset="img/logo.png 1x, {} 2x""#,
            site.url("/img/absent.png")
        )),
        "{index}"
    );
    assert!(
        index.contains(&format!(r#"<img src="../{other}/cdn/pic.png">"#)),
        "{index}"
    );
    assert!(index.contains(r#"<a href="files/doc.pdf">"#), "{index}");
    assert!(index.contains(&format!(
        r#"<a href="{}">zip"#,
        site.url("/files/autre.zip")
    )));
    assert!(index.contains(&format!(
        r#"<a href="{}">prive"#,
        site.url("/private/p.html")
    )));
    let other_page = site.url("/page.html").replace("127.0.0.1", "localhost");
    assert!(
        index.contains(&format!(r#"<a href="{other_page}">ailleurs"#)),
        "{index}"
    );
    assert!(index.contains(r#"<a href="mailto:x@y">"#));

    let a = read(&root.join(&main).join("a.html"));
    assert!(a.contains(r#"<head><meta charset="utf-8"></head>"#), "{a}");
    assert!(a.contains(r#"<a href="index.html">"#), "{a}");
    assert!(
        a.contains(&format!(r#"<a href="{}">loin"#, site.url("/loin.html"))),
        "{a}"
    );
    let b = read(&root.join(&main).join("b").join("index.html"));
    assert!(b.contains(r#"<img src="../img/logo.png">"#), "{b}");
    assert!(b.contains(r#"<a href="../a.html">"#), "{b}");
    let css = read(&root.join(&main).join("style.css"));
    assert_eq!(
        css,
        r#"@import "more.css"; body { background: url("img/bg.png") } @font-face { src: url("fonts/f.woff2") }"#
    );
    assert_eq!(
        std::fs::read(root.join(&main).join("img/bg.png")).unwrap(),
        PNG
    );
    assert_eq!(
        std::fs::read(root.join(&other).join("cdn/pic.png")).unwrap(),
        PNG
    );
    assert_eq!(
        std::fs::read(root.join(&main).join("files/doc.pdf")).unwrap(),
        b"%PDF-1.7 document"
    );
    assert!(!root.join(&main).join("files/autre.zip").exists());
    assert!(!root.join(&main).join("loin.html").exists());
    assert_local_links_lead_somewhere(&root);
    // robots.txt respecté, page au-delà de la profondeur jamais lue, fichier non coché
    // jamais téléchargé.
    assert_eq!(site.asked("/private/p.html"), 0);
    assert_eq!(site.asked("/loin.html"), 0);
    assert_eq!(site.asked("/files/autre.zip"), 0);
    assert_eq!(site.asked("/img/logo.png"), 1);

    let conn = crate::store::open(&dir.path().join("auro.db")).unwrap();
    let job = store_site::load_job(&conn, &id).unwrap();
    assert_eq!(job.stop.as_deref(), Some("finished"));
    assert_eq!(
        job.start_path.as_deref(),
        Some(format!("{main}/index.html").as_str())
    );
    let counts = store_site::counts(&conn, &id).unwrap();
    assert_eq!((counts.pages, counts.files, counts.failed), (3, 1, 1));
    let history = engine
        .history(crate::store::history::HistoryFilter::default())
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].final_path.as_deref(),
        Some(root.to_str().unwrap())
    );
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn a_second_copy_goes_to_a_new_folder() {
    let site = Site::new(&[("/", HTML, b"<html>x</html>")]);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let first = add(&engine, dir.path(), request(&site, 1, &[])).await;
    let first = finished(&engine, &first).await;
    let second = add(&engine, dir.path(), request(&site, 1, &[])).await;
    let second = finished(&engine, &second).await;
    let (main, _) = site.hosts();
    assert_eq!(first.file_name.as_deref(), Some(main.as_str()));
    assert_eq!(second.file_name, Some(format!("{main} (1)")));
    assert!(
        dir.path()
            .join(format!("{main} (1)"))
            .join(&main)
            .join("index.html")
            .is_file()
    );
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn the_page_limit_stops_reading_pages_but_keeps_their_resources() {
    let mut pages: Vec<(String, String)> = vec![(
        "/".into(),
        (1..=6)
            .map(|i| format!(r#"<a href="/p{i}.html">{i}</a>"#))
            .collect::<String>(),
    )];
    for i in 1..=6 {
        pages.push((format!("/p{i}.html"), format!(r#"<img src="/i{i}.png">"#)));
        pages.push((format!("/i{i}.png"), "png".into()));
    }
    let table: Vec<(&str, &str, &[u8])> = pages
        .iter()
        .map(|(p, b)| {
            let kind = if p.ends_with(".png") {
                "image/png"
            } else {
                HTML
            };
            (p.as_str(), kind, b.as_bytes())
        })
        .collect();
    let site = Site::new(&table);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let id = add(
        &engine,
        dir.path(),
        SiteRequest {
            max_pages: Some(3),
            ..request(&site, 1, &[])
        },
    )
    .await;
    let task = finished(&engine, &id).await;
    assert_eq!(task.state, TaskState::Completed, "{:?}", task.error_detail);
    let conn = crate::store::open(&dir.path().join("auro.db")).unwrap();
    assert_eq!(
        store_site::load_job(&conn, &id).unwrap().stop.as_deref(),
        Some("maxPages")
    );
    let items = store_site::items(&conn, &id).unwrap();
    let read_pages: Vec<_> = items
        .iter()
        .filter(|i| i.kind == store_site::ItemKind::Page && i.state == ItemState::Done)
        .collect();
    assert_eq!(read_pages.len(), 3);
    // Chaque page lue a son image.
    let images = items
        .iter()
        .filter(|i| i.kind == store_site::ItemKind::Resource && i.state == ItemState::Done)
        .count();
    assert_eq!(images, 2);
    let pages_asked = (1..=6)
        .filter(|i| site.asked(&format!("/p{i}.html")) > 0)
        .count();
    assert_eq!(pages_asked, 2);
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn dead_links_and_oversized_resources_stay_online() {
    let html =
        br#"<html><img src="/mort.png"><img src="/enorme.png"><a href="/x.html">x</a></html>"#;
    let site = Site::with(&[("/", HTML, html), ("/x.html", HTML, b"<p>")], |r| {
        (r.path == "/enorme.png").then(|| {
            reply(
                "200 OK",
                &[
                    ("Content-Type", "image/png"),
                    ("Content-Length", "62914560"),
                ],
                b"",
            )
        })
    });
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let id = add(&engine, dir.path(), request(&site, 1, &[])).await;
    let task = finished(&engine, &id).await;
    assert_eq!(task.state, TaskState::Completed, "{:?}", task.error_detail);
    let (main, _) = site.hosts();
    let root = dir.path().join(&main);
    let index = read(&root.join(&main).join("index.html"));
    assert!(
        index.contains(&format!(r#"<img src="{}">"#, site.url("/mort.png"))),
        "{index}"
    );
    assert!(
        index.contains(&format!(r#"<img src="{}">"#, site.url("/enorme.png"))),
        "{index}"
    );
    assert!(index.contains(r#"<a href="x.html">"#), "{index}");
    let conn = crate::store::open(&dir.path().join("auro.db")).unwrap();
    let errors: Vec<_> = store_site::items(&conn, &id)
        .unwrap()
        .into_iter()
        .filter_map(|i| i.error)
        .collect();
    assert_eq!(errors, ["HTTP_STATUS", "TOO_BIG"]);
    assert!(!root.join(&main).join("enorme.png.part").exists());
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn a_start_page_that_is_not_html_fails_the_copy() {
    let site = Site::new(&[("/", "application/zip", b"PK")]);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let id = add(&engine, dir.path(), request(&site, 1, &[])).await;
    let task = finished(&engine, &id).await;
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.error_code.as_deref(), Some("SITE_NOT_HTML"));
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn bad_requests_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let base = SiteRequest {
        page_url: "https://exemple.org/".into(),
        ..SiteRequest::default()
    };
    for bad in [
        SiteRequest {
            depth: Some(0),
            ..base.clone()
        },
        SiteRequest {
            depth: Some(6),
            ..base.clone()
        },
        SiteRequest {
            max_pages: Some(2001),
            ..base.clone()
        },
        SiteRequest {
            max_bytes: Some(1000),
            ..base.clone()
        },
        SiteRequest {
            max_bytes: Some(21 << 30),
            ..base.clone()
        },
        SiteRequest {
            exclude: Some("(".into()),
            ..base.clone()
        },
        SiteRequest {
            files: vec!["ftp://exemple.org/a".into()],
            ..base.clone()
        },
        SiteRequest {
            page_url: "file:///etc/passwd".into(),
            ..base.clone()
        },
    ] {
        let refused = engine
            .add(AddTask {
                site: Some(bad.clone()),
                dest_dir: Some(dir.path().to_string_lossy().into_owned()),
                ..AddTask::default()
            })
            .await;
        assert!(refused.is_err(), "accepté : {bad:?}");
    }
    let with_name = engine
        .add(AddTask {
            site: Some(base),
            file_name: Some("x".into()),
            ..AddTask::default()
        })
        .await;
    assert!(with_name.is_err());
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn a_paused_copy_resumes_where_it_was() {
    let mut body = String::new();
    for i in 0..20 {
        body.push_str(&format!(r#"<img src="/i{i}.png">"#));
    }
    let mut table: Vec<(String, String, Vec<u8>)> =
        vec![("/".into(), HTML.into(), body.into_bytes())];
    for i in 0..20 {
        table.push((
            format!("/i{i}.png"),
            "image/png".into(),
            format!("image {i}").into_bytes(),
        ));
    }
    let rows: Vec<(&str, &str, &[u8])> = table
        .iter()
        .map(|(p, k, b)| (p.as_str(), k.as_str(), b.as_slice()))
        .collect();
    let site = Site::with(&rows, |r| {
        if r.path.ends_with(".png") {
            std::thread::sleep(Duration::from_millis(40));
        }
        None
    });
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let id = add(&engine, dir.path(), request(&site, 1, &[])).await;
    // Quelques images copiées, puis la pause.
    timeout(WAIT, async {
        loop {
            let conn = crate::store::open(&dir.path().join("auro.db")).unwrap();
            if store_site::counts(&conn, &id).unwrap().resources >= 3 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    engine.pause(&id).await.unwrap();
    wait(&engine, &id, &[TaskState::Paused]).await;
    let asked_before: usize = (0..20).map(|i| site.asked(&format!("/i{i}.png"))).sum();
    assert!(asked_before < 20, "tout était lu avant la pause");
    engine.resume(&id).await.unwrap();
    let task = finished(&engine, &id).await;
    assert_eq!(task.state, TaskState::Completed, "{:?}", task.error_detail);
    let (main, _) = site.hosts();
    let root = dir.path().join(&main).join(&main);
    for i in 0..20 {
        assert_eq!(
            std::fs::read(root.join(format!("i{i}.png"))).unwrap(),
            format!("image {i}").into_bytes()
        );
    }
    assert_eq!(site.asked("/"), 1, "la page lue n'est pas relue");
    assert_local_links_lead_somewhere(&dir.path().join(&main));
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn removing_a_copy_deletes_only_what_it_wrote() {
    let site = full_site();
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let id = add(&engine, dir.path(), request(&site, 1, &["/files/doc.pdf"])).await;
    let task = finished(&engine, &id).await;
    assert_eq!(task.state, TaskState::Completed);
    let root: PathBuf = dir.path().join(task.file_name.as_deref().unwrap());
    let (main, _) = site.hosts();
    let mine = root.join(&main).join("img").join("mien.txt");
    std::fs::write(&mine, b"a moi").unwrap();
    engine.remove(&id, true).await.unwrap();
    assert!(mine.is_file(), "un fichier de l'utilisateur a été effacé");
    assert!(!root.join(&main).join("index.html").exists());
    assert!(!root.join(&main).join("files").exists());
    assert!(!root.join(&main).join("img").join("logo.png").exists());
    // Il ne reste que le chemin du fichier gardé.
    let mut left = Vec::new();
    for entry in walk(&root) {
        left.push(entry.strip_prefix(&root).unwrap().to_path_buf());
    }
    assert_eq!(left, [Path::new(&main).join("img").join("mien.txt")]);
    crate::testing::stop(&engine).await;
}

#[tokio::test]
async fn a_restarted_copy_starts_again_from_an_empty_folder() {
    let site = full_site();
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let id = add(&engine, dir.path(), request(&site, 1, &["/files/doc.pdf"])).await;
    let first = finished(&engine, &id).await;
    assert_eq!(first.state, TaskState::Completed);
    engine.restart(&id, false).await.unwrap();
    let again = finished(&engine, &id).await;
    assert_eq!(
        again.state,
        TaskState::Completed,
        "{:?}",
        again.error_detail
    );
    assert_eq!(again.file_name, first.file_name);
    assert_eq!(site.asked("/"), 2);
    let root = dir.path().join(again.file_name.as_deref().unwrap());
    assert_local_links_lead_somewhere(&root);
    crate::testing::stop(&engine).await;
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}
