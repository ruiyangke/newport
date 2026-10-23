//! GUI command handoff. A waiting process keeps argv, cwd and stdio until exec.
use crate::{gui_protocol as protocol, wire};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::PermissionsExt,
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
    let mut dirs = vec![env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
        .join("applications")];
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
    if let Some(terminal) = terminal
        .filter(|_| needs_display(&program, arguments, &catalogue(&desktop_dirs(), &shim_dir)))
    {
        // A shell can become ready just before its independent integration transport.
        let connecting = Instant::now();
        let mut socket = loop {
            let connected = crate::routing::select(Some("gui-launch"))
                .and_then(|selection| UnixStream::connect(selection.root.join("gui.sock")));
            match connected {
                Ok(socket) => break socket,
                Err(error) if connecting.elapsed() >= Duration::from_secs(10) => {
                    return Err(io::Error::other(format!("GUI connection unavailable: {error}. Check the server connection in Newport and run the command again.")));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        };
        socket.set_write_timeout(Some(Duration::from_secs(2)))?;
        socket.set_read_timeout(Some(Duration::from_secs(protocol::WAIT_SECONDS)))?;
        let request = protocol::Request {
            id: uuid::Uuid::new_v4(),
            terminal,
            application: program
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        };
        wire::write(&mut socket, b'L', &protocol::encode(&request)?)?;
        let (kind, bytes) = wire::read_limited(&mut socket, protocol::MAX_MESSAGE)?;
        let reply: protocol::Reply = protocol::decode(&bytes)?;
        if kind != b'J' || reply.id != request.id {
            return Err(io::Error::other("Invalid GUI launch reply"));
        }
        let environment = reply.environment.ok_or_else(|| {
            io::Error::other(reply.error.unwrap_or_else(|| "GUI launch cancelled".into()))
        })?;
        validate_environment(&environment)?;
        for (name, value) in &environment {
            match value {
                Some(value) => {
                    command.env(name, value);
                }
                None => {
                    command.env_remove(name);
                }
            }
        }
        command.env(
            "NEWPORT_GUI_DISPLAY",
            environment["DISPLAY"].as_ref().unwrap(),
        );
        command.env(
            "NEWPORT_GUI_AUTHORITY",
            environment["XAUTHORITY"].as_ref().unwrap(),
        );
    }
    Err(command.exec())
}

fn validate_environment(values: &BTreeMap<String, Option<String>>) -> io::Result<()> {
    let allowed = |key: &str| {
        matches!(
            key,
            "DISPLAY"
                | "XAUTHORITY"
                | "WAYLAND_DISPLAY"
                | "WAYLAND_SOCKET"
                | "GDK_BACKEND"
                | "QT_QPA_PLATFORM"
                | "GDK_SCALE"
                | "GDK_DPI_SCALE"
                | "QT_FONT_DPI"
                | "QT_SCALE_FACTOR"
                | "QT_AUTO_SCREEN_SCALE_FACTOR"
                | "QT_ENABLE_HIGHDPI_SCALING"
                | "GTK2_RC_FILES"
                | "XMODIFIERS"
                | "GTK_IM_MODULE"
                | "QT_IM_MODULE"
                | "IBUS_USE_PORTAL"
                | "PULSE_SERVER"
                | "PULSE_SINK"
        )
    };
    let display = values
        .get("DISPLAY")
        .and_then(Option::as_deref)
        .unwrap_or_default();
    let authority = values
        .get("XAUTHORITY")
        .and_then(Option::as_deref)
        .unwrap_or_default();
    if values.len() > 24
        || values.iter().any(|(k, v)| {
            !allowed(k)
                || v.as_ref()
                    .is_some_and(|v| v.len() > 4096 || v.contains('\0'))
        })
        || !display.starts_with(':')
        || display.len() < 2
        || !display[1..].bytes().all(|b| b.is_ascii_digit())
        || !authority.starts_with('/')
        || values.get("WAYLAND_DISPLAY") != Some(&None)
        || values.get("WAYLAND_SOCKET") != Some(&None)
    {
        return Err(io::Error::other("Invalid GUI display environment"));
    }
    Ok(())
}

pub(crate) struct Broker {
    listener: UnixListener,
    _endpoint: crate::local_socket::Endpoint,
    pending: HashMap<uuid::Uuid, (UnixStream, Instant)>,
}
impl Broker {
    pub fn start(root: &Path) -> io::Result<Self> {
        let (listener, endpoint) = crate::local_socket::Endpoint::fixed(&root.join("gui.sock"))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            _endpoint: endpoint,
            pending: HashMap::new(),
        })
    }
    pub fn reply(&mut self, data: &[u8]) -> io::Result<()> {
        let reply: protocol::Reply = protocol::decode(data)?;
        if let Some((mut socket, _)) = self.pending.remove(&reply.id) {
            let _ = wire::write(&mut socket, b'J', data);
        }
        Ok(())
    }
    pub fn poll(&mut self, output: &mut impl Write) -> io::Result<()> {
        let ended: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, (socket, at))| {
                let mut byte = 0u8;
                let count = unsafe {
                    libc::recv(
                        socket.as_raw_fd(),
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
                    || at.elapsed() >= Duration::from_secs(protocol::WAIT_SECONDS))
                .then_some(*id)
            })
            .collect();
        for id in ended {
            self.pending.remove(&id);
            wire::write(output, b'K', &protocol::encode(&id)?)?;
        }
        if let Ok((mut socket, _)) = self.listener.accept() {
            socket.set_read_timeout(Some(Duration::from_millis(100)))?;
            socket.set_write_timeout(Some(Duration::from_millis(100)))?;
            let request =
                wire::read_limited(&mut socket, protocol::MAX_MESSAGE).and_then(|(kind, bytes)| {
                    if kind != b'L' {
                        return Err(io::Error::other("Invalid GUI launch frame"));
                    }
                    let request: protocol::Request = protocol::decode(&bytes)?;
                    request.validate()?;
                    Ok(request)
                });
            if let Ok(request) = request {
                if self.pending.len() < protocol::MAX_PENDING
                    && !self.pending.contains_key(&request.id)
                {
                    wire::write(output, b'L', &protocol::encode(&request)?)?;
                    self.pending.insert(request.id, (socket, Instant::now()));
                } else {
                    let reply = protocol::Reply {
                        id: request.id,
                        environment: None,
                        error: Some("Too many pending GUI launches. Try again.".into()),
                    };
                    let _ = wire::write(&mut socket, b'J', &protocol::encode(&reply)?);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
