use super::*;
use serde_json::json;
use std::{io::Write, net::TcpListener, thread};

const KEY: &str = include_str!("../../../../resources/test-fixtures/updater/public.key");
const SIGNATURE: &str = include_str!("../../../../resources/test-fixtures/updater/payload.txt.sig");
const PAYLOAD: &[u8] = include_bytes!("../../../../resources/test-fixtures/updater/payload.txt");
const TARGETS: [Target; 3] = [Target::MacArm64, Target::WindowsX64, Target::WindowsArm64];

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n").into_bytes();
    bytes.extend_from_slice(body);
    bytes
}
fn ok(body: &[u8]) -> Vec<u8> {
    response(
        "200 OK",
        &format!("Content-Length: {}\r\n", body.len()),
        body,
    )
}
fn manifest(base: &str, target: Target, version: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"version":version, "platforms":{target.key():{
        "url":format!("{base}/payload"), "signature":SIGNATURE.trim()
    }}}))
    .unwrap()
}
fn serve(make: impl FnOnce(&str) -> Vec<Vec<u8>>) -> (Url, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let responses = make(&base);
    let task = thread::spawn(move || {
        for response in responses {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "fixture received fewer requests than expected"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
                assert!(headers.len() < 8192);
            }
            // Clients may reject a size header or cancel before the body arrives.
            let _ = socket.write_all(&response);
        }
    });
    (Url::parse(&format!("{base}/latest.json")).unwrap(), task)
}
fn local(endpoint: Url, current: &str, target: Target) -> Updater {
    Updater::configured(endpoint, KEY, current, target, Transport::Loopback).unwrap()
}

#[test]
fn downloads_and_verifies_the_shipping_format_for_every_target() {
    for target in TARGETS {
        let (endpoint, server) =
            serve(|base| vec![ok(&manifest(base, target, "2.0.0")), ok(PAYLOAD)]);
        let mut events = Vec::new();
        let update = local(endpoint, "1.0.0", target)
            .check(|| false, |progress| events.push(progress))
            .unwrap()
            .unwrap();
        server.join().unwrap();
        assert_eq!(update.bytes(), PAYLOAD);
        assert_eq!(update.artifact_name(), Some("payload.txt"));
        assert_eq!(update.target(), target);
        assert_eq!(update.version(), &version("2.0.0").unwrap());
        assert_eq!(events.first().unwrap().downloaded, 0);
        assert_eq!(events.last().unwrap().downloaded, PAYLOAD.len() as u64);
        assert_eq!(events.last().unwrap().total, Some(PAYLOAD.len() as u64));
    }
}

#[test]
fn rejects_modified_payload_and_manifest_version_substitution() {
    for target in TARGETS {
        for (announced, body) in [("2.0.0", b"tampered".as_slice()), ("3.0.0", PAYLOAD)] {
            let (endpoint, server) =
                serve(|base| vec![ok(&manifest(base, target, announced)), ok(body)]);
            assert!(
                local(endpoint, "1.0.0", target)
                    .check(|| false, |_| {})
                    .is_err()
            );
            server.join().unwrap();
        }
    }
}

#[test]
fn never_reinstalls_or_downgrades_from_unsigned_manifest_metadata() {
    for current in ["2.0.0", "2.0.0+build.99", "3.0.0"] {
        let (endpoint, server) =
            serve(|base| vec![ok(&manifest(base, Target::MacArm64, "v2.0.0"))]);
        assert!(
            local(endpoint, current, Target::MacArm64)
                .check(|| false, |_| {})
                .unwrap()
                .is_none()
        );
        server.join().unwrap();
    }
}

#[test]
fn checks_the_global_signature_before_trusting_its_version_comment() {
    let updater = local(
        Url::parse("http://127.0.0.1:1/").unwrap(),
        "1.0.0",
        Target::MacArm64,
    );
    let text = decode_text(SIGNATURE).unwrap();
    for edited in [
        text.replace("version:2.0.0", "version:3.0.0"),
        text.replace("\tversion:2.0.0", ""),
        text.replace("version:2.0.0", "version:2.0.0\tversion:3.0.0"),
    ] {
        let signature = Signature::decode(&edited).unwrap();
        assert!(
            updater
                .verify(PAYLOAD.to_vec(), signature, version("3.0.0").unwrap())
                .is_err()
        );
    }
    let production = Updater::production("1.0.0", Target::MacArm64).unwrap();
    assert!(
        production
            .verify(
                PAYLOAD.to_vec(),
                Signature::decode(&text).unwrap(),
                version("2.0.0").unwrap()
            )
            .is_err()
    );
}

#[test]
fn accepts_https_only_and_checks_redirects_before_following_them() {
    for url in [
        "http://example.com/latest.json",
        "https://user:pass@example.com/latest.json",
        "https://example.com/latest.json#fragment",
    ] {
        assert!(Updater::new(url, KEY, "1.0.0", Target::MacArm64).is_err());
    }
    let (endpoint, server) = serve(|_| {
        vec![response(
            "302 Found",
            "Location: http://example.com/payload\r\nContent-Length: 0\r\n",
            b"",
        )]
    });
    let result = local(endpoint, "1.0.0", Target::MacArm64).check(|| false, |_| {});
    server.join().unwrap();
    assert!(result.err().unwrap().contains("HTTPS"));
    let (endpoint, server) = serve(|base| {
        vec![
            response(
                "302 Found",
                "Location: /manifest\r\nContent-Length: 0\r\n",
                b"",
            ),
            ok(&manifest(base, Target::MacArm64, "2.0.0")),
            ok(PAYLOAD),
        ]
    });
    assert!(
        local(endpoint, "1.0.0", Target::MacArm64)
            .check(|| false, |_| {})
            .unwrap()
            .is_some()
    );
    server.join().unwrap();
}

#[test]
fn refuses_wrong_target_bad_metadata_and_insecure_artifact_urls() {
    let updater = Updater::new(
        "https://example.com/latest.json",
        KEY,
        "1.0.0",
        Target::MacArm64,
    )
    .unwrap();
    for body in [
        b"invalid json".to_vec(),
        manifest("https://example.com", Target::WindowsX64, "2.0.0"),
        manifest("http://example.com", Target::MacArm64, "2.0.0"),
        manifest("https://example.com", Target::MacArm64, "nonsense"),
    ] {
        assert!(updater.select(&body).is_err());
    }
    assert!(decode_text("not-base64").is_err());
    assert!(decode_text(&"A".repeat(SIGNATURE_LIMIT + 1)).is_err());
}

#[test]
fn rejects_oversize_and_truncated_bodies_and_reports_no_update() {
    for response in [
        response(
            "200 OK",
            &format!("Content-Length: {}\r\n", MANIFEST_LIMIT + 1),
            b"",
        ),
        response("200 OK", "Content-Length: 900\r\n", b"{}"),
    ] {
        let (endpoint, server) = serve(|_| vec![response]);
        assert!(
            local(endpoint, "1.0.0", Target::MacArm64)
                .check(|| false, |_| {})
                .is_err()
        );
        server.join().unwrap();
    }
    let (endpoint, server) = serve(|_| vec![response("204 No Content", "", b"")]);
    assert!(
        local(endpoint, "1.0.0", Target::MacArm64)
            .check(|| false, |_| {})
            .unwrap()
            .is_none()
    );
    server.join().unwrap();
}

#[test]
fn rejects_authentic_signatures_with_missing_or_ambiguous_versions() {
    let key = include_str!("../../../../resources/test-fixtures/updater/version-fields.public.key");
    let updater = Updater::new(
        "https://example.com/latest.json",
        key,
        "1.0.0",
        Target::MacArm64,
    )
    .unwrap();
    for encoded in [
        include_str!("../../../../resources/test-fixtures/updater/versionless.sig"),
        include_str!("../../../../resources/test-fixtures/updater/duplicate-version.sig"),
    ] {
        let signature = Signature::decode(&decode_text(encoded).unwrap()).unwrap();
        updater
            .public_key
            .verify(PAYLOAD, &signature, true)
            .unwrap();
        assert!(
            updater
                .verify(PAYLOAD.to_vec(), signature, version("2.0.0").unwrap())
                .is_err()
        );
    }
}

#[test]
fn chunked_downloads_report_unknown_length_and_oversize_payloads_are_refused() {
    let (endpoint, server) = serve(|base| {
        let mut body = format!("{:x}\r\n", PAYLOAD.len()).into_bytes();
        body.extend_from_slice(PAYLOAD);
        body.extend_from_slice(b"\r\n0\r\n\r\n");
        vec![
            ok(&manifest(base, Target::MacArm64, "2.0.0")),
            response("200 OK", "Transfer-Encoding: chunked\r\n", &body),
        ]
    });
    let mut last = None;
    let update = local(endpoint, "1.0.0", Target::MacArm64)
        .check(|| false, |progress| last = Some(progress))
        .unwrap()
        .unwrap();
    server.join().unwrap();
    assert_eq!(update.bytes(), PAYLOAD);
    assert_eq!(last.unwrap().total, None);
    let (endpoint, server) = serve(|base| {
        vec![
            ok(&manifest(base, Target::MacArm64, "2.0.0")),
            response(
                "200 OK",
                &format!("Content-Length: {}\r\n", PAYLOAD_LIMIT + 1),
                b"",
            ),
        ]
    });
    assert!(
        local(endpoint, "1.0.0", Target::MacArm64)
            .check(|| false, |_| {})
            .err()
            .unwrap()
            .contains("size limit")
    );
    server.join().unwrap();
}

#[test]
fn limits_redirect_loops_and_bodies_without_a_content_length() {
    let (endpoint, server) = serve(|_| {
        (0..=REDIRECT_LIMIT)
            .map(|_| {
                response(
                    "302 Found",
                    "Location: /latest.json\r\nContent-Length: 0\r\n",
                    b"",
                )
            })
            .collect()
    });
    let error = local(endpoint, "1.0.0", Target::MacArm64)
        .check(|| false, |_| {})
        .err()
        .unwrap();
    assert!(error.contains("Too many"));
    server.join().unwrap();
    let (endpoint, server) =
        serve(|_| vec![response("200 OK", "", &vec![b' '; MANIFEST_LIMIT + 1])]);
    let error = local(endpoint, "1.0.0", Target::MacArm64)
        .check(|| false, |_| {})
        .err()
        .unwrap();
    assert!(error.contains("size limit"));
    server.join().unwrap();
}

#[test]
fn cancelled_checks_never_return_a_verified_update() {
    let updater = local(
        Url::parse("http://127.0.0.1:1/").unwrap(),
        "1.0.0",
        Target::MacArm64,
    );
    assert!(
        updater
            .check(|| true, |_| {})
            .err()
            .unwrap()
            .contains("cancelled")
    );
    let (endpoint, server) =
        serve(|base| vec![ok(&manifest(base, Target::MacArm64, "2.0.0")), ok(PAYLOAD)]);
    let cancelled = std::cell::Cell::new(false);
    let result = local(endpoint, "1.0.0", Target::MacArm64).check(
        || cancelled.get(),
        |event| {
            if event.downloaded > 0 {
                cancelled.set(true);
            }
        },
    );
    assert!(result.err().unwrap().contains("cancelled"));
    server.join().unwrap();
}
