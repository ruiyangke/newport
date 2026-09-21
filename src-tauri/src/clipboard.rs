use crate::{model::Server, ssh::ExecSession};
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSPasteboard};
use objc2_foundation::{NSDictionary, NSString};
use std::{collections::BTreeMap, time::Duration};
use tokio::sync::watch;

const MAX_DATA: usize = 32 * 1024 * 1024;
const HELPER: &str = include_str!("porthop-clip.sh");
const INSTALLER: &str = include_str!("clipboard-install.sh");

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

// Called on the app's main thread; clipboard data never enters the webview.
fn capture(previous: Option<isize>) -> Result<Option<(isize, Vec<u8>)>, String> {
    let board = NSPasteboard::generalPasteboard();
    let count = board.changeCount();
    if previous == Some(count) {
        return Ok(None);
    }
    let mut formats = BTreeMap::new();
    let mut size = 0;
    let mut add = |kind: &str, bytes: Option<Vec<u8>>| -> Result<(), String> {
        if let Some(bytes) = bytes {
            size += bytes.len();
            if size > MAX_DATA {
                return Err("Clipboard content exceeds 32 MiB. Copy a smaller item and enable syncing again.".into());
            }
            formats.insert(kind.to_owned(), bytes);
        }
        Ok(())
    };
    add("text/plain", data(&board, "public.utf8-plain-text"))?;
    add("text/html", data(&board, "public.html"))?;
    add("image/png", data(&board, "public.png"))?;
    add("text/uri-list", urls(&board))?;
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
                add(&kind, data(&board, &kind))?;
            }
        }
    }
    // Retry next tick if an external app changed the pasteboard during capture.
    if board.changeCount() != count {
        return Ok(None);
    }
    archive(formats).map(|bytes| Some((count, bytes)))
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
) -> Result<Option<(isize, Vec<u8>)>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(capture(previous));
    })
    .map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .map_err(|_| "Timed out reading the Mac clipboard".to_owned())?
        .map_err(|e| e.to_string())?
}

pub async fn install(session: &ExecSession) -> Result<String, String> {
    let command = format!("sh -c '{}'", INSTALLER.replace('\'', "'\"'\"'"));
    session.execute(&command, Some(HELPER.as_bytes())).await
}

fn command(action: &str, token: uuid::Uuid) -> String {
    format!("bash \"$HOME/.local/bin/porthop-clip\" --{action} {token}")
}

// Stable per local profile; session tokens still change to fence off late writes.
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
// failures. A fresh connection and session token fence off the previous upload.
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
    loop {
        if *cancel.borrow() {
            return Ok(());
        }
        let started = tokio::time::Instant::now();
        let result = attempt().await;
        let Err(error) = result else { return Ok(()) };
        let transport_timeout = !error.contains("Remote command failed")
            && (error.contains("SSH transport interrupted:")
                || error.contains("SSH command timed out")
                || error.contains("SSH connection or authentication timed out"));
        if !transport_timeout {
            return Err(error);
        }
        log::warn!("Clipboard sync interrupted; reconnecting: {error}");
        retrying();
        // A stable session starts a new recovery cycle.
        if started.elapsed() >= Duration::from_secs(60) {
            wait = delay;
        }
        tokio::select! {
            _ = cancel.changed() => return Ok(()),
            _ = tokio::time::sleep(wait) => {}
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
    let token = uuid::Uuid::new_v4();
    let mut begun = false;
    let outcome = async {
        let installed = install(&session).await
            .map_err(|error| format!("Clipboard setup failed: {error}"))?;
        let path_needed = installed.lines().any(|line| line == "PORTHOP_SHIM_PATH=missing");
        let note = installed.lines().filter(|line| !line.starts_with("PORTHOP_SHIM_PATH=")).collect::<Vec<_>>().join(" ");
        if *cancel.borrow() {
            return Ok(());
        }
        // Cleanup must also run if the begin reply is lost after the remote write.
        begun = true;
        let begin = format!("{} {client}", command("begin", token));
        // Old helpers have no persistent client identity. Allow their lease to
        // expire without stealing ownership from another active installation.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(125);
        loop {
            match session.execute(&begin, None).await {
                Ok(_) => break,
                Err(error) if error.contains("Clipboard sync is already active for this server account.") => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err("Clipboard sync is active on another connection. Stop it there, then try again.".into());
                    }
                    tokio::select! {
                        _ = cancel.changed() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    }
                }
                Err(error) => return Err(format!("Clipboard session could not start: {error}")),
            }
        }
        let mut previous = None;
        let mut heartbeat = tokio::time::Instant::now();
        let mut timer = tokio::time::interval(Duration::from_millis(200));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if *cancel.borrow() {
                break;
            }
            tokio::select! {
                _ = cancel.changed() => break,
                _ = timer.tick() => {}
            }
            if let Some((count, bytes)) = snapshot(&app, previous).await? {
                if *cancel.borrow() {
                    break;
                }
                let backend = session
                    .execute(&command("receive", token), Some(&bytes))
                    .await
                    .map_err(|error| format!("Clipboard upload failed: {error}"))?;
                previous = Some(count);
                heartbeat = tokio::time::Instant::now();
                ready(format!("{} {}", backend.trim(), note.trim()), path_needed);
            } else if heartbeat.elapsed() >= Duration::from_secs(30) {
                session.execute(&command("heartbeat", token), None).await
                    .map_err(|error| format!("Clipboard connection check failed: {error}"))?;
                heartbeat = tokio::time::Instant::now();
            }
        }
        Ok(())
    }
    .await;
    let cleanup = if begun {
        tokio::time::timeout(
            Duration::from_secs(3),
            session.execute(&command("clear", token), None),
        )
        .await
        .map_err(|_| "Remote cleanup timed out".to_owned())
        .and_then(|r| r.map(|_| ()))
    } else {
        Ok(())
    };
    session.close().await;
    match (outcome, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(format!("Sync stopped, but the remote snapshot could not be deleted. It expires within two minutes. {error}")),
        (Ok(()), Ok(())) => Ok(()),
    }
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
    async fn ssh_snapshot_install_push_replace_and_clear() {
        let server = Server {
            id: uuid::Uuid::new_v4(),
            name: "Clipboard fixture".into(),
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
        };
        let session = ExecSession::connect(&server).await.unwrap();
        assert!(install(&session)
            .await
            .unwrap()
            .contains("PORTHOP_SHIM_PATH="));
        let token = uuid::Uuid::new_v4();
        session
            .execute(&command("begin", token), None)
            .await
            .unwrap();
        let bytes = archive(BTreeMap::from([
            (
                "text/plain".into(),
                "SSH clipboard 世界\n".as_bytes().to_vec(),
            ),
            ("image/png".into(), vec![0, 255, 128, 10]),
        ]))
        .unwrap();
        session
            .execute(&command("receive", token), Some(&bytes))
            .await
            .unwrap();
        let read = "\"$HOME/.local/bin/xclip\" -selection clipboard -o";
        assert_eq!(
            session.execute(read, None).await.unwrap(),
            "SSH clipboard 世界\n"
        );
        let targets = "\"$HOME/.local/bin/xclip\" -o -t TARGETS";
        assert!(session
            .execute(targets, None)
            .await
            .unwrap()
            .contains("image/png"));
        session
            .execute(&command("heartbeat", token), None)
            .await
            .unwrap();
        session
            .execute(
                &command("receive", token),
                Some(&archive(BTreeMap::new()).unwrap()),
            )
            .await
            .unwrap();
        assert_eq!(session.execute(targets, None).await.unwrap(), "TARGETS\n");
        assert!(session
            .execute(read, None)
            .await
            .unwrap_err()
            .contains("Remote command failed"));
        session
            .execute(&command("clear", token), None)
            .await
            .unwrap();
        assert!(session
            .execute(read, None)
            .await
            .unwrap_err()
            .contains("No clipboard snapshot"));
        session.close().await;

        // Drop a real SSH transport, then exercise the same recovery loop used
        // by clipboard sync and verify the next connection can publish/read.
        let (_tx, rx) = watch::channel(false);
        let mut attempts = 0;
        recover_connection(
            || {
                attempts += 1;
                let first = attempts == 1;
                let server = &server;
                let bytes = &bytes;
                async move {
                    let session = ExecSession::connect(server).await?;
                    if first {
                        let _ = session.execute("drop", None).await;
                        return session.execute("printf ok", None).await.map(|_| ());
                    }
                    install(&session).await?;
                    let token = uuid::Uuid::new_v4();
                    session.execute(&command("begin", token), None).await?;
                    session
                        .execute(&command("receive", token), Some(bytes))
                        .await?;
                    assert_eq!(session.execute(read, None).await?, "SSH clipboard 世界\n");
                    session.execute(&command("clear", token), None).await?;
                    session.close().await;
                    Ok(())
                }
            },
            rx,
            || {},
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(attempts, 2);
    }
}
