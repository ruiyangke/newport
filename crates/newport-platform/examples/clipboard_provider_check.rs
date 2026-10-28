//! Real cross-process AppKit checks. Only disposable named pasteboards are
//! used; this never opens the general clipboard or starts Newport's UI/backend.
#[cfg(target_os = "macos")]
mod check {
    use anyhow::{Context, Result, bail, ensure};
    use newport_platform::clipboard::provider::{Payload, Publication, Reply, Source};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSPasteboard};
    use objc2_foundation::{NSData, NSDate, NSDefaultRunLoopMode, NSRunLoop, NSString};
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    const PREFIX: &str = "org.newport.test.lazy.";
    const TEXT: &str = "Newport 世界\n";
    const LOCAL: &str = "Newer local copy";
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    fn emit(value: Value) {
        println!("{value}");
        std::io::stdout().flush().unwrap();
    }
    fn board(name: &str) -> objc2::rc::Retained<NSPasteboard> {
        assert!(name.starts_with(PREFIX), "test pasteboards only");
        NSPasteboard::pasteboardWithName(&NSString::from_str(name))
    }

    struct Data {
        bytes: Vec<u8>,
        invalidate: bool,
        reads: AtomicUsize,
    }
    impl Payload for Data {
        fn bytes(&self) -> Option<&[u8]> {
            (!self.invalidate || self.reads.fetch_add(1, Ordering::Relaxed) == 0)
                .then_some(self.bytes.as_slice())
        }
    }
    struct Remote {
        mode: String,
        current: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
    }
    impl Source for Remote {
        fn is_current(&self) -> bool {
            self.current.load(Ordering::Acquire)
        }
        fn request(&self, format: &str, reply: Reply) {
            emit(json!({"event":"request", "format":format}));
            if self.mode == "blocking-local" {
                // Deliberately uncooperative foreign clipboard fixture. Unlike
                // a product Source, it blocks its native callback until released
                // so we can prove the consuming helper's main loop stays live.
                while !self.release.load(Ordering::Acquire) && self.current.load(Ordering::Acquire)
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                reply.complete(Box::new(Data {
                    bytes: TEXT.as_bytes().to_vec(),
                    invalidate: false,
                    reads: AtomicUsize::new(0),
                }));
                emit(json!({"event":"completed", "cancelled":false}));
                return;
            }
            let mode = self.mode.clone();
            let format = format.to_owned();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(if mode == "normal" {
                    100
                } else {
                    400
                }));
                let bytes = if mode == "invalid" {
                    vec![0xff]
                } else if mode == "budget" {
                    vec![b'a'; 17 * 1024 * 1024]
                } else if mode == "oversized" {
                    vec![b'a'; 33 * 1024 * 1024]
                } else if format == "image/png" {
                    PNG.to_vec()
                } else {
                    TEXT.as_bytes().to_vec()
                };
                let cancelled = reply.is_cancelled();
                reply.complete(Box::new(Data {
                    bytes,
                    invalidate: mode == "fence",
                    reads: AtomicUsize::new(0),
                }));
                emit(json!({"event":"completed", "cancelled":cancelled}));
            });
        }
    }

    fn owner(name: &str, mode: &str) -> Result<()> {
        let main = MainThreadMarker::new().context("requires helper main thread")?;
        NSApplication::sharedApplication(main)
            .setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        let board = board(name);
        board.clearContents();
        ensure!(board.setString_forType(
            &NSString::from_str(LOCAL),
            &NSString::from_str("public.utf8-plain-text")
        ));
        let expected = board.changeCount();
        let current = Arc::new(AtomicBool::new(true));
        let release = Arc::new(AtomicBool::new(false));
        let source = Arc::new(Remote {
            mode: mode.to_owned(),
            current: current.clone(),
            release: release.clone(),
        });
        let formats = if mode == "unsupported" {
            vec!["application/x-no-native-uti".into()]
        } else {
            vec!["text/plain".into(), "image/png".into(), "text/html".into()]
        };
        if mode == "stale" {
            board.clearContents();
            board.setString_forType(
                &NSString::from_str(LOCAL),
                &NSString::from_str("public.utf8-plain-text"),
            );
        }
        let publication = Publication::publish(
            main,
            board.clone(),
            expected,
            &formats,
            source,
            Duration::from_millis(if mode == "timeout" { 80 } else { 1500 }),
        );
        if matches!(mode, "unsupported" | "stale") {
            ensure!(publication.is_err());
            ensure!(
                board
                    .stringForType(&NSString::from_str("public.utf8-plain-text"))
                    .unwrap()
                    .to_string()
                    == LOCAL
            );
            board.clearContents();
            emit(json!({"event":"rejected"}));
            return Ok(());
        }
        let publication = publication.map_err(anyhow::Error::msg)?;
        let stop = Arc::new(AtomicBool::new(false));
        let withdraw = Arc::new(AtomicBool::new(false));
        let (control_stop, control_withdraw) = (stop.clone(), withdraw.clone());
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                match line.as_deref() {
                    Ok("release") => {
                        release.store(true, Ordering::Release);
                    }
                    Ok("revoke") => {
                        current.store(false, Ordering::Release);
                        emit(json!({"event":"revoked"}));
                    }
                    Ok("withdraw") => {
                        current.store(false, Ordering::Release);
                        control_withdraw.store(true, Ordering::Release);
                    }
                    _ => break,
                }
            }
            current.store(false, Ordering::Release);
            control_stop.store(true, Ordering::Release);
        });
        emit(json!({"event":"ready", "revision":publication.revision()}));
        let deadline = Instant::now() + Duration::from_secs(15);
        while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
            objc2::rc::autoreleasepool(|_| {
                NSRunLoop::currentRunLoop().runMode_beforeDate(
                    unsafe { NSDefaultRunLoopMode },
                    &NSDate::dateWithTimeIntervalSinceNow(0.01),
                );
                if withdraw.swap(false, Ordering::AcqRel) {
                    emit(json!({"event":"withdrawn", "owned":publication.withdraw()}));
                }
            });
        }
        publication.withdraw();
        Ok(())
    }

    fn consumer(name: &str, operation: &str) -> Result<()> {
        let board = board(name);
        match operation {
            "types" => emit(
                json!({"types":board.types().map(|types| types.iter().map(|kind| kind.to_string()).collect::<Vec<_>>()).unwrap_or_default()}),
            ),
            "local" => {
                board.clearContents();
                ensure!(board.setString_forType(
                    &NSString::from_str(LOCAL),
                    &NSString::from_str("public.utf8-plain-text")
                ));
                emit(json!({"local":true}));
            }
            "local-rich" | "local-large" | "local-bulk" | "local-empty" | "local-oversized" => {
                board.clearContents();
                let bytes = match operation {
                    "local-large" => vec![b'a'; 2 * 1024 * 1024],
                    "local-bulk" => vec![b'a'; 24 * 1024 * 1024],
                    "local-oversized" => vec![b'a'; 33 * 1024 * 1024],
                    "local-empty" => Vec::new(),
                    _ => TEXT.as_bytes().to_vec(),
                };
                ensure!(board.setData_forType(
                    Some(&NSData::with_bytes(&bytes)),
                    &NSString::from_str("public.utf8-plain-text")
                ));
                if operation == "local-rich" {
                    ensure!(board.setData_forType(
                        Some(&NSData::with_bytes(PNG)),
                        &NSString::from_str("public.png")
                    ));
                }
                emit(json!({"revision":board.changeCount()}));
            }
            kind => {
                let bytes = board
                    .dataForType(&NSString::from_str(kind))
                    .map(|data| data.to_vec());
                emit(
                    json!({"length":bytes.as_ref().map(Vec::len), "text":bytes.as_deref() == Some(TEXT.as_bytes()), "png":bytes.as_deref() == Some(PNG), "local":bytes.as_deref() == Some(LOCAL.as_bytes()), "allA":bytes.as_ref().is_some_and(|bytes| bytes.iter().all(|b| *b == b'a'))}),
                );
            }
        }
        Ok(())
    }

    struct Helper {
        child: Child,
        input: Option<ChildStdin>,
        events: mpsc::Receiver<Value>,
        name: String,
    }
    impl Drop for Helper {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let board = board(&self.name);
            board.clearContents();
            // Declared in AppKit's NSPasteboard.h but omitted by objc2's
            // generated bindings. The argument-free method returns void and
            // releases only this test's unique named pasteboard server object.
            let _: () = unsafe { objc2::msg_send![&*board, releaseGlobally] };
        }
    }
    impl Helper {
        fn start(mode: &str) -> Result<Self> {
            let name = format!("{PREFIX}{}", uuid::Uuid::new_v4());
            let mut child = Command::new(std::env::current_exe()?)
                .args(["owner", &name, mode])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()?;
            let input = child.stdin.take().unwrap();
            let output = child.stdout.take().unwrap();
            let (sender, events) = mpsc::channel();
            std::thread::spawn(move || {
                for line in BufReader::new(output).lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    let Ok(event) = serde_json::from_str(&line) else {
                        break;
                    };
                    if sender.send(event).is_err() {
                        break;
                    }
                }
            });
            Ok(Self {
                child,
                input: Some(input),
                events,
                name,
            })
        }
        fn event(&self, expected: &str) -> Result<Value> {
            let event = self.events.recv_timeout(Duration::from_secs(4))?;
            ensure!(
                event["event"] == expected,
                "Expected {expected}, got {event}"
            );
            Ok(event)
        }
        fn command(&mut self, command: &str) -> Result<()> {
            let input = self.input.as_mut().context("helper input closed")?;
            writeln!(input, "{command}")?;
            input.flush()?;
            Ok(())
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
        fn stop(mut self) -> Result<()> {
            self.command("stop")?;
            ensure!(self.child.wait()?.success());
            Ok(())
        }
    }
    // Poll a real consumer with a deadline. This also represents the parent's
    // UI heartbeat while its helper's AppKit callback waits for remote bytes.
    struct Consumer(Child);
    impl Drop for Consumer {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn result(mut consumer: Consumer) -> Result<Value> {
        let child = &mut consumer.0;
        let start = Instant::now();
        let mut ticks = 0;
        while child.try_wait()?.is_none() {
            if start.elapsed() > Duration::from_secs(4) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("native consumer timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
            ticks += 1;
        }
        ensure!(child.wait()?.success());
        let mut output = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut output)?;
        if start.elapsed() > Duration::from_millis(100) {
            ensure!(ticks >= 5);
        }
        Ok(serde_json::from_slice(&output)?)
    }

    pub fn run() -> Result<()> {
        let args: Vec<_> = std::env::args().collect();
        if args.len() == 2 && args[1] == newport_platform::clipboard::helper::ARGUMENT {
            return newport_platform::clipboard::helper::run_from_stdio()
                .map_err(anyhow::Error::msg);
        }
        if args.len() == 4 {
            return match args[1].as_str() {
                "owner" => owner(&args[2], &args[3]),
                "consumer" => consumer(&args[2], &args[3]),
                _ => bail!("unknown mode"),
            };
        }
        if args.len() == 2 && args[1] == "local-ipc" {
            return local_ipc_checks();
        }
        ensure!(args.len() == 1);
        let h = Helper::start("normal")?;
        h.event("ready")?;
        let types = result(h.read("types")?)?;
        for kind in ["public.utf8-plain-text", "public.png", "public.html"] {
            ensure!(types["types"].as_array().unwrap().contains(&json!(kind)));
        }
        ensure!(h.events.try_recv().is_err(), "metadata fetched payloads");
        ensure!(result(h.read("public.utf8-plain-text")?)?["text"] == true);
        ensure!(h.event("request")?["format"] == "text/plain");
        h.event("completed")?;
        ensure!(result(h.read("public.utf8-plain-text")?)?["text"] == true);
        ensure!(
            h.events.try_recv().is_err(),
            "native cache fetched text twice"
        );
        ensure!(result(h.read("public.png")?)?["png"] == true);
        ensure!(h.event("request")?["format"] == "image/png");
        h.event("completed")?;
        h.stop()?;
        println!(
            "PASS: metadata-only offers; Unicode/PNG fetched separately; native cache avoids repeat fetch"
        );

        for mode in [
            "revoke",
            "replace",
            "timeout",
            "invalid",
            "oversized",
            "fence",
        ] {
            let mut h = Helper::start(mode)?;
            h.event("ready")?;
            let reader = h.read("public.utf8-plain-text")?;
            h.event("request")?;
            if mode == "revoke" {
                h.command("revoke")?;
                h.event("revoked")?;
            }
            if mode == "replace" {
                ensure!(result(h.read("local")?)?["local"] == true);
            }
            let value = result(reader)?;
            ensure!(
                value["length"].is_null(),
                "{mode}: stale/invalid data returned: {value}"
            );
            let completed = h.event("completed")?;
            if matches!(mode, "revoke" | "replace" | "timeout") {
                ensure!(completed["cancelled"] == true);
            }
            if mode == "replace" {
                h.command("withdraw")?;
                ensure!(h.event("withdrawn")?["owned"] == false);
                ensure!(result(h.read("public.utf8-plain-text")?)?["local"] == true);
            }
            h.stop()?;
            println!("PASS: {mode} rejects unsafe data and late completion");
        }

        let mut h = Helper::start("budget")?;
        h.event("ready")?;
        ensure!(result(h.read("public.utf8-plain-text")?)?["length"] == 17 * 1024 * 1024);
        h.event("request")?;
        h.event("completed")?;
        ensure!(result(h.read("public.png")?)?["length"].is_null());
        h.event("request")?;
        h.event("completed")?;
        h.command("withdraw")?;
        ensure!(h.event("withdrawn")?["owned"] == true);
        ensure!(
            result(h.read("types")?)?["types"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        h.stop()?;
        println!("PASS: aggregate native byte budget; explicit withdrawal removes cached formats");

        let mut h = Helper::start("disconnect")?;
        h.event("ready")?;
        let reader = h.read("public.utf8-plain-text")?;
        h.event("request")?;
        drop(h.input.take());
        ensure!(result(reader)?["length"].is_null());
        ensure!(h.child.wait()?.success());
        ensure!(
            result(h.read("types")?)?["types"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        println!("PASS: parent EOF cancels a pending callback and withdraws its native offer");

        for mode in ["stale", "unsupported"] {
            let mut h = Helper::start(mode)?;
            h.event("rejected")?;
            ensure!(h.child.wait()?.success());
            println!("PASS: {mode} publication preserves existing clipboard");
        }
        ipc_checks()?;
        local_ipc_checks()?;
        Ok(())
    }

    use newport_platform::clipboard::helper::{self, Board, Client, Command as Control, Event};
    struct Endpoint {
        client: Option<Client>,
        name: String,
        pending: std::collections::VecDeque<Event>,
    }
    impl Drop for Endpoint {
        fn drop(&mut self) {
            drop(self.client.take());
            let board = board(&self.name);
            board.clearContents();
            let _: () = unsafe { objc2::msg_send![&*board, releaseGlobally] };
        }
    }
    impl Endpoint {
        fn start() -> Result<(Self, isize)> {
            let name = format!("{PREFIX}{}", uuid::Uuid::new_v4());
            let client = Client::spawn(
                &std::env::current_exe()?,
                Board::Test { name: name.clone() },
            )?;
            let mut this = Self {
                client: Some(client),
                name,
                pending: Default::default(),
            };
            let Event::Snapshot {
                revision,
                origin: None,
                ..
            } = this.event(|e| matches!(e, Event::Snapshot { .. }))?
            else {
                bail!("bad initial origin")
            };
            Ok((this, revision))
        }
        fn sender(&self) -> helper::Sender {
            self.client.as_ref().unwrap().sender()
        }
        fn event(&mut self, matches: impl Fn(&Event) -> bool) -> Result<Event> {
            if let Some(index) = self.pending.iter().position(&matches) {
                return Ok(self.pending.remove(index).unwrap());
            }
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                ensure!(
                    Instant::now() < deadline,
                    "helper event timed out: {:?}",
                    self.pending
                );
                if let Some(event) = self
                    .client
                    .as_mut()
                    .unwrap()
                    .receive(Duration::from_millis(100))?
                {
                    if matches(&event) {
                        return Ok(event);
                    }
                    if !matches!(event, Event::Snapshot { .. }) {
                        self.pending.push_back(event);
                    }
                    ensure!(self.pending.len() < 64);
                }
            }
        }
        fn publish(&mut self, generation: u64, expected: isize) -> Result<isize> {
            self.sender().send(
                &Control::Publish {
                    generation,
                    expected_revision: expected,
                    formats: vec!["text/plain".into(), "image/png".into()],
                },
                &[],
            )?;
            let Event::Published {
                generation: actual,
                revision,
            } = self.event(|e| matches!(e, Event::Published { .. } | Event::Rejected { .. }))?
            else {
                bail!("publication rejected")
            };
            ensure!(actual == generation);
            let Event::Snapshot {
                revision: native,
                origin: Some(origin),
                ..
            } = self.event(|e| matches!(e, Event::Snapshot { .. }))?
            else {
                bail!("missing import origin")
            };
            ensure!(native == revision && origin == generation);
            Ok(revision)
        }
        fn read(&self, kind: &str) -> Result<Consumer> {
            Ok(Consumer(
                Command::new(std::env::current_exe()?)
                    .args(["consumer", &self.name, kind])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()?,
            ))
        }
        fn request(&mut self, generation: u64) -> Result<u64> {
            let Event::Read {
                generation: actual,
                id,
                ..
            } = self.event(|e| matches!(e, Event::Read { .. }))?
            else {
                unreachable!()
            };
            ensure!(actual == generation);
            Ok(id)
        }
        fn data(&self, generation: u64, id: u64, bytes: &[u8]) -> Result<()> {
            let sender = self.sender();
            for (index, chunk) in bytes.chunks(helper::CHUNK).enumerate() {
                let offset = index * helper::CHUNK;
                sender.send(
                    &Control::Data {
                        generation,
                        id,
                        offset,
                        eof: offset + chunk.len() == bytes.len(),
                    },
                    chunk,
                )?;
            }
            if bytes.is_empty() {
                sender.send(
                    &Control::Data {
                        generation,
                        id,
                        offset: 0,
                        eof: true,
                    },
                    &[],
                )?;
            }
            Ok(())
        }
        fn write_local(&mut self, operation: &str) -> Result<isize> {
            let revision = result(self.read(operation)?)?["revision"].as_i64().unwrap() as isize;
            self.event(|e| {
                matches!(e, Event::Snapshot { revision: value, origin: None, formats }
                if *value == revision && formats.iter().any(|f| f == "text/plain")
                && (operation != "local-rich" || formats.iter().any(|f| f == "image/png")))
            })?;
            Ok(revision)
        }
        fn local_request(&self, id: u64, revision: isize, format: &str) -> Result<()> {
            self.sender().send(
                &Control::ReadLocal {
                    id,
                    revision,
                    format: format.into(),
                },
                &[],
            )?;
            Ok(())
        }
        fn local_result(&mut self, id: u64, revision: isize) -> Result<Option<Vec<u8>>> {
            if let Some(index) = self
                .pending
                .iter()
                .position(|e| matches!(e, Event::LocalFailed { id: value } if *value == id))
            {
                self.pending.remove(index);
                return Ok(None);
            }
            let mut bytes = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                ensure!(
                    Instant::now() < deadline,
                    "local clipboard result timed out"
                );
                let Some(packet) = self
                    .client
                    .as_mut()
                    .unwrap()
                    .receive_packet(Duration::from_millis(100))
                    .with_context(|| format!("local clipboard read {id} failed"))?
                else {
                    continue;
                };
                match packet.event {
                    Event::LocalFailed { id: actual } if actual == id => return Ok(None),
                    Event::LocalFailed { id: actual } => {
                        self.pending.push_back(Event::LocalFailed { id: actual });
                    }
                    Event::LocalData {
                        id: actual,
                        revision: source,
                        offset,
                        eof,
                    } => {
                        ensure!(actual == id && source == revision && offset == bytes.len());
                        bytes.extend_from_slice(&packet.bytes);
                        ensure!(bytes.len() <= newport_platform::clipboard::provider::MAX_BYTES);
                        if eof {
                            return Ok(Some(bytes));
                        }
                    }
                    Event::Snapshot { .. } => {}
                    other => bail!("Unexpected event during local read: {other:?}"),
                }
            }
        }
    }

    fn ipc_checks() -> Result<()> {
        // This is the same early branch used by the shipping desktop. It must
        // reject ordinary stdin before opening a native board or profile.
        let rejected = Command::new(std::env::current_exe()?)
            .arg(helper::ARGUMENT)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        ensure!(!rejected.success());
        ensure!(
            Client::spawn(
                &std::env::current_exe()?,
                Board::Test {
                    name: "not-a-disposable-board".into()
                }
            )
            .is_err()
        );
        let (mut endpoint, initial) = Endpoint::start()?;
        let mut revision = endpoint.publish(1, initial)?;
        ensure!(
            result(endpoint.read("types")?)?["types"]
                .as_array()
                .unwrap()
                .contains(&json!("public.png"))
        );
        ensure!(
            endpoint
                .client
                .as_mut()
                .unwrap()
                .receive(Duration::from_millis(30))?
                .is_none(),
            "metadata triggered payload demand"
        );
        let reader = endpoint.read("public.utf8-plain-text")?;
        let id = endpoint.request(1)?;
        endpoint.data(1, id, TEXT.as_bytes())?;
        ensure!(result(reader)?["text"] == true);
        let reader = endpoint.read("public.png")?;
        let id = endpoint.request(1)?;
        endpoint.data(1, id, PNG)?;
        ensure!(result(reader)?["png"] == true);
        revision = endpoint.publish(2, revision)?;
        let reader = endpoint.read("public.utf8-plain-text")?;
        let id = endpoint.request(2)?;
        endpoint.data(2, id, &vec![b'a'; 2 * 1024 * 1024])?;
        let value = result(reader)?;
        ensure!(value["length"] == 2 * 1024 * 1024 && value["allA"] == true);
        println!(
            "PASS: real helper IPC publishes lazy Unicode/PNG and assembles 2 MiB of chunks with native origin metadata"
        );

        endpoint.publish(3, revision)?;
        let reader = endpoint.read("public.utf8-plain-text")?;
        let stale = endpoint.request(3)?;
        endpoint.sender().send(
            &Control::Data {
                generation: 3,
                id: stale,
                offset: 0,
                eof: false,
            },
            b"partial",
        )?;
        endpoint
            .sender()
            .send(&Control::Revoke { generation: 3 }, &[])?;
        endpoint.event(|e| matches!(e, Event::Cancelled { id, .. } if *id == stale))?;
        let Event::Revoked {
            revision: Some(revision),
            ..
        } = endpoint.event(|e| matches!(e, Event::Revoked { generation: 3, .. }))?
        else {
            bail!("missing revoke revision")
        };
        ensure!(result(reader)?["length"].is_null());
        endpoint.publish(4, revision)?;
        let reader = endpoint.read("public.utf8-plain-text")?;
        let fresh = endpoint.request(4)?;
        ensure!(fresh > stale);
        endpoint.sender().send(
            &Control::Data {
                generation: 3,
                id: stale,
                offset: 7,
                eof: true,
            },
            b"stale",
        )?;
        endpoint.data(4, fresh, TEXT.as_bytes())?;
        ensure!(result(reader)?["text"] == true);
        endpoint
            .sender()
            .send(&Control::Revoke { generation: 3 }, &[])?;
        ensure!(matches!(
            endpoint.event(|e| matches!(e, Event::Revoked { generation: 3, .. }))?,
            Event::Revoked { revision: None, .. }
        ));
        ensure!(result(endpoint.read("public.utf8-plain-text")?)?["text"] == true);
        println!(
            "PASS: control reader revokes a blocked callback; late chunks cannot populate a replacement offer"
        );
        drop(endpoint);

        let (mut endpoint, initial) = Endpoint::start()?;
        endpoint.publish(1, initial)?;
        let reader = endpoint.read("public.utf8-plain-text")?;
        let id = endpoint.request(1)?;
        ensure!(result(endpoint.read("local")?)?["local"] == true);
        endpoint.event(|e| matches!(e, Event::Cancelled { .. }))?;
        endpoint.data(1, id, b"stale")?;
        ensure!(result(reader)?["length"].is_null());
        drop(endpoint.client.take());
        ensure!(result(endpoint.read("public.utf8-plain-text")?)?["local"] == true);
        println!("PASS: local copy cancels IPC demand and survives helper shutdown");

        let (mut endpoint, initial) = Endpoint::start()?;
        endpoint.publish(1, initial)?;
        let reader = endpoint.read("public.utf8-plain-text")?;
        endpoint.request(1)?;
        let sender = endpoint.sender();
        let pid = endpoint.client.as_ref().unwrap().process_id();
        drop(endpoint.client.take());
        ensure!(result(reader)?["length"].is_null());
        ensure!(
            result(endpoint.read("types")?)?["types"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        ensure!(sender.send(&Control::Stop {}, &[]).is_err());
        ensure!(unsafe { libc::kill(pid as i32, 0) } == -1);
        println!(
            "PASS: parent disconnect withdraws a pending offer, invalidates cloned senders and reaps the helper"
        );

        for malformed in [false, true] {
            let (mut endpoint, initial) = Endpoint::start()?;
            let revision = endpoint.publish(1, initial)?;
            let reader = endpoint.read("public.utf8-plain-text")?;
            let id = endpoint.request(1)?;
            if malformed {
                endpoint.sender().send(
                    &Control::Data {
                        generation: 1,
                        id,
                        offset: 1,
                        eof: true,
                    },
                    b"bad",
                )?;
            } else {
                endpoint.sender().send(
                    &Control::Publish {
                        generation: 1,
                        expected_revision: revision,
                        formats: vec!["text/plain".into()],
                    },
                    &[],
                )?;
            }
            ensure!(
                endpoint
                    .client
                    .as_mut()
                    .unwrap()
                    .receive(Duration::from_secs(3))
                    .is_err()
            );
            ensure!(result(reader)?["length"].is_null());
            drop(endpoint.client.take());
            ensure!(
                result(endpoint.read("types")?)?["types"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
        println!(
            "PASS: reused generations and malformed chunk offsets close the connection and withdraw owned promises"
        );

        let (mut endpoint, initial) = Endpoint::start()?;
        endpoint.publish(1, initial)?;
        let reader = endpoint.read("public.utf8-plain-text")?;
        let id = endpoint.request(1)?;
        let bytes = vec![b'a'; helper::CHUNK];
        for offset in (0..=32 * 1024 * 1024).step_by(helper::CHUNK) {
            if endpoint
                .sender()
                .send(
                    &Control::Data {
                        generation: 1,
                        id,
                        offset,
                        eof: false,
                    },
                    &bytes,
                )
                .is_err()
            {
                break;
            }
        }
        ensure!(
            endpoint
                .client
                .as_mut()
                .unwrap()
                .receive(Duration::from_secs(3))
                .is_err()
        );
        ensure!(result(reader)?["length"].is_null());
        drop(endpoint.client.take());
        println!(
            "PASS: oversized chunk assembly closes the connection before publishing any bytes"
        );

        let (mut endpoint, initial) = Endpoint::start()?;
        endpoint.publish(1, initial)?;
        let reader = endpoint.read("public.utf8-plain-text")?;
        let old_id = endpoint.request(1)?;
        let old_sender = endpoint.sender();
        let pid = endpoint.client.as_ref().unwrap().process_id();
        // Kill only the fixture child created immediately above.
        ensure!(unsafe { libc::kill(pid as i32, libc::SIGKILL) } == 0);
        ensure!(
            endpoint
                .client
                .as_mut()
                .unwrap()
                .receive(Duration::from_secs(3))
                .is_err()
        );
        ensure!(result(reader)?["length"].is_null());
        drop(endpoint.client.take());
        endpoint.client = Some(Client::spawn(
            &std::env::current_exe()?,
            Board::Test {
                name: endpoint.name.clone(),
            },
        )?);
        let Event::Snapshot {
            revision,
            origin: None,
            ..
        } = endpoint.event(|e| matches!(e, Event::Snapshot { .. }))?
        else {
            bail!("restart inherited old authority")
        };
        endpoint.publish(1, revision)?;
        let reader = endpoint.read("public.utf8-plain-text")?;
        let id = endpoint.request(1)?;
        ensure!(
            id == old_id,
            "fixture should exercise reused IDs on a fresh connection"
        );
        ensure!(
            old_sender
                .send(
                    &Control::Data {
                        generation: 1,
                        id,
                        offset: 0,
                        eof: true
                    },
                    b"stale"
                )
                .is_err()
        );
        endpoint.data(1, id, TEXT.as_bytes())?;
        ensure!(result(reader)?["text"] == true);
        println!(
            "PASS: crashed helper is reaped; fresh connection reuses IDs without accepting the old sender"
        );
        drop(endpoint);
        let (mut endpoint, _) = Endpoint::start()?;
        let pid = endpoint.client.as_ref().unwrap().process_id();
        // Stop only the owned fixture child to exercise the supervisor's
        // bounded fallback without risking any unrelated desktop process.
        ensure!(unsafe { libc::kill(pid as i32, libc::SIGSTOP) } == 0);
        let start = Instant::now();
        drop(endpoint.client.take());
        ensure!(start.elapsed() < Duration::from_secs(3));
        ensure!(unsafe { libc::kill(pid as i32, 0) } == -1);
        println!(
            "PASS: an unresponsive owned helper is killed and reaped within the shutdown bound"
        );
        Ok(())
    }

    fn local_ipc_checks() -> Result<()> {
        let (mut endpoint, _) = Endpoint::start()?;
        let revision = endpoint.write_local("local-rich")?;
        endpoint.local_request(1, revision, "text/plain")?;
        ensure!(endpoint.local_result(1, revision)?.unwrap() == TEXT.as_bytes());
        endpoint.local_request(2, revision, "image/png")?;
        ensure!(endpoint.local_result(2, revision)?.unwrap() == PNG);
        endpoint.local_request(3, revision, "not-advertised")?;
        ensure!(endpoint.local_result(3, revision)?.is_none());
        let new_revision = endpoint.write_local("local-large")?;
        endpoint.local_request(4, revision, "text/plain")?;
        ensure!(endpoint.local_result(4, revision)?.is_none());
        endpoint.local_request(5, new_revision, "text/plain")?;
        let data = endpoint.local_result(5, new_revision)?.unwrap();
        ensure!(data.len() == 2 * 1024 * 1024 && data.iter().all(|b| *b == b'a'));
        let revision = endpoint.write_local("local-empty")?;
        endpoint.local_request(6, revision, "text/plain")?;
        ensure!(endpoint.local_result(6, revision)? == Some(Vec::new()));
        let revision = endpoint.write_local("local-oversized")?;
        endpoint.local_request(7, revision, "text/plain")?;
        ensure!(endpoint.local_result(7, revision)?.is_none());
        println!(
            "PASS: local reads preserve Unicode/PNG/empty data, chunk large payloads and reject stale/unadvertised/oversized reads"
        );
        drop(endpoint);

        let (mut endpoint, revision) = Endpoint::start()?;
        let revision = endpoint.publish(1, revision)?;
        endpoint.local_request(1, revision, "text/plain")?;
        ensure!(endpoint.local_result(1, revision)?.is_none());
        ensure!(
            endpoint
                .client
                .as_mut()
                .unwrap()
                .receive_packet(Duration::from_millis(30))?
                .is_none()
        );
        println!("PASS: a local read cannot recurse into or echo the helper's imported promise");
        drop(endpoint);

        // Another process owns a slow promised clipboard. The production helper
        // must keep accepting cancellation while the native read waits on it.
        let owner = Helper::start("slow-local")?;
        let revision = owner.event("ready")?["revision"].as_i64().unwrap() as isize;
        let client = Client::spawn(
            &std::env::current_exe()?,
            Board::Test {
                name: owner.name.clone(),
            },
        )?;
        let mut endpoint = Endpoint {
            client: Some(client),
            name: owner.name.clone(),
            pending: Default::default(),
        };
        endpoint.event(
            |e| matches!(e, Event::Snapshot { revision: value, .. } if *value == revision),
        )?;
        endpoint.local_request(1, revision, "text/plain")?;
        ensure!(owner.event("request")?["format"] == "text/plain");
        endpoint.local_request(2, revision, "image/png")?;
        endpoint
            .sender()
            .send(&Control::CancelLocal { id: 1 }, &[])?;
        endpoint
            .sender()
            .send(&Control::CancelLocal { id: 2 }, &[])?;
        ensure!(endpoint.local_result(1, revision)?.is_none());
        ensure!(endpoint.local_result(2, revision)?.is_none());
        owner.event("completed")?;
        ensure!(
            owner.events.try_recv().is_err(),
            "cancelled queued format was rendered"
        );
        endpoint.local_request(3, revision, "text/plain")?;
        ensure!(endpoint.local_result(3, revision)?.unwrap() == TEXT.as_bytes());
        println!(
            "PASS: cancellation fences a blocked native read and prevents a queued read from rendering"
        );
        drop(endpoint);
        owner.stop()?;

        let (mut endpoint, _) = Endpoint::start()?;
        let revision = endpoint.write_local("local-bulk")?;
        endpoint.local_request(1, revision, "text/plain")?;
        let first = endpoint
            .client
            .as_mut()
            .unwrap()
            .receive_packet(Duration::from_secs(3))?
            .unwrap();
        ensure!(matches!(
            first.event,
            Event::LocalData {
                id: 1,
                offset: 0,
                eof: false,
                ..
            }
        ));
        endpoint
            .sender()
            .send(&Control::CancelLocal { id: 1 }, &[])?;
        let mut failed = false;
        for _ in 0..512 {
            let packet = endpoint
                .client
                .as_mut()
                .unwrap()
                .receive_packet(Duration::from_secs(2))?
                .unwrap();
            match packet.event {
                Event::LocalFailed { id: 1 } => {
                    failed = true;
                    break;
                }
                Event::LocalData {
                    id: 1, eof: false, ..
                } => {}
                other => bail!("Cancelled transfer completed: {other:?}"),
            }
        }
        ensure!(failed);
        println!(
            "PASS: cancellation during a large local transfer discards partial bytes without poisoning the connection"
        );
        drop(endpoint);

        let mut owner = Helper::start("blocking-local")?;
        let revision = owner.event("ready")?["revision"].as_i64().unwrap() as isize;
        let client = Client::spawn(
            &std::env::current_exe()?,
            Board::Test {
                name: owner.name.clone(),
            },
        )?;
        let mut endpoint = Endpoint {
            client: Some(client),
            name: owner.name.clone(),
            pending: Default::default(),
        };
        endpoint.event(
            |e| matches!(e, Event::Snapshot { revision: value, .. } if *value == revision),
        )?;
        endpoint.local_request(1, revision, "text/plain")?;
        owner.event("request")?;
        endpoint.publish(1, revision)?;
        ensure!(
            owner.events.try_recv().is_err(),
            "foreign provider unexpectedly completed before publication"
        );
        owner.command("release")?;
        owner.event("completed")?;
        ensure!(endpoint.local_result(1, revision)?.is_none());
        println!(
            "PASS: a blocked foreign native read does not stall publication or metadata, and cannot return stale bytes"
        );
        drop(endpoint);
        owner.stop()?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    check::run()
}
#[cfg(not(target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("This native clipboard check requires macOS")
}
