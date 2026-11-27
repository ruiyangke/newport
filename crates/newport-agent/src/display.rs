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

/// The variables that name a display to a shell, unset where `None`.
pub(crate) type Variables = Vec<(&'static str, Option<OsString>)>;

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
        let source = terminal_source(terminal, copies);
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

    /// Its handoff to the next program of its process, and the descriptors
    /// that must stay open for it.
    fn handoff(
        &self,
        terminal: &str,
        tty: Option<&Path>,
        control: std::os::fd::RawFd,
    ) -> (Handoff, Vec<std::os::fd::RawFd>) {
        let mut kept = vec![control];
        let x11 = self.x11.as_ref().map(|server| {
            let (handoff, fds) = server.handoff();
            kept.extend(fds);
            handoff
        });
        let wayland = self.wayland.as_ref().and_then(|server| {
            let (handoff, fds) = server.handoff()?;
            kept.extend(fds);
            Some(handoff)
        });
        (
            Handoff {
                terminal: terminal.to_owned(),
                tty: tty.map(Path::to_owned),
                x11,
                wayland,
                control,
            },
            kept,
        )
    }
}

/// What a terminal's clipboard serves: reads from the computer the terminal
/// is used on, chosen at each read, and copies to `copies`.
fn terminal_source(
    terminal: &str,
    copies: crate::clipboard_source::Copy,
) -> crate::clipboard_source::Source {
    use crate::routing::{Held, select_held_once};
    let terminal = uuid::Uuid::parse_str(terminal)
        .map(|id| id.to_string())
        .unwrap_or_default();
    let route: crate::clipboard_source::Route =
        std::sync::Arc::new(move || match select_held_once(&terminal, "clipboard")? {
            Held::Ready(selection) => Ok(selection.root.join("snapshot.tar")),
            Held::Waiting(mac) => Err(Held::unavailable(mac.as_deref(), "clipboard")),
        });
    crate::clipboard_source::Source::routed(route).with_copies(copies)
}

/// Copies sent to the terminal device `tty`, as programs' OSC 52.
fn copies(tty: Option<PathBuf>) -> crate::clipboard_source::Copy {
    crate::clipboard_copy::to_terminal(std::sync::Arc::new(
        tty.map(std::sync::OnceLock::from).unwrap_or_default(),
    ))
}

/// A held shell's clipboard display, handed to the next program of its
/// process: what it serves for and the servers' sockets, which the shell's
/// `DISPLAY`, `XAUTHORITY` and `WAYLAND_DISPLAY` name.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Handoff {
    terminal: String,
    tty: Option<PathBuf>,
    x11: Option<x_server::Handoff>,
    wayland: Option<wl_server::Handoff>,
    /// The holder's pipe.
    control: std::os::fd::RawFd,
}

/// How long a new build may take to show it can take a display over.
const DRY_RUN: Duration = Duration::from_secs(10);

/// Serves until the holder is gone, its pipe at an end. The holder tells it
/// on the pipe to move to the build the holder moved to, `upgrade <path>`.
fn follow(
    terminal: &str,
    tty: Option<&Path>,
    display: &TerminalDisplay,
    control: io::PipeReader,
    handoff: &Path,
) {
    // A byte at a time: what follows a line stays in the pipe for the build
    // this program may become.
    let mut line = Vec::new();
    let mut follow = crate::terminal::handoff::Follow::default();
    loop {
        let mut byte = [0u8];
        match io::Read::read(&mut &control, &mut byte) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
        if byte[0] != b'\n' {
            line.push(byte[0]);
            continue;
        }
        if let Some(build) = std::mem::take(&mut line).strip_prefix(b"upgrade ") {
            use std::os::unix::ffi::OsStrExt;
            follow.to(build, |build| {
                let build = Path::new(std::ffi::OsStr::from_bytes(build));
                move_display(terminal, tty, display, control.as_raw_fd(), build, handoff)
            });
        }
    }
}

/// Replaces this program with `build` serving on the same sockets, after a
/// dry run in which the build reads the handoff written at `path`. Returns
/// only if this program keeps serving, with whether the build refused.
/// Clients connected at that moment close with this program, and with them
/// a selection a program such as `xclip` or `wl-copy` was serving;
/// connections not yet accepted wait for the new one.
fn move_display(
    terminal: &str,
    tty: Option<&Path>,
    display: &TerminalDisplay,
    control: std::os::fd::RawFd,
    build: &Path,
    path: &Path,
) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let (handoff, kept) = display.handoff(terminal, tty, control);
    let Ok(bytes) = serde_json::to_vec(&handoff) else {
        return false;
    };
    if crate::paths::private_write(path, &bytes).is_err() {
        return false;
    }
    let checked = crate::detached::bounded::status(
        std::process::Command::new(build)
            .args([
                std::ffi::OsStr::new("terminal"),
                "display-check".as_ref(),
                path.as_os_str(),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
        std::time::Instant::now() + DRY_RUN,
    );
    let argv = [
        build.as_os_str(),
        "terminal".as_ref(),
        "display-resume".as_ref(),
        path.as_os_str(),
    ]
    .map(|argument| std::ffi::CString::new(argument.as_bytes()));
    if checked.as_ref().is_ok_and(|status| status.success())
        && let [Ok(program), Ok(a), Ok(b), Ok(c)] = &argv
    {
        let pointers = [
            program.as_ptr(),
            a.as_ptr(),
            b.as_ptr(),
            c.as_ptr(),
            std::ptr::null(),
        ];
        for &fd in &kept {
            set_cloexec(fd, false);
        }
        // SAFETY: the array is NUL-terminated and outlives the call, which
        // returns only if it fails; the other threads end with this program.
        unsafe { libc::execv(program.as_ptr(), pointers.as_ptr()) };
        for &fd in &kept {
            set_cloexec(fd, true);
        }
    }
    let _ = std::fs::remove_file(path);
    crate::terminal::handoff::refused(&checked)
}

fn set_cloexec(fd: std::os::fd::RawFd, on: bool) {
    // SAFETY: fcntl takes a live descriptor and scalar arguments.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        let flags = if on {
            flags | libc::FD_CLOEXEC
        } else {
            flags & !libc::FD_CLOEXEC
        };
        libc::fcntl(fd, libc::F_SETFD, flags);
    }
}

fn read_handoff(path: &Path) -> io::Result<Handoff> {
    serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid display handoff."))
}

/// `terminal display-check <handoff>`: a new build's dry run, which reads
/// the handoff a held shell's display wrote, to show it can take it over.
pub(crate) fn check(args: &[String]) -> io::Result<()> {
    use crate::terminal::handoff::refusal;
    let [path] = args else {
        return Err(refusal(io::Error::other("Invalid display arguments.")));
    };
    let handoff = read_handoff(Path::new(path))?;
    let paths = handoff
        .x11
        .iter()
        .flat_map(|x11| [x11.directory.as_path(), x11.authority.as_path()])
        .chain(
            handoff
                .wayland
                .iter()
                .map(|wayland| wayland.socket.as_path()),
        );
    for path in paths {
        std::fs::symlink_metadata(path).map_err(refusal)?;
    }
    Ok(())
}

/// `terminal display-resume <handoff>`: a held shell's display once it has
/// moved to this build, serving on the sockets the old build served.
pub(crate) fn resume(args: &[String]) -> io::Result<()> {
    use std::os::fd::{FromRawFd, RawFd};
    let [path] = args else {
        return Err(io::Error::other("Invalid display arguments."));
    };
    let read = read_handoff(Path::new(path));
    let _ = std::fs::remove_file(path);
    let handoff = read?;
    let fds: Vec<RawFd> = std::iter::once(handoff.control)
        .chain(
            handoff
                .x11
                .iter()
                .flat_map(|x11| [x11.listener, x11.directory_lease]),
        )
        .chain(
            handoff
                .wayland
                .iter()
                .flat_map(|wayland| [wayland.listener, wayland.lock]),
        )
        .collect();
    // Only descriptors this process has, each once and none of the standard
    // ones: taking another would close it.
    // SAFETY: fcntl only reads a descriptor's flags.
    if fds
        .iter()
        .any(|&fd| fd < 3 || unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0)
        || (1..fds.len()).any(|i| fds[..i].contains(&fds[i]))
    {
        return Err(io::Error::other("Invalid display handoff."));
    }
    for &fd in &fds {
        set_cloexec(fd, true);
    }
    let source = terminal_source(&handoff.terminal, copies(handoff.tty.clone()));
    // SAFETY: the old build kept these open for this one, and nothing else in
    // this process owns them.
    let control = unsafe { io::PipeReader::from_raw_fd(handoff.control) };
    let x11 = handoff.x11.and_then(|x11| {
        // SAFETY: as above.
        let (listener, lease) = unsafe {
            (
                std::os::unix::net::UnixListener::from_raw_fd(x11.listener),
                std::fs::File::from_raw_fd(x11.directory_lease),
            )
        };
        let directory = crate::owned_directory::Directory::adopt(&x11.directory, lease).ok()?;
        x_server::Server::adopt(source.clone(), x11, listener, directory).ok()
    });
    let wayland = handoff.wayland.as_ref().and_then(|wayland| {
        // SAFETY: as above.
        let (listener, lock) = unsafe {
            (
                std::os::unix::net::UnixListener::from_raw_fd(wayland.listener),
                std::fs::File::from_raw_fd(wayland.lock),
            )
        };
        wl_server::Server::adopt(source, wayland, listener, lock).ok()
    });
    let display = TerminalDisplay { x11, wayland };
    follow(
        &handoff.terminal,
        handoff.tty.as_deref(),
        &display,
        control,
        Path::new(path),
    );
    drop(display);
    Ok(())
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
    serve_from_child(terminal, crate::clipboard_copy::own_terminal(), None, None)
}

/// Starts a held shell's clipboard display in a process of its own, which
/// lives as long as this holder does, across the holder replacing its own
/// program: its threads would end with the old program. The display serves
/// its Wayland socket at `wayland` and sends copies to the shell's terminal
/// `tty`. It keeps none of the holder's descriptors, so the shell's terminal
/// and the holder's lock close with the holder. It runs the holder's build
/// for the holder's whole life, which keeps that build installed. Returns
/// the variables that name it, or `None` when no part of it started.
pub(crate) fn serve_held(
    terminal: &str,
    wayland: &Path,
    tty: PathBuf,
) -> Option<(Variables, std::os::fd::OwnedFd)> {
    let (control, holder) = io::pipe().ok()?;
    let variables = serve_from_child(terminal, Some(tty), Some(wayland.to_owned()), Some(control))?;
    Some((variables, holder.into()))
}

/// With `control`, the holder's pipe: the display keeps only it and its
/// result pipe, and follows the holder through builds (see [`follow`]).
fn serve_from_child(
    terminal: &str,
    tty: Option<PathBuf>,
    wayland: Option<PathBuf>,
    control: Option<io::PipeReader>,
) -> Option<Vec<(&'static str, Option<OsString>)>> {
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
                    let (writer, control) = match control {
                        Some(control) => match keep_only(writer, control) {
                            Some((writer, control)) => (writer, Some(control)),
                            None => libc::_exit(1),
                        },
                        None => (writer, None),
                    };
                    serve_display(terminal, tty, wayland, writer, parent, control)
                }
                libc::_exit(0)
            }
        }
        child => {
            // SAFETY: waits for the first child only, which ends at once.
            unsafe { libc::waitpid(child, std::ptr::null_mut(), 0) };
            drop((writer, control));
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
fn serve_display(
    terminal: &str,
    tty: Option<PathBuf>,
    wayland: Option<PathBuf>,
    mut names: io::PipeWriter,
    shell: u32,
    control: Option<io::PipeReader>,
) -> ! {
    // The directory the shell started in stays free to unmount.
    let _ = std::env::set_current_dir("/");
    // SAFETY: setsid, signal and dup2 change only this process's own state.
    unsafe {
        libc::setsid();
        // A client that disconnects mid-write ends its connection only, as
        // in a display that has replaced its program.
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
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
    let display = TerminalDisplay::start(terminal, wayland.as_deref(), copies(tty.clone()));
    if display.started() && names.write_all(&encode(&display.variables())).is_ok() {
        drop(names);
        // A held shell's display follows its holder, which writes the
        // handoff beside the Wayland socket.
        let handoff = wayland.as_ref().map(|socket| {
            let mut path = socket.clone().into_os_string();
            path.push(".handoff");
            PathBuf::from(path)
        });
        match (control, handoff) {
            (Some(control), Some(handoff)) => {
                follow(terminal, tty.as_deref(), &display, control, &handoff)
            }
            _ => Process::watch(shell).wait(),
        }
    }
    drop(display);
    // SAFETY: _exit ends only this child, skipping what the parent owns.
    unsafe { libc::_exit(0) }
}

/// Closes every descriptor but the standard ones, `writer`, which moves to
/// descriptor 3, and `control`, which moves to 4, and gives every signal its
/// default action, unblocked: the holder's handlers write to a socket closed
/// here. Only for a process that has started no threads.
fn keep_only(
    writer: io::PipeWriter,
    control: io::PipeReader,
) -> Option<(io::PipeWriter, io::PipeReader)> {
    use std::os::fd::{FromRawFd, IntoRawFd};
    const WRITER: libc::c_int = 3;
    const CONTROL: libc::c_int = 4;
    const KEPT: libc::c_int = CONTROL;
    let (writer, control) = (writer.into_raw_fd(), control.into_raw_fd());
    // SAFETY: fcntl, dup2 and close take descriptors this process owns;
    // nothing else runs while they change.
    unsafe {
        // Both above the targets first, so moving one can't overwrite the other.
        let writer_above = libc::fcntl(writer, libc::F_DUPFD, 10);
        let control_above = libc::fcntl(control, libc::F_DUPFD, 10);
        if writer_above < 0
            || control_above < 0
            || libc::dup2(writer_above, WRITER) < 0
            || libc::dup2(control_above, CONTROL) < 0
        {
            return None;
        }
        // Kernels before 5.9 have no close_range.
        #[cfg(target_os = "linux")]
        let closed = libc::syscall(libc::SYS_close_range, KEPT + 1, u32::MAX, 0) == 0;
        #[cfg(not(target_os = "linux"))]
        let closed = false;
        if !closed {
            for other in KEPT + 1..libc::getdtablesize() {
                libc::close(other);
            }
        }
        for signal in 1..65 {
            libc::signal(signal, libc::SIG_DFL);
        }
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
        Some((
            io::PipeWriter::from_raw_fd(WRITER),
            io::PipeReader::from_raw_fd(CONTROL),
        ))
    }
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
