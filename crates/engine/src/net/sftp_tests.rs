//! Tests de `net::sftp` (F-08) face au serveur SFTP de test (`testing::sftp`).

use std::time::{Duration, SystemTime};

use super::*;
use crate::net::auth::{Login as SiteLogin, Site};
use crate::net::{Client, RangeRequest};
use crate::testing::content;
use crate::testing::sftp::{SftpOptions, SftpSite, user_key};

fn client() -> Client {
    Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap()
}

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

fn site_auth(user: &str, password: &str) -> Auth {
    Auth::new(SiteLogin {
        site: Site::parse("sftp://127.0.0.1").unwrap(),
        username: user.into(),
        password: password.into(),
    })
}

async fn read_all(mut body: crate::net::Body) -> Result<Vec<u8>, NetError> {
    let mut out = Vec::new();
    while let Some(chunk) = body.chunk().await? {
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Accepte la clé du serveur pour ce client, comme le ferait l'utilisateur.
async fn trust(client: &Client, site: &SftpSite, at: &str, auth: &Auth) {
    let e = client.probe_as(at, &[], Some(auth)).await.unwrap_err();
    let NetError::HostKeyUnknown {
        host,
        port,
        fingerprint,
        ..
    } = e
    else {
        panic!("clé inconnue attendue : {e}");
    };
    assert_eq!(fingerprint, site.fingerprint());
    client.host_keys().accept(&host, port, &fingerprint);
}

#[test]
fn the_path_is_absolute_or_under_home() {
    assert_eq!(file_path(&url("sftp://h/srv/f.iso")).unwrap(), "/srv/f.iso");
    assert_eq!(file_path(&url("sftp://h/~/f.iso")).unwrap(), "f.iso");
    assert_eq!(file_path(&url("sftp://h/a%20b")).unwrap(), "/a b");
    for bad in ["sftp://h/", "sftp://h/d/", "sftp://h", "sftp://h/~/"] {
        assert!(
            matches!(file_path(&url(bad)), Err(NetError::InvalidUrl(_))),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn an_unknown_key_waits_for_the_user_then_ranges_follow() {
    let site = SftpSite::start(SftpOptions {
        password: Some(("moi", "secret")),
        ..SftpOptions::default()
    })
    .await;
    let data = content(700_000);
    site.put("pub/f.bin", &data);
    let client = client();
    let at = site.url("pub/f.bin");
    let auth = site_auth("moi", "secret");

    let e = client.probe_as(&at, &[], Some(&auth)).await.unwrap_err();
    assert_eq!(e.error_code(), Some("HOST_KEY_UNKNOWN"));
    assert!(!e.retryable());
    let pending = client.host_keys().pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].fingerprint, site.fingerprint());
    assert!(!pending[0].changed);
    assert!(e.to_string().contains(&site.fingerprint()), "{e}");
    let (host, port) = (pending[0].host.clone(), pending[0].port);
    assert!(
        client
            .host_keys()
            .pending_for(&host, port, "SHA256:autre")
            .is_none()
    );
    client.host_keys().accept(&host, port, &site.fingerprint());
    assert!(client.host_keys().pending().is_empty());

    let probe = client.probe_as(&at, &[], Some(&auth)).await.unwrap();
    assert_eq!(probe.total_bytes, Some(700_000));
    assert!(probe.accepts_ranges);
    assert_eq!(probe.file_name, "f.bin");
    let modified = probe.last_modified.clone().expect("date");
    for (first, last) in [(0, 99_999), (100_000, 650_000), (650_001, 699_999)] {
        let req = RangeRequest {
            first,
            last,
            total: 700_000,
            if_range: Some(&modified),
        };
        let body = client
            .get_range_as(&at, &[], &req, Some(&auth))
            .await
            .unwrap();
        assert_eq!(
            read_all(body).await.unwrap(),
            data[first as usize..=last as usize]
        );
    }
    let whole = client
        .get_all_as(&at, &[], Some(700_000), Some(&auth))
        .await
        .unwrap();
    assert_eq!(read_all(whole).await.unwrap(), data);
    // Une connexion refusée (clé inconnue), puis une seule, réutilisée.
    assert_eq!(site.accepted(), 2);

    // Clé changée : bloqué, avec la nouvelle empreinte en attente.
    site.replace_host_key();
    let fresh = self::client();
    fresh.host_keys().accept(&host, port, "SHA256:ancienne");
    let e = fresh.probe_as(&at, &[], Some(&auth)).await.unwrap_err();
    assert_eq!(e.error_code(), Some("HOST_KEY_CHANGED"), "{e}");
    assert!(fresh.host_keys().pending()[0].changed);
}

#[tokio::test]
async fn credentials_and_refusals() {
    let (public, private) = user_key(Some("phrase"));
    let site = SftpSite::start(SftpOptions {
        password: Some(("moi", "secret")),
        key: Some(("cle", public)),
    })
    .await;
    site.put("f.bin", &content(1000));
    let client = client();
    let at = site.url("f.bin");
    trust(&client, &site, &at, &site_auth("moi", "secret")).await;

    // Sans identifiant.
    let e = client.probe(&at, &[]).await.unwrap_err();
    assert!(matches!(e, NetError::AuthRequired), "{e}");
    // Mot de passe faux.
    let e = client
        .probe_as(&at, &[], Some(&site_auth("moi", "faux")))
        .await
        .unwrap_err();
    assert!(matches!(e, NetError::AuthRefused), "{e}");
    // Identifiants de l'adresse.
    let named = format!("sftp://moi:secret@{}/f.bin", site.addr);
    assert_eq!(
        client.probe(&named, &[]).await.unwrap().total_bytes,
        Some(1000)
    );
    // Clé privée chiffrée, phrase de passe au coffre.
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("id_ed25519");
    std::fs::write(&key_file, private).unwrap();
    let key_path = Some(key_file.to_string_lossy().into_owned());
    let by_key = site_auth("cle", "phrase").with_key(key_path.clone());
    assert_eq!(
        client
            .probe_as(&at, &[], Some(&by_key))
            .await
            .unwrap()
            .total_bytes,
        Some(1000)
    );
    let wrong = site_auth("cle", "pas la bonne").with_key(key_path);
    let e = client.probe_as(&at, &[], Some(&wrong)).await.unwrap_err();
    assert!(matches!(e, NetError::KeyUnreadable), "{e}");
    assert_eq!(e.error_code(), Some("AUTH_FAILED"));
    // Des identifiants à clé ne servent pas à FTP.
    let ftp = url("ftp://127.0.0.1/f");
    assert!(
        site_auth("cle", "phrase")
            .with_key(Some("x".into()))
            .credentials_for(&ftp)
            .is_none()
    );
}

#[tokio::test]
async fn a_changed_or_missing_file_is_reported() {
    let site = SftpSite::start(SftpOptions {
        password: Some(("moi", "secret")),
        ..SftpOptions::default()
    })
    .await;
    site.put("f.bin", &content(10_000));
    let client = client();
    let at = site.url("f.bin");
    let auth = site_auth("moi", "secret");
    trust(&client, &site, &at, &auth).await;
    let modified = client
        .probe_as(&at, &[], Some(&auth))
        .await
        .unwrap()
        .last_modified
        .unwrap();
    let file = std::fs::File::options()
        .write(true)
        .open(site.root.join("f.bin"))
        .unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    drop(file);
    let req = RangeRequest {
        first: 0,
        last: 99,
        total: 10_000,
        if_range: Some(&modified),
    };
    let e = client
        .get_range_as(&at, &[], &req, Some(&auth))
        .await
        .unwrap_err();
    assert!(matches!(e, NetError::FileChanged), "{e}");
    let missing = site.url("absent.bin");
    let e = client
        .probe_as(&missing, &[], Some(&auth))
        .await
        .unwrap_err();
    assert!(matches!(e, NetError::SftpStatus(2)), "{e}");
    assert!(!e.retryable());
    assert!(!e.to_string().contains("absent"), "{e}");
}

#[tokio::test]
async fn public_only_refuses_the_local_network() {
    let site = SftpSite::start(SftpOptions::default()).await;
    let restricted = client().restricted().unwrap();
    let at = format!("sftp://moi:x@{}/f.bin", site.addr);
    let e = restricted.probe(&at, &[]).await.unwrap_err();
    assert!(matches!(e, NetError::AddressForbidden), "{e}");
    assert_eq!(site.accepted(), 0);
}

/// Audit F6 : une clé en attente part quand la bonne revient, ou quand l'utilisateur oublie
/// l'hôte ; la table des clés en attente reste bornée.
#[test]
fn pending_keys_leave_and_stay_bounded() {
    let keys = HostKeys::default();
    keys.set_accepted([("h".to_owned(), 22, "SHA256:bon".to_owned())]);
    let check = |host: &str, fingerprint: &str| {
        keys.check(host, 22, "ssh-ed25519".into(), fingerprint.into())
    };
    assert!(matches!(check("h", "SHA256:faux"), Verdict::Refused(_)));
    assert_eq!(keys.pending().len(), 1);
    assert!(matches!(check("h", "SHA256:bon"), Verdict::Trusted));
    assert!(keys.pending().is_empty(), "la bonne clé est revenue");

    assert!(matches!(check("autre", "SHA256:x"), Verdict::Refused(_)));
    keys.forget("autre", 22);
    assert!(keys.pending().is_empty(), "hôte oublié");

    for i in 0..MAX_PENDING + 50 {
        let _ = check(&format!("h{i}"), "SHA256:x");
    }
    assert_eq!(keys.pending().len(), MAX_PENDING);
}
