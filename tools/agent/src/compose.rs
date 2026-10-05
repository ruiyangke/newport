//! Compose actions for the Containers tab: up, down, pull, build and scale
//! through the Compose CLI, because the Engine API does not understand
//! compose files. The CLI runs as the SSH user -- never sudo, never a shell --
//! in the project's working directory with `-p <project>` and one `-f` per
//! config file, both taken from the containers' Compose labels. Commands run
//! as background jobs in their own process group; their output is kept in
//! memory, capped, and read back by offset, one job per project at a time.
use crate::containers::valid_project;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    io::{ErrorKind, Read},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_SERVICES: usize = 50;
const MAX_SERVICE_NAME: usize = 64;
const MAX_REPLICAS: u32 = 20;
const MAX_FILES: usize = 20;
const MAX_FILE_TEXT: usize = 256 << 10;
const MAX_CONFIG_ERROR: usize = 4 << 10;
const MAX_CONFIG_OUTPUT: usize = 8 << 20;
/// File texts in one project reply, as serialized, under the desktop's 4 MiB.
const FILES_BUDGET: usize = 3 << 20;
const TOO_LARGE: &str = "Not shown: the project's files are too large to show together.";
/// A job's output: the newest 1 MiB is kept.
pub const OUTPUT_CAP: usize = 1 << 20;
/// Output may grow this far past the cap before the oldest bytes are dropped.
const OUTPUT_SLACK: usize = 64 << 10;
/// A line without an end is passed on once it reaches this length.
const MAX_LINE: usize = 64 << 10;
const MAX_FINISHED: usize = 20;
const KEEP_FINISHED: Duration = Duration::from_secs(15 * 60);
const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// A failed discovery is retried after this, so installing Compose is noticed.
const MISSING_TTL: Duration = Duration::from_secs(30);
/// Budgets under the desktop's 30 s for a project and 10 s for a run.
const PROJECT_BUDGET: Duration = Duration::from_secs(26);
const RUN_BUDGET: Duration = Duration::from_secs(8);
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const CONFIG_TIMEOUT: Duration = Duration::from_secs(20);

pub const CLI_MISSING: &str = "The Compose CLI isn't installed on this server.";
const UNKNOWN_JOB: &str = "That Compose command is no longer known; the agent may have restarted.";

fn no_containers(project: &str) -> String {
    format!("No containers remain for {project}, so Newport can't find its compose files.")
}
fn busy(project: &str) -> String {
    format!("Another Compose command is running for {project}.")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Compose's service-name rules, kept to what is safe as an argument.
pub fn valid_service(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SERVICE_NAME
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn services_arg(names: &[String]) -> Result<Vec<String>, String> {
    if names.len() > MAX_SERVICES {
        return Err(format!("Choose at most {MAX_SERVICES} services."));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for name in names {
        if !valid_service(name) {
            return Err(format!("Not a valid Compose service: {name}"));
        }
        if seen.insert(name.as_str()) {
            out.push(name.clone());
        }
    }
    Ok(out)
}

/// What to run, as the request names it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    Up {
        #[serde(default)]
        services: Vec<String>,
        #[serde(default)]
        build: bool,
        #[serde(default)]
        pull: bool,
        #[serde(default)]
        force_recreate: bool,
    },
    Down {
        #[serde(default)]
        volumes: bool,
        #[serde(default)]
        remove_orphans: bool,
    },
    Pull {
        #[serde(default)]
        services: Vec<String>,
    },
    Build {
        #[serde(default)]
        services: Vec<String>,
        #[serde(default)]
        no_cache: bool,
    },
    Scale {
        service: String,
        replicas: i64,
    },
}

impl Action {
    /// The action from a request, validated.
    pub fn parse(value: &Value) -> Result<Self, String> {
        let kind = match value.get("kind").and_then(Value::as_str) {
            Some(kind) => kind,
            None => return Err("Choose a Compose action.".into()),
        };
        if !matches!(kind, "up" | "down" | "pull" | "build" | "scale") {
            return Err(format!("Unknown Compose action: {kind}"));
        }
        if kind == "scale" && !value["replicas"].is_i64() && !value["replicas"].is_u64() {
            return Err(format!("Replicas must be 0 to {MAX_REPLICAS}."));
        }
        let action: Self = serde_json::from_value(value.clone())
            .map_err(|e| format!("Invalid Compose action: {e}"))?;
        action.validated()
    }

    fn validated(self) -> Result<Self, String> {
        Ok(match self {
            Self::Up {
                services,
                build,
                pull,
                force_recreate,
            } => Self::Up {
                services: services_arg(&services)?,
                build,
                pull,
                force_recreate,
            },
            Self::Pull { services } => Self::Pull {
                services: services_arg(&services)?,
            },
            Self::Build { services, no_cache } => Self::Build {
                services: services_arg(&services)?,
                no_cache,
            },
            Self::Scale { service, replicas } => {
                if !valid_service(&service) {
                    return Err(format!("Not a valid Compose service: {service}"));
                }
                if !(0..=MAX_REPLICAS as i64).contains(&replicas) {
                    return Err(format!("Replicas must be 0 to {MAX_REPLICAS}."));
                }
                Self::Scale { service, replicas }
            }
            down @ Self::Down { .. } => down,
        })
    }

    /// Everything but `down` needs the project's files.
    fn needs_files(&self) -> bool {
        !matches!(self, Self::Down { .. })
    }

    /// The subcommand and its arguments.
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        let mut push = |items: &[&str]| args.extend(items.iter().map(|s| (*s).to_owned()));
        match self {
            Self::Up {
                services,
                build,
                pull,
                force_recreate,
            } => {
                push(&["up", "-d"]);
                if *build {
                    push(&["--build"]);
                }
                if *pull {
                    push(&["--pull", "always"]);
                }
                if *force_recreate {
                    push(&["--force-recreate"]);
                }
                if !services.is_empty() {
                    push(&["--no-deps"]);
                }
                args.extend(services.iter().cloned());
            }
            Self::Down {
                volumes,
                remove_orphans,
            } => {
                push(&["down"]);
                if *volumes {
                    push(&["--volumes"]);
                }
                if *remove_orphans {
                    push(&["--remove-orphans"]);
                }
            }
            Self::Pull { services } => {
                push(&["pull"]);
                args.extend(services.iter().cloned());
            }
            Self::Build { services, no_cache } => {
                push(&["build"]);
                if *no_cache {
                    push(&["--no-cache"]);
                }
                args.extend(services.iter().cloned());
            }
            Self::Scale { service, replicas } => {
                push(&["up", "-d", "--no-deps", "--scale"]);
                args.push(format!("{service}={replicas}"));
                args.push(service.clone());
            }
        }
        args
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Plugin,
    Standalone,
    Podman,
}
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            Self::Standalone => "standalone",
            Self::Podman => "podman",
        }
    }
    fn binary(self) -> &'static str {
        match self {
            Self::Plugin => "docker",
            Self::Standalone => "docker-compose",
            Self::Podman => "podman",
        }
    }
    /// The arguments before Compose's own.
    fn prefix(self) -> &'static [&'static str] {
        match self {
            Self::Standalone => &[],
            Self::Plugin | Self::Podman => &["compose"],
        }
    }
    fn version_args(self) -> &'static [&'static str] {
        match self {
            Self::Plugin => &["compose", "version", "--short"],
            Self::Standalone => &["version", "--short"],
            Self::Podman => &["compose", "version"],
        }
    }
    /// Discovery order; a Podman engine tries `podman compose` first.
    pub fn order(podman: bool) -> [Self; 3] {
        if podman {
            [Self::Podman, Self::Plugin, Self::Standalone]
        } else {
            [Self::Plugin, Self::Standalone, Self::Podman]
        }
    }
}

/// A working Compose CLI.
#[derive(Clone, Debug, PartialEq)]
pub struct Cli {
    pub kind: Kind,
    pub version: String,
    pub program: PathBuf,
    /// Whether it takes the global `--progress plain`.
    pub progress: bool,
}
impl Cli {
    fn json(&self) -> Value {
        json!({"kind": self.kind.name(), "version": self.version})
    }
}

/// The first version-looking word, preferring lines that mention Compose
/// (`podman compose version` also prints Podman's own version).
pub fn parse_version(text: &str) -> Option<String> {
    let word = |line: &str| {
        line.split_whitespace().find_map(|token| {
            let token = token
                .trim_start_matches('v')
                .trim_end_matches([',', ';', ':']);
            let ok = token.starts_with(|c: char| c.is_ascii_digit())
                && token.contains('.')
                && token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c));
            ok.then(|| token.to_owned())
        })
    };
    let lines: Vec<&str> = text.lines().collect();
    lines
        .iter()
        .filter(|l| l.to_ascii_lowercase().contains("compose"))
        .chain(lines.iter())
        .find_map(|line| word(line))
}

/// An executable named `name` in the first of `dirs` that has one.
pub fn which(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|dir| dir.join(name)).find(|path| {
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// Where the CLI is looked for: PATH, then the usual places a non-login
/// SSH command's PATH may miss.
fn system_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    for dir in ["/usr/local/bin", "/usr/bin", "/bin", "/snap/bin"] {
        dirs.push(dir.into());
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/bin"));
        dirs.push(Path::new(&home).join("bin"));
    }
    dirs.retain(|d| d.is_absolute());
    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

/// The environment every Compose process gets: plain progress, no colour,
/// the C locale, and the engine the project was found on.
pub fn environment(socket: Option<&Path>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = [
        ("COMPOSE_ANSI", "never"),
        ("BUILDKIT_PROGRESS", "plain"),
        ("COMPOSE_PROGRESS", "plain"),
        ("LC_ALL", "C"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect();
    if let Some(socket) = socket {
        env.push(("DOCKER_HOST".into(), format!("unix://{}", socket.display())));
    }
    env
}

/// A command ready to run: no shell, every argument separate.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    /// What the person sees, e.g. `docker compose -p app up -d`.
    pub display: String,
}

/// The command line for `action`, before or without files (`down` with
/// missing files names the project only).
pub fn plan(
    cli: &Cli,
    project: &str,
    files: &[String],
    cwd: &Path,
    socket: Option<&Path>,
    action: &Action,
) -> Plan {
    let mut args: Vec<String> = cli.kind.prefix().iter().map(|s| (*s).to_owned()).collect();
    if cli.progress {
        args.extend(["--progress".into(), "plain".into()]);
    }
    args.extend(["-p".into(), project.to_owned()]);
    for file in files {
        args.extend(["-f".into(), file.clone()]);
    }
    let action_args = action.args();
    args.extend(action_args.iter().cloned());
    let mut display = vec![cli.kind.binary().to_owned()];
    display.extend(cli.kind.prefix().iter().map(|s| (*s).to_owned()));
    display.extend(["-p".into(), project.to_owned()]);
    display.extend(action_args);
    Plan {
        program: cli.program.clone(),
        args,
        cwd: cwd.to_owned(),
        env: environment(socket),
        display: display.join(" "),
    }
}

fn command(plan: &Plan) -> Command {
    let mut command = Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if plan.env.iter().any(|(k, _)| k == "DOCKER_HOST") {
        // An explicit host and a context together confuse the CLI.
        command.env_remove("DOCKER_CONTEXT");
    }
    for (key, value) in &plan.env {
        command.env(key, value);
    }
    command
}

fn signal_group(pgid: i32, signal: i32) {
    if pgid > 0 {
        // SAFETY: kill has no memory-safety preconditions; a negative pid
        // names the process group this agent created for the command.
        unsafe {
            libc::kill(-pgid, signal);
        }
    }
}

fn spawn_error(program: &Path, error: &std::io::Error) -> String {
    match error.kind() {
        ErrorKind::NotFound => CLI_MISSING.into(),
        _ => format!("Couldn't start {}: {error}", program.display()),
    }
}

/// Reads `pipe` to the end, keeping at most `cap` bytes.
fn drain(mut pipe: impl Read, cap: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len()).min(n);
                kept.extend_from_slice(&buffer[..room]);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    kept
}

/// A finished short command (a probe or `config`).
#[derive(Debug)]
struct Captured {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

fn capture(plan: &Plan, timeout: Duration, cap: usize) -> Result<Captured, String> {
    let mut child = command(plan)
        .spawn()
        .map_err(|e| spawn_error(&plan.program, &e))?;
    let pgid = child.id() as i32;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || drain(stdout, cap));
    let err = std::thread::spawn(move || drain(stderr, cap));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                timed_out = true;
                break None;
            }
            Err(_) => break None,
        }
    };
    // Descendants holding the pipes go too, so the readers finish.
    signal_group(pgid, libc::SIGKILL);
    let status = status.or_else(|| child.wait().ok());
    Ok(Captured {
        code: status.and_then(|s| s.code()),
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
        timed_out,
    })
}

/// Cuts `text` to at most `cap` bytes on a character boundary.
fn cut(text: &str, cap: usize) -> &str {
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A config file's contents for the project view.
pub fn read_file(path: &str) -> Value {
    let reason = |e: &std::io::Error| match e.kind() {
        ErrorKind::NotFound => "No such file".to_owned(),
        ErrorKind::PermissionDenied => "Permission denied".to_owned(),
        _ => e.to_string(),
    };
    let opened = std::fs::File::open(path).and_then(|file| {
        if file.metadata()?.is_dir() {
            return Err(std::io::Error::other("Not a file"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE_TEXT as u64 + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    match opened {
        Ok(mut bytes) => {
            let truncated = bytes.len() > MAX_FILE_TEXT;
            if truncated {
                let mut end = MAX_FILE_TEXT;
                // Never split a UTF-8 character.
                while end > 0 && bytes[end] & 0xC0 == 0x80 {
                    end -= 1;
                }
                bytes.truncate(end);
            }
            json!({
                "path": path,
                "text": String::from_utf8_lossy(&bytes),
                "error": truncated.then_some("Truncated at 256 KiB"),
            })
        }
        Err(e) => json!({"path": path, "text": null, "error": reason(&e)}),
    }
}

/// Files in order while their serialized texts fit the reply; the rest are
/// listed without text.
fn within_budget(mut files: Vec<Value>) -> Vec<Value> {
    let mut used = 0;
    for file in &mut files {
        used += file["text"].to_string().len();
        if used > FILES_BUDGET {
            file["text"] = Value::Null;
            file["error"] = TOO_LARGE.into();
        }
    }
    files
}

/// Why a file cannot be used for `up`, `pull`, `build` or `scale`.
fn unreadable(path: &str) -> Option<String> {
    match read_file(path)["error"].as_str() {
        None | Some("Truncated at 256 KiB") => None,
        Some(reason) => Some(format!("Newport can't read {path}: {reason}.")),
    }
}

/// Services from `config --format json`, by name.
pub fn services(config: &Value) -> Option<Vec<Value>> {
    let map = config["services"].as_object()?;
    let mut rows: Vec<(String, Value)> = map
        .iter()
        .map(|(name, service)| {
            let profiles: Vec<&str> = service["profiles"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let image = service["image"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let row = json!({
                "name": name,
                "image": image,
                "build": !service["build"].is_null(),
                "profiles": profiles,
            });
            (name.clone(), row)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    Some(rows.into_iter().map(|(_, row)| row).collect())
}

/// Where a project's files are, from its containers' labels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub working_dir: Option<String>,
    pub config_files: Vec<String>,
}

/// The labels of the newest container that carries them (`rows` come newest
/// first); one-off containers only when no other has them.
pub fn files(rows: &[Value]) -> Files {
    let label = |row: &Value, key: &str| {
        row["Labels"][key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let oneoff = |row: &Value| {
        label(row, "com.docker.compose.oneoff").is_some_and(|v| v.eq_ignore_ascii_case("true"))
    };
    let ordered: Vec<&Value> = rows
        .iter()
        .filter(|r| !oneoff(r))
        .chain(rows.iter().filter(|r| oneoff(r)))
        .collect();
    let working_dir = ordered
        .iter()
        .find_map(|row| label(row, "com.docker.compose.project.working_dir"))
        .filter(|dir| dir.starts_with('/'));
    let listed = ordered
        .iter()
        .find_map(|row| label(row, "com.docker.compose.project.config_files"))
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let config_files = listed
        .split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .filter_map(|file| {
            if file.starts_with('/') {
                Some(file.to_owned())
            } else {
                // Older Compose recorded paths relative to the project.
                let dir = working_dir.as_deref()?;
                Some(Path::new(dir).join(file).to_string_lossy().into_owned())
            }
        })
        .filter(|f| seen.insert(f.clone()))
        .take(MAX_FILES)
        .collect();
    Files {
        working_dir,
        config_files,
    }
}

/// Plain lines from a stream: ANSI escapes and control characters removed,
/// carriage-return rewrites split into lines of their own.
#[derive(Debug, Default)]
pub struct Lines {
    current: Vec<u8>,
    after_cr: bool,
}
impl Lines {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &byte in bytes {
            match byte {
                b'\n' => {
                    // "\r\n" ends the line the "\r" already passed on.
                    if !(self.current.is_empty() && self.after_cr) {
                        out.push(clean(&self.current));
                    }
                    self.current.clear();
                    self.after_cr = false;
                }
                b'\r' => {
                    if !self.current.is_empty() {
                        out.push(clean(&self.current));
                        self.current.clear();
                    }
                    self.after_cr = true;
                }
                _ => {
                    self.after_cr = false;
                    self.current.push(byte);
                    if self.current.len() >= MAX_LINE {
                        out.push(clean(&self.current));
                        self.current.clear();
                    }
                }
            }
        }
        out
    }
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.current);
        (!rest.is_empty()).then(|| clean(&rest))
    }
}

/// One line without escape sequences or control characters (tabs stay).
pub fn clean(line: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if line[i] != 0x1B {
            bytes.push(line[i]);
            i += 1;
            continue;
        }
        i += 1;
        match line.get(i) {
            // CSI: parameters, intermediates, one final byte.
            Some(b'[') => {
                i += 1;
                while i < line.len() && !(0x40..=0x7E).contains(&line[i]) {
                    i += 1;
                }
                i += 1;
            }
            // OSC: up to BEL or ST.
            Some(b']') => {
                i += 1;
                while i < line.len() {
                    if line[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if line[i] == 0x1B && line.get(i + 1) == Some(&b'\\') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            // Other escapes: intermediates, then one final byte.
            Some(_) => {
                while i < line.len() && (0x20..=0x2F).contains(&line[i]) {
                    i += 1;
                }
                i += 1;
            }
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes)
        .chars()
        .filter(|c| *c == '\t' || !c.is_control())
        .collect()
}

/// The newest output of a job, addressed by byte offset from its start.
#[derive(Debug, Default)]
pub struct Output {
    data: Vec<u8>,
    /// The offset of `data[0]`.
    start: u64,
    cap: usize,
}
impl Output {
    pub fn new(cap: usize) -> Self {
        Self {
            data: Vec::new(),
            start: 0,
            cap,
        }
    }
    pub fn end(&self) -> u64 {
        self.start + self.data.len() as u64
    }
    pub fn push_line(&mut self, line: &str) {
        self.data.extend_from_slice(line.as_bytes());
        self.data.push(b'\n');
        if self.data.len() > self.cap + OUTPUT_SLACK.min(self.cap) {
            self.trim();
        }
    }
    fn trim(&mut self) {
        let len = self.data.len();
        let mut drop = len.saturating_sub(self.cap);
        if drop > 0 && self.data[drop - 1] != b'\n' {
            // Whole lines where one ends in what is kept, else whole characters.
            match self.data[drop..len - 1].iter().position(|b| *b == b'\n') {
                Some(end) => drop += end + 1,
                None => {
                    while drop < len && self.data[drop] & 0xC0 == 0x80 {
                        drop += 1;
                    }
                }
            }
        }
        self.data.drain(..drop);
        self.start += drop as u64;
    }
    /// Output from `offset` (or the oldest kept byte), and the next offset.
    pub fn read(&mut self, offset: u64) -> (String, u64) {
        if self.data.len() > self.cap {
            self.trim();
        }
        let end = self.end();
        let mut index = (offset.clamp(self.start, end) - self.start) as usize;
        while index < self.data.len() && self.data[index] & 0xC0 == 0x80 {
            index += 1;
        }
        (
            String::from_utf8_lossy(&self.data[index..]).into_owned(),
            end,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}
impl State {
    fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug)]
struct Progress {
    state: State,
    exit_code: Option<i32>,
    finished_at: Option<i64>,
    finished: Option<Instant>,
    cancelled: bool,
    /// The process itself has exited (its group may be finishing).
    exited: bool,
    output: Output,
}

#[derive(Debug)]
pub struct Job {
    id: String,
    project: String,
    command: String,
    started_at: i64,
    pgid: i32,
    progress: Mutex<Progress>,
}
impl Job {
    fn append(&self, lines: Vec<String>) {
        let mut progress = self.progress.lock().unwrap();
        for line in lines {
            progress.output.push_line(&line);
        }
    }
    fn running(&self) -> bool {
        self.progress.lock().unwrap().state == State::Running
    }
    fn json(&self, offset: Option<u64>) -> Value {
        let mut progress = self.progress.lock().unwrap();
        let (output, offset) = match offset {
            Some(offset) => progress.output.read(offset),
            None => (String::new(), 0),
        };
        json!({
            "job": self.id,
            "project": self.project,
            "command": self.command,
            "state": progress.state.name(),
            "exitCode": progress.exit_code,
            "startedAt": self.started_at,
            "finishedAt": progress.finished_at,
            "output": output,
            "offset": offset,
        })
    }
    fn finish(&self, status: Option<ExitStatus>) {
        let mut progress = self.progress.lock().unwrap();
        let code = status.and_then(|s| s.code());
        progress.state = if progress.cancelled {
            State::Cancelled
        } else if code == Some(0) {
            State::Succeeded
        } else {
            State::Failed
        };
        progress.exit_code = code;
        progress.finished_at = Some(now_ms());
        progress.finished = Some(Instant::now());
    }
}

fn pump(job: Arc<Job>, pipe: impl Read + Send + 'static) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut pipe = pipe;
        let mut lines = Lines::default();
        let mut buffer = [0; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let complete = lines.feed(&buffer[..n]);
                    if !complete.is_empty() {
                        job.append(complete);
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        if let Some(rest) = lines.finish() {
            job.append(vec![rest]);
        }
    })
}

/// Waits for the process, then its output, then records how it ended.
fn watch(job: Arc<Job>, mut child: Child, readers: Vec<JoinHandle<()>>) {
    std::thread::spawn(move || {
        let status = child.wait().ok();
        job.progress.lock().unwrap().exited = true;
        // Anything it left behind in its group goes too, closing the pipes.
        signal_group(job.pgid, libc::SIGKILL);
        for reader in readers {
            let _ = reader.join();
        }
        job.finish(status);
    });
}

/// What the containers module found for a project on the engine.
pub struct Found {
    pub socket: Option<PathBuf>,
    pub podman: bool,
    /// The project's containers as the engine lists them, newest first.
    pub rows: Vec<Value>,
}

pub type Finder<'a> = &'a dyn Fn(&str, Instant) -> Result<Found, String>;

pub struct Compose {
    dirs: Box<dyn Fn() -> Vec<PathBuf> + Send + Sync>,
    cli: Mutex<Option<(Result<Cli, String>, Instant)>>,
    jobs: Mutex<Vec<Arc<Job>>>,
    grace: Duration,
    keep: Duration,
    output_cap: usize,
}

impl Compose {
    pub fn system() -> Self {
        Self::new(Box::new(system_dirs))
    }
    pub fn new(dirs: Box<dyn Fn() -> Vec<PathBuf> + Send + Sync>) -> Self {
        Self {
            dirs,
            cli: Mutex::new(None),
            jobs: Mutex::new(Vec::new()),
            grace: CANCEL_GRACE,
            keep: KEEP_FINISHED,
            output_cap: OUTPUT_CAP,
        }
    }

    /// The first CLI that answers, cached for this agent process (a missing
    /// one is looked for again after a while).
    pub fn cli(&self, podman: bool, deadline: Instant) -> Result<Cli, String> {
        let mut cached = self.cli.lock().unwrap();
        match &*cached {
            Some((Ok(cli), _)) if cli.program.exists() => return Ok(cli.clone()),
            Some((Err(error), at)) if at.elapsed() < MISSING_TTL => return Err(error.clone()),
            _ => {}
        }
        let dirs = (self.dirs)();
        let found = Kind::order(podman)
            .into_iter()
            .find_map(|kind| probe(kind, &dirs, deadline))
            .ok_or_else(|| CLI_MISSING.to_owned());
        // Running out of time proves nothing about what is installed.
        if found.is_ok() || time_left(deadline, PROBE_TIMEOUT).is_some() {
            *cached = Some((found.clone(), Instant::now()));
        }
        found
    }

    fn purge(&self) {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.retain(|job| {
            job.progress
                .lock()
                .unwrap()
                .finished
                .is_none_or(|at| at.elapsed() < self.keep)
        });
        let mut finished = jobs.iter().filter(|j| !j.running()).count();
        // Oldest first: the list is in start order.
        jobs.retain(|job| {
            if finished > MAX_FINISHED && !job.running() {
                finished -= 1;
                false
            } else {
                true
            }
        });
    }

    fn find(&self, request: &Value) -> Result<Arc<Job>, String> {
        let id = request["job"].as_str().ok_or("Choose a Compose command.")?;
        self.jobs
            .lock()
            .unwrap()
            .iter()
            .find(|job| job.id == id)
            .cloned()
            .ok_or_else(|| UNKNOWN_JOB.to_owned())
    }

    /// Starts `plan` as a job unless the project already has one running.
    pub fn start(&self, project: &str, plan: &Plan) -> Result<String, String> {
        self.purge();
        let mut jobs = self.jobs.lock().unwrap();
        if jobs
            .iter()
            .any(|job| job.project == project && job.running())
        {
            return Err(busy(project));
        }
        let mut child = command(plan)
            .spawn()
            .map_err(|e| spawn_error(&plan.program, &e))?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let job = Arc::new(Job {
            id: id.clone(),
            project: project.to_owned(),
            command: plan.display.clone(),
            started_at: now_ms(),
            pgid: child.id() as i32,
            progress: Mutex::new(Progress {
                state: State::Running,
                exit_code: None,
                finished_at: None,
                finished: None,
                cancelled: false,
                exited: false,
                output: Output::new(self.output_cap),
            }),
        });
        let readers = vec![
            pump(job.clone(), child.stdout.take().expect("piped")),
            pump(job.clone(), child.stderr.take().expect("piped")),
        ];
        watch(job.clone(), child, readers);
        jobs.push(job);
        Ok(id)
    }

    pub fn job(&self, request: &Value) -> Result<Value, String> {
        let offset = match &request["offset"] {
            Value::Null => 0,
            value => value.as_u64().ok_or("Invalid output offset.")?,
        };
        self.purge();
        Ok(self.find(request)?.json(Some(offset)))
    }

    pub fn jobs(&self) -> Value {
        self.purge();
        let jobs = self.jobs.lock().unwrap();
        let listed: Vec<Value> = jobs.iter().rev().map(|job| job.json(None)).collect();
        json!({"jobs": listed})
    }

    /// SIGTERM to the job's process group, SIGKILL after the grace period.
    pub fn cancel(&self, request: &Value) -> Result<Value, String> {
        let job = self.find(request)?;
        {
            let mut progress = job.progress.lock().unwrap();
            if progress.state != State::Running || progress.exited || progress.cancelled {
                return Ok(json!({"ok": true}));
            }
            progress.cancelled = true;
        }
        signal_group(job.pgid, libc::SIGTERM);
        let grace = self.grace;
        std::thread::spawn(move || {
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                if job.progress.lock().unwrap().exited {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if !job.progress.lock().unwrap().exited {
                signal_group(job.pgid, libc::SIGKILL);
            }
        });
        Ok(json!({"ok": true}))
    }

    /// Waits up to `limit` for running jobs, so a closed channel does not
    /// cut a command off half-way.
    pub fn wait_idle(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline && self.jobs.lock().unwrap().iter().any(|j| j.running()) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn project(&self, request: &Value, find: Finder) -> Result<Value, String> {
        let project = project_arg(&request["project"])?;
        let deadline = Instant::now() + PROJECT_BUDGET;
        let found = find(project, deadline)?;
        if found.rows.is_empty() {
            return Err(no_containers(project));
        }
        let files = files(&found.rows);
        let cli = self.cli(found.podman, deadline);
        let read = within_budget(files.config_files.iter().map(|f| read_file(f)).collect());
        let (services, config_error) = match &cli {
            Err(_) => (Value::Null, Value::Null),
            Ok(_) if files.config_files.is_empty() => (
                Value::Null,
                json!("Compose didn't record this project's files."),
            ),
            Ok(cli) => match config(cli, project, &files, found.socket.as_deref(), deadline) {
                Ok(services) => (json!(services), Value::Null),
                Err(error) => (Value::Null, json!(error)),
            },
        };
        Ok(json!({
            "cli": cli.as_ref().ok().map(Cli::json),
            "cliError": cli.as_ref().err(),
            "workingDir": files.working_dir,
            "configFiles": files.config_files,
            "files": read,
            "services": services,
            "configError": config_error,
        }))
    }

    pub fn run(&self, request: &Value, find: Finder) -> Result<Value, String> {
        let project = project_arg(&request["project"])?;
        let action = Action::parse(&request["action"])?;
        let deadline = Instant::now() + RUN_BUDGET;
        let found = find(project, deadline)?;
        let files = if found.rows.is_empty() {
            if action.needs_files() {
                return Err(no_containers(project));
            }
            Files::default()
        } else {
            files(&found.rows)
        };
        let readable = files.config_files.iter().find_map(|f| unreadable(f));
        let config_files = if action.needs_files() {
            if files.config_files.is_empty() {
                return Err(format!("Compose didn't record the files of {project}."));
            }
            if let Some(error) = readable {
                return Err(error);
            }
            files.config_files.clone()
        } else if readable.is_none() {
            files.config_files.clone()
        } else {
            // `down` works from the project's name alone.
            Vec::new()
        };
        let cwd = working_dir(&files);
        let cli = self.cli(found.podman, deadline)?;
        let plan = plan(
            &cli,
            project,
            &config_files,
            &cwd,
            found.socket.as_deref(),
            &action,
        );
        let job = self.start(project, &plan)?;
        Ok(json!({"job": job}))
    }

    /// Answers a Compose request, or None when the op is not one of ours.
    pub fn handle(&self, request: &Value, find: Finder) -> Option<Result<Value, String>> {
        Some(match request.get("op").and_then(Value::as_str)? {
            "compose_project" => self.project(request, find),
            "compose_run" => self.run(request, find),
            "compose_job" => self.job(request),
            "compose_cancel" => self.cancel(request),
            "compose_jobs" => Ok(self.jobs()),
            _ => return None,
        })
    }
}

fn project_arg(value: &Value) -> Result<&str, String> {
    match value.as_str() {
        Some(p) if valid_project(p) => Ok(p),
        Some(p) => Err(format!("Not a valid Compose project: {p}")),
        None => Err("Choose a Compose project.".into()),
    }
}

/// The project's directory when it still exists, else home, else `/`.
fn working_dir(files: &Files) -> PathBuf {
    files
        .working_dir
        .as_ref()
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .filter(|d| d.is_dir())
        .unwrap_or_else(|| "/".into())
}

fn time_left(deadline: Instant, cap: Duration) -> Option<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    (left >= Duration::from_millis(500)).then(|| left.min(cap))
}

fn probe(kind: Kind, dirs: &[PathBuf], deadline: Instant) -> Option<Cli> {
    let program = which(kind.binary(), dirs)?;
    let short = |args: &[&str]| Plan {
        program: program.clone(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        cwd: "/".into(),
        env: environment(None),
        display: String::new(),
    };
    let ran = capture(
        &short(kind.version_args()),
        time_left(deadline, PROBE_TIMEOUT)?,
        64 << 10,
    )
    .ok()?;
    if ran.code != Some(0) {
        return None;
    }
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    let version = parse_version(&text)?;
    let progress = kind != Kind::Podman && {
        let mut help: Vec<&str> = kind.prefix().to_vec();
        help.push("--help");
        time_left(deadline, PROBE_TIMEOUT)
            .and_then(|timeout| capture(&short(&help), timeout, 256 << 10).ok())
            .is_some_and(|h| String::from_utf8_lossy(&h.stdout).contains("--progress"))
    };
    Some(Cli {
        kind,
        version,
        program,
        progress,
    })
}

/// The services as Compose resolves them, every profile included, or the
/// CLI's own words for why it couldn't.
fn config(
    cli: &Cli,
    project: &str,
    files: &Files,
    socket: Option<&Path>,
    deadline: Instant,
) -> Result<Vec<Value>, String> {
    let timeout = time_left(deadline, CONFIG_TIMEOUT)
        .ok_or("Newport ran out of time before Compose could read the project.")?;
    let mut args: Vec<String> = cli.kind.prefix().iter().map(|s| (*s).to_owned()).collect();
    args.extend(["-p".into(), project.to_owned()]);
    for file in &files.config_files {
        args.extend(["-f".into(), file.clone()]);
    }
    args.extend(["--profile", "*", "config", "--format", "json"].map(String::from));
    let plan = Plan {
        program: cli.program.clone(),
        args,
        cwd: working_dir(files),
        env: environment(socket),
        display: String::new(),
    };
    let ran = capture(&plan, timeout, MAX_CONFIG_OUTPUT)?;
    if ran.timed_out {
        return Err("Compose didn't finish reading the project in time.".into());
    }
    if ran.code != Some(0) {
        let stderr = String::from_utf8_lossy(&ran.stderr);
        let stdout = String::from_utf8_lossy(&ran.stdout);
        let words = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        let words = clean(words.as_bytes());
        return Err(if words.is_empty() {
            format!(
                "Compose couldn't read the project (exit status {}).",
                ran.code.map_or("unknown".into(), |c| c.to_string())
            )
        } else {
            cut(words.trim(), MAX_CONFIG_ERROR).to_owned()
        });
    }
    serde_json::from_slice::<Value>(&ran.stdout)
        .ok()
        .as_ref()
        .and_then(services)
        .ok_or_else(|| "Compose printed a configuration Newport couldn't read.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn cli(kind: Kind, progress: bool) -> Cli {
        Cli {
            kind,
            version: "2.29.1".into(),
            program: format!("/usr/bin/{}", kind.binary()).into(),
            progress,
        }
    }
    fn up(services: &[&str], build: bool, pull: bool, force: bool) -> Action {
        Action::Up {
            services: services.iter().map(|s| (*s).to_owned()).collect(),
            build,
            pull,
            force_recreate: force,
        }
    }

    #[test]
    fn every_action_builds_its_own_argument_list() {
        let files = vec![
            "/srv/app/compose.yaml".to_owned(),
            "/srv/app/prod.yaml".to_owned(),
        ];
        let socket = Path::new("/run/user/1000/docker.sock");
        let plugin = cli(Kind::Plugin, true);
        let p = plan(
            &plugin,
            "app",
            &files,
            Path::new("/srv/app"),
            Some(socket),
            &up(&[], false, true, false),
        );
        assert_eq!(p.program, Path::new("/usr/bin/docker"));
        assert_eq!(
            p.args,
            [
                "compose",
                "--progress",
                "plain",
                "-p",
                "app",
                "-f",
                "/srv/app/compose.yaml",
                "-f",
                "/srv/app/prod.yaml",
                "up",
                "-d",
                "--pull",
                "always"
            ]
        );
        assert_eq!(p.cwd, Path::new("/srv/app"));
        assert_eq!(p.display, "docker compose -p app up -d --pull always");
        assert_eq!(
            p.env,
            [
                ("COMPOSE_ANSI", "never"),
                ("BUILDKIT_PROGRESS", "plain"),
                ("COMPOSE_PROGRESS", "plain"),
                ("LC_ALL", "C"),
                ("DOCKER_HOST", "unix:///run/user/1000/docker.sock"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
        );

        let args = |action: &Action| action.args().join(" ");
        assert_eq!(args(&up(&[], false, false, false)), "up -d");
        assert_eq!(
            args(&up(&["web", "db"], true, true, true)),
            "up -d --build --pull always --force-recreate --no-deps web db"
        );
        assert_eq!(
            args(&Action::Down {
                volumes: false,
                remove_orphans: false
            }),
            "down"
        );
        assert_eq!(
            args(&Action::Down {
                volumes: true,
                remove_orphans: true
            }),
            "down --volumes --remove-orphans"
        );
        assert_eq!(args(&Action::Pull { services: vec![] }), "pull");
        assert_eq!(
            args(&Action::Pull {
                services: vec!["web".into()]
            }),
            "pull web"
        );
        assert_eq!(
            args(&Action::Build {
                services: vec!["api".into()],
                no_cache: true
            }),
            "build --no-cache api"
        );
        assert_eq!(
            args(&Action::Build {
                services: vec![],
                no_cache: false
            }),
            "build"
        );
        assert_eq!(
            args(&Action::Scale {
                service: "web".into(),
                replicas: 3
            }),
            "up -d --no-deps --scale web=3 web"
        );

        // The standalone CLI has no prefix; Podman's takes no --progress;
        // `down` without readable files names the project only.
        let down = Action::Down {
            volumes: true,
            remove_orphans: false,
        };
        let standalone = plan(
            &cli(Kind::Standalone, false),
            "app",
            &[],
            Path::new("/"),
            None,
            &down,
        );
        assert_eq!(standalone.args, ["-p", "app", "down", "--volumes"]);
        assert_eq!(standalone.display, "docker-compose -p app down --volumes");
        assert!(standalone.env.iter().all(|(k, _)| k != "DOCKER_HOST"));
        let podman = plan(
            &cli(Kind::Podman, false),
            "app",
            &files[..1],
            Path::new("/srv/app"),
            None,
            &Action::Pull { services: vec![] },
        );
        assert_eq!(
            podman.args,
            [
                "compose",
                "-p",
                "app",
                "-f",
                "/srv/app/compose.yaml",
                "pull"
            ]
        );
        assert_eq!(podman.display, "podman compose -p app pull");
    }

    #[test]
    fn actions_are_validated() {
        let parse = |v: Value| Action::parse(&v);
        assert_eq!(
            parse(
                json!({"kind": "up", "services": ["web", "web", "db"], "build": true, "pull": false, "forceRecreate": true})
            ),
            Ok(up(&["web", "db"], true, false, true))
        );
        // Missing flags are off.
        assert_eq!(
            parse(json!({"kind": "up"})),
            Ok(up(&[], false, false, false))
        );
        assert_eq!(
            parse(json!({"kind": "down", "volumes": true, "removeOrphans": false})),
            Ok(Action::Down {
                volumes: true,
                remove_orphans: false
            })
        );
        assert_eq!(
            parse(json!({"kind": "scale", "service": "web", "replicas": 0})),
            Ok(Action::Scale {
                service: "web".into(),
                replicas: 0
            })
        );
        for good in ["web", "my_app.web-1", "X", &"a".repeat(64)] {
            assert!(valid_service(good), "{good}");
        }
        for bad in [
            "", "-d", "--build", ".x", "_x", "a b", "a/b", "a;b", "$(id)", "a\nb", "é",
        ] {
            assert!(!valid_service(bad), "{bad:?}");
        }
        assert!(!valid_service(&"a".repeat(65)));
        assert_eq!(
            parse(json!({"kind": "up", "services": ["--force"]})).unwrap_err(),
            "Not a valid Compose service: --force"
        );
        assert_eq!(
            parse(json!({"kind": "build", "services": ["a b"]})).unwrap_err(),
            "Not a valid Compose service: a b"
        );
        assert_eq!(
            parse(json!({"kind": "scale", "service": "-x", "replicas": 1})).unwrap_err(),
            "Not a valid Compose service: -x"
        );
        let many: Vec<String> = (0..51).map(|i| format!("s{i}")).collect();
        assert_eq!(
            parse(json!({"kind": "pull", "services": many})).unwrap_err(),
            "Choose at most 50 services."
        );
        assert!(parse(json!({"kind": "pull", "services": &many[..50]})).is_ok());
        for replicas in [json!(21), json!(-1), json!(1.5), json!("2"), Value::Null] {
            assert_eq!(
                parse(json!({"kind": "scale", "service": "web", "replicas": replicas}))
                    .unwrap_err(),
                "Replicas must be 0 to 20.",
                "{replicas}"
            );
        }
        assert!(parse(json!({"kind": "scale", "service": "web", "replicas": 20})).is_ok());
        assert_eq!(
            parse(json!({"kind": "restart"})).unwrap_err(),
            "Unknown Compose action: restart"
        );
        assert_eq!(parse(json!({})).unwrap_err(), "Choose a Compose action.");
        assert!(parse(json!({"kind": "up", "build": "yes"}))
            .unwrap_err()
            .starts_with("Invalid Compose action: "));
        assert!(parse(json!({"kind": "up", "services": "web"})).is_err());
    }

    #[test]
    fn labels_name_the_working_directory_and_files() {
        let row = |dir: &str, files: &str, oneoff: bool| {
            json!({"Labels": {
                "com.docker.compose.project.working_dir": dir,
                "com.docker.compose.project.config_files": files,
                "com.docker.compose.oneoff": if oneoff { "True" } else { "False" },
            }})
        };
        assert_eq!(
            files(&[
                row("/srv/job", "/srv/job/compose.yaml", true),
                row(
                    "/srv/app",
                    "/srv/app/compose.yaml, override.yaml,,/srv/app/compose.yaml",
                    false
                ),
                row("/srv/old", "/srv/old/compose.yaml", false),
            ]),
            Files {
                working_dir: Some("/srv/app".into()),
                config_files: vec![
                    "/srv/app/compose.yaml".into(),
                    "/srv/app/override.yaml".into()
                ],
            }
        );
        // Only one-off containers left: theirs will do.
        assert_eq!(
            files(&[row("/srv/job", "/srv/job/compose.yaml", true)]).working_dir,
            Some("/srv/job".into())
        );
        assert_eq!(files(&[json!({"Labels": {}})]), Files::default());
        // A relative directory is not trusted; relative files need one.
        assert_eq!(
            files(&[row("srv", "compose.yaml", false)]),
            Files::default()
        );
    }

    #[test]
    fn config_services_and_files_for_the_project_view() {
        let config = json!({"services": {
            "web": {"image": "nginx:1.27", "networks": {"default": null}},
            "api": {"build": {"context": "/srv/app"}, "image": "app/api"},
            "tool": {"build": {"context": "."}, "profiles": ["debug", "ops"]},
        }});
        assert_eq!(
            services(&config).unwrap(),
            vec![
                json!({"name": "api", "image": "app/api", "build": true, "profiles": []}),
                json!({"name": "tool", "image": null, "build": true, "profiles": ["debug", "ops"]}),
                json!({"name": "web", "image": "nginx:1.27", "build": false, "profiles": []}),
            ]
        );
        assert_eq!(services(&json!({"name": "x"})), None);

        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("compose.yaml");
        fs::write(&small, "services: {}\n").unwrap();
        let path = small.to_str().unwrap();
        assert_eq!(
            read_file(path),
            json!({"path": path, "text": "services: {}\n", "error": null})
        );
        assert_eq!(unreadable(path), None);
        let big = dir.path().join("big.yaml");
        // A multi-byte character straddles the cap.
        let text = format!("{}é{}", "a".repeat(MAX_FILE_TEXT - 1), "b".repeat(100));
        fs::write(&big, &text).unwrap();
        let read = read_file(big.to_str().unwrap());
        assert_eq!(read["error"], "Truncated at 256 KiB");
        assert_eq!(read["text"].as_str().unwrap().len(), MAX_FILE_TEXT - 1);
        assert_eq!(unreadable(big.to_str().unwrap()), None);
        let missing = dir.path().join("missing.yaml");
        let missing = missing.to_str().unwrap();
        assert_eq!(
            read_file(missing),
            json!({"path": missing, "text": null, "error": "No such file"})
        );
        assert_eq!(
            unreadable(missing),
            Some(format!("Newport can't read {missing}: No such file."))
        );
        assert_eq!(
            read_file(dir.path().to_str().unwrap())["error"],
            "Not a file"
        );
        // Twenty full files would not fit in one reply: the later ones go
        // without text.
        let full = read_file(big.to_str().unwrap());
        let shown = within_budget(vec![full.clone(); 20]);
        let kept = shown.iter().filter(|f| f["text"].is_string()).count();
        assert_eq!(kept, FILES_BUDGET / full["text"].to_string().len());
        assert_eq!(shown[0], full);
        assert_eq!(shown[19]["text"], Value::Null);
        assert_eq!(shown[19]["error"], TOO_LARGE);
        assert!(serde_json::to_string(&shown).unwrap().len() < 4 << 20);
        // Root reads everything, so permissions only bite other accounts.
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            let locked = dir.path().join("locked.yaml");
            fs::write(&locked, "x").unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
            assert_eq!(
                read_file(locked.to_str().unwrap())["error"],
                "Permission denied"
            );
        }
    }

    #[test]
    fn output_is_plain_lines() {
        assert_eq!(
            clean(b"\x1b[1;32mgreen\x1b[0m \x1b]0;title\x07done\x1b(B\x08!\tok"),
            "green done!\tok"
        );
        assert_eq!(clean(b"\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\"), "link");
        assert_eq!(clean(b"cut \x1b["), "cut ");
        let mut lines = Lines::default();
        // Chunks split anywhere, CRLF included.
        let mut out = lines.feed(b"#1 [web] build\r");
        out.extend(lines.feed(b"\n progress 1\rprogress 2\r"));
        out.extend(lines.feed(b"\x1b[2Kprogress 3\n\nlast"));
        out.extend(lines.feed("é".as_bytes().split_at(1).0));
        out.extend(lines.feed("é".as_bytes().split_at(1).1));
        assert_eq!(
            out,
            [
                "#1 [web] build",
                " progress 1",
                "progress 2",
                "progress 3",
                ""
            ]
        );
        assert_eq!(lines.finish().as_deref(), Some("lasté"));
        assert_eq!(lines.finish(), None);
        let long = vec![b'x'; MAX_LINE + 10];
        let out = Lines::default().feed(&long);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), MAX_LINE);
    }

    #[test]
    fn output_keeps_the_newest_bytes_by_offset() {
        let mut output = Output::new(16);
        output.push_line("abc");
        output.push_line("def");
        assert_eq!(output.read(0), ("abc\ndef\n".into(), 8));
        assert_eq!(output.read(4), ("def\n".into(), 8));
        assert_eq!(output.read(8), (String::new(), 8));
        // An offset past the end reads nothing and points at the end.
        assert_eq!(output.read(99), (String::new(), 8));
        output.push_line("ghijklmnop");
        // 19 bytes: the oldest whole lines go once read, and an older offset
        // starts at the oldest kept byte.
        assert_eq!(output.read(0), ("def\nghijklmnop\n".into(), 19));
        assert_eq!(output.read(18), ("\n".into(), 19));
        // A line longer than the cap keeps its newest whole characters, and
        // an offset inside a character starts at the next one.
        let mut wide = Output::new(5);
        wide.push_line("ééé");
        assert_eq!(wide.read(0), ("éé\n".into(), 7));
        assert_eq!(wide.read(3), ("é\n".into(), 7));
        // Growth past the cap is trimmed as it is written.
        let mut big = Output::new(1 << 10);
        for i in 0..1000 {
            big.push_line(&format!("line {i:04}"));
        }
        assert!(big.data.len() <= (1 << 10) * 2);
        let (text, end) = big.read(0);
        assert_eq!(end, 10_000);
        assert!(text.len() <= 1 << 10);
        assert!(text.ends_with("line 0999\n"));
    }

    #[test]
    fn versions_are_read_from_each_cli() {
        for (text, version) in [
            ("2.29.1\n", "2.29.1"),
            ("v2.24.5\n", "2.24.5"),
            ("Docker Compose version v2.24.5\n", "2.24.5"),
            ("docker-compose version 1.29.2, build 5becea4c\n", "1.29.2"),
            ("podman-compose version 1.0.6\npodman version 4.9.3\n", "1.0.6"),
            (
                ">>>> Executing external compose provider \"/usr/libexec/docker/cli-plugins/docker-compose\". <<<<\n\nDocker Compose version v2.27.0\n",
                "2.27.0",
            ),
            ("podman version 5.0.1\n", "5.0.1"),
        ] {
            assert_eq!(parse_version(text).as_deref(), Some(version), "{text}");
        }
        assert_eq!(
            parse_version("docker: 'compose' is not a docker command."),
            None
        );
        assert_eq!(parse_version(""), None);
    }

    /// A fake CLI: a shell script standing in for docker, docker-compose or
    /// podman. Executed once before use, so a test forking elsewhere while
    /// the file was open for writing cannot make it busy.
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        for _ in 0..100 {
            match Command::new(&path).arg("--warm-up").output() {
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => break,
            }
        }
        path
    }
    fn compose_in(dir: &Path) -> Compose {
        let dir = dir.to_owned();
        Compose::new(Box::new(move || vec![dir.clone()]))
    }
    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    #[test]
    fn the_cli_is_discovered_in_order_and_cached() {
        let dir = tempfile::tempdir().unwrap();
        let docker = "case \"$*\" in\n  'compose version --short') echo v2.29.1;;\n  'compose --help') echo '      --progress string   Set type of progress output';;\n  *) exit 1;;\nesac";
        let no_plugin = "echo \"docker: 'compose' is not a docker command.\" >&2; exit 1";
        let standalone = "[ \"$*\" = 'version --short' ] && echo 1.29.2";
        let podman = "[ \"$*\" = 'compose version' ] && { echo 'podman-compose version 1.0.6'; echo 'podman version 4.9.3'; }";

        // Nothing installed.
        let compose = compose_in(dir.path());
        assert_eq!(compose.cli(false, soon()).unwrap_err(), CLI_MISSING);
        // A missing CLI is remembered for a while...
        script(dir.path(), "podman", podman);
        assert_eq!(compose.cli(false, soon()).unwrap_err(), CLI_MISSING);
        // ...but not by a fresh agent.
        let found = compose_in(dir.path()).cli(false, soon()).unwrap();
        assert_eq!(
            (found.kind, found.version.as_str()),
            (Kind::Podman, "1.0.6")
        );
        assert!(!found.progress);

        script(dir.path(), "docker-compose", standalone);
        let found = compose_in(dir.path()).cli(false, soon()).unwrap();
        assert_eq!(
            (found.kind, found.version.as_str()),
            (Kind::Standalone, "1.29.2")
        );
        assert!(!found.progress);
        // Docker without its plugin falls through to docker-compose.
        script(dir.path(), "docker", no_plugin);
        assert_eq!(
            compose_in(dir.path()).cli(false, soon()).unwrap().kind,
            Kind::Standalone
        );
        script(dir.path(), "docker", docker);
        let compose = compose_in(dir.path());
        let found = compose.cli(false, soon()).unwrap();
        assert_eq!(
            found,
            Cli {
                kind: Kind::Plugin,
                version: "2.29.1".into(),
                program: dir.path().join("docker"),
                progress: true,
            }
        );
        // Cached: a later Podman hint does not change it.
        assert_eq!(compose.cli(true, soon()).unwrap().kind, Kind::Plugin);
        // A Podman engine tries podman first.
        assert_eq!(
            compose_in(dir.path()).cli(true, soon()).unwrap().kind,
            Kind::Podman
        );
        // No time left: nothing is run.
        assert_eq!(
            compose_in(dir.path())
                .cli(false, Instant::now())
                .unwrap_err(),
            CLI_MISSING
        );

        // Not executable, or a directory: not a CLI.
        let other = tempfile::tempdir().unwrap();
        fs::write(other.path().join("docker"), "#!/bin/sh\necho 2.0.0\n").unwrap();
        fs::create_dir(other.path().join("docker-compose")).unwrap();
        assert_eq!(which("docker", &[other.path().into()]), None);
        assert_eq!(which("docker-compose", &[other.path().into()]), None);
        assert_eq!(
            which("docker", &[other.path().into(), dir.path().into()]),
            Some(dir.path().join("docker"))
        );
    }

    /// A fake docker that prints what it was given, then behaves as the
    /// first service name says.
    const FAKE: &str = r#"case "$*" in
  'compose version --short') echo 2.29.1; exit 0;;
  'compose --help') echo '--progress'; exit 0;;
esac
echo "argv: $*"
echo "cwd: $(pwd)"
echo "env: $COMPOSE_ANSI $BUILDKIT_PROGRESS $COMPOSE_PROGRESS $LC_ALL"
case "$*" in
  *slow*) printf 'step 1\r'; sleep 30;;
  *stubborn*) trap '' TERM; echo waiting; sleep 30;;
  *fail*) echo 'boom' >&2; exit 3;;
  *chatty*) i=0; while [ $i -lt 3000 ]; do echo "line $i of the chatty build output"; i=$((i+1)); done;;
esac
printf '\033[32mgreen\033[0m\r\nprogress 1\rprogress 2\r\n'"#;

    fn wait(compose: &Compose, job: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = compose.job(&json!({"job": job, "offset": 0})).unwrap();
            if state["state"] != "running" {
                return state;
            }
            assert!(Instant::now() < deadline, "job never finished: {state}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn fake_plan(compose: &Compose, cwd: &Path, services: &[&str]) -> Plan {
        let cli = compose.cli(false, soon()).unwrap();
        plan(
            &cli,
            "app",
            &["/srv/app/compose.yaml".into()],
            cwd,
            None,
            &up(services, false, false, false),
        )
    }

    #[test]
    fn jobs_run_in_the_background_and_report_their_output() {
        let bin = tempfile::tempdir().unwrap();
        script(bin.path(), "docker", FAKE);
        let compose = compose_in(bin.path());
        let work = tempfile::tempdir().unwrap();
        let before = now_ms();
        let job = compose
            .start("app", &fake_plan(&compose, work.path(), &["web"]))
            .unwrap();
        assert_eq!(job.len(), 32);
        let done = wait(&compose, &job);
        assert_eq!(done["job"], json!(job));
        assert_eq!(done["project"], "app");
        assert_eq!(done["command"], "docker compose -p app up -d --no-deps web");
        assert_eq!(done["state"], "succeeded");
        assert_eq!(done["exitCode"], 0);
        assert!(done["startedAt"].as_i64().unwrap() >= before);
        assert!(done["finishedAt"].as_i64().unwrap() >= done["startedAt"].as_i64().unwrap());
        let cwd = fs::canonicalize(work.path()).unwrap();
        assert_eq!(
            done["output"].as_str().unwrap(),
            format!(
                "argv: compose --progress plain -p app -f /srv/app/compose.yaml up -d --no-deps web\ncwd: {}\nenv: never plain plain C\ngreen\nprogress 1\nprogress 2\n",
                cwd.display()
            )
        );
        let end = done["offset"].as_u64().unwrap();
        assert_eq!(end as usize, done["output"].as_str().unwrap().len());
        // Reading on from the end gives nothing new.
        let again = compose.job(&json!({"job": job, "offset": end})).unwrap();
        assert_eq!(
            (again["output"].as_str(), again["offset"].as_u64()),
            (Some(""), Some(end))
        );
        let tail = compose
            .job(&json!({"job": job, "offset": end - 11}))
            .unwrap();
        assert_eq!(tail["output"], "progress 2\n");

        let failed = compose
            .start("app", &fake_plan(&compose, work.path(), &["fail"]))
            .unwrap();
        let failed = wait(&compose, &failed);
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["exitCode"], 3);
        assert!(failed["output"].as_str().unwrap().contains("boom\n"));

        // Newest first, without output.
        let listed = compose.jobs();
        let listed = listed["jobs"].as_array().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["state"], "failed");
        assert_eq!(listed[1]["job"], json!(job));
        assert!(listed.iter().all(|j| j["output"] == "" && j["offset"] == 0));

        assert_eq!(
            compose
                .job(&json!({"job": "nope", "offset": 0}))
                .unwrap_err(),
            UNKNOWN_JOB
        );
        assert_eq!(
            compose.cancel(&json!({"job": "nope"})).unwrap_err(),
            UNKNOWN_JOB
        );
        assert_eq!(
            compose.job(&json!({"job": job, "offset": -1})).unwrap_err(),
            "Invalid output offset."
        );
        assert_eq!(
            compose.job(&json!({})).unwrap_err(),
            "Choose a Compose command."
        );
        // A finished job cannot be cancelled, and that is fine.
        assert_eq!(
            compose.cancel(&json!({"job": job})).unwrap(),
            json!({"ok": true})
        );
        assert_eq!(wait(&compose, &job)["state"], "succeeded");
    }

    #[test]
    fn one_job_per_project_and_cancel_ends_the_group() {
        let bin = tempfile::tempdir().unwrap();
        script(bin.path(), "docker", FAKE);
        let mut compose = compose_in(bin.path());
        compose.grace = Duration::from_millis(300);
        let work = tempfile::tempdir().unwrap();
        let slow = compose
            .start("app", &fake_plan(&compose, work.path(), &["slow"]))
            .unwrap();
        assert_eq!(
            compose
                .start("app", &fake_plan(&compose, work.path(), &["web"]))
                .unwrap_err(),
            "Another Compose command is running for app."
        );
        // Another project is free to run.
        let mut other = fake_plan(&compose, work.path(), &["web"]);
        other.display = "other".into();
        let other = compose.start("other", &other).unwrap();
        assert_eq!(wait(&compose, &other)["state"], "succeeded");

        // Output arrives while it runs, a "\r" ending a line of its own.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        let mut offset = 0;
        while !seen.contains("step 1\n") {
            assert!(Instant::now() < deadline, "no output: {seen:?}");
            let page = compose
                .job(&json!({"job": slow, "offset": offset}))
                .unwrap();
            assert_eq!(page["state"], "running");
            seen += page["output"].as_str().unwrap();
            offset = page["offset"].as_u64().unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        // The listing shows it running, after the newer job of the other project.
        let listed = compose.jobs();
        assert_eq!(listed["jobs"][0]["job"], json!(other));
        assert_eq!(listed["jobs"][1]["job"], json!(slow));
        assert_eq!(listed["jobs"][1]["state"], "running");

        let started = Instant::now();
        assert_eq!(
            compose.cancel(&json!({"job": slow})).unwrap(),
            json!({"ok": true})
        );
        // A second cancel while it stops is fine too.
        assert_eq!(
            compose.cancel(&json!({"job": slow})).unwrap(),
            json!({"ok": true})
        );
        let done = wait(&compose, &slow);
        assert_eq!(done["state"], "cancelled");
        assert_eq!(done["exitCode"], Value::Null);
        assert!(started.elapsed() < Duration::from_secs(5));
        // The project is free again.
        let next = compose
            .start("app", &fake_plan(&compose, work.path(), &["web"]))
            .unwrap();
        assert_eq!(wait(&compose, &next)["state"], "succeeded");

        // One that ignores SIGTERM is killed after the grace period.
        let stubborn = compose
            .start("app", &fake_plan(&compose, work.path(), &["stubborn"]))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !compose.job(&json!({"job": stubborn})).unwrap()["output"]
            .as_str()
            .unwrap()
            .contains("waiting")
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        let started = Instant::now();
        compose.cancel(&json!({"job": stubborn})).unwrap();
        let done = wait(&compose, &stubborn);
        assert_eq!(done["state"], "cancelled");
        assert!(started.elapsed() >= Duration::from_millis(250));
        assert!(started.elapsed() < Duration::from_secs(10));
        compose.wait_idle(Duration::from_secs(1));
    }

    #[test]
    fn finished_jobs_expire_and_output_is_capped() {
        let bin = tempfile::tempdir().unwrap();
        script(bin.path(), "docker", FAKE);
        let mut compose = compose_in(bin.path());
        compose.output_cap = 4096;
        let work = tempfile::tempdir().unwrap();
        let chatty = compose
            .start("app", &fake_plan(&compose, work.path(), &["chatty"]))
            .unwrap();
        let done = wait(&compose, &chatty);
        let text = done["output"].as_str().unwrap();
        assert!(text.len() <= 4096, "{}", text.len());
        assert!(text.ends_with("progress 2\n"));
        assert!(done["offset"].as_u64().unwrap() > 100_000);
        // An offset older than what is kept reads from the oldest kept byte.
        let from_one = compose.job(&json!({"job": chatty, "offset": 1})).unwrap();
        assert_eq!(from_one["output"], done["output"]);

        compose.keep = Duration::from_millis(100);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            compose.job(&json!({"job": chatty})).unwrap_err(),
            UNKNOWN_JOB
        );
        assert_eq!(compose.jobs(), json!({"jobs": []}));

        // At most MAX_FINISHED finished jobs stay, the newest.
        compose.keep = KEEP_FINISHED;
        let mut ids = Vec::new();
        for _ in 0..MAX_FINISHED + 2 {
            let id = compose
                .start("app", &fake_plan(&compose, work.path(), &["web"]))
                .unwrap();
            wait(&compose, &id);
            ids.push(id);
        }
        let listed = compose.jobs();
        let listed = listed["jobs"].as_array().unwrap();
        assert_eq!(listed.len(), MAX_FINISHED);
        assert_eq!(listed[0]["job"], json!(ids.last().unwrap()));
        assert!(compose.job(&json!({"job": ids[0]})).is_err());
    }

    #[test]
    fn requests_find_the_project_through_its_containers() {
        let bin = tempfile::tempdir().unwrap();
        script(bin.path(), "docker", FAKE);
        let compose = compose_in(bin.path());
        let project = tempfile::tempdir().unwrap();
        let file = project.path().join("compose.yaml");
        fs::write(&file, "services:\n  web:\n    image: busybox\n").unwrap();
        let dir = project.path().to_str().unwrap().to_owned();
        let file = file.to_str().unwrap().to_owned();
        let rows = vec![json!({"Labels": {
            "com.docker.compose.project.working_dir": dir,
            "com.docker.compose.project.config_files": file,
        }})];
        let with = |rows: Vec<Value>| {
            move |_: &str, _: Instant| -> Result<Found, String> {
                Ok(Found {
                    socket: Some("/var/run/docker.sock".into()),
                    podman: false,
                    rows: rows.clone(),
                })
            }
        };
        let found = with(rows.clone());
        let empty = with(Vec::new());
        let ask = |request: Value, find: Finder| compose.handle(&request, find).unwrap();

        assert!(compose
            .handle(&json!({"op": "containers"}), &found)
            .is_none());
        assert_eq!(
            ask(json!({"op": "compose_project", "project": "App"}), &found).unwrap_err(),
            "Not a valid Compose project: App"
        );
        assert_eq!(
            ask(json!({"op": "compose_project", "project": "app"}), &empty).unwrap_err(),
            "No containers remain for app, so Newport can't find its compose files."
        );
        let gone = |_: &str, _: Instant| -> Result<Found, String> {
            Err(crate::docker::NOT_RUNNING.into())
        };
        assert_eq!(
            ask(json!({"op": "compose_project", "project": "app"}), &gone).unwrap_err(),
            crate::docker::NOT_RUNNING
        );

        // The fake prints its argv instead of JSON: config can't be read.
        let view = ask(json!({"op": "compose_project", "project": "app"}), &found).unwrap();
        assert_eq!(view["cli"], json!({"kind": "plugin", "version": "2.29.1"}));
        assert_eq!(view["cliError"], Value::Null);
        assert_eq!(view["workingDir"], json!(dir));
        assert_eq!(view["configFiles"], json!([file]));
        assert_eq!(
            view["files"],
            json!([{"path": file, "text": "services:\n  web:\n    image: busybox\n", "error": null}])
        );
        assert_eq!(view["services"], Value::Null);
        assert_eq!(
            view["configError"],
            "Compose printed a configuration Newport couldn't read."
        );

        let run = |action: Value, find: Finder| {
            ask(
                json!({"op": "compose_run", "project": "app", "action": action}),
                find,
            )
        };
        assert_eq!(
            run(json!({"kind": "up", "services": ["a;b"]}), &found).unwrap_err(),
            "Not a valid Compose service: a;b"
        );
        assert_eq!(
            run(json!({"kind": "build"}), &empty).unwrap_err(),
            "No containers remain for app, so Newport can't find its compose files."
        );
        let job = run(
            json!({"kind": "up", "services": ["web"], "forceRecreate": true}),
            &found,
        )
        .unwrap()["job"]
            .as_str()
            .unwrap()
            .to_owned();
        let done = wait(&compose, &job);
        assert_eq!(done["state"], "succeeded");
        let output = done["output"].as_str().unwrap();
        assert!(
            output.starts_with(&format!(
                "argv: compose --progress plain -p app -f {file} up -d --force-recreate --no-deps web\ncwd: "
            )),
            "{output}"
        );
        // `down` with no containers left names the project only.
        let job = run(json!({"kind": "down", "volumes": true}), &empty).unwrap()["job"]
            .as_str()
            .unwrap()
            .to_owned();
        let done = wait(&compose, &job);
        assert_eq!(done["command"], "docker compose -p app down --volumes");
        assert!(done["output"]
            .as_str()
            .unwrap()
            .starts_with("argv: compose --progress plain -p app down --volumes\n"));

        // Missing files stop everything but `down`, which drops them.
        fs::remove_file(&file).unwrap();
        assert_eq!(
            run(json!({"kind": "pull"}), &found).unwrap_err(),
            format!("Newport can't read {file}: No such file.")
        );
        assert_eq!(
            run(
                json!({"kind": "scale", "service": "web", "replicas": 2}),
                &found
            )
            .unwrap_err(),
            format!("Newport can't read {file}: No such file.")
        );
        let job = run(json!({"kind": "down"}), &found).unwrap()["job"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(wait(&compose, &job)["output"]
            .as_str()
            .unwrap()
            .starts_with("argv: compose --progress plain -p app down\n"));
        let view = ask(json!({"op": "compose_project", "project": "app"}), &found).unwrap();
        assert_eq!(view["files"][0]["error"], "No such file");

        let unlabelled = with(vec![json!({"Labels": {}})]);
        assert_eq!(
            run(json!({"kind": "up"}), &unlabelled).unwrap_err(),
            "Compose didn't record the files of app."
        );
        let view = ask(
            json!({"op": "compose_project", "project": "app"}),
            &unlabelled,
        )
        .unwrap();
        assert_eq!(
            view["configError"],
            "Compose didn't record this project's files."
        );

        // Without a CLI the project still shows its files.
        let nowhere = tempfile::tempdir().unwrap();
        let bare = compose_in(nowhere.path());
        let view = bare
            .handle(&json!({"op": "compose_project", "project": "app"}), &found)
            .unwrap()
            .unwrap();
        assert_eq!(view["cli"], Value::Null);
        assert_eq!(view["cliError"], CLI_MISSING);
        assert_eq!(view["services"], Value::Null);
        assert_eq!(view["configError"], Value::Null);
        assert_eq!(view["files"][0]["path"], json!(file));
        assert_eq!(
            bare.handle(
                &json!({"op": "compose_run", "project": "app", "action": {"kind": "down"}}),
                &found
            )
            .unwrap()
            .unwrap_err(),
            CLI_MISSING
        );
    }

    #[test]
    fn config_output_becomes_services_or_an_error() {
        let bin = tempfile::tempdir().unwrap();
        script(
            bin.path(),
            "docker",
            r#"case "$*" in
  'compose version --short') echo 2.29.1; exit 0;;
  *broken*) printf '\033[31myaml: line 3: did not find expected node content\033[0m\n' >&2; exit 1;;
  *--profile\ \**config\ --format\ json) echo '{"services": {"web": {"image": "busybox"}, "tool": {"build": {"context": "."}, "profiles": ["debug"]}}}';;
  *) exit 2;;
esac"#,
        );
        let compose = compose_in(bin.path());
        let cli = compose.cli(false, soon()).unwrap();
        assert!(!cli.progress);
        let files = |name: &str| Files {
            working_dir: None,
            config_files: vec![format!("/srv/{name}/compose.yaml")],
        };
        assert_eq!(
            config(&cli, "app", &files("app"), None, soon()).unwrap(),
            vec![
                json!({"name": "tool", "image": null, "build": true, "profiles": ["debug"]}),
                json!({"name": "web", "image": "busybox", "build": false, "profiles": []}),
            ]
        );
        assert_eq!(
            config(&cli, "app", &files("broken"), None, soon()).unwrap_err(),
            "yaml: line 3: did not find expected node content"
        );
        assert_eq!(
            config(&cli, "app", &files("app"), None, Instant::now()).unwrap_err(),
            "Newport ran out of time before Compose could read the project."
        );
    }
}
