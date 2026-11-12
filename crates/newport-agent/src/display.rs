//! Standalone X11 or Wayland display launcher, and a terminal's clipboard
//! display.
use crate::{wl_server, x_server};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    X11,
    Wayland,
}

/// Run the display subcommand, returning the wrapped process's exit status.
pub fn run(args: impl Iterator<Item = std::ffi::OsString>) -> io::Result<i32> {
    let mut backend = Backend::X11;
    let mut args = args;
    let mut display = None;
    let mut service = false;
    let mut socket = None;
    let mut command = Vec::new();
    let mut snapshot = None;
    let mut terminal = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--service") => service = true,
            Some("--terminal") => {
                terminal = Some(
                    args.next()
                        .and_then(|id| id.into_string().ok())
                        .ok_or_else(|| io::Error::other("--terminal needs a terminal id"))?,
                );
            }
            Some("--socket") => {
                socket = Some(
                    args.next()
                        .map(PathBuf::from)
                        .ok_or_else(|| io::Error::other("--socket needs a path"))?,
                );
            }
            Some("--snapshot") => {
                snapshot = Some(
                    args.next()
                        .map(PathBuf::from)
                        .ok_or_else(|| io::Error::other("--snapshot needs a path"))?,
                )
            }
            Some("--backend") => {
                backend = match args.next().as_deref().and_then(|arg| arg.to_str()) {
                    Some("x11") => Backend::X11,
                    Some("wayland") => Backend::Wayland,
                    _ => return Err(io::Error::other("--backend must be x11 or wayland")),
                };
            }
            Some("--display") => {
                display = Some(
                    args.next()
                        .and_then(|v| v.to_str().and_then(|s| s.parse().ok()))
                        .filter(|n| *n > 0)
                        .ok_or_else(|| {
                            io::Error::other("--display needs a positive display number")
                        })?,
                )
            }
            Some("--") => {
                command.extend(args);
                break;
            }
            Some("--help") => {
                println!(
                    "Usage: newport-agent display [--backend x11|wayland] [--snapshot PATH] [--display NUMBER] [--service [--socket PATH]] [-- COMMAND ...]\n       newport-agent display --terminal ID -- COMMAND ...\n\nStarts a clipboard-only display. The default backend is x11.\n--service starts Wayland at the fixed socket; omit it when Newport already manages the agent.\n--terminal runs COMMAND, a Newport terminal's shell, with the terminal's X11 and Wayland clipboard display in place of this process."
                );
                return Ok(0);
            }
            _ => return Err(io::Error::other("unknown argument; use --help")),
        }
    }
    if let Some(terminal) = terminal {
        if command.is_empty() || service || display.is_some() || snapshot.is_some() {
            return Err(io::Error::other("--terminal takes only the command to run"));
        }
        return Err(run_terminal(&terminal, &command));
    }
    let snapshot = match snapshot {
        Some(snapshot) => snapshot,
        None => crate::paths::root()?.join("snapshot.tar"),
    };
    if backend == Backend::Wayland && display.is_some() {
        return Err(io::Error::other(
            "--display is only supported by the x11 backend",
        ));
    }
    if service && (backend != Backend::Wayland || !command.is_empty()) {
        return Err(io::Error::other(
            "--service requires Wayland and cannot launch a command",
        ));
    }
    if socket.is_some() && !service {
        return Err(io::Error::other("--socket requires --service"));
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    ctrlc::set_handler(move || {
        flag.store(true, Ordering::Relaxed);
    })
    .map_err(io::Error::other)?;
    let bridge = if backend == Backend::Wayland {
        Bridge::Wayland(if service {
            let socket = socket.unwrap_or(crate::paths::root()?.join("wayland.sock"));
            wl_server::Server::start_fixed(&snapshot, &socket)?
        } else {
            wl_server::Server::start(&snapshot)?
        })
    } else {
        Bridge::X11(x_server::Server::start(&snapshot, display)?)
    };
    let (variables, removed) = bridge.environment();
    if command.is_empty() {
        for (key, value) in &variables {
            println!("export {key}='{}'", value.replace('\'', "'\\''"));
        }
        println!("unset {}", removed.join(" "));
        while !stopped.load(Ordering::Relaxed) {
            if let Bridge::Wayland(server) = &bridge
                && !server.is_running()
            {
                return Err(io::Error::other("Wayland service stopped unexpectedly"));
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(0)
    } else {
        let mut launcher = Command::new(&command[0]);
        launcher.args(&command[1..]).envs(variables);
        for variable in removed {
            launcher.env_remove(variable);
        }
        let mut child = launcher.spawn()?;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status.code().unwrap_or(1));
            }
            if stopped.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(130);
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

enum Bridge {
    X11(x_server::Server),
    Wayland(wl_server::Server),
}
type Environment = (Vec<(&'static str, String)>, Vec<&'static str>);
impl Bridge {
    fn environment(&self) -> Environment {
        match self {
            Self::X11(server) => (
                vec![
                    ("DISPLAY", server.display.clone()),
                    ("XAUTHORITY", server.authority().display().to_string()),
                ],
                vec!["WAYLAND_DISPLAY", "WAYLAND_SOCKET"],
            ),
            Self::Wayland(server) => (
                vec![("WAYLAND_DISPLAY", server.socket().display().to_string())],
                vec!["DISPLAY", "XAUTHORITY", "WAYLAND_SOCKET"],
            ),
        }
    }
}

/// A Newport terminal's clipboard display: X11 and Wayland, each where it
/// can start. Each read goes to the computer the terminal is used on, chosen
/// then; programs' copies go to `copies`.
pub(crate) struct TerminalDisplay {
    x11: Option<x_server::Server>,
    wayland: Option<wl_server::Server>,
}

impl TerminalDisplay {
    /// Starts it, with its Wayland socket at `wayland`, or a private one.
    pub(crate) fn start(
        terminal: &str,
        wayland: Option<&Path>,
        copies: crate::clipboard_source::Copy,
    ) -> Self {
        use crate::routing::{Held, select_held_once};
        let terminal = uuid::Uuid::parse_str(terminal)
            .map(|id| id.to_string())
            .unwrap_or_default();
        let route: crate::clipboard_source::Route =
            std::sync::Arc::new(move || match select_held_once(&terminal, "clipboard")? {
                Held::Ready(selection) => Ok(selection.root.join("snapshot.tar")),
                Held::Waiting(mac) => Err(Held::unavailable(mac.as_deref(), "clipboard")),
            });
        let source = crate::clipboard_source::Source::routed(route).with_copies(copies);
        let x11 = x_server::Server::start_source(source.clone(), None).ok();
        let wayland = match wayland {
            Some(socket) => wl_server::Server::start_source(source, socket),
            None => wl_server::Server::start_private(source),
        }
        .ok();
        Self { x11, wayland }
    }

    /// The variables that name it, unset for the parts that didn't start.
    pub(crate) fn variables(&self) -> Vec<(&'static str, Option<OsString>)> {
        let x11 = self.x11.as_ref();
        vec![
            ("DISPLAY", x11.map(|server| server.display.clone().into())),
            ("XAUTHORITY", x11.map(|server| server.authority().into())),
            (
                "WAYLAND_DISPLAY",
                self.wayland.as_ref().map(|server| server.socket().into()),
            ),
            ("WAYLAND_SOCKET", None),
        ]
    }

    fn started(&self) -> bool {
        self.x11.is_some() || self.wayland.is_some()
    }
}

/// How long a terminal's shell waits for its display before it starts
/// without one.
const DISPLAY_START: Duration = Duration::from_secs(2);

/// Replaces this process with `command`, a Newport terminal's shell, naming
/// the terminal's clipboard display. The display serves from a child, so
/// the shell keeps this process: the terminal's job control, signals and
/// exit status are its own. Whatever fails, the shell runs, without the
/// display if need be. Returns only if the shell can't run.
fn run_terminal(terminal: &str, command: &[OsString]) -> io::Error {
    let mut shell = Command::new(&command[0]);
    shell.args(&command[1..]);
    for (name, value) in serve_terminal(terminal).unwrap_or_default() {
        match value {
            Some(value) => shell.env(name, value),
            None => shell.env_remove(name),
        };
    }
    shell.exec()
}

/// Starts the terminal's display in a child that lives as long as this
/// process, and returns the variables that name it.
fn serve_terminal(terminal: &str) -> Option<Vec<(&'static str, Option<OsString>)>> {
    let tty = crate::clipboard_copy::own_terminal();
    let (mut reader, writer) = io::pipe().ok()?;
    let parent = std::process::id();
    // SAFETY: this process has started no threads, so the children may run
    // anything. The first only starts the display's and ends at once, so the
    // shell, which some shells' job control expects to reap only its own
    // jobs, never has a child it doesn't know of.
    match unsafe { libc::fork() } {
        -1 => None,
        0 => {
            drop(reader);
            // SAFETY: as above; the display's child never returns.
            unsafe {
                if libc::fork() == 0 {
                    serve_display(terminal, tty, writer, parent)
                }
                libc::_exit(0)
            }
        }
        child => {
            // SAFETY: waits for the first child only, which ends at once.
            unsafe { libc::waitpid(child, std::ptr::null_mut(), 0) };
            drop(writer);
            let mut poll = libc::pollfd {
                fd: reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // The child writes the names whole, then closes the pipe.
            // SAFETY: one initialized pollfd on the live reader.
            let ready = unsafe { libc::poll(&mut poll, 1, DISPLAY_START.as_millis() as i32) };
            if ready != 1 {
                return None;
            }
            let mut names = Vec::new();
            reader.read_to_end(&mut names).ok()?;
            decode(&names)
        }
    }
}

/// The display's child: it leaves the terminal's session, so the terminal's
/// signals and hangup never reach it, and ends as soon as `shell` does, so
/// no copy goes to a terminal device the system has given another terminal.
fn serve_display(terminal: &str, tty: Option<PathBuf>, mut names: io::PipeWriter, shell: u32) -> ! {
    let shell = Process::watch(shell);
    // The directory the shell started in stays free to unmount.
    let _ = std::env::set_current_dir("/");
    // SAFETY: setsid and dup2 change only this process's own state.
    unsafe {
        libc::setsid();
        // Holding no descriptor of the terminal lets it close with the shell.
        if let Ok(null) = std::fs::File::options()
            .read(true)
            .write(true)
            .open("/dev/null")
        {
            for fd in 0..3 {
                libc::dup2(null.as_raw_fd(), fd);
            }
        }
    }
    let copies = crate::clipboard_copy::to_terminal(std::sync::Arc::new(
        tty.map(std::sync::OnceLock::from).unwrap_or_default(),
    ));
    let display = TerminalDisplay::start(terminal, None, copies);
    if display.started() && names.write_all(&encode(&display.variables())).is_ok() {
        drop(names);
        shell.wait();
    }
    drop(display);
    // SAFETY: _exit ends only this child, skipping what the parent owns.
    unsafe { libc::_exit(0) }
}

/// A process the display waits on, which isn't its parent.
struct Process {
    pid: u32,
    /// Its pidfd where the system has them, which can't name another
    /// process that later takes its pid.
    #[cfg(target_os = "linux")]
    pidfd: Option<std::os::fd::OwnedFd>,
}
impl Process {
    fn watch(pid: u32) -> Self {
        Self {
            pid,
            #[cfg(target_os = "linux")]
            // SAFETY: pidfd_open takes a pid and no flags, and returns a new
            // descriptor this process then owns, or -1.
            pidfd: match unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) } {
                // SAFETY: a non-negative result is a new descriptor nothing else owns.
                fd if fd >= 0 => Some(unsafe {
                    <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd as i32)
                }),
                _ => None,
            },
        }
    }
    /// Returns once the process has ended.
    fn wait(&self) {
        #[cfg(target_os = "linux")]
        if let Some(pidfd) = &self.pidfd {
            let mut poll = libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one initialized pollfd on a live descriptor.
            while unsafe { libc::poll(&mut poll, 1, -1) } < 0
                && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
            {}
            return;
        }
        // SAFETY: signal 0 only checks that the process exists.
        while unsafe { libc::kill(self.pid as libc::pid_t, 0) } == 0 {
            thread::sleep(PROCESS_CHECK);
        }
    }
}
/// How often a process is checked where the system has no pidfd.
const PROCESS_CHECK: Duration = Duration::from_millis(100);

/// Each variable as its name, then `=` and its value or `-` for unset, each
/// part ended by a NUL.
fn encode(variables: &[(&'static str, Option<OsString>)]) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    let mut bytes = Vec::new();
    for (name, value) in variables {
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        match value {
            Some(value) => {
                bytes.push(b'=');
                bytes.extend_from_slice(value.as_bytes());
            }
            None => bytes.push(b'-'),
        }
        bytes.push(0);
    }
    bytes
}

fn decode(bytes: &[u8]) -> Option<Vec<(&'static str, Option<OsString>)>> {
    use std::os::unix::ffi::OsStrExt;
    const NAMES: [&str; 4] = ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET"];
    let mut parts = bytes.split(|byte| *byte == 0);
    let mut variables = Vec::new();
    while let Some(name) = parts.next().filter(|name| !name.is_empty()) {
        let name = NAMES.into_iter().find(|known| known.as_bytes() == name)?;
        let value = match parts.next()?.split_first()? {
            (b'=', value) => Some(std::ffi::OsStr::from_bytes(value).to_owned()),
            (b'-', []) => None,
            _ => return None,
        };
        variables.push((name, value));
    }
    (!variables.is_empty()).then_some(variables)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_gets_the_names_the_display_sent_or_none() {
        let names = vec![
            ("DISPLAY", Some(OsString::from(":12"))),
            ("XAUTHORITY", Some(OsString::from("/tmp/x/auth=1"))),
            ("WAYLAND_DISPLAY", None),
            ("WAYLAND_SOCKET", None),
        ];
        assert_eq!(decode(&encode(&names)), Some(names.clone()));
        // A display that sent nothing, or a cut or foreign message, names
        // nothing: the shell starts as it is.
        let whole = encode(&names);
        for broken in [
            &b""[..],
            &whole[..whole.len() - 3],
            b"PATH\0=/tmp\0",
            b"DISPLAY\0:12\0",
        ] {
            assert_eq!(decode(broken), None, "{broken:?}");
        }
    }
}
