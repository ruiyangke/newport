//! Port discovery: TCP listeners read from /proc/net, the processes that own
//! them, and the projects those processes run in. Nothing here needs ss,
//! netstat or a shell on the server. The desktop decides what each listener
//! is; this module only reports what the server shows.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const MAX_LISTENERS: usize = 4096;
const MAX_PROCESSES: usize = 256;
const MAX_FIELD: usize = 4096;
const MAX_COMPOSE_ROOTS: usize = 64;
const MAX_ELEVATED_REPLY: usize = 4 << 20;
const ELEVATED_TIMEOUT: Duration = Duration::from_secs(10);
/// `/proc/net/tcp` state for a listening socket.
const LISTEN: &str = "0A";

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Listener {
    pub address: String,
    pub port: u16,
    /// The socket's owner; known even when its process is not visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Process {
    pub pid: u32,
    /// Linux marks a replaced or removed path " (deleted)"; kept as read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// What one account can see of the listening sockets and their owners.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Sockets {
    pub listeners: Vec<Listener>,
    pub processes: Vec<Process>,
}

/// A directory's project, found the way Projectile finds one.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub root: String,
    /// What identified the root: `.git`, `package.json`, `Makefile`…
    pub marker: String,
    /// The checkout the directory is in when the root is a Git repository;
    /// `root` is then the repository every worktree shares.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main: Option<bool>,
}

/// Reads a server's filesystem below `root` ("/" outside tests).
struct Host {
    root: PathBuf,
}

impl Host {
    fn new(root: &Path) -> Self {
        Self {
            root: std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned()),
        }
    }
    fn path(&self, logical: &str) -> PathBuf {
        self.root.join(logical.trim_start_matches('/'))
    }
    fn read(&self, logical: &str) -> Option<String> {
        use std::io::Read;
        let mut text = Vec::new();
        std::fs::File::open(self.path(logical))
            .ok()?
            .take(MAX_FIELD as u64)
            .read_to_end(&mut text)
            .ok()?;
        Some(String::from_utf8_lossy(&text).into_owned())
    }
    fn link(&self, logical: &str) -> Option<String> {
        let target = std::fs::read_link(self.path(logical)).ok()?;
        let mut target = target.to_string_lossy().into_owned();
        truncate(&mut target, MAX_FIELD);
        Some(target)
    }
    fn exists(&self, logical: &str) -> bool {
        self.path(logical).exists()
    }
    /// The physical path, as `cd dir && pwd -P` would print it.
    fn real(&self, logical: &str) -> Option<String> {
        let real = std::fs::canonicalize(self.path(logical)).ok()?;
        let inside = real.strip_prefix(&self.root).ok()?;
        Some(format!("/{}", inside.to_string_lossy()))
    }
}

fn truncate(text: &mut String, limit: usize) {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

/// `/proc/net/tcp` prints each 32-bit word of an address as the host stores
/// it, so the bytes come back through the host's own byte order.
fn address(hex: &str) -> Option<String> {
    let words: Option<Vec<[u8; 4]>> = (0..hex.len() / 8)
        .map(|i| {
            let word = u32::from_str_radix(hex.get(i * 8..i * 8 + 8)?, 16).ok()?;
            Some(word.to_ne_bytes())
        })
        .collect();
    let bytes: Vec<u8> = words?.concat();
    match bytes.len() {
        4 => Some(std::net::Ipv4Addr::from(<[u8; 4]>::try_from(bytes).ok()?).to_string()),
        16 => Some(std::net::Ipv6Addr::from(<[u8; 16]>::try_from(bytes).ok()?).to_string()),
        _ => None,
    }
}

/// Listening sockets in one `/proc/net/tcp{,6}` table, with their inodes.
fn parse_table(text: &str, users: &HashMap<u32, String>) -> Vec<(Listener, u64)> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            // sl local remote state queues timer retransmits uid timeout inode
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.get(3) != Some(&LISTEN) {
                return None;
            }
            let (host, port) = fields.get(1)?.split_once(':')?;
            let port = u16::from_str_radix(port, 16).ok().filter(|p| *p > 0)?;
            let uid = fields.get(7).and_then(|u| u.parse::<u32>().ok());
            let inode = fields.get(9).and_then(|i| i.parse().ok()).unwrap_or(0);
            Some((
                Listener {
                    address: address(host)?,
                    port,
                    user: uid.and_then(|uid| users.get(&uid).cloned()),
                    ..Listener::default()
                },
                inode,
            ))
        })
        .collect()
}

pub(crate) fn users(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let uid = fields.nth(1)?.parse().ok()?;
            Some((uid, name.to_owned()))
        })
        .collect()
}

fn status_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Start time (`/proc/[pid]/stat` field 22), counted from the last ')' since
/// the name may hold spaces or parentheses. A changed value means the pid
/// was reused while it was being read.
fn start_time(stat: &str) -> Option<u64> {
    let fields: Vec<&str> = stat
        .get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .collect();
    fields.get(22 - 3)?.parse().ok()
}

/// The pid owning each wanted socket inode, as far as this account can see.
fn owners(host: &Host, wanted: &HashSet<u64>) -> HashMap<u64, u32> {
    let mut found = HashMap::new();
    let Ok(entries) = std::fs::read_dir(host.path("/proc")) else {
        return found;
    };
    for entry in entries.flatten() {
        if found.len() == wanted.len() {
            break;
        }
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        // Another account's descriptors are unreadable without root.
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let inode = target
                .to_str()
                .and_then(|t| t.strip_prefix("socket:["))
                .and_then(|t| t.strip_suffix(']'))
                .and_then(|t| t.parse::<u64>().ok());
            if let Some(inode) = inode.filter(|i| wanted.contains(i)) {
                found.entry(inode).or_insert(pid);
            }
        }
    }
    found
}

fn process(host: &Host, pid: u32, users: &HashMap<u32, String>) -> Option<Process> {
    let dir = format!("/proc/{pid}");
    let before = start_time(&host.read(&format!("{dir}/stat"))?)?;
    let arguments = host
        .read(&format!("{dir}/cmdline"))
        .unwrap_or_default()
        .split('\0')
        .filter(|arg| !arg.is_empty())
        .map(str::to_owned)
        .collect();
    let user = host
        .read(&format!("{dir}/status"))
        .as_deref()
        .and_then(status_uid)
        .and_then(|uid| users.get(&uid).cloned());
    let process = Process {
        pid,
        executable: host.link(&format!("{dir}/exe")),
        working_directory: host.link(&format!("{dir}/cwd")),
        arguments,
        user,
    };
    // A pid reused mid-read would describe the wrong program.
    (start_time(&host.read(&format!("{dir}/stat"))?)? == before).then_some(process)
}

/// Every listening TCP socket this account can see, and its owner where the
/// account may look at that process.
fn scan(host: &Host) -> Result<Sockets, String> {
    let users = users(&host.read("/etc/passwd").unwrap_or_default());
    let tables: Vec<Option<String>> = ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .map(|table| std::fs::read_to_string(host.path(table)).ok())
        .collect();
    if tables.iter().all(Option::is_none) {
        return Err("Couldn’t read this server’s listening sockets from /proc/net/tcp.".into());
    }
    let mut found: Vec<(Listener, u64)> = tables
        .iter()
        .flatten()
        .flat_map(|text| parse_table(text, &users))
        .collect();
    found.truncate(MAX_LISTENERS);
    let wanted: HashSet<u64> = found.iter().map(|(_, i)| *i).filter(|i| *i != 0).collect();
    let owners = owners(host, &wanted);
    let mut pids: Vec<u32> = Vec::new();
    let listeners = found
        .into_iter()
        .map(|(mut listener, inode)| {
            if let Some(pid) = owners.get(&inode).copied() {
                listener.pid = Some(pid);
                listener.process = host
                    .read(&format!("/proc/{pid}/comm"))
                    .map(|name| name.trim_end().to_owned())
                    .filter(|name| !name.is_empty());
                pids.push(pid);
            }
            listener
        })
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids.truncate(MAX_PROCESSES);
    let processes = pids
        .into_iter()
        .filter_map(|pid| process(host, pid, &users))
        .collect();
    Ok(Sockets {
        listeners,
        processes,
    })
}

/// The view of root, when this account already has passwordless sudo:
/// other accounts' services then show their process too. Never prompts.
fn merge(own: Sockets, elevated: Sockets) -> Sockets {
    let mut processes = elevated.processes;
    let known: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
    processes.extend(
        own.processes
            .into_iter()
            .filter(|p| !known.contains(&p.pid)),
    );
    Sockets {
        listeners: if elevated.listeners.is_empty() {
            own.listeners
        } else {
            elevated.listeners
        },
        processes,
    }
}

/// Directories that hold projects but are never one themselves.
fn boundary(dir: &str) -> bool {
    if let Some(rest) = dir.strip_prefix("/home/") {
        return !rest.contains('/');
    }
    if dir.starts_with("/root/") {
        return false;
    }
    matches!(
        dir,
        "" | "/" | "/root" | "/tmp" | "/var" | "/srv" | "/opt" | "/mnt" | "/media"
    )
}

/// System directories never belong to a project.
fn system(dir: &str) -> bool {
    [
        "/usr",
        "/bin",
        "/sbin",
        "/etc",
        "/proc",
        "/sys",
        "/dev",
        "/run",
        "/nix/store",
        "/var/lib",
        "/var/log",
    ]
    .iter()
    .any(|prefix| {
        dir.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

const VCS: [&str; 9] = [
    ".git",
    ".hg",
    ".jj",
    ".sl",
    ".fslckout",
    "_FOSSIL_",
    ".bzr",
    "_darcs",
    ".svn",
];
const PROJECT_FILES: [&str; 27] = [
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    "Gemfile",
    "composer.json",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "mix.exs",
    "deno.json",
    "deno.jsonc",
    "flake.nix",
    "stack.yaml",
    "dune-project",
    "build.sbt",
    "project.clj",
    "deps.edn",
    "pubspec.yaml",
    "Package.swift",
    "CMakeLists.txt",
    "compose.yaml",
    "compose.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
];

/// Projectile's root order: `.projectile`, the nearest VCS marker, the
/// topmost project file, then the top of the nearest Makefile chain.
fn project_root(host: &Host, dir: &str) -> Option<(String, String)> {
    if system(dir) {
        return None;
    }
    let (mut local, mut vcs, mut top, mut make) = (None, None, None, None);
    let mut make_done = false;
    let mut current = dir.trim_end_matches('/').to_owned();
    for _ in 0..32 {
        if boundary(&current) {
            break;
        }
        let at = |name: &str| host.exists(&format!("{current}/{name}"));
        if local.is_none() && at(".projectile") {
            local = Some(current.clone());
        }
        if vcs.is_none() {
            vcs = VCS
                .iter()
                .find(|marker| at(marker))
                .map(|marker| (current.clone(), (*marker).to_owned()));
        }
        if let Some(marker) = PROJECT_FILES.iter().find(|marker| at(marker)) {
            top = Some((current.clone(), (*marker).to_owned()));
        }
        if !make_done {
            if at("Makefile") {
                make = Some(current.clone());
            } else if make.is_some() {
                make_done = true;
            }
        }
        match current.rfind('/') {
            Some(slash) => current.truncate(slash),
            None => break,
        }
    }
    local
        .map(|root| (root, ".projectile".to_owned()))
        .or(vcs)
        .or(top)
        .or_else(|| make.map(|root| (root, "Makefile".to_owned())))
}

/// The checkout at `dir` (whose root holds `.git`), the repository it
/// belongs to, its branch, and whether it is the main checkout. A linked
/// worktree's `.git` file names its gitdir, whose commondir leads to the
/// repository every worktree shares. Reads files; needs no git binary.
fn git_worktree(host: &Host, dir: &str) -> Option<(String, String, String, bool)> {
    let dot_git = format!("{dir}/.git");
    let git_dir = if host.path(&dot_git).is_dir() {
        dot_git
    } else {
        let named = host
            .read(&dot_git)?
            .lines()
            .find_map(|line| line.strip_prefix("gitdir: "))?
            .to_owned();
        let named = if named.starts_with('/') {
            named
        } else {
            format!("{dir}/{named}")
        };
        host.real(&named)?
    };
    let here = host.real(dir).unwrap_or_else(|| dir.to_owned());
    let mut top = here.clone();
    let mut main = true;
    // A submodule's gitdir has no commondir; it is its own main checkout.
    if let Some(common) = host.read(&format!("{git_dir}/commondir")) {
        let common = common.lines().next().unwrap_or_default();
        let common = if common.starts_with('/') {
            common.to_owned()
        } else {
            format!("{git_dir}/{common}")
        };
        if let Some(common) = host.real(&common) {
            main = false;
            top = common
                .strip_suffix("/.git")
                .map(str::to_owned)
                .unwrap_or(common);
        }
    }
    let head = host
        .read(&format!("{git_dir}/HEAD"))
        .and_then(|h| h.lines().next().map(str::to_owned))
        .unwrap_or_default();
    let branch = match head.strip_prefix("ref: ") {
        Some(name) => name.strip_prefix("refs/heads/").unwrap_or(name).to_owned(),
        None => head.chars().take(7).collect(),
    };
    Some((here, top, branch, main))
}

fn project(host: &Host, dir: &str) -> Option<Project> {
    let (root, marker) = project_root(host, dir)?;
    let mut project = Project {
        root,
        marker,
        ..Project::default()
    };
    if project.marker == ".git" {
        if let Some((here, top, branch, main)) = git_worktree(host, &project.root) {
            project.worktree = Some(here);
            project.root = top;
            project.branch = Some(branch).filter(|b| !b.is_empty());
            project.main = Some(main);
        }
    }
    Some(project)
}

/// Each directory's project, resolved once: processes often share one.
fn projects<'a>(host: &Host, dirs: impl Iterator<Item = &'a str>) -> HashMap<String, Value> {
    let mut found = HashMap::new();
    for dir in dirs {
        if found.contains_key(dir) {
            continue;
        }
        // A directory removed under a running process: walking up from it
        // would credit the process to whatever contained it.
        let resolved = if dir.ends_with(" (deleted)") || !dir.starts_with('/') {
            None
        } else {
            project(host, dir)
        };
        found.insert(dir.to_owned(), json!(resolved));
    }
    found
}

pub struct Ports {
    host: Host,
    /// Root's view of the sockets, or None to use this account's own.
    elevated: Box<dyn Fn() -> Option<Sockets> + Send + Sync>,
}

impl Ports {
    pub fn system() -> Self {
        // SAFETY: geteuid has no preconditions and cannot fail.
        let root = unsafe { libc::geteuid() } == 0;
        // Tried until it fails once: a sudo check on every refresh would
        // fill the auth log of a server that never allows it.
        let allowed = Mutex::new(!root);
        Self {
            host: Host::new(Path::new("/")),
            elevated: Box::new(move || {
                let mut allowed = allowed.lock().unwrap();
                if !*allowed {
                    return None;
                }
                let found = elevated_scan();
                *allowed = found.is_some();
                found
            }),
        }
    }

    #[cfg(test)]
    fn at(root: &Path, elevated: Option<Sockets>) -> Self {
        let elevated = Mutex::new(elevated);
        Self {
            host: Host::new(root),
            elevated: Box::new(move || elevated.lock().unwrap().take()),
        }
    }

    /// Listeners, their processes and projects, and published container
    /// ports. Docker being absent or unreachable leaves the listeners.
    fn snapshot(&self, containers: &[Value]) -> Result<Value, String> {
        let own = scan(&self.host)?;
        let sockets = match (self.elevated)() {
            Some(elevated) => merge(own, elevated),
            None => own,
        };
        let mut containers: Vec<Value> = containers.to_vec();
        let workdirs: Vec<String> = containers
            .iter()
            .filter_map(|c| c["workdir"].as_str().map(str::to_owned))
            .collect::<HashSet<_>>()
            .into_iter()
            .take(MAX_COMPOSE_ROOTS)
            .collect();
        let found = projects(
            &self.host,
            sockets
                .processes
                .iter()
                .filter_map(|p| p.working_directory.as_deref())
                .chain(workdirs.iter().map(String::as_str)),
        );
        for container in &mut containers {
            if let Some(project) = container["workdir"].as_str().and_then(|w| found.get(w)) {
                container["workdirProject"] = project.clone();
            }
        }
        let processes: Vec<Value> = sockets
            .processes
            .iter()
            .map(|process| {
                let mut value = json!(process);
                if let Some(project) = process
                    .working_directory
                    .as_deref()
                    .and_then(|dir| found.get(dir))
                {
                    value["project"] = project.clone();
                }
                value
            })
            .collect();
        Ok(json!({
            "listeners": sockets.listeners,
            "processes": processes,
            "containers": containers,
        }))
    }

    /// Answers a Ports request, or None when the op is not one of ours.
    pub fn handle(
        &self,
        request: &Value,
        containers: &crate::containers::Containers,
    ) -> Option<Value> {
        if request.get("op").and_then(Value::as_str)? != "ports" {
            return None;
        }
        Some(
            self.snapshot(&containers.published_ports())
                .unwrap_or_else(|error| json!({"error": error})),
        )
    }
}

/// `sudo -n <this agent> listeners`: root's view, only where this account
/// already has passwordless sudo. Never prompts or changes sudoers.
fn elevated_scan() -> Option<Sockets> {
    use crate::services::Run;
    let agent = std::env::current_exe().ok()?;
    let output = crate::services::System
        .run(
            "sudo",
            &[
                "-n".into(),
                agent.to_string_lossy().into_owned(),
                "listeners".into(),
            ],
            &[],
            ELEVATED_TIMEOUT,
            MAX_ELEVATED_REPLY,
        )
        .ok()?;
    if !output.success {
        return None;
    }
    serde_json::from_str(&output.stdout).ok()
}

/// The `listeners` command: this account's view of the sockets as JSON.
pub fn print_listeners() -> std::io::Result<()> {
    let sockets = scan(&Host::new(Path::new("/"))).map_err(std::io::Error::other)?;
    serde_json::to_writer(std::io::stdout().lock(), &sockets)?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative.trim_start_matches('/'));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn mkdir(root: &Path, relative: &str) {
        fs::create_dir_all(root.join(relative.trim_start_matches('/'))).unwrap();
    }
    fn link(root: &Path, relative: &str, target: &str) {
        let path = root.join(relative.trim_start_matches('/'));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, path).unwrap();
    }
    /// One `/proc/net/tcp` row; the address is already in table form.
    fn row(local: &str, state: &str, uid: u32, inode: u64) -> String {
        format!(
            "   0: {local} 00000000:0000 {state} 00000000:00000000 00:00000000 00000000  {uid}        0 {inode} 1 0000000000000000 100 0 0 10 0\n"
        )
    }
    fn table(rows: &[String]) -> String {
        let mut text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n".to_owned();
        for row in rows {
            text += row;
        }
        text
    }
    fn hex4(ip: [u8; 4]) -> String {
        format!("{:08X}", u32::from_ne_bytes(ip))
    }
    fn hex6(ip: std::net::Ipv6Addr) -> String {
        ip.octets()
            .chunks(4)
            .map(|c| format!("{:08X}", u32::from_ne_bytes([c[0], c[1], c[2], c[3]])))
            .collect()
    }
    fn stat(pid: u32, name: &str, started: u64) -> String {
        let mut fields = vec!["0".to_string(); 22];
        fields[0] = "S".into();
        fields[19] = started.to_string();
        format!("{pid} ({name}) {}", fields.join(" "))
    }
    fn task(
        root: &Path,
        pid: u32,
        name: &str,
        uid: u32,
        cwd: &str,
        argv: &[&str],
        sockets: &[u64],
    ) {
        write(root, &format!("proc/{pid}/stat"), &stat(pid, name, 4242));
        write(root, &format!("proc/{pid}/comm"), &format!("{name}\n"));
        write(
            root,
            &format!("proc/{pid}/status"),
            &format!("Name:\t{name}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        );
        write(
            root,
            &format!("proc/{pid}/cmdline"),
            &(argv.join("\0") + "\0"),
        );
        link(
            root,
            &format!("proc/{pid}/exe"),
            &format!("/usr/bin/{name}"),
        );
        link(root, &format!("proc/{pid}/cwd"), cwd);
        mkdir(root, &format!("proc/{pid}/fd"));
        for (fd, inode) in sockets.iter().enumerate() {
            link(
                root,
                &format!("proc/{pid}/fd/{}", fd + 3),
                &format!("socket:[{inode}]"),
            );
        }
    }
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "etc/passwd",
            "root:x:0:0::/root:/bin/sh\nfixture:x:1000:1000::/home/fixture:/bin/sh\n",
        );
        write(
            root,
            "proc/net/tcp",
            &table(&[
                row(&format!("{}:1538", hex4([127, 0, 0, 1])), LISTEN, 0, 11),
                row(&format!("{}:1F90", hex4([0, 0, 0, 0])), LISTEN, 1000, 12),
                // Established connections are not listeners.
                row(&format!("{}:1F90", hex4([10, 0, 0, 2])), "01", 1000, 13),
            ]),
        );
        write(
            root,
            "proc/net/tcp6",
            &table(&[row(
                &format!("{}:1F90", hex6(std::net::Ipv6Addr::UNSPECIFIED)),
                LISTEN,
                1000,
                14,
            )]),
        );
        task(
            root,
            700,
            "node",
            1000,
            "/home/fixture/app/web",
            &["node", "/home/fixture/app/node_modules/.bin/vite"],
            &[12, 14],
        );
        write(root, "home/fixture/app/package.json", "{}");
        mkdir(root, "home/fixture/app/.git");
        write(root, "home/fixture/app/.git/HEAD", "ref: refs/heads/main\n");
        dir
    }

    #[test]
    fn decodes_table_addresses_in_host_order() {
        assert_eq!(address(&hex4([127, 0, 0, 1])).as_deref(), Some("127.0.0.1"));
        assert_eq!(
            address(&hex6("::1".parse().unwrap())).as_deref(),
            Some("::1")
        );
        assert_eq!(
            address(&hex6("fe80::1:2".parse().unwrap())).as_deref(),
            Some("fe80::1:2")
        );
        assert_eq!(address("XYZ"), None);
    }

    #[test]
    fn finds_listeners_owners_and_projects_without_ss() {
        let dir = fixture();
        let ports = Ports::at(dir.path(), None);
        let reply = ports.snapshot(&[]).unwrap();
        let listeners: Vec<Listener> = serde_json::from_value(reply["listeners"].clone()).unwrap();
        assert_eq!(
            listeners,
            vec![
                Listener {
                    address: "127.0.0.1".into(),
                    port: 5432,
                    user: Some("root".into()),
                    ..Listener::default()
                },
                Listener {
                    address: "0.0.0.0".into(),
                    port: 8080,
                    user: Some("fixture".into()),
                    pid: Some(700),
                    process: Some("node".into()),
                },
                Listener {
                    address: "::".into(),
                    port: 8080,
                    user: Some("fixture".into()),
                    pid: Some(700),
                    process: Some("node".into()),
                },
            ]
        );
        let process = &reply["processes"][0];
        assert_eq!(process["pid"], 700);
        assert_eq!(process["executable"], "/usr/bin/node");
        assert_eq!(process["workingDirectory"], "/home/fixture/app/web");
        assert_eq!(process["user"], "fixture");
        assert_eq!(
            process["arguments"][1],
            "/home/fixture/app/node_modules/.bin/vite"
        );
        assert_eq!(
            process["project"],
            json!({"root": "/home/fixture/app", "marker": ".git", "worktree": "/home/fixture/app", "branch": "main", "main": true})
        );
    }

    #[test]
    fn root_view_names_other_accounts_services() {
        let dir = fixture();
        let elevated = Sockets {
            listeners: vec![Listener {
                address: "127.0.0.1".into(),
                port: 5432,
                user: Some("root".into()),
                pid: Some(90),
                process: Some("postgres".into()),
            }],
            processes: vec![Process {
                pid: 90,
                executable: Some("/usr/lib/postgresql/16/bin/postgres".into()),
                working_directory: Some("/var/lib/postgresql".into()),
                arguments: vec!["postgres".into()],
                user: Some("postgres".into()),
            }],
        };
        let reply = Ports::at(dir.path(), Some(elevated)).snapshot(&[]).unwrap();
        assert_eq!(reply["listeners"][0]["pid"], 90);
        // The account's own processes still come with their projects.
        let pids: Vec<u64> = reply["processes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["pid"].as_u64().unwrap())
            .collect();
        assert_eq!(pids, [90, 700]);
        assert_eq!(reply["processes"][0]["project"], Value::Null);
    }

    #[test]
    fn missing_tables_explain_themselves() {
        let dir = tempfile::tempdir().unwrap();
        let error = Ports::at(dir.path(), None).snapshot(&[]).unwrap_err();
        assert!(error.contains("/proc/net/tcp"), "{error}");
    }

    #[test]
    fn worktrees_of_one_repository_share_its_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        mkdir(root, "srv/repo/.git/worktrees/feature");
        write(root, "srv/repo/.git/HEAD", "ref: refs/heads/main\n");
        write(root, "srv/repo/.git/worktrees/feature/commondir", "../..\n");
        write(
            root,
            "srv/repo/.git/worktrees/feature/HEAD",
            "0123456789abcdef\n",
        );
        write(
            root,
            "srv/feature/.git",
            "gitdir: /srv/repo/.git/worktrees/feature\n",
        );
        mkdir(root, "srv/feature/api");
        let host = Host::new(root);
        assert_eq!(
            project(&host, "/srv/feature/api"),
            Some(Project {
                root: "/srv/repo".into(),
                marker: ".git".into(),
                worktree: Some("/srv/feature".into()),
                branch: Some("0123456".into()),
                main: Some(false),
            })
        );
        assert_eq!(project(&host, "/srv/repo").unwrap().main, Some(true));
    }

    #[test]
    fn roots_follow_projectile_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // The topmost project file wins over a nearer one.
        write(root, "opt/mono/package.json", "{}");
        write(root, "opt/mono/packages/web/package.json", "{}");
        // The top of the nearest Makefile chain.
        write(root, "srv/build/Makefile", "");
        write(root, "srv/build/sub/Makefile", "");
        let host = Host::new(root);
        assert_eq!(
            project_root(&host, "/opt/mono/packages/web"),
            Some(("/opt/mono".into(), "package.json".into()))
        );
        assert_eq!(
            project_root(&host, "/srv/build/sub"),
            Some(("/srv/build".into(), "Makefile".into()))
        );
        // Home and system directories are never projects themselves.
        write(root, "home/fixture/package.json", "{}");
        assert_eq!(project_root(&host, "/home/fixture"), None);
        assert_eq!(project_root(&host, "/usr/lib/thing"), None);
        // A removed directory is not credited to its parent.
        assert_eq!(
            projects(&host, ["/opt/mono/gone (deleted)"].into_iter())["/opt/mono/gone (deleted)"],
            Value::Null
        );
    }

    #[test]
    fn container_workdirs_get_their_projects() {
        let dir = fixture();
        mkdir(dir.path(), "home/fixture/app/deploy");
        let containers = [
            json!({"name": "web", "bindings": [{"address": "0.0.0.0", "port": 3000}], "workdir": "/home/fixture/app/deploy"}),
        ];
        let reply = Ports::at(dir.path(), None).snapshot(&containers).unwrap();
        assert_eq!(
            reply["containers"][0]["workdirProject"]["root"],
            "/home/fixture/app"
        );
    }
}
