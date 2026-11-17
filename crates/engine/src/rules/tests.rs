use std::path::Path;

use serde_json::json;

use super::*;

fn rule(value: Value) -> Rule {
    Rule::from_json(&value).unwrap()
}

fn subject<'a>(url: &'a str, host: Option<&'a str>, name: &'a str) -> Subject<'a> {
    Subject { url, host, name }
}

fn auto(base: &Path) -> Scope<'_> {
    Scope {
        auto_dest: true,
        base,
        rename: true,
        date: "2026-11-16",
    }
}

/// Les types de fichier sont ceux de la page : mêmes catégories, mêmes extensions.
#[test]
fn categories_are_those_of_the_page() {
    let model = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/model.js"),
    )
    .unwrap();
    let cats: Vec<String> = CATEGORIES.iter().map(|c| format!("\"{c}\"")).collect();
    assert!(
        model.contains(&format!("export const CATEGORIES = [{}];", cats.join(", "))),
        "CATEGORIES diffère de ui/src/model.js"
    );
    for (cat, list) in EXTENSIONS {
        assert!(
            model.contains(&format!("  {cat}: \"{list}\",")),
            "extensions de {cat} différentes de ui/src/model.js"
        );
    }
    assert_eq!(category("film.MKV"), "video");
    assert_eq!(category("a.tar.gz"), "compressed");
    assert_eq!(category("archive.r01"), "compressed");
    assert_eq!(category("archive.7z.001"), "compressed");
    assert_eq!(category(".bashrc"), "other");
    assert_eq!(category("LISEZMOI"), "other");
}

#[test]
fn every_criterion_must_match() {
    let r = rule(json!({
        "match": {"categories": ["programs"], "domains": ["debian.org"], "nameRegex": "^debian-"}
    }));
    let url = "https://cdimage.debian.org/x/debian-13.iso";
    assert!(
        r.criteria
            .matches(&subject(url, Some("cdimage.debian.org"), "debian-13.iso"))
    );
    assert!(
        r.criteria
            .matches(&subject(url, Some("debian.org"), "debian-13.iso"))
    );
    // Pas un sous-domaine : « notdebian.org ».
    assert!(
        !r.criteria
            .matches(&subject(url, Some("notdebian.org"), "debian-13.iso"))
    );
    assert!(
        !r.criteria
            .matches(&subject(url, Some("debian.org"), "debian-13.zip"))
    );
    assert!(
        !r.criteria
            .matches(&subject(url, Some("debian.org"), "ubuntu.iso"))
    );
    // Un torrent n'a pas d'hôte : une règle à domaine ne le prend pas.
    assert!(
        !r.criteria
            .matches(&subject("magnet:?xt=…", None, "debian-13.iso"))
    );
    // Sans critère : tout.
    assert!(
        rule(json!({}))
            .criteria
            .matches(&subject("magnet:?", None, "x"))
    );
}

#[test]
fn type_is_a_category_or_an_extension() {
    let r = rule(json!({"match": {"categories": ["video"], "extensions": [".ISO"]}}));
    assert_eq!(r.criteria.extensions, ["iso"]);
    let host = Some("h.example");
    assert!(
        r.criteria
            .matches(&subject("https://h.example/a", host, "a.webm"))
    );
    assert!(
        r.criteria
            .matches(&subject("https://h.example/a", host, "A.Iso"))
    );
    assert!(
        !r.criteria
            .matches(&subject("https://h.example/a", host, "a.pdf"))
    );
    assert!(
        !r.criteria
            .matches(&subject("https://h.example/a", host, "iso"))
    );
}

#[test]
fn url_expression_sees_the_whole_address() {
    let r = rule(json!({"match": {"urlRegex": "(?i)/releases/"}}));
    let host = Some("example.org");
    assert!(r.criteria.matches(&subject(
        "https://example.org/RELEASES/a.zip",
        host,
        "a.zip"
    )));
    assert!(
        !r.criteria
            .matches(&subject("https://example.org/nightly/a.zip", host, "a.zip"))
    );
}

#[test]
fn the_first_enabled_rule_wins_alone() {
    let rules = vec![
        Rule {
            id: "off".into(),
            ..rule(json!({"enabled": false, "actions": {"folder": "Éteinte"}}))
        },
        Rule {
            id: "videos".into(),
            ..rule(json!({"match": {"categories": ["video"]}, "actions": {"folder": "Vidéos"}}))
        },
        Rule {
            id: "all".into(),
            ..rule(json!({"actions": {"folder": "Divers", "rename": "{date} {nom}.{ext}"}}))
        },
    ];
    let base = Path::new("/dl");
    let s = subject("https://v.example/f.mp4", Some("v.example"), "f.mp4");
    let p = place(&rules, &s, &auto(base));
    assert_eq!(p.rule_id.as_deref(), Some("videos"));
    assert_eq!(p.dir, Some(base.join("Vidéos")));
    assert_eq!(p.name, None, "la règle des vidéos ne renomme pas");
    let s = subject("https://v.example/f.pdf", Some("v.example"), "f.pdf");
    let p = place(&rules, &s, &auto(base));
    assert_eq!(p.rule_id.as_deref(), Some("all"));
    assert_eq!(p.dir, Some(base.join("Divers")));
    assert_eq!(p.name.as_deref(), Some("2026-11-16 f.pdf"));
}

/// Une tâche ajoutée avec un dossier garde sa règle (sa commande), pas son rangement ; un
/// nom choisi l'emporte sur le modèle.
#[test]
fn a_chosen_folder_or_name_wins() {
    let rules = vec![Rule {
        id: "r".into(),
        ..rule(json!({"actions": {"folder": ELSEWHERE, "rename": "x-{nom}.{ext}"}}))
    }];
    let base = Path::new("/dl");
    let s = subject("https://h.example/a.bin", Some("h.example"), "a.bin");
    let chosen = Scope {
        auto_dest: false,
        ..auto(base)
    };
    assert_eq!(
        place(&rules, &s, &chosen),
        Placement {
            rule_id: Some("r".into()),
            dir: None,
            name: None
        }
    );
    let named = Scope {
        rename: false,
        ..auto(base)
    };
    let p = place(&rules, &s, &named);
    assert_eq!(p.dir, Some(Path::new(ELSEWHERE).to_path_buf()));
    assert_eq!(p.name, None);
    assert_eq!(place(&[], &s, &auto(base)), Placement::default());
}

#[test]
fn name_templates() {
    let host = Some("files.example.org");
    let r = |t: &str, name: &str| render_name(t, name, host, "2026-11-16");
    assert_eq!(
        r("{hôte}-{nom}.{ext}", "a.tar.gz").as_deref(),
        Some("files.example.org-a.tar.gz")
    );
    assert_eq!(
        r("{host}_{name}.{ext}", "LISEZMOI").as_deref(),
        Some("files.example.org_LISEZMOI")
    );
    assert_eq!(r("{date}", "a.iso").as_deref(), Some("2026-11-16"));
    // Le nom du serveur reste nettoyé : rien ne sort du dossier.
    let out = r("{nom}.{ext}", "..").unwrap();
    assert_ne!(out, "..");
    assert_eq!(r("  ", "a.iso"), None);
    assert_eq!(
        render_name("{hôte}", "a", None, "d"),
        None,
        "vide : le nom d'origine reste"
    );
}

#[test]
fn folders() {
    let base = Path::new("/dl");
    assert_eq!(
        folder_dir(base, "Vidéos/Séries"),
        base.join("Vidéos").join("Séries")
    );
    #[cfg(windows)]
    let abs = r"D:\Rangement";
    #[cfg(not(windows))]
    let abs = "/srv/rangement";
    assert_eq!(folder_dir(base, abs), Path::new(abs));
    assert!(check_folder(abs).is_ok());
    for bad in [
        "../haut",
        "a/../b",
        "a//b",
        "./a",
        "a/b/c/d/e/f/g/h/i",
        "a/\u{1}",
    ] {
        assert!(check_folder(bad).is_err(), "{bad:?}");
    }
    #[cfg(windows)]
    for bad in ["a\\b", "CON", "a:b"] {
        assert!(check_folder(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn bad_rules_are_refused() {
    let bad = [
        json!([]),
        json!({"surprise": 1}),
        json!({"name": "x".repeat(101)}),
        json!({"enabled": "oui"}),
        json!({"match": {"categories": ["films"]}}),
        json!({"match": {"extensions": ["i so"]}}),
        json!({"match": {"extensions": ["x".repeat(17)]}}),
        json!({"match": {"domains": ["192.168.1.1"]}}),
        json!({"match": {"domains": ["*.example.org"]}}),
        json!({"match": {"domains": ["exa mple.org"]}}),
        json!({"match": {"domains": vec!["a.org"; 51]}}),
        json!({"match": {"urlRegex": "("}}),
        json!({"match": {"urlRegex": "a".repeat(1001)}}),
        // Automate démesuré : refusé à l'écriture, jamais compilé sans borne.
        json!({"match": {"nameRegex": "\\w{1000}\\w{1000}\\w{1000}"}}),
        json!({"match": {"autre": []}}),
        json!({"actions": {"rename": "{inconnue}"}}),
        json!({"actions": {"rename": "{nom"}}),
        json!({"actions": {"rename": "a}b"}}),
        json!({"actions": {"rename": "a/{nom}"}}),
        json!({"actions": {"rename": "a\\b"}}),
        json!({"actions": {"folder": "../x"}}),
        json!({"actions": {"command": {"program": "relatif.exe"}}}),
        json!({"actions": {"command": {"args": ["{chemin}"]}}}),
        json!({"actions": {"command": {"program": ABS, "args": ["{inconnue}"]}}}),
        json!({"actions": {"command": {"program": ABS, "args": "{chemin}"}}}),
        json!({"actions": {"command": {"program": ABS, "args": vec!["a"; 65]}}}),
        json!({"actions": {"command": {"program": ABS, "args": ["a\u{0}b"]}}}),
        json!({"actions": {"command": {"program": ABS, "timeoutS": 0}}}),
        json!({"actions": {"command": {"program": ABS, "timeoutS": 86_401}}}),
        json!({"actions": {"command": {"program": ABS, "shell": true}}}),
    ];
    for value in bad {
        assert!(Rule::from_json(&value).is_err(), "acceptée : {value}");
    }
}

#[cfg(windows)]
const ELSEWHERE: &str = r"D:\Ailleurs";
#[cfg(not(windows))]
const ELSEWHERE: &str = "/ailleurs";

#[cfg(windows)]
const ABS: &str = r"C:\Outils\analyse.exe";
#[cfg(not(windows))]
const ABS: &str = "/usr/bin/analyse";

#[test]
fn a_rule_round_trips_and_hides_its_command_from_tokens() {
    let value = json!({
        "ruleId": "r1",
        "name": "Images d'Ubuntu",
        "enabled": true,
        "match": {
            "categories": ["programs"],
            "extensions": ["iso"],
            "domains": ["ubuntu.com"],
            "urlRegex": "releases",
            "nameRegex": null
        },
        "actions": {
            "folder": "ISO/Ubuntu",
            "rename": "{nom}-{date}.{ext}",
            "command": {"program": ABS, "args": ["--scan", "{chemin}", "{sha256}"], "timeoutS": 60}
        }
    });
    let r = rule(value.clone());
    assert_eq!(r.to_json(true), value);
    let token = r.to_json(false);
    assert!(token["actions"].get("command").is_none());
    assert_eq!(token["actions"]["hasCommand"], true);
    // Valeurs par défaut.
    let d = rule(json!({"actions": {"command": {"program": ABS}}}));
    assert!(d.enabled);
    assert_eq!(d.actions.command.unwrap().timeout_s, DEFAULT_TIMEOUT_S);
    assert!(
        check_list(&[r.clone(), r]).is_err(),
        "même identifiant deux fois"
    );
}

#[test]
fn substitution() {
    let v = |var: &str| (var == "x").then(|| "1".to_owned());
    assert_eq!(substitute("a{x}b{x}", v).as_deref(), Some("a1b1"));
    assert_eq!(substitute("sans", v).as_deref(), Some("sans"));
    assert_eq!(substitute("{y}", v), None);
    assert_eq!(substitute("{x", v), None);
    assert_eq!(substitute("x}", v), None);
    assert_eq!(substitute("{{x}}", v), None);
}
