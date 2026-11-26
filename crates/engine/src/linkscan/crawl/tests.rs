//! Exploration d'un site (SPEC §7.18 bis) sur un faux site local.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::time::timeout;

use super::*;
use crate::scheduler::{Config, DISK_RECHECK, LoginChange, Planning};
use crate::testing::{Request, Server, reply, serve};
use crate::transfer::Options;

const WAIT: Duration = Duration::from_secs(30);
/// Rythme des tests : assez court pour aller vite.
const FAST: Duration = Duration::from_millis(5);

fn engine(dir: &Path) -> Engine {
    Engine::start(Config {
        db_path: dir.join("auro.db"),
        client: Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
        transfer: Options::default(),
        disk_recheck: DISK_RECHECK,
        planning: Planning::default(),
    })
    .unwrap()
}

/// Chemin, type et contenu de chaque page.
type Pages = HashMap<String, (String, Vec<u8>)>;

/// Un faux site : chaque chemin, son type et son contenu ; `{base}` y vaut l'adresse du
/// serveur, `{other}` la même vue par un autre nom d'hôte (`localhost`). Un chemin absent
/// répond `404`.
struct Site {
    server: Server,
}

impl Site {
    fn new(pages: &[(&str, &str, &str)]) -> Site {
        Self::with(pages, |_| None)
    }

    /// Avec `special` : une réponse choisie pour certaines requêtes.
    fn with(
        pages: &[(&str, &str, &str)],
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
                    let body = String::from_utf8_lossy(body)
                        .replace("{base}", &base)
                        .replace("{other}", &other);
                    reply(
                        "200 OK",
                        &[
                            ("Content-Type", kind.as_str()),
                            ("Content-Length", &body.len().to_string()),
                        ],
                        body.as_bytes(),
                    )
                }
                None => reply("404 Not Found", &[("Content-Length", "0")], b""),
            }
        });
        base.set(server.base.clone()).unwrap();
        for (path, kind, body) in pages {
            table.lock().unwrap().insert(
                (*path).to_owned(),
                ((*kind).to_owned(), body.as_bytes().to_vec()),
            );
        }
        Site { server }
    }

    fn url(&self, path: &str) -> Url {
        Url::parse(&self.server.url(path)).unwrap()
    }

    fn text(&self, path: &str) -> String {
        self.server.url(path)
    }

    /// Nombre de requêtes à `path`.
    fn asked(&self, path: &str) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|r| r.path == path)
            .count()
    }
}

const HTML: &str = "text/html; charset=utf-8";

fn fast(depth: u8) -> Settings {
    Settings {
        depth,
        spacing: FAST,
        ..Settings::default()
    }
}

/// Attend la fin de l'exploration.
async fn finish(handle: &Handle) -> (Progress, Vec<Found>) {
    let shared = handle.shared();
    let mut changed = shared.watch();
    timeout(WAIT, async {
        while shared.progress().state == State::Running {
            changed.changed().await.unwrap();
        }
    })
    .await
    .expect("exploration trop longue");
    shared.result()
}

async fn crawl(engine: &Engine, start: Url, settings: Settings) -> (Progress, Vec<Found>) {
    let handle = super::start(engine, "essai".into(), start, settings).unwrap();
    finish(&handle).await
}

fn by_name<'a>(found: &'a [Found], name: &str) -> &'a Found {
    found
        .iter()
        .find(|f| f.link.name == name)
        .unwrap_or_else(|| panic!("{name} absent : {found:#?}"))
}

fn visit(found: &[Found], name: &str) -> Option<Visit> {
    by_name(found, name).visit
}

/// Un petit site en boucle : chaque page n'est lue qu'une fois, jusqu'à la profondeur
/// choisie ; les fichiers portent leur niveau et la page qui les cite ; la page de départ
/// n'est pas dans la liste.
#[tokio::test]
async fn pages_are_read_once_down_to_the_chosen_depth() {
    let site = Site::new(&[
        (
            "/",
            HTML,
            r#"<a href="a.html">A</a> <a href="b.html">B</a> <a href="un.zip">1</a>
               <img src="logo.png"> <a href="/">accueil</a>"#,
        ),
        (
            "/a.html",
            HTML,
            r#"<a href="/">retour</a> <a href="c.html">C</a> <a href="deux.zip">2</a>"#,
        ),
        ("/b.html", HTML, r#"<a href="a.html">A</a>"#),
        ("/c.html", HTML, r#"<a href="trois.zip">3</a>"#),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());

    let (progress, found) = crawl(&engine, site.url("/"), fast(1)).await;
    assert_eq!(progress.state, State::Done);
    assert_eq!(progress.stop, Some(Stop::Finished));
    assert_eq!(progress.pages_read, 3, "{found:#?}");
    assert_eq!(progress.page_url.as_deref(), Some(site.text("/").as_str()));
    assert!(found.iter().all(|f| f.link.url != site.text("/")));
    assert_eq!(visit(&found, "a.html"), Some(Visit::Read));
    assert_eq!(visit(&found, "b.html"), Some(Visit::Read));
    assert_eq!(visit(&found, "c.html"), Some(Visit::Depth));
    let un = by_name(&found, "un.zip");
    assert_eq!(
        (un.depth, un.found_on.as_str()),
        (1, site.text("/").as_str())
    );
    assert_eq!(un.visit, None);
    let deux = by_name(&found, "deux.zip");
    assert_eq!(
        (deux.depth, deux.found_on.as_str()),
        (2, site.text("/a.html").as_str())
    );
    assert_eq!(by_name(&found, "logo.png").visit, None);
    assert!(found.iter().all(|f| f.link.name != "trois.zip"));
    assert_eq!(site.asked("/a.html"), 1);
    assert_eq!(site.asked("/c.html"), 0);
    assert_eq!(site.asked("/"), 1);

    let (progress, found) = crawl(&engine, site.url("/"), fast(2)).await;
    assert_eq!(progress.pages_read, 4);
    assert_eq!(visit(&found, "c.html"), Some(Visit::Read));
    let trois = by_name(&found, "trois.zip");
    assert_eq!(
        (trois.depth, trois.found_on.as_str()),
        (3, site.text("/c.html").as_str())
    );
    crate::testing::stop(&engine).await;
}

/// Seules les pages du même hôte se lisent ; les fichiers des autres sont relevés. Avec
/// la case, seulement sous le dossier de départ. Une redirection vers un autre hôte n'est
/// pas lue.
#[tokio::test]
async fn only_the_same_host_is_explored_and_other_files_are_listed() {
    let site = Site::with(
        &[
            (
                "/docs/index.html",
                HTML,
                r#"<a href="{other}/docs/ailleurs.html">x</a> <a href="{other}/f.iso">iso</a>
                   <a href="/autre/page.html">p</a> <a href="sous/page2.html">q</a>
                   <a href="/va">va</a>"#,
            ),
            ("/docs/ailleurs.html", HTML, "rien"),
            ("/autre/page.html", HTML, r#"<a href="g.zip">g</a>"#),
            ("/docs/sous/page2.html", HTML, r#"<a href="h.zip">h</a>"#),
        ],
        |r| {
            (r.path == "/va").then(|| {
                let to = r
                    .header("host")
                    .unwrap_or_default()
                    .replace("127.0.0.1", "localhost");
                reply(
                    "302 Found",
                    &[
                        ("Location", &format!("http://{to}/docs/ailleurs.html")),
                        ("Content-Length", "0"),
                    ],
                    b"",
                )
            })
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());

    let (_, found) = crawl(&engine, site.url("/docs/index.html"), fast(2)).await;
    assert_eq!(visit(&found, "ailleurs.html"), Some(Visit::Offsite));
    assert_eq!(by_name(&found, "f.iso").link.kind, LinkKind::File);
    assert_eq!(visit(&found, "page.html"), Some(Visit::Read));
    assert_eq!(visit(&found, "page2.html"), Some(Visit::Read));
    assert_eq!(visit(&found, "va"), Some(Visit::Offsite));
    by_name(&found, "g.zip");
    // La page de l'autre hôte n'est jamais demandée ; la redirection y mène une fois (le
    // client suit les redirections), mais sa page n'est pas lue.
    assert_eq!(site.asked("/docs/ailleurs.html"), 1);
    assert!(
        found
            .iter()
            .all(|f| f.found_on != site.text("/docs/ailleurs.html"))
    );

    let settings = Settings {
        same_folder: true,
        ..fast(2)
    };
    let (_, found) = crawl(&engine, site.url("/docs/index.html"), settings).await;
    assert_eq!(visit(&found, "page.html"), Some(Visit::Folder));
    assert_eq!(visit(&found, "page2.html"), Some(Visit::Read));
    assert!(found.iter().all(|f| f.link.name != "g.zip"));
    crate::testing::stop(&engine).await;
}

/// robots.txt : ses pages refusées ne sont pas lues, sauf avec la case ; `5xx` ferme le
/// site sauf la page de départ ; absent, tout est permis.
#[tokio::test]
async fn robots_txt_is_respected_unless_ignored() {
    let pages = [
        (
            "/",
            HTML,
            r#"<a href="/prive/p.html">p</a> <a href="/libre.html">l</a>"#,
        ),
        ("/prive/p.html", HTML, r#"<a href="secret.zip">s</a>"#),
        ("/libre.html", HTML, r#"<a href="libre.zip">l</a>"#),
        (
            "/robots.txt",
            "text/plain",
            "User-agent: *\nDisallow: /prive/\n",
        ),
    ];
    let site = Site::new(&pages);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());

    let (_, found) = crawl(&engine, site.url("/"), fast(1)).await;
    assert_eq!(visit(&found, "p.html"), Some(Visit::Robots));
    assert_eq!(visit(&found, "libre.html"), Some(Visit::Read));
    assert_eq!(site.asked("/prive/p.html"), 0);
    assert_eq!(site.asked("/robots.txt"), 1);

    let settings = Settings {
        ignore_robots: true,
        ..fast(1)
    };
    let (_, found) = crawl(&engine, site.url("/"), settings).await;
    assert_eq!(visit(&found, "p.html"), Some(Visit::Read));
    by_name(&found, "secret.zip");
    assert_eq!(site.asked("/robots.txt"), 1, "case cochée : pas lu");

    // Serveur en panne pour robots.txt : rien d'autre que la page de départ.
    let broken = Site::with(&pages[..3], |r| {
        (r.path == "/robots.txt")
            .then(|| reply("503 Service Unavailable", &[("Content-Length", "0")], b""))
    });
    let (progress, found) = crawl(&engine, broken.url("/"), fast(1)).await;
    assert_eq!(progress.pages_read, 1);
    assert_eq!(visit(&found, "libre.html"), Some(Visit::Robots));

    // Absent (404) : tout est permis.
    let open = Site::new(&pages[..3]);
    let (_, found) = crawl(&engine, open.url("/"), fast(1)).await;
    assert_eq!(visit(&found, "p.html"), Some(Visit::Read));
    crate::testing::stop(&engine).await;
}

/// Pages à ne pas ouvrir : l'expression des clients, comparée à l'adresse décodée.
#[tokio::test]
async fn excluded_pages_are_never_opened() {
    let site = Site::new(&[
        (
            "/",
            HTML,
            r#"<a href="/compte/Logout?x=1">sortir</a> <a href="/d%C3%A9connexion">d</a>
               <a href="/page.html">p</a>"#,
        ),
        ("/compte/Logout?x=1", HTML, "déconnecté"),
        ("/d%C3%A9connexion", HTML, "déconnecté"),
        ("/page.html", HTML, "ok"),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let settings = Settings {
        exclude: Some(crate::rules::compile(DEFAULT_EXCLUDE).unwrap()),
        ..fast(1)
    };
    let (_, found) = crawl(&engine, site.url("/"), settings).await;
    assert_eq!(visit(&found, "Logout"), Some(Visit::Excluded));
    assert_eq!(visit(&found, "déconnexion"), Some(Visit::Excluded));
    assert_eq!(visit(&found, "page.html"), Some(Visit::Read));
    assert_eq!(site.asked("/compte/Logout?x=1"), 0);
    assert_eq!(site.asked("/d%C3%A9connexion"), 0);
    crate::testing::stop(&engine).await;
}

/// Un lien de page qui donne un fichier devient un fichier, sans être téléchargé ; une page
/// trop grosse, une erreur : `failed`.
#[tokio::test]
async fn a_page_link_that_gives_a_file_becomes_a_file() {
    let big = "x".repeat(MAX_INPUT + 1);
    let file = vec![7u8; 8 << 20];
    let site = Site::with(
        &[
            (
                "/",
                HTML,
                r#"<a href="/telecharger?id=3">t</a> <a href="/grosse.html">g</a>
                   <a href="/absente.html">a</a>"#,
            ),
            ("/grosse.html", HTML, &big),
        ],
        move |r| {
            (r.path == "/telecharger?id=3").then(|| {
                reply(
                    "200 OK",
                    &[
                        ("Content-Type", "application/zip"),
                        ("Content-Length", &file.len().to_string()),
                    ],
                    &file,
                )
            })
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let (progress, found) = crawl(&engine, site.url("/"), fast(1)).await;
    let t = by_name(&found, "telecharger");
    assert_eq!((t.link.kind, t.visit), (LinkKind::File, None));
    assert_eq!(visit(&found, "grosse.html"), Some(Visit::Failed));
    assert_eq!(visit(&found, "absente.html"), Some(Visit::Failed));
    // Ni le fichier, ni la page trop grosse n'ont été lus en entier.
    assert!(
        progress.bytes_read < 2 * MAX_INPUT as u64,
        "{}",
        progress.bytes_read
    );
    crate::testing::stop(&engine).await;
}

/// La limite de pages arrête l'exploration ; les pages en attente sont marquées.
#[tokio::test]
async fn the_page_limit_stops_the_exploration() {
    let mut pages: Vec<(String, String)> = vec![(
        "/".into(),
        (1..=10)
            .map(|i| format!(r#"<a href="/p{i}.html">{i}</a>"#))
            .collect(),
    )];
    for i in 1..=10 {
        pages.push((
            format!("/p{i}.html"),
            format!(r#"<a href="f{i}.zip">f</a>"#),
        ));
    }
    let table: Vec<(&str, &str, &str)> = pages
        .iter()
        .map(|(p, b)| (p.as_str(), HTML, b.as_str()))
        .collect();
    let site = Site::new(&table);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let settings = Settings {
        max_pages: 4,
        ..fast(1)
    };
    let (progress, found) = crawl(&engine, site.url("/"), settings).await;
    assert_eq!(progress.stop, Some(Stop::MaxPages));
    assert_eq!(progress.pages_read, 4);
    assert_eq!(progress.pages_queued, 0);
    let read = found
        .iter()
        .filter(|f| f.visit == Some(Visit::Read))
        .count();
    let limit = found
        .iter()
        .filter(|f| f.visit == Some(Visit::Limit))
        .count();
    assert_eq!((read, limit), (3, 7), "{found:#?}");
    crate::testing::stop(&engine).await;
}

/// Une page protégée se lit avec les identifiants du site, sur le canal local ; une
/// connexion à jeton explore sans eux.
#[tokio::test]
async fn protected_pages_use_the_site_login_on_the_local_channel_only() {
    let site = Site::with(
        &[
            ("/", HTML, r#"<a href="/prive/index.html">p</a>"#),
            ("/prive/index.html", HTML, r#"<a href="doc.pdf">d</a>"#),
        ],
        |r| {
            if !r.path.starts_with("/prive/") {
                return None;
            }
            let ok = r.header("authorization").is_some_and(|v| {
                crate::net::auth::digest_valid(v, &r.method, "moi", "secret", "n1")
            });
            (!ok).then(|| {
                reply(
                    "401 Unauthorized",
                    &[
                        (
                            "WWW-Authenticate",
                            r#"Digest realm="essai", qop="auth", nonce="n1", algorithm=MD5"#,
                        ),
                        ("Content-Length", "0"),
                    ],
                    b"",
                )
            })
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    engine
        .set_login(LoginChange {
            id: None,
            site: site.text("/prive"),
            username: "moi".into(),
            password: Some("secret".into()),
            allow_plain: None,
            key_path: None,
        })
        .await
        .unwrap();
    let (_, found) = crawl(&engine, site.url("/"), fast(1)).await;
    assert_eq!(visit(&found, "index.html"), Some(Visit::Read));
    by_name(&found, "doc.pdf");

    // Par jeton (réseau privé permis, pour joindre le serveur de test) : sans identifiants.
    let remote = engine.restricted(true).unwrap();
    let (_, found) = crawl(&remote, site.url("/"), fast(1)).await;
    assert_eq!(visit(&found, "index.html"), Some(Visit::Failed));
    crate::testing::stop(&engine).await;
}

/// Rythme (décision) : 2 lectures au plus en même temps, 250 ms entre deux débuts.
#[tokio::test]
async fn pages_are_read_two_at_a_time_250_ms_apart() {
    let starts: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let seen = Arc::clone(&starts);
    let mut pages = vec![(
        "/".to_owned(),
        (1..=5)
            .map(|i| format!(r#"<a href="/p{i}.html">{i}</a>"#))
            .collect::<String>(),
    )];
    for i in 1..=5 {
        pages.push((format!("/p{i}.html"), "fin".to_owned()));
    }
    let table: Vec<(&str, &str, &str)> = pages
        .iter()
        .map(|(p, b)| (p.as_str(), HTML, b.as_str()))
        .collect();
    let site = Site::with(&table, move |r| {
        if r.path.starts_with("/p") {
            seen.lock().unwrap().push(Instant::now());
            // Une lecture lente : les suivantes ne l'attendent pas, jusqu'à deux.
            std::thread::sleep(Duration::from_millis(300));
        }
        None
    });
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let settings = Settings {
        depth: 1,
        ..Settings::default()
    };
    let (progress, _) = crawl(&engine, site.url("/"), settings).await;
    assert_eq!(progress.pages_read, 6);
    let starts = starts.lock().unwrap().clone();
    assert_eq!(starts.len(), 5);
    for pair in starts.windows(2) {
        let gap = pair[1].duration_since(pair[0]);
        assert!(gap >= Duration::from_millis(240), "{gap:?}");
    }
    assert!(site.server.peak_connections() <= 2);
    crate::testing::stop(&engine).await;
}

/// Une page qui répond `429` avec `Retry-After` est redemandée une fois.
#[tokio::test]
async fn a_busy_page_is_asked_again_once() {
    let tries = Arc::new(Mutex::new(0));
    let count = Arc::clone(&tries);
    let site = Site::with(
        &[
            ("/", HTML, r#"<a href="/lente.html">l</a>"#),
            ("/lente.html", HTML, r#"<a href="x.zip">x</a>"#),
        ],
        move |r| {
            if r.path != "/lente.html" {
                return None;
            }
            let mut n = count.lock().unwrap();
            *n += 1;
            (*n == 1).then(|| {
                reply(
                    "429 Too Many Requests",
                    &[("Retry-After", "1"), ("Content-Length", "0")],
                    b"",
                )
            })
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let (_, found) = crawl(&engine, site.url("/"), fast(1)).await;
    assert_eq!(visit(&found, "lente.html"), Some(Visit::Read));
    by_name(&found, "x.zip");
    assert_eq!(*tries.lock().unwrap(), 2);
    crate::testing::stop(&engine).await;
}

/// Une page de départ injoignable, ou qui n'est pas du HTML : l'exploration échoue, avec
/// son code.
#[tokio::test]
async fn a_bad_start_page_fails_the_exploration() {
    let site = Site::new(&[("/f.txt", "text/plain", "texte")]);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let (progress, found) = crawl(&engine, site.url("/f.txt"), fast(1)).await;
    assert_eq!(progress.state, State::Failed);
    assert_eq!(progress.error.unwrap().code, "INVALID_PARAMS");
    assert!(found.is_empty());
    let (progress, _) = crawl(&engine, site.url("/absente"), fast(1)).await;
    assert_eq!(progress.error.unwrap().code, "SOURCE_UNREACHABLE");
    let remote = engine.restricted(false).unwrap();
    let (progress, _) = crawl(&remote, site.url("/"), fast(1)).await;
    assert_eq!(progress.error.unwrap().code, "ADDRESS_FORBIDDEN");
    crate::testing::stop(&engine).await;
}

/// 4 explorations au plus dans le moteur ; une place se rend à la fin, ou quand
/// l'exploration est lâchée.
#[tokio::test]
async fn at_most_four_explorations_run_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let slots: Vec<_> = (0..MAX_RUNNING)
        .map(|_| engine.crawl_slot().unwrap())
        .collect();
    assert!(matches!(
        engine.crawl_slot(),
        Err(EngineError::CrawlBusy(MAX_RUNNING))
    ));
    drop(slots);
    let site = Site::new(&[("/", HTML, "rien")]);
    let handle = super::start(&engine, "a".into(), site.url("/"), fast(1)).unwrap();
    // Lâchée : sa place revient.
    drop(handle);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let slots: Vec<_> = (0..MAX_RUNNING)
        .map(|_| engine.crawl_slot().unwrap())
        .collect();
    drop(slots);
    crate::testing::stop(&engine).await;
}

/// Arrêtée : `cancelled`, ses liens gardés, les pages en attente marquées.
#[tokio::test]
async fn a_cancelled_exploration_keeps_its_links() {
    let mut pages = vec![(
        "/".to_owned(),
        (1..=6)
            .map(|i| format!(r#"<a href="/p{i}.html">{i}</a>"#))
            .collect::<String>(),
    )];
    for i in 1..=6 {
        pages.push((format!("/p{i}.html"), "fin".to_owned()));
    }
    let table: Vec<(&str, &str, &str)> = pages
        .iter()
        .map(|(p, b)| (p.as_str(), HTML, b.as_str()))
        .collect();
    let site = Site::new(&table);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let settings = Settings {
        spacing: Duration::from_secs(2),
        ..fast(1)
    };
    let handle = super::start(&engine, "c".into(), site.url("/"), settings).unwrap();
    let mut changed = handle.shared().watch();
    timeout(WAIT, async {
        while handle.shared().progress().links < 6 {
            changed.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    handle.cancel();
    let (progress, found) = handle.shared().result();
    assert_eq!(progress.state, State::Cancelled);
    assert_eq!(found.len(), 6);
    assert!(found.iter().all(|f| f.visit != Some(Visit::Queued)));
    assert!(found.iter().any(|f| f.visit == Some(Visit::Limit)));
    drop(handle);
    crate::testing::stop(&engine).await;
}

#[test]
fn site_paths_keep_the_tree_of_the_site() {
    assert_eq!(
        site_path("https://exemple.org/docs/a%20b/f.zip"),
        "exemple.org/docs/a b"
    );
    assert_eq!(site_path("https://exemple.org/f.zip"), "exemple.org");
    assert_eq!(
        site_path("http://exemple.org:8080/x/"),
        "exemple.org_8080/x"
    );
    assert_eq!(
        site_path("https://h/1/2/3/4/5/6/7/8/9/f"),
        "h/1/2/3/4/5/6/7"
    );
    assert_eq!(site_path("magnet:?xt=urn:btih:abc"), "");
    // Chaque partie est un nom valide tel quel (`subDir`).
    for part in site_path("https://h/a:b/c*d/%2E%2E/e/f").split('/') {
        assert_eq!(part, sanitize(part, NameRules::native()));
        assert!(part != ".." && part != ".");
    }
}

/// Les pages à ne pas ouvrir par défaut sont celles de la page (`model.js`, `index.html`).
#[test]
fn the_default_exclusion_is_that_of_the_page() {
    let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src");
    let model = std::fs::read_to_string(ui.join("model.js")).unwrap();
    assert!(
        model.contains(&format!(
            "export const DEFAULT_EXCLUDE = \"{DEFAULT_EXCLUDE}\";"
        )),
        "DEFAULT_EXCLUDE diffère de ui/src/model.js"
    );
    let page = std::fs::read_to_string(ui.join("index.html")).unwrap();
    assert!(
        page.contains(&format!("id=\"l-exclude\" type=\"text\" spellcheck=\"false\" autocomplete=\"off\" value=\"{DEFAULT_EXCLUDE}\"")),
        "valeur de l-exclude différente dans ui/src/index.html"
    );
}
