//! Plugins (SPEC §7.20) : plugins d'essai écrits en texte WebAssembly (`wat`), manifeste
//! ajouté en section personnalisée ; requêtes servies par un serveur local, sous les noms
//! déclarés (`Plugins::resolve_to`).

use std::net::SocketAddr;

use super::runtime::{self, Limits};
use super::{Plugins, digest_of, inspect, links_of, manifest};
use crate::store::plugins::PluginRow;
use crate::testing::plugin_files::{MANIFEST, fetch_call, fixed, packed, plugin, with_section};
use crate::testing::{self, Reply, Request, reply};

fn row_for(bytes: &[u8]) -> PluginRow {
    let m = manifest::read(bytes).unwrap();
    PluginRow {
        id: m.id,
        name: m.name,
        version: m.version,
        author: m.author,
        description: m.description,
        digest: digest_of(bytes),
        sites: m.sites,
        fetch: m.fetch,
        enabled: true,
        installed_at: 0,
    }
}

/// Des plugins rangés dans un dossier temporaire, et la ligne de `bytes`.
fn installed(bytes: &[u8]) -> (tempfile::TempDir, Plugins, PluginRow) {
    let dir = tempfile::tempdir().unwrap();
    let plugins = Plugins::new(dir.path(), Default::default());
    let row = row_for(bytes);
    plugins.save(&row.digest, bytes).unwrap();
    (dir, plugins, row)
}

#[test]
fn a_plugin_file_is_inspected_and_checked() {
    let good = fixed(r#"{"links": []}"#);
    let found = inspect(&good).unwrap();
    assert_eq!(found.manifest.id, "org.essai.fichiers");
    assert_eq!(found.digest, digest_of(&good));
    assert_eq!(found.digest.len(), 64);

    // Un import autre que auro.fetch et auro.log : refusé.
    let other = wat::parse_str(
        r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "auro_alloc") (param i32) (result i32) (i32.const 0))
            (func (export "auro_resolve") (param i32 i32) (result i64) (i64.const 0)))"#,
    )
    .unwrap();
    let other = with_section(other, manifest::SECTION, MANIFEST.as_bytes());
    assert!(inspect(&other).unwrap_err().contains("import non permis"));
    // Export manquant, ou mal typé.
    let missing = with_section(
        wat::parse_str(r#"(module (memory (export "memory") 1))"#).unwrap(),
        manifest::SECTION,
        MANIFEST.as_bytes(),
    );
    assert!(inspect(&missing).unwrap_err().contains("auro_alloc"));
    let typed = with_section(
        wat::parse_str(
            r#"(module (memory (export "memory") 1)
                (func (export "auro_alloc") (param i32) (result i32) (i32.const 0))
                (func (export "auro_resolve") (param i32) (result i32) (i32.const 0)))"#,
        )
        .unwrap(),
        manifest::SECTION,
        MANIFEST.as_bytes(),
    );
    assert!(inspect(&typed).unwrap_err().contains("auro_resolve"));
    // Pas de manifeste.
    let bare = wat::parse_str(r#"(module)"#).unwrap();
    assert!(inspect(&bare).unwrap_err().contains("manifeste"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn links_are_checked_and_names_cleaned() {
    let output = r#"{"links": [
        {"url": "https://cdn.exemple/f.iso", "name": "../f.iso", "size": 12},
        {"url": "https://cdn.exemple/g.iso", "referer": "https://fichiers.exemple/page"},
        {"url": "https://cdn.exemple/h.iso", "referer": "javascript:alert(1)"}]}"#;
    let (_dir, plugins, row) = installed(&fixed(output));
    let r = plugins
        .run(&row, "https://fichiers.exemple/page")
        .await
        .unwrap();
    assert_eq!(r.links.len(), 3);
    assert_eq!(r.links[0].size, Some(12));
    let name = r.links[0].name.clone().unwrap();
    assert!(!name.contains('/') && name.ends_with("f.iso"), "{name}");
    assert_eq!(r.links[1].name, None);
    assert_eq!(
        r.links[1].referer.as_deref(),
        Some("https://fichiers.exemple/page")
    );
    // Un Referer qui n'est pas une adresse http(s) est ignoré.
    assert_eq!(r.links[2].referer, None);
    assert_eq!(r.plugin_id, "org.essai.fichiers");

    // Sorties refusées.
    for (output, wanted) in [
        (
            r#"{"error": "page\u0007 introuvable"}"#,
            "le plugin dit : page introuvable",
        ),
        (r#"{"links": [{"url": "file:///etc/passwd"}]}"#, "http(s)"),
        (r#"{"links": [{"name": "x"}]}"#, "http(s)"),
        (r#"{"autre": 1}"#, "ni links ni error"),
        ("pas du JSON", "illisible"),
    ] {
        let (_dir, plugins, row) = installed(&fixed(output));
        let e = plugins
            .run(&row, "https://fichiers.exemple/page")
            .await
            .unwrap_err();
        assert!(e.contains(wanted), "{output} : {e}");
    }
    let many = format!(
        r#"{{"links": [{}]}}"#,
        vec![r#"{"url": "https://a.exemple/"}"#; super::MAX_LINKS + 1].join(",")
    );
    assert!(links_of(many.as_bytes()).unwrap_err().contains("plus de"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modified_plugin_file_is_refused() {
    let bytes = fixed(r#"{"links": []}"#);
    let (dir, plugins, row) = installed(&bytes);
    let path = dir
        .path()
        .join("plugins")
        .join(format!("{}.wasm", row.digest));
    let mut changed = bytes.clone();
    changed.push(0);
    std::fs::write(&path, changed).unwrap();
    let e = plugins
        .run(&row, "https://fichiers.exemple/")
        .await
        .unwrap_err();
    assert!(e.contains("modifié"), "{e}");
    std::fs::remove_file(&path).unwrap();
    let e = plugins
        .run(&row, "https://fichiers.exemple/")
        .await
        .unwrap_err();
    assert!(e.contains("introuvable"), "{e}");
}

/// Un site d'essai : page qui pose un cookie de session, formulaire qui l'exige,
/// redirection vers un autre domaine, grosse réponse.
fn site() -> testing::Server {
    testing::serve(|request: &Request| -> Reply {
        match request.path.as_str() {
            "/page" => reply(
                "200 OK",
                &[
                    ("Content-Type", "text/html"),
                    ("Set-Cookie", "session=abc; Domain=fichiers.exemple; Path=/"),
                ],
                b"<a href='/form'>dl</a>",
            ),
            "/form" if request.header("Cookie") == Some("session=abc") => {
                reply("200 OK", &[], b"ok")
            }
            "/form" => reply("403 Forbidden", &[], b"sans session"),
            "/ailleurs" => reply(
                "302 Found",
                &[("Location", "http://ailleurs.exemple/secret")],
                b"",
            ),
            "/ici" => reply("302 Found", &[("Location", "/page")], b""),
            "/gros" => reply("200 OK", &[], &vec![b'a'; super::fetch::MAX_BODY + 1]),
            _ => reply("404 Not Found", &[], b""),
        }
    })
}

fn addr_of(server: &testing::Server) -> SocketAddr {
    server.base.trim_start_matches("http://").parse().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_stay_within_declared_domains_and_keep_the_site_cookies() {
    let server = site();
    let page = r#"{"url": "http://fichiers.exemple/page"}"#;
    let form = r#"{"url": "http://www.fichiers.exemple/form", "method": "POST",
        "form": {"op": "telecharger"}, "headers": {"X-Requested-With": "XMLHttpRequest"}}"#;
    let links = r#"{"links": [{"url": "http://cdn.fichiers.exemple/f.iso", "name": "f.iso"},
        {"url": "http://autre.exemple/g.iso"}]}"#;
    // Page, puis formulaire (avec le cookie de la page), puis les liens.
    let resolve = format!(
        "(drop {}) (drop {}) {}",
        fetch_call(1, page),
        fetch_call(2, form),
        packed(0, links)
    );
    let bytes = plugin(MANIFEST, &[links, page, form], &resolve, "");
    let (_dir, plugins, row) = installed(&bytes);
    for name in ["fichiers.exemple", "www.fichiers.exemple"] {
        plugins.resolve_to(name, addr_of(&server));
    }
    let r = plugins
        .run(&row, "http://fichiers.exemple/page")
        .await
        .unwrap();
    let seen = server.requests();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let posted = &seen[1];
    assert_eq!(
        (posted.method.as_str(), posted.path.as_str()),
        ("POST", "/form")
    );
    assert_eq!(posted.header("Cookie"), Some("session=abc"));
    assert_eq!(posted.header("X-Requested-With"), Some("XMLHttpRequest"));
    assert_eq!(
        posted.header("Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
    // Les cookies du site vont au lien de son domaine, pas ailleurs ; jamais au plugin.
    assert_eq!(r.cookie_for(&r.links[0]).as_deref(), Some("session=abc"));
    assert_eq!(r.cookie_for(&r.links[1]), None);

    // Ce que le plugin reçoit, vu par lui : la réponse de fetch rendue telle quelle.
    let echo = |request: &str| {
        plugin(
            MANIFEST,
            &[request],
            &format!("(return {})", fetch_call(0, request)),
            "",
        )
    };
    for (request, wanted) in [
        (
            r#"{"url": "http://ailleurs.exemple/secret"}"#,
            "domaine non déclaré",
        ),
        (
            r#"{"url": "http://fichiers.exemple/ailleurs"}"#,
            "hors des domaines",
        ),
        (
            r#"{"url": "http://fichiers.exemple/page", "headers": {"Cookie": "x=1"}}"#,
            "en-tête non permis",
        ),
        (
            r#"{"url": "http://fichiers.exemple/page", "method": "PUT"}"#,
            "GET ou POST",
        ),
        (
            r#"{"url": "http://fichiers.exemple/page", "form": {"a": "b"}}"#,
            "seulement avec POST",
        ),
        (
            r#"{"url": "http://fichiers.exemple/gros"}"#,
            "réponse de plus de",
        ),
        (r#"{"url": "file:///etc/passwd"}"#, "http et https"),
    ] {
        let (_dir, plugins, row) = installed(&echo(request));
        plugins.resolve_to("fichiers.exemple", addr_of(&server));
        let e = plugins
            .run(&row, "http://fichiers.exemple/page")
            .await
            .unwrap_err();
        assert!(e.contains(wanted), "{request} : {e}");
    }
    // Rien n'est parti vers le domaine non déclaré (le serveur aurait vu /secret).
    assert!(server.requests().iter().all(|r| r.path != "/secret"));
}

#[test]
fn limits_stop_a_hostile_plugin() {
    let small = Limits {
        fuel: 20_000_000,
        memory: 8 << 20,
        depth: 1_000,
    };
    let run = |resolve: &str, extra: &str| {
        let bytes = plugin(MANIFEST, &[], resolve, extra);
        let refuse: runtime::Fetch = Box::new(|_| b"{}".to_vec());
        runtime::resolve(&bytes, b"{}", refuse, &small)
            .output
            .unwrap_err()
    };
    // Boucle sans fin.
    let e = run("(loop $l (br $l)) (unreachable)", "");
    assert!(e.contains("limite de calcul"), "{e}");
    // Mémoire demandée sans fin : refusée au-delà de la limite, le plugin s'arrête.
    let e = run(
        "(loop $l (br_if $l (i32.ne (memory.grow (i32.const 16)) (i32.const -1)))) (unreachable)",
        "",
    );
    assert!(e.contains("s'est arrêté"), "{e}");
    // Récursion sans fond.
    let e = run("(call $deep) (unreachable)", "(func $deep (call $deep))");
    assert!(e.contains("limite de pile"), "{e}");
}

/// Revue de sécurité (étape 5) : une table de fonctions démesurée, déclarée ou agrandie,
/// ne fait pas allouer au moteur plus que sa limite.
#[test]
fn a_huge_table_is_refused() {
    let run = |resolve: &str, extra: &str| {
        let bytes = plugin(MANIFEST, &[], resolve, extra);
        let refuse: runtime::Fetch = Box::new(|_| b"{}".to_vec());
        runtime::resolve(&bytes, b"{}", refuse, &runtime::LIMITS).output
    };
    // Déclarée trop grande : refusée à l'instanciation.
    let e = run("(i64.const 0)", "(table 1000000 funcref)").unwrap_err();
    assert!(e.contains("s'est arrêté"), "{e}");
    let e = run("(i64.const 0)", "(table 4294967295 funcref)").unwrap_err();
    assert!(e.contains("s'est arrêté"), "{e}");
    // Agrandie sans fin : refusée au-delà de la limite, le plugin s'arrête.
    let e = run(
        "(loop $l (br_if $l (i32.ne (table.grow $t (ref.null func) (i32.const 50000)) \
         (i32.const -1)))) (unreachable)",
        "(table $t 1 funcref)",
    )
    .unwrap_err();
    assert!(e.contains("s'est arrêté"), "{e}");
    // Une table de taille ordinaire reste permise.
    assert!(run("(i64.const 0)", "(table 1000 funcref)").is_ok());
}

/// Revue de sécurité (étape 5) : une fonction de démarrage s'exécuterait dès
/// l'instanciation, avant l'accord de l'utilisateur ; un tel module est refusé, à la
/// vérification comme à l'exécution.
#[test]
fn a_start_function_is_refused() {
    let bytes = plugin(
        MANIFEST,
        &[],
        "(i64.const 0)",
        "(func $boucle (loop $l (br $l))) (start $boucle)",
    );
    let e = runtime::check(&bytes).unwrap_err();
    assert!(e.contains("démarrage"), "{e}");
    assert!(crate::plugins::inspect(&bytes).is_err());
    let refuse: runtime::Fetch = Box::new(|_| b"{}".to_vec());
    let e = runtime::resolve(&bytes, b"{}", refuse, &runtime::LIMITS)
        .output
        .unwrap_err();
    assert!(e.contains("démarrage"), "{e}");
}

#[test]
fn matching_finds_the_enabled_plugin_of_a_site() {
    let mut row = row_for(&fixed("{}"));
    let rows = vec![row.clone()];
    assert!(super::matching(&rows, "https://www.fichiers.exemple/f/1").is_some());
    assert!(super::matching(&rows, "https://fichiers.exemple.evil/f").is_none());
    assert!(super::matching(&rows, "ftp://fichiers.exemple/f").is_none());
    row.enabled = false;
    assert!(super::matching(&[row], "https://fichiers.exemple/").is_none());
}

#[test]
fn resolutions_are_kept_for_a_while() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = Plugins::new(dir.path(), Default::default());
    let r = super::Resolution {
        id: "r1".into(),
        plugin_id: "p".into(),
        plugin_name: "P".into(),
        page: "https://fichiers.exemple/".into(),
        links: Vec::new(),
        jar: super::fetch::Jar::default(),
        logs: Vec::new(),
        created: std::time::Instant::now(),
    };
    plugins.keep(r);
    assert!(plugins.kept("r1").is_some());
    assert!(plugins.kept("r2").is_none());
}

// ---------- Fuzzing des lecteurs (fichier ou plugin hostile) ----------

mod fuzz {
    use proptest::prelude::*;

    use super::super::fetch::Jar;
    use super::super::{links_of, manifest};

    /// Un en-tête de module suivi d'octets quelconques, ou des octets quelconques.
    fn wasm_like() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            proptest::collection::vec(any::<u8>(), 0..300),
            proptest::collection::vec(any::<u8>(), 0..300).prop_map(|tail| {
                let mut bytes = b"\0asm\x01\0\0\0".to_vec();
                bytes.extend(tail);
                bytes
            }),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        /// Le lecteur de manifeste ne panique jamais ; ce qu'il accepte est valide.
        #[test]
        fn manifest_reader_never_panics(bytes in wasm_like()) {
            if let Ok(m) = manifest::read(&bytes) {
                prop_assert!(!m.sites.is_empty() && m.sites.iter().all(|d| manifest::valid_domain(d)));
                prop_assert!(m.fetch.iter().all(|d| manifest::valid_domain(d)));
            }
        }

        /// Sortie d'un plugin : jamais de panique ; un lien accepté est une adresse http(s)
        /// et son nom ne quitte jamais son dossier.
        #[test]
        fn plugin_output_is_checked(
            url in "(https?|ftp|file|javascript)://[a-z./%-]{0,30}",
            name in r"\PC{0,40}",
            junk in proptest::collection::vec(any::<u8>(), 0..200),
        ) {
            let _ = links_of(&junk);
            let output = serde_json::json!({"links": [{"url": url, "name": name}]});
            if let Ok(links) = links_of(output.to_string().as_bytes()) {
                prop_assert!(links[0].url.starts_with("http://") || links[0].url.starts_with("https://"));
                if let Some(name) = &links[0].name {
                    prop_assert!(
                        !name.contains('/') && !name.contains('\\') && name != ".." && name != "."
                    );
                }
            }
        }

        /// `Set-Cookie` quelconque : jamais de panique ; un cookie ne part que vers les
        /// hôtes que couvre son domaine.
        #[test]
        fn cookies_never_leak_to_other_hosts(set in "[ -~]{0,80}", host in r"[a-z]{1,8}\.exemple") {
            let mut jar = Jar::default();
            jar.store(&host, &set);
            prop_assert!(jar.header_for("autre.domaine").is_none());
        }
    }
}
