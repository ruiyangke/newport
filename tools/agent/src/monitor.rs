//! Overview metrics and the protocol used by the per-feature monitor channels
//! (see services.rs, containers.rs and compose.rs).
//! A long-lived sampler keeps the previous reading
//! so CPU, per-core and per-process usage describe the last second, not a
//! lifetime average. Requests and replies are newline-delimited JSON on stdio.
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const INTERVAL: Duration = Duration::from_secs(1);
const MAX_PROCESSES: usize = 32768;
const MAX_CPUS: usize = 1024;
const MAX_INTERFACES: usize = 1024;
const MAX_MOUNTS: usize = 256;
const MAX_FILE: u64 = 1 << 20;
// Room for a few hundred container ids in one request.
const MAX_REQUEST: u64 = 64 << 10;
const TOP_PROCESSES: usize = 50;
/// Seconds of per-core usage kept for the cores grid, as Task Manager shows.
const CORE_HISTORY: usize = 60;

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

/// Busy and total jiffies; "idle" includes iowait, as cockpit.sh counts it.
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
        } else if let Ok(id) = id.parse::<u32>() {
            if cores.len() < MAX_CPUS {
                cores.insert(id, times);
            }
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

#[derive(Serialize)]
struct Process {
    pid: u32,
    user: String,
    cpu: f64,
    memory: u64,
    state: String,
    name: String,
    /// Seconds since the process started; absent when unknowable.
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed: Option<u64>,
    /// The command line, for the few processes sent; kernel threads have none.
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
}

/// Room for a readable command line without shipping whole environments.
const MAX_COMMAND: usize = 512;

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
        let mut end = MAX_COMMAND;
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

/// Local filesystems as `df -P -B1 -l` filters them in cockpit.sh: a
/// /dev-backed source, or whatever is mounted at "/".
fn mounts(text: &str) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mount)) = (fields.next(), fields.next()) else {
            continue;
        };
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

fn network(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let (name, counters) = line.split_once(':')?;
            let fields: Vec<u64> = counters
                .split_whitespace()
                .map(|v| v.parse().unwrap_or(0))
                .collect();
            Some(json!({
                "name": name.trim(),
                "received": fields.first().copied().unwrap_or(0),
                "sent": fields.get(8).copied().unwrap_or(0),
            }))
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
    /// The Overview contract (see crates/newport-backend/src/monitoring.rs), from two samples.
    fn snapshot(&self, previous: &Sample, latest: &Sample) -> Value {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|age| age.as_secs())
            .unwrap_or(0);
        self.snapshot_at(previous, latest, now)
    }
    fn snapshot_at(&self, previous: &Sample, latest: &Sample, now_epoch: u64) -> Value {
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
        let cpus: Vec<Value> = latest
            .cores
            .iter()
            .filter_map(|(id, times)| {
                let before = previous.cores.get(id)?;
                Some(json!({"id": id, "usage": (share(*before, *times) * 10.0).round() / 10.0}))
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
        let disks: Vec<Value> = mounts(&self.read("proc/self/mounts").unwrap_or_default())
            .into_iter()
            .filter_map(|(device, mount)| {
                let usage = (self.statvfs)(&mount)?;
                Some(json!({
                    "mount": mount, "device": device,
                    "total": usage.total, "used": usage.used, "available": usage.available,
                }))
            })
            .collect();
        json!({
            "hostname": line("proc/sys/kernel/hostname"),
            "os": os_name(&self.read("etc/os-release").unwrap_or_default()),
            "kernel": line("proc/sys/kernel/osrelease"),
            "cores": latest.cores.len().max(1),
            "uptime": uptime,
            "load": [load.first().copied().unwrap_or(0.0), load.get(1).copied().unwrap_or(0.0), load.get(2).copied().unwrap_or(0.0)],
            "cpu": share(previous.cpu, latest.cpu),
            "cpus": cpus,
            "memoryTotal": total,
            "memoryUsed": total.saturating_sub(available),
            "swapTotal": swap_total,
            "swapUsed": swap_total.saturating_sub(swap_free),
            "disks": disks,
            "network": network(&self.read("proc/net/dev").unwrap_or_default()),
            "processes": top,
            "processCount": latest.tasks.len(),
        })
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
        let mut samples = self.samples.lock().unwrap();
        samples.0 = samples.1.take();
        samples.1 = Some(sample);
        if let (Some(previous), Some(latest)) = &*samples {
            let mut history = self.history.lock().unwrap();
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
            std::thread::spawn(move || loop {
                let Some(current) = monitor.upgrade() else {
                    break;
                };
                current.tick();
                drop(current);
                std::thread::sleep(INTERVAL);
            });
        });
    }
    /// Waits briefly for a second sample so the first reply already has rates.
    fn snapshot(&self) -> Value {
        let samples = self.samples.lock().unwrap();
        let (samples, _) = self
            .ready
            .wait_timeout_while(samples, INTERVAL * 3, |s| s.0.is_none() || s.1.is_none())
            .unwrap();
        let mut snapshot = match &*samples {
            (Some(previous), Some(latest)) => self.source.snapshot(previous, latest),
            _ => return json!({"error": "The first readings are not ready yet."}),
        };
        drop(samples);
        let history = self.history.lock().unwrap();
        if let Some(cpus) = snapshot["cpus"].as_array_mut() {
            for core in cpus {
                let id = core["id"].as_u64().and_then(|id| u32::try_from(id).ok());
                if let Some(values) = id.and_then(|id| history.get(&id)) {
                    core["history"] = json!(values);
                }
            }
        }
        snapshot
    }
}

/// Answers `{"op":"snapshot"}` until stdin closes.
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
    snapshot: impl Fn() -> Value,
    services: &crate::services::Services,
    containers: &crate::containers::Containers,
    input: impl BufRead,
    mut output: impl Write,
    cancellation: &crate::transport::Cancellation,
) -> io::Result<()> {
    let mut input = input;
    let mut file_watches: Option<crate::file_watch::Watches> = None;
    loop {
        cancellation.check()?;
        let mut line = String::new();
        let read = (&mut input).take(MAX_REQUEST).read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        if !line.ends_with('\n') && read as u64 == MAX_REQUEST {
            return Err(io::Error::other("Monitor request too long"));
        }
        cancellation.check()?;
        let request: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
        let reply = match request.get("op").and_then(Value::as_str) {
            Some("snapshot") => snapshot(),
            Some("files_watch") => file_watches
                .get_or_insert_with(crate::file_watch::Watches::new)
                .handle(&request),
            // The desktop gives each feature its own channel. Non-metrics
            // channels never start the CPU/process sampling thread.
            Some(op) => services
                .handle(&request)
                .or_else(|| containers.handle(&request))
                .unwrap_or_else(|| json!({"error": format!("Unknown monitor request: {op}")})),
            None => json!({"error": "Monitor requests need an \"op\"."}),
        };
        serde_json::to_writer(&mut output, &reply)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
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
        let processes = snapshot["processes"].as_array().unwrap();
        assert_eq!(processes[0]["name"], "old");
        // Started one second after boot, read one hour later.
        assert_eq!(processes[0]["elapsed"], 3600);
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
        let processes = snapshot["processes"].as_array().unwrap();
        assert!(processes[0].get("elapsed").is_none());
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
        assert_eq!(snapshot["cpu"], 50.0);
        assert_eq!(snapshot["cores"], 2);
        assert_eq!(
            snapshot["cpus"],
            json!([{"id": 0, "usage": 100.0}, {"id": 2, "usage": 0.0}])
        );
        let processes = snapshot["processes"].as_array().unwrap();
        assert_eq!(processes[0]["name"], "busy");
        assert_eq!(processes[0]["cpu"], 80.0);
        // A pid with no baseline reports nothing yet, however many ticks it has.
        assert_eq!(processes[1]["name"], "new");
        assert_eq!(processes[1]["cpu"], 0.0);
        assert_eq!(processes[1]["memory"], 50 * 4096);
        assert_eq!(snapshot["processCount"], 2);
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
        let processes = snapshot["processes"].as_array().unwrap();
        // 250 ticks over 2 s at 100 Hz is 125%: more than one core, as htop shows.
        assert_eq!(processes[0]["pid"], 7);
        assert_eq!(processes[0]["cpu"], 125.0);
        assert_eq!(processes[1]["pid"], 60);
        assert_eq!(processes[2]["pid"], 59);
        // The top 50 by CPU (7, then 60..=12 by memory) plus the top 50 by
        // memory adds 11, which is large but idle; the smallest stay out.
        let pids: Vec<u64> = processes
            .iter()
            .map(|p| p["pid"].as_u64().unwrap())
            .collect();
        assert_eq!(pids.len(), 51);
        assert!(pids.contains(&11));
        assert!(!pids.contains(&10));
        assert_eq!(snapshot["processCount"], 60);
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
        assert!(long.len() <= MAX_COMMAND + '…'.len_utf8());
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
        write(root, "proc/meminfo", "MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 400 kB\nSwapTotal: 200 kB\nSwapFree: 150 kB\n");
        write(root, "proc/loadavg", "0.50 0.40 0.30 1/100 4242\n");
        write(root, "proc/uptime", "1234.5 99.0\n");
        write(
            root,
            "proc/self/mounts",
            "/dev/sda1 / ext4 rw 0 0\nproc /proc proc rw 0 0\ntmpfs /run tmpfs rw 0 0\n/dev/sdb1 /srv/my\\040data ext4 rw 0 0\n/dev/sdb1 /srv/my\\040data ext4 rw 0 0\n/dev/sdc1 /broken ext4 rw 0 0\n",
        );
        write(root, "proc/net/dev", "Inter-|   Receive\n face |bytes\n    lo: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n  eth0: 300 5 0 0 0 0 0 0 400 6 0 0 0 0 0 0\n");
        write(root, "proc/9/stat", &task(9, "sshd", 0, 3));
        write(
            root,
            "proc/9/status",
            "Name:\tsshd\nUid:\t1000\t1000\t1000\t1000\n",
        );
        let first = source.sample();
        let snapshot = source.snapshot(&first, &first);
        assert_eq!(snapshot["hostname"], "atlas-01");
        assert_eq!(snapshot["kernel"], "6.12.0");
        assert_eq!(snapshot["os"], "Debian GNU/Linux 13");
        assert_eq!(snapshot["memoryTotal"], 1000 * 1024);
        assert_eq!(snapshot["memoryUsed"], 600 * 1024);
        assert_eq!(snapshot["swapTotal"], 200 * 1024);
        assert_eq!(snapshot["swapUsed"], 50 * 1024);
        assert_eq!(snapshot["load"], json!([0.5, 0.4, 0.3]));
        assert_eq!(snapshot["uptime"], 1234.5);
        // /dev-backed or "/", deduplicated, unescaped; an unreadable one is skipped.
        assert_eq!(
            snapshot["disks"],
            json!([
                {"mount": "/", "device": "/dev/sda1", "total": 100, "used": 40, "available": 55},
                {"mount": "/srv/my data", "device": "/dev/sdb1", "total": 1000, "used": 900, "available": 100},
            ])
        );
        assert_eq!(
            snapshot["network"],
            json!([
                {"name": "lo", "received": 10, "sent": 20},
                {"name": "eth0", "received": 300, "sent": 400},
            ])
        );
        assert_eq!(snapshot["processes"][0]["user"], "ci");
        // Zero elapsed time never divides by zero.
        assert_eq!(snapshot["cpu"], 0.0);
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
        assert_eq!(source.snapshot(&sample, &sample)["memoryUsed"], 6 * 1024);
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
        let snapshot = monitor.snapshot();
        let busy = snapshot["cpus"][0]["history"].as_array().unwrap();
        assert_eq!(busy.len(), CORE_HISTORY, "capped at a minute");
        assert!(busy.iter().all(|v| v == 100.0));
        assert!(snapshot["cpus"][1]["history"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v == 0.0));
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

    #[test]
    fn nonmetrics_channel_does_not_start_process_sampling() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = Monitor::new(source(dir.path()));
        let mut output = Vec::new();
        handle_bound(
            || {
                monitor.start();
                monitor.snapshot()
            },
            &no_services(),
            &no_containers(),
            b"{\"op\":\"containers\"}\n".as_slice(),
            &mut output,
            &crate::transport::Cancellation::default(),
        )
        .unwrap();
        assert!(monitor.started.get().is_none());
        assert!(monitor.samples.lock().unwrap().1.is_none());
        assert_eq!(
            serde_json::from_slice::<Value>(&output).unwrap()["error"],
            crate::docker::NOT_RUNNING
        );
    }

    #[test]
    fn protocol_answers_until_stdin_closes() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "proc/stat", "cpu  1 0 0 1 0 0 0 0\n");
        let monitor = Monitor::new(source(dir.path()));
        monitor.tick();
        monitor.tick();
        let mut output = Vec::new();
        handle(
            &monitor,
            &no_services(),
            &no_containers(),
            "{\"op\":\"snapshot\"}\n{\"op\":\"reboot\"}\n{\"op\":\"containers\"}\nnot json\n"
                .as_bytes(),
            &mut output,
        )
        .unwrap();
        let lines: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0]["cpu"].is_number());
        assert_eq!(lines[1]["error"], "Unknown monitor request: reboot");
        // Containers answer on the same channel; here no engine is reachable.
        assert_eq!(lines[2]["error"], crate::docker::NOT_RUNNING);
        assert!(lines[3]["error"].is_string());
        let long = format!("{}\n", "x".repeat(MAX_REQUEST as usize + 10));
        assert!(handle(
            &monitor,
            &no_services(),
            &no_containers(),
            long.as_bytes(),
            Vec::new()
        )
        .is_err());
    }
}
