//! Overview metrics and the protocol used by the per-feature monitor channels
//! (see services.rs, containers.rs and compose.rs).
//! A long-lived sampler keeps the previous reading
//! so CPU, per-core and per-process usage describe the last second, not a
//! lifetime average. Numbered protobuf messages carry observations and actions.
use newport_protocol::{
    common::{Error, Retry},
    monitor::metrics::{
        Core, Disk, MAX_COMMAND, MAX_CPUS, MAX_DISKS as MAX_MOUNTS, MAX_HISTORY as CORE_HISTORY,
        MAX_INTERFACES, Network, Process, Snapshot,
    },
    monitor::{Reply, Request, Response, wire},
};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const INTERVAL: Duration = Duration::from_secs(1);
const MAX_PROCESSES: usize = 32768;
const MAX_FILE: u64 = 1 << 20;
const TOP_PROCESSES: usize = 50;

/// Free space as statvfs reports it, in bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FsUsage {
    pub total: u64,
    pub used: u64,
    pub available: i64,
}

/// Where readings come from; tests point it at a fake tree.
pub struct Source {
    pub root: PathBuf,
    pub clock_ticks: f64,
    pub page_size: u64,
    pub statvfs: fn(&str) -> Option<FsUsage>,
}
impl Source {
    pub fn system() -> Self {
        // SAFETY: sysconf has no preconditions; failures return -1.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        Self {
            root: PathBuf::from("/"),
            clock_ticks: if ticks > 0 { ticks as f64 } else { 100.0 },
            page_size: if page > 0 { page as u64 } else { 4096 },
            statvfs,
        }
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
    fn read(&self, relative: &str) -> Option<String> {
        read_capped(&self.path(relative))
    }
}

fn read_capped(path: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_FILE)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn statvfs(path: &str) -> Option<FsUsage> {
    let path = std::ffi::CString::new(path).ok()?;
    // SAFETY: a zeroed statvfs is a valid out-parameter; the path is NUL-terminated.
    let mut fs: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut fs) } != 0 {
        return None;
    }
    let unit = fs.f_frsize as u64;
    Some(FsUsage {
        total: fs.f_blocks as u64 * unit,
        used: (fs.f_blocks as u64).saturating_sub(fs.f_bfree as u64) * unit,
        available: (fs.f_bavail as u64 * unit).min(i64::MAX as u64) as i64,
    })
}

/// Busy and total jiffies; "idle" includes iowait.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Times {
    total: u64,
    idle: u64,
}
#[derive(Clone, Debug)]
struct Task {
    ticks: u64,
    /// Start time in clock ticks since boot (`/proc/[pid]/stat` field 22).
    started: u64,
    rss_pages: u64,
    state: String,
    name: String,
}
#[derive(Clone, Debug)]
struct Sample {
    at: Instant,
    cpu: Times,
    cores: BTreeMap<u32, Times>,
    /// Boot wall time (`/proc/stat` btime), for process ages. Zero when unread.
    boot: u64,
    tasks: HashMap<u32, Task>,
}

fn parse_stat(text: &str) -> (Times, BTreeMap<u32, Times>) {
    let mut cpu = Times::default();
    let mut cores = BTreeMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(id) = fields.next().and_then(|name| name.strip_prefix("cpu")) else {
            break;
        };
        let values: Vec<u64> = fields.take(8).map(|v| v.parse().unwrap_or(0)).collect();
        let times = Times {
            total: values.iter().sum(),
            idle: values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0),
        };
        if id.is_empty() {
            cpu = times;
        } else if let Ok(id) = id.parse::<u32>()
            && cores.len() < MAX_CPUS
        {
            cores.insert(id, times);
        }
    }
    (cpu, cores)
}

/// `/proc/[pid]/stat`: the name is parenthesised and may itself hold spaces
/// or parentheses, so fields are counted from the last ')'.
fn parse_task(text: &str) -> Option<Task> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text.get(open + 1..close)?.to_owned();
    let fields: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    // After the name: state is field 3, utime 14, stime 15, starttime 22,
    // rss 24 (1-based). A missing starttime only loses the age, never the row.
    let number = |index: usize| fields.get(index - 3)?.parse::<u64>().ok();
    Some(Task {
        state: fields.first()?.to_string(),
        ticks: number(14)? + number(15)?,
        started: number(22).unwrap_or(0),
        rss_pages: number(24).unwrap_or(0),
        name,
    })
}

/// Boot wall time (`/proc/stat` btime), so process start ticks resolve to an
/// age without trusting the client's clock. Zero when unread.
fn parse_btime(text: &str) -> u64 {
    text.lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            if fields.next()? != "btime" {
                return None;
            }
            fields.next()?.parse().ok()
        })
        .unwrap_or(0)
}

/// Seconds since the process started, from its start tick and the boot time.
/// `None` when the age is unknowable (no boot time, no start tick); a
/// just-started process is `Some(0)`, never confused with unknown.
fn elapsed_secs(now_epoch: u64, boot_epoch: u64, started_ticks: u64, hz: f64) -> Option<u64> {
    if boot_epoch == 0 || started_ticks == 0 || hz <= 0.0 {
        return None;
    }
    let start = boot_epoch.saturating_add((started_ticks as f64 / hz) as u64);
    Some(now_epoch.saturating_sub(start))
}

impl Source {
    fn sample(&self) -> Sample {
        let stat = self.read("proc/stat").unwrap_or_default();
        let (cpu, cores) = parse_stat(&stat);
        let boot = parse_btime(&stat);
        let mut tasks = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(self.path("proc")) {
            for entry in entries.flatten() {
                if tasks.len() >= MAX_PROCESSES {
                    break;
                }
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.parse::<u32>().ok())
                else {
                    continue;
                };
                // A process can exit between listing and reading; skip it.
                if let Some(task) =
                    read_capped(&entry.path().join("stat")).and_then(|t| parse_task(&t))
                {
                    tasks.insert(pid, task);
                }
            }
        }
        Sample {
            at: Instant::now(),
            cpu,
            cores,
            boot,
            tasks,
        }
    }
}

fn share(previous: Times, latest: Times) -> f64 {
    let total = latest.total.saturating_sub(previous.total);
    let idle = latest.idle.saturating_sub(previous.idle);
    if total == 0 {
        return 0.0;
    }
    (100.0 * (1.0 - idle as f64 / total as f64)).clamp(0.0, 100.0)
}

/// `/proc/[pid]/cmdline` is NUL-separated argv; shown space-joined and capped.
fn command_line(raw: &str) -> Option<String> {
    let mut command = raw
        .split('\0')
        .filter(|arg| !arg.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if command.is_empty() {
        return None;
    }
    if command.len() > MAX_COMMAND {
        let mut end = MAX_COMMAND - '…'.len_utf8();
        while !command.is_char_boundary(end) {
            end -= 1;
        }
        command.truncate(end);
        command.push('…');
    }
    Some(command)
}

fn users(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let uid = fields.nth(1)?.parse().ok()?;
            Some((uid, name.to_owned()))
        })
        .collect()
}
fn uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Mount table paths escape space, tab, newline and backslash as octal.
fn unescape(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value =
                (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            out.push(value);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Capacity-bearing local filesystems: a /dev-backed source, or the root mount.
/// SquashFS package images (including Snap) are immutable and normally report
/// no free space. Their backing files are already counted on the real disk.
fn mounts(text: &str) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mount), Some(filesystem)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if filesystem == "squashfs" {
            continue;
        }
        let (device, mount) = (unescape(device), unescape(mount));
        if !(device.starts_with("/dev/") || mount == "/") || seen.contains(&mount) {
            continue;
        }
        seen.push(mount.clone());
        out.push((device, mount));
        if out.len() >= MAX_MOUNTS {
            break;
        }
    }
    out
}

fn network(text: &str) -> Vec<Network> {
    text.lines()
        .filter_map(|line| {
            let (name, counters) = line.split_once(':')?;
            let fields: Vec<u64> = counters
                .split_whitespace()
                .map(|v| v.parse().unwrap_or(0))
                .collect();
            Some(Network {
                name: name.trim().into(),
                received: fields.first().copied().unwrap_or(0),
                sent: fields.get(8).copied().unwrap_or(0),
            })
        })
        .take(MAX_INTERFACES)
        .collect()
}

fn meminfo(text: &str) -> HashMap<&str, u64> {
    text.lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
            Some((key, kib * 1024))
        })
        .collect()
}

fn os_name(text: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim().trim_matches('"').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Linux".into())
}

impl Source {
    fn snapshot(&self, previous: &Sample, latest: &Sample) -> Snapshot {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|age| age.as_secs())
            .unwrap_or(0);
        self.snapshot_at(previous, latest, now)
    }
    fn snapshot_at(&self, previous: &Sample, latest: &Sample, now_epoch: u64) -> Snapshot {
        let elapsed = latest
            .at
            .duration_since(previous.at)
            .as_secs_f64()
            .max(0.001);
        let mut processes: Vec<(f64, u32, &Task)> = latest
            .tasks
            .iter()
            .map(|(pid, task)| {
                // First-seen or reused pids have no fair baseline yet.
                let cpu = previous
                    .tasks
                    .get(pid)
                    .filter(|before| before.ticks <= task.ticks)
                    .map(|before| {
                        (task.ticks - before.ticks) as f64 / (elapsed * self.clock_ticks) * 100.0
                    })
                    .unwrap_or(0.0);
                (cpu, *pid, task)
            })
            .collect();
        processes.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then(b.2.rss_pages.cmp(&a.2.rss_pages))
                .then(a.1.cmp(&b.1))
        });
        let names = users(&self.read("etc/passwd").unwrap_or_default());
        // The busiest by CPU and the largest by memory, so sorting the list
        // either way shows the real leaders rather than re-sorting one set.
        let mut by_memory: Vec<&(f64, u32, &Task)> = processes.iter().collect();
        by_memory.sort_by(|a, b| b.2.rss_pages.cmp(&a.2.rss_pages).then(a.1.cmp(&b.1)));
        let largest: std::collections::HashSet<u32> = by_memory
            .iter()
            .take(TOP_PROCESSES)
            .map(|(_, pid, _)| *pid)
            .collect();
        let top: Vec<Process> = processes
            .iter()
            .enumerate()
            .filter(|(rank, (_, pid, _))| *rank < TOP_PROCESSES || largest.contains(pid))
            .map(|(_, process)| process)
            .map(|(cpu, pid, task)| Process {
                pid: *pid,
                user: self
                    .read(&format!("proc/{pid}/status"))
                    .and_then(|status| uid(&status))
                    .map(|uid| names.get(&uid).cloned().unwrap_or_else(|| uid.to_string()))
                    .unwrap_or_default(),
                cpu: (cpu * 10.0).round() / 10.0,
                memory: task.rss_pages * self.page_size,
                state: task.state.clone(),
                name: task.name.clone(),
                elapsed: elapsed_secs(now_epoch, latest.boot, task.started, self.clock_ticks),
                command: self
                    .read(&format!("proc/{pid}/cmdline"))
                    .and_then(|raw| command_line(&raw)),
            })
            .collect();
        let cpus = latest
            .cores
            .iter()
            .filter_map(|(id, times)| {
                let before = previous.cores.get(id)?;
                Some(Core {
                    id: *id,
                    usage: (share(*before, *times) * 10.0).round() / 10.0,
                    history: Vec::new(),
                })
            })
            .collect();
        let memory = self.read("proc/meminfo").unwrap_or_default();
        let memory = meminfo(&memory);
        let total = memory.get("MemTotal").copied().unwrap_or(0);
        let available = memory
            .get("MemAvailable")
            .or(memory.get("MemFree"))
            .copied()
            .unwrap_or(0);
        let swap_total = memory.get("SwapTotal").copied().unwrap_or(0);
        let swap_free = memory.get("SwapFree").copied().unwrap_or(0);
        let load: Vec<f64> = self
            .read("proc/loadavg")
            .unwrap_or_default()
            .split_whitespace()
            .take(3)
            .map(|v| v.parse().unwrap_or(0.0))
            .collect();
        let uptime = self
            .read("proc/uptime")
            .and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok())
            .unwrap_or(0.0);
        let line = |relative: &str| self.read(relative).unwrap_or_default().trim().to_owned();
        let disks = mounts(&self.read("proc/self/mounts").unwrap_or_default())
            .into_iter()
            .filter_map(|(device, mount)| {
                let usage = (self.statvfs)(&mount)?;
                Some(Disk {
                    mount,
                    device,
                    total: usage.total,
                    used: usage.used,
                    available: usage.available,
                })
            })
            .collect();
        Snapshot {
            hostname: line("proc/sys/kernel/hostname"),
            os: os_name(&self.read("etc/os-release").unwrap_or_default()),
            kernel: line("proc/sys/kernel/osrelease"),
            cores: latest.cores.len().max(1) as u32,
            uptime,
            load: [
                load.first().copied().unwrap_or(0.0),
                load.get(1).copied().unwrap_or(0.0),
                load.get(2).copied().unwrap_or(0.0),
            ],
            cpu: share(previous.cpu, latest.cpu),
            cpus,
            memory_total: total,
            memory_used: total.saturating_sub(available),
            swap_total,
            swap_used: swap_total.saturating_sub(swap_free),
            disks,
            network: network(&self.read("proc/net/dev").unwrap_or_default()),
            processes: top,
            process_count: latest.tasks.len() as u32,
        }
    }
}

/// The two most recent samples, refreshed by a background thread, and each
/// core's usage over the last minute.
pub struct Monitor {
    source: Source,
    samples: Mutex<(Option<Sample>, Option<Sample>)>,
    history: Mutex<BTreeMap<u32, VecDeque<f64>>>,
    ready: Condvar,
    started: OnceLock<()>,
}
impl Monitor {
    pub fn new(source: Source) -> Arc<Self> {
        Arc::new(Self {
            source,
            samples: Mutex::new((None, None)),
            history: Mutex::new(BTreeMap::new()),
            ready: Condvar::new(),
            started: OnceLock::new(),
        })
    }
    fn tick(&self) {
        let sample = self.source.sample();
        let mut samples = self
            .samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        samples.0 = samples.1.take();
        samples.1 = Some(sample);
        if let (Some(previous), Some(latest)) = &*samples {
            let mut history = self
                .history
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A core that went offline takes its history with it.
            history.retain(|id, _| latest.cores.contains_key(id));
            for (id, times) in &latest.cores {
                let Some(before) = previous.cores.get(id) else {
                    continue;
                };
                let values = history.entry(*id).or_default();
                if values.len() == CORE_HISTORY {
                    values.pop_front();
                }
                values.push_back((share(*before, *times) * 10.0).round() / 10.0);
            }
        }
        self.ready.notify_all();
    }
    pub fn start(self: &Arc<Self>) {
        self.started.get_or_init(|| {
            let monitor = Arc::downgrade(self);
            std::thread::spawn(move || {
                loop {
                    let Some(current) = monitor.upgrade() else {
                        break;
                    };
                    current.tick();
                    drop(current);
                    std::thread::sleep(INTERVAL);
                }
            });
        });
    }
    /// Waits briefly for a second sample so the first reply already has rates.
    fn snapshot(&self) -> Result<Snapshot, Error> {
        let samples = self
            .samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (samples, _) = self
            .ready
            .wait_timeout_while(samples, INTERVAL * 3, |s| s.0.is_none() || s.1.is_none())
            .unwrap();
        let mut snapshot = match &*samples {
            (Some(previous), Some(latest)) => self.source.snapshot(previous, latest),
            _ => {
                return Err(Error {
                    code: "not_ready".into(),
                    message: "The first readings are not ready yet.".into(),
                    retry: Retry::Yes,
                });
            }
        };
        drop(samples);
        let history = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for core in &mut snapshot.cpus {
            if let Some(values) = history.get(&core.id) {
                core.history = values.iter().copied().collect();
            }
        }
        snapshot.validate().map_err(|error| Error {
            code: "invalid_metrics".into(),
            message: error.to_string(),
            retry: Retry::No,
        })?;
        Ok(snapshot)
    }
}

/// Answers monitor calls until stdin closes.
pub fn handle(
    monitor: &Monitor,
    services: &crate::services::Services,
    containers: &crate::containers::Containers,
    input: impl BufRead,
    output: impl Write,
) -> io::Result<()> {
    handle_bound(
        || monitor.snapshot(),
        services,
        containers,
        input,
        output,
        &crate::transport::Cancellation::default(),
    )
}
fn handle_bound(
    snapshot: impl Fn() -> Result<Snapshot, Error>,
    services: &crate::services::Services,
    containers: &crate::containers::Containers,
    input: impl BufRead,
    mut output: impl Write,
    cancellation: &crate::transport::Cancellation,
) -> io::Result<()> {
    let mut input = input;
    let mut file_watches: Option<crate::file_watch::Watches> = None;
    #[cfg(target_os = "linux")]
    let mut ports = crate::ports::discovery::Session::default();
    loop {
        cancellation.check()?;
        let Some(call) = wire::read_request(&mut input)? else {
            return Ok(());
        };
        cancellation.check()?;
        let result = match &call.request {
            Request::Metrics => snapshot().map(Reply::Metrics),
            Request::FileWatch(request) => file_watches
                .get_or_insert_with(crate::file_watch::Watches::new)
                .handle(request)
                .map(Reply::FileWatch),
            #[cfg(target_os = "linux")]
            Request::Ports(request) => ports
                .handle(request, containers, &|| cancellation.check().is_err())
                .map(|snapshot| Reply::Ports(Box::new(snapshot))),
            #[cfg(not(target_os = "linux"))]
            Request::Ports(_) => Err(Error {
                code: "unsupported_platform".into(),
                message: "Port discovery requires a Linux server.".into(),
                retry: Retry::No,
            }),
            Request::StableDisplay => Ok(Reply::StableDisplay(
                crate::gui_attachment::live_stable_display(
                    crate::gui_attachment::stable_display_folder().as_deref(),
                ),
            )),
            Request::Services(request) => services.handle(request).map(Reply::Services),
            Request::Containers(request) => containers.handle(request).map(Reply::Containers),
            Request::Compose(request) => containers.compose(request).map(Reply::Compose),
        };
        let response = Response {
            id: call.id,
            reply: result.unwrap_or_else(failure),
        };
        let bytes = match wire::encode_reply(&response, &call) {
            Ok(bytes) => bytes,
            Err(error) => wire::encode_reply(
                &Response {
                    id: call.id,
                    reply: failure(Error {
                        code: "invalid_response".into(),
                        message: error.to_string(),
                        retry: Retry::No,
                    }),
                },
                &call,
            )?,
        };
        output.write_all(&bytes)?;
        output.flush()?;
    }
}

fn failure(mut error: Error) -> Reply {
    error.message = error.message.replace('\0', "�");
    if error.message.is_empty() {
        error.message = "Monitor request failed.".into();
    }
    let mut end = error
        .message
        .len()
        .min(newport_protocol::monitor::MAX_ERROR_MESSAGE);
    while !error.message.is_char_boundary(end) {
        end -= 1;
    }
    error.message.truncate(end);
    Reply::Failed(error)
}

pub fn serve() -> io::Result<()> {
    serve_bound(crate::transport::Cancellation::default())
}
pub(crate) fn serve_bound(cancellation: crate::transport::Cancellation) -> io::Result<()> {
    let monitor = Monitor::new(Source::system());
    let services = crate::services::Services::system();
    let containers = crate::containers::Containers::system();
    let result = handle_bound(
        || {
            monitor.start();
            monitor.snapshot()
        },
        &services,
        &containers,
        io::BufReader::new(cancellation.stdin()?),
        io::stdout().lock(),
        &cancellation,
    );
    // The desktop may close the channel while a Compose command runs; it
    // finishes rather than losing its output pipes half-way.
    containers.wait_for_compose(Duration::from_secs(3600));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn usage(mount: &str) -> Option<FsUsage> {
        match mount {
            "/" => Some(FsUsage {
                total: 100,
                used: 40,
                available: 55,
            }),
            "/srv/my data" => Some(FsUsage {
                total: 1000,
                used: 900,
                available: 100,
            }),
            _ => None,
        }
    }
    fn source(root: &Path) -> Source {
        Source {
            root: root.into(),
            clock_ticks: 100.0,
            page_size: 4096,
            statvfs: usage,
        }
    }
    fn task(pid: u32, name: &str, ticks: u64, rss: u64) -> String {
        // pid (comm) state then fields 4..24; utime=14 and stime=15 split the ticks.
        let mut fields = vec!["0".to_string(); 22];
        fields[0] = "S".into();
        fields[11] = (ticks / 2).to_string();
        fields[12] = (ticks - ticks / 2).to_string();
        fields[21] = rss.to_string();
        format!("{pid} ({name}) {}", fields.join(" "))
    }
    fn at(sample: &mut Sample, base: Instant, seconds: f64) {
        sample.at = base + Duration::from_secs_f64(seconds);
    }

    #[test]
    fn task_names_may_hold_spaces_and_parentheses() {
        let parsed = parse_task(&task(7, "web (worker) 2", 30, 12)).unwrap();
        assert_eq!(parsed.name, "web (worker) 2");
        assert_eq!(parsed.ticks, 30);
        assert_eq!(parsed.rss_pages, 12);
        assert_eq!(parsed.state, "S");
    }

    #[test]
    fn task_starttime_is_field_22() {
        let parsed =
            parse_task("10 (busy) S 0 0 0 0 0 0 0 0 0 0 40 40 0 0 0 0 0 0 100 0 10").unwrap();
        assert_eq!(parsed.ticks, 80);
        assert_eq!(parsed.started, 100);
        assert_eq!(parsed.rss_pages, 10);
    }

    #[test]
    fn btime_comes_from_proc_stat() {
        assert_eq!(
            parse_btime("cpu  100 0 0 900 0 0 0 0\nbtime 1700000000\nintr 1\n"),
            1700000000
        );
        assert_eq!(parse_btime("cpu  100 0 0 900 0 0 0 0\n"), 0);
        assert_eq!(parse_btime("btime not-a-number\n"), 0);
    }

    #[test]
    fn elapsed_needs_boot_start_and_rate() {
        assert_eq!(
            elapsed_secs(1_700_003_601, 1_700_000_000, 100, 100.0),
            Some(3600)
        );
        // A just-started process is zero, never confused with unknown.
        assert_eq!(
            elapsed_secs(1_700_000_001, 1_700_000_000, 100, 100.0),
            Some(0)
        );
        // Clock skew the other way saturates instead of underflowing.
        assert_eq!(elapsed_secs(100, 1000, 5000, 100.0), Some(0));
        assert_eq!(elapsed_secs(1_700_003_601, 0, 100, 100.0), None);
        assert_eq!(elapsed_secs(1_700_003_601, 1_700_000_000, 0, 100.0), None);
        assert_eq!(elapsed_secs(1_700_003_601, 1_700_000_000, 100, 0.0), None);
    }

    #[test]
    fn snapshot_reports_process_ages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = source(root);
        write(
            root,
            "proc/stat",
            "cpu  100 0 0 900 0 0 0 0\nbtime 1700000000\n",
        );
        write(
            root,
            "proc/10/stat",
            "10 (old) S 0 0 0 0 0 0 0 0 0 0 40 40 0 0 0 0 0 0 100 0 10",
        );
        let first = source.sample();
        let second = source.sample();
        let snapshot = source.snapshot_at(&first, &second, 1_700_003_601);
        let processes = &snapshot.processes;
        assert_eq!(processes[0].name, "old");
        // Started one second after boot, read one hour later.
        assert_eq!(processes[0].elapsed, Some(3600));
    }

    #[test]
    fn snapshot_omits_ages_without_a_boot_time() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = source(root);
        write(root, "proc/stat", "cpu  100 0 0 900 0 0 0 0\n");
        write(
            root,
            "proc/10/stat",
            "10 (old) S 0 0 0 0 0 0 0 0 0 0 40 40 0 0 0 0 0 0 100 0 10",
        );
        let first = source.sample();
        let second = source.sample();
        let snapshot = source.snapshot_at(&first, &second, 1_700_003_601);
        assert!(snapshot.processes[0].elapsed.is_none());
    }

    #[test]
    fn rates_come_from_the_last_interval() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = source(root);
        write(
            root,
            "proc/stat",
            "cpu  100 0 0 900 0 0 0 0\ncpu0 50 0 0 450 0 0 0 0\ncpu2 50 0 0 450 0 0 0 0\nintr 1\n",
        );
        write(root, "proc/10/stat", &task(10, "busy", 100, 10));
        write(root, "proc/11/stat", &task(11, "idle", 5, 99));
        let base = Instant::now();
        let mut first = source.sample();
        at(&mut first, base, 0.0);
        // One second later: cpu0 fully busy, cpu2 idle; pid 10 used 80 ticks,
        // pid 12 appeared, pid 11 exited.
        write(
            root,
            "proc/stat",
            "cpu  200 0 0 1000 0 0 0 0\ncpu0 150 0 0 450 0 0 0 0\ncpu2 50 0 0 550 0 0 0 0\n",
        );
        write(root, "proc/10/stat", &task(10, "busy", 180, 10));
        fs::remove_dir_all(root.join("proc/11")).unwrap();
        write(root, "proc/12/stat", &task(12, "new", 500, 50));
        let mut second = source.sample();
        at(&mut second, base, 1.0);
        let snapshot = source.snapshot(&first, &second);
        snapshot.validate().unwrap();
        assert_eq!(snapshot.cpu, 50.0);
        assert_eq!(snapshot.cores, 2);
        assert_eq!(
            snapshot.cpus,
            [
                Core {
                    id: 0,
                    usage: 100.0,
                    history: Vec::new()
                },
                Core {
                    id: 2,
                    usage: 0.0,
                    history: Vec::new()
                }
            ]
        );
        let processes = &snapshot.processes;
        assert_eq!(processes[0].name, "busy");
        assert_eq!(processes[0].cpu, 80.0);
        // A pid with no baseline reports nothing yet, however many ticks it has.
        assert_eq!(processes[1].name, "new");
        assert_eq!(processes[1].cpu, 0.0);
        assert_eq!(processes[1].memory, 50 * 4096);
        assert_eq!(snapshot.process_count, 2);
    }

    #[test]
    fn processes_are_the_busiest_and_the_largest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = source(root);
        write(root, "proc/stat", "cpu  0 0 0 0 0 0 0 0\n");
        for pid in 1..=60 {
            write(
                root,
                &format!("proc/{pid}/stat"),
                &task(pid, "p", 0, pid as u64),
            );
        }
        let base = Instant::now();
        let mut first = source.sample();
        at(&mut first, base, 0.0);
        write(root, "proc/7/stat", &task(7, "hot", 250, 1));
        let mut second = source.sample();
        at(&mut second, base, 2.0);
        let snapshot = source.snapshot(&first, &second);
        snapshot.validate().unwrap();
        let processes = &snapshot.processes;
        // 250 ticks over 2 s at 100 Hz is 125%: more than one core, as htop shows.
        assert_eq!(processes[0].pid, 7);
        assert_eq!(processes[0].cpu, 125.0);
        assert_eq!(processes[1].pid, 60);
        assert_eq!(processes[2].pid, 59);
        // The top 50 by CPU (7, then 60..=12 by memory) plus the top 50 by
        // memory adds 11, which is large but idle; the smallest stay out.
        let pids: Vec<u32> = processes.iter().map(|p| p.pid).collect();
        assert_eq!(pids.len(), 51);
        assert!(pids.contains(&11));
        assert!(!pids.contains(&10));
        assert_eq!(snapshot.process_count, 60);
    }

    #[test]
    fn command_lines_are_joined_capped_and_absent_for_kernel_threads() {
        assert_eq!(
            command_line("node\0/srv/site/server.js\0--port\0\x34173\0").as_deref(),
            Some("node /srv/site/server.js --port 4173")
        );
        assert_eq!(command_line(""), None);
        let long = command_line(&format!("x\0{}", "é".repeat(400))).unwrap();
        assert!(long.ends_with('…'));
        assert!(long.len() <= MAX_COMMAND);
    }

    #[test]
    fn identity_memory_disks_and_network() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = source(root);
        write(
            root,
            "proc/stat",
            "cpu  1 0 0 1 0 0 0 0\ncpu0 1 0 0 1 0 0 0 0\n",
        );
        write(root, "proc/sys/kernel/hostname", "atlas-01\n");
        write(root, "proc/sys/kernel/osrelease", "6.12.0\n");
        write(
            root,
            "etc/os-release",
            "NAME=Debian\nPRETTY_NAME=\"Debian GNU/Linux 13\"\n",
        );
        write(
            root,
            "etc/passwd",
            "root:x:0:0::/root:/bin/sh\nci:x:1000:1000::/home/ci:/bin/sh\n",
        );
        write(
            root,
            "proc/meminfo",
            "MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 400 kB\nSwapTotal: 200 kB\nSwapFree: 150 kB\n",
        );
        write(root, "proc/loadavg", "0.50 0.40 0.30 1/100 4242\n");
        write(root, "proc/uptime", "1234.5 99.0\n");
        write(
            root,
            "proc/self/mounts",
            "/dev/sda1 / ext4 rw 0 0\nproc /proc proc rw 0 0\ntmpfs /run tmpfs rw 0 0\n/dev/sdb1 /srv/my\\040data ext4 rw 0 0\n/dev/sdb1 /srv/my\\040data ext4 rw 0 0\n/dev/sdc1 /broken ext4 rw 0 0\n",
        );
        write(
            root,
            "proc/net/dev",
            "Inter-|   Receive\n face |bytes\n    lo: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n  eth0: 300 5 0 0 0 0 0 0 400 6 0 0 0 0 0 0\n",
        );
        write(root, "proc/9/stat", &task(9, "sshd", 0, 3));
        write(
            root,
            "proc/9/status",
            "Name:\tsshd\nUid:\t1000\t1000\t1000\t1000\n",
        );
        let first = source.sample();
        let snapshot = source.snapshot(&first, &first);
        snapshot.validate().unwrap();
        assert_eq!(snapshot.hostname, "atlas-01");
        assert_eq!(snapshot.kernel, "6.12.0");
        assert_eq!(snapshot.os, "Debian GNU/Linux 13");
        assert_eq!(snapshot.memory_total, 1000 * 1024);
        assert_eq!(snapshot.memory_used, 600 * 1024);
        assert_eq!(snapshot.swap_total, 200 * 1024);
        assert_eq!(snapshot.swap_used, 50 * 1024);
        assert_eq!(snapshot.load, [0.5, 0.4, 0.3]);
        assert_eq!(snapshot.uptime, 1234.5);
        // /dev-backed or "/", deduplicated, unescaped; an unreadable one is skipped.
        assert_eq!(
            snapshot.disks,
            [
                Disk {
                    mount: "/".into(),
                    device: "/dev/sda1".into(),
                    total: 100,
                    used: 40,
                    available: 55
                },
                Disk {
                    mount: "/srv/my data".into(),
                    device: "/dev/sdb1".into(),
                    total: 1000,
                    used: 900,
                    available: 100
                },
            ]
        );
        assert_eq!(
            snapshot.network,
            [
                Network {
                    name: "lo".into(),
                    received: 10,
                    sent: 20
                },
                Network {
                    name: "eth0".into(),
                    received: 300,
                    sent: 400
                },
            ]
        );
        assert_eq!(snapshot.processes[0].user, "ci");
        // Zero elapsed time never divides by zero.
        assert_eq!(snapshot.cpu, 0.0);
    }

    #[test]
    fn storage_ignores_package_images_without_hiding_real_full_disks() {
        let dir = tempfile::tempdir().unwrap();
        // Installed package revisions must not consume the disk reporting
        // budget, even when they precede the physical disks in the mount table.
        let mut table = (0..=MAX_MOUNTS)
            .map(|n| format!("/dev/loop{n} /snap/package/{n} squashfs ro,nodev 0 0\n"))
            .collect::<String>();
        table.push_str(
            "/dev/loop900 /var/lib/snapd/snap/editor/1 squashfs ro 0 0\n\
             /dev/loop901 /opt/application-image squashfs ro 0 0\n\
             overlay / overlay rw 0 0\n\
             /dev/sda2 /boot ext4 rw 0 0\n\
             /dev/sda1 /boot/efi vfat rw 0 0\n\
             /dev/sdb1 /snap/data ext4 rw 0 0\n\
             /dev/loop902 /srv/loop\\040data ext4 rw 0 0\n\
             /dev/sdc1 /recovery ext4 ro 0 0\n",
        );
        write(dir.path(), "proc/self/mounts", &table);
        let mut source = source(dir.path());
        source.statvfs = |_| {
            Some(FsUsage {
                total: 100,
                used: 100,
                available: 0,
            })
        };
        let sample = source.sample();
        let snapshot = source.snapshot(&sample, &sample);
        let disks = &snapshot.disks;
        assert_eq!(
            disks
                .iter()
                .map(|disk| disk.mount.as_str())
                .collect::<Vec<_>>(),
            [
                "/",
                "/boot",
                "/boot/efi",
                "/snap/data",
                "/srv/loop data",
                "/recovery"
            ]
        );
        assert!(
            disks
                .iter()
                .all(|disk| disk.used == 100 && disk.available == 0)
        );
    }

    #[test]
    fn memory_falls_back_to_free_without_available() {
        assert_eq!(
            meminfo("MemTotal: 10 kB\nMemFree: 4 kB\n").get("MemAvailable"),
            None
        );
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "proc/meminfo", "MemTotal: 10 kB\nMemFree: 4 kB\n");
        let source = source(root);
        let sample = source.sample();
        assert_eq!(source.snapshot(&sample, &sample).memory_used, 6 * 1024);
    }

    #[test]
    fn each_core_keeps_its_last_minute_of_usage() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let monitor = Monitor::new(source(root));
        // Core 0 is busy for every interval; core 1 idles, then goes offline.
        let stat = |n: u64, with_one: bool| {
            let mut text = format!(
                "cpu  {} 0 0 {} 0 0 0 0\ncpu0 {} 0 0 0 0 0 0 0\n",
                n * 10,
                n * 10,
                n * 10
            );
            if with_one {
                text.push_str(&format!("cpu1 0 0 0 {} 0 0 0 0\n", n * 10));
            }
            text
        };
        for n in 0..=CORE_HISTORY as u64 + 5 {
            write(root, "proc/stat", &stat(n, true));
            monitor.tick();
        }
        let snapshot = monitor.snapshot().unwrap();
        let busy = &snapshot.cpus[0].history;
        assert_eq!(busy.len(), CORE_HISTORY, "capped at a minute");
        assert!(busy.iter().all(|v| *v == 100.0));
        assert!(snapshot.cpus[1].history.iter().all(|v| *v == 0.0));
        write(root, "proc/stat", &stat(CORE_HISTORY as u64 + 6, false));
        monitor.tick();
        assert!(!monitor.history.lock().unwrap().contains_key(&1));
    }

    /// Services that never get to run anything: these tests send no services ops.
    fn no_services() -> crate::services::Services {
        crate::services::Services::new(Box::new(crate::services::System), 0, Vec::new())
    }
    fn no_containers() -> crate::containers::Containers {
        crate::containers::Containers::new(Box::new(Vec::new))
    }

    fn call(id: u64, request: Request) -> newport_protocol::monitor::Call {
        newport_protocol::monitor::Call { id, request }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unsupported_discovery_keeps_the_monitor_stream_usable() {
        use newport_protocol::monitor::ports::discovery;
        let calls = [
            call(1, Request::Ports(discovery::Request { refresh: false })),
            call(2, Request::Ports(discovery::Request { refresh: true })),
            call(3, Request::StableDisplay),
        ];
        let input: Vec<_> = calls
            .iter()
            .flat_map(|call| wire::encode_request(call).unwrap())
            .collect();
        let mut output = Vec::new();
        handle_bound(
            || panic!("discovery must not sample metrics"),
            &no_services(),
            &no_containers(),
            input.as_slice(),
            &mut output,
            &crate::transport::Cancellation::default(),
        )
        .unwrap();
        let mut output = output.as_slice();
        for call in &calls[..2] {
            let Reply::Failed(error) = wire::read_reply(&mut output, call).unwrap().reply else {
                panic!("unsupported discovery must not look like an empty successful scan");
            };
            assert_eq!(error.code, "unsupported_platform");
            assert_eq!(error.retry, Retry::No);
            assert_eq!(error.message, "Port discovery requires a Linux server.");
        }
        assert!(matches!(
            wire::read_reply(&mut output, &calls[2]).unwrap().reply,
            Reply::StableDisplay(_)
        ));
        assert!(output.is_empty());
    }

    #[test]
    fn nonmetrics_channel_does_not_start_process_sampling() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = Monitor::new(source(dir.path()));
        let call = call(
            1,
            Request::Containers(newport_protocol::monitor::containers::Request::Snapshot),
        );
        let mut output = Vec::new();
        handle_bound(
            || {
                monitor.start();
                monitor.snapshot()
            },
            &no_services(),
            &no_containers(),
            wire::encode_request(&call).unwrap().as_slice(),
            &mut output,
            &crate::transport::Cancellation::default(),
        )
        .unwrap();
        assert!(monitor.started.get().is_none());
        assert!(monitor.samples.lock().unwrap().1.is_none());
        let Reply::Failed(error) = wire::read_reply(&mut output.as_slice(), &call)
            .unwrap()
            .reply
        else {
            panic!("missing engine must fail");
        };
        assert_eq!(error.message, crate::docker::NOT_RUNNING);
    }

    #[test]
    fn the_stable_display_request_says_whether_it_runs() {
        let call = call(9, Request::StableDisplay);
        let mut output = Vec::new();
        handle_bound(
            || panic!("stable display requests must not sample metrics"),
            &no_services(),
            &no_containers(),
            wire::encode_request(&call).unwrap().as_slice(),
            &mut output,
            &crate::transport::Cancellation::default(),
        )
        .unwrap();
        let Reply::StableDisplay(state) = wire::read_reply(&mut output.as_slice(), &call)
            .unwrap()
            .reply
        else {
            panic!("display state");
        };
        state.validate().unwrap();
    }

    #[test]
    fn protocol_answers_numbered_calls_until_stdin_closes() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "proc/stat", "cpu  1 0 0 1 0 0 0 0\n");
        let monitor = Monitor::new(source(dir.path()));
        monitor.tick();
        monitor.tick();
        let requests = [
            call(11, Request::Metrics),
            call(
                12,
                Request::Containers(newport_protocol::monitor::containers::Request::Snapshot),
            ),
        ];
        let input: Vec<u8> = requests
            .iter()
            .flat_map(|call| wire::encode_request(call).unwrap())
            .collect();
        let mut output = Vec::new();
        handle(
            &monitor,
            &no_services(),
            &no_containers(),
            input.as_slice(),
            &mut output,
        )
        .unwrap();
        let mut input = output.as_slice();
        assert!(matches!(
            wire::read_reply(&mut input, &requests[0]).unwrap().reply,
            Reply::Metrics(_)
        ));
        let Reply::Failed(error) = wire::read_reply(&mut input, &requests[1]).unwrap().reply else {
            panic!("engine unavailable");
        };
        assert_eq!(error.message, crate::docker::NOT_RUNNING);
        assert!(input.is_empty());
    }

    #[test]
    fn invalid_frames_execute_no_operation() {
        let mut oversized = Vec::new();
        newport_protocol::encoding::protobuf::write_frame(
            &mut oversized,
            &vec![0; newport_protocol::monitor::CLIENT_LIMITS.message + 1],
        )
        .unwrap();
        for bytes in [oversized, vec![0x82], vec![4, 0x0a, 2, 0x12, 0]] {
            assert!(
                handle_bound(
                    || panic!("invalid call reached sampling"),
                    &no_services(),
                    &no_containers(),
                    bytes.as_slice(),
                    Vec::new(),
                    &crate::transport::Cancellation::default()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn oversized_failures_and_invalid_observations_keep_the_stream_usable() {
        use std::cell::Cell;
        let next = Cell::new(0);
        let requests = [
            call(1, Request::Metrics),
            call(2, Request::Metrics),
            call(3, Request::Metrics),
        ];
        let input: Vec<u8> = requests
            .iter()
            .flat_map(|call| wire::encode_request(call).unwrap())
            .collect();
        let mut output = Vec::new();
        handle_bound(
            || {
                next.set(next.get() + 1);
                match next.get() {
                    1 => Err(Error {
                        code: "sample_failed".into(),
                        message: format!(
                            "cause: \0{}",
                            "é".repeat(newport_protocol::monitor::MAX_ERROR_MESSAGE)
                        ),
                        retry: Retry::Yes,
                    }),
                    2 => Ok(Snapshot::default()),
                    _ => Ok(Snapshot {
                        cores: 1,
                        ..Default::default()
                    }),
                }
            },
            &no_services(),
            &no_containers(),
            input.as_slice(),
            &mut output,
            &crate::transport::Cancellation::default(),
        )
        .unwrap();
        let mut input = output.as_slice();
        let Reply::Failed(error) = wire::read_reply(&mut input, &requests[0]).unwrap().reply else {
            panic!("sampling failure");
        };
        assert_eq!(error.code, "sample_failed");
        assert!(error.message.starts_with("cause: �"));
        assert!(error.message.len() <= newport_protocol::monitor::MAX_ERROR_MESSAGE);
        let Reply::Failed(error) = wire::read_reply(&mut input, &requests[1]).unwrap().reply else {
            panic!("invalid observation");
        };
        assert_eq!(error.code, "invalid_response");
        assert!(matches!(
            wire::read_reply(&mut input, &requests[2]).unwrap().reply,
            Reply::Metrics(_)
        ));
        assert!(input.is_empty());
    }
}
