use crate::{model::Server, ssh::ExecSession};
#[cfg(test)]
use std::collections::BTreeMap;
use std::time::Duration;
#[allow(dead_code)]
#[path = "../../tools/agent/src/clipboard_wire.rs"]
mod clipboard_wire;
use tokio::sync::watch;

const MAX_DATA: usize = 32 * 1024 * 1024;

#[cfg(test)]
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

struct ClipboardReply {
    request: clipboard_wire::Request,
    chunks: Option<std::collections::VecDeque<(u8, Vec<u8>)>>,
}
async fn on_main<T: Send + 'static>(
    app: &tauri::AppHandle,
    action: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        if !tx.is_closed() {
            let _ = tx.send(action());
        }
    })
    .map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .map_err(|_| "Timed out reading the clipboard".to_owned())?
        .map_err(|e| e.to_string())
}
async fn clipboard_offer(
    app: &tauri::AppHandle,
    previous: Option<isize>,
) -> Result<Option<clipboard_wire::Offer>, String> {
    let result = on_main(app, move || {
        crate::platform::clipboard::offer(previous).map(|(revision, mut formats)| {
            formats.retain(|s| clipboard_wire::valid_format(s) && s != "TARGETS");
            formats.truncate(256);
            log::info!(
                "Clipboard offer revision={revision} formats={}",
                formats.len()
            );
            clipboard_wire::Offer {
                revision: revision as i64,
                formats,
            }
        })
    })
    .await;
    match result {
        Err(error) if error == "Timed out reading the clipboard" => {
            log::warn!("Clipboard metadata temporarily unavailable; retrying");
            Ok(None)
        }
        other => other,
    }
}
async fn read_requested(
    app: &tauri::AppHandle,
    request: clipboard_wire::Request,
) -> ClipboardReply {
    let revision = request.revision;
    let format = request.format.clone();
    let started = tokio::time::Instant::now();
    let bytes = on_main(app, move || {
        let (current, formats) = crate::platform::clipboard::offer(None)?;
        if current as i64 != revision || !formats.contains(&format) {
            return None;
        }
        crate::platform::clipboard::read_format(current, &format).filter(|b| b.len() <= MAX_DATA)
    })
    .await
    .ok()
    .flatten();
    log::info!(
        "Clipboard request id={} revision={} success={} bytes={} elapsed_ms={}",
        request.id,
        revision,
        bytes.is_some(),
        bytes.as_ref().map_or(0, Vec::len),
        started.elapsed().as_millis()
    );
    let original_bytes = bytes.as_ref().map_or(0, Vec::len);
    // Compression runs off the UI and async event-loop threads.
    let chunks = match bytes {
        Some(bytes) => tokio::task::spawn_blocking(move || {
            if bytes.is_empty() {
                return Ok(std::collections::VecDeque::from([(0, Vec::new())]));
            }
            bytes
                .chunks(clipboard_wire::CHUNK)
                .map(clipboard_wire::compress_chunk)
                .collect::<std::io::Result<std::collections::VecDeque<_>>>()
        })
        .await
        .ok()
        .and_then(Result::ok),
        None => None,
    };
    if let Some(chunks) = &chunks {
        let transfer_bytes: usize = chunks.iter().map(|(_, bytes)| bytes.len()).sum();
        log::info!("Clipboard transfer prepared id={} revision={} original_bytes={} transfer_bytes={} compressed_chunks={}", request.id, revision, original_bytes, transfer_bytes, chunks.iter().filter(|(status, _)| *status == 2).count());
    }
    ClipboardReply { request, chunks }
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
            let id = uuid::Uuid::new_v4();
            let mut file = crate::platform::filesystem::private_file_options()
                .write(true)
                .create_new(true)
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

// Retry transport failures and agent-declared temporary failures (including
// exhausted disk space), never permission, ownership or authentication
// failures. Each agent owns its files until the SSH channel closes or its lease expires.
async fn recover_connection<F, Fut>(
    mut attempt: F,
    mut cancel: watch::Receiver<bool>,
    retrying: impl Fn(),
    delay: Duration,
    mut changes: Option<watch::Receiver<Option<isize>>>,
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
        if let Some(changes) = &mut changes {
            changes.borrow_and_update();
        }
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
            _ = async {
                if let Some(changes) = &mut changes {
                    if changes.changed().await.is_ok() {
                        return;
                    }
                }
                std::future::pending::<()>().await;
            } => {},
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
    let initial = if server.clipboard_enabled {
        revision(&app).await
    } else {
        None
    };
    let (send, receive) = watch::channel(initial);
    let recovery = recover_connection(
        || run_once(server.clone(), client, app.clone(), cancel.clone(), &ready),
        cancel.clone(),
        retrying,
        Duration::from_secs(2),
        server.clipboard_enabled.then_some(receive),
    );
    // Observe only the cheap revision counter; actual contents are read by
    // run_once after reconnecting. Keep this scoped to the integration session.
    let monitor = async {
        if !server.clipboard_enabled {
            std::future::pending::<()>().await;
        }
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Some(count) = revision(&app).await {
                send.send_if_modified(|previous| {
                    if *previous == Some(count) {
                        return false;
                    }
                    *previous = Some(count);
                    log::info!("Clipboard change detected revision={count}");
                    true
                });
            }
        }
    };
    tokio::select! {
        result = recovery => result,
        _ = monitor => unreachable!(),
    }
}

async fn revision(app: &tauri::AppHandle) -> Option<isize> {
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = send.send(crate::platform::clipboard::revision());
    })
    .ok()?;
    tokio::time::timeout(Duration::from_secs(5), receive)
        .await
        .ok()?
        .ok()?
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
        let mut previous = None;
        let mut heartbeat = tokio::time::Instant::now();
        let mut timer = tokio::time::interval(Duration::from_millis(200));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut reads = tokio::task::JoinSet::new();
        let mut replies = std::collections::VecDeque::<ClipboardReply>::new();
        let outcome = async {
            loop {
                if *cancel.borrow() { break; }
                tokio::select! {
                    _ = cancel.changed() => break,
                    event = agent.event() => {
                        let (kind, data) = event?;
                        match kind {
                            b'O' => agent.open(&data).await?,
                            b'C' if server.clipboard_enabled => {
                                let request = clipboard_wire::Request::decode(&data).map_err(|e| e.to_string())?;
                                if reads.len() + replies.len() >= 8 {
                                    agent.clipboard_reply(&clipboard_wire::reply(&request, 1, true, &[])).await?;
                                } else {
                                    let app = app.clone();
                                    reads.spawn(async move { read_requested(&app, request).await });
                                }
                            }
                            _ => return Err("Unexpected agent event.".into()),
                        }
                    }
                    result = reads.join_next(), if !reads.is_empty() => {
                        replies.push_back(result.unwrap().map_err(|e| e.to_string())?);
                    }
                    // One bounded chunk per turn lets events and timers run between writes.
                    _ = tokio::task::yield_now(), if !replies.is_empty() => {
                        let mut reply = replies.pop_front().unwrap();
                        let current = crate::platform::clipboard::revision();
                        if current != Some(reply.request.revision as isize) { reply.chunks = None; }
                        let (status, bytes) = reply.chunks.as_mut().and_then(|chunks| chunks.pop_front()).unwrap_or((1, Vec::new()));
                        let done = reply.chunks.as_ref().is_none_or(|chunks| chunks.is_empty());
                        agent.clipboard_reply(&clipboard_wire::reply(&reply.request, status, done, &bytes)).await?;
                        if !done { replies.push_back(reply); }
                    }
                    _ = timer.tick() => {
                        if server.clipboard_enabled && reads.is_empty() {
                            if let Some(offer) = clipboard_offer(&app, previous).await? {
                                let revision = offer.revision;
                                let backend = agent.request(b'M', &offer.encode()).await?;
                                previous = Some(revision as isize);
                                heartbeat = tokio::time::Instant::now();
                                ready(backend, path_needed);
                            }
                        }
                        if heartbeat.elapsed() >= Duration::from_secs(5) {
                            agent.request(b'H', &[]).await?;
                            heartbeat = tokio::time::Instant::now();
                        }
                    }
                }
            }
            Ok(())
        }.await;
        reads.abort_all();
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
            None,
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
            None,
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
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert!(error.contains("already active"));
    }

    #[tokio::test]
    async fn copy_during_failed_attempt_is_not_lost() {
        let (_cancel, rx) = watch::channel(false);
        let (changes, revisions) = watch::channel(Some(1));
        let mut attempts = 0;
        tokio::time::timeout(
            Duration::from_millis(200),
            recover_connection(
                || {
                    attempts += 1;
                    std::future::ready(if attempts == 1 {
                        changes.send(Some(2)).unwrap();
                        Err("SSH transport interrupted: No space left on device".into())
                    } else {
                        Ok(())
                    })
                },
                rx,
                || {},
                Duration::from_secs(30),
                Some(revisions),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(attempts, 2);
    }

    #[tokio::test]
    async fn new_copy_wakes_backoff_once_without_repeating_unchanged_copies() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let (_cancel, rx) = watch::channel(false);
        let (changes, revisions) = watch::channel(Some(1));
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        let recovery = recover_connection(
            move || {
                let attempt = count.fetch_add(1, Ordering::SeqCst) + 1;
                std::future::ready(if attempt < 3 {
                    Err("SSH transport interrupted: No space left on device".into())
                } else {
                    Ok(())
                })
            },
            rx,
            || {},
            Duration::from_secs(30),
            Some(revisions),
        );
        tokio::pin!(recovery);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut recovery)
                .await
                .is_err()
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        changes.send(Some(2)).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut recovery)
                .await
                .is_err()
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut recovery)
                .await
                .is_err()
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        changes.send(Some(3)).unwrap();
        tokio::time::timeout(Duration::from_millis(200), &mut recovery)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn disk_full_agent_retries_until_space_is_available() {
        let (_tx, rx) = watch::channel(false);
        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                std::future::ready(if attempts < 4 {
                    Err(crate::agent::checked_event(
                        b'T',
                        b"No space left on device (os error 28) at path /clipboard/.tmp".to_vec(),
                    )
                    .unwrap_err())
                } else {
                    Ok(())
                })
            },
            rx,
            || {},
            Duration::ZERO,
            None,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 4);
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
            None,
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
            None,
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
            None,
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
        assert_eq!(event(&mut stream, b'R').await, b"porthop-agent/5");
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
