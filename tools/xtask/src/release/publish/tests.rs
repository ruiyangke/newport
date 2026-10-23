use super::*;

fn fixture(version: &str) -> (tempfile::TempDir, Plan) {
    let dir = tempfile::tempdir().unwrap();
    let fixtures = util::root().join("crates/newport-release/tests/fixtures");
    let key = newport_release::secret_key(
        &fs::read_to_string(fixtures.join("empty-password.key")).unwrap(),
        String::new(),
    )
    .unwrap();
    let public = fs::read_to_string(fixtures.join("empty-password.key.pub")).unwrap();
    let mut fragments = Vec::new();
    for (target, suffix) in [
        ("darwin-aarch64", "macos-arm64.app.tar.gz"),
        ("windows-x86_64", "windows-x86_64-setup.exe"),
        ("windows-aarch64", "windows-aarch64-setup.exe"),
    ] {
        let payload = dir.path().join(format!("Newport-{version}-{suffix}"));
        fs::write(&payload, target).unwrap();
        fragments.push(
            manifest::write_fragment(
                &payload,
                target,
                &format!("{target}.json"),
                version,
                &public,
                &key,
            )
            .unwrap(),
        );
    }
    fs::write(
        dir.path()
            .join(format!("{}.zip", manifest::macos_name(version))),
        "manual installer",
    )
    .unwrap();
    let manifest = manifest::merge(&fragments, version, dir.path()).unwrap();
    manifest.write(&dir.path().join("latest.json")).unwrap();
    manifest::write_checksums(&manifest, dir.path(), "latest.json").unwrap();
    fs::write(dir.path().join("private.key"), "must never be uploaded").unwrap();
    let plan = prepare(dir.path(), &public).unwrap();
    (dir, plan)
}

#[derive(Default)]
struct MemoryStore {
    objects: BTreeMap<String, Vec<u8>>,
    writes: Vec<String>,
    fail_key: Option<String>,
    corrupt_key: Option<String>,
    conflict_latest: bool,
}
impl Store for MemoryStore {
    fn get(&mut self, key: &str) -> Result<Option<Object>> {
        Ok(self.objects.get(key).map(|bytes| Object {
            bytes: bytes.clone(),
            etag: format!("\"{}\"", util::sha256_hex(bytes)),
        }))
    }
    fn put(&mut self, key: &str, bytes: &[u8], condition: Condition<'_>) -> Result<()> {
        ensure!(
            self.fail_key.as_deref() != Some(key),
            "simulated upload failure"
        );
        if key == LATEST && self.conflict_latest {
            bail!("concurrent publisher changed the feed");
        }
        let existing = self.get(key)?;
        match condition {
            Condition::Missing => ensure!(existing.is_none(), "conditional create failed"),
            Condition::Matches(etag) => ensure!(
                existing.is_some_and(|old| old.etag == etag),
                "conditional replacement failed"
            ),
        }
        let bytes = if self.corrupt_key.as_deref() == Some(key) {
            b"corrupted"
        } else {
            bytes
        };
        self.objects.insert(key.into(), bytes.to_vec());
        self.writes.push(key.into());
        Ok(())
    }
}

#[test]
fn publication_promotes_last_and_identical_retries_do_not_rewrite_objects() {
    let (_dir, plan) = fixture("1.2.3");
    let mut store = MemoryStore::default();
    publish(&plan, &mut store).unwrap();
    assert_eq!(store.writes.last().unwrap(), LATEST);
    assert_eq!(store.objects.len(), 11); // nine files, record, active feed
    assert!(!store.objects.keys().any(|key| key.contains("private.key")));
    let writes = store.writes.clone();
    publish(&plan, &mut store).unwrap();
    assert_eq!(store.writes, writes);
    assert_eq!(store.objects[LATEST], plan.manifest);
}

#[test]
fn interrupted_or_corrupt_uploads_never_change_the_active_feed_and_can_resume() {
    let (_dir, plan) = fixture("1.2.3");
    let failure = "releases/1.2.3/SHA256SUMS";
    let mut store = MemoryStore {
        fail_key: Some(failure.into()),
        ..Default::default()
    };
    let mut previous = plan.release.manifest.clone();
    previous.version = "1.2.2".into();
    let previous = serde_json::to_vec(&previous).unwrap();
    store.objects.insert(LATEST.into(), previous.clone());
    assert!(publish(&plan, &mut store).is_err());
    assert_eq!(store.objects[LATEST], previous);
    assert!(!store.objects.contains_key("releases/1.2.3/release.json"));
    store.fail_key = None;
    publish(&plan, &mut store).unwrap();
    assert_eq!(store.objects[LATEST], plan.manifest);

    let mut store = MemoryStore {
        corrupt_key: Some(failure.into()),
        ..Default::default()
    };
    assert!(publish(&plan, &mut store)
        .unwrap_err()
        .to_string()
        .contains("failed verification"));
    assert!(!store.objects.contains_key(LATEST));
}

#[test]
fn immutable_releases_and_newer_feeds_cannot_be_overwritten() {
    let (_dir, plan) = fixture("1.2.3");
    let mut store = MemoryStore::default();
    let mut newer = plan.release.manifest.clone();
    newer.version = "1.10.0".into();
    store
        .objects
        .insert(LATEST.into(), serde_json::to_vec(&newer).unwrap());
    assert!(publish(&plan, &mut store)
        .unwrap_err()
        .to_string()
        .contains("newer"));
    assert!(store.writes.is_empty());

    store.objects.clear();
    store.objects.insert(
        "releases/1.2.3/SHA256SUMS".into(),
        b"old version data".to_vec(),
    );
    assert!(publish(&plan, &mut store)
        .unwrap_err()
        .to_string()
        .contains("Published object differs"));
    assert!(!store.objects.contains_key(LATEST));

    let mut store = MemoryStore {
        conflict_latest: true,
        ..Default::default()
    };
    assert!(publish(&plan, &mut store)
        .unwrap_err()
        .to_string()
        .contains("concurrent"));
    assert!(!store.objects.contains_key(LATEST));
}

#[test]
fn verification_rejects_stale_checksums_missing_installers_and_changed_files() {
    let (dir, plan) = fixture("1.2.3");
    let public = fs::read_to_string(
        util::root().join("crates/newport-release/tests/fixtures/empty-password.key.pub"),
    )
    .unwrap();
    let zip = dir
        .path()
        .join(format!("{}.zip", manifest::macos_name("1.2.3")));
    fs::write(&zip, "changed installer").unwrap();
    assert!(prepare(dir.path(), &public)
        .unwrap_err_string()
        .contains("SHA256SUMS"));
    let mut store = MemoryStore::default();
    assert!(publish(&plan, &mut store)
        .unwrap_err()
        .to_string()
        .contains("changed after verification"));
    assert!(!store.objects.contains_key(LATEST));
    fs::remove_file(zip).unwrap();
    assert!(prepare(dir.path(), &public).is_err());
    for version in ["../1.2.3", "v1.2.3", "1.2.3-rc1", "1.2.3+build"] {
        assert!(stable_version(version).is_err());
    }
}

#[test]
fn s3_transport_signs_requests_and_passes_conditional_writes() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for method in ["GET", "PUT"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
            }
            let headers = String::from_utf8(headers).unwrap().to_lowercase();
            assert!(headers.starts_with(&method.to_lowercase()));
            assert!(headers.contains("authorization: aws4-hmac-sha256"));
            assert!(headers.contains("/auto/s3/aws4_request"));
            assert!(headers.contains("x-amz-content-sha256:"));
            if method == "PUT" {
                assert!(headers.contains("if-none-match: *"));
                let mut body = [0; 7];
                stream.read_exact(&mut body).unwrap();
                assert_eq!(&body, b"payload");
            }
            let status = if method == "GET" {
                "404 Not Found"
            } else {
                "200 OK"
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        }
    });
    let mut store = R2 {
        base: format!("http://{address}/test-bucket"),
        credentials: Credentials::new("test-access", "test-secret", None, None, "test"),
        agent: ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .new_agent(),
    };
    assert!(store.get("test-file").unwrap().is_none());
    store
        .put("test-file", b"payload", Condition::Missing)
        .unwrap();
    server.join().unwrap();
}

// Avoid requiring the secret-bearing transport or plan to implement Debug.
trait ErrorString {
    fn unwrap_err_string(self) -> String;
}
impl<T> ErrorString for Result<T> {
    fn unwrap_err_string(self) -> String {
        match self {
            Ok(_) => panic!("expected rejection"),
            Err(error) => error.to_string(),
        }
    }
}
