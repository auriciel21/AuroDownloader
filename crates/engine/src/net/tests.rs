//! Préparation d'une tâche face à un serveur HTTP local qui simule les cas de la
//! SPEC §9.1 : `Range` accepté ou refusé, `HEAD` refusé, redirections, erreurs, lenteur.

use std::io::Read;
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use super::*;
use crate::testing::{reply, serve};

fn client() -> Client {
    Client::with_timeouts(Duration::from_secs(5), Duration::from_secs(5)).unwrap()
}

#[test]
fn task_headers_are_checked() {
    let h = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect()
    };
    assert!(check_task_headers(&h(&[("Referer", "https://x/"), ("Cookie", "a=1")])).is_ok());
    assert!(check_task_headers(&[]).is_ok());
    for bad in [
        h(&[("host", "x")]),
        h(&[("If-Range", "\"v\"")]),
        h(&[("Accept-Encoding", "gzip")]),
        h(&[("Cookie", "a"), ("COOKIE", "b")]),
        h(&[("Nom invalide", "x")]),
        h(&[("X-Ligne", "a\r\nb")]),
    ] {
        assert!(check_task_headers(&bad).is_err(), "{bad:?}");
    }
    let long = vec![(
        "Cookie".to_owned(),
        "x".repeat(crate::secrets::MAX_SECRET + 1),
    )];
    let refused = check_task_headers(&long).unwrap_err().to_string();
    assert!(!refused.contains("xxxx"), "la valeur n'est jamais citée");
    let many: Vec<_> = (0..=MAX_TASK_HEADERS)
        .map(|i| (format!("X-{i}"), "v".to_owned()))
        .collect();
    assert!(check_task_headers(&many).is_err());
    assert!(is_secret_header("cookie") && is_secret_header("Authorization"));
    assert!(is_secret_header("PROXY-AUTHORIZATION") && !is_secret_header("Referer"));
}

#[tokio::test]
async fn head_announcing_ranges_and_size_is_enough() {
    let server = serve(|_| {
        reply(
            "200 OK",
            &[
                ("Content-Length", "12345"),
                ("Accept-Ranges", "bytes"),
                ("ETag", "\"v1\""),
                ("Last-Modified", "Wed, 21 Oct 2026 07:28:00 GMT"),
                (
                    "Content-Disposition",
                    "attachment; filename=\"rapport.pdf\"",
                ),
            ],
            b"",
        )
    });
    let probe = client().probe(&server.url("/dl?id=3"), &[]).await.unwrap();
    assert_eq!(probe.total_bytes, Some(12345));
    assert!(probe.accepts_ranges);
    assert_eq!(probe.strong_etag(), Some("\"v1\""));
    assert_eq!(
        probe.last_modified.as_deref(),
        Some("Wed, 21 Oct 2026 07:28:00 GMT")
    );
    assert_eq!(probe.file_name, "rapport.pdf");
    assert!(!probe.downgraded);

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "pas de GET : {requests:?}");
    assert_eq!(requests[0].method, "HEAD");
    assert_eq!(requests[0].header("accept-encoding"), Some("identity"));
    assert_eq!(requests[0].header("user-agent"), Some(USER_AGENT));
}

#[tokio::test]
async fn refused_head_falls_back_to_a_one_byte_get() {
    let server = serve(|r| match r.method.as_str() {
        "HEAD" => reply("405 Method Not Allowed", &[("Content-Length", "0")], b""),
        _ => reply(
            "206 Partial Content",
            &[
                ("Content-Range", "bytes 0-0/5000"),
                ("Content-Length", "1"),
                ("ETag", "W/\"faible\""),
            ],
            b"x",
        ),
    });
    let probe = client()
        .probe(&server.url("/f/image.iso"), &[])
        .await
        .unwrap();
    assert_eq!(probe.total_bytes, Some(5000));
    assert!(probe.accepts_ranges);
    assert_eq!(probe.etag.as_deref(), Some("W/\"faible\""));
    assert_eq!(
        probe.strong_etag(),
        None,
        "un ETag faible ne sert pas à If-Range"
    );
    assert_eq!(probe.file_name, "image.iso");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].header("range"), Some("bytes=0-0"));
    assert_eq!(requests[1].header("accept-encoding"), Some("identity"));
}

#[tokio::test]
async fn server_ignoring_range_means_a_single_stream() {
    let body = vec![b'a'; 4000];
    let server = serve(move |r| match r.method.as_str() {
        "HEAD" => reply("200 OK", &[("Content-Length", "4000")], b""),
        _ => reply("200 OK", &[("Content-Length", "4000")], &body),
    });
    let probe = client().probe(&server.url("/a.bin"), &[]).await.unwrap();
    assert_eq!(probe.total_bytes, Some(4000));
    assert!(!probe.accepts_ranges);
    assert_eq!(server.requests().len(), 2, "HEAD muet sur les plages : GET");
}

/// Annonce des plages sans validateur, puis plages ignorées (API de Wikimedia Commons,
/// essai réel du 2026-11-09) : le `GET` d'un octet le révèle, un seul flux.
#[tokio::test]
async fn ranges_announced_without_a_validator_are_checked() {
    let body = vec![b'w'; 2086];
    let server = serve(move |r| match r.method.as_str() {
        "HEAD" => reply(
            "200 OK",
            &[("Content-Length", "2086"), ("Accept-Ranges", "bytes")],
            b"",
        ),
        _ => reply(
            "200 OK",
            &[("Content-Length", "2086"), ("Accept-Ranges", "bytes")],
            &body,
        ),
    });
    let probe = client()
        .probe(&server.url("/w/api.php?lang=fr"), &[])
        .await
        .unwrap();
    assert_eq!(probe.total_bytes, Some(2086));
    assert!(!probe.accepts_ranges, "annonce démentie par le GET");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].header("range"), Some("bytes=0-0"));
}

#[tokio::test]
async fn accept_ranges_none_is_trusted() {
    let server = serve(|_| {
        reply(
            "200 OK",
            &[("Content-Length", "10"), ("Accept-Ranges", "none")],
            b"",
        )
    });
    let probe = client().probe(&server.url("/a.bin"), &[]).await.unwrap();
    assert_eq!((probe.total_bytes, probe.accepts_ranges), (Some(10), false));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn empty_file_answers_416_with_its_size() {
    let server = serve(|r| match r.method.as_str() {
        "HEAD" => reply("501 Not Implemented", &[("Content-Length", "0")], b""),
        _ => reply(
            "416 Range Not Satisfiable",
            &[("Content-Range", "bytes */0"), ("Content-Length", "0")],
            b"",
        ),
    });
    let probe = client().probe(&server.url("/vide.txt"), &[]).await.unwrap();
    assert_eq!(probe.total_bytes, Some(0));
    assert_eq!(probe.file_name, "vide.txt");
}

#[tokio::test]
async fn compressed_body_gives_no_size_and_no_ranges() {
    let server = serve(|r| match r.method.as_str() {
        "HEAD" => reply(
            "200 OK",
            &[
                ("Content-Length", "300"),
                ("Content-Encoding", "gzip"),
                ("Accept-Ranges", "bytes"),
            ],
            b"",
        ),
        _ => reply(
            "200 OK",
            &[("Content-Length", "3"), ("Content-Encoding", "gzip")],
            b"abc",
        ),
    });
    let probe = client().probe(&server.url("/page"), &[]).await.unwrap();
    assert_eq!((probe.total_bytes, probe.accepts_ranges), (None, false));
}

#[tokio::test]
async fn task_headers_are_sent_and_override_the_user_agent() {
    let server = serve(|_| {
        reply(
            "200 OK",
            &[("Content-Length", "1"), ("Accept-Ranges", "bytes")],
            b"",
        )
    });
    let headers = [
        ("User-Agent".to_owned(), "Navigateur/1.0".to_owned()),
        ("Referer".to_owned(), "https://exemple.org/".to_owned()),
    ];
    client().probe(&server.url("/a"), &headers).await.unwrap();
    let r = &server.requests()[0];
    assert_eq!(r.header("user-agent"), Some("Navigateur/1.0"));
    assert_eq!(r.header("referer"), Some("https://exemple.org/"));
}

#[tokio::test]
async fn redirects_are_followed_and_the_name_comes_from_the_final_url() {
    let server = serve(|r| match r.path.as_str() {
        "/court" => reply(
            "302 Found",
            &[
                ("Location", "/fichiers/b%C3%A9b%C3%A9.iso"),
                ("Content-Length", "0"),
            ],
            b"",
        ),
        _ => reply(
            "200 OK",
            &[("Content-Length", "7"), ("Accept-Ranges", "bytes")],
            b"",
        ),
    });
    let probe = client().probe(&server.url("/court"), &[]).await.unwrap();
    assert_eq!(probe.final_url.path(), "/fichiers/b%C3%A9b%C3%A9.iso");
    assert_eq!(probe.file_name, "bébé.iso");
}

/// Les en-têtes secrets d'une tâche (SPEC §5.1) suivent une redirection vers le même hôte,
/// jamais vers un autre : le cookie d'un site ne part pas chez son hébergeur de fichiers.
/// Le `Referer` reste celui de la tâche, et pas l'adresse qui redirige (SPEC §8.2).
#[tokio::test]
async fn secret_headers_do_not_follow_a_redirect_to_another_host() {
    let elsewhere = serve(|_| {
        reply(
            "200 OK",
            &[("Content-Length", "7"), ("Accept-Ranges", "bytes")],
            b"",
        )
    });
    let target = elsewhere.url("/f.bin");
    let site = serve(move |r| match r.path.as_str() {
        "/ici" => reply(
            "302 Found",
            &[("Location", "/f.bin"), ("Content-Length", "0")],
            b"",
        ),
        "/ailleurs" => reply(
            "302 Found",
            &[("Location", target.as_str()), ("Content-Length", "0")],
            b"",
        ),
        _ => reply(
            "200 OK",
            &[("Content-Length", "7"), ("Accept-Ranges", "bytes")],
            b"",
        ),
    });
    let headers = [
        ("Cookie".to_owned(), "session=1".to_owned()),
        ("Authorization".to_owned(), "Bearer jeton".to_owned()),
        ("Referer".to_owned(), "https://exemple.org/".to_owned()),
    ];
    client().probe(&site.url("/ici"), &headers).await.unwrap();
    let followed = site.requests();
    let last = followed.last().unwrap();
    assert_eq!(last.path, "/f.bin");
    assert_eq!(last.header("cookie"), Some("session=1"));
    assert_eq!(last.header("authorization"), Some("Bearer jeton"));

    client()
        .probe(&site.url("/ailleurs"), &headers)
        .await
        .unwrap();
    let away = elsewhere.requests();
    assert!(!away.is_empty());
    for r in away {
        assert_eq!(r.header("cookie"), None);
        assert_eq!(r.header("authorization"), None);
        assert_eq!(r.header("referer"), Some("https://exemple.org/"));
    }
    // Sans `Referer` dans la tâche, aucun après la redirection.
    let before = elsewhere.requests().len();
    client().probe(&site.url("/ailleurs"), &[]).await.unwrap();
    for r in &elsewhere.requests()[before..] {
        assert_eq!(r.header("referer"), None);
    }
}

#[tokio::test]
async fn redirect_to_another_protocol_is_refused() {
    let server = serve(|_| {
        reply(
            "302 Found",
            &[("Location", "file:///etc/passwd"), ("Content-Length", "0")],
            b"",
        )
    });
    let err = client().probe(&server.url("/a"), &[]).await.unwrap_err();
    assert!(
        matches!(err, NetError::RedirectNotFollowed { status: 302 }),
        "{err:?}"
    );
    assert_eq!(err.error_code(), Some("HTTP_STATUS"));
    assert!(!err.retryable());
    assert_eq!(server.requests().len(), 1, "ni suivie ni retentée en GET");
}

#[tokio::test]
async fn redirect_loop_stops() {
    let server = serve(|_| {
        reply(
            "302 Found",
            &[("Location", "/a"), ("Content-Length", "0")],
            b"",
        )
    });
    let err = client().probe(&server.url("/a"), &[]).await.unwrap_err();
    assert!(matches!(err, NetError::Redirect(_)), "{err:?}");
    assert_eq!(server.requests().len(), MAX_REDIRECTS + 1);
}

#[tokio::test]
async fn http_errors_are_classified() {
    let server = serve(|r| match r.path.as_str() {
        "/absent" => reply("404 Not Found", &[("Content-Length", "0")], b""),
        _ => reply(
            "503 Service Unavailable",
            &[("Content-Length", "0"), ("Retry-After", "120")],
            b"",
        ),
    });
    let err = client()
        .probe(&server.url("/absent"), &[])
        .await
        .unwrap_err();
    assert!(matches!(err, NetError::Status { status: 404, .. }));
    assert!(!err.is_overload());
    assert_eq!(err.error_code(), Some("HTTP_STATUS"));
    assert!(!err.retryable());

    let err = client()
        .probe(&server.url("/occupe"), &[])
        .await
        .unwrap_err();
    assert!(matches!(err, NetError::Status { status: 503, .. }));
    assert!(err.is_overload());
    assert!(err.retryable());
    assert_eq!(err.retry_after(), Some(Duration::from_secs(120)));
}

#[tokio::test]
async fn closed_port_is_unreachable_and_the_url_is_not_leaked() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let err = client()
        .probe(&format!("http://127.0.0.1:{port}/f?token=secret"), &[])
        .await
        .unwrap_err();
    assert_eq!(err.error_code(), Some("NET_UNREACHABLE"), "{err:?}");
    assert!(err.is_overload(), "connexion refusée");
    assert!(err.retryable());
    let mut text = err.to_string();
    let mut source = std::error::Error::source(&err);
    while let Some(s) = source {
        text.push_str(&s.to_string());
        source = s.source();
    }
    assert!(!text.contains("secret"), "{text}");
}

#[tokio::test]
async fn silent_server_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/lent", listener.local_addr().unwrap());
    thread::spawn(move || {
        // Accepte et lit, sans jamais répondre.
        let mut open = Vec::new();
        for mut s in listener.incoming().flatten() {
            let _ = s.read(&mut [0; 1024]);
            open.push(s);
        }
    });
    let client = Client::with_timeouts(Duration::from_secs(5), Duration::from_millis(300)).unwrap();
    let err = client.probe(&url, &[]).await.unwrap_err();
    assert_eq!(err.error_code(), Some("NET_TIMEOUT"), "{err:?}");
    assert!(err.retryable());
}

#[tokio::test]
#[cfg_attr(
    target_os = "android",
    ignore = "HTTPS indisponible sous Android (tls_available)"
)]
async fn untrusted_certificate_is_refused_without_retry() {
    let key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert = key.cert.der().clone();
    let private = rustls::pki_types::PrivateKeyDer::Pkcs8(key.signing_key.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert], private)
    .unwrap();
    let config = Arc::new(config);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for tcp in listener.incoming().flatten() {
            let conn = rustls::ServerConnection::new(Arc::clone(&config)).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            let _ = tls.read(&mut [0; 1024]);
        }
    });

    let err = client()
        .probe(&format!("https://localhost:{port}/f"), &[])
        .await
        .unwrap_err();
    assert!(matches!(err, NetError::InvalidCertificate(_)), "{err:?}");
    assert_eq!(err.error_code(), Some("TLS_INVALID_CERT"));
    assert!(!err.retryable());
}

#[tokio::test]
async fn invalid_requests_are_refused_before_any_connection() {
    let c = client();
    assert!(matches!(
        c.probe("pas une url", &[]).await,
        Err(NetError::InvalidUrl(_))
    ));
    let err = c.probe("ftp://exemple.org/a", &[]).await.unwrap_err();
    assert!(matches!(err, NetError::UnsupportedScheme(ref s) if s == "ftp"));
    assert_eq!(err.error_code(), None);
    let bad = [("Authorization".to_owned(), "Bearer a\nb".to_owned())];
    let err = c.probe("http://127.0.0.1:1/", &bad).await.unwrap_err();
    assert!(matches!(err, NetError::BadHeader(_)));
    assert!(!err.to_string().contains("Bearer"), "{err}");
}

mod segments {
    use std::sync::Mutex;

    use super::*;
    use crate::testing::{FileSite, TestTls, content, serve_file, serve_file_tls};

    async fn read_all(mut body: Body) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(chunk) = body.chunk().await.unwrap() {
            out.extend_from_slice(&chunk);
        }
        out
    }

    fn range(first: u64, last: u64, total: u64, if_range: Option<&str>) -> RangeRequest<'_> {
        RangeRequest {
            first,
            last,
            total,
            if_range,
        }
    }

    #[tokio::test]
    async fn matching_range_is_streamed() {
        let data = content(10_000);
        let server = serve_file(Arc::new(Mutex::new(FileSite::new(data.len() as u64))));
        let body = client()
            .get_range(
                &server.url("/f"),
                &[],
                &range(100, 4099, 10_000, Some("\"v1\"")),
            )
            .await
            .unwrap();
        assert_eq!(read_all(body).await, data[100..4100]);
        let r = &server.requests()[0];
        assert_eq!(r.header("range"), Some("bytes=100-4099"));
        assert_eq!(r.header("if-range"), Some("\"v1\""));
        assert_eq!(r.header("accept-encoding"), Some("identity"));
    }

    #[tokio::test]
    #[cfg_attr(
        target_os = "android",
        ignore = "HTTPS indisponible sous Android (tls_available)"
    )]
    async fn added_authority_is_trusted_and_nothing_else_changes() {
        let tls = TestTls::new();
        let data = content(10_000);
        let site = Arc::new(Mutex::new(FileSite::new(data.len() as u64)));
        let server = serve_file_tls(site, None, &tls);
        // Sans l'autorité : refusé, comme tout certificat inconnu.
        let err = client().probe(&server.url("/f"), &[]).await.unwrap_err();
        assert!(matches!(err, NetError::InvalidCertificate(_)), "{err:?}");

        let trusting = Client::with_extra_roots(tls.ca_pem.as_bytes()).unwrap();
        let probe = trusting.probe(&server.url("/f"), &[]).await.unwrap();
        assert_eq!(probe.total_bytes, Some(10_000));
        assert!(probe.accepts_ranges);
        let body = trusting
            .get_range(
                &server.url("/f"),
                &[],
                &range(100, 4099, 10_000, Some("\"v1\"")),
            )
            .await
            .unwrap();
        assert_eq!(read_all(body).await, data[100..4100]);

        // Une autre autorité ne suffit pas : la validation reste stricte (N-07).
        let other = Client::with_extra_roots(TestTls::new().ca_pem.as_bytes()).unwrap();
        let err = other.probe(&server.url("/f"), &[]).await.unwrap_err();
        assert!(matches!(err, NetError::InvalidCertificate(_)), "{err:?}");
    }

    /// Sous Android, tant que TLS n'est pas initialisé, HTTPS est refusé par une erreur,
    /// directement comme après une redirection, sans même joindre le serveur ; ailleurs,
    /// les deux passent.
    #[tokio::test]
    async fn https_depends_on_tls_availability() {
        let tls = TestTls::new();
        let secure = serve_file_tls(Arc::new(Mutex::new(FileSite::new(1000))), None, &tls);
        let target = secure.url("/f");
        let redirect = serve(move |_| {
            reply(
                "302 Found",
                &[("Location", target.as_str()), ("Content-Length", "0")],
                b"",
            )
        });
        let c = match Client::with_extra_roots(tls.ca_pem.as_bytes()) {
            Ok(c) => c,
            Err(NetError::TlsUnavailable) if !tls_available() => client(),
            Err(e) => panic!("{e:?}"),
        };
        for url in [secure.url("/f"), redirect.url("/a")] {
            let result = c.probe(&url, &[]).await;
            if tls_available() {
                assert_eq!(result.unwrap().total_bytes, Some(1000));
            } else {
                let err = result.unwrap_err();
                assert!(matches!(err, NetError::TlsUnavailable), "{err:?}");
                assert_eq!(err.error_code(), Some("TLS_UNAVAILABLE"));
                assert!(!err.retryable());
            }
        }
        if !tls_available() {
            assert!(secure.requests().is_empty());
        }
    }

    #[test]
    fn unreadable_authorities_are_refused() {
        for pem in [&b""[..], b"pas un certificat"] {
            let err = Client::with_extra_roots(pem).unwrap_err();
            assert!(matches!(err, NetError::BadRootCertificates(_)), "{err:?}");
            assert_eq!(err.error_code(), None);
        }
    }

    #[tokio::test]
    async fn changed_validator_means_file_changed() {
        let site = Arc::new(Mutex::new(FileSite::new(1000)));
        let server = serve_file(Arc::clone(&site));
        site.lock().unwrap().etag = Some("\"v2\"".into());
        let err = client()
            .get_range(&server.url("/f"), &[], &range(0, 99, 1000, Some("\"v1\"")))
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::FileChanged), "{err:?}");
        assert_eq!(err.error_code(), Some("FILE_CHANGED"));
        assert!(!err.retryable());
    }

    #[tokio::test]
    async fn changed_size_means_file_changed() {
        let server = serve_file(Arc::new(Mutex::new(FileSite::new(1000))));
        let c = client();
        let err = c
            .get_range(&server.url("/f"), &[], &range(0, 99, 2000, None))
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::FileChanged), "{err:?}");
        // Fichier raccourci : la plage n'existe plus (416).
        let err = c
            .get_range(&server.url("/f"), &[], &range(1500, 1999, 1000, None))
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::FileChanged), "{err:?}");
    }

    #[tokio::test]
    async fn range_ignored_without_validator() {
        let site = Arc::new(Mutex::new(FileSite::new(1000)));
        site.lock().unwrap().ranges = false;
        let server = serve_file(site);
        let err = client()
            .get_range(&server.url("/f"), &[], &range(10, 99, 1000, None))
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::RangeIgnored), "{err:?}");
        assert_eq!(err.error_code(), Some("RANGE_UNSUPPORTED"));
    }

    #[tokio::test]
    async fn cut_body_is_a_retryable_network_error() {
        let site = Arc::new(Mutex::new(FileSite::new(100_000)));
        {
            let mut s = site.lock().unwrap();
            s.cut_responses = 1;
            s.cut_after = 5000;
        }
        let server = serve_file(site);
        let mut body = client()
            .get_range(&server.url("/f"), &[], &range(0, 99_999, 100_000, None))
            .await
            .unwrap();
        let mut received = 0;
        let err = loop {
            match body.chunk().await {
                Ok(Some(c)) => received += c.len(),
                Ok(None) => panic!("fin propre après {received} octets"),
                Err(e) => break e,
            }
        };
        assert_eq!(received, 5000);
        assert!(err.retryable(), "{err:?}");
    }

    #[tokio::test]
    async fn whole_file_checks_the_expected_size() {
        let data = content(3000);
        let server = serve_file(Arc::new(Mutex::new(FileSite::new(data.len() as u64))));
        let c = client();
        let body = c.get_all(&server.url("/f"), &[], Some(3000)).await.unwrap();
        assert_eq!(read_all(body).await, data);
        assert!(server.requests()[0].header("range").is_none());
        let err = c
            .get_all(&server.url("/f"), &[], Some(2999))
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::FileChanged), "{err:?}");
    }
}

/// Identification d'une tâche (F-06, SPEC §7.14) face au serveur de test.
mod authentication {
    use std::sync::Mutex;

    use super::*;
    use crate::net::auth::{Auth, Login, Site};
    use crate::testing::{FileSite, SiteAuth, TestTls, serve_file, serve_file_tls};

    fn protected(auth: SiteAuth) -> crate::testing::Server {
        let mut site = FileSite::new(10_000);
        site.auth = Some(auth);
        serve_file(Arc::new(Mutex::new(site)))
    }

    fn login(site: &str, password: &str) -> Auth {
        Auth::new(Login {
            site: Site::parse(site).unwrap(),
            username: "moi".into(),
            password: password.into(),
        })
    }

    fn authorizations(server: &crate::testing::Server) -> Vec<Option<String>> {
        server
            .requests()
            .iter()
            .map(|r| r.header("authorization").map(str::to_owned))
            .collect()
    }

    #[tokio::test]
    async fn digest_is_answered_then_sent_with_every_request() {
        for algorithm in ["MD5", "SHA-256", "SHA-256-sess"] {
            let server = protected(SiteAuth::digest("moi", "secret", algorithm));
            let url = server.url("/f.bin?jeton=1");
            let auth = login("127.0.0.1", "secret");
            let probe = client().probe_as(&url, &[], Some(&auth)).await.unwrap();
            assert_eq!(probe.total_bytes, Some(10_000), "{algorithm}");
            let req = RangeRequest {
                first: 0,
                last: 99,
                total: 10_000,
                if_range: None,
            };
            client()
                .get_range_as(&url, &[], &req, Some(&auth))
                .await
                .unwrap();
            let sent = authorizations(&server);
            // Le premier `HEAD` sans rien, refusé ; puis toujours l'identification.
            assert_eq!(sent[0], None, "{algorithm}");
            assert!(
                sent[1..].iter().all(Option::is_some),
                "{algorithm} : {sent:?}"
            );
            assert_eq!(sent.len(), 3, "un seul refus : {sent:?}");
            assert!(!sent.iter().flatten().any(|v| v.contains("secret")));
        }
    }

    #[tokio::test]
    async fn wrong_or_missing_credentials_are_reported() {
        let server = protected(SiteAuth::digest("moi", "secret", "MD5"));
        let url = server.url("/f.bin");
        let err = client().probe(&url, &[]).await.unwrap_err();
        assert!(matches!(err, NetError::AuthRequired), "{err:?}");
        assert_eq!(err.error_code(), Some("AUTH_REQUIRED"));
        assert!(!err.retryable());

        let wrong = login("127.0.0.1", "faux");
        let err = client()
            .probe_as(&url, &[], Some(&wrong))
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::AuthRefused), "{err:?}");
        assert_eq!(err.error_code(), Some("AUTH_FAILED"));

        // Identifiants d'un autre site : rien n'est envoyé.
        let elsewhere = login("exemple.org", "secret");
        let before = server.requests().len();
        let err = client()
            .probe_as(&url, &[], Some(&elsewhere))
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::AuthRequired), "{err:?}");
        assert!(
            authorizations(&server)[before..]
                .iter()
                .all(Option::is_none)
        );
    }

    #[tokio::test]
    async fn basic_goes_over_https_only() {
        // En clair : refusé, sans jamais envoyer le mot de passe.
        let plain = protected(SiteAuth::basic("moi", "secret"));
        let auth = login("127.0.0.1", "secret");
        let err = client()
            .probe_as(&plain.url("/f.bin"), &[], Some(&auth))
            .await
            .unwrap_err();
        assert!(matches!(err, NetError::AuthInsecure), "{err:?}");
        assert!(authorizations(&plain).iter().all(Option::is_none));

        if !tls_available() {
            return;
        }
        let tls = TestTls::new();
        let mut site = FileSite::new(10_000);
        site.auth = Some(SiteAuth::basic("moi", "secret"));
        let server = serve_file_tls(Arc::new(Mutex::new(site)), None, &tls);
        let trusting = Client::with_extra_roots(tls.ca_pem.as_bytes()).unwrap();
        let probe = trusting
            .probe_as(&server.url("/f.bin"), &[], Some(&auth))
            .await
            .unwrap();
        assert_eq!(probe.total_bytes, Some(10_000));
        assert_eq!(
            authorizations(&server).last().unwrap().as_deref(),
            Some("Basic bW9pOnNlY3JldA==")
        );
    }
}

/// Tâche venue d'une connexion à jeton (SPEC §7.19) : le client restreint ne joint pas le
/// serveur local, ni par son adresse ni par `localhost`, et ne contacte jamais le serveur ;
/// le client ordinaire le joint.
#[tokio::test]
async fn a_restricted_client_never_reaches_internal_addresses() {
    let server = serve(|_| reply("200 OK", &[("Content-Length", "3")], b"abc"));
    let open = client();
    let restricted = open.restricted().unwrap();
    assert!(restricted.is_public_only() && !open.is_public_only());
    assert!(open.reaching(false).map(|c| !c.is_public_only()).unwrap());
    let url = server.url("/f");
    let by_name = url.replace("127.0.0.1", "localhost");
    for target in [&url, &by_name] {
        let refused = restricted.probe(target, &[]).await.unwrap_err();
        assert!(matches!(refused, NetError::AddressForbidden), "{refused:?}");
        assert_eq!(refused.error_code(), Some("ADDRESS_FORBIDDEN"));
        assert!(!refused.retryable());
    }
    assert!(server.requests().is_empty());
    assert_eq!(open.probe(&url, &[]).await.unwrap().total_bytes, Some(3));
    // Le client restreint est fabriqué une fois, puis partagé.
    assert!(restricted.restricted().unwrap().is_public_only());
}

/// Le statut relu dans le détail d'un échec enregistré (plugins, SPEC §7.20) suit le
/// message de `NetError::Status`.
#[test]
fn status_detail_round_trip() {
    for status in [403, 410, 503] {
        let detail = NetError::Status {
            status,
            retry_after: None,
        }
        .to_string();
        assert_eq!(super::status_in_detail(&detail), Some(status), "{detail}");
    }
    assert_eq!(super::status_in_detail("délai dépassé"), None);
}

/// Proxy (F-07) : les requêtes passent par un proxy HTTP manuel, avec son identifiant ;
/// une exception et le mode « aucun » vont en direct ; toutes les copies d'un client
/// suivent un changement ; des réglages refusés ne changent rien.
#[tokio::test]
async fn requests_follow_the_proxy_settings() {
    use crate::testing::proxies::http_proxy;
    use proxy::{Effective, Mode, Settings};

    let site = crate::testing::serve_file(Arc::new(std::sync::Mutex::new(
        crate::testing::FileSite::new(1000),
    )));
    let relay = http_proxy();
    let client = client();
    let copy = client.clone();
    let manual = |exceptions: Vec<String>| Effective {
        settings: Settings {
            mode: Mode::Manual,
            url: Some(relay.url("http")),
            exceptions,
            username: Some("moi".into()),
        },
        password: Some("secret".into()),
    };
    client.set_proxy(manual(Vec::new())).unwrap();
    let url = site.url("/f.bin");
    assert_eq!(copy.probe(&url, &[]).await.unwrap().total_bytes, Some(1000));
    let seen = relay.seen();
    assert!(
        !seen.is_empty() && seen.iter().all(|s| s.target == url),
        "{seen:?}"
    );
    assert_eq!(
        seen[0].credentials.as_deref(),
        Some("Basic bW9pOnNlY3JldA==")
    );

    let before = relay.seen().len();
    client.set_proxy(manual(vec!["127.0.0.1".into()])).unwrap();
    copy.probe(&url, &[]).await.unwrap();
    assert_eq!(relay.seen().len(), before, "exception : en direct");
    let none = Effective {
        settings: Settings {
            mode: Mode::None,
            ..Settings::default()
        },
        password: None,
    };
    client.set_proxy(none).unwrap();
    copy.probe(&url, &[]).await.unwrap();
    assert_eq!(relay.seen().len(), before, "aucun : en direct");

    let refused = Effective {
        settings: Settings {
            mode: Mode::Manual,
            url: Some("ftp://p.lan:21".into()),
            ..Settings::default()
        },
        password: None,
    };
    assert!(client.set_proxy(refused).is_err());
    assert_eq!(client.proxy().settings.mode, Mode::None);
}

/// SOCKS5 (F-07) : `socks5h` laisse le proxy résoudre le nom, `socks5` le résout ici ;
/// l'identifiant est donné. Une connexion limitée au réseau public refuse une adresse
/// interne avant même de joindre le proxy.
#[tokio::test]
async fn socks5_proxies_resolve_where_asked_and_public_connections_stay_checked() {
    use crate::testing::proxies::socks5_proxy;
    use proxy::{Effective, Mode, Settings};

    let site = crate::testing::serve_file(Arc::new(std::sync::Mutex::new(
        crate::testing::FileSite::new(1000),
    )));
    let socks = socks5_proxy(Some(("moi", "secret")));
    let client = client();
    let via = |scheme: &str| Effective {
        settings: Settings {
            mode: Mode::Manual,
            url: Some(socks.url(scheme)),
            exceptions: Vec::new(),
            username: Some("moi".into()),
        },
        password: Some("secret".into()),
    };
    let url = site.url("/f.bin");
    let named = url.replace("127.0.0.1", "localhost");

    client.set_proxy(via("socks5h")).unwrap();
    client.probe(&named, &[]).await.unwrap();
    let seen = socks.seen();
    assert!(
        seen.iter().all(|s| s.by_name
            && s.target.starts_with("localhost:")
            && s.credentials.as_deref() == Some("moi")),
        "{seen:?}"
    );

    client.set_proxy(via("socks5")).unwrap();
    let before = socks.seen().len();
    client.probe(&url, &[]).await.unwrap();
    let last = socks.seen().pop().unwrap();
    assert!(socks.seen().len() > before && !last.by_name, "{last:?}");

    // Réseau public seulement : `localhost` est refusé par le résolveur filtrant, avant le
    // proxy ; une adresse interne écrite telle quelle aussi.
    let restricted = client.restricted().unwrap();
    let before = socks.seen().len();
    assert!(restricted.probe(&named, &[]).await.is_err());
    assert!(matches!(
        restricted.probe(&url, &[]).await,
        Err(NetError::AddressForbidden)
    ));
    assert_eq!(socks.seen().len(), before, "rien n'a joint le proxy");
}
