//! Tests de `net::ftp` (F-08) : adresses, identifiants, chemin du proxy, et échanges avec le
//! serveur FTP de test (`testing::ftp`).

use std::time::{Duration, SystemTime};

use super::*;
use crate::net::auth::{Login, Site};
use crate::net::{Client, NetError, RangeRequest, proxy};
use crate::testing::content;
use crate::testing::ftp::{FtpOptions, FtpSite};
use crate::testing::proxies::socks5_proxy;

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

fn client() -> Client {
    Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap()
}

fn site_auth(site: &str, user: &str, pass: &str) -> Auth {
    Auth::new(Login {
        site: Site::parse(site).unwrap(),
        username: user.into(),
        password: pass.into(),
    })
}

async fn read_all(mut body: crate::net::Body) -> Result<Vec<u8>, NetError> {
    let mut out = Vec::new();
    while let Some(chunk) = body.chunk().await? {
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

#[test]
fn the_path_is_relative_decoded_and_checked() {
    assert_eq!(file_path(&url("ftp://h/pub/f.iso")).unwrap(), "pub/f.iso");
    assert_eq!(file_path(&url("ftp://h/%2Fetc/f")).unwrap(), "/etc/f");
    assert_eq!(
        file_path(&url("ftp://h/a%20b.txt;type=i")).unwrap(),
        "a b.txt"
    );
    assert_eq!(file_path(&url("ftps://h:2121/x/y.bin")).unwrap(), "x/y.bin");
    for bad in [
        "ftp://h/",
        "ftp://h/pub/",
        "ftp://h",
        "ftp://h/f%0d%0aDELE%20x",
    ] {
        assert!(
            matches!(file_path(&url(bad)), Err(NetError::InvalidUrl(_))),
            "{bad}"
        );
    }
}

#[test]
fn credentials_leave_the_address() {
    let (clean, found) = split_credentials("ftp://moi:s%40cret@h:21/f.iso").unwrap();
    assert_eq!(clean, "ftp://h/f.iso");
    assert_eq!(found, Some(("moi".into(), "s@cret".into())));
    let (same, none) = split_credentials("ftp://h/f.iso").unwrap();
    assert_eq!((same.as_str(), none), ("ftp://h/f.iso", None));
    // Audit F1 : le nom seul reste dans l'adresse ; aucun en-tête au mot de passe vide.
    let (kept, none) = split_credentials("sftp://moi@h/f.iso").unwrap();
    assert_eq!((kept.as_str(), none), ("sftp://moi@h/f.iso", None));
    let header = basic_header("moi", "s@cret");
    let headers = vec![("Authorization".to_owned(), header)];
    assert_eq!(
        basic_credentials(&headers),
        Some(("moi".into(), "s@cret".into()))
    );
}

#[test]
fn credentials_come_from_the_task_then_the_address_then_the_site() {
    let site = site_auth("h", "site", "s");
    let header = vec![("Authorization".to_owned(), basic_header("tache", "t"))];
    let pick = |u: &str, h: &[(String, String)], a: Option<&Auth>| {
        let c = credentials(&url(u), h, a);
        (c.user, c.pass, c.anonymous)
    };
    assert_eq!(
        pick("ftp://u:p@h/f", &header, Some(&site)),
        ("tache".into(), "t".into(), false)
    );
    assert_eq!(
        pick("ftp://u:p@h/f", &[], Some(&site)),
        ("u".into(), "p".into(), false)
    );
    assert_eq!(
        pick("ftp://h/f", &[], Some(&site)),
        ("site".into(), "s".into(), false)
    );
    assert!(pick("ftp://autre/f", &[], Some(&site)).2);
    // Audit F1 : le nom seul dans l'adresse prend le mot de passe du site s'il est pour ce
    // nom, aucun sinon.
    assert_eq!(
        pick("ftp://site@h/f", &[], Some(&site)),
        ("site".into(), "s".into(), false)
    );
    assert_eq!(
        pick("ftp://autre@h/f", &[], Some(&site)),
        ("autre".into(), String::new(), false)
    );
    assert!(pick("ftp://anonymous:x@h/f", &[], None).2);
    // Audit F4 : `ftp` avec un vrai mot de passe est un vrai compte (NAS), jamais en clair
    // sans autorisation ; vide ou de forme adresse, l'accès anonyme habituel.
    assert!(!pick("ftp://ftp:vrai@h/f", &[], None).2);
    assert!(pick("ftp://ftp@h/f", &[], None).2);
    assert!(pick("ftp://ftp:moi%40exemple.org@h/f", &[], None).2);
    // Jamais le mot de passe dans un journal.
    assert!(
        !format!("{:?}", credentials(&url("ftp://u:secret@h/f"), &[], None)).contains("secret")
    );
}

#[test]
fn only_a_socks5_proxy_is_followed() {
    let manual = |u: &str, exceptions: &[&str]| proxy::Effective {
        settings: proxy::Settings {
            mode: proxy::Mode::Manual,
            url: Some(u.into()),
            exceptions: exceptions.iter().map(|e| (*e).to_owned()).collect(),
            username: Some("moi".into()),
        },
        password: Some("pw".into()),
    };
    let direct = Route::Direct { public_only: false };
    assert_eq!(route_for(&proxy::Effective::default(), false, "h"), direct);
    assert_eq!(route_for(&manual("http://p:3128", &[]), false, "h"), direct);
    let socks = route_for(&manual("socks5h://p:1080", &[]), false, "h");
    assert_eq!(
        socks,
        Route::Socks {
            proxy: "p:1080".into(),
            login: Some(("moi".into(), "pw".into())),
            local: false,
            public_only: false,
        }
    );
    // Réseau public : résolution locale, même en socks5h.
    assert!(matches!(
        route_for(&manual("socks5h://p:1080", &[]), true, "h"),
        Route::Socks { local: true, .. }
    ));
    let e = manual("socks5://p:1080", &["exemple.org", ".lan", "10.0.0.0/8"]);
    assert_eq!(route_for(&e, false, "ftp.exemple.org"), direct);
    assert_eq!(route_for(&e, false, "nas.lan"), direct);
    assert_eq!(route_for(&e, false, "10.1.2.3"), direct);
    assert!(matches!(
        route_for(&e, false, "11.1.2.3"),
        Route::Socks { .. }
    ));
    // Le mot de passe du proxy n'entre pas dans la clé lisible de la réserve.
    assert!(!format!("{socks:?}").contains("pw"));
}

#[tokio::test]
async fn anonymous_probe_ranges_and_whole_file() {
    let site = FtpSite::start(FtpOptions::default()).await;
    let data = content(300_000);
    site.put("pub/fichier.bin", &data);
    let client = client();
    let probe = client
        .probe(&site.url("pub/fichier.bin"), &[])
        .await
        .unwrap();
    assert_eq!(probe.total_bytes, Some(300_000));
    assert!(probe.accepts_ranges);
    assert_eq!(probe.file_name, "fichier.bin");
    assert_eq!(probe.etag, None);
    let modified = probe.last_modified.clone().expect("MDTM");
    assert_eq!(modified.len(), 14);

    // Des plages, au milieu et jusqu'à la fin, sur la même connexion de commande.
    for (first, last) in [
        (1000, 70_000),
        (70_001, 200_000),
        (200_001, 299_999),
        (5, 5),
    ] {
        let req = RangeRequest {
            first,
            last,
            total: 300_000,
            if_range: Some(&modified),
        };
        let body = client
            .get_range(&site.url("pub/fichier.bin"), &[], &req)
            .await
            .unwrap();
        let got = read_all(body).await.unwrap();
        assert_eq!(got, data[first as usize..=last as usize], "{first}-{last}");
    }
    let whole = client
        .get_all(&site.url("pub/fichier.bin"), &[], Some(300_000))
        .await
        .unwrap();
    assert_eq!(read_all(whole).await.unwrap(), data);
    assert_eq!(site.accepted(), 1, "connexion de commande réutilisée");
}

#[tokio::test]
async fn a_changed_file_stops_the_ranges() {
    let site = FtpSite::start(FtpOptions::default()).await;
    site.put("f.bin", &content(10_000));
    let client = client();
    let at = site.url("f.bin");
    let probe = client.probe(&at, &[]).await.unwrap();
    let modified = probe.last_modified.unwrap();
    let req = |total| RangeRequest {
        first: 0,
        last: 99,
        total,
        if_range: None,
    };
    // Autre taille.
    site.put("f.bin", &content(12_000));
    let e = client.get_range(&at, &[], &req(10_000)).await.unwrap_err();
    assert!(matches!(e, NetError::FileChanged), "{e}");
    // Même taille, autre date.
    site.put("f.bin", &content(10_000));
    let file = std::fs::File::options()
        .write(true)
        .open(site.root.join("f.bin"))
        .unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    drop(file);
    let changed = RangeRequest {
        if_range: Some(&modified),
        ..req(10_000)
    };
    let e = client.get_range(&at, &[], &changed).await.unwrap_err();
    assert!(matches!(e, NetError::FileChanged), "{e}");
    // Fichier disparu.
    std::fs::remove_file(site.root.join("f.bin")).unwrap();
    let e = client.probe(&at, &[]).await.unwrap_err();
    assert!(matches!(e, NetError::FtpReply(550)), "{e}");
    assert!(!e.retryable());
    assert_eq!(e.error_code(), Some("HTTP_STATUS"));
    assert!(!e.to_string().contains("f.bin"), "{e}");
}

#[tokio::test]
async fn logins_are_given_and_refusals_reported() {
    let site = FtpSite::start(FtpOptions {
        login: Some(("moi", "secret")),
        ..FtpOptions::default()
    })
    .await;
    site.put("f.bin", &content(1000));
    let client = client();
    let at = site.url("f.bin");
    // Anonyme refusé.
    let e = client.probe(&at, &[]).await.unwrap_err();
    assert!(matches!(e, NetError::AuthRequired), "{e}");
    // Mot de passe sans TLS : refusé d'office.
    let auth = site_auth("127.0.0.1", "moi", "secret");
    let e = client.probe_as(&at, &[], Some(&auth)).await.unwrap_err();
    assert!(matches!(e, NetError::FtpTlsMissing), "{e}");
    assert_eq!(e.error_code(), Some("AUTH_FAILED"));
    // Autorisé sans chiffrement pour ce site : en clair.
    let plain = site_auth("127.0.0.1", "moi", "secret").allowing_plain(true);
    let probe = client.probe_as(&at, &[], Some(&plain)).await.unwrap();
    assert_eq!(probe.total_bytes, Some(1000));
    let wrong = site_auth("127.0.0.1", "moi", "faux").allowing_plain(true);
    let e = client.probe_as(&at, &[], Some(&wrong)).await.unwrap_err();
    assert!(matches!(e, NetError::AuthRefused), "{e}");
    // L'autorisation d'un autre site ne vaut pas ici.
    let elsewhere = site_auth("ailleurs.example", "moi", "secret").allowing_plain(true);
    let header = vec![("Authorization".to_owned(), basic_header("moi", "secret"))];
    let e = client
        .probe_as(&at, &header, Some(&elsewhere))
        .await
        .unwrap_err();
    assert!(matches!(e, NetError::FtpTlsMissing), "{e}");
}

#[tokio::test]
async fn ftps_verifies_the_certificate_and_encrypts() {
    let site = FtpSite::start(FtpOptions {
        login: Some(("moi", "secret")),
        tls: true,
        require_tls: true,
        ..FtpOptions::default()
    })
    .await;
    let data = content(200_000);
    site.put("f.bin", &data);
    let at = site.url("f.bin");
    let auth = site_auth("127.0.0.1", "moi", "secret");
    // Autorité inconnue : refusé.
    let e = client().probe_as(&at, &[], Some(&auth)).await.unwrap_err();
    assert!(matches!(e, NetError::FtpTls(_)), "{e}");
    assert_eq!(e.error_code(), Some("TLS_INVALID_CERT"));

    let client = Client::with_extra_roots(site.ca_pem.as_deref().unwrap().as_bytes()).unwrap();
    let probe = client.probe_as(&at, &[], Some(&auth)).await.unwrap();
    assert_eq!(probe.total_bytes, Some(200_000));
    let req = RangeRequest {
        first: 10,
        last: 150_000,
        total: 200_000,
        if_range: probe.last_modified.as_deref(),
    };
    let body = client
        .get_range_as(&at, &[], &req, Some(&auth))
        .await
        .unwrap();
    assert_eq!(read_all(body).await.unwrap(), data[10..=150_000]);
    assert_eq!(
        site.accepted(),
        2,
        "une connexion refusée, puis une réutilisée"
    );
}

#[tokio::test]
async fn a_full_server_answers_421_an_overload() {
    let site = FtpSite::start(FtpOptions {
        max_open: Some(1),
        ..FtpOptions::default()
    })
    .await;
    site.put("f.bin", &content(100_000));
    let client = client();
    let at = site.url("f.bin");
    let req = RangeRequest {
        first: 0,
        last: 99_999,
        total: 100_000,
        if_range: None,
    };
    // Une plage en cours garde sa connexion : la suivante en demande une autre.
    let held = client.get_range(&at, &[], &req).await.unwrap();
    let e = client.get_range(&at, &[], &req).await.unwrap_err();
    assert!(matches!(e, NetError::FtpReply(421)), "{e}");
    assert!(e.is_overload() && e.retryable());
    assert_eq!(site.refused(), 1);
    drop(held);
}

#[tokio::test]
async fn socks5_carries_commands_and_data() {
    let site = FtpSite::start(FtpOptions::default()).await;
    let data = content(50_000);
    site.put("f.bin", &data);
    let socks = socks5_proxy(None);
    let client = client();
    client
        .set_proxy(proxy::Effective {
            settings: proxy::Settings {
                mode: proxy::Mode::Manual,
                url: Some(socks.url("socks5h")),
                ..proxy::Settings::default()
            },
            password: None,
        })
        .unwrap();
    let at = site.url("f.bin");
    client.probe(&at, &[]).await.unwrap();
    let whole = client.get_all(&at, &[], Some(50_000)).await.unwrap();
    assert_eq!(read_all(whole).await.unwrap(), data);
    let seen = socks.seen();
    assert!(seen.len() >= 2, "commande et données : {seen:?}");
    assert_eq!(seen[0].target, site.addr.to_string());
    assert!(seen.iter().all(|s| s.target.starts_with("127.0.0.1:")));
}

/// Essai réel (6e, Internet ; ignoré par défaut) : `test.rebex.net` par le proxy SOCKS5 de
/// test, qui n'écoute que sur la boucle locale. FTP chiffré par `AUTH TLS` et SFTP (clé
/// acceptée comme le ferait l'utilisateur) passent par lui, commande et données ; FTPS
/// implicite est refusé avant toute connexion.
#[tokio::test]
#[ignore = "Internet : test.rebex.net"]
async fn real_socks5_rebex() {
    let socks = socks5_proxy(None);
    let client = Client::with_timeouts(Duration::from_secs(20), Duration::from_secs(20)).unwrap();
    client
        .set_proxy(proxy::Effective {
            settings: proxy::Settings {
                mode: proxy::Mode::Manual,
                url: Some(socks.url("socks5h")),
                ..proxy::Settings::default()
            },
            password: None,
        })
        .unwrap();
    let check = |data: &[u8]| {
        assert!(
            data.starts_with(b"Welcome"),
            "{:?}",
            String::from_utf8_lossy(data)
        );
    };

    let ftp = "ftp://demo:password@test.rebex.net/readme.txt";
    let probe = client.probe(ftp, &[]).await.unwrap();
    let whole = client.get_all(ftp, &[], probe.total_bytes).await.unwrap();
    let data = read_all(whole).await.unwrap();
    check(&data);
    let after_ftp = socks.seen();
    println!("FTP : {} octets ; proxy : {after_ftp:?}", data.len());
    assert!(after_ftp.len() >= 2, "commande et données : {after_ftp:?}");
    assert_eq!(after_ftp[0].target, "test.rebex.net:21");
    assert!(after_ftp[0].by_name, "nom résolu par le proxy (socks5h)");

    let sftp = "sftp://demo:password@test.rebex.net/readme.txt";
    let e = client.probe(sftp, &[]).await.unwrap_err();
    let NetError::HostKeyUnknown {
        host,
        port,
        fingerprint,
        ..
    } = e
    else {
        panic!("clé inconnue attendue : {e}");
    };
    println!("SFTP : clé {fingerprint}");
    client.host_keys().accept(&host, port, &fingerprint);
    let probe = client.probe(sftp, &[]).await.unwrap();
    let whole = client.get_all(sftp, &[], probe.total_bytes).await.unwrap();
    let data = read_all(whole).await.unwrap();
    check(&data);
    let seen = socks.seen();
    println!(
        "SFTP : {} octets ; proxy : {:?}",
        data.len(),
        &seen[after_ftp.len()..]
    );
    assert!(
        seen[after_ftp.len()..]
            .iter()
            .all(|s| s.target == "test.rebex.net:22"),
        "{seen:?}"
    );

    let e = client
        .probe("ftps://demo:password@test.rebex.net/readme.txt", &[])
        .await
        .unwrap_err();
    assert!(matches!(e, NetError::Proxy(_)), "{e}");
    assert_eq!(
        socks.seen().len(),
        seen.len(),
        "refusé avant toute connexion"
    );
}

#[tokio::test]
async fn public_only_refuses_the_local_network() {
    let site = FtpSite::start(FtpOptions::default()).await;
    site.put("f.bin", &content(10));
    let restricted = client().restricted().unwrap();
    let e = restricted.probe(&site.url("f.bin"), &[]).await.unwrap_err();
    assert!(matches!(e, NetError::AddressForbidden), "{e}");
    let named = format!("ftp://localhost:{}/f.bin", site.addr.port());
    let e = restricted.probe(&named, &[]).await.unwrap_err();
    assert!(matches!(e, NetError::AddressForbidden), "{e}");
    assert_eq!(site.accepted(), 0);
}

/// Audit F8 : une connexion de commande au repos se ferme à la fin de son repos, même si
/// plus rien ne passe par la réserve.
#[tokio::test]
async fn idle_command_connections_close_after_their_rest() {
    let site = FtpSite::start(FtpOptions::default()).await;
    site.put("f.bin", &content(1000));
    let client = client();
    client.probe(&site.url("f.bin"), &[]).await.unwrap();
    let shared = Arc::clone(client.ftp().shared);
    assert_eq!(shared.idle_count(), 1);
    tokio::time::pause();
    tokio::time::advance(POOL_IDLE + Duration::from_secs(2)).await;
    // Le minuteur de la réserve se déclenche pendant cette attente (horloge du test).
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(shared.idle_count(), 0);
}

/// Audit F10 : seul un refus d'`EPSV` fait passer la connexion en `PASV`.
#[test]
fn only_an_epsv_refusal_falls_back_to_pasv() {
    use suppaftp::Status;
    use suppaftp::types::Response;
    let reply =
        |status, text: &str| FtpError::UnexpectedResponse(Response::new(status, text.into()));
    assert!(epsv_refused(&reply(
        Status::BadCommand,
        "500 Unknown command"
    )));
    assert!(epsv_refused(&reply(
        Status::NotImplemented,
        "502 Not implemented"
    )));
    assert!(epsv_refused(&reply(
        Status::BadArguments,
        "501 EPSV not allowed"
    )));
    assert!(!epsv_refused(&reply(
        Status::BadArguments,
        "501 Invalid file name"
    )));
    assert!(!epsv_refused(&reply(
        Status::FileUnavailable,
        "550 No such file"
    )));
}
