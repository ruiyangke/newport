//! GUI command handoff. A waiting process keeps argv, cwd and stdio until exec.
use crate::{gui_protocol as protocol, wire};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    env, fs,
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn desktop_dirs() -> Vec<PathBuf> {
    let home = PathBuf::from(env::var_os("HOME").unwrap_or_default());
    let mut dirs = vec![
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("applications"),
    ];
    dirs.extend(
        env::split_paths(
            &env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
        )
        .map(|p| p.join("applications")),
    );
    dirs.extend([
        PathBuf::from("/var/lib/snapd/desktop/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
        home.join(".local/share/flatpak/exports/share/applications"),
    ]);
    dirs
}

#[derive(Default)]
struct Catalogue {
    commands: BTreeMap<String, PathBuf>,
    flatpaks: HashSet<String>,
    snaps: HashSet<String>,
}
fn command_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        && !matches!(
            name,
            "env"
                | "sh"
                | "bash"
                | "dash"
                | "zsh"
                | "fish"
                | "python"
                | "python3"
                | "java"
                | "node"
                | "perl"
                | "ruby"
                | "sudo"
                | "xclip"
                | "wl-paste"
                | "xdg-open"
                | "newport-agent"
                | "true"
                | "false"
        )
}
fn native_program(name: &str, shim_dir: &Path) -> Option<PathBuf> {
    let path = Path::new(name);
    let candidates: Vec<_> = if path.is_absolute() {
        vec![path.to_owned()]
    } else {
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .filter(|p| p != shim_dir)
            .map(|p| p.join(name))
            .chain(std::iter::once(Path::new("/snap/bin").join(name)))
            .collect()
    };
    candidates.into_iter().find(|p| {
        p.is_absolute()
            && !p.starts_with(shim_dir)
            && fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}
fn desktop_exec(text: &str) -> Option<Vec<String>> {
    let mut entry = false;
    let mut executable = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            entry = line == "[Desktop Entry]";
            continue;
        }
        if !entry {
            continue;
        }
        if matches!(line, "Terminal=true" | "Hidden=true" | "NoDisplay=true") {
            return None;
        }
        if let Some(value) = line.strip_prefix("Exec=") {
            executable = shlex::split(value);
        }
    }
    let mut args = executable?;
    if args
        .first()
        .is_some_and(|v| Path::new(v).file_name().is_some_and(|v| v == "env"))
    {
        args.remove(0);
        while args
            .first()
            .is_some_and(|v| v.contains('=') || v.starts_with('-'))
        {
            args.remove(0);
        }
    }
    (!args.is_empty()).then_some(args)
}
fn catalogue(dirs: &[PathBuf], shim_dir: &Path) -> Catalogue {
    let mut result = Catalogue::default();
    for file in dirs
        .iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .take(4096)
    {
        let file = file.path();
        if file.extension().is_none_or(|e| e != "desktop")
            || fs::metadata(&file).is_ok_and(|m| m.len() > 128 * 1024)
        {
            continue;
        }
        let Some(args) = fs::read_to_string(file).ok().and_then(|v| desktop_exec(&v)) else {
            continue;
        };
        let name = Path::new(&args[0])
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        if !command_name(&name) {
            continue;
        }
        if name == "flatpak" {
            if let Some(id) = args
                .iter()
                .skip(1)
                .find(|a| !a.starts_with('-') && a.matches('.').count() >= 2)
            {
                result.flatpaks.insert(id.clone());
            } else {
                continue;
            }
        }
        if Path::new(&args[0]).parent() == Some(Path::new("/snap/bin")) {
            result.snaps.insert(name.to_string());
        }
        if name == "snap" {
            let Some(at) = args.iter().position(|a| a == "run") else {
                continue;
            };
            let Some(app) = args[at + 1..]
                .iter()
                .find(|a| !a.starts_with('-') && command_name(a))
            else {
                continue;
            };
            result.snaps.insert(app.clone());
            if let Some(program) = native_program(&format!("/snap/bin/{app}"), shim_dir) {
                result.commands.insert(app.clone(), program);
            }
        }
        if let Some(program) = native_program(&args[0], shim_dir) {
            result.commands.insert(name.into_owned(), program);
        }
    }
    if !result.snaps.is_empty() {
        if let Some(program) = native_program("snap", shim_dir) {
            result.commands.insert("snap".into(), program);
        }
    }
    result
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Kept out of system PATH. Each shim executes a fixed program, never a shell command from a reply.
pub(crate) fn prepare() -> io::Result<PathBuf> {
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    let dir = crate::paths::private_home_directory(&home, ".cache/newport/gui-bin")?;
    let installed = home.join(".local/bin/newport-agent");
    let agent = if installed.is_file() {
        installed
    } else {
        env::current_exe()?
    };
    let catalogue = catalogue(&desktop_dirs(), &dir);
    for file in fs::read_dir(&dir)?.flatten() {
        let name = file.file_name().to_string_lossy().into_owned();
        if !catalogue.commands.contains_key(&name)
            && fs::read_to_string(file.path()).is_ok_and(|source| {
                source.starts_with("#!/bin/sh\nexec ")
                    && source.contains(" gui --program ")
                    && source.ends_with(" -- \"$@\"\n")
            })
        {
            fs::remove_file(file.path())?;
        }
    }
    for (name, program) in &catalogue.commands {
        let source = format!(
            "#!/bin/sh\nexec {} gui --program {} -- \"$@\"\n",
            shell_quote(&agent.to_string_lossy()),
            shell_quote(&program.to_string_lossy())
        );
        let target = dir.join(name);
        if fs::read_to_string(&target).ok().as_deref() == Some(source.as_str()) {
            continue;
        }
        let mut file = tempfile::NamedTempFile::new_in(&dir)?;
        file.write_all(source.as_bytes())?;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o700))?;
        file.persist(target).map_err(|e| e.error)?;
    }
    Ok(dir)
}

fn needs_display(program: &Path, args: &[std::ffi::OsString], catalogue: &Catalogue) -> bool {
    let args: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
    if args.iter().any(|a| {
        matches!(a.as_ref(), "--version" | "--help" | "--headless")
            || a.starts_with("--headless=")
            || a.starts_with("--ozone-platform=headless")
    }) {
        return false;
    }
    let ids = match program.file_name().and_then(|n| n.to_str()) {
        Some("flatpak") => Some(&catalogue.flatpaks),
        Some("snap") => Some(&catalogue.snaps),
        _ => None,
    };
    ids.is_none_or(|ids| {
        args.iter()
            .find(|a| !a.starts_with('-'))
            .is_some_and(|a| a == "run")
            && args.iter().any(|a| ids.contains(a.as_ref()))
    })
}

pub fn run(args: Vec<std::ffi::OsString>) -> io::Result<()> {
    if args.len() < 3 || args[0] != "--program" || args[2] != "--" {
        return Err(io::Error::other(
            "Usage: newport-agent gui --program /path/to/app -- [arguments]",
        ));
    }
    let program = PathBuf::from(&args[1]);
    if !program.is_absolute() {
        return Err(io::Error::other("GUI executable must be absolute"));
    }
    let arguments = &args[3..];
    let home = PathBuf::from(env::var_os("HOME").unwrap_or_default());
    let shim_dir = home.join(".cache/newport/gui-bin");
    if program.starts_with(&shim_dir) {
        return Err(io::Error::other("Recursive GUI launcher"));
    }
    let mut command = Command::new(&program);
    command.args(arguments);
    let terminal = env::var("NEWPORT_TERMINAL")
        .ok()
        .and_then(|id| uuid::Uuid::parse_str(&id).ok());
    let inherited = crate::gui_attachment::inherited();
    let graphical = (inherited || terminal.is_some())
        && needs_display(&program, arguments, &catalogue(&desktop_dirs(), &shim_dir));
    if inherited && graphical {
        // An inherited claim runs only on its current display. A partial or
        // overridden claim must never become a fresh l/L acquisition.
        crate::gui_attachment::validate()?;
    }
    if let Some(terminal) = terminal.filter(|_| !inherited && graphical) {
        // A shell can become ready just before its independent integration transport.
        let connecting = Instant::now();
        let (mut socket, selection) = loop {
            let connected = crate::routing::select(Some("gui-launch")).and_then(|selection| {
                UnixStream::connect(selection.root.join("gui.sock"))
                    .map(|socket| (socket, selection))
            });
            match connected {
                Ok(connected) => break connected,
                Err(error) if connecting.elapsed() >= Duration::from_secs(10) => {
                    return Err(io::Error::other(format!(
                        "GUI connection unavailable: {error}. Check the server connection in Newport and run the command again."
                    )));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        };
        socket.set_write_timeout(Some(Duration::from_secs(2)))?;
        socket.set_read_timeout(Some(Duration::from_secs(protocol::WAIT_SECONDS)))?;
        let application = program
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if !selection.supports("gui-launch-v2") {
            // This peer has no shared display to hand off; its reply says why.
            let request = protocol::Request {
                id: uuid::Uuid::new_v4(),
                terminal,
                application,
            };
            request.validate()?;
            wire::write(&mut socket, b'L', &protocol::encode(&request)?)?;
            let (kind, bytes) = wire::read_limited(&mut socket, protocol::MAX_MESSAGE)?;
            let reply: protocol::Reply = protocol::decode(&bytes)?;
            if kind != b'J' || reply.id != request.id || reply.environment.is_some() {
                return Err(io::Error::other("Invalid GUI launch reply"));
            }
            return Err(io::Error::other(
                reply.error.unwrap_or_else(|| "GUI launch cancelled".into()),
            ));
        }
        let epoch = env::var("NEWPORT_TERMINAL_EPOCH")
            .ok()
            .and_then(|value| uuid::Uuid::parse_str(&value).ok())
            .filter(|id| !id.is_nil())
            .ok_or_else(|| {
                io::Error::other("Reconnect this terminal before enabling the shared GUI display.")
            })?;
        let session = selection
            .session
            .as_deref()
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .ok_or_else(|| {
                io::Error::other("Shared GUI launch requires an owned Newport session")
            })?;
        let request = protocol::SharedRequest {
            id: uuid::Uuid::new_v4(),
            terminal,
            epoch,
            application,
            session_bus: selection.supports("gui-launch-v3"),
            native_wayland: selection.supports("gui-launch-v4"),
            application_environment: selection.supports("gui-launch-v5"),
            application_environment_v2: selection.supports("gui-launch-v6"),
        };
        request.validate()?;
        wire::write(&mut socket, b'l', &protocol::encode(&request)?)?;
        let (kind, bytes) = wire::read_limited(&mut socket, protocol::MAX_MESSAGE)?;
        if kind != b'j' {
            return Err(io::Error::other("Invalid shared GUI launch frame"));
        }
        let reply: protocol::SharedReply = protocol::decode(&bytes)?;
        let environment = reply.accept(&request, session)?;
        // A successful handoff refers to ready local endpoints. Do not run
        // the user's command against a clipboard-only or vanished display.
        if !fs::metadata(&environment.wayland_socket).is_ok_and(|m| m.file_type().is_socket())
            || environment.authority.as_ref().is_some_and(|authority| {
                !fs::metadata(authority).is_ok_and(|m| m.is_file())
                    || fs::File::open(authority).is_err()
            })
        {
            return Err(io::Error::other(
                "The shared GUI display ended before launch. Run the command again.",
            ));
        }
        if env::var_os("XDG_RUNTIME_DIR").is_none_or(|value| value.is_empty()) {
            command.env(
                "XDG_RUNTIME_DIR",
                Path::new(&environment.wayland_socket).parent().unwrap(),
            );
        }
        if let Some(address) = &environment.fallback_bus {
            if ["DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS"]
                .iter()
                .all(|name| env::var_os(name).is_none_or(|value| value.is_empty()))
            {
                if !fs::metadata(environment.fallback_bus_socket().unwrap())
                    .is_ok_and(|m| m.file_type().is_socket())
                {
                    return Err(io::Error::other(
                        "The shared GUI session bus ended before launch. Run the command again.",
                    ));
                }
                command
                    .env("DBUS_SESSION_BUS_ADDRESS", address)
                    .env_remove("DBUS_STARTER_ADDRESS")
                    .env_remove("DBUS_STARTER_BUS_TYPE");
            }
        }
        for (name, value) in environment.shell_assignments().map_err(io::Error::other)? {
            match value {
                Some(value) => {
                    command.env(name, value);
                }
                None => {
                    command.env_remove(name);
                }
            }
        }
    }
    Err(command.exec())
}

pub(crate) struct Broker {
    listener: UnixListener,
    _endpoint: crate::local_socket::Endpoint,
    shared: bool,
    session_bus: bool,
    native_wayland: bool,
    application_environment: bool,
    application_environment_v2: bool,
    attachment_checks: bool,
    admitting: VecDeque<Admission>,
    pending: HashMap<uuid::Uuid, Pending>,
}
const MAX_ADMITTING: usize = protocol::MAX_PENDING;
const ACCEPTS_PER_POLL: usize = 4;
const ADMISSION_READ_BYTES: usize = 4096;
const ADMISSION_DEADLINE: Duration = Duration::from_secs(5);

/// A connected helper may be descheduled before its first write. Keep only one
/// bounded frame per client; neither idle clients nor trickled bytes can block
/// the integration pump or extend this absolute admission deadline.
struct Admission {
    socket: UnixStream,
    at: Instant,
    header: [u8; 5],
    header_read: usize,
    body: Vec<u8>,
    body_read: usize,
}
impl Admission {
    fn new(socket: UnixStream, at: Instant) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            at,
            header: [0; 5],
            header_read: 0,
            body: Vec::new(),
            body_read: 0,
        })
    }
    fn current(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) < ADMISSION_DEADLINE
    }
    fn read(&mut self, now: Instant) -> io::Result<Option<(u8, Vec<u8>)>> {
        if !self.current(now) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "GUI admission timed out",
            ));
        }
        if self.header_read < self.header.len() {
            let count = read_available(&mut self.socket, &mut self.header[self.header_read..])?;
            self.header_read += count;
            if self.header_read < self.header.len() {
                return Ok(None);
            }
            // Validate before allocating. The existing wire limit includes its
            // bounded MessagePack overhead, and decode checks the payload too.
            let length = wire::payload_length(&self.header, protocol::MAX_MESSAGE)?;
            self.body.resize(length, 0);
        }
        let end = (self.body_read + ADMISSION_READ_BYTES).min(self.body.len());
        self.body_read += read_available(&mut self.socket, &mut self.body[self.body_read..end])?;
        if self.body_read == self.body.len() {
            wire::decode(&self.body, protocol::MAX_MESSAGE).map(Some)
        } else {
            Ok(None)
        }
    }
}
fn read_available(socket: &mut UnixStream, buffer: &mut [u8]) -> io::Result<usize> {
    match socket.read(buffer) {
        Ok(0) => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "GUI helper closed",
        )),
        Ok(count) => Ok(count),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(0)
        }
        Err(error) => Err(error),
    }
}
struct Pending {
    socket: UnixStream,
    at: Instant,
    epoch: Option<uuid::Uuid>,
    session_bus: bool,
    native_wayland: bool,
    application_environment: bool,
    application_environment_v2: bool,
    attachment: Option<protocol::attachment::Check>,
}
impl Broker {
    pub fn start(root: &Path) -> io::Result<Self> {
        let (listener, endpoint) = crate::local_socket::Endpoint::fixed(&root.join("gui.sock"))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            _endpoint: endpoint,
            admitting: VecDeque::new(),
            pending: HashMap::new(),
            shared: false,
            session_bus: false,
            native_wayland: false,
            application_environment: false,
            application_environment_v2: false,
            attachment_checks: false,
        })
    }
    pub fn enable_shared(&mut self) {
        self.shared = true;
    }
    pub fn shared(&self) -> bool {
        self.shared
    }
    pub fn enable_session_bus(&mut self) {
        self.shared = true;
        self.session_bus = true;
    }
    pub fn session_bus(&self) -> bool {
        self.session_bus
    }
    pub fn enable_native_wayland(&mut self) {
        self.enable_session_bus();
        self.native_wayland = true;
    }
    pub fn native_wayland(&self) -> bool {
        self.native_wayland
    }
    pub fn enable_application_environment(&mut self) {
        self.enable_native_wayland();
        self.application_environment = true;
    }
    pub fn application_environment(&self) -> bool {
        self.application_environment
    }
    pub fn enable_application_environment_v2(&mut self) {
        self.enable_application_environment();
        self.application_environment_v2 = true;
    }
    pub fn application_environment_v2(&self) -> bool {
        self.application_environment_v2
    }
    pub fn enable_attachment_checks(&mut self) {
        self.attachment_checks = true;
    }
    pub fn attachment_checks(&self) -> bool {
        self.attachment_checks
    }
    pub fn reply(&mut self, kind: u8, data: &[u8]) -> io::Result<()> {
        if kind == protocol::attachment::REPLY && self.attachment_checks {
            let reply: protocol::attachment::Checked = protocol::attachment::decode(data)?;
            reply.validate()?;
            if self
                .pending
                .get(&reply.id)
                .and_then(|p| p.attachment.as_ref())
                .is_some_and(|request| reply.accept(request).is_ok())
            {
                let mut pending = self.pending.remove(&reply.id).unwrap();
                let _ = wire::write(&mut pending.socket, kind, data);
            }
            return Ok(());
        }
        let (id, epoch) = match kind {
            b'J' => {
                let reply: protocol::Reply = protocol::decode(data)?;
                (reply.id, None)
            }
            b'j' if self.shared => {
                let reply: protocol::SharedReply = protocol::decode(data)?;
                if reply
                    .environment
                    .as_ref()
                    .is_some_and(|e| e.fallback_bus.is_some())
                    && self.pending.get(&reply.id).is_some_and(|p| {
                        p.attachment.is_none() && p.epoch == Some(reply.epoch) && !p.session_bus
                    })
                {
                    return Err(io::Error::other("Unrequested GUI session bus reply"));
                }
                if reply
                    .environment
                    .as_ref()
                    .is_some_and(|e| e.native_wayland())
                    && self.pending.get(&reply.id).is_some_and(|p| {
                        p.attachment.is_none() && p.epoch == Some(reply.epoch) && !p.native_wayland
                    })
                {
                    return Err(io::Error::other("Unrequested native Wayland GUI reply"));
                }
                if reply
                    .environment
                    .as_ref()
                    .is_some_and(|e| !e.application.is_empty())
                    && self.pending.get(&reply.id).is_some_and(|p| {
                        p.attachment.is_none()
                            && p.epoch == Some(reply.epoch)
                            && !p.application_environment
                    })
                {
                    return Err(io::Error::other(
                        "Unrequested GUI application environment reply",
                    ));
                }
                if let Some(environment) = reply.environment.as_ref().filter(|_| {
                    self.pending.get(&reply.id).is_some_and(|pending| {
                        pending.attachment.is_none()
                            && pending.epoch == Some(reply.epoch)
                            && !pending.application_environment_v2
                    })
                }) {
                    protocol::environment::contract::application_v1::validate(
                        &environment.application,
                    )
                    .map_err(io::Error::other)?;
                }
                (reply.id, Some(reply.epoch))
            }
            _ => return Err(io::Error::other("Invalid GUI reply frame")),
        };
        // Never consume a waiting command with an old-mode or old-shell reply.
        if self
            .pending
            .get(&id)
            .is_some_and(|pending| pending.epoch == epoch && pending.attachment.is_none())
        {
            let mut pending = self.pending.remove(&id).unwrap();
            let _ = wire::write(&mut pending.socket, kind, data);
        }
        Ok(())
    }
    pub fn poll(&mut self, output: &mut impl Write) -> io::Result<()> {
        self.poll_at(output, Instant::now())
    }
    fn poll_at(&mut self, output: &mut impl Write, now: Instant) -> io::Result<()> {
        let ended: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, pending)| {
                let mut byte = 0u8;
                let count = unsafe {
                    libc::recv(
                        pending.socket.as_raw_fd(),
                        (&mut byte as *mut u8).cast(),
                        1,
                        libc::MSG_PEEK | libc::MSG_DONTWAIT,
                    )
                };
                (count >= 0
                    || (count < 0
                        && !matches!(
                            io::Error::last_os_error().kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ))
                    || now.saturating_duration_since(pending.at)
                        >= Duration::from_secs(if pending.attachment.is_some() {
                            protocol::attachment::WAIT_SECONDS
                        } else {
                            protocol::WAIT_SECONDS
                        }))
                .then_some(*id)
            })
            .collect();
        for id in ended {
            let pending = self.pending.remove(&id).unwrap();
            // Validation has no asynchronous backend work to cancel. Do not
            // let its UUID cancel an unrelated GUI launch.
            if pending.attachment.is_none() {
                wire::write(output, b'K', &protocol::encode(&id)?)?;
            }
        }
        self.admitting.retain(|client| client.current(now));
        for _ in 0..ACCEPTS_PER_POLL {
            let Ok((socket, _)) = self.listener.accept() else {
                break;
            };
            if self.admitting.len() < MAX_ADMITTING {
                if let Ok(client) = Admission::new(socket, now) {
                    self.admitting.push_back(client);
                }
            }
        }
        // Visit every existing client once, including when new connections
        // arrive each turn. Each gets at most a header and one bounded read.
        for _ in 0..self.admitting.len() {
            let mut client = self.admitting.pop_front().unwrap();
            let frame = match client.read(now) {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    self.admitting.push_back(client);
                    continue;
                }
                Err(_) => continue,
            };
            let mut socket = client.socket;
            // A helper can exit before accept. Some kernels reject socket
            // options on that closed peer; it must not end all integrations.
            if socket
                // Preserve the existing bounded reply behavior after a full
                // frame is admitted. Incomplete clients stay nonblocking.
                .set_nonblocking(false)
                .and_then(|()| socket.set_write_timeout(Some(Duration::from_millis(100))))
                .is_err()
            {
                continue;
            }
            let request = (|| {
                let (kind, bytes) = frame;
                let mut attachment = None;
                let (
                    id,
                    epoch,
                    session_bus,
                    native_wayland,
                    application_environment,
                    application_environment_v2,
                ) = match kind {
                    b'L' => {
                        let request: protocol::Request = protocol::decode(&bytes)?;
                        request.validate()?;
                        (request.id, None, false, false, false, false)
                    }
                    b'l' if self.shared => {
                        let request: protocol::SharedRequest = protocol::decode(&bytes)?;
                        request.validate()?;
                        if request.session_bus && !self.session_bus {
                            return Err(io::Error::other("GUI session bus was not negotiated"));
                        }
                        if request.native_wayland && !self.native_wayland {
                            return Err(io::Error::other(
                                "Native Wayland GUI handoff was not negotiated",
                            ));
                        }
                        if request.application_environment && !self.application_environment {
                            return Err(io::Error::other(
                                "GUI application environment was not negotiated",
                            ));
                        }
                        if request.application_environment_v2 && !self.application_environment_v2 {
                            return Err(io::Error::other(
                                "Expanded GUI application environment was not negotiated",
                            ));
                        }
                        (
                            request.id,
                            Some(request.epoch),
                            request.session_bus,
                            request.native_wayland,
                            request.application_environment,
                            request.application_environment_v2,
                        )
                    }
                    protocol::attachment::REQUEST if self.attachment_checks => {
                        let request: protocol::attachment::Check =
                            protocol::attachment::decode(&bytes)?;
                        request.validate()?;
                        let identity =
                            (request.id, Some(request.epoch), false, false, false, false);
                        attachment = Some(request);
                        identity
                    }
                    _ => return Err(io::Error::other("Invalid GUI launch frame")),
                };
                Ok((
                    kind,
                    bytes,
                    id,
                    epoch,
                    session_bus,
                    native_wayland,
                    application_environment,
                    application_environment_v2,
                    attachment,
                ))
            })();
            if let Ok((
                kind,
                bytes,
                id,
                epoch,
                session_bus,
                native_wayland,
                application_environment,
                application_environment_v2,
                attachment,
            )) = request
            {
                if self.pending.len() < protocol::MAX_PENDING && !self.pending.contains_key(&id) {
                    wire::write(output, kind, &bytes)?;
                    self.pending.insert(
                        id,
                        Pending {
                            socket,
                            at: now,
                            epoch,
                            session_bus,
                            native_wayland,
                            application_environment,
                            application_environment_v2,
                            attachment,
                        },
                    );
                } else {
                    if let Some(request) = attachment {
                        let reply = protocol::attachment::Checked::error(
                            &request,
                            "Too many pending GUI requests. Try again.",
                        );
                        let _ = wire::write(
                            &mut socket,
                            protocol::attachment::REPLY,
                            &protocol::attachment::encode(&reply)?,
                        );
                        continue;
                    }
                    let error = Some("Too many pending GUI launches. Try again.".into());
                    let (kind, bytes) = match epoch {
                        Some(epoch) => (
                            b'j',
                            protocol::encode(&protocol::SharedReply {
                                id,
                                epoch,
                                environment: None,
                                error,
                            })?,
                        ),
                        None => (
                            b'J',
                            protocol::encode(&protocol::Reply {
                                id,
                                environment: None,
                                error,
                            })?,
                        ),
                    };
                    let _ = wire::write(&mut socket, kind, &bytes);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
impl Broker {
    /// Polls until `done`. A socket a test closes can stay open a moment
    /// longer in a child another test is forking, so one poll may not see it.
    fn poll_until(&mut self, output: &mut Vec<u8>, done: impl Fn(&Self, &[u8]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(self, output) && Instant::now() < deadline {
            self.poll(output).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
#[path = "gui_launch_admission_tests.rs"]
mod admission_tests;
#[cfg(test)]
#[path = "gui_attachment_tests.rs"]
mod attachment_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upgraded_broker_never_sends_native_environment_to_waiting_old_helper() {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut broker = Broker::start(root.path()).unwrap();
        broker.enable_session_bus();
        let mut socket = UnixStream::connect(root.path().join("gui.sock")).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let request = protocol::SharedRequest {
            id: uuid::Uuid::new_v4(),
            terminal: uuid::Uuid::new_v4(),
            epoch: uuid::Uuid::new_v4(),
            application: "fixture".into(),
            session_bus: true,
            native_wayland: false,
            application_environment: false,
            application_environment_v2: false,
        };
        wire::write(&mut socket, b'l', &protocol::encode(&request).unwrap()).unwrap();
        let mut output = Vec::new();
        broker.poll(&mut output).unwrap();
        assert_eq!(wire::read(&mut &output[..]).unwrap().0, b'l');
        broker.enable_native_wayland();
        let mut reply = protocol::SharedReply {
            id: request.id,
            epoch: request.epoch,
            error: None,
            environment: Some(protocol::SharedGuiEnvironment {
                session: uuid::Uuid::new_v4(),
                display: None,
                authority: None,
                wayland_socket: "/tmp/owned/wayland-0".into(),
                fallback_bus: None,
                toolkit: BTreeMap::new(),
                application: Default::default(),
            }),
        };
        assert!(
            broker
                .reply(b'j', &protocol::encode(&reply).unwrap())
                .is_err()
        );
        assert!(broker.pending.contains_key(&request.id));
        reply.environment = None;
        reply.error = Some("Reconnect this terminal".into());
        broker
            .reply(b'j', &protocol::encode(&reply).unwrap())
            .unwrap();
        assert!(broker.pending.is_empty());
        let (kind, bytes) = wire::read(&mut socket).unwrap();
        assert_eq!(kind, b'j');
        assert_eq!(
            protocol::decode::<protocol::SharedReply>(&bytes)
                .unwrap()
                .error,
            reply.error
        );
    }
    #[test]
    fn upgraded_broker_never_sends_application_environment_to_waiting_old_helper() {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut broker = Broker::start(root.path()).unwrap();
        broker.enable_native_wayland();
        let mut helpers = Vec::new();
        let mut output = Vec::new();
        for requested in [false, true] {
            if requested {
                broker.enable_application_environment();
            }
            let socket = UnixStream::connect(root.path().join("gui.sock")).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let request = protocol::SharedRequest {
                id: uuid::Uuid::new_v4(),
                terminal: uuid::Uuid::new_v4(),
                epoch: uuid::Uuid::new_v4(),
                application: "fixture".into(),
                session_bus: true,
                native_wayland: true,
                application_environment: requested,
                application_environment_v2: false,
            };
            wire::write(&mut &socket, b'l', &protocol::encode(&request).unwrap()).unwrap();
            broker.poll(&mut output).unwrap();
            assert!(broker.pending.contains_key(&request.id));
            helpers.push((socket, request));
        }
        assert!(broker.application_environment());
        for (mut socket, request) in helpers {
            let reply = protocol::SharedReply {
                id: request.id,
                epoch: request.epoch,
                error: None,
                environment: Some(protocol::SharedGuiEnvironment {
                    session: uuid::Uuid::new_v4(),
                    display: None,
                    authority: None,
                    wayland_socket: "/tmp/owned/wayland-0".into(),
                    fallback_bus: None,
                    toolkit: BTreeMap::new(),
                    application: [("MOZ_ENABLE_WAYLAND".into(), Some("1".into()))].into(),
                }),
            };
            let delivered = broker.reply(b'j', &protocol::encode(&reply).unwrap());
            assert_eq!(delivered.is_ok(), request.application_environment);
            if request.application_environment {
                assert_eq!(wire::read(&mut socket).unwrap().0, b'j');
            } else {
                assert!(broker.pending.contains_key(&request.id));
            }
        }
    }
    #[test]
    fn upgraded_broker_preserves_each_waiting_helpers_application_profile() {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut broker = Broker::start(root.path()).unwrap();
        broker.enable_application_environment();
        let mut output = Vec::new();
        let mut helpers = Vec::new();
        for expanded in [false, true] {
            if expanded {
                broker.enable_application_environment_v2();
            }
            let socket = UnixStream::connect(root.path().join("gui.sock")).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let request = protocol::SharedRequest {
                id: uuid::Uuid::new_v4(),
                terminal: uuid::Uuid::new_v4(),
                epoch: uuid::Uuid::new_v4(),
                application: "fixture".into(),
                session_bus: true,
                native_wayland: true,
                application_environment: true,
                application_environment_v2: expanded,
            };
            wire::write(&mut &socket, b'l', &protocol::encode(&request).unwrap()).unwrap();
            broker.poll(&mut output).unwrap();
            assert!(broker.pending.contains_key(&request.id));
            helpers.push((socket, request));
        }
        for (mut socket, request) in helpers {
            let mut environment = protocol::SharedGuiEnvironment {
                session: uuid::Uuid::new_v4(),
                display: None,
                authority: None,
                wayland_socket: "/tmp/owned/wayland-0".into(),
                fallback_bus: None,
                toolkit: BTreeMap::new(),
                application: [("GTK_A11Y".into(), Some("none".into()))].into(),
            };
            let reply = |environment| protocol::SharedReply {
                id: request.id,
                epoch: request.epoch,
                error: None,
                environment: Some(environment),
            };
            let sent = broker.reply(
                b'j',
                &protocol::encode(&reply(environment.clone())).unwrap(),
            );
            assert_eq!(sent.is_ok(), request.application_environment_v2);
            if !request.application_environment_v2 {
                assert!(broker.pending.contains_key(&request.id));
                request.project_application(&mut environment);
                assert!(environment.application.is_empty());
                broker
                    .reply(b'j', &protocol::encode(&reply(environment)).unwrap())
                    .unwrap();
            }
            assert_eq!(wire::read(&mut socket).unwrap().0, b'j');
            assert!(!broker.pending.contains_key(&request.id));
        }
    }
    #[test]
    fn accepted_helper_waits_for_its_first_write_without_inheriting_nonblocking_mode() {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut broker = Broker::start(root.path()).unwrap();
        let mut socket = UnixStream::connect(root.path().join("gui.sock")).unwrap();
        let request = protocol::Request {
            id: uuid::Uuid::new_v4(),
            terminal: uuid::Uuid::new_v4(),
            application: "fixture".into(),
        };
        let id = request.id;
        let client = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            wire::write(&mut socket, b'L', &protocol::encode(&request).unwrap()).unwrap();
            socket
        });
        let mut output = Vec::new();
        broker.poll(&mut output).unwrap();
        let socket = client.join().unwrap();
        broker.poll(&mut output).unwrap();
        let (kind, bytes) = wire::read(&mut &output[..]).unwrap();
        assert_eq!(kind, b'L');
        assert_eq!(
            protocol::decode::<protocol::Request>(&bytes).unwrap().id,
            id
        );
        drop(socket);
        output.clear();
        broker.poll_until(&mut output, |_, output| !output.is_empty());
        let (kind, bytes) = wire::read(&mut &output[..]).unwrap();
        assert_eq!(kind, b'K');
        assert_eq!(protocol::decode::<uuid::Uuid>(&bytes).unwrap(), id);
    }
    #[test]
    fn incomplete_helper_does_not_block_prompt_helper_or_lose_first_frame() {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut broker = Broker::start(root.path()).unwrap();
        let mut delayed = UnixStream::connect(root.path().join("gui.sock")).unwrap();
        let late = protocol::Request {
            id: uuid::Uuid::new_v4(),
            terminal: uuid::Uuid::new_v4(),
            application: "delayed".into(),
        };
        let late_id = late.id;
        let release = std::sync::Arc::new(std::sync::Barrier::new(2));
        let writer = release.clone();
        let client = std::thread::spawn(move || {
            // The broker has accepted this socket and processed another helper
            // before this writer is released. No scheduler timing assumption.
            writer.wait();
            let result = wire::write(&mut delayed, b'L', &protocol::encode(&late).unwrap());
            (delayed, result)
        });
        let mut output = Vec::new();
        broker.poll(&mut output).unwrap();
        assert!(output.is_empty());
        let mut prompt = UnixStream::connect(root.path().join("gui.sock")).unwrap();
        let request = protocol::Request {
            id: uuid::Uuid::new_v4(),
            terminal: uuid::Uuid::new_v4(),
            application: "prompt".into(),
        };
        wire::write(&mut prompt, b'L', &protocol::encode(&request).unwrap()).unwrap();
        broker.poll(&mut output).unwrap();
        let (kind, bytes) = wire::read(&mut output.as_slice()).unwrap();
        assert_eq!(kind, b'L');
        assert_eq!(
            protocol::decode::<protocol::Request>(&bytes).unwrap().id,
            request.id
        );
        release.wait();
        let (_delayed, result) = client.join().unwrap();
        assert!(result.is_ok(), "Delayed first frame was reset: {result:?}");
        output.clear();
        broker.poll(&mut output).unwrap();
        let (kind, bytes) = wire::read(&mut output.as_slice()).unwrap();
        assert_eq!(kind, b'L');
        assert_eq!(
            protocol::decode::<protocol::Request>(&bytes).unwrap().id,
            late_id
        );
    }
    #[test]
    fn desktop_discovery_excludes_terminal_apps_and_handles_quoted_exec() {
        assert_eq!(
            desktop_exec("[Desktop Entry]\nExec=env FLAG=yes \"/opt/My App/bin/app\" %U\n"),
            Some(vec!["/opt/My App/bin/app".into(), "%U".into()])
        );
        for flag in ["Terminal=true", "Hidden=true", "NoDisplay=true"] {
            assert!(desktop_exec(&format!("[Desktop Entry]\nExec=chromium\n{flag}\n")).is_none());
        }
        assert!(desktop_exec("[Desktop Action Other]\nExec=chromium\n").is_none());
        for name in [
            "xclip",
            "wl-paste",
            "xdg-open",
            "python3",
            "sh",
            "../chromium",
        ] {
            assert!(!command_name(name));
        }
    }
    #[test]
    fn packaged_commands_only_gate_registered_gui_launches() {
        let catalogue = Catalogue {
            flatpaks: HashSet::from(["org.newport.Fixture".into()]),
            snaps: HashSet::from(["chromium".into()]),
            ..Default::default()
        };
        for (program, args, expected) in [
            ("flatpak", vec!["run", "org.newport.Fixture"], true),
            (
                "flatpak",
                vec!["--user", "run", "org.newport.Fixture"],
                true,
            ),
            ("flatpak", vec!["run", "org.example.Cli"], false),
            ("flatpak", vec!["info", "org.newport.Fixture"], false),
            ("snap", vec!["run", "chromium"], true),
            ("snap", vec!["install", "chromium"], false),
            ("snap", vec!["run", "chromium", "--headless=new"], false),
            ("chromium", vec!["--version"], false),
            ("chromium", vec!["--app=https://example.com"], true),
        ] {
            let args: Vec<_> = args.into_iter().map(Into::into).collect();
            assert_eq!(
                needs_display(Path::new(program), &args, &catalogue),
                expected,
                "{program} {args:?}"
            );
        }
    }
}
