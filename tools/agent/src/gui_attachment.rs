//! Validate an inherited attachment without acquiring a display or replaying a
//! command. Current is a point-in-time observation, not an ownership receipt.
use crate::{gui_protocol::attachment as protocol, routing, wire};
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            net::UnixStream,
        },
    },
    path::Path,
    time::{Duration, Instant},
};
use uuid::Uuid;

const FIELDS: [&str; 12] = [
    "NEWPORT_SESSION",
    "NEWPORT_TERMINAL",
    "NEWPORT_TERMINAL_EPOCH",
    "DISPLAY",
    "XAUTHORITY",
    "NEWPORT_GUI_DISPLAY",
    "NEWPORT_GUI_AUTHORITY",
    "WAYLAND_DISPLAY",
    "WAYLAND_SOCKET",
    "NEWPORT_GUI_WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "NEWPORT_GUI_RUNTIME_DIR",
];
const GUIDANCE: &str = "Open or reconnect a current Newport terminal before running this command. The inherited GUI attachment was not replaced.";

/// A partial or empty marker is still an inherited claim, not permission to
/// acquire another display. Presence does not establish validity or ownership.
pub(crate) fn inherited() -> bool {
    [
        "NEWPORT_GUI_DISPLAY",
        "NEWPORT_GUI_AUTHORITY",
        "NEWPORT_GUI_WAYLAND_DISPLAY",
        "NEWPORT_GUI_RUNTIME_DIR",
    ]
    .iter()
    .any(|name| env::var_os(name).is_some())
}

struct Claim(BTreeMap<&'static str, Option<OsString>>);
impl Claim {
    fn capture(mut get: impl FnMut(&str) -> Option<OsString>) -> Self {
        Self(FIELDS.into_iter().map(|name| (name, get(name))).collect())
    }
    fn value(&self, name: &str) -> io::Result<Option<&str>> {
        self.0[name]
            .as_ref()
            .map(|value| {
                value
                    .to_str()
                    .ok_or_else(|| io::Error::other(format!("Invalid inherited {name}")))
            })
            .transpose()
    }
    fn required(&self, name: &str) -> io::Result<&str> {
        self.value(name)?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| io::Error::other(format!("Missing inherited {name}")))
    }
    fn identity(&self, name: &str) -> io::Result<Uuid> {
        Uuid::parse_str(self.required(name)?)
            .ok()
            .filter(|id| !id.is_nil())
            .ok_or_else(|| io::Error::other(format!("Invalid inherited {name}")))
    }
    fn request(&self) -> io::Result<protocol::Check> {
        if self.0["WAYLAND_SOCKET"].is_some() {
            return Err(io::Error::other(
                "An inherited Wayland fd is not a shared GUI pathname attachment",
            ));
        }
        let display = self.value("DISPLAY")?;
        let authority = self.value("XAUTHORITY")?;
        if (display.is_some() || authority.is_some())
            && (display.is_none()
                || authority.is_none()
                || display != self.value("NEWPORT_GUI_DISPLAY")?
                || authority != self.value("NEWPORT_GUI_AUTHORITY")?)
        {
            return Err(io::Error::other(
                "Inherited X11 endpoints do not match the GUI attachment markers",
            ));
        }
        let wayland = self.required("WAYLAND_DISPLAY")?;
        if Some(wayland) != self.value("NEWPORT_GUI_WAYLAND_DISPLAY")? {
            return Err(io::Error::other(
                "Inherited Wayland endpoint does not match the GUI attachment marker",
            ));
        }
        let wayland_socket = if Path::new(wayland).is_absolute() {
            wayland.to_owned()
        } else {
            let runtime = self.required("XDG_RUNTIME_DIR")?;
            if Some(runtime) != self.value("NEWPORT_GUI_RUNTIME_DIR")? {
                return Err(io::Error::other(
                    "Relative Wayland endpoint has no matching GUI runtime marker",
                ));
            }
            Path::new(runtime)
                .join(wayland)
                .to_str()
                .ok_or_else(|| io::Error::other("Invalid Wayland endpoint"))?
                .to_owned()
        };
        let request = protocol::Check {
            id: Uuid::new_v4(),
            session: self.identity("NEWPORT_SESSION")?,
            terminal: self.identity("NEWPORT_TERMINAL")?,
            epoch: self.identity("NEWPORT_TERMINAL_EPOCH")?,
            endpoints: protocol::Endpoints {
                display: display.map(str::to_owned),
                authority: authority.map(str::to_owned),
                wayland_socket,
            },
        };
        request.validate()?;
        Ok(request)
    }
}

/// A peer that does not advertise this independent feature cannot validate a
/// claim, so the claim is refused. Routing/IO/permission errors also refuse.
pub(crate) fn validate() -> io::Result<routing::Selection> {
    let deadline = Instant::now() + Duration::from_secs(protocol::WAIT_SECONDS);
    // Routing may reconcile an old selector. It must not rewrite this claim.
    let claim = Claim::capture(|name| env::var_os(name));
    let result = (|| {
        let selection = routing::select(Some("gui-launch"))?;
        remaining(deadline)?;
        let features = read_features(&selection.root)?;
        remaining(deadline)?;
        if !features
            .split_whitespace()
            .any(|feature| feature == "gui-launch")
        {
            return Err(io::Error::other(
                "GUI permission ended before attachment validation",
            ));
        }
        if !features
            .split_whitespace()
            .any(|feature| feature == protocol::FEATURE)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "This Newport peer does not support GUI attachment validation",
            ));
        }
        let request = claim.request()?;
        let reply = exchange(&selection.root.join("gui.sock"), &request, deadline)?;
        match reply.accept(&request)? {
            protocol::Outcome::Current {} => {
                if selection.session.as_deref() != Some(request.session.to_string().as_str()) {
                    return Err(io::Error::other(
                        "Current attachment reply contradicts the selected integration session",
                    ));
                }
                Ok(selection)
            }
            protocol::Outcome::NotCurrent { reason } => Err(io::Error::other(format!(
                "Inherited GUI attachment is not current ({reason:?})"
            ))),
            protocol::Outcome::Error { message } => Err(io::Error::other(format!(
                "GUI attachment validation failed: {message}"
            ))),
        }
    })();
    result.map_err(|error| io::Error::new(error.kind(), format!("{error}. {GUIDANCE}")))
}

// A feature-file read failure is not proof of an old peer. Bound the second
// read and reject links/special files instead of silently downgrading.
fn read_features(root: &Path) -> io::Result<String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("features"))?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 4096
    {
        return Err(io::Error::other("Invalid GUI feature publication"));
    }
    let mut features = String::new();
    file.take(4097).read_to_string(&mut features)?;
    if features.len() > 4096 {
        return Err(io::Error::other(
            "GUI feature publication exceeds its limit",
        ));
    }
    Ok(features)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "GUI attachment validation timed out",
            )
        })
}
fn wait(socket: &impl AsRawFd, events: libc::c_short, deadline: Instant) -> io::Result<()> {
    loop {
        let left = remaining(deadline)?;
        let mut fd = libc::pollfd {
            fd: socket.as_raw_fd(),
            events,
            revents: 0,
        };
        let milliseconds = left.as_millis().saturating_add(1).min(i32::MAX as u128) as i32;
        let result = unsafe { libc::poll(&mut fd, 1, milliseconds) };
        remaining(deadline)?;
        if result > 0 {
            return Ok(());
        }
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}
struct Connection {
    socket: UnixStream,
    deadline: Instant,
}
impl Read for Connection {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            remaining(self.deadline)?;
            match self.socket.read(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait(&self.socket, libc::POLLIN, self.deadline)?
                }
                result => return result,
            }
        }
    }
}
impl Write for Connection {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            remaining(self.deadline)?;
            match self.socket.write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait(&self.socket, libc::POLLOUT, self.deadline)?
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        remaining(self.deadline).map(|_| ())
    }
}
fn exchange(
    path: &Path,
    request: &protocol::Check,
    deadline: Instant,
) -> io::Result<protocol::Checked> {
    remaining(deadline)?;
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    socket.set_nonblocking(true)?;
    match socket.connect(&SockAddr::unix(path)?) {
        Ok(()) => (),
        Err(error) if error.raw_os_error() == Some(libc::EINPROGRESS) => {
            wait(&socket, libc::POLLOUT, deadline)?;
            if let Some(error) = socket.take_error()? {
                return Err(error);
            }
        }
        // In particular, a full Unix accept backlog must never block connect.
        Err(error) => return Err(error),
    }
    let mut connection = Connection {
        socket: UnixStream::from(OwnedFd::from(socket)),
        deadline,
    };
    wire::write(
        &mut connection,
        protocol::REQUEST,
        &protocol::encode(request)?,
    )?;
    let (kind, bytes) = wire::read_limited(&mut connection, protocol::MAX_MESSAGE)?;
    remaining(deadline)?;
    if kind != protocol::REPLY {
        return Err(io::Error::other("Invalid GUI attachment reply frame"));
    }
    let reply: protocol::Checked = protocol::decode(&bytes)?;
    reply.accept(request)?;
    Ok(reply)
}

#[cfg(test)]
#[path = "gui_attachment_client_tests.rs"]
mod tests;
