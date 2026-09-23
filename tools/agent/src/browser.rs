//! Browser commands and the agent-side request broker.
use crate::{paths::root, wire};
use std::{
    env, fs,
    io::{self, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    process::Command,
    time::{Duration, Instant},
};
pub fn open(args: &[String], desktop: bool) -> io::Result<()> {
    if args.len() != 1 {
        return Err(io::Error::other("usage: porthop-agent open URL"));
    }
    let root = root()?;
    let wayland = env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let fake_wayland = wayland == root.join("wayland.sock").to_string_lossy()
        || wayland.starts_with("/tmp/porthop-wl-");
    let fake_x = fs::read_to_string(root.join("display"))
        .ok()
        .is_some_and(|v| env::var("DISPLAY").ok().as_deref() == Some(v.trim()));
    let graphical = (env::var_os("DISPLAY").is_some() && !fake_x)
        || (!wayland.is_empty() && !fake_wayland)
        || env::var_os("WAYLAND_SOCKET").is_some();
    if desktop && graphical && env::var("PORTHOP_OPEN_ON_MAC").as_deref() != Ok("1") {
        if let Some(native) = crate::native::find("xdg-open") {
            use std::os::unix::process::CommandExt;
            return Err(Command::new(native).args(args).env_remove("BROWSER").exec());
        }
        return Err(io::Error::other(
            "desktop opener unavailable; use porthop-agent open URL for your Mac",
        ));
    }
    if wire::web_url(&args[0]).is_none() {
        return Err(io::Error::other("only HTTP and HTTPS URLs are supported"));
    }
    let mut stream = UnixStream::connect(root.join("agent.sock"))
        .map_err(|_| io::Error::other("enable Browser in Porthop’s Integration page first"))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(25)))?;
    stream.write_all(args[0].as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    stream.take(256).read_to_string(&mut reply)?;
    if let Some(warning) = reply.strip_prefix("ok\n") {
        eprintln!("Porthop: {warning}");
    } else if reply != "ok" {
        return Err(io::Error::other(if reply.is_empty() {
            "browser connection closed"
        } else {
            &reply
        }));
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
    pub(crate) fn expire(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, _, started)| started.elapsed() > Duration::from_secs(20))
        {
            if let Some((_, mut stream, _)) = self.pending.take() {
                let _ = stream.write_all(b"Mac browser setup timed out; try again");
            }
        }
    }
    pub(crate) fn reply(&mut self, data: &[u8]) {
        if let Some((id, reply)) = std::str::from_utf8(data)
            .ok()
            .and_then(|v| v.split_once('\n'))
        {
            if self
                .pending
                .as_ref()
                .is_some_and(|(expected, _, _)| id.parse::<u64>().ok() == Some(*expected))
            {
                if let Some((_, mut stream, _)) = self.pending.take() {
                    let _ = stream.write_all(reply.as_bytes());
                }
            }
        }
    }
    pub(crate) fn poll(&mut self, output: &mut impl Write) -> io::Result<()> {
        // Private local IPC, never a network listener. One bounded request per turn.
        if let Ok((mut stream, _)) = self.listener.accept() {
            stream.set_read_timeout(Some(Duration::from_millis(100)))?;
            stream.set_write_timeout(Some(Duration::from_millis(100)))?;
            let mut request = String::new();
            let read = (&mut stream).take(8225).read_to_string(&mut request);
            if self.enabled
                && read.is_ok()
                && wire::web_url(&request).is_some()
                && self.pending.is_none()
                && self.last_open.elapsed() >= Duration::from_secs(1)
            {
                self.request_id += 1;
                wire::write(
                    output,
                    b'O',
                    format!("{}\n{request}", self.request_id).as_bytes(),
                )?;
                self.last_open = Instant::now();
                self.pending = Some((self.request_id, stream, Instant::now()));
            } else {
                let _ = stream.write_all(
                    b"browser request rejected or another request is pending; try again",
                );
            }
        }
        Ok(())
    }
}
