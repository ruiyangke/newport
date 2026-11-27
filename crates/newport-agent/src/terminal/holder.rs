//! A terminal's holder: owns the PTY and the shell, keeps a replica of the
//! tabs' terminal, serves every computer attached to it and writes the shell's
//! record. The terminal worker starts it detached, so it outlives the
//! connection that started it.
use super::Files;
use bytes::Bytes;
use newport_protocol::encoding::protobuf::FrameDecoder;
use newport_protocol::terminal::{
    Computer, Exit,
    holder::{self as protocol, Attach, ClientMessage, ServerMessage, wire},
};
use newport_terminal::Replica;
use newport_terminal::RgbColor;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::{CString, OsString},
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::{OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Output queued for a client beyond this means the client isn't reading;
/// it is dropped and gets a snapshot when it attaches again.
const CLIENT_QUEUE: usize = 8 << 20;
const MAX_CLIENTS: usize = 64;
const ATTACH_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a shell that exited while no client was attached keeps its
/// holder, so the next attach can show how it ended.
const EXIT_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
/// How long Close waits after hanging up the shell before killing it.
const END_GRACE: Duration = Duration::from_secs(2);
/// How often the holder checks its socket, children and deadlines.
const TICK: Duration = Duration::from_secs(1);
/// How long a new build may take to show it can take over a shell.
const DRY_RUN: Duration = Duration::from_secs(10);
/// How long the shell's output pauses before the holder moves, so no escape
/// sequence is cut between two builds: programs write one in a single burst,
/// and a progress display that redraws several times a second still pauses
/// this long between redraws.
const QUIET: Duration = Duration::from_millis(40);
/// How long a holder waits for that pause, as under a flood of output,
/// before it tries again later.
const QUIET_WAIT: Duration = Duration::from_secs(30);
/// How long computers told the holder is moving get to hang up, their last
/// input reaching the shell first.
const LEAVING: Duration = Duration::from_secs(1);
/// How often the loop wakes while a move is under way.
const MOVING_TICK: Duration = Duration::from_millis(25);
/// How long a holder waits to try a build again after a handover that
/// couldn't happen, rather than one the build refused; it doubles with each
/// one in a row, up to `UPGRADE_RETRY_MAX`.
const UPGRADE_RETRY: Duration = Duration::from_secs(60);
const UPGRADE_RETRY_MAX: Duration = Duration::from_secs(60 * 60);
/// How often a holder that moved tells its reaper and clipboard display
/// again, should one have been unable to follow it.
const REMIND: Duration = Duration::from_secs(10 * 60);
/// How long a holder that moved keeps the computer that was active for it
/// to attach again, still active.
const RETURN: Duration = Duration::from_secs(10);

enum Handover {
    /// The new build's dry run said it can't take over: not tried again.
    Refused,
    /// The handover couldn't happen now, as with a full disk, a slow dry run,
    /// output that never paused or a signal that came meanwhile.
    Failed,
    /// The shell exited or is ending: there is nothing to move.
    Abandoned,
}

/// A holder's moves to new builds of Needle.
#[derive(Default)]
struct Upgrade {
    /// The move under way: its dry run, then the wait for a pause.
    moving: Option<Moving>,
    /// A build that refused to take over in its dry run, not tried again.
    declined: Option<PathBuf>,
    /// The build this holder moved to, never moved to again should the
    /// entry point name it by another path.
    arrived: Option<PathBuf>,
    /// Until when the build that couldn't be moved to, for another reason,
    /// isn't tried, and how many times in a row it couldn't. Another build
    /// is tried at once.
    retry: Option<(PathBuf, Instant)>,
    failures: u32,
    /// When a holder that moved next tells its children to follow.
    remind_at: Option<Instant>,
    /// The computer that was active when the holder moved, and until when it
    /// is active again as soon as it attaches.
    returning: Option<(Option<String>, Instant)>,
}

struct Moving {
    build: PathBuf,
    /// The dry run's process, and its wait status once reaped.
    check: libc::pid_t,
    status: Option<libc::c_int>,
    started: Instant,
    /// When the dry run passed.
    passed: Option<Instant>,
}

/// `terminal holder <id> <columns> <rows>`, with the shell's command in the
/// id's start file.
pub(crate) fn run(args: &[String]) -> io::Result<()> {
    let [id, columns, rows] = args else {
        return Err(io::Error::other("Invalid terminal holder arguments."));
    };
    let id = super::valid_id(id)?;
    let size = parse_size(columns, rows)?;
    crate::detached::detach()?;
    // Errors past this point have no one to report to; the worker sees the
    // socket never appear.
    let _ = serve(&id, size);
    crate::linger::reconcile();
    Ok(())
}

/// `terminal holder-check <id> <handoff>`: a new build's dry run. It reads
/// the handoff the running holder wrote and rebuilds its replica, with none
/// of its descriptors, to show it can take the shell over.
pub(crate) fn check(args: &[String]) -> io::Result<()> {
    let [id, path] = args else {
        return Err(io::Error::other("Invalid terminal holder arguments."));
    };
    super::valid_id(id).map_err(super::handoff::refusal)?;
    let (handoff, snapshot) = super::handoff::read(std::path::Path::new(path))?;
    let (columns, rows) = handoff.size;
    Replica::resume(columns, rows, &snapshot, handoff.prompted)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid terminal snapshot."))?;
    Ok(())
}

/// `terminal holder-resume <id> <handoff>`: the holder once it has replaced
/// its own program with this build, picking the shell up where the old build
/// left it. The descriptors the handoff names were kept open for it.
pub(crate) fn resume(args: &[String]) -> io::Result<()> {
    // First: SIGTERM and SIGINT, blocked across the exec, then reach the
    // loop as they would have the old build's. Without the handlers they keep
    // their default action, which ends the holder as before it had any.
    let signals = handle_signals().or_else(|_| {
        let (read, write) = UnixStream::pair()?;
        std::mem::forget(write);
        Ok::<_, io::Error>(read)
    });
    // SAFETY: the set is valid for the calls.
    unsafe {
        let mut ending: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut ending);
        libc::sigaddset(&mut ending, libc::SIGTERM);
        libc::sigaddset(&mut ending, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &ending, std::ptr::null_mut());
    }
    let signals = signals?;
    let [id, path] = args else {
        return Err(io::Error::other("Invalid terminal holder arguments."));
    };
    let id = super::valid_id(id)?;
    let read = super::handoff::read(std::path::Path::new(path));
    let _ = fs::remove_file(path);
    let (handoff, snapshot) = read?;
    let fds: Vec<RawFd> = [handoff.master, handoff.listener, handoff.lock]
        .into_iter()
        .chain(handoff.status)
        .chain(handoff.reaper)
        .chain(handoff.display)
        .collect();
    // Only descriptors this process has, each once and none of the standard
    // ones: taking another would close it.
    // SAFETY: fcntl only reads a descriptor's flags.
    if fds
        .iter()
        .any(|&fd| fd < 3 || unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0)
        || (1..fds.len()).any(|i| fds[..i].contains(&fds[i]))
    {
        // The shell ends with this process; so do its terminal's files.
        Files {
            directory: handoff.directory,
            id,
        }
        .remove();
        return Err(io::Error::other("Invalid terminal handoff."));
    }
    // SAFETY: the old build kept these open for this one, and nothing else in
    // this process owns them.
    let (master, status, listener, lock, reaper, display) = unsafe {
        (
            OwnedFd::from_raw_fd(handoff.master),
            handoff.status.map(|fd| OwnedFd::from_raw_fd(fd)),
            UnixListener::from_raw_fd(handoff.listener),
            File::from_raw_fd(handoff.lock),
            handoff.reaper.map(|fd| OwnedFd::from_raw_fd(fd)),
            handoff.display.map(|fd| OwnedFd::from_raw_fd(fd)),
        )
    };
    for &fd in &fds {
        cloexec(fd);
    }
    let (columns, rows) = handoff.size;
    // From here the shell is this build's: nothing ends it. A screen that
    // can't be rebuilt starts empty, and the loop writes the record again.
    let replica = Replica::resume(columns, rows, &snapshot, handoff.prompted)
        .or_else(|_| Replica::new(columns, rows))
        .map_err(|_| io::Error::other("Invalid terminal size."))?;
    let mut holder = Holder {
        files: Files {
            directory: handoff.directory,
            id,
        },
        creator: handoff.creator,
        started: handoff.started,
        name: handoff.name,
        listener,
        signals,
        shell: Shell {
            master,
            pid: handoff.shell,
            status,
            reaper,
        },
        replica,
        size: handoff.size,
        clients: Vec::new(),
        pty_open: true,
        exited: None,
        ending: None,
        // The record, with this same pid, stays until the computers that were
        // attached are again, or `RETURN` is up.
        stale: false,
        recorded_title: None,
        recorded_agent: None,
        reporter: handoff.reporter,
        program: handoff.program,
        lock,
        display,
        last_output: Instant::now(),
        upgrade: Upgrade {
            arrived: Some(handoff.build),
            remind_at: Some(Instant::now() + REMIND),
            returning: Some((handoff.active, Instant::now() + RETURN)),
            ..Upgrade::default()
        },
    };
    become_subreaper();
    holder.lead_children();
    let result = holder.run();
    holder.files.remove();
    drop(holder);
    crate::linger::reconcile();
    result
}

fn parse_size(columns: &str, rows: &str) -> io::Result<(u16, u16)> {
    let parse = |v: &str| v.parse::<u16>().ok().filter(|v| *v > 0);
    match (parse(columns), parse(rows)) {
        (Some(c), Some(r)) => Ok((c, r)),
        _ => Err(io::Error::other("Invalid terminal size.")),
    }
}

fn cloexec(fd: RawFd) {
    // SAFETY: fcntl takes scalar arguments; a bad descriptor only fails.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
    }
}

/// Keeps `fd` open across `exec`.
fn inherit(fd: RawFd) {
    // SAFETY: fcntl takes a live descriptor and scalar arguments.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
    }
}

fn nonblocking(fd: RawFd) {
    // SAFETY: fcntl takes scalar arguments; a bad descriptor only fails.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

fn winsize(columns: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

struct Shell {
    master: OwnedFd,
    pid: libc::pid_t,
    /// The reaper's pipe, which brings the shell's wait status, until it has.
    status: Option<OwnedFd>,
    /// The pipe that tells the reaper to move to a new build.
    reaper: Option<OwnedFd>,
}

use super::display::Change;
use super::reaper;

/// How long the reaper may take to start the shell, its scope included.
const REAPER_START: Duration = Duration::from_secs(5);

/// This process's environment with `changes` made, for the shell. Built
/// here rather than set on the holder, whose display threads read its own.
fn shell_environment(changes: &[Change]) -> Vec<CString> {
    let mut variables: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    for (name, value) in changes {
        match value {
            Some(value) => variables.insert(name.into(), value.clone()),
            None => variables.remove(&OsString::from(name)),
        };
    }
    variables
        .into_iter()
        .filter_map(|(name, value)| {
            let mut entry = name.into_vec();
            entry.push(b'=');
            entry.extend(value.into_vec());
            CString::new(entry).ok()
        })
        .collect()
}

/// A pipe whose ends close on exec.
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // SAFETY: `fds` has room for both ends.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for fd in fds {
        cloexec(fd);
    }
    // SAFETY: both ends came from pipe and nothing else owns them.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// A copy of `fd` above the descriptors a child is given, so moving one into
/// place can't overwrite another.
fn above_handed_over(fd: &OwnedFd) -> io::Result<OwnedFd> {
    // SAFETY: fcntl takes a live descriptor and scalar arguments.
    let copy = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the copy is new and nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(copy) })
}

/// Reads one value the reaper sent, waiting until `deadline`.
fn receive(fd: &OwnedFd, deadline: Instant) -> Option<libc::c_int> {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let left = deadline.checked_duration_since(Instant::now())?;
    // SAFETY: the pointer and count describe `poll`.
    if unsafe { libc::poll(&mut poll, 1, left.as_millis() as _) } <= 0 {
        return None;
    }
    let mut bytes = [0u8; 4];
    // SAFETY: the pointer and length describe `bytes`.
    let read = unsafe { libc::read(fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
    // A pipe moves a write this short whole.
    (read == bytes.len() as isize).then(|| libc::c_int::from_ne_bytes(bytes))
}

/// Opens the PTY and starts the terminal's reaper, which starts `sh -c
/// command` as the PTY's session leader, in `directory` when it can, with
/// `changes` made to its environment. Returns once the reaper has said the
/// shell's pid.
/// The shell's PTY, its controlling side first, at `size`.
fn open_pty((columns, rows): (u16, u16)) -> io::Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut slave) = (-1, -1);
    let size = winsize(columns, rows);
    // SAFETY: both descriptor slots and the size are valid for the call, and
    // the name and termios pointers are null.
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &size as *const _ as *mut _,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both came from openpty and nothing else owns them.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    cloexec(master.as_raw_fd());
    cloexec(slave.as_raw_fd());
    Ok((master, slave))
}

/// The shell's terminal device, where its clipboard display sends copies.
fn terminal_name(master: &OwnedFd) -> io::Result<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: ptsname reads the descriptor's state into a buffer that
    // nothing else in the holder uses; the name is copied out at once.
    let name = unsafe { libc::ptsname(master.as_raw_fd()) };
    if name.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ptsname returned a non-null NUL-terminated string.
    let name = unsafe { std::ffi::CStr::from_ptr(name) };
    Ok(std::ffi::OsStr::from_bytes(name.to_bytes()).into())
}

fn spawn(
    id: &str,
    command: &str,
    directory: Option<&str>,
    (master, slave): (OwnedFd, OwnedFd),
    changes: &[Change],
) -> io::Result<Shell> {
    let executable = crate::paths::own_program()?;
    let executable = CString::new(executable.into_os_string().into_vec())
        .map_err(|_| io::Error::other("Invalid executable path."))?;
    let arguments = [
        executable.clone(),
        CString::new("terminal").unwrap(),
        CString::new("reaper").unwrap(),
        CString::new(id).map_err(|_| io::Error::other("Invalid terminal id."))?,
        // Empty for none: a folder to start in is always absolute.
        CString::new(directory.unwrap_or_default())
            .map_err(|_| io::Error::other("Invalid terminal folder."))?,
        CString::new(command).map_err(|_| io::Error::other("Invalid shell command."))?,
    ];
    let mut argv: Vec<*const libc::c_char> = arguments.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    // The PTY's own terminal type, as sshd's pty-req sets it.
    let mut changes = changes.to_vec();
    changes.push(("TERM", Some("xterm-256color".into())));
    // Built before the fork: the child only execs.
    let environment = shell_environment(&changes);
    let mut envp: Vec<*const libc::c_char> = environment.iter().map(|e| e.as_ptr()).collect();
    envp.push(std::ptr::null());
    let terminal = above_handed_over(&slave)?;
    drop(slave);
    let (status, sender) = pipe()?;
    let sender = above_handed_over(&sender)?;
    let (control, reaper) = pipe()?;
    let control = above_handed_over(&control)?;
    let null: OwnedFd = File::options()
        .read(true)
        .write(true)
        .open("/dev/null")?
        .into();
    // No handler runs between the fork and the exec.
    // SAFETY: both sets are valid for the calls.
    let mut blocked: libc::sigset_t = unsafe { std::mem::zeroed() };
    let mut before: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigfillset(&mut blocked);
        libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut before);
    }
    // SAFETY: the child only calls async-signal-safe functions on memory
    // prepared before the fork, then execs or exits.
    let reaper_pid = unsafe { libc::fork() };
    if reaper_pid == 0 {
        // SAFETY: as above, on descriptors this process owns.
        unsafe {
            reaper::default_signals();
            for fd in 0..3 {
                libc::dup2(null.as_raw_fd(), fd);
            }
            libc::dup2(terminal.as_raw_fd(), reaper::TERMINAL_FD);
            libc::dup2(sender.as_raw_fd(), reaper::STATUS_FD);
            libc::dup2(control.as_raw_fd(), reaper::CONTROL_FD);
            // Every other descriptor closes on exec already; this also
            // closes one a library opened without saying so.
            #[cfg(target_os = "linux")]
            libc::syscall(libc::SYS_close_range, reaper::CONTROL_FD + 1, u32::MAX, 0);
            libc::execve(executable.as_ptr(), argv.as_ptr(), envp.as_ptr());
            libc::_exit(127)
        }
    }
    let error = io::Error::last_os_error();
    // SAFETY: `before` is the mask this thread had.
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &before, std::ptr::null_mut()) };
    if reaper_pid < 0 {
        return Err(error);
    }
    // The reaper and the shell have their own copies.
    drop((terminal, sender, control, null));
    let Some(pid) = receive(&status, Instant::now() + REAPER_START).filter(|pid| *pid > 0) else {
        // SAFETY: the reaper is this holder's child, and is collected here.
        unsafe {
            libc::kill(reaper_pid, libc::SIGKILL);
            libc::waitpid(reaper_pid, std::ptr::null_mut(), 0);
        }
        return Err(io::Error::other("The shell did not start."));
    };
    nonblocking(master.as_raw_fd());
    nonblocking(status.as_raw_fd());
    Ok(Shell {
        master,
        pid,
        status: Some(status),
        reaper: Some(reaper),
    })
}

/// Only if the reaper is killed: the shell and what the reaper adopted then
/// come back to the holder, which keeps the shell's exit status, rather than
/// going to a PID 1 that may not reap them.
fn become_subreaper() {
    #[cfg(target_os = "linux")]
    // SAFETY: prctl with this option takes scalar arguments.
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }
}

/// SIGTERM and SIGINT as a byte on the returned socket, which the loop
/// polls to end the shell as Close does. The holder ignores the job control
/// signals, SIGQUIT and the user signals, which would otherwise end it.
fn handle_signals() -> io::Result<UnixStream> {
    for signal in [
        libc::SIGQUIT,
        libc::SIGTSTP,
        libc::SIGTTIN,
        libc::SIGTTOU,
        libc::SIGUSR1,
        libc::SIGUSR2,
    ] {
        // SAFETY: signal takes scalar arguments.
        unsafe { libc::signal(signal, libc::SIG_IGN) };
    }
    let (read, write) = UnixStream::pair()?;
    read.set_nonblocking(true)?;
    write.set_nonblocking(true)?;
    for signal in [libc::SIGTERM, libc::SIGINT] {
        signal_hook::low_level::pipe::register(signal, write.try_clone()?)?;
    }
    Ok(read)
}

/// Whether the connecting process belongs to this account.
fn same_account(stream: &UnixStream) -> bool {
    #[cfg(target_os = "linux")]
    // SAFETY: the credential buffer and its length describe a valid ucred.
    unsafe {
        let mut cred: libc::ucred = std::mem::zeroed();
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut length,
        ) == 0
            && cred.uid == libc::geteuid()
    }
    #[cfg(not(target_os = "linux"))]
    // SAFETY: both out-pointers are valid for the call.
    unsafe {
        let (mut uid, mut gid) = (0, 0);
        libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) == 0 && uid == libc::geteuid()
    }
}

/// What the worker hands a new holder in its start file.
#[derive(Deserialize, Serialize)]
pub(crate) struct Start {
    pub command: String,
    pub creator: Computer,
    /// The account's stable display will show on the creating computer.
    pub stable_display: bool,
    /// The folder the shell starts in, when it can.
    pub directory: Option<String>,
}

struct Queued {
    bytes: Vec<u8>,
    offset: usize,
    snapshot: bool,
}

struct Client {
    stream: UnixStream,
    decoder: FrameDecoder,
    queue: VecDeque<Queued>,
    queued_bytes: usize,
    snapshots: usize,
    /// Its hello, once it has said it.
    hello: Option<Attach>,
    /// Its terminal's size, as last told.
    size: Option<(u16, u16)>,
    /// When it said hello.
    attached_at: Instant,
    /// When someone last used the terminal on its computer: an `Activate`, or a
    /// hello saying the tab was opened. Output a tab sends by itself (focus
    /// and mouse motion reports, query replies), resizes and reattaching
    /// don't count, so a computer nobody is using never takes the shell.
    used: Option<Instant>,
    /// Whether it was last told it is the active one.
    told_active: Option<bool>,
    /// The shell size it was last told; none until it is attached and has
    /// its first screen.
    told_size: Option<(u16, u16)>,
    /// The shell's exit has been queued for it.
    told_exit: bool,
    /// Control connections accept one mutation and close after its outcome.
    mutation: bool,
    ending: bool,
    read_closed: bool,
    close_when_flushed: bool,
}

impl Client {
    fn admit(clients: &mut Vec<Self>, stream: UnixStream, terminal_id: &str) {
        if clients.len() < MAX_CLIENTS && same_account(&stream) {
            clients.push(Self::new(stream, terminal_id));
        }
    }
    fn new(stream: UnixStream, terminal_id: &str) -> Self {
        nonblocking(stream.as_raw_fd());
        let mut client = Client {
            stream,
            decoder: FrameDecoder::new(protocol::CLIENT_LIMITS.message),
            queue: VecDeque::new(),
            queued_bytes: 0,
            snapshots: 0,
            hello: None,
            size: None,
            attached_at: Instant::now(),
            used: None,
            told_active: None,
            told_size: None,
            told_exit: false,
            mutation: false,
            ending: false,
            read_closed: false,
            close_when_flushed: false,
        };
        let _ = client.send(&ServerMessage::Hello(protocol::Hello {
            version: protocol::VERSION,
            terminal_id: terminal_id.into(),
        }));
        client
    }
    /// Attached for output: said hello, and not a control connection.
    fn attached(&self) -> bool {
        self.hello.as_ref().is_some_and(|hello| !hello.control)
    }
    /// Orders clients by when their computer was last used, then by when they
    /// attached: until someone uses one, the latest to attach leads.
    fn recency(&self) -> (Option<Instant>, Instant) {
        (self.used, self.attached_at)
    }
    fn mac(&self) -> Option<Computer> {
        let hello = self.hello.as_ref().filter(|hello| !hello.control)?;
        hello.computer.clone()
    }
    /// Queues a frame; false when the client has fallen too far behind.
    fn send(&mut self, message: &ServerMessage) -> bool {
        let payload = match message {
            ServerMessage::Screen(bytes)
            | ServerMessage::Full(bytes)
            | ServerMessage::Output(bytes) => bytes.len(),
            _ => 0,
        };
        if self.queue.len() >= 256 || self.queued_bytes.saturating_add(payload) > CLIENT_QUEUE {
            return false;
        }
        let Ok(frame) = wire::encode_server(message) else {
            return false;
        };
        if self.queued_bytes.saturating_add(frame.len()) > CLIENT_QUEUE {
            return false;
        }
        self.queued_bytes += frame.len();
        self.queue.push_back(Queued {
            bytes: frame,
            offset: 0,
            snapshot: false,
        });
        true
    }
    fn can_snapshot(&self) -> bool {
        self.snapshots == 0 && self.queue.len() <= 254
    }
    fn snapshot(&mut self, screen: Bytes, full: Bytes) -> bool {
        if !self.can_snapshot() {
            return false;
        }
        // One snapshot batch may exceed the ordinary output budget. Its two
        // bounded frames retain their allocation accounting until popped.
        let Ok(screen) = wire::encode_server(&ServerMessage::Screen(screen)) else {
            return false;
        };
        let Ok(full) = wire::encode_server(&ServerMessage::Full(full)) else {
            return false;
        };
        self.queue.push_back(Queued {
            bytes: screen,
            offset: 0,
            snapshot: true,
        });
        self.queue.push_back(Queued {
            bytes: full,
            offset: 0,
            snapshot: true,
        });
        self.snapshots = 2;
        true
    }
    /// Writes what the socket takes now; false when the client is gone.
    fn flush(&mut self) -> bool {
        while let Some(front) = self.queue.front_mut() {
            match self.stream.write(&front.bytes[front.offset..]) {
                Ok(0) => return false,
                Ok(count) => {
                    front.offset += count;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return true,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
            if front.offset == front.bytes.len()
                && let Some(frame) = self.queue.pop_front()
            {
                if frame.snapshot {
                    self.snapshots -= 1;
                } else {
                    self.queued_bytes -= frame.bytes.len();
                }
            }
        }
        true
    }
}

fn compress(data: &[u8]) -> io::Result<Vec<u8>> {
    use flate2::{Compression, write::DeflateEncoder};
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(data)?;
    encoder.finish()
}

fn write_all_pty(fd: RawFd, mut bytes: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !bytes.is_empty() && Instant::now() < deadline {
        // SAFETY: the pointer and length describe the live `bytes` slice.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n > 0 {
            bytes = &bytes[n as usize..];
            continue;
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => {
                let mut pfd = libc::pollfd {
                    fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: one initialized pollfd.
                unsafe { libc::poll(&mut pfd, 1, 100) };
            }
            _ => return,
        }
    }
}

/// Whether the process group is stopped, as by Ctrl-Z: any of its
/// processes, since a pipeline's leader may have exited before it.
#[cfg(target_os = "linux")]
fn stopped(group: libc::pid_t) -> bool {
    // The state and group follow the command name, which may hold any
    // character: `pid (comm) state ppid pgrp ...`.
    let stopped_member = |stat: &str| {
        let Some((_, rest)) = stat.rsplit_once(')') else {
            return false;
        };
        let mut fields = rest.split_whitespace();
        let state = fields.next().unwrap_or_default();
        let pgrp = fields
            .nth(1)
            .and_then(|pgrp| pgrp.parse::<libc::pid_t>().ok());
        pgrp == Some(group) && matches!(state, "T" | "t")
    };
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{group}/stat")) {
        return stopped_member(&stat);
    }
    std::fs::read_dir("/proc").is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name.bytes().all(|b| b.is_ascii_digit())
                    && std::fs::read_to_string(format!("/proc/{name}/stat"))
                        .is_ok_and(|stat| stopped_member(&stat))
            })
        })
    })
}

/// Without `/proc`, a group with any process left counts as stopped.
#[cfg(not(target_os = "linux"))]
fn stopped(group: libc::pid_t) -> bool {
    // SAFETY: signal 0 sends nothing; it only asks whether the group exists.
    unsafe { libc::kill(-group, 0) == 0 }
}

fn exit_of(status: libc::c_int) -> Exit {
    if libc::WIFSIGNALED(status) {
        Exit::Signal(libc::WTERMSIG(status) as u32)
    } else {
        Exit::Code(libc::WEXITSTATUS(status) as u32)
    }
}

fn rgb([r, g, b]: [u8; 3]) -> RgbColor {
    RgbColor { r, g, b }
}

struct Holder {
    files: Files,
    creator: Computer,
    started: u64,
    /// The name someone gave the shell, if any.
    name: Option<String>,
    listener: UnixListener,
    /// A byte for each SIGTERM or SIGINT.
    signals: UnixStream,
    shell: Shell,
    replica: Replica,
    size: (u16, u16),
    clients: Vec<Client>,
    /// The shell's PTY still has a reader on the other side.
    pty_open: bool,
    exited: Option<(Exit, Instant)>,
    ending: Option<Instant>,
    /// The record no longer says what is true.
    stale: bool,
    /// The title the record carries, for tabs not attached yet.
    recorded_title: Option<String>,
    /// What the shell's coding agent reported, as the record carries it.
    recorded_agent: Option<String>,
    /// The process group in the shell's foreground when its programs last
    /// reported their status. Their status lasts only as long as it does.
    reporter: Option<libc::pid_t>,
    /// The program group the shell last gave the terminal to, until the
    /// shell has it back and the program isn't stopped.
    program: Option<libc::pid_t>,
    /// Held for the holder's life, across builds: the terminal's one holder.
    lock: File,
    /// The pipe that tells the shell's clipboard display to follow this
    /// holder to a new build; it ends the display when the holder ends.
    display: Option<OwnedFd>,
    /// When the shell last wrote output.
    last_output: Instant,
    upgrade: Upgrade,
}

fn serve(id: &str, size: (u16, u16)) -> io::Result<()> {
    let files = Files::create(id)?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(files.lock())?;
    // SAFETY: flock takes a live descriptor and a scalar operation.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::other("This terminal already has a holder."));
    }
    cloexec(lock.as_raw_fd());
    let start_file = files.start();
    let start: Start = serde_json::from_slice(&fs::read(&start_file)?)
        .map_err(|_| io::Error::other("Invalid terminal start file."))?;
    let _ = fs::remove_file(&start_file);

    let _ = fs::remove_file(files.socket());
    let listener = UnixListener::bind(files.socket())?;
    fs::set_permissions(files.socket(), fs::Permissions::from_mode(0o600))?;
    cloexec(listener.as_raw_fd());
    nonblocking(listener.as_raw_fd());

    // Before any thread starts, as signal handlers should be.
    let signals = handle_signals()?;
    become_subreaper();
    let (master, slave) = open_pty(size)?;
    // The account's stable display, running or expected, only when the
    // creating computer may show it: windows and clipboard on the computer
    // showing it. Otherwise a clipboard display of its own, even while
    // another computer shows the stable display, so the shell's clipboard
    // stays its own computer's: the server's clipboard programs and those
    // that read X11 or Wayland themselves, as Codex does through arboard,
    // paste from the computer someone last used the shell on, and their
    // copies go to the shell's terminal, which sends them to the active
    // computer. The shell keeps its `DISPLAY` and `WAYLAND_DISPLAY` for its
    // whole life; graphical apps get their computer's display at launch.
    let display = start
        .stable_display
        .then(|| super::display::running().or_else(super::display::expected))
        .flatten();
    let (environment, display) = match display {
        Some(environment) => (environment, None),
        None => match crate::display::serve_held(
            files.id(),
            &files.clipboard(),
            terminal_name(&master)?,
        ) {
            Some((environment, control)) => (environment, Some(control)),
            None => (
                ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET"]
                    .map(|name| (name, None))
                    .to_vec(),
                None,
            ),
        },
    };
    let shell = spawn(
        id,
        &start.command,
        start.directory.as_deref(),
        (master, slave),
        &environment,
    )?;
    let mut holder = Holder {
        files,
        creator: start.creator,
        started: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        name: None,
        listener,
        signals,
        shell,
        replica: Replica::new(size.0, size.1).map_err(|_| io::Error::other("Invalid size"))?,
        size,
        clients: Vec::new(),
        pty_open: true,
        exited: None,
        ending: None,
        stale: false,
        recorded_title: None,
        recorded_agent: None,
        reporter: None,
        program: None,
        lock,
        display,
        last_output: Instant::now(),
        upgrade: Upgrade::default(),
    };
    holder.write_record()?;
    let result = holder.run();
    holder.files.remove();
    result
}

impl Holder {
    /// The index of the active client, which sets the size and gets what
    /// goes to one computer.
    fn active(&self) -> Option<usize> {
        self.clients
            .iter()
            .enumerate()
            .filter(|(_, client)| client.attached())
            .max_by_key(|(_, client)| client.recency())
            .map(|(index, _)| index)
    }

    fn write_record(&mut self) -> io::Result<()> {
        // Most recently used first: Needle's routing tries computers in this order.
        let mut clients: Vec<_> = self.clients.iter().filter(|c| c.attached()).collect();
        clients.sort_by_key(|client| std::cmp::Reverse(client.recency()));
        let attached: Vec<_> = clients
            .iter()
            .enumerate()
            .filter_map(|(index, client)| {
                let mac = client.mac()?;
                Some(serde_json::json!({
                    "id": mac.id,
                    "name": mac.name,
                    "active": index == 0,
                }))
            })
            .collect();
        let exited = self.exited.map(|(exit, _)| match exit {
            Exit::Code(code) => serde_json::json!({ "code": code }),
            Exit::Signal(signal) => serde_json::json!({ "signal": signal }),
        });
        let record = serde_json::json!({
            "version": protocol::VERSION,
            "pid": std::process::id(),
            "columns": self.size.0,
            "rows": self.size.1,
            "started": self.started,
            "name": self.name,
            "title": self.replica.title(),
            "agent": self.replica.agent_report(),
            "creator": self.creator,
            "attached": attached,
            "exited": exited,
        });
        self.recorded_title = self.replica.title().map(str::to_owned);
        self.recorded_agent = self.replica.agent_report();
        crate::paths::private_write(&self.files.record(), record.to_string().as_bytes())
    }

    fn run(&mut self) -> io::Result<()> {
        let mut buffer = vec![0u8; 64 * 1024];
        let mut last_tick = Instant::now();
        loop {
            let master = self.shell.master.as_raw_fd();
            let mut fds: Vec<libc::pollfd> = [self.listener.as_raw_fd(), self.signals.as_raw_fd()]
                .into_iter()
                .chain(self.shell.status.as_ref().map(|status| status.as_raw_fd()))
                .map(|fd| libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();
            if self.pty_open {
                fds.push(libc::pollfd {
                    fd: master,
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
            for client in &self.clients {
                let mut events = if client.read_closed { 0 } else { libc::POLLIN };
                if !client.queue.is_empty() {
                    events |= libc::POLLOUT;
                }
                fds.push(libc::pollfd {
                    fd: client.stream.as_raw_fd(),
                    events,
                    revents: 0,
                });
            }
            let wait = if self.upgrade.moving.is_some() {
                MOVING_TICK
            } else {
                TICK
            };
            // SAFETY: the pointer and count describe the live `fds` vector.
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, wait.as_millis() as _) };

            let ready = |fd: RawFd| fds.iter().any(|p| p.fd == fd && p.revents != 0);
            if self.pty_open && ready(master) {
                self.read_pty(&mut buffer);
            }
            if ready(self.listener.as_raw_fd()) {
                self.accept();
            }
            if ready(self.signals.as_raw_fd()) {
                // Each byte is one signal; any of them ends the shell.
                while self.signals.read(&mut buffer).is_ok_and(|n| n > 0) {}
                self.end();
            }
            if let Some(status) = &self.shell.status
                && ready(status.as_raw_fd())
            {
                self.read_status();
            }
            let waiting: Vec<RawFd> = self
                .clients
                .iter()
                .map(|client| client.stream.as_raw_fd())
                .filter(|&fd| ready(fd))
                .collect();
            for fd in waiting {
                self.serve_client(fd, &mut buffer);
            }
            if last_tick.elapsed() >= TICK {
                last_tick = Instant::now();
                self.tick();
            }
            self.reap();
            self.advance_upgrade();
            // Before the exit: nothing follows it to a client.
            self.tell_active();
            if self.exited.is_some() {
                self.report_exit();
            }
            if std::mem::take(&mut self.stale) {
                let _ = self.write_record();
            }
            if self.finished() {
                return Ok(());
            }
        }
    }

    /// Feeds output to the replica and every attached client. The holder
    /// answers the program's terminal queries itself: with several computers
    /// attached, each answering would give the program several replies.
    fn read_pty(&mut self, buffer: &mut [u8]) {
        // Before the shell's output: a prompt that turns modes on keeps them.
        self.take_back_input();
        let master = self.shell.master.as_raw_fd();
        // SAFETY: the pointer and length describe the live `buffer` slice.
        let n = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), buffer.len()) };
        if n <= 0 {
            let error = io::Error::last_os_error();
            if n == 0
                || !matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                )
            {
                // EIO: every process with the PTY open is gone.
                self.pty_open = false;
            }
            return;
        }
        self.last_output = Instant::now();
        let bytes = &buffer[..n as usize];
        if let Ok(effects) = self.replica.advance(bytes) {
            for reply in effects.replies {
                write_all_pty(master, &reply);
            }
            if effects.program_statuses {
                // SAFETY: tcgetpgrp takes a live descriptor.
                let foreground = unsafe { libc::tcgetpgrp(master) };
                self.reporter =
                    (self.replica.agent_report().is_some() && foreground > 0).then_some(foreground);
            }
        }
        self.broadcast(&ServerMessage::Output(Bytes::copy_from_slice(bytes)));
    }

    /// Follows who holds the terminal. When the shell has it back from a
    /// program that isn't stopped, as when the program exited or was killed,
    /// turns off the input reporting the program left on, for the replica
    /// and every client: a killed program can't turn it off itself, and the
    /// shell would read each mouse move and key report as typed text, with
    /// Ctrl-C no longer interrupting. A stopped program keeps its modes for
    /// `fg`. Runs before the next output is read, so what the shell turns on
    /// at its prompt comes after. The shell is the one the holder started: a
    /// shell started in it holds the terminal as one of its programs. A
    /// program is seen holding the terminal when its output is read; one that
    /// gives it back before then is left as the shell turning modes on.
    fn take_back_input(&mut self) {
        // SAFETY: tcgetpgrp and getpgid take scalar arguments.
        let (foreground, shell) = unsafe {
            (
                libc::tcgetpgrp(self.shell.master.as_raw_fd()),
                libc::getpgid(self.shell.pid),
            )
        };
        if foreground <= 0 || shell <= 0 {
            return;
        }
        if foreground != shell {
            self.program = Some(foreground);
            return;
        }
        if self.program.is_none_or(stopped) {
            return;
        }
        // `fg` gives a stopped job the terminal before it continues: seen
        // running, it may hold the terminal again by now.
        // SAFETY: tcgetpgrp takes a live descriptor.
        if unsafe { libc::tcgetpgrp(self.shell.master.as_raw_fd()) } != shell {
            return;
        }
        self.program = None;
        if let Some(release) = self.replica.release_input() {
            self.broadcast(&ServerMessage::Output(Bytes::from_static(release)));
        }
    }

    fn broadcast(&mut self, message: &ServerMessage) {
        let before = self.clients.len();
        self.clients
            .retain_mut(|client| !client.attached() || client.send(message));
        if self.clients.len() != before {
            self.left();
        }
    }

    fn accept(&mut self) {
        let Ok((stream, _)) = self.listener.accept() else {
            return;
        };
        Client::admit(&mut self.clients, stream, self.files.id());
    }

    fn position(&self, fd: RawFd) -> Option<usize> {
        self.clients
            .iter()
            .position(|client| client.stream.as_raw_fd() == fd)
    }

    fn drop_client(&mut self, index: usize) {
        let client = self.clients.remove(index);
        if client.attached() {
            self.left();
        }
    }

    /// Tells each attached client whether it is the active one, when that
    /// isn't what it was last told.
    fn tell_active(&mut self) {
        let active = self.active();
        let before = self.clients.len();
        let mut index = 0;
        self.clients.retain_mut(|client| {
            let is_active = Some(index) == active;
            index += 1;
            if !client.attached() || client.told_exit || client.told_active == Some(is_active) {
                return true;
            }
            client.told_active = Some(is_active);
            client.send(&ServerMessage::Active(is_active))
        });
        if self.clients.len() != before {
            self.left();
        }
    }

    /// An attached client went away: another may set the size now.
    fn left(&mut self) {
        self.stale = true;
        self.apply_active();
    }

    fn serve_client(&mut self, fd: RawFd, buffer: &mut [u8]) {
        let mut closed = false;
        // Parse as bytes arrive; a continuously writing peer must not make an
        // unbounded buffer or monopolize the holder's poll loop.
        let mut budget = protocol::CLIENT_LIMITS.message;
        while budget > 0 {
            let Some(index) = self.position(fd) else {
                return;
            };
            if self.clients[index].read_closed || self.clients[index].close_when_flushed {
                break;
            }
            let limit = buffer.len().min(budget);
            let read = match self.clients[index].stream.read(&mut buffer[..limit]) {
                Ok(0) => {
                    if self.clients[index].decoder.finish().is_ok() && self.clients[index].mutation
                    {
                        self.clients[index].read_closed = true;
                    } else {
                        closed = true;
                    }
                    break;
                }
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    closed = true;
                    break;
                }
            };
            budget -= read;
            let mut input = &buffer[..read];
            while !input.is_empty() {
                let Some(index) = self.position(fd) else {
                    return;
                };
                let message = match self.clients[index].decoder.consume(&mut input) {
                    Ok(Some(frame)) => match wire::decode_client(frame.body()) {
                        Ok(message) => message,
                        Err(_) => {
                            closed = true;
                            break;
                        }
                    },
                    Ok(None) => break,
                    Err(_) => {
                        closed = true;
                        break;
                    }
                };
                if !self.handle(index, message) {
                    closed = true;
                    break;
                }
            }
            if closed {
                break;
            }
        }
        if let Some(index) = self.position(fd)
            && (closed
                || !self.clients[index].flush()
                || (self.clients[index].close_when_flushed && self.clients[index].queue.is_empty()))
        {
            self.drop_client(index);
        }
    }

    fn resize(&mut self, size: (u16, u16)) {
        if size != self.size && self.replica.resize(size.0, size.1).is_ok() {
            self.size = size;
            self.stale = true;
            let ws = winsize(size.0, size.1);
            // SAFETY: TIOCSWINSZ reads one winsize, which `ws` is.
            unsafe { libc::ioctl(self.shell.master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
            // Before any output the shell draws at this size.
            self.tell_size();
        }
    }

    /// Tells each attached tab the shell's size when it changed, as tmux
    /// draws a window at its one size on every client: a tab that kept its
    /// own size would read output drawn for another computer's width.
    fn tell_size(&mut self) {
        let size = self.size;
        let before = self.clients.len();
        self.clients.retain_mut(|client| {
            if !client.attached() || client.told_size.is_none_or(|told| told == size) {
                return true;
            }
            client.told_size = Some(size);
            client.send(&ServerMessage::Size {
                columns: size.0,
                rows: size.1,
            })
        });
        if self.clients.len() != before {
            self.left();
        }
    }

    /// `apply_active` for the client at `index`, which it finds again after:
    /// telling the others the new size drops any that fell behind, and moves
    /// those after them. None when it was dropped itself.
    fn apply_active_for(&mut self, index: usize) -> Option<usize> {
        let fd = self.clients[index].stream.as_raw_fd();
        self.apply_active();
        self.position(fd)
    }

    /// The active client sets the size and the query colours, as tmux's
    /// `window-size latest` does.
    fn apply_active(&mut self) {
        let Some(index) = self.active() else {
            return;
        };
        let (size, colors) = {
            let client = &self.clients[index];
            (
                client.size,
                client.hello.as_ref().and_then(|hello| hello.colors),
            )
        };
        if let Some(size) = size {
            self.resize(size);
        }
        if let Some([foreground, background, cursor]) = colors {
            let _ = self
                .replica
                .set_colors(rgb(foreground), rgb(background), rgb(cursor));
        }
    }

    /// Someone used the terminal on the computer of the client at `index`. False
    /// drops the client.
    fn used(&mut self, index: usize) -> bool {
        let was = self.active();
        self.clients[index].used = Some(Instant::now());
        // The record lists computers most recently used first.
        self.stale = true;
        if was == Some(index) {
            return true;
        }
        let size = self.size;
        let Some(index) = self.apply_active_for(index) else {
            return false;
        };
        // Its tab showed the shell at another computer's size; the shell now
        // redraws at this one's, over the screen as it is at that size.
        self.size == size || self.send_screen(index)
    }

    /// The terminal's screen and history as they are now.
    fn send_screen(&mut self, index: usize) -> bool {
        if !self.clients[index].can_snapshot() {
            return false;
        }
        let snapshot = self.replica.snapshot();
        if snapshot.screen.len() > protocol::MAX_SNAPSHOT
            || snapshot.full.len() > protocol::MAX_SNAPSHOT
        {
            return false;
        }
        let Ok(full) = compress(&snapshot.full) else {
            return false;
        };
        let client = &mut self.clients[index];
        client.snapshot(snapshot.screen.into(), full.into())
    }

    /// Handles one validated message; false drops only this client.
    fn handle(&mut self, index: usize, message: ClientMessage) -> bool {
        let greeted = self.clients[index].hello.is_some();
        match message {
            ClientMessage::Attach(hello) if !greeted => {
                let (control, opened) = (hello.control, hello.opened);
                self.clients[index].size = Some((hello.columns, hello.rows));
                self.clients[index].hello = Some(hello);
                self.clients[index].attached_at = Instant::now();
                let prompted = self.replica.prompted();
                if control {
                    return self.clients[index].send(&ServerMessage::Attached { prompted });
                }
                let returning = self
                    .upgrade
                    .returning
                    .as_ref()
                    .and_then(|(mac, _)| mac.as_ref());
                if opened
                    || returning.is_some_and(|mac| {
                        self.clients[index]
                            .mac()
                            .is_some_and(|attached| attached.id == *mac)
                    })
                {
                    self.clients[index].used = Some(Instant::now());
                }
                self.stale = true;
                let Some(index) = self.apply_active_for(index) else {
                    return false;
                };
                // The size comes before the screen, which is drawn at it.
                let size = self.size;
                self.clients[index].told_size = Some(size);
                let mut ok = self.clients[index].send(&ServerMessage::Attached { prompted })
                    && self.clients[index].send(&ServerMessage::Size {
                        columns: size.0,
                        rows: size.1,
                    })
                    && self.send_screen(index);
                if let Some((exit, _)) = self.exited {
                    let client = &mut self.clients[index];
                    client.told_exit = true;
                    ok &= client.send(&ServerMessage::Exit(exit));
                }
                ok
            }
            ClientMessage::Input(payload) if greeted && self.clients[index].attached() => {
                if self.exited.is_none() {
                    write_all_pty(self.shell.master.as_raw_fd(), &payload);
                }
                true
            }
            ClientMessage::Resize { columns, rows }
                if greeted && self.clients[index].attached() =>
            {
                self.clients[index].size = Some((columns, rows));
                self.apply_active();
                true
            }
            ClientMessage::Activate if greeted && self.clients[index].attached() => {
                self.used(index)
            }
            ClientMessage::Rename(name)
                if greeted && !self.clients[index].attached() && !self.clients[index].mutation =>
            {
                self.clients[index].mutation = true;
                let name = name.trim().to_owned();
                let old = self.name.replace(name.clone());
                if name.is_empty() {
                    self.name = None;
                }
                let response = match self.write_record() {
                    Ok(()) => {
                        self.stale = false;
                        ServerMessage::Renamed(name)
                    }
                    Err(error) => {
                        self.name = old;
                        self.stale = true;
                        ServerMessage::Failed(newport_protocol::common::Error {
                            code: "rename_failed".into(),
                            message: format!("The terminal name could not be persisted: {error}"),
                            retry: newport_protocol::common::Retry::Unknown,
                        })
                    }
                };
                self.clients[index].close_when_flushed = true;
                self.clients[index].send(&response)
            }
            ClientMessage::End if greeted && !self.clients[index].mutation => {
                if !self.clients[index].attached() {
                    self.clients[index].mutation = true;
                    self.clients[index].ending = true;
                }
                self.end();
                true
            }
            _ => false,
        }
    }

    /// Hangs up the shell: its session, and the foreground job if that is a
    /// separate group. The tick kills what is left after the grace period.
    fn end(&mut self) {
        if self.ending.is_some() || self.exited.is_some() {
            if self.exited.is_some() {
                self.ending.get_or_insert_with(Instant::now);
            }
            return;
        }
        self.ending = Some(Instant::now());
        self.signal_shell(libc::SIGHUP);
        self.signal_shell(libc::SIGCONT);
    }

    fn signal_shell(&self, signal: libc::c_int) {
        // SAFETY: tcgetpgrp takes a live descriptor.
        let foreground = unsafe { libc::tcgetpgrp(self.shell.master.as_raw_fd()) };
        // SAFETY: kill takes scalar arguments; each group is the shell's or its
        // terminal's foreground group.
        unsafe {
            libc::kill(-self.shell.pid, signal);
            if foreground > 0 && foreground != self.shell.pid {
                libc::kill(-foreground, signal);
            }
        }
    }

    /// Collects every exited child: the reaper, and once a reaper is killed,
    /// the shell and what the reaper had adopted.
    fn reap(&mut self) {
        loop {
            let mut status = 0;
            // SAFETY: `status` is a valid out-pointer.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid <= 0 {
                return;
            }
            if pid == self.shell.pid {
                self.shell_exited(status);
            } else if let Some(moving) = &mut self.upgrade.moving
                && moving.check == pid
            {
                moving.status = Some(status);
            }
        }
    }

    /// The shell's wait status from the reaper, or the pipe closing without
    /// it when the reaper is gone.
    fn read_status(&mut self) {
        let Some(status) = &self.shell.status else {
            return;
        };
        let mut bytes = [0u8; 4];
        // SAFETY: the pointer and length describe `bytes`.
        let read =
            unsafe { libc::read(status.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
        if read < 0 && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
            return;
        }
        self.shell.status = None;
        // A pipe moves a write this short whole.
        if read == bytes.len() as isize {
            self.shell_exited(libc::c_int::from_ne_bytes(bytes));
        } else {
            self.lost_reaper();
        }
    }

    /// The reaper ended without sending the shell's status. A shell still
    /// there, even as a zombie, is the holder's to reap now. One already gone
    /// was collected by the reaper, which a kill of the whole scope then
    /// stopped before it could send the status.
    fn lost_reaper(&mut self) {
        // SAFETY: signal 0 sends nothing; it only asks whether the pid exists.
        let gone = unsafe { libc::kill(self.shell.pid, 0) } == -1
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            self.shell_exited(libc::SIGKILL);
        }
    }

    fn shell_exited(&mut self, status: libc::c_int) {
        if self.exited.is_none() {
            self.exited = Some((exit_of(status), Instant::now()));
            self.stale = true;
        }
    }

    /// Clears what the shell's programs reported once the process group that
    /// reported it is gone, as when an agent is killed before it can clear
    /// its own status. The clear goes to every client as output, so their
    /// tabs agree with the record.
    fn clear_orphaned_status(&mut self) {
        const CLEAR: &[u8] = b"\x1b]7501;state=clear\x1b\\";
        let Some(reporter) = self.reporter else {
            return;
        };
        // SAFETY: signal 0 sends nothing; it only asks whether the group exists.
        let gone = unsafe { libc::kill(-reporter, 0) } == -1
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if !gone {
            return;
        }
        self.reporter = None;
        let _ = self.replica.advance(CLEAR);
        self.broadcast(&ServerMessage::Output(Bytes::from_static(CLEAR)));
    }

    fn tick(&mut self) {
        self.clear_orphaned_status();
        self.take_back_input();
        // Once a tick at most: some programs change the title many times a
        // second.
        self.stale |= self.replica.title() != self.recorded_title.as_deref()
            || self.replica.agent_report() != self.recorded_agent;
        if let Some(started) = self.ending
            && self.exited.is_none()
            && started.elapsed() >= END_GRACE
        {
            self.signal_shell(libc::SIGKILL);
        }
        self.clients.retain(|client| {
            client.hello.is_some() || client.attached_at.elapsed() < ATTACH_TIMEOUT
        });
        self.keep_socket();
        let now = Instant::now();
        if self
            .upgrade
            .returning
            .as_ref()
            .is_some_and(|(_, until)| now >= *until)
        {
            self.upgrade.returning = None;
            self.stale = true;
        }
        if self.upgrade.remind_at.is_some_and(|at| now >= at) {
            self.upgrade.remind_at = Some(now + REMIND);
            self.lead_children();
        }
        self.upgrade();
    }

    /// Tells the reaper and the clipboard display to move to the build this
    /// holder now runs, each after its own check, so a shell keeps no part of
    /// an old build. One that can't keeps its build and goes on as before.
    fn lead_children(&mut self) {
        let Ok(build) = fs::read_link("/proc/self/exe")
            .or_else(|_| std::env::current_exe())
            .and_then(fs::canonicalize)
        else {
            return;
        };
        let mut message = b"upgrade ".to_vec();
        message.extend_from_slice(build.as_os_str().as_bytes());
        message.push(b'\n');
        for control in [&self.shell.reaper, &self.display].into_iter().flatten() {
            // A child that is gone reads nothing; the pipe has room for this.
            // SAFETY: the pointer and length describe `message`.
            unsafe { libc::write(control.as_raw_fd(), message.as_ptr().cast(), message.len()) };
        }
    }

    /// Starts a move to the build Needle's entry point names when it is
    /// another, as after an update: the shell, its terminal and the socket
    /// computers attach to carry over, and attached computers attach again.
    /// Only after the build has shown in a dry run, which runs beside the
    /// loop, that it can take over; one that can't keeps this build, and
    /// isn't tried again.
    fn upgrade(&mut self) {
        if self.upgrade.moving.is_some()
            || self.exited.is_some()
            || self.ending.is_some()
            || !self.pty_open
        {
            return;
        }
        let Some(build) = crate::deployment::active_build() else {
            return;
        };
        if crate::paths::is_own_program(&build)
            || self.upgrade.arrived.as_ref() == Some(&build)
            || self.upgrade.declined.as_ref() == Some(&build)
            || self
                .upgrade
                .retry
                .as_ref()
                .is_some_and(|(failed, at)| *failed == build && Instant::now() < *at)
        {
            return;
        }
        // The dry run reads the handoff as the shell is now, with none of
        // the descriptors; the move writes it again as the shell is then.
        if self.write_handoff(&build, self.active_mac()).is_err() {
            return self.moved(Handover::Failed, &build);
        }
        use std::os::unix::process::CommandExt;
        let check = std::process::Command::new(&build)
            .args(self.arguments("holder-check"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn();
        // The loop's waits collect it.
        match check.map(|child| libc::pid_t::try_from(child.id())) {
            Ok(Ok(check)) => {
                self.upgrade.moving = Some(Moving {
                    build,
                    check,
                    status: None,
                    started: Instant::now(),
                    passed: None,
                })
            }
            _ => self.moved(Handover::Failed, &build),
        }
    }

    /// Takes a move a step further: from its dry run to a pause in the
    /// shell's output, and from there to the new build.
    fn advance_upgrade(&mut self) {
        let Some(moving) = &mut self.upgrade.moving else {
            return;
        };
        let build = moving.build.clone();
        let outcome = if self.exited.is_some() || self.ending.is_some() || !self.pty_open {
            Some(Handover::Abandoned)
        } else if let Some(passed) = moving.passed {
            if self.last_output.elapsed() >= QUIET {
                self.upgrade.moving = None;
                Some(self.hand_over(&build))
            } else {
                (passed.elapsed() >= QUIET_WAIT).then_some(Handover::Failed)
            }
        } else {
            match moving.status {
                None => (moving.started.elapsed() >= DRY_RUN).then_some(Handover::Failed),
                Some(status) if libc::WIFEXITED(status) => match libc::WEXITSTATUS(status) {
                    0 => {
                        moving.passed = Some(Instant::now());
                        None
                    }
                    super::handoff::REFUSED => Some(Handover::Refused),
                    _ => Some(Handover::Failed),
                },
                Some(_) => Some(Handover::Failed),
            }
        };
        if let Some(outcome) = outcome {
            self.moved(outcome, &build);
        }
    }

    /// Ends a move that left the shell with this build.
    fn moved(&mut self, outcome: Handover, build: &Path) {
        if let Some(moving) = self.upgrade.moving.take()
            && moving.status.is_none()
        {
            // SAFETY: kill takes scalar arguments; the group is the dry run's
            // own, and its process isn't reaped yet, so the id is still its.
            unsafe { libc::kill(-moving.check, libc::SIGKILL) };
        }
        let _ = fs::remove_file(self.files.handoff());
        match outcome {
            Handover::Refused => {
                self.upgrade.declined = Some(build.to_owned());
                self.upgrade.failures = 0;
            }
            Handover::Failed => {
                if self
                    .upgrade
                    .retry
                    .as_ref()
                    .is_none_or(|(failed, _)| failed != build)
                {
                    self.upgrade.failures = 0;
                }
                let wait = UPGRADE_RETRY
                    .saturating_mul(1 << self.upgrade.failures.min(16))
                    .min(UPGRADE_RETRY_MAX);
                self.upgrade.retry = Some((build.to_owned(), Instant::now() + wait));
                self.upgrade.failures += 1;
            }
            Handover::Abandoned => {}
        }
    }

    fn arguments(&self, command: &str) -> [OsString; 4] {
        [
            OsString::from("terminal"),
            command.into(),
            self.files.id().into(),
            self.files.handoff().into(),
        ]
    }

    /// The descriptors the new build takes over.
    fn kept(&self) -> Vec<RawFd> {
        [
            self.shell.master.as_raw_fd(),
            self.listener.as_raw_fd(),
            self.lock.as_raw_fd(),
        ]
        .into_iter()
        .chain(
            [&self.shell.status, &self.shell.reaper, &self.display]
                .into_iter()
                .flatten()
                .map(AsRawFd::as_raw_fd),
        )
        .collect()
    }

    fn active_mac(&self) -> Option<String> {
        self.active()
            .and_then(|index| self.clients[index].mac())
            .map(|mac| mac.id)
    }

    fn write_handoff(&mut self, build: &Path, active: Option<String>) -> io::Result<()> {
        let handoff = super::handoff::Handoff {
            directory: self.files.directory.clone(),
            build: build.to_owned(),
            creator: self.creator.clone(),
            started: self.started,
            name: self.name.clone(),
            size: self.size,
            shell: self.shell.pid,
            master: self.shell.master.as_raw_fd(),
            status: self.shell.status.as_ref().map(|status| status.as_raw_fd()),
            listener: self.listener.as_raw_fd(),
            lock: self.lock.as_raw_fd(),
            reaper: self.shell.reaper.as_ref().map(AsRawFd::as_raw_fd),
            display: self.display.as_ref().map(AsRawFd::as_raw_fd),
            reporter: self.reporter,
            program: self.program,
            prompted: self.replica.prompted(),
            active,
        };
        super::handoff::write(
            &self.files.handoff(),
            &handoff,
            &self.replica.snapshot().full,
        )
    }

    /// Tells attached computers the holder is moving, gives them `LEAVING` to
    /// hang up, serving what they sent before they heard, and control
    /// connections to finish their mutation, then replaces this program with
    /// `build`. Connections not accepted yet wait for the new build. Output the shell writes meanwhile waits in its
    /// terminal for the new build. Returns only if this build keeps the
    /// shell; computers told attach again to it.
    fn hand_over(&mut self, build: &Path) -> Handover {
        // Its active computer, before any of them leaves.
        let active = self.active_mac();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut told = Vec::new();
        let deadline = Instant::now() + LEAVING;
        loop {
            // Each attached computer, including one whose attach was already
            // on its way, is told once.
            let before = self.clients.len();
            self.clients.retain_mut(|client| {
                let fd = client.stream.as_raw_fd();
                if !client.attached() || told.contains(&fd) {
                    return true;
                }
                told.push(fd);
                client.send(&ServerMessage::Moving)
            });
            if self.clients.len() != before {
                self.left();
            }
            // Gone, or done with their mutation.
            while let Some(index) = self.clients.iter_mut().position(|client| {
                !client.flush() || (client.close_when_flushed && client.queue.is_empty())
            }) {
                self.drop_client(index);
            }
            let mut fds: Vec<libc::pollfd> = Vec::new();
            for client in &self.clients {
                let mut events = if client.read_closed { 0 } else { libc::POLLIN };
                if !client.queue.is_empty() {
                    events |= libc::POLLOUT;
                }
                fds.push(libc::pollfd {
                    fd: client.stream.as_raw_fd(),
                    events,
                    revents: 0,
                });
            }
            // An idle connection that never attached doesn't hold the move.
            let waiting = self
                .clients
                .iter()
                .any(|client| client.attached() || client.mutation);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if !waiting || remaining.is_zero() {
                break;
            }
            // SAFETY: the pointer and count describe the live `fds` vector.
            unsafe {
                libc::poll(
                    fds.as_mut_ptr(),
                    fds.len() as _,
                    remaining.as_millis().max(1) as _,
                )
            };
            for fd in fds.iter().filter(|fd| fd.revents != 0).map(|fd| fd.fd) {
                self.serve_client(fd, &mut buffer);
            }
        }
        self.reap();
        if self.exited.is_some() || self.ending.is_some() {
            return Handover::Abandoned;
        }
        if self.write_handoff(build, active).is_err() {
            return Handover::Failed;
        }
        let kept = self.kept();
        let Ok(argv) = std::iter::once(build.as_os_str().to_owned())
            .chain(self.arguments("holder-resume"))
            .map(|argument| CString::new(argument.into_vec()))
            .collect::<Result<Vec<_>, _>>()
        else {
            return Handover::Refused;
        };
        let mut pointers: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
        pointers.push(std::ptr::null());
        // SIGTERM and SIGINT wait, blocked across the exec, for the new
        // build's handlers; one that came during the move is handled here.
        // SAFETY: both sets are valid for the calls.
        let mut ending: libc::sigset_t = unsafe { std::mem::zeroed() };
        let mut before: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut ending);
            libc::sigaddset(&mut ending, libc::SIGTERM);
            libc::sigaddset(&mut ending, libc::SIGINT);
            libc::pthread_sigmask(libc::SIG_BLOCK, &ending, &mut before);
        }
        let mut signalled = libc::pollfd {
            fd: self.signals.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one live pollfd, and no wait.
        let signalled = unsafe { libc::poll(&mut signalled, 1, 0) } > 0;
        if !signalled {
            for &fd in &kept {
                inherit(fd);
            }
            // SAFETY: both arrays are NUL-terminated and outlive the call,
            // which returns only if it fails.
            unsafe { libc::execv(argv[0].as_ptr(), pointers.as_ptr()) };
            for &fd in &kept {
                cloexec(fd);
            }
        }
        // SAFETY: `before` is the mask this thread had.
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &before, std::ptr::null_mut()) };
        Handover::Failed
    }

    /// Sends every attached client the shell's last output and how it ended.
    fn report_exit(&mut self) {
        let Some((exit, _)) = self.exited else {
            return;
        };
        self.drain_pty();
        let before = self.clients.len();
        self.clients.retain_mut(|client| {
            if (!client.attached() && !client.ending) || client.told_exit {
                return true;
            }
            client.told_exit = true;
            client.close_when_flushed = client.ending;
            client.send(&ServerMessage::Exit(exit))
                && client.flush()
                && !(client.close_when_flushed && client.queue.is_empty())
        });
        if self.clients.len() != before {
            self.left();
        }
    }

    /// Output the shell wrote just before it exited.
    fn drain_pty(&mut self) {
        let mut buffer = vec![0u8; 64 * 1024];
        while self.pty_open {
            let master = self.shell.master.as_raw_fd();
            // SAFETY: the pointer and length describe the live `buffer` vector.
            let n = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), buffer.len()) };
            if n <= 0 {
                let error = io::Error::last_os_error();
                if n == 0 || error.kind() != io::ErrorKind::Interrupted {
                    if n == 0 || error.kind() != io::ErrorKind::WouldBlock {
                        self.pty_open = false;
                    }
                    return;
                }
                continue;
            }
            let bytes = buffer[..n as usize].to_vec();
            let _ = self.replica.advance(&bytes);
            self.broadcast(&ServerMessage::Output(bytes.into()));
        }
    }

    /// The runtime directory can vanish under a running holder, as logind
    /// removes it once the account's last session ends. Bind again where
    /// clients will look.
    fn keep_socket(&mut self) {
        if self.files.socket().exists() {
            return;
        }
        let Ok(files) =
            Files::create(self.files.id()).or_else(|_| Files::fallback(self.files.id()))
        else {
            return;
        };
        let _ = fs::remove_file(files.socket());
        let Ok(listener) = UnixListener::bind(files.socket()) else {
            return;
        };
        let _ = fs::set_permissions(files.socket(), fs::Permissions::from_mode(0o600));
        cloexec(listener.as_raw_fd());
        nonblocking(listener.as_raw_fd());
        if let Ok(lock) = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(files.lock())
        {
            // SAFETY: flock takes a live descriptor and a scalar operation.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                cloexec(lock.as_raw_fd());
                // Held for the holder's life, like the first lock, which it
                // replaces: a new build is handed this one.
                self.lock = lock;
            }
        }
        self.listener = listener;
        self.files = files;
        self.stale = true;
    }

    /// Done once the shell has exited and every attached client has seen how,
    /// or, with none attached, once it was ended or kept long enough.
    fn finished(&self) -> bool {
        let Some((_, exited_at)) = self.exited else {
            return false;
        };
        let mut attached = self
            .clients
            .iter()
            .filter(|client| client.attached() || client.ending)
            .peekable();
        if attached.peek().is_some() {
            return attached.all(|client| client.told_exit && client.queue.is_empty());
        }
        self.ending.is_some() || exited_at.elapsed() >= EXIT_RETENTION
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    #[test]
    fn one_maximum_snapshot_is_allowed_and_its_budget_survives_partial_writes() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        socket2::SockRef::from(&stream)
            .set_send_buffer_size(1024)
            .unwrap();
        let mut client = Client::new(stream, "0123456789abcdef0123456789abcdef");
        let snapshot = Bytes::from(vec![42; protocol::MAX_SNAPSHOT]);
        assert!(client.snapshot(Bytes::new(), snapshot.clone()));
        assert!(!client.snapshot(Bytes::new(), snapshot));
        let output = ServerMessage::Output(Bytes::from(vec![1; protocol::MAX_DATA]));
        let output_length = wire::encode_server(&output).unwrap().len();
        assert!(client.send(&output));
        let before = client.queued_bytes;
        assert!(client.flush());
        assert!(client.snapshots > 0);
        assert_eq!(client.queued_bytes, output_length);
        assert!(client.queued_bytes <= before);
        assert!(!client.snapshot(Bytes::new(), Bytes::new()));
    }
    #[test]
    fn an_incompressible_maximum_restore_fits_one_snapshot_batch() {
        let mut state = 0x123456789abcdef0u64;
        let raw: Vec<u8> = (0..protocol::MAX_SNAPSHOT)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        let compressed = compress(&raw).unwrap();
        assert!(compressed.len() > protocol::MAX_SNAPSHOT);
        assert!(compressed.len() <= protocol::MAX_COMPRESSED_SNAPSHOT);
        let mut restored = Vec::new();
        flate2::read::DeflateDecoder::new(compressed.as_slice())
            .read_to_end(&mut restored)
            .unwrap();
        assert_eq!(restored, raw);
        let (stream, _peer) = UnixStream::pair().unwrap();
        let mut client = Client::new(stream, "0123456789abcdef0123456789abcdef");
        assert!(client.snapshot(Bytes::new(), compressed.into()));
        assert!(!client.can_snapshot());
    }
    #[test]
    fn admission_bounds_idle_descriptors_and_frees_a_slot_on_detach() {
        let mut clients = Vec::new();
        let mut peers = Vec::new();
        for _ in 0..MAX_CLIENTS {
            let (stream, peer) = UnixStream::pair().unwrap();
            Client::admit(&mut clients, stream, "0123456789abcdef0123456789abcdef");
            peers.push(peer);
        }
        assert_eq!(clients.len(), MAX_CLIENTS);
        let (stream, mut rejected) = UnixStream::pair().unwrap();
        rejected
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        Client::admit(&mut clients, stream, "0123456789abcdef0123456789abcdef");
        assert_eq!(rejected.read(&mut [0]).unwrap(), 0);
        assert_eq!(clients.len(), MAX_CLIENTS);
        clients.pop();
        let (stream, peer) = UnixStream::pair().unwrap();
        Client::admit(&mut clients, stream, "0123456789abcdef0123456789abcdef");
        peers.push(peer);
        assert_eq!(clients.len(), MAX_CLIENTS);
    }
    #[test]
    fn snapshot_capacity_is_reserved_before_generating_a_restore() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let mut client = Client::new(stream, "0123456789abcdef0123456789abcdef");
        while client.send(&ServerMessage::Active(false)) {}
        assert!(!client.can_snapshot());
        assert!(!client.snapshot(Bytes::new(), Bytes::new()));
    }
    #[test]
    fn ordinary_output_is_bounded_before_it_is_queued() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let mut client = Client::new(stream, "0123456789abcdef0123456789abcdef");
        let output = ServerMessage::Output(Bytes::from(vec![0; protocol::MAX_DATA]));
        while client.send(&output) {}
        let before = client.queued_bytes;
        assert!(before <= CLIENT_QUEUE);
        assert!(!client.send(&output));
        assert_eq!(client.queued_bytes, before);
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    /// A holder with no shell behind it, for what it does with its clients.
    fn holder(directory: &std::path::Path) -> Holder {
        fs::create_dir_all(directory).unwrap();
        let (signals, _) = UnixStream::pair().unwrap();
        Holder {
            files: Files {
                directory: directory.into(),
                id: ID.into(),
            },
            creator: computer("creator"),
            started: 0,
            name: None,
            listener: UnixListener::bind(directory.join("holder.sock")).unwrap(),
            signals,
            shell: Shell {
                master: File::open("/dev/null").unwrap().into(),
                pid: libc::pid_t::MAX,
                status: None,
                reaper: None,
            },
            replica: Replica::new(80, 24).unwrap(),
            size: (80, 24),
            clients: Vec::new(),
            pty_open: true,
            exited: None,
            ending: None,
            stale: false,
            recorded_title: None,
            recorded_agent: None,
            reporter: None,
            program: None,
            lock: File::create(directory.join("holder.lock")).unwrap(),
            display: None,
            last_output: Instant::now(),
            upgrade: Upgrade::default(),
        }
    }

    fn computer(id: &str) -> Computer {
        Computer {
            id: id.into(),
            name: id.into(),
        }
    }

    fn attach(columns: u16, rows: u16, opened: bool, id: &str) -> ClientMessage {
        ClientMessage::Attach(Attach {
            version: protocol::VERSION,
            columns,
            rows,
            control: false,
            opened,
            computer: Some(computer(id)),
            colors: None,
        })
    }

    /// A computer attached at its own size, which then stops reading.
    fn stalled(holder: &mut Holder) -> UnixStream {
        let (stream, peer) = UnixStream::pair().unwrap();
        holder.clients.push(Client::new(stream, ID));
        assert!(holder.handle(0, attach(80, 24, true, "stalled")));
        while holder.clients[0].send(&ServerMessage::Active(false)) {}
        peer
    }

    /// The holder's end of a status pipe the reaper closed without writing.
    fn closed_status() -> OwnedFd {
        let mut fds = [0; 2];
        // SAFETY: pipe fills two descriptors, which `fds` holds.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: both are fresh descriptors this test owns.
        unsafe {
            libc::close(fds[1]);
            OwnedFd::from_raw_fd(fds[0])
        }
    }

    #[test]
    fn a_shell_whose_reaper_died_after_collecting_it_has_ended() {
        let directory = std::env::temp_dir().join(format!("np-reaped-{}", std::process::id()));
        let mut holder = holder(&directory);
        let mut collected = std::process::Command::new("true").spawn().unwrap();
        collected.wait().unwrap();
        holder.shell.pid = collected.id() as libc::pid_t;
        holder.shell.status = Some(closed_status());
        holder.read_status();
        assert_eq!(
            holder.exited.map(|(exit, _)| exit),
            Some(Exit::Signal(libc::SIGKILL as u32))
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn a_shell_that_outlives_its_reaper_keeps_running() {
        let directory = std::env::temp_dir().join(format!("np-outlived-{}", std::process::id()));
        let mut holder = holder(&directory);
        let mut running = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        holder.shell.pid = running.id() as libc::pid_t;
        holder.shell.status = Some(closed_status());
        holder.read_status();
        assert!(holder.exited.is_none());
        running.kill().unwrap();
        running.wait().unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn a_computer_that_attaches_while_another_falls_behind_gets_the_shell() {
        let directory = std::env::temp_dir().join(format!("np-attach-{}", std::process::id()));
        let mut holder = holder(&directory);
        let _stalled = stalled(&mut holder);
        let (stream, _peer) = UnixStream::pair().unwrap();
        holder.clients.push(Client::new(stream, ID));
        // Its size goes to the stalled computer, which can't take it.
        assert!(holder.handle(1, attach(100, 30, true, "fresh")));
        assert_eq!(holder.clients.len(), 1);
        assert_eq!(holder.clients[0].mac(), Some(computer("fresh")));
        assert_eq!(holder.clients[0].told_size, Some((100, 30)));
        assert_eq!(holder.size, (100, 30));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn a_computer_used_while_another_falls_behind_gets_the_shell() {
        let directory = std::env::temp_dir().join(format!("np-used-{}", std::process::id()));
        let mut holder = holder(&directory);
        let _stalled = stalled(&mut holder);
        let (stream, _peer) = UnixStream::pair().unwrap();
        holder.clients.push(Client::new(stream, ID));
        // Attached but not used yet: the shell keeps the stalled one's size.
        assert!(holder.handle(1, attach(100, 30, false, "fresh")));
        assert_eq!(holder.size, (80, 24));
        // As the holder's loop does after each message.
        assert!(holder.clients[1].flush());
        assert!(holder.handle(1, ClientMessage::Activate));
        assert_eq!(holder.clients.len(), 1);
        assert_eq!(holder.clients[0].mac(), Some(computer("fresh")));
        assert_eq!(holder.size, (100, 30));
        let _ = fs::remove_dir_all(directory);
    }
}
