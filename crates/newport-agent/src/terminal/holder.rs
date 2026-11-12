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
            ffi::OsStringExt,
            fs::{OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
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
}

use super::display::Change;

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

/// Opens the PTY and starts `sh -c command` as its session leader, with
/// `changes` made to its environment.
fn spawn(command: &str, (columns, rows): (u16, u16), changes: &[Change]) -> io::Result<Shell> {
    let shell = CString::new("/bin/sh").unwrap();
    let argv = [
        CString::new("sh").unwrap(),
        CString::new("-c").unwrap(),
        CString::new(command).map_err(|_| io::Error::other("Invalid shell command."))?,
    ];
    let mut argv: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    // The PTY's own terminal type, as sshd's pty-req sets it.
    let mut changes = changes.to_vec();
    changes.push(("TERM", Some("xterm-256color".into())));
    // Built before the fork: the child only execs.
    let environment = shell_environment(&changes);
    let mut envp: Vec<*const libc::c_char> = environment.iter().map(|e| e.as_ptr()).collect();
    envp.push(std::ptr::null());
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
    // SAFETY: the child only calls async-signal-safe functions on memory
    // prepared before the fork, then execs or exits.
    match unsafe { libc::fork() } {
        -1 => {
            let error = io::Error::last_os_error();
            // SAFETY: both descriptors came from openpty and are used only here.
            unsafe {
                libc::close(slave);
                libc::close(master);
            }
            Err(error)
        }
        // SAFETY: the child calls only async-signal-safe functions, on argv and envp
        // built before the fork.
        0 => unsafe {
            libc::setsid();
            libc::ioctl(slave, libc::TIOCSCTTY as _, 0);
            for fd in 0..3 {
                libc::dup2(slave, fd);
            }
            libc::close(slave);
            libc::close(master);
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::execve(shell.as_ptr(), argv.as_ptr(), envp.as_ptr());
            libc::_exit(127)
        },
        pid => {
            // SAFETY: the shell has its own copy of the slave.
            unsafe { libc::close(slave) };
            cloexec(master);
            nonblocking(master);
            Ok(Shell {
                // SAFETY: master came from openpty and nothing else owns it.
                master: unsafe { OwnedFd::from_raw_fd(master) },
                pid,
            })
        }
    }
}

/// Orphans the shell leaves behind become the holder's to reap, not a PID 1
/// that may not reap them.
fn become_subreaper() {
    #[cfg(target_os = "linux")]
    // SAFETY: prctl with this option takes scalar arguments.
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }
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
    _clipboard: Option<ClipboardDisplay>,
}

/// The shell's clipboard display, so the server's clipboard programs and
/// those that read X11 or Wayland themselves, as Codex does through arboard,
/// paste from the computer someone last used the shell on, decided at each read:
/// the shell keeps its `DISPLAY` and `WAYLAND_DISPLAY` for its whole life.
/// Their copies are written to the shell's terminal, which sends them to the
/// active computer. It holds no windows: graphical apps get their computer's display at
/// launch.
struct ClipboardDisplay {
    display: crate::display::TerminalDisplay,
    terminal: crate::clipboard_copy::Terminal,
}

impl ClipboardDisplay {
    /// Starts it. A display that can't start leaves the shell without it, as
    /// it would be without Newport.
    fn start(files: &Files) -> Self {
        let terminal = crate::clipboard_copy::Terminal::default();
        let display = crate::display::TerminalDisplay::start(
            files.id(),
            Some(&files.clipboard()),
            crate::clipboard_copy::to_terminal(terminal.clone()),
        );
        Self { display, terminal }
    }

    /// Sends copies to the shell's terminal, whose controlling side is
    /// `master`.
    fn shown_on(&self, master: &OwnedFd) {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: ptsname reads the descriptor's state into a buffer that
        // nothing else in the holder uses; the name is copied out at once.
        let name = unsafe { libc::ptsname(master.as_raw_fd()) };
        if !name.is_null() {
            // SAFETY: ptsname returned a non-null NUL-terminated string.
            let name = unsafe { std::ffi::CStr::from_ptr(name) };
            let _ = self
                .terminal
                .set(std::ffi::OsStr::from_bytes(name.to_bytes()).into());
        }
    }

    /// The variables that name it to the shell, and unset those it lacks.
    fn environment(&self) -> Vec<Change> {
        self.display.variables()
    }
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

    become_subreaper();
    // The account's stable display, running or expected, only when the
    // creating computer may show it: windows and clipboard on the computer
    // showing it. Otherwise a clipboard display of its own, even while
    // another computer shows the stable display, so the shell's clipboard
    // stays its own computer's.
    let display = start
        .stable_display
        .then(|| super::display::running().or_else(super::display::expected))
        .flatten();
    let (clipboard, environment) = match display {
        Some(environment) => (None, environment),
        None => {
            let clipboard = ClipboardDisplay::start(&files);
            let environment = clipboard.environment();
            (Some(clipboard), environment)
        }
    };
    let shell = spawn(&start.command, size, &environment)?;
    if let Some(clipboard) = &clipboard {
        clipboard.shown_on(&shell.master);
    }
    let mut holder = Holder {
        files,
        creator: start.creator,
        started: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        name: None,
        listener,
        shell,
        replica: Replica::new(size.0, size.1).map_err(|_| io::Error::other("Invalid size"))?,
        size,
        clients: Vec::new(),
        pty_open: true,
        exited: None,
        ending: None,
        _clipboard: clipboard,
        stale: false,
        recorded_title: None,
    };
    holder.write_record()?;
    let result = holder.run();
    holder.files.remove();
    drop(lock);
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
            "creator": self.creator,
            "attached": attached,
            "exited": exited,
        });
        self.recorded_title = self.replica.title().map(str::to_owned);
        crate::paths::private_write(&self.files.record(), record.to_string().as_bytes())
    }

    fn run(&mut self) -> io::Result<()> {
        let mut buffer = vec![0u8; 64 * 1024];
        let mut last_tick = Instant::now();
        loop {
            let master = self.shell.master.as_raw_fd();
            let mut fds = vec![libc::pollfd {
                fd: self.listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            }];
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
            // SAFETY: the pointer and count describe the live `fds` vector.
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, TICK.as_millis() as _) };

            let ready = |fd: RawFd| fds.iter().any(|p| p.fd == fd && p.revents != 0);
            if self.pty_open && ready(master) {
                self.read_pty(&mut buffer);
            }
            if ready(self.listener.as_raw_fd()) {
                self.accept();
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
        let bytes = &buffer[..n as usize];
        if let Ok(effects) = self.replica.advance(bytes) {
            for reply in effects.replies {
                write_all_pty(master, &reply);
            }
        }
        self.broadcast(&ServerMessage::Output(Bytes::copy_from_slice(bytes)));
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
        }
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
        self.apply_active();
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
                if opened {
                    self.clients[index].used = Some(Instant::now());
                }
                self.stale = true;
                self.apply_active();
                let mut ok = self.clients[index].send(&ServerMessage::Attached { prompted })
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

    /// Collects every exited child: the shell, and orphans the holder adopted.
    fn reap(&mut self) {
        loop {
            let mut status = 0;
            // SAFETY: `status` is a valid out-pointer.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid <= 0 {
                return;
            }
            if pid == self.shell.pid && self.exited.is_none() {
                self.exited = Some((exit_of(status), Instant::now()));
                self.stale = true;
            }
        }
    }

    fn tick(&mut self) {
        // Once a tick at most: some programs change the title many times a
        // second.
        self.stale |= self.replica.title() != self.recorded_title.as_deref();
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
                // Held for the holder's life, like the first lock.
                std::mem::forget(lock);
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
