//! Persistent terminals: shells that outlive the connection that started
//! them. A holder process owns each shell (see `holder`); this worker starts,
//! lists, watches, renames and ends holders, and `terminal attach` bridges an
//! exec channel to one when socket forwarding is off. `terminal route`
//! records the computer a plain terminal runs on, and `terminal display` names the
//! account's display to its shell.
mod display;
mod holder;

use crate::transport::Cancellation;
use newport_protocol::terminal::{
    Attached, Computer, Exit, Holder, Reply, Request, holder as protocol, wire,
};
use serde::Deserialize;
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{fs::OpenOptionsExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The worker's version. Desktops speaking another one are told to update.
pub(crate) const VERSIONS: &[u32] = &[newport_protocol::terminal::VERSION];
/// How long `start` waits for a new holder's socket.
const START_WAIT: Duration = Duration::from_secs(10);

/// A terminal id: a UUID in hex without dashes, short enough for socket paths.
pub(crate) fn valid_id(id: &str) -> io::Result<String> {
    let id = id.to_ascii_lowercase();
    if newport_protocol::terminal::valid_id(&id) {
        Ok(id)
    } else {
        Err(io::Error::other("Invalid terminal id."))
    }
}

/// The directories holders keep their files in, in the order to look: the
/// runtime directory's, then the home directory's, which a holder moves to
/// when the runtime directory disappears.
fn directories() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = [primary_directory(), fallback_directory()]
        .into_iter()
        .flatten()
        .collect();
    found.dedup();
    found
}

fn primary_directory() -> io::Result<PathBuf> {
    terminals_in(crate::paths::runtime()?)
}

fn fallback_directory() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?;
    terminals_in(crate::paths::private_home_directory(
        Path::new(&home),
        ".cache/newport/run",
    )?)
}

fn terminals_in(root: PathBuf) -> io::Result<PathBuf> {
    crate::paths::private_home_directory(&root, "terminals")
}

/// A holder's files, named by its terminal id.
pub(crate) struct Files {
    directory: PathBuf,
    id: String,
}

impl Files {
    fn create(id: &str) -> io::Result<Self> {
        Ok(Files {
            directory: primary_directory()?,
            id: id.into(),
        })
    }
    fn fallback(id: &str) -> io::Result<Self> {
        Ok(Files {
            directory: fallback_directory()?,
            id: id.into(),
        })
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn file(&self, extension: &str) -> PathBuf {
        self.directory.join(format!("{}.{extension}", self.id))
    }
    /// What clients attach to.
    fn socket(&self) -> PathBuf {
        self.file("sock")
    }
    /// Held with `flock` for the holder's life.
    fn lock(&self) -> PathBuf {
        self.file("lock")
    }
    /// The shell's facts, written only by its holder, for `list`.
    fn record(&self) -> PathBuf {
        self.file("json")
    }
    /// The shell's command and creator, from the worker to the holder it
    /// starts. A file, not an argument: other accounts can read a process's
    /// arguments.
    fn start(&self) -> PathBuf {
        self.file("start")
    }
    /// The Wayland socket of the shell's clipboard display.
    fn clipboard(&self) -> PathBuf {
        self.file("wl")
    }
    fn remove(&self) {
        for extension in ["sock", "json", "start", "lock", "wl", "wl.lock"] {
            let _ = fs::remove_file(self.file(extension));
        }
    }
    /// Whether a holder holds this id's lock.
    fn alive(&self) -> bool {
        let Ok(lock) = File::options().write(true).open(self.lock()) else {
            return false;
        };
        // SAFETY: flock takes a live descriptor and a scalar operation.
        let taken = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if taken {
            // SAFETY: flock takes a live descriptor and a scalar operation.
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
        }
        !taken
    }
}

/// Whether `path` is a held shell's clipboard display: a socket a holder
/// serves, which has no windows and so is no desktop.
pub(crate) fn clipboard_display(path: &Path) -> bool {
    if path.extension().is_none_or(|extension| extension != "wl")
        || !path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| valid_id(stem).is_ok_and(|id| id == stem))
    {
        return false;
    }
    // Compared resolved: the runtime directory may be reached through a link.
    let directories = directories();
    path.parent()
        .and_then(|parent| parent.canonicalize().ok())
        .is_some_and(|parent| {
            directories
                .iter()
                .any(|directory| directory.canonicalize().is_ok_and(|d| d == parent))
        })
}

/// The live holder for an id, wherever it keeps its files.
fn find(id: &str) -> Option<Files> {
    directories()
        .into_iter()
        .map(|directory| Files {
            directory,
            id: id.into(),
        })
        .find(|files| files.alive() && files.socket().exists())
}

/// A live lock owns the identity even while its socket is being recreated.
fn occupied(id: &str) -> Option<Files> {
    occupied_in(id, directories())
}
fn occupied_in(id: &str, directories: impl IntoIterator<Item = PathBuf>) -> Option<Files> {
    directories
        .into_iter()
        .map(|directory| Files {
            directory,
            id: id.into(),
        })
        .find(Files::alive)
}

/// A holder's record.
#[derive(Deserialize)]
struct Record {
    version: u32,
    columns: u16,
    rows: u16,
    started: u64,
    name: Option<String>,
    #[serde(default)]
    title: Option<String>,
    creator: Computer,
    attached: Vec<Attached>,
    exited: Option<Exit>,
}

/// One request and result, or watch updates until input closes.
pub(crate) fn serve_bound(cancellation: Cancellation) -> io::Result<()> {
    let mut input = io::BufReader::new(cancellation.stdin()?);
    let request = wire::read_request(&mut input);
    cancellation.check()?;
    let reply = match request {
        Ok(Request::Watch) => {
            return watch(&directories(), input, io::stdout().lock(), &cancellation);
        }
        Ok(request) => respond(request).unwrap_or_else(holder_failure),
        Err(error) => failure("invalid_request", error.to_string()),
    };
    emit_reply(&mut io::stdout().lock(), &reply).map(drop)
}

fn emit_reply(output: &mut impl Write, reply: &Reply) -> io::Result<bool> {
    let (bytes, accepted) = match wire::encode_reply(reply) {
        Ok(bytes) => (bytes, true),
        Err(error) => (
            wire::encode_reply(&failure("invalid_response", error.to_string()))?,
            false,
        ),
    };
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(accepted)
}

fn failure(code: &str, message: String) -> Reply {
    Reply::Failed(newport_protocol::common::Error {
        code: code.into(),
        message,
        retry: newport_protocol::common::Retry::Unknown,
    })
}

#[derive(Debug)]
struct HolderGreeting(io::Error);
impl std::fmt::Display for HolderGreeting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.kind() == io::ErrorKind::InvalidData {
            write!(f, "{} ({})", protocol::REOPEN_MESSAGE, self.0)
        } else {
            write!(
                f,
                "The terminal did not complete its greeting: {}. If it predates this update, close the tab to end its shell, then open a new one.",
                self.0
            )
        }
    }
}
impl std::error::Error for HolderGreeting {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
#[derive(Debug)]
struct HolderOutcome(io::Error);
impl std::fmt::Display for HolderOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The terminal operation did not complete its response: {}. Its outcome could not be confirmed; do not replay it automatically.",
            self.0
        )
    }
}
impl std::error::Error for HolderOutcome {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
fn outcome_failed(error: io::Error) -> io::Error {
    if error
        .get_ref()
        .is_some_and(|cause| cause.is::<newport_protocol::common::Error>())
    {
        error
    } else {
        io::Error::new(error.kind(), HolderOutcome(error))
    }
}
fn holder_failure(error: io::Error) -> Reply {
    if error
        .get_ref()
        .is_some_and(|cause| cause.is::<HolderGreeting>())
    {
        if error.kind() == io::ErrorKind::InvalidData {
            Reply::Failed(protocol::reopen_required())
        } else {
            Reply::Failed(newport_protocol::common::Error {
                code: protocol::GREETING_INCOMPLETE.into(),
                message: error.to_string(),
                retry: newport_protocol::common::Retry::No,
            })
        }
    } else if error
        .get_ref()
        .is_some_and(|cause| cause.is::<HolderOutcome>())
    {
        Reply::Failed(newport_protocol::common::Error {
            code: "holder_outcome_unknown".into(),
            message: error.to_string(),
            retry: newport_protocol::common::Retry::Unknown,
        })
    } else if error
        .get_ref()
        .is_some_and(|cause| cause.is::<NotStarted>())
    {
        Reply::Failed(newport_protocol::common::Error {
            code: newport_protocol::terminal::NOT_STARTED.into(),
            message: error.to_string(),
            retry: newport_protocol::common::Retry::No,
        })
    } else if let Some(remote) = error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<newport_protocol::common::Error>())
    {
        Reply::Failed(remote.clone())
    } else {
        failure("failed", error.to_string())
    }
}

/// A start that failed before anything was launched.
#[derive(Debug)]
struct NotStarted(io::Error);
impl std::fmt::Display for NotStarted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for NotStarted {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
fn not_started(error: io::Error) -> io::Error {
    io::Error::new(error.kind(), NotStarted(error))
}

fn greeting_failed(error: io::Error) -> io::Error {
    io::Error::new(error.kind(), HolderGreeting(error))
}

struct HolderDeadline<'a> {
    stream: &'a mut UnixStream,
    until: Instant,
}
impl HolderDeadline<'_> {
    fn remaining(&self) -> io::Result<Duration> {
        self.until
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Terminal holder operation timed out",
                )
            })
    }
}
impl Read for HolderDeadline<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}
impl Write for HolderDeadline<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}
fn connect_holder(files: &Files) -> io::Result<(UnixStream, Instant)> {
    let until = Instant::now() + Duration::from_secs(2);
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket
        .connect_timeout(
            &socket2::SockAddr::unix(files.socket())?,
            Duration::from_secs(2),
        )
        .map_err(greeting_failed)?;
    let descriptor: std::os::fd::OwnedFd = socket.into();
    let mut stream = UnixStream::from(descriptor);
    let hello = protocol::wire::read_server(&mut HolderDeadline {
        stream: &mut stream,
        until,
    })
    .map_err(io::Error::from)
    .map_err(greeting_failed)?;
    match hello {
        protocol::ServerMessage::Hello(hello) => hello
            .require(files.id())
            .map_err(|error| greeting_failed(io::Error::new(io::ErrorKind::InvalidData, error)))?,
        protocol::ServerMessage::Failed(error) => return Err(io::Error::other(error)),
        _ => {
            return Err(greeting_failed(io::Error::new(
                io::ErrorKind::InvalidData,
                "Terminal holder did not announce its identity",
            )));
        }
    }
    Ok((stream, until))
}

fn respond(request: Request) -> io::Result<Reply> {
    match request {
        Request::Start {
            id,
            columns,
            rows,
            command,
            creator,
            stable_display,
        } => {
            let start_file = holder::Start {
                command,
                creator,
                stable_display,
            };
            let socket = start(&valid_id(&id)?, columns, rows, &start_file)?;
            Ok(Reply::Started { socket })
        }
        Request::List => Ok(Reply::Listed(list()?)),
        Request::Rename { id, name } => Ok(Reply::Renamed {
            renamed: rename(&valid_id(&id)?, &name)?,
        }),
        Request::End { id } => Ok(Reply::Ended {
            ended: end(&valid_id(&id)?)?,
        }),
        Request::Watch => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Watch requires a streaming response",
        )),
    }
}

/// The holder's socket: a new holder's, or, for an id that has one, that
/// holder's.
fn start(id: &str, columns: u16, rows: u16, start_file: &holder::Start) -> io::Result<String> {
    if columns == 0 || rows == 0 {
        return Err(io::Error::other("Invalid terminal size."));
    }
    if occupied(id).is_some() {
        // A holder binds its socket just before it writes its record.
        let deadline = Instant::now() + START_WAIT;
        while Instant::now() < deadline {
            if let Some(files) = occupied(id).filter(|files| files.socket().exists())
                && let Some(holder) = holder(&files)
            {
                connect_holder(&files)?;
                return Ok(holder.socket);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        return Err(greeting_failed(io::Error::new(
            io::ErrorKind::TimedOut,
            "The existing terminal still owns this identity but its socket is unavailable",
        )));
    }
    // Nothing runs for this id until `launch`: a failure before it is certain.
    let files = Files::create(id).map_err(not_started)?;
    let mut start = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(files.start())
        .map_err(not_started)?;
    let written = serde_json::to_vec(start_file)
        .map_err(io::Error::from)
        .and_then(|bytes| start.write_all(&bytes));
    drop(start);
    if let Err(error) = written {
        let _ = fs::remove_file(files.start());
        return Err(not_started(error));
    }
    launch(id, columns, rows)?;
    let deadline = Instant::now() + START_WAIT;
    while Instant::now() < deadline {
        if files.alive() && files.socket().exists() {
            connect_holder(&files)?;
            return Ok(files.socket().display().to_string());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_file(files.start());
    Err(io::Error::other(
        "The terminal did not start on the server.",
    ))
}

/// Starts the holder from this worker's own build; it detaches itself.
fn launch(id: &str, columns: u16, rows: u16) -> io::Result<()> {
    let args = [
        "terminal".to_string(),
        "holder".into(),
        id.into(),
        columns.to_string(),
        rows.to_string(),
    ];
    crate::detached::launch(&format!("newport-terminal-{id}"), &args)
        .map_err(|_| io::Error::other("The terminal did not start on the server."))
}

fn list() -> io::Result<Vec<Holder>> {
    list_in(&directories())
}

/// Live holders, oldest first. Files of holders that are gone are removed.
fn list_in(directories: &[PathBuf]) -> io::Result<Vec<Holder>> {
    let mut holders = Vec::new();
    let mut text_bytes = 0usize;
    for directory in directories {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".lock")) else {
                continue;
            };
            let Ok(id) = valid_id(id) else {
                continue;
            };
            let files = Files {
                directory: directory.to_owned(),
                id: id.clone(),
            };
            if !files.alive() {
                // A start file younger than a starting holder is not stale.
                if !files.start().exists() {
                    files.remove();
                }
                continue;
            }
            if holders.iter().any(|h: &Holder| h.id == id) {
                continue;
            }
            if let Some(holder) = holder(&files) {
                text_bytes = text_bytes.saturating_add(holder.text_bytes());
                if holders.len() == newport_protocol::terminal::MAX_HOLDERS
                    || text_bytes > newport_protocol::terminal::SERVER_LIMITS.metadata
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Terminal list exceeds its protocol limit",
                    ));
                }
                holders.push(holder);
            }
        }
    }
    holders.sort_by_key(|h| h.started);
    Ok(holders)
}

/// What a holder's record says, when it has written one this Needle reads.
fn holder(files: &Files) -> Option<Holder> {
    let limit = newport_protocol::terminal::SERVER_LIMITS.metadata;
    let mut bytes = Vec::new();
    File::open(files.record())
        .ok()?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > limit {
        return None;
    }
    let record: Record = serde_json::from_slice(&bytes).ok()?;
    let holder = Holder {
        socket: files.socket().display().to_string(),
        id: files.id.clone(),
        version: record.version,
        columns: record.columns,
        rows: record.rows,
        started: record.started,
        name: record.name,
        title: record.title,
        creator: record.creator,
        attached: record.attached,
        exited: record.exited,
    };
    holder.validate().ok()?;
    Some(holder)
}

/// The computers a terminal's programs' requests may go to, as `NEWPORT_TERMINAL`
/// names it: those attached to its live holder, most recently used first, or
/// else the one a plain terminal runs on, as its route says.
pub(crate) fn attached(terminal: &str) -> Option<Vec<Attached>> {
    let id = uuid::Uuid::parse_str(terminal).ok()?.simple().to_string();
    if let Some(holder) = find(&id).and_then(|files| holder(&files)) {
        return Some(holder.attached);
    }
    let mac = read_route(&routes_directory().ok()?.join(ROUTES), &id)?;
    Some(vec![Attached {
        id: mac.id,
        name: mac.name,
        active: true,
    }])
}

/// The folder of plain terminals' routes in a terminal directory.
const ROUTES: &str = "routes";
/// The most routes kept. A plain terminal's route outlives its tab, for the
/// programs it leaves running. Each use makes a route the newest, so the
/// least recently used go first.
const MAX_ROUTES: usize = 256;
/// The largest route: a computer's id and name.
const MAX_ROUTE: u64 = 4096;
/// How old a write's leftover temporary file is before it is removed: a
/// write in progress is younger.
const ORPHAN_AGE: Duration = Duration::from_secs(60);

/// Records the computer a plain terminal runs on, before its shell starts: its
/// programs' requests go there, as a held shell's go to its active computer.
fn route(terminal: &str, mac: Computer) -> io::Result<()> {
    let id = uuid::Uuid::parse_str(terminal)
        .map_err(|_| io::Error::other("Invalid terminal id."))?
        .simple()
        .to_string();
    if mac.name.chars().any(char::is_control) {
        return Err(io::Error::other("Invalid computer name."));
    }
    let mac = Computer {
        id: uuid::Uuid::parse_str(&mac.id)
            .map_err(|_| io::Error::other("Invalid computer id."))?
            .to_string(),
        name: mac.name,
    };
    route_in(&routes_directory()?, &id, &mac)
}

/// Where routes are kept: in the home directory, so programs a terminal
/// leaves running, such as tmux, keep theirs when the runtime directory goes
/// at the account's last logout, as holders do.
fn routes_directory() -> io::Result<PathBuf> {
    fallback_directory()
}

/// A computer name as `terminal route` takes it: its UTF-8 in hexadecimal,
/// which passes unchanged through every shell between the computer and Needle.
fn name_from_hex(hex: &str) -> io::Result<String> {
    let invalid = || io::Error::other("Invalid computer name.");
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| invalid()))
        .collect::<io::Result<Vec<_>>>()?;
    String::from_utf8(bytes).map_err(|_| invalid())
}

/// As `route`, in the terminal directory `terminals`.
fn route_in(terminals: &Path, id: &str, mac: &Computer) -> io::Result<()> {
    let directory = crate::paths::private_home_directory(terminals, ROUTES)?;
    // Bounded as a holder bounds the name a computer attaches with.
    let mac = Computer {
        id: mac.id.clone(),
        name: mac.name.chars().take(protocol::MAX_NAME).collect(),
    };
    crate::paths::private_write(
        &directory.join(format!("{id}.json")),
        &serde_json::to_vec(&mac)?,
    )?;
    prune_routes(&directory, MAX_ROUTES);
    Ok(())
}

/// Removes the least recently used routes beyond `keep`, and temporary
/// files that writes which never finished left behind.
fn prune_routes(directory: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut routes = Vec::new();
    for entry in entries.flatten() {
        let Some(modified) = entry.metadata().and_then(|m| m.modified()).ok() else {
            continue;
        };
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            routes.push((modified, path));
        } else if entry.file_name().to_string_lossy().starts_with(".tmp")
            && now
                .duration_since(modified)
                .is_ok_and(|age| age >= ORPHAN_AGE)
        {
            let _ = fs::remove_file(path);
        }
    }
    if routes.len() <= keep {
        return;
    }
    routes.sort();
    for (_, path) in &routes[..routes.len() - keep] {
        let _ = fs::remove_file(path);
    }
}

/// A terminal's route, when its folder and file are private to this account.
/// Reading it makes it the newest, so routes in use are pruned last.
fn read_route(directory: &Path, id: &str) -> Option<Computer> {
    use std::os::unix::fs::MetadataExt;
    let private = |metadata: &fs::Metadata| {
        metadata.uid() == crate::paths::effective_uid() && metadata.mode() & 0o077 == 0
    };
    if !fs::symlink_metadata(directory).is_ok_and(|m| m.is_dir() && private(&m)) {
        return None;
    }
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(directory.join(format!("{id}.json")))
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || !private(&metadata) || metadata.len() > MAX_ROUTE {
        return None;
    }
    let mut bytes = Vec::new();
    (&file).take(MAX_ROUTE).read_to_end(&mut bytes).ok()?;
    let mac = serde_json::from_slice(&bytes).ok()?;
    let _ = file.set_modified(std::time::SystemTime::now());
    Some(mac)
}

/// Ends a shell, as Close does: its holder tells every attached computer.
fn end(id: &str) -> io::Result<bool> {
    let Some(mut stream) = control(id)? else {
        return Ok(false);
    };
    let until = Instant::now() + START_WAIT;
    protocol::wire::write_client(
        &mut HolderDeadline {
            stream: &mut stream,
            until,
        },
        &protocol::ClientMessage::End,
    )
    .map_err(io::Error::from)
    .map_err(outcome_failed)?;
    control_completed_until(&mut stream, None, until).map_err(outcome_failed)?;
    Ok(true)
}

/// Renames a shell through its holder, the record's only writer.
fn rename(id: &str, name: &str) -> io::Result<bool> {
    let Some(mut stream) = control(id)? else {
        return Ok(false);
    };
    let name: String = name
        .chars()
        .filter(|character| !character.is_control())
        .take(protocol::MAX_RENAME)
        .collect();
    let name = name.trim().to_owned();
    let until = Instant::now() + START_WAIT;
    protocol::wire::write_client(
        &mut HolderDeadline {
            stream: &mut stream,
            until,
        },
        &protocol::ClientMessage::Rename(name.clone()),
    )
    .map_err(io::Error::from)
    .map_err(outcome_failed)?;
    control_completed_until(&mut stream, Some(&name), until).map_err(outcome_failed)?;
    Ok(true)
}

/// A single matching outcome followed by clean EOF completes the mutation.
/// `None` expects a reaped shell's Exit; `Some` expects the persisted name.
fn control_completed_until(
    stream: &mut UnixStream,
    name: Option<&str>,
    until: Instant,
) -> io::Result<()> {
    let mut confirmed = false;
    loop {
        let Some(body) = newport_protocol::encoding::protobuf::read_frame(
            &mut HolderDeadline { stream, until },
            protocol::SERVER_LIMITS.metadata,
        )
        .map_err(protocol::wire::Error::from)?
        else {
            return if confirmed {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Terminal holder closed before confirming the operation",
                ))
            };
        };
        match protocol::wire::decode_server(body)? {
            protocol::ServerMessage::Exit(_) if name.is_none() && !confirmed => confirmed = true,
            protocol::ServerMessage::Renamed(value)
                if name == Some(value.as_str()) && !confirmed =>
            {
                confirmed = true
            }
            protocol::ServerMessage::Failed(error) if !confirmed => {
                return Err(io::Error::other(error));
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unexpected terminal control completion",
                ));
            }
        }
    }
}

/// A control connection to a live holder: no snapshot, and not listed as
/// attached.
fn control(id: &str) -> io::Result<Option<UnixStream>> {
    let Some(files) = occupied(id) else {
        return Ok(None);
    };
    let (mut stream, until) = connect_holder(&files)?;
    let hello = protocol::Attach {
        version: protocol::VERSION,
        columns: 80,
        rows: 24,
        control: true,
        opened: false,
        computer: None,
        colors: None,
    };
    protocol::wire::write_client(
        &mut HolderDeadline {
            stream: &mut stream,
            until,
        },
        &protocol::ClientMessage::Attach(hello),
    )?;
    let reply = protocol::wire::read_server(&mut HolderDeadline {
        stream: &mut stream,
        until,
    })
    .map_err(io::Error::from)?;
    match reply {
        protocol::ServerMessage::Attached { .. } => {}
        protocol::ServerMessage::Failed(error) => return Err(io::Error::other(error)),
        _ => {
            return Err(greeting_failed(io::Error::new(
                io::ErrorKind::InvalidData,
                "Terminal holder refused control attachment",
            )));
        }
    }
    Ok(Some(stream))
}

/// How often `watch` lists again when nothing changed in the directories: a
/// holder that was killed leaves its record behind, and only its lock tells.
const RELIST: Duration = Duration::from_secs(5);
/// Changes arriving together, such as a record's temporary file and rename,
/// are listed once.
const SETTLE: Duration = Duration::from_millis(100);

enum Wake {
    Changed,
    Closed,
}

/// Writes the list, then again whenever it changes, until `input` closes or
/// a write fails.
fn watch(
    directories: &[PathBuf],
    input: impl Read + Send + 'static,
    mut output: impl Write,
    cancellation: &Cancellation,
) -> io::Result<()> {
    use notify::Watcher;
    let (sender, wakes) = std::sync::mpsc::channel();
    let changed = sender.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if !event.is_ok_and(|event| matches!(event.kind, notify::EventKind::Access(_))) {
            let _ = changed.send(Wake::Changed);
        }
    })
    .map_err(io::Error::other)?;
    // Without a watch on a directory the periodic list still notices
    // changes, only later.
    for directory in directories {
        let _ = watcher.watch(directory, notify::RecursiveMode::NonRecursive);
    }
    std::thread::spawn(move || {
        let mut input = input;
        let mut sink = [0u8; 1024];
        while matches!(input.read(&mut sink), Ok(n) if n > 0) {}
        let _ = sender.send(Wake::Closed);
    });
    let mut last = None;
    loop {
        cancellation.check()?;
        let holders = match list_in(directories) {
            Ok(holders) => holders,
            Err(error) => {
                emit_reply(&mut output, &failure("failed", error.to_string()))?;
                return Ok(());
            }
        };
        if last.as_ref() != Some(&holders) {
            if !emit_reply(&mut output, &Reply::Listed(holders.clone()))? {
                return Ok(());
            }
            last = Some(holders);
        }
        match wakes.recv_timeout(RELIST) {
            Ok(Wake::Closed) => return Ok(()),
            Ok(Wake::Changed) => {
                std::thread::sleep(SETTLE);
                for wake in wakes.try_iter() {
                    if matches!(wake, Wake::Closed) {
                        return Ok(());
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// `terminal <holder|attach|display|route> …`; `route <terminal> <mac-id>
/// <name>` takes the name as `name_from_hex` reads it.
pub fn run(args: &[String]) -> io::Result<()> {
    match args.first().map(String::as_str) {
        Some("holder") => holder::run(&args[1..]),
        Some("attach") if args.len() == 2 => attach(&valid_id(&args[1])?),
        Some("display") if args.len() == 1 => display::print(),
        Some("route") if args.len() == 4 => route(
            &args[1],
            Computer {
                id: args[2].clone(),
                name: name_from_hex(&args[3])?,
            },
        ),
        _ => Err(io::Error::other("Unknown terminal command.")),
    }
}

/// Copies bytes between stdin/stdout and a holder's socket, for an exec
/// channel when the server refuses socket forwarding.
fn attach(id: &str) -> io::Result<()> {
    let files = find(id).ok_or_else(|| io::Error::other("This terminal is no longer running."))?;
    let socket = UnixStream::connect(files.socket())?;
    let mut from_holder = socket.try_clone()?;
    let output = std::thread::spawn(move || {
        let result = copy_live(&mut from_holder, &mut io::stdout().lock());
        // The holder closed: stop waiting on stdin too.
        finish_attach(result);
    });
    let mut to_holder = socket;
    if let Err(error) = io::copy(&mut io::stdin().lock(), &mut to_holder) {
        finish_attach(Err(error));
    }
    let _ = to_holder.shutdown(std::net::Shutdown::Both);
    output
        .join()
        .map_err(|_| io::Error::other("Terminal bridge output thread panicked"))?;
    Ok(())
}

fn finish_attach(result: io::Result<u64>) -> ! {
    match result {
        Ok(_) => std::process::exit(0),
        Err(error) => {
            eprintln!("newport-agent: terminal bridge: {error}");
            std::process::exit(1);
        }
    }
}

/// Holder records are binary and need not end with a newline. Flush each
/// copied chunk before waiting for more input from the still-live holder.
fn copy_live(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<u64> {
    struct Flushed<'a, W>(&'a mut W);
    impl<W: Write> Write for Flushed<'_, W> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let count = self.0.write(bytes)?;
            self.0.flush()?;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
    io::copy(reader, &mut Flushed(writer))
}

#[cfg(test)]
mod tests;
