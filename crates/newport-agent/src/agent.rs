//! Integration session event loop: dispatch frames, track liveness, and coordinate services.
use crate::{
    snapshot,
    transport::{DeadlineWriter, TrackedReader},
};
use newport_protocol::{
    clipboard::imports::Encoding,
    integration::{self, ClientMessage, Reply, Request, Response, ServerMessage, wire},
};
use std::{
    fs,
    io::{self, Write},
    os::fd::AsFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

fn retry(error: &io::Error) -> newport_protocol::common::Retry {
    match error.kind() {
        io::ErrorKind::WouldBlock
        | io::ErrorKind::TimedOut
        | io::ErrorKind::Interrupted
        | io::ErrorKind::StorageFull
        | io::ErrorKind::QuotaExceeded => newport_protocol::common::Retry::Yes,
        _ => newport_protocol::common::Retry::No,
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
    clipboard_access: Option<crate::clipboard_service::Access>,
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
    clipboard_access: crate::clipboard_service::Access,
) -> io::Result<()> {
    serve_inner(
        client,
        SessionOptions {
            managed: true,
            cancellation,
            permissions: Some(permissions),
            scope: Some(scope),
            clipboard_access: Some(clipboard_access),
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
    crate::transport::set_nonblocking(stdout.as_fd())?;
    let mut output = DeadlineWriter {
        inner: stdout,
        timeout: Duration::from_secs(5),
        cancellation: options.cancellation.clone(),
    };
    let result = run(client, options, &mut output);
    if let Err(error) = &result {
        let _ = wire::write(&mut output, &ServerMessage::Failed(failure(error)));
    }
    result
}
fn run(client: &str, options: SessionOptions, output: &mut impl Write) -> io::Result<()> {
    let SessionOptions {
        mut clipboard,
        browser,
        managed,
        cancellation,
        permissions,
        scope,
        clipboard_access,
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
    let mut demand = crate::clipboard_channel::Channel::new(clipboard_access)?;
    let mut exports = demand
        .export_reader()
        .map(crate::clipboard_export_relay::Relay::new);
    demand.enabled(clipboard)?;
    let mut clipboard_service = if demand.local_reads() {
        Some(crate::clipboard_source::Service::start(
            snapshot,
            demand.data(),
        )?)
    } else {
        None
    };
    if clipboard_service.is_some() {
        session.enable_local_clipboard();
        session.publish_features(browser)?;
    }
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
                wire::read_limited::<ClientMessage>(&mut input, integration::SERVER_LIMITS.message)
            } else {
                wire::read::<ClientMessage>(&mut input)
            };
            let frame = frame.map_err(io::Error::from);
            let failed = frame.is_err();
            if tx.send(frame).is_err() || failed {
                break;
            }
        }
    });
    let mut applied: Option<newport_protocol::runtime::Applied> = None;
    let mut browser = crate::browser::Broker::new(listener, browser);
    loop {
        browser.expire();
        if stopped.load(Ordering::Relaxed) || cancellation.check().is_err() {
            return Ok(());
        }
        if last_input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed()
            > Duration::from_secs(45)
        {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "agent heartbeat expired",
            ));
        }
        session.check_backends()?;
        if let Some(requests) = &permissions
            && let Ok(request) = requests.try_recv()
        {
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
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(Ok(message)) => {
                cancellation.check()?;
                match message {
                    ClientMessage::Call(call) => {
                        let mut event = None;
                        let reply = match call.request {
                            Request::Export(command) if managed && exports.is_some() => {
                                event = exports.as_mut().unwrap().control(command)?;
                                acknowledged("")
                            }
                            Request::Permissions(requested) if managed && permissions.is_none() => {
                                let current = apply_permissions(
                                    requested,
                                    &mut applied,
                                    &mut session,
                                    &mut browser,
                                    &mut demand,
                                    &mut clipboard_service,
                                )?;
                                clipboard = session.clipboard_enabled();
                                Reply::Permissions(current)
                            }
                            Request::ClipboardOffer(offer) if clipboard => {
                                if clipboard_service.is_none() {
                                    clipboard_service =
                                        Some(crate::clipboard_source::Service::start(
                                            snapshot,
                                            demand.data(),
                                        )?);
                                }
                                crate::diagnostics::event(
                                    snapshot,
                                    "clipboard_offer",
                                    &format!(
                                        "revision={} formats={}",
                                        offer.revision,
                                        offer.formats.len()
                                    ),
                                );
                                demand.update(offer)?;
                                session.source.enable_data(demand.data());
                                session.refresh_clipboard()?;
                                acknowledged("Clipboard available on demand.")
                            }
                            Request::Snapshot(data)
                                if !managed && clipboard && clipboard_service.is_none() =>
                            {
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
                                        &format!(
                                            "kind={:?} os={:?}",
                                            error.kind(),
                                            error.raw_os_error()
                                        ),
                                    );
                                    return Err(error);
                                }
                                session.refresh_clipboard()?;
                                let backend = crate::native::update(snapshot);
                                crate::diagnostics::event(
                                    snapshot,
                                    "snapshot_acknowledged",
                                    &format!("elapsed_ms={}", started.elapsed().as_millis()),
                                );
                                acknowledged(backend)
                            }
                            Request::GuiNotices if managed => {
                                session.enable_gui_notices()?;
                                session.publish_features(browser.enabled())?;
                                acknowledged("")
                            }
                            Request::Heartbeat => {
                                if let Ok(file) = fs::OpenOptions::new().write(true).open(snapshot)
                                {
                                    file.set_modified(std::time::SystemTime::now())?;
                                }
                                acknowledged("")
                            }
                            _ => return Err(io::Error::other("unsupported agent request")),
                        };
                        wire::write(
                            output,
                            &ServerMessage::Response(Response { id: call.id, reply }),
                        )?;
                        if let Some(event) = event {
                            crate::clipboard_export_relay::Relay::send(output, &event)?;
                        }
                    }
                    ClientMessage::Clipboard(reply) if clipboard || managed => {
                        // Replies already sent before revocation cannot reopen clipboard access.
                        if !clipboard {
                            continue;
                        }
                        demand.reply(&reply)?;
                        if reply.done {
                            crate::diagnostics::event(
                                snapshot,
                                "clipboard_reply",
                                &format!(
                                    "id={} revision={} sender_success={}",
                                    reply.id,
                                    reply.revision,
                                    reply.encoding != Encoding::Failed
                                ),
                            );
                        }
                    }
                    ClientMessage::Browser(reply) if browser.enabled() || managed => {
                        browser.reply(reply)
                    }
                    ClientMessage::Quit => return Ok(()),
                    _ => return Err(io::Error::other("unsupported agent message")),
                }
            }
            Ok(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Ok(Err(e)) => return Err(e),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        }
        if session.source.gui.take() {
            wire::write(output, &ServerMessage::GuiNotice)?;
        }
        demand.poll(output)?;
        if let Some(exports) = &mut exports {
            exports.poll(output)?;
        }
        browser.poll(output)?;
    }
}

fn acknowledged(message: &str) -> Reply {
    Reply::Acknowledged {
        message: message.into(),
    }
}
fn failure(error: &io::Error) -> newport_protocol::common::Error {
    let mut message = error.to_string().replace('\0', "");
    if message.len() > integration::MAX_TEXT {
        let mut end = integration::MAX_TEXT;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    newport_protocol::common::Error {
        code: "integration_failed".into(),
        message,
        retry: retry(error),
    }
}

fn apply_permissions(
    requested: newport_protocol::runtime::Permissions,
    applied: &mut Option<newport_protocol::runtime::Applied>,
    session: &mut crate::session::Session,
    browser: &mut crate::browser::Broker,
    demand: &mut crate::clipboard_channel::Channel,
    clipboard_service: &mut Option<crate::clipboard_source::Service>,
) -> io::Result<newport_protocol::runtime::Applied> {
    if requested.revision == 0 {
        return Err(io::Error::other("invalid permission revision"));
    }
    if let Some(previous) = applied
        && requested.revision <= previous.permissions.revision
    {
        return Ok(previous.clone());
    }
    if !requested.clipboard {
        demand.enabled(false)?;
        if !demand.local_reads() {
            *clipboard_service = None;
        }
    }
    browser.set_enabled(requested.browser);
    let error = session
        .set_clipboard(requested.clipboard)
        .err()
        .map(|error| newport_protocol::common::Error {
            code: "clipboard_unavailable".into(),
            message: error.to_string(),
            retry: newport_protocol::common::Retry::Unknown,
        });
    demand.enabled(session.clipboard_enabled())?;
    session.publish_features(browser.enabled())?;
    let current = newport_protocol::runtime::Applied {
        permissions: newport_protocol::runtime::Permissions {
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
            assert_eq!(retry(&error), newport_protocol::common::Retry::Yes);
            // tempfile attaches the failing path, preserving ErrorKind rather
            // than raw_os_error. Classification must survive that wrapper.
            let wrapped = io::Error::new(error.kind(), format!("{error} at path /clipboard/.tmp"));
            assert_eq!(retry(&wrapped), newport_protocol::common::Retry::Yes);
        }
        for code in [libc::EACCES, libc::EROFS] {
            assert_eq!(
                retry(&io::Error::from_raw_os_error(code)),
                newport_protocol::common::Retry::No
            );
        }
    }
    #[test]
    fn permanent_errors_do_not_inherit_retry_from_their_text() {
        assert_eq!(
            retry(&io::Error::new(io::ErrorKind::TimedOut, "heartbeat")),
            newport_protocol::common::Retry::Yes
        );
        assert_eq!(
            retry(&io::Error::new(
                io::ErrorKind::PermissionDenied,
                "SSH transport interrupted: permission"
            )),
            newport_protocol::common::Retry::No
        );
    }
}
