//! Browser commands and the agent-side request broker.
use crate::wire;
use std::{
    io::{self, Write},
    os::unix::net::{UnixListener, UnixStream},
    process::Command,
    time::{Duration, Instant},
};
pub fn open(args: &[String], desktop: bool) -> io::Result<()> {
    if args.len() != 1 {
        return Err(io::Error::other("usage: newport-agent open URL"));
    }
    let selection = crate::routing::select(Some("browser"));
    let graphical = crate::native::real_desktop(selection.as_ref().ok().map(|s| s.root.as_path()));
    if desktop && graphical {
        if let Some(native) = crate::native::find("xdg-open") {
            use std::os::unix::process::CommandExt;
            return Err(Command::new(native).args(args).env_remove("BROWSER").exec());
        }
        // Display variables may outlive a desktop (for example in tmux), or
        // describe a forwarded display on a headless server. Without a native
        // opener, use the same permission-checked HTTP(S) route as `open`.
    }
    if wire::web_url(&args[0]).is_none() {
        return Err(io::Error::other("only HTTP and HTTPS URLs are supported"));
    }
    let root = selection?.root;
    let mut stream = UnixStream::connect(root.join("agent.sock"))
        .map_err(|_| io::Error::other("enable Browser in Newport’s Integration page first"))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(25)))?;
    wire::write(&mut stream, b'O', &wire::browser_request(0, &args[0])?)?;
    let (kind, bytes) = wire::read_limited(&mut stream, 8224)?;
    let (id, success, message) = wire::parse_browser_reply(&bytes)?;
    if kind != b'B' || id != 0 {
        return Err(io::Error::other("invalid browser response"));
    }
    if !success {
        return Err(io::Error::other(
            message.unwrap_or_else(|| "browser request failed".into()),
        ));
    }
    if let Some(warning) = message {
        eprintln!("Newport: {warning}");
    }
    Ok(())
}

/// Owns pending local browser requests without blocking clipboard/heartbeat processing.
pub(crate) struct Broker {
    listener: UnixListener,
    enabled: bool,
    last_open: Instant,
    request_id: u64,
    pending: Option<(u64, UnixStream, Instant)>,
}
impl Broker {
    pub(crate) fn new(listener: UnixListener, enabled: bool) -> Self {
        Self {
            listener,
            enabled,
            last_open: Instant::now() - Duration::from_secs(2),
            request_id: 0,
            pending: None,
        }
    }
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            if let Some((_, mut stream, _)) = self.pending.take() {
                let _ = local_reply(&mut stream, false, Some("Browser access was disabled."));
            }
        }
    }
    pub(crate) fn expire(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, _, started)| started.elapsed() > Duration::from_secs(20))
        {
            if let Some((_, mut stream, _)) = self.pending.take() {
                let _ = local_reply(
                    &mut stream,
                    false,
                    Some("Mac browser setup timed out; try again"),
                );
            }
        }
    }
    pub(crate) fn reply(&mut self, data: &[u8]) {
        if let Ok((id, success, message)) = wire::parse_browser_reply(data) {
            if self
                .pending
                .as_ref()
                .is_some_and(|(expected, _, _)| id == *expected)
            {
                if let Some((_, mut stream, _)) = self.pending.take() {
                    let _ = local_reply(&mut stream, success, message.as_deref());
                }
            }
        }
    }
    pub(crate) fn poll(&mut self, output: &mut impl Write) -> io::Result<()> {
        // Private local IPC, never a network listener. One bounded request per turn.
        if let Ok((mut stream, _)) = self.listener.accept() {
            stream.set_read_timeout(Some(Duration::from_millis(100)))?;
            stream.set_write_timeout(Some(Duration::from_millis(100)))?;
            let request = wire::read_limited(&mut stream, 8224).and_then(|(kind, bytes)| {
                if kind != b'O' {
                    return Err(io::Error::other("invalid browser request"));
                }
                let (id, url) = wire::parse_browser_request(&bytes)?;
                if id != 0 {
                    return Err(io::Error::other("invalid local browser request ID"));
                }
                Ok(url)
            });
            if let Some(request) = request.ok().filter(|_| {
                self.enabled
                    && self.pending.is_none()
                    && self.last_open.elapsed() >= Duration::from_secs(1)
            }) {
                self.request_id += 1;
                wire::write(
                    output,
                    b'O',
                    &wire::browser_request(self.request_id, &request)?,
                )?;
                self.last_open = Instant::now();
                self.pending = Some((self.request_id, stream, Instant::now()));
            } else {
                let _ = local_reply(
                    &mut stream,
                    false,
                    Some("browser request rejected or another request is pending; try again"),
                );
            }
        }
        Ok(())
    }
}

fn local_reply(stream: &mut UnixStream, success: bool, message: Option<&str>) -> io::Result<()> {
    wire::write(stream, b'B', &wire::browser_reply(0, success, message)?)
}
