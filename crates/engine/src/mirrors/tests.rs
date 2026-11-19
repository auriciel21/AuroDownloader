//! Validation d'un miroir sur des serveurs locaux (SPEC §7.7).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::testing::{FileSite, Server, serve_file};

const LEN: u64 = 300_000;

fn site(edit: impl FnOnce(&mut FileSite)) -> Server {
    let mut site = FileSite::new(LEN);
    edit(&mut site);
    serve_file(Arc::new(Mutex::new(site)))
}

fn client() -> Client {
    Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap()
}

async fn check(reference: &Server, mirror_url: &str, total: u64) -> Verdict {
    let reference_url = reference.url("/f.iso");
    let reference = Side {
        url: &reference_url,
        headers: &[],
        auth: None,
        if_range: Some("\"v1\""),
    };
    let mirror = Side {
        url: mirror_url,
        headers: &[],
        auth: None,
        if_range: None,
    };
    validate(&client(), &reference, &mirror, total).await
}

#[tokio::test]
async fn the_same_file_elsewhere_is_accepted() {
    let reference = site(|_| {});
    let mirror = site(|s| s.etag = Some("\"autre-serveur\"".into()));
    match check(&reference, &mirror.url("/miroir/f.iso"), LEN).await {
        Verdict::Accepted(facts) => {
            assert_eq!(facts.content_length, LEN);
            // Ses propres validateurs, pour la reprise sur lui.
            assert_eq!(facts.etag.as_deref(), Some("\"autre-serveur\""));
        }
        other => panic!("{other:?}"),
    }
    // Les échantillons ont bien été lus sur les deux serveurs.
    for server in [&reference, &mirror] {
        let ranged = server
            .requests()
            .iter()
            .filter(|r| r.header("range").is_some())
            .count();
        assert!(ranged >= SAMPLES, "{ranged} plages");
    }
}

#[tokio::test]
async fn another_file_is_refused() {
    let reference = site(|_| {});
    // Même taille, autres octets.
    let shifted = site(|s| s.shift = 1);
    match check(&reference, &shifted.url("/f.iso"), LEN).await {
        Verdict::Refused(reason) => assert!(reason.contains("contenu différent"), "{reason}"),
        other => panic!("{other:?}"),
    }
    let longer = site(|s| s.len = LEN + 1);
    match check(&reference, &longer.url("/f.iso"), LEN).await {
        Verdict::Refused(reason) => assert!(reason.contains("taille différente"), "{reason}"),
        other => panic!("{other:?}"),
    }
    let no_ranges = site(|s| s.ranges = false);
    match check(&reference, &no_ranges.url("/f.iso"), LEN).await {
        Verdict::Refused(reason) => assert!(reason.contains("plage"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_unreachable_mirror_is_only_postponed() {
    let reference = site(|_| {});
    // Port fermé : un serveur arrêté.
    let gone = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    match check(&reference, &format!("http://{gone}/f.iso"), LEN).await {
        Verdict::Unreachable(_) => {}
        other => panic!("{other:?}"),
    }
    // La référence qui a changé n'accuse pas le miroir.
    let mirror = site(|_| {});
    let changed = site(|s| s.etag = Some("\"v2\"".into()));
    match check(&changed, &mirror.url("/f.iso"), LEN).await {
        Verdict::Reference(e) => assert!(matches!(e, NetError::FileChanged), "{e:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn samples_cover_the_start_and_stay_inside_the_file() {
    assert!(sample_ranges(0, 1).is_empty());
    assert_eq!(sample_ranges(10, 1), [(0, 9)]);
    assert_eq!(
        sample_ranges(SAMPLE_BYTES * 3, 1),
        [(0, SAMPLE_BYTES * 3 - 1)]
    );
    for seed in [1, 7, 1 << 40, u64::MAX] {
        let total = 1_000_000;
        let ranges = sample_ranges(total, seed);
        assert_eq!(ranges.len(), SAMPLES);
        assert_eq!(ranges[0], (0, SAMPLE_BYTES - 1));
        for (first, last) in ranges {
            assert_eq!(last - first + 1, SAMPLE_BYTES);
            assert!(last < total);
        }
    }
    assert_ne!(sample_ranges(1_000_000, 1), sample_ranges(1_000_000, 2));
}

#[test]
fn page_headers_stay_with_their_host() {
    let headers = vec![
        ("User-Agent".to_owned(), "Navigateur/1".to_owned()),
        ("Cookie".to_owned(), "session=1".to_owned()),
        ("Referer".to_owned(), "https://a.exemple/page".to_owned()),
    ];
    let same = headers_for(
        &headers,
        "https://a.exemple/f",
        "https://a.exemple/miroir/f",
    );
    assert_eq!(same, headers);
    let other = headers_for(&headers, "https://a.exemple/f", "https://b.exemple/f");
    assert_eq!(
        other,
        [("User-Agent".to_owned(), "Navigateur/1".to_owned())]
    );
    // Audit T6 : même hôte, mais en clair ou sur un autre port : le seul `User-Agent`.
    for elsewhere in ["http://a.exemple/f", "https://a.exemple:8443/f"] {
        assert_eq!(
            headers_for(&headers, "https://a.exemple/f", elsewhere),
            [("User-Agent".to_owned(), "Navigateur/1".to_owned())],
            "{elsewhere}"
        );
    }
    // Le port par défaut écrit ou non, et la casse de l'hôte, ne changent rien.
    assert_eq!(
        headers_for(&headers, "https://a.exemple/f", "https://A.EXEMPLE:443/g"),
        headers
    );
}

#[test]
fn same_origin_compares_scheme_host_and_port() {
    use crate::net::same_origin;
    assert!(same_origin("ftp://h/a", "ftp://H:21/b"));
    assert!(same_origin("sftp://h/a", "sftp://h/b"));
    assert!(!same_origin("sftp://h/a", "sftp://h:2222/b"));
    assert!(!same_origin("https://h/a", "http://h/a"));
    assert!(!same_origin("https://h/a", "pas une adresse"));
}
