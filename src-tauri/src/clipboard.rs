use crate::{model::Server, ssh::ExecSession};
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSPasteboard};
use objc2_foundation::{NSDictionary, NSString};
use std::{collections::BTreeMap, time::Duration};
use tokio::sync::watch;

const MAX_DATA: usize = 32 * 1024 * 1024;

fn data(board: &NSPasteboard, kind: &str) -> Option<Vec<u8>> {
    let t = NSString::from_str(kind);
    if matches!(
        kind,
        "public.utf8-plain-text" | "public.html" | "public.url" | "public.file-url"
    ) {
        return board.stringForType(&t).map(|s| s.to_string().into_bytes());
    }
    if let Some(data) = board.dataForType(&t) {
        return Some(data.to_vec());
    }
    if kind == "public.png" {
        let tiff = board
            .dataForType(&NSString::from_str("public.tiff"))
            .or_else(|| {
                NSImage::initWithPasteboard(NSImage::alloc(), board)
                    .and_then(|image| image.TIFFRepresentation())
            })?;
        let image = NSBitmapImageRep::imageRepWithData(&tiff)?;
        return unsafe {
            image.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .map(|d| d.to_vec());
    }
    None
}
fn urls(board: &NSPasteboard) -> Option<Vec<u8>> {
    let mut urls = Vec::new();
    if let Some(items) = board.pasteboardItems() {
        for item in items.iter() {
            if let Some(url) = item
                .stringForType(&NSString::from_str("public.url"))
                .or_else(|| item.stringForType(&NSString::from_str("public.file-url")))
            {
                urls.push(url.to_string());
            }
        }
    }
    if urls.is_empty() {
        None
    } else {
        Some(urls.join("\n").into_bytes())
    }
}

// Prefer portable representations; optional formats must not reject a usable copy.
#[derive(Default)]
struct Formats {
    values: BTreeMap<String, Vec<u8>>,
    size: usize,
    skipped: bool,
}
impl Formats {
    fn add(&mut self, kind: &str, bytes: Option<Vec<u8>>, limit: usize) {
        // Format names become tar paths and TARGETS lines. Ignore malformed
        // names and aliases already captured in their canonical representation.
        if kind.is_empty()
            || kind.len() > 256
            || kind == "TARGETS"
            || kind.chars().any(char::is_control)
            || kind.split('/').any(|part| matches!(part, "" | "." | ".."))
            || self.values.contains_key(kind)
        {
            return;
        }
        if let Some(bytes) = bytes {
            if self.values.len() >= 256 || bytes.len() > limit.saturating_sub(self.size) {
                self.skipped = true;
                return;
            }
            self.size += bytes.len();
            self.values.insert(kind.to_owned(), bytes);
        }
    }
    fn finish(self, count: isize) -> Result<Snapshot, String> {
        let notice = (self.values.is_empty() && self.skipped)
            .then_some("Clipboard item exceeds 32 MiB. Waiting for a smaller copy.");
        // Publish an empty snapshot for an oversized item, replacing stale data.
        // Remember its change count so we wait for a new copy without rereading it.
        Ok(Snapshot {
            count,
            bytes: archive(self.values)?,
            notice,
        })
    }
}

struct Snapshot {
    count: isize,
    bytes: Vec<u8>,
    notice: Option<&'static str>,
}

// Called on the app's main thread; clipboard data never enters the webview.
fn capture(previous: Option<isize>) -> Result<Option<Snapshot>, String> {
    let board = NSPasteboard::generalPasteboard();
    let count = board.changeCount();
    if previous == Some(count) {
        return Ok(None);
    }
    let mut formats = Formats::default();
    formats.add(
        "text/plain",
        data(&board, "public.utf8-plain-text"),
        MAX_DATA,
    );
    formats.add("image/png", data(&board, "public.png"), MAX_DATA);
    formats.add("text/html", data(&board, "public.html"), MAX_DATA);
    formats.add("text/uri-list", urls(&board), MAX_DATA);
    if let Some(types) = board.types() {
        for kind in types.iter().map(|t| t.to_string()) {
            if !matches!(
                kind.as_str(),
                "public.utf8-plain-text"
                    | "public.html"
                    | "public.png"
                    | "public.url"
                    | "public.file-url"
            ) {
                formats.add(&kind, data(&board, &kind), MAX_DATA);
            }
        }
    }
    // Retry next tick if an external app changed the pasteboard during capture.
    if board.changeCount() != count {
        return Ok(None);
    }
    formats.finish(count).map(Some)
}

fn archive(mut formats: BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut targets: Vec<String> = formats.keys().cloned().collect();
    for (alias, kind) in [
        ("UTF8_STRING", "text/plain"),
        ("STRING", "text/plain"),
        ("TEXT", "text/plain"),
        ("text/plain;charset=utf-8", "text/plain"),
        ("text/plain; charset=utf-8", "text/plain"),
        ("public.utf8-plain-text", "text/plain"),
        ("public.html", "text/html"),
        ("public.png", "image/png"),
        ("public.url", "text/uri-list"),
        ("public.file-url", "text/uri-list"),
    ] {
        if formats.contains_key(kind) {
            targets.push(alias.into());
        }
    }
    targets.push("TARGETS".into());
    formats.insert("TARGETS".into(), (targets.join("\n") + "\n").into_bytes());
    let mut builder = tar::Builder::new(Vec::new());
    for (kind, bytes) in formats {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        builder
            .append_data(&mut header, kind, bytes.as_slice())
            .map_err(|e| e.to_string())?;
    }
    builder.into_inner().map_err(|e| e.to_string())
}

async fn snapshot(
    app: &tauri::AppHandle,
    previous: Option<isize>,
) -> Result<Option<Snapshot>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        if !tx.is_closed() {
            let _ = tx.send(capture(previous));
        }
    })
    .map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .map_err(|_| "Timed out reading the Mac clipboard".to_owned())?
        .map_err(|e| e.to_string())?
}

// Stable per local profile so reconnects can identify their previous agent.
pub fn client_identity(
    directory: &std::path::Path,
    server: uuid::Uuid,
) -> Result<uuid::Uuid, String> {
    let path = directory.join(format!("clipboard-client-{server}"));
    match std::fs::read_to_string(&path) {
        Ok(value) => uuid::Uuid::parse_str(value.trim()).map_err(|e| e.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let id = uuid::Uuid::new_v4();
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .map_err(|e| e.to_string())?;
            file.write_all(id.to_string().as_bytes())
                .map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            Ok(id)
        }
        Err(error) => Err(error.to_string()),
    }
}

// Retry transport failures, never permission, ownership or authentication
// failures. Each agent owns its files until the SSH channel closes or its lease expires.
async fn recover_connection<F, Fut>(
    mut attempt: F,
    mut cancel: watch::Receiver<bool>,
    retrying: impl Fn(),
    delay: Duration,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut wait = delay;
    let mut recovery = crate::system_events::subscribe();
    loop {
        if *cancel.borrow() {
            return Ok(());
        }
        tokio::select! {
            biased;
            _ = cancel.changed() => return Ok(()),
            _ = recovery.ready() => {},
        }
        let started = tokio::time::Instant::now();
        let result = tokio::select! {
            biased;
            _ = cancel.changed() => return Ok(()),
            result = attempt() => result,
        };
        let Err(error) = result else { return Ok(()) };
        let transport_timeout = !error.starts_with("Remote command failed")
            && !error.starts_with("Agent error:")
            && (error.contains("SSH transport interrupted:")
                || error.contains("SSH command timed out")
                || error.contains("SSH connection or authentication timed out"));
        if !transport_timeout {
            log::error!("Integration stopped after a non-retryable error");
            return Err(error);
        }
        log::warn!("Integration interrupted; reconnecting: {error}");
        retrying();
        // A stable session starts a new recovery cycle.
        if started.elapsed() >= Duration::from_secs(60) {
            wait = delay;
        }
        tokio::select! {
            _ = cancel.changed() => return Ok(()),
            _ = recovery.wait(wait) => {}
        }
        wait = (wait * 2).min(Duration::from_secs(30));
    }
}

pub async fn run(
    server: Server,
    client: uuid::Uuid,
    app: tauri::AppHandle,
    cancel: watch::Receiver<bool>,
    ready: impl Fn(String, bool),
    retrying: impl Fn(),
) -> Result<(), String> {
    recover_connection(
        || run_once(server.clone(), client, app.clone(), cancel.clone(), &ready),
        cancel.clone(),
        retrying,
        Duration::from_secs(2),
    )
    .await
}

/// One authenticated SSH connection carries all updates. No HTTP listener or forwarding.
async fn run_once(
    server: Server,
    client: uuid::Uuid,
    app: tauri::AppHandle,
    mut cancel: watch::Receiver<bool>,
    ready: impl Fn(String, bool),
) -> Result<(), String> {
    let session = tokio::select! {
        _ = cancel.changed() => return Ok(()),
        result = ExecSession::connect(&server) => result?,
    };
    let outcome = async {
        let installed = crate::agent::install(&session).await?;
        let path_needed = installed
            .lines()
            .any(|line| line == "PORTHOP_SHIM_PATH=missing");
        let stream = session
            .stream(&format!(
                "exec \"$HOME/.local/bin/porthop-agent\" serve {client} {} {}",
                if server.clipboard_enabled {
                    "--clipboard"
                } else {
                    ""
                },
                if server.browser_enabled {
                    "--browser"
                } else {
                    ""
                }
            ))
            .await?;
        let mut agent = crate::agent::Agent::start(
            stream,
            app.clone(),
            server.browser_enabled,
            session.callbacks(),
        )
        .await?;
        if !server.clipboard_enabled {
            ready("Agent connected.".into(), path_needed);
        }
        log::info!(
            "Integration agent connected (clipboard={}, browser={})",
            server.clipboard_enabled,
            server.browser_enabled
        );
        let mut capture_failed = false;
        let mut previous = None;
        let mut heartbeat = tokio::time::Instant::now();
        let mut timer = tokio::time::interval(Duration::from_millis(200));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let outcome = async {
            loop {
                if *cancel.borrow() {
                    break;
                }
                tokio::select! {
                    _ = cancel.changed() => break,
                    event = agent.event() => {
                        let (kind, data) = event?;
                        if kind != b'O' { return Err("Unexpected agent event.".into()); }
                        agent.open(&data).await?;
                        continue;
                    }
                    _ = timer.tick() => {}
                }
                if let Some(snapshot) = if server.clipboard_enabled {
                    match snapshot(&app, previous).await {
                        Ok(snapshot) => {
                            if capture_failed {
                                log::info!("Mac clipboard reading recovered");
                            }
                            capture_failed = false;
                            snapshot
                        }
                        Err(error) if error.starts_with("Timed out reading the Mac clipboard") => {
                            if !capture_failed {
                                log::warn!("Mac clipboard temporarily unavailable; retrying");
                            }
                            capture_failed = true;
                            None
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    None
                } {
                    if *cancel.borrow() {
                        break;
                    }
                    let backend = agent.request(b'S', &snapshot.bytes).await?;
                    previous = Some(snapshot.count);
                    heartbeat = tokio::time::Instant::now();
                    ready(
                        snapshot.notice.unwrap_or(backend.trim()).to_owned(),
                        path_needed,
                    );
                } else if heartbeat.elapsed() >= Duration::from_secs(5) {
                    agent.request(b'H', &[]).await?;
                    heartbeat = tokio::time::Instant::now();
                }
            }
            Ok(())
        }
        .await;
        agent.close().await;
        outcome
    }
    .await;
    session.close().await;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_extra_representation_does_not_reject_portable_content() {
        let mut formats = Formats::default();
        formats.add("image/png", Some(vec![1; 4]), 10);
        formats.add("public.tiff", Some(vec![2; 20]), 10);
        formats.add("custom-small", Some(vec![3; 2]), 10);
        let values = formats.values;
        assert_eq!(values["image/png"], vec![1; 4]);
        assert!(!values.contains_key("public.tiff"));
        assert_eq!(values["custom-small"], vec![3; 2]);
    }

    #[test]
    fn combined_formats_stay_within_budget_and_oversize_can_fall_back() {
        let mut formats = Formats::default();
        formats.add("text/plain", Some(vec![1; 11]), 10);
        formats.add("image/png", Some(vec![2; 6]), 10);
        formats.add("text/html", Some(vec![3; 5]), 10);
        formats.add("text/uri-list", Some(vec![4; 4]), 10);
        let values = formats.values;
        assert_eq!(values.values().map(Vec::len).sum::<usize>(), 10);
        assert!(values.contains_key("image/png"));
        assert!(!values.contains_key("text/html"));
    }

    #[test]
    fn oversized_copy_clears_snapshot_and_next_copy_recovers() {
        let empty = Formats::default().finish(1).unwrap();
        assert!(empty.notice.is_none());
        let mut formats = Formats::default();
        formats.add("text/plain", Some(vec![1; 11]), 10);
        let skipped = formats.finish(2).unwrap();
        assert_eq!(skipped.count, 2);
        assert!(skipped
            .notice
            .unwrap()
            .contains("Waiting for a smaller copy"));
        assert_eq!(skipped.bytes, empty.bytes);
        let mut next = Formats::default();
        next.add("text/plain", Some(b"small".to_vec()), 10);
        let recovered = next.finish(3).unwrap();
        assert_eq!(recovered.count, 3);
        assert!(recovered.notice.is_none());
        assert_ne!(recovered.bytes, empty.bytes);
    }

    #[test]
    fn malformed_and_duplicate_formats_do_not_break_snapshot() {
        let mut formats = Formats::default();
        formats.add("text/plain", Some(b"original".to_vec()), 10);
        for kind in [
            "text/plain",
            "../bad",
            "/absolute",
            "bad\nname",
            "bad\0name",
            "TARGETS",
        ] {
            formats.add(kind, Some(vec![1; 20]), 10);
        }
        assert!(!formats.skipped);
        assert_eq!(formats.size, 8);
        let snapshot = formats.finish(1).unwrap();
        let mut archive = tar::Archive::new(snapshot.bytes.as_slice());
        let paths: Vec<_> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().into_owned())
            .collect();
        assert_eq!(
            paths,
            vec![
                std::path::PathBuf::from("TARGETS"),
                std::path::PathBuf::from("text/plain")
            ]
        );
    }

    #[test]
    fn format_count_and_names_match_agent_limits() {
        let mut formats = Formats::default();
        formats.add(&"x".repeat(257), Some(vec![1]), MAX_DATA);
        assert!(formats.values.is_empty());
        for n in 0..257 {
            formats.add(&format!("application/x-test-{n}"), Some(vec![1]), MAX_DATA);
        }
        assert_eq!(formats.values.len(), 256);
        assert!(formats.skipped);
        let snapshot = formats.finish(1).unwrap();
        let mut archive = tar::Archive::new(snapshot.bytes.as_slice());
        assert_eq!(archive.entries().unwrap().count(), 257);
    }
    #[tokio::test]
    async fn timeout_recovery_retries_but_preserves_permanent_errors() {
        let (_tx, rx) = watch::channel(false);
        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(if attempts == 1 {
                    Err("Clipboard upload failed: SSH command timed out after 25 seconds".into())
                } else {
                    Ok(())
                })
            },
            rx.clone(),
            || {},
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 2);

        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(if attempts < 6 {
                    Err("SSH transport interrupted: Connection reset by peer".into())
                } else {
                    Ok(())
                })
            },
            rx.clone(),
            || {},
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 6);

        let mut attempts = 0;
        let error = recover_connection(
            || {
                attempts += 1;
                std::future::ready(Err(
                    "Clipboard sync is already active for this server account.".into(),
                ))
            },
            rx,
            || panic!("Ownership errors must not retry"),
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert!(error.contains("already active"));
    }

    #[tokio::test]
    async fn disabling_sync_cancels_pending_recovery() {
        let (tx, rx) = watch::channel(false);
        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(Err("SSH command timed out after 25 seconds".into()))
            },
            rx,
            || {
                tx.send(true).unwrap();
            },
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn disabling_sync_cancels_an_inflight_attempt_without_waiting_for_timeout() {
        let (tx, rx) = watch::channel(false);
        let run = recover_connection(
            std::future::pending::<Result<(), String>>,
            rx,
            || panic!("Cancellation must not retry"),
            Duration::from_secs(60),
        );
        let cancel = async {
            tokio::task::yield_now().await;
            tx.send(true).unwrap();
        };
        let (result, _) = tokio::time::timeout(Duration::from_millis(200), async {
            tokio::join!(run, cancel)
        })
        .await
        .expect("Cancellation must interrupt in-flight work");
        result.unwrap();
    }

    #[tokio::test]
    async fn permanent_agent_failures_do_not_retry_even_with_transport_words() {
        let (_tx, rx) = watch::channel(false);
        let error = recover_connection(
            || {
                std::future::ready(Err(
                    "Agent error: SSH transport interrupted: disk is read-only".into(),
                ))
            },
            rx,
            || panic!("Permanent agent errors must remain visible"),
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(error.starts_with("Agent error:"));
    }
    #[test]
    fn client_identity_survives_restart_and_isolates_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let server = uuid::Uuid::new_v4();
        let first = client_identity(dir.path(), server).unwrap();
        assert_eq!(first, client_identity(dir.path(), server).unwrap());
        assert_ne!(
            first,
            client_identity(dir.path(), uuid::Uuid::new_v4()).unwrap()
        );
    }

    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh and its isolated remote HOME"]
    async fn ssh_agent_snapshot_browser_and_cleanup() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let server = Server {
            id: uuid::Uuid::new_v4(),
            name: "Agent fixture".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: std::env::var("PORTHOP_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("PORTHOP_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: crate::model::AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
        };
        let session = ExecSession::connect(&server).await.unwrap();
        let mut stream = session.stream("fixture-agent").await.unwrap();
        async fn event(stream: &mut (impl tokio::io::AsyncRead + Unpin), kind: u8) -> Vec<u8> {
            let mut header = [0; 5];
            tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut header))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(header[0], kind);
            let mut data = vec![0; u32::from_be_bytes(header[1..].try_into().unwrap()) as usize];
            stream.read_exact(&mut data).await.unwrap();
            data
        }
        assert_eq!(event(&mut stream, b'R').await, b"porthop-agent/3");
        let bytes = archive(BTreeMap::from([(
            "text/plain".into(),
            "SSH clipboard 世界\n".as_bytes().to_vec(),
        )]))
        .unwrap();
        let mut frame = vec![b'S'];
        frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        frame.extend(bytes);
        stream.write_all(&frame).await.unwrap();
        event(&mut stream, b'A').await;
        assert_eq!(
            session.execute("fixture-clipboard", None).await.unwrap(),
            "SSH clipboard 世界\n"
        );
        let (opened, ()) = tokio::join!(session.execute("fixture-open", None), async {
            assert_eq!(
                event(&mut stream, b'O').await,
                b"1\nhttps://example.com/login?code=fixture"
            );
            stream
                .write_all(&[b'B', 0, 0, 0, 4, b'1', b'\n', b'o', b'k'])
                .await
                .unwrap();
        });
        opened.unwrap();
        stream.write_all(&[b'Q', 0, 0, 0, 0]).await.unwrap();
        let mut end = Vec::new();
        stream.read_to_end(&mut end).await.unwrap();
        assert!(session.execute("fixture-clipboard", None).await.is_err());
        session.close().await;
    }
}
