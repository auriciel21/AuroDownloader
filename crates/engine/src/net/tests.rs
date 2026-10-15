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
