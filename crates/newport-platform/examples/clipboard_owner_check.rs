//! Background clipboard owner acceptance checks using disposable native boards.
//! No general clipboard, running desktop, credential helper or server is used.
#[cfg(target_os = "macos")]
mod check {
    use anyhow::{Context, Result, bail, ensure};
    use newport_platform::clipboard::{
        helper::{
            self, Board,
            owner::{Owner, Snapshot},
        },
        provider::{Payload, Reply, Source},
    };
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::{NSData, NSString};
    use serde_json::{Value, json};
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};

    const TEXT: &str = "Newport 世界\n";
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    fn board(name: &str) -> objc2::rc::Retained<NSPasteboard> {
        assert!(
            name.strip_prefix("org.newport.test.lazy.")
                .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
        );
        NSPasteboard::pasteboardWithName(&NSString::from_str(name))
    }
    fn consumer(name: &str, operation: &str) -> Result<()> {
        let board = board(name);
        let value = if operation.starts_with("local-") {
            board.clearContents();
            let cleared = board.changeCount();
            if operation == "local-delayed" {
                std::thread::sleep(Duration::from_millis(150));
            }
            let bytes = match operation {
                "local-empty" => Vec::new(),
                "local-bulk" => vec![b'a'; 24 * 1024 * 1024],
                _ => TEXT.as_bytes().to_vec(),
            };
            if operation != "local-clear" {
                ensure!(board.setData_forType(
                    Some(&NSData::with_bytes(&bytes)),
                    &NSString::from_str("public.utf8-plain-text")
                ));
            }
            if operation == "local-rich" {
                ensure!(board.setData_forType(
                    Some(&NSData::with_bytes(PNG)),
                    &NSString::from_str("public.png")
                ));
            }
            json!({"revision":board.changeCount(), "cleared":cleared})
        } else if operation == "release" {
            board.clearContents();
            let _: () = unsafe { objc2::msg_send![&*board, releaseGlobally] };
            json!({"released":true})
        } else if operation == "revision" {
            json!({"revision":board.changeCount()})
        } else {
            let bytes = board
                .dataForType(&NSString::from_str(operation))
                .map(|b| b.to_vec());
            json!({"length":bytes.as_ref().map(Vec::len), "text":bytes.as_deref() == Some(TEXT.as_bytes()), "png":bytes.as_deref() == Some(PNG)})
        };
        println!("{value}");
        Ok(())
    }
    struct Consumer(Child);
    impl Drop for Consumer {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn result(mut consumer: Consumer) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(4);
        while consumer.0.try_wait()?.is_none() {
            ensure!(Instant::now() < deadline, "native consumer timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        ensure!(consumer.0.wait()?.success());
        let mut bytes = Vec::new();
        consumer.0.stdout.take().unwrap().read_to_end(&mut bytes)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
    struct Fixture {
        name: String,
        owner: Owner,
        directory: std::path::PathBuf,
    }
    impl Fixture {
        fn new() -> Result<Self> {
            let name = format!("org.newport.test.lazy.{}", uuid::Uuid::new_v4());
            let directory =
                std::env::temp_dir().join(format!("newport-owner-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&directory)?;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
            let executable = directory.join("helper");
            let quote = |path: &std::path::Path| {
                format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
            };
            // Record only this test's child PID before exec, for the crash test.
            std::fs::write(
                &executable,
                format!(
                    "#!/bin/sh\necho $$ > {}\nexec {} \"$@\"\n",
                    quote(&directory.join("pid")),
                    quote(&std::env::current_exe()?)
                ),
            )?;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
            let owner = Owner::new(executable, Board::Test { name: name.clone() });
            ensure!(
                !directory.join("pid").exists(),
                "construction eagerly started a helper"
            );
            Ok(Self {
                name,
                owner,
                directory,
            })
        }
        fn read(&self, operation: &str) -> Result<Consumer> {
            Ok(Consumer(
                Command::new(std::env::current_exe()?)
                    .args(["consumer", &self.name, operation])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()?,
            ))
        }
        async fn changed(&self, previous: Option<isize>) -> Result<Snapshot> {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let snapshot = self
                    .owner
                    .snapshot()
                    .await
                    .context("owner metadata unavailable")?;
                if Some(snapshot.revision) != previous {
                    return Ok(snapshot);
                }
                ensure!(
                    Instant::now() < deadline,
                    "owner did not observe native change"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        async fn copy(&self, operation: &str) -> Result<Snapshot> {
            let before = self.owner.snapshot().await.map(|s| s.revision);
            result(self.read(operation)?)?;
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let snapshot = self.changed(before).await?;
                if snapshot.formats.iter().any(|f| f == "text/plain")
                    && (operation != "local-rich"
                        || snapshot.formats.iter().any(|f| f == "image/png"))
                {
                    return Ok(snapshot);
                }
                ensure!(
                    Instant::now() < deadline,
                    "owner only observed the intermediate native clear"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.owner.close();
            let board = board(&self.name);
            board.clearContents();
            let _: () = unsafe { objc2::msg_send![&*board, releaseGlobally] };
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    struct Data(Vec<u8>);
    impl Payload for Data {
        fn bytes(&self) -> Option<&[u8]> {
            Some(&self.0)
        }
    }
    struct SourceFixture {
        current: AtomicBool,
        requests: AtomicUsize,
    }
    impl Source for SourceFixture {
        fn is_current(&self) -> bool {
            self.current.load(Ordering::Acquire)
        }
        fn request(&self, format: &str, reply: Reply) {
            self.requests.fetch_add(1, Ordering::AcqRel);
            let bytes = if format == "image/png" {
                PNG.to_vec()
            } else {
                TEXT.as_bytes().to_vec()
            };
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                reply.complete(Box::new(Data(bytes)));
            });
        }
    }
    async fn checks() -> Result<()> {
        let fixture = Fixture::new()?;
        let owner = &fixture.owner;
        let initial = owner
            .snapshot()
            .await
            .context("initial owner snapshot failed")?;
        let delayed = result(fixture.read("local-delayed")?)?;
        ensure!(
            delayed["revision"] == delayed["cleared"],
            "fixture did not exercise unchanged native ownership"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let late_formats = loop {
            let offer = fixture.changed(Some(initial.revision)).await?;
            if offer.formats.iter().any(|f| f == "text/plain") {
                break offer;
            }
            ensure!(
                Instant::now() < deadline,
                "late native formats were never observed"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        ensure!(
            owner
                .read(late_formats.revision, "text/plain".into())
                .await
                .context("late formats read failed")?
                .bytes()
                == Some(TEXT.as_bytes())
        );
        println!(
            "PASS: native types added after clear are observed without an ownership-counter change"
        );
        let snapshot = fixture.copy("local-rich").await?;
        let (text, png) = tokio::join!(
            owner.read(snapshot.revision, "text/plain".into()),
            owner.read(snapshot.revision, "image/png".into())
        );
        let text = text.context("owner text read failed")?;
        let png = png.context("owner PNG read failed")?;
        ensure!(text.bytes() == Some(TEXT.as_bytes()) && png.bytes() == Some(PNG));
        let current = fixture.copy("local-empty").await?;
        ensure!(!snapshot.is_current() && text.bytes().is_none() && png.bytes().is_none());
        drop((text, png));
        ensure!(
            owner
                .read(snapshot.revision, "text/plain".into())
                .await
                .is_none()
        );
        let mut held = Vec::new();
        for _ in 0..8 {
            held.push(
                owner
                    .read(current.revision, "text/plain".into())
                    .await
                    .context("empty read slot unavailable")?,
            );
        }
        ensure!(held.iter().all(|data| data.bytes() == Some(&[][..])));
        ensure!(
            owner
                .read(current.revision, "text/plain".into())
                .await
                .is_none(),
            "held results bypassed slot limit"
        );
        held.pop();
        ensure!(
            owner
                .read(current.revision, "text/plain".into())
                .await
                .is_some()
        );
        drop(held);
        let current = fixture.copy("local-bulk").await?;
        let bulk = owner
            .read(current.revision, "text/plain".into())
            .await
            .context("bulk read failed")?;
        ensure!(bulk.bytes().unwrap().len() == 24 * 1024 * 1024);
        ensure!(
            owner
                .read(current.revision, "text/plain".into())
                .await
                .is_none(),
            "held results bypassed byte budget"
        );
        drop(bulk);
        ensure!(
            owner
                .read(current.revision, "text/plain".into())
                .await
                .is_some()
        );
        println!(
            "PASS: background owner multiplexes local reads and fences stale/held data with aggregate byte and slot limits"
        );

        let current = fixture.copy("local-rich").await?;
        let held = owner
            .read(current.revision, "text/plain".into())
            .await
            .context("pre-crash read failed")?;
        let native = result(fixture.read("revision")?)?["revision"]
            .as_i64()
            .unwrap();
        let pid: i32 = std::fs::read_to_string(fixture.directory.join("pid"))?
            .trim()
            .parse()?;
        ensure!(unsafe { libc::kill(pid, libc::SIGKILL) } == 0);
        let deadline = Instant::now() + Duration::from_secs(3);
        while current.is_current() {
            ensure!(Instant::now() < deadline, "owner did not fence helper EOF");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        ensure!(held.bytes().is_none());
        let next = fixture.changed(Some(current.revision)).await?;
        ensure!(next.revision > current.revision);
        ensure!(
            result(fixture.read("revision")?)?["revision"].as_i64() == Some(native),
            "restart changed the local native clipboard"
        );
        ensure!(
            owner
                .read(current.revision, "text/plain".into())
                .await
                .is_none()
        );
        ensure!(
            owner
                .read(next.revision, "text/plain".into())
                .await
                .context("restart read failed")?
                .bytes()
                == Some(TEXT.as_bytes())
        );
        drop(held);
        println!(
            "PASS: helper crash fences delivered data; restart preserves native data but never reuses logical revisions"
        );

        let source = Arc::new(SourceFixture {
            current: AtomicBool::new(true),
            requests: AtomicUsize::new(0),
        });
        let publication = owner
            .publish(
                next.revision,
                vec!["text/plain".into(), "image/png".into()],
                source.clone(),
            )
            .await
            .context("owner publication failed")?;
        let imported = fixture.changed(Some(next.revision)).await?;
        ensure!(imported.origin == Some(publication.generation));
        ensure!(
            source.requests.load(Ordering::Acquire) == 0,
            "metadata eagerly fetched remote bytes"
        );
        ensure!(
            owner
                .read(imported.revision, "text/plain".into())
                .await
                .is_none(),
            "import was echoed through local reads"
        );
        let consumer = fixture.read("public.utf8-plain-text")?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while source.requests.load(Ordering::Acquire) == 0 {
            ensure!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let snapshot_start = Instant::now();
        ensure!(owner.snapshot().await.is_some());
        ensure!(
            snapshot_start.elapsed() < Duration::from_millis(200),
            "metadata waited for remote payload"
        );
        ensure!(result(consumer)?["text"] == true);
        ensure!(result(fixture.read("public.png")?)?["png"] == true);
        ensure!(source.requests.load(Ordering::Acquire) == 2);
        let pid: i32 = std::fs::read_to_string(fixture.directory.join("pid"))?
            .trim()
            .parse()?;
        ensure!(unsafe { libc::kill(pid, libc::SIGKILL) } == 0);
        let deadline = Instant::now() + Duration::from_secs(3);
        while publication.is_current() {
            ensure!(
                Instant::now() < deadline,
                "crashed publication retained authority"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let recovered = fixture.changed(Some(imported.revision)).await?;
        ensure!(
            recovered.origin == Some(publication.generation),
            "restart forgot imported native provenance"
        );
        ensure!(
            owner
                .read(recovered.revision, "text/plain".into())
                .await
                .is_none(),
            "restart echoed the dead helper's native cache"
        );
        let replacement = owner
            .publish(
                recovered.revision,
                vec!["text/plain".into()],
                source.clone(),
            )
            .await
            .context("replacement after crash failed")?;
        ensure!(
            replacement.generation > publication.generation,
            "restart reused publication generation"
        );
        drop(publication);
        let imported = fixture.changed(Some(recovered.revision)).await?;
        ensure!(imported.origin == Some(replacement.generation) && replacement.is_current());
        drop(replacement);
        println!(
            "PASS: imported native provenance survives helper crash; old guards cannot revoke a replacement generation"
        );
        let cleared = fixture.changed(Some(imported.revision)).await?;
        ensure!(
            cleared.origin == imported.origin && cleared.formats.is_empty(),
            "owned withdrawal lost imported provenance and could echo a local clear"
        );
        println!(
            "PASS: one owner publishes lazy remote text/PNG, pumps metadata during demand, prevents echo and withdraws on guard drop"
        );

        let replaced = owner
            .publish(cleared.revision, vec!["text/plain".into()], source.clone())
            .await
            .context("local replacement publication failed")?;
        let local = fixture.copy("local-rich").await?;
        ensure!(local.origin.is_none());
        ensure!(
            owner
                .read(local.revision, "text/plain".into())
                .await
                .context("new local copy was suppressed")?
                .bytes()
                == Some(TEXT.as_bytes())
        );
        drop(replaced);
        // Sending a new publication waits behind any old revocation. The
        // native ownership check must preserve the intervening local copy.
        let publication = owner
            .publish(local.revision, vec!["text/plain".into()], source.clone())
            .await
            .context("second publication failed")?;
        let imported = fixture.changed(Some(local.revision)).await?;
        let consumer = fixture.read("public.utf8-plain-text")?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while source.requests.load(Ordering::Acquire) < 3 {
            ensure!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        source.current.store(false, Ordering::Release);
        ensure!(result(consumer)?["length"].is_null());
        let cleared = fixture.changed(Some(imported.revision)).await?;
        ensure!(cleared.formats.is_empty());
        drop(publication);
        let local = fixture.copy("local-rich").await?;
        let data = owner
            .read(local.revision, "text/plain".into())
            .await
            .context("final local read failed")?;
        owner.close();
        ensure!(
            !local.is_current() && data.bytes().is_none(),
            "close did not immediately fence delivered data"
        );
        tokio::time::timeout(Duration::from_secs(3), owner.shutdown()).await?;
        ensure!(owner.snapshot().await.is_none());
        ensure!(
            result(fixture.read("public.utf8-plain-text")?)?["text"] == true,
            "shutdown cleared newer local data"
        );
        println!(
            "PASS: source revocation rejects late native replies; explicit shutdown fences results, reaps the helper and preserves local copies"
        );
        Ok(())
    }
    pub fn run() -> Result<()> {
        let args: Vec<_> = std::env::args().collect();
        if args.len() == 2 && args[1] == helper::ARGUMENT {
            return helper::run_from_stdio().map_err(anyhow::Error::msg);
        }
        if args.len() == 4 && args[1] == "consumer" {
            return consumer(&args[2], &args[3]);
        }
        if args.len() != 1 {
            bail!("unknown mode");
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(checks())
    }
}
#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    check::run()
}
#[cfg(not(target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("Requires macOS")
}
