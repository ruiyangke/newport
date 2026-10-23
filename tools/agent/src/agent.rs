//! Integration session event loop: dispatch frames, track liveness, and coordinate services.
use crate::{
    snapshot,
    transport::{DeadlineWriter, TrackedReader},
    wire,
};
use std::{
    fs,
    io::{self, Write},
    os::fd::{AsFd, AsRawFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

fn error_kind(error: &io::Error) -> u8 {
    match error.kind() {
        io::ErrorKind::WouldBlock
        | io::ErrorKind::TimedOut
        | io::ErrorKind::Interrupted
        | io::ErrorKind::StorageFull
        | io::ErrorKind::QuotaExceeded => b'T',
        _ => b'E',
    }
}

#[derive(Default)]
struct SessionOptions {
    clipboard: bool,
    browser: bool,
    managed: bool,
    cancellation: crate::transport::Cancellation,
    permissions: Option<mpsc::Receiver<crate::worker_session::PermissionCall>>,
    scope: Option<crate::routing::Scope>,
}

pub fn serve(client: &str, clipboard: bool, browser: bool) -> io::Result<()> {
    serve_inner(
        client,
        SessionOptions {
            clipboard,
            browser,
            ..Default::default()
        },
    )
}
pub fn serve_managed(client: &str) -> io::Result<()> {
    serve_inner(
        client,
        SessionOptions {
            managed: true,
            ..Default::default()
        },
    )
}
pub(crate) fn serve_bound(
    client: &str,
    cancellation: crate::transport::Cancellation,
    permissions: mpsc::Receiver<crate::worker_session::PermissionCall>,
    scope: crate::routing::Scope,
) -> io::Result<()> {
    serve_inner(
        client,
        SessionOptions {
            managed: true,
            cancellation,
            permissions: Some(permissions),
            scope: Some(scope),
            ..Default::default()
        },
    )
}
fn serve_inner(client: &str, options: SessionOptions) -> io::Result<()> {
    if client.len() > 64
        || !client
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(io::Error::other("invalid client identity"));
    }
    let stdout = fs::File::from(io::stdout().as_fd().try_clone_to_owned()?);
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut output = DeadlineWriter {
        inner: stdout,
        timeout: Duration::from_secs(5),
        cancellation: options.cancellation.clone(),
    };
    let result = run(client, options, &mut output);
    if let Err(error) = &result {
        let _ = wire::write(&mut output, error_kind(error), error.to_string().as_bytes());
    }
    result
}
fn run(client: &str, options: SessionOptions, output: &mut impl Write) -> io::Result<()> {
    use crate::integration_control as control;
    let SessionOptions {
        mut clipboard,
        browser,
        managed,
        cancellation,
        permissions,
        scope,
    } = options;
    let (mut session, listener) =
        crate::session::Session::start(client, clipboard, browser, scope)?;
    let snapshot_path = session.snapshot.clone();
    let snapshot = &snapshot_path;
    crate::diagnostics::event(
        snapshot,
        "session_started",
        &format!("clipboard={clipboard} browser={browser}"),
    );
    let mut demand = crate::demand::Store::new();
    let mut clipboard_service = None;
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed)).map_err(io::Error::other)?;
    let last_input = Arc::new(Mutex::new(Instant::now()));
    let activity = last_input.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    let input = cancellation.stdin()?;
    thread::spawn(move || {
        let mut input = TrackedReader { input, activity };
        loop {
            let frame = if managed {
                wire::read_limited(&mut input, crate::clipboard_wire::CHUNK + 1024)
            } else {
                wire::read(&mut input)
            };
            let failed = frame.is_err();
            if tx.send(frame).is_err() || failed {
                break;
            }
        }
    });
    if managed {
        wire::write(
            output,
            b'R',
            &control::encode(&control::Hello::new(uuid::Uuid::new_v4().to_string()))?,
        )?;
    } else {
        wire::write(output, b'R', wire::VERSION.as_bytes())?;
    }
    let mut applied: Option<control::Applied> = None;
    let mut browser = crate::browser::Broker::new(listener, browser);
    loop {
        browser.expire();
        if stopped.load(Ordering::Relaxed) || cancellation.check().is_err() {
            return Ok(());
        }
        if last_input.lock().unwrap().elapsed() > Duration::from_secs(45) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "agent heartbeat expired",
            ));
        }
        session.check_backends()?;
        if let Some(requests) = &permissions {
            if let Ok(request) = requests.try_recv() {
                let current = apply_permissions(
                    request.permissions,
                    &mut applied,
                    &mut session,
                    &mut browser,
                    &mut demand,
                    &mut clipboard_service,
                )?;
                clipboard = session.clipboard_enabled();
                let _ = request.reply.send(current);
            }
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(Ok((kind, data))) => {
                cancellation.check()?;
                match kind {
                    b'P' if managed && permissions.is_none() => {
                        let requested: control::Permissions = control::decode(&data)?;
                        let current = apply_permissions(
                            requested,
                            &mut applied,
                            &mut session,
                            &mut browser,
                            &mut demand,
                            &mut clipboard_service,
                        )?;
                        clipboard = session.clipboard_enabled();
                        wire::write(output, b'A', &control::encode(&current)?)?;
                    }
                    b'M' if clipboard => {
                        if clipboard_service.is_none() {
                            clipboard_service = Some(crate::clipboard_source::Service::start(
                                snapshot,
                                demand.clone(),
                            )?);
                        }
                        let offer = crate::clipboard_wire::Offer::decode(&data)?;
                        crate::diagnostics::event(
                            snapshot,
                            "clipboard_offer",
                            &format!(
                                "revision={} formats={}",
                                offer.revision,
                                offer.formats.len()
                            ),
                        );
                        demand.update(offer);
                        session.source.enable(demand.clone());
                        session.refresh_clipboard()?;
                        wire::write(output, b'A', b"Clipboard available on demand.")?;
                    }
                    b'D' if clipboard || managed => {
                        // A request already emitted before revocation may receive
                        // a late refusal/data frame. Validate then discard it;
                        // never reopen the clipboard or stop another capability.
                        if !clipboard {
                            crate::clipboard_wire::parse_reply(&data)?;
                            continue;
                        }
                        demand.reply(&data)?;
                        let (id, revision, status, done, _) =
                            crate::clipboard_wire::parse_reply(&data)?;
                        if done {
                            crate::diagnostics::event(
                                snapshot,
                                "clipboard_reply",
                                &format!(
                                    "id={id} revision={revision} sender_success={}",
                                    status != 1
                                ),
                            );
                        }
                    }
                    b'S' if !managed && clipboard && clipboard_service.is_none() => {
                        let started = Instant::now();
                        crate::diagnostics::event(
                            snapshot,
                            "snapshot_received",
                            &format!("archive_bytes={}", data.len()),
                        );
                        if let Err(error) = snapshot::publish(snapshot, &data) {
                            crate::diagnostics::event(
                                snapshot,
                                "snapshot_failed",
                                &format!("kind={:?} os={:?}", error.kind(), error.raw_os_error()),
                            );
                            return Err(error);
                        }
                        session.refresh_clipboard()?;
                        let backend = crate::native::update(snapshot);
                        wire::write(output, b'A', backend.as_bytes())?;
                        crate::diagnostics::event(
                            snapshot,
                            "snapshot_acknowledged",
                            &format!("elapsed_ms={}", started.elapsed().as_millis()),
                        );
                    }
                    b'G' if managed && data.is_empty() => {
                        session.enable_gui_notices()?;
                        session.publish_features(browser.enabled())?;
                        wire::write(output, b'A', b"")?;
                    }
                    b'L' if managed && data.is_empty() => {
                        if session.gui_launch.is_none() {
                            session.gui_launch = Some(crate::gui_launch::Broker::start(
                                snapshot.parent().unwrap(),
                            )?);
                        }
                        session.publish_features(browser.enabled())?;
                        wire::write(output, b'A', b"")?;
                    }
                    b'J' if managed => {
                        if let Some(broker) = &mut session.gui_launch {
                            broker.reply(&data)?;
                        }
                        wire::write(output, b'A', b"")?;
                    }
                    b'H' if data.is_empty() => {
                        if let Ok(file) = fs::OpenOptions::new().write(true).open(snapshot) {
                            file.set_modified(std::time::SystemTime::now())?;
                        }
                        wire::write(output, b'A', b"")?;
                    }
                    b'B' if browser.enabled() || managed => browser.reply(&data),
                    b'Q' if data.is_empty() => return Ok(()),
                    _ => return Err(io::Error::other("unsupported agent message")),
                }
            }
            Ok(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(e)) => return Err(e),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        }
        if session.source.gui.take() {
            wire::write(output, b'G', b"")?;
        }
        demand.poll(output)?;
        browser.poll(output)?;
        if let Some(broker) = &mut session.gui_launch {
            broker.poll(output)?;
        }
    }
}

fn apply_permissions(
    requested: crate::integration_control::Permissions,
    applied: &mut Option<crate::integration_control::Applied>,
    session: &mut crate::session::Session,
    browser: &mut crate::browser::Broker,
    demand: &mut Arc<crate::demand::Store>,
    clipboard_service: &mut Option<crate::clipboard_source::Service>,
) -> io::Result<crate::integration_control::Applied> {
    use crate::integration_control as control;
    if requested.revision == 0 {
        return Err(io::Error::other("invalid permission revision"));
    }
    if let Some(previous) = applied {
        if requested.revision <= previous.permissions.revision {
            return Ok(previous.clone());
        }
    }
    if !requested.clipboard {
        demand.close();
        *clipboard_service = None;
        *demand = crate::demand::Store::new();
    }
    browser.set_enabled(requested.browser);
    let error = session
        .set_clipboard(requested.clipboard)
        .err()
        .map(|e| e.to_string());
    session.publish_features(browser.enabled())?;
    let current = control::Applied {
        permissions: control::Permissions {
            revision: requested.revision,
            clipboard: session.clipboard_enabled(),
            browser: browser.enabled(),
        },
        error,
    };
    *applied = Some(current.clone());
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disk_capacity_errors_are_retryable_including_path_wrappers() {
        for code in [libc::ENOSPC, libc::EDQUOT] {
            let error = io::Error::from_raw_os_error(code);
            assert_eq!(error_kind(&error), b'T');
            // tempfile attaches the failing path, preserving ErrorKind rather
            // than raw_os_error. Classification must survive that wrapper.
            let wrapped = io::Error::new(error.kind(), format!("{error} at path /clipboard/.tmp"));
            assert_eq!(error_kind(&wrapped), b'T');
        }
        for code in [libc::EACCES, libc::EROFS] {
            assert_eq!(error_kind(&io::Error::from_raw_os_error(code)), b'E');
        }
    }
    #[test]
    fn permanent_errors_do_not_inherit_retry_from_their_text() {
        assert_eq!(
            error_kind(&io::Error::new(io::ErrorKind::TimedOut, "heartbeat")),
            b'T'
        );
        assert_eq!(
            error_kind(&io::Error::new(
                io::ErrorKind::PermissionDenied,
                "SSH transport interrupted: permission"
            )),
            b'E'
        );
    }
}
