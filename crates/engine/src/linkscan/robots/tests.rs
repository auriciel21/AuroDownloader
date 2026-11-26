use proptest::prelude::*;

use super::*;

fn url(path: &str) -> Url {
    Url::parse(&format!("https://site.example{path}")).unwrap()
}

fn allowed(robots: &Robots, path: &str) -> bool {
    robots.allows(&url(path))
}

#[test]
fn the_star_group_applies_without_a_named_one() {
    let robots = Robots::parse(
        "User-agent: Googlebot\nDisallow: /\n\nUser-agent: *\nDisallow: /prive/\nAllow: /prive/public\n",
    );
    assert!(allowed(&robots, "/"));
    assert!(allowed(&robots, "/page.html"));
    assert!(!allowed(&robots, "/prive/a.html"));
    assert!(allowed(&robots, "/prive/public/a.html"));
}

#[test]
fn a_group_naming_auro_downloader_wins_over_the_star_one() {
    let robots = Robots::parse(
        "User-agent: *\nDisallow: /\n\nUser-agent: AuroDownloader/0.1\nDisallow: /lent\n",
    );
    assert!(allowed(&robots, "/page"));
    assert!(!allowed(&robots, "/lent/page"));
    // Sans casse, et plusieurs groupes du même nom s'additionnent.
    let robots = Robots::parse(
        "user-agent: aurodownloader\ndisallow: /a\n\nUSER-AGENT: AURODOWNLOADER\nDISALLOW: /b\n",
    );
    assert!(!allowed(&robots, "/a"));
    assert!(!allowed(&robots, "/b"));
    assert!(allowed(&robots, "/c"));
}

#[test]
fn consecutive_agents_share_their_rules() {
    let robots = Robots::parse("User-agent: autre\nUser-agent: *\nDisallow: /x\n");
    assert!(!allowed(&robots, "/x/y"));
    // Une règle avant tout groupe ne compte pas.
    let robots = Robots::parse("Disallow: /\nUser-agent: *\nDisallow: /x\n");
    assert!(allowed(&robots, "/y"));
}

#[test]
fn the_longest_rule_wins_and_allow_wins_ties() {
    let robots = Robots::parse("User-agent: *\nAllow: /p\nDisallow: /\n");
    assert!(allowed(&robots, "/page"));
    assert!(!allowed(&robots, "/autre"));
    let robots = Robots::parse("User-agent: *\nDisallow: /dossier\nAllow: /dossier\n");
    assert!(allowed(&robots, "/dossier/a"));
    let robots = Robots::parse("User-agent: *\nAllow: /dossier\nDisallow: /dossier/\n");
    assert!(!allowed(&robots, "/dossier/a"));
    assert!(allowed(&robots, "/dossier"));
}

#[test]
fn stars_and_dollars_follow_rfc_9309() {
    let robots = Robots::parse("User-agent: *\nDisallow: /*.php$\nDisallow: /*?session=\n");
    assert!(!allowed(&robots, "/index.php"));
    assert!(!allowed(&robots, "/a/b/c.php"));
    assert!(allowed(&robots, "/index.php5"));
    assert!(allowed(&robots, "/index.php/suite"));
    assert!(!allowed(&robots, "/page?session=12"));
    assert!(!allowed(&robots, "/page?a=1&x?session=2"));
    assert!(allowed(&robots, "/page?id=12"));
    let robots = Robots::parse("User-agent: *\nDisallow: /fin$\n");
    assert!(!allowed(&robots, "/fin"));
    assert!(allowed(&robots, "/fin/"));
}

#[test]
fn empty_rules_comments_and_unknown_lines_say_nothing() {
    let robots = Robots::parse(
        "# en-tête\nUser-agent: * # tous\nDisallow:\nSitemap: https://site.example/plan.xml\nnimporte quoi\nDisallow: /x # pas là\n",
    );
    assert!(allowed(&robots, "/"));
    assert!(!allowed(&robots, "/x"));
    assert!(allowed(&robots, "/y"));
}

#[test]
fn robots_txt_itself_is_always_allowed() {
    assert!(Robots::deny_all().allows(&url("/robots.txt")));
    assert!(!Robots::deny_all().allows(&url("/")));
    assert!(Robots::allow_all().allows(&url("/tout")));
}

#[test]
fn encodings_compare_alike() {
    // Un motif écrit en clair couvre l'adresse encodée, et la casse de l'encodage ne compte
    // pas (RFC 9309, §2.2.2).
    let robots = Robots::parse("User-agent: *\nDisallow: /déconnexion\nDisallow: /a%3cb\n");
    assert!(!allowed(&robots, "/d%C3%A9connexion"));
    assert!(!allowed(&robots, "/déconnexion"));
    assert!(!allowed(&robots, "/a%3Cb"));
}

#[test]
fn the_crawl_delay_of_the_chosen_group_is_read() {
    let robots = Robots::parse("User-agent: *\nCrawl-delay: 2.5\nDisallow: /x\n");
    assert_eq!(robots.crawl_delay, Some(Duration::from_millis(2500)));
    let robots =
        Robots::parse("User-agent: autre\nCrawl-delay: 30\n\nUser-agent: *\nCrawl-delay: abc\n");
    assert_eq!(robots.crawl_delay, None);
}

proptest! {
    // Un fichier quelconque : jamais de panique, et `robots.txt` reste permis.
    #[test]
    fn any_text_never_panics(text in "\\PC{0,400}", path in "/[\\PC]{0,40}") {
        let robots = Robots::parse(&text);
        prop_assert!(robots.allows(&url("/robots.txt")));
        if let Ok(target) = Url::parse(&format!("https://site.example{path}")) {
            let _ = robots.allows(&target);
        }
    }

    // Des lignes de robots.txt mêlées : jamais de panique, et le calcul reste borné.
    #[test]
    fn rule_like_text_never_panics(lines in proptest::collection::vec(prop_oneof![
        Just("User-agent: *".to_owned()),
        Just("User-agent: AuroDownloader".to_owned()),
        "(Allow|Disallow): [/*$a-c%?=]{0,12}",
        "Crawl-delay: [0-9.]{0,5}",
        "[\\PC]{0,20}",
    ], 0..40), path in "/[a-c*$%?=/]{0,30}") {
        let robots = Robots::parse(&lines.join("\n"));
        if let Ok(target) = Url::parse(&format!("https://site.example{path}")) {
            let _ = robots.allows(&target);
        }
    }

    // Un motif sans joker ni ancre couvre exactement les chemins qui commencent par lui.
    #[test]
    fn a_plain_pattern_is_a_prefix(prefix in "/[a-z]{0,6}", rest in "[a-z/]{0,6}") {
        let robots = Robots::parse(&format!("User-agent: *\nDisallow: {prefix}\n"));
        let path = format!("{prefix}{rest}");
        prop_assert!(!allowed(&robots, &path));
    }
}
