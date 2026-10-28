//! Containers for the Containers tab: inventory with live figures, inspect,
//! logs, lifecycle actions, removal, disk usage and prune, all through the
//! Docker Engine API (see docker.rs) as the SSH user -- never sudo, never the
//! `docker` CLI. Names, ids and projects are validated before they reach a
//! URL; requests are bounded in time and in how many run at once.
use crate::docker::{self, Engine, Line};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const MAX_CONTAINERS: usize = 500;
const MAX_IDS: usize = 500;
const CONCURRENCY: usize = 8;
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);
const STATS_TIMEOUT: Duration = Duration::from_secs(2);
const LOGS_TIMEOUT: Duration = Duration::from_secs(15);
const DF_TIMEOUT: Duration = Duration::from_secs(18);
const ACTION_TIMEOUT: Duration = Duration::from_secs(60);
/// The desktop's action wait is longer than this; a project's last containers are
/// reported as not attempted rather than outliving it.
const PROJECT_DEADLINE: Duration = Duration::from_secs(70);
const PRUNE_TIMEOUT: Duration = Duration::from_secs(110);
/// Whole-request budgets, under the desktop's waits for reads and actions,
/// however many containers there are or however slow the engine.
const SNAPSHOT_BUDGET: Duration = Duration::from_secs(15);
const LOGS_BUDGET: Duration = Duration::from_secs(16);
const OUT_OF_TIME: &str = "Docker didn't answer in time.";
const ENGINE_TTL: Duration = Duration::from_secs(60);
const DEFAULT_LINES: u64 = 200;
const MAX_LINES: u64 = 2000;
const MAX_PROJECT_CONTAINERS: usize = 20;
/// Serialized messages per log page, under the desktop's reply cap.
const LOG_BUDGET: usize = 3 << 20;
const MAX_CURSOR: usize = 4096;
const MAX_DISK_ROWS: usize = 2000;
const CHANGED: &str =
    "Project containers changed since you opened it. Refresh and review the project again.";

pub fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A container id (12 to 64 hex digits) or a container name.
pub fn valid_container(target: &str) -> bool {
    let hex = (12..=64).contains(&target.len()) && target.bytes().all(|b| b.is_ascii_hexdigit());
    let name = !target.is_empty()
        && target.len() <= 128
        && target.as_bytes()[0].is_ascii_alphanumeric()
        && target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b));
    hex || name
}

/// Compose's project-name rules.
pub fn valid_project(project: &str) -> bool {
    !project.is_empty()
        && project.len() <= 256
        && project.as_bytes()[0].is_ascii_alphanumeric()
        && project
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}

fn container_arg(request: &Value) -> Result<&str, String> {
    match request["container"].as_str() {
        Some(c) if valid_container(c) => Ok(c),
        Some(c) => Err(format!("Not a valid container: {c}")),
        None => Err("Choose a container.".into()),
    }
}
fn project_arg(value: &Value) -> Result<&str, String> {
    match value.as_str() {
        Some(p) if valid_project(p) => Ok(p),
        Some(p) => Err(format!("Not a valid Compose project: {p}")),
        None => Err("Choose a Compose project.".into()),
    }
}

/// Time left before `deadline`, at most `cap`; None once it has run out.
fn within(deadline: Instant, cap: Duration) -> Option<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    (left >= Duration::from_millis(100)).then(|| left.min(cap))
}

/// Drops rows without a full container id; a malformed engine reply must not
/// reach code that shortens ids.
fn keep_identified(rows: &mut Vec<Value>) {
    rows.retain(|row| valid_id(row["Id"].as_str().unwrap_or_default()));
}

/// Runs `work` over `items`, at most `limit` at a time, keeping order.
fn bounded<T: Sync, R: Send>(items: &[T], limit: usize, work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..limit.min(items.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    *results[i].lock().unwrap() = Some(work(item));
                }
            });
        }
    });
    results
        .into_iter()
        .map(|r| r.into_inner().unwrap().expect("every item ran"))
        .collect()
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The engine's container state, in the contract's words. Podman's own
/// states map onto Docker's.
fn state(raw: &str) -> &'static str {
    match raw.to_ascii_lowercase().as_str() {
        "created" | "configured" | "initialized" => "created",
        "running" | "stopping" => "running",
        "paused" => "paused",
        "restarting" => "restarting",
        "removing" => "removing",
        "exited" | "stopped" => "exited",
        _ => "dead",
    }
}

fn health(raw: Option<&str>) -> Option<&'static str> {
    match raw? {
        "healthy" => Some("healthy"),
        "unhealthy" => Some("unhealthy"),
        "starting" => Some("starting"),
        _ => None,
    }
}

/// Health as the list's status text shows it, for a container whose
/// inspect is unavailable.
fn status_health(status: &str) -> Option<&'static str> {
    if status.contains("(healthy)") {
        Some("healthy")
    } else if status.contains("(unhealthy)") {
        Some("unhealthy")
    } else if status.contains("(health: starting)") {
        Some("starting")
    } else {
        None
    }
}

fn restart_policy(host_config: &Value) -> Option<String> {
    match host_config["RestartPolicy"]["Name"].as_str()? {
        "" => Some("no".into()),
        name => Some(name.to_owned()),
    }
}

/// What a snapshot needs from inspect, cached while the container's listed
/// state and status stay the same.
#[derive(Clone, Debug, Default, PartialEq)]
struct Summary {
    image: Option<String>,
    health: Option<&'static str>,
    exit_code: Option<i64>,
    oom_killed: bool,
    error: Option<String>,
    restart_count: u64,
    restart_policy: Option<String>,
    started_at: Option<i64>,
    finished_at: Option<i64>,
}
fn summary(inspect: &Value) -> Summary {
    let s = &inspect["State"];
    Summary {
        image: text(&inspect["Config"]["Image"]),
        health: health(s["Health"]["Status"].as_str()),
        exit_code: s["ExitCode"].as_i64(),
        oom_killed: s["OOMKilled"].as_bool().unwrap_or(false),
        error: text(&s["Error"]),
        restart_count: inspect["RestartCount"].as_u64().unwrap_or(0),
        restart_policy: restart_policy(&inspect["HostConfig"]),
        started_at: docker::time_ms(s["StartedAt"].as_str()),
        finished_at: docker::time_ms(s["FinishedAt"].as_str()),
    }
}

/// Published and exposed ports, without the IPv4/IPv6 wildcard twins.
pub fn ports(list: &Value) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut rows: Vec<(Option<String>, Option<u64>, u64, String)> = Vec::new();
    for port in list.as_array().into_iter().flatten() {
        let Some(container) = port["PrivatePort"].as_u64() else {
            continue;
        };
        let protocol = match port["Type"].as_str().unwrap_or("tcp") {
            p @ ("udp" | "sctp") => p.to_owned(),
            _ => "tcp".to_owned(),
        };
        let host = port["PublicPort"].as_u64().filter(|p| *p > 0);
        let ip = text(&port["IP"]).filter(|_| host.is_some());
        let wildcard = matches!(ip.as_deref(), Some("0.0.0.0" | "::"));
        let key = (
            if wildcard {
                Some("*".into())
            } else {
                ip.clone()
            },
            host,
            container,
            protocol.clone(),
        );
        if seen.insert(key) {
            rows.push((ip, host, container, protocol));
        }
    }
    // An exposed port that is also published needs no second, bare row.
    let published: HashSet<(u64, String)> = rows
        .iter()
        .filter(|r| r.1.is_some())
        .map(|r| (r.2, r.3.clone()))
        .collect();
    rows.retain(|r| r.1.is_some() || !published.contains(&(r.2, r.3.clone())));
    rows.sort_by(|a, b| (a.2, &a.3, a.1, &a.0).cmp(&(b.2, &b.3, b.1, &b.0)));
    rows.into_iter()
        .map(|(ip, host, container, protocol)| {
            json!({"ip": ip, "hostPort": host, "containerPort": container, "protocol": protocol})
        })
        .collect()
}

fn compose(labels: &Value) -> Value {
    let Some(project) = text(&labels["com.docker.compose.project"]) else {
        return Value::Null;
    };
    json!({
        "project": project,
        "service": labels["com.docker.compose.service"].as_str().unwrap_or_default(),
        "oneoff": labels["com.docker.compose.oneoff"].as_str().is_some_and(|v| v.eq_ignore_ascii_case("true")),
        "number": labels["com.docker.compose.container-number"].as_str().and_then(|n| n.parse::<u64>().ok()),
        "workingDir": text(&labels["com.docker.compose.project.working_dir"]),
        "configFiles": text(&labels["com.docker.compose.project.config_files"]),
    })
}

fn listed_name(row: &Value) -> String {
    let names: Vec<&str> = row["Names"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|n| n.trim_start_matches('/'))
        .collect();
    // Legacy links add "other/alias" names; the container's own has no slash.
    names
        .iter()
        .find(|n| !n.contains('/'))
        .or(names.first())
        .map(|n| (*n).to_owned())
        .unwrap_or_default()
}

/// Percent of one core over the interval.
pub fn cpu_percent(before: u64, after: u64, elapsed: Duration) -> Option<f64> {
    let elapsed = elapsed.as_nanos() as f64;
    (after >= before && elapsed > 0.0)
        .then(|| ((after - before) as f64 / elapsed * 1000.0).round() / 10.0)
}

/// Memory as `docker stats` shows it: usage less the inactive page cache
/// (cgroup v1 `total_inactive_file`, v2 `inactive_file`).
pub fn memory_usage(stats: &Value) -> Option<u64> {
    let memory = &stats["memory_stats"];
    let usage = memory["usage"].as_u64()?;
    let detail = &memory["stats"];
    if let Some(v1) = detail["total_inactive_file"].as_u64() {
        if v1 < usage {
            return Some(usage - v1);
        }
    }
    match detail["inactive_file"].as_u64() {
        Some(v2) if v2 < usage => Some(usage - v2),
        _ => Some(usage),
    }
}

/// The container's memory limit, or None when it may use all of the host's.
pub fn memory_limit(stats: &Value, host_total: u64) -> Option<u64> {
    stats["memory_stats"]["limit"]
        .as_u64()
        .filter(|l| *l > 0 && *l < 1 << 62 && (host_total == 0 || *l < host_total))
}

fn cpu_total(stats: &Value) -> Option<u64> {
    stats["cpu_stats"]["cpu_usage"]["total_usage"].as_u64()
}

struct Figures {
    cpu: Option<f64>,
    memory: Option<u64>,
    limit: Option<u64>,
}

fn container_json(row: &Value, summary: Option<&Summary>, figures: Option<&Figures>) -> Value {
    let raw_state = row["State"].as_str().unwrap_or_default();
    let state = state(raw_state);
    let status = row["Status"].as_str().unwrap_or_default();
    let stopped = matches!(state, "exited" | "dead" | "restarting" | "removing");
    let exit_code = match summary {
        Some(s) => s.exit_code,
        // "Exited (137) 2 minutes ago"
        None => status
            .split_once('(')
            .and_then(|(_, rest)| rest.split_once(')'))
            .and_then(|(code, _)| code.parse().ok()),
    }
    .filter(|_| stopped);
    let running = matches!(state, "running" | "paused" | "restarting");
    json!({
        "id": row["Id"].as_str().unwrap_or_default(),
        "name": listed_name(row),
        "image": summary.and_then(|s| s.image.clone()).or_else(|| text(&row["Image"])).unwrap_or_default(),
        "imageId": row["ImageID"].as_str().unwrap_or_default(),
        "command": row["Command"].as_str().unwrap_or_default(),
        "state": state,
        "status": status,
        "health": summary.map_or_else(|| status_health(status), |s| s.health),
        "exitCode": exit_code,
        "oomKilled": summary.is_some_and(|s| s.oom_killed),
        "error": summary.and_then(|s| s.error.clone()),
        "restartCount": summary.map_or(0, |s| s.restart_count),
        "restartPolicy": summary.and_then(|s| s.restart_policy.clone()),
        "created": row["Created"].as_i64().unwrap_or(0) * 1000,
        "startedAt": summary.and_then(|s| s.started_at),
        "finishedAt": summary.and_then(|s| s.finished_at).filter(|_| !running || state == "restarting"),
        "ports": ports(&row["Ports"]),
        "compose": compose(&row["Labels"]),
        "cpu": figures.and_then(|f| f.cpu),
        "memory": figures.and_then(|f| f.memory),
        "memoryLimit": figures.and_then(|f| f.limit),
    })
}

fn split_env(entry: &str) -> Value {
    let (name, value) = entry.split_once('=').unwrap_or((entry, ""));
    json!({"name": name, "value": value})
}
fn strings(value: &Value) -> Option<Vec<String>> {
    value.as_array().map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}

/// Inspect, in the contract's shape.
pub fn inspect_json(inspect: &Value) -> Value {
    let config = &inspect["Config"];
    let mounts: Vec<Value> = inspect["Mounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            json!({
                "type": m["Type"].as_str().unwrap_or("bind"),
                "source": text(&m["Source"]),
                "destination": m["Destination"].as_str().unwrap_or_default(),
                "mode": text(&m["Mode"]),
                "rw": m["RW"].as_bool().unwrap_or(false),
                "name": text(&m["Name"]),
            })
        })
        .collect();
    let networks: Vec<Value> = inspect["NetworkSettings"]["Networks"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, n)| {
            json!({
                "name": name,
                "ipAddress": text(&n["IPAddress"]),
                "aliases": strings(&n["Aliases"]).unwrap_or_default(),
            })
        })
        .collect();
    let labels: BTreeMap<&str, &str> = config["Labels"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.as_str(), v.as_str()?)))
        .collect();
    let check = &config["Healthcheck"];
    let healthcheck = match strings(&check["Test"]) {
        Some(test) if !test.is_empty() && test != ["NONE"] => json!({
            "test": test,
            "interval": check["Interval"].as_u64().filter(|ns| *ns > 0).map(|ns| ns / 1_000_000),
        }),
        _ => Value::Null,
    };
    let log = inspect["State"]["Health"]["Log"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let last_health: Vec<Value> = log[log.len().saturating_sub(5)..]
        .iter()
        .filter_map(|entry| {
            Some(json!({
                "at": docker::time_ms(entry["Start"].as_str()).or_else(|| docker::time_ms(entry["End"].as_str()))?,
                "exitCode": entry["ExitCode"].as_i64().unwrap_or(-1),
                "output": entry["Output"].as_str().unwrap_or_default(),
            }))
        })
        .collect();
    json!({
        "id": inspect["Id"].as_str().unwrap_or_default(),
        "name": inspect["Name"].as_str().unwrap_or_default().trim_start_matches('/'),
        "image": config["Image"].as_str().unwrap_or_default(),
        "created": docker::time_ms(inspect["Created"].as_str()).unwrap_or(0),
        "entrypoint": strings(&config["Entrypoint"]),
        "cmd": strings(&config["Cmd"]),
        "workingDir": text(&config["WorkingDir"]),
        "user": text(&config["User"]),
        "hostname": text(&config["Hostname"]),
        "env": config["Env"].as_array().into_iter().flatten().filter_map(Value::as_str).map(split_env).collect::<Vec<_>>(),
        "mounts": mounts,
        "networks": networks,
        "restartPolicy": restart_policy(&inspect["HostConfig"]),
        "labels": labels,
        "healthcheck": healthcheck,
        "lastHealth": last_health,
    })
}

/// Where a log reader left off: the last timestamp delivered and how many
/// lines carried exactly that timestamp (the engine's `since` is inclusive).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub at: i64,
    pub seen: usize,
}

#[derive(Debug, PartialEq)]
pub enum Cursor {
    Container(Mark),
    Project(BTreeMap<String, Mark>),
}
impl Cursor {
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() > MAX_CURSOR {
            return None;
        }
        let mark = |at: &str, seen: &str| -> Option<Mark> {
            let at = at.parse::<i64>().ok().filter(|a| *a >= 0)?;
            Some(Mark {
                at,
                seen: seen.parse().ok()?,
            })
        };
        if let Some(rest) = text.strip_prefix("c1.") {
            let (at, seen) = rest.split_once('.')?;
            return Some(Self::Container(mark(at, seen)?));
        }
        let rest = text.strip_prefix("p1.")?;
        let mut marks = BTreeMap::new();
        for part in rest.split(',').filter(|p| !p.is_empty()) {
            let mut fields = part.split('-');
            let (id, at, seen) = (fields.next()?, fields.next()?, fields.next()?);
            if fields.next().is_some()
                || id.len() != 12
                || !id.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return None;
            }
            marks.insert(id.to_owned(), mark(at, seen)?);
        }
        (marks.len() <= MAX_IDS).then_some(Self::Project(marks))
    }
    pub fn encode(&self) -> String {
        match self {
            Self::Container(m) => format!("c1.{}.{}", m.at, m.seen),
            Self::Project(marks) => {
                let parts: Vec<String> = marks
                    .iter()
                    .map(|(id, m)| format!("{id}-{}-{}", m.at, m.seen))
                    .collect();
                format!("p1.{}", parts.join(","))
            }
        }
    }
}

/// Lines strictly after `mark`: older ones and the first `seen` at the mark
/// itself were delivered before. Undated lines cannot be placed, so stay.
pub fn after(lines: Vec<Line>, mark: Option<Mark>) -> Vec<Line> {
    let Some(mark) = mark else { return lines };
    let mut skipped = 0;
    lines
        .into_iter()
        .filter(|line| match line.at {
            None => true,
            Some(at) if at < mark.at => false,
            Some(at) if at == mark.at && skipped < mark.seen => {
                skipped += 1;
                false
            }
            Some(_) => true,
        })
        .collect()
}

/// The mark after reading `lines` (everything the engine returned since the
/// old mark, including lines at it), or the old mark when nothing is dated.
pub fn advance(lines: &[Line], old: Option<Mark>) -> Option<Mark> {
    let last = lines.iter().filter_map(|l| l.at).max();
    match last {
        Some(at) if old.is_none_or(|o| at >= o.at) => Some(Mark {
            at,
            seen: lines.iter().filter(|l| l.at == Some(at)).count(),
        }),
        _ => old,
    }
}

fn since(mark: Mark) -> String {
    format!(
        "{}.{:09}",
        mark.at.div_euclid(1_000_000_000),
        mark.at.rem_euclid(1_000_000_000)
    )
}

/// Keeps the newest entries whose serialized size fits the page budget.
fn within_budget(entries: &mut Vec<Value>) {
    let mut used = 0;
    let mut keep = entries.len();
    for (i, entry) in entries.iter().enumerate().rev() {
        used += entry.to_string().len() + 1;
        if used > LOG_BUDGET {
            break;
        }
        keep = i;
    }
    entries.drain(..keep);
}

fn entry(line: &Line, container: Option<&str>) -> Value {
    json!({
        "at": line.at.map(|ns| ns.div_euclid(1_000_000)),
        "stream": if line.stderr { "stderr" } else { "stdout" },
        "message": line.message,
        "container": container,
    })
}

/// Sizes and counts for one kind of disk usage.
fn total(count: usize, size: u64, reclaimable: u64) -> Value {
    json!({"count": count, "size": size, "reclaimable": reclaimable.min(size)})
}

fn size(value: &Value) -> Option<u64> {
    value.as_i64().and_then(|n| u64::try_from(n).ok())
}

/// Volumes as `/system/df` lists them, but without sizes: names, drivers and
/// labels from `GET /volumes`, references counted from container mounts.
fn unmeasured_volumes(listed: &Value, containers: &Value) -> Value {
    let mut references: HashMap<&str, i64> = HashMap::new();
    for container in containers.as_array().into_iter().flatten() {
        for mount in container["Mounts"].as_array().into_iter().flatten() {
            if mount["Type"] == "volume" {
                if let Some(name) = mount["Name"].as_str() {
                    *references.entry(name).or_default() += 1;
                }
            }
        }
    }
    Value::Array(
        listed["Volumes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|volume| {
                let name = volume["Name"].as_str().unwrap_or_default();
                json!({
                    "Name": name,
                    "Driver": volume["Driver"],
                    "Labels": volume["Labels"],
                    "UsageData": {
                        "Size": -1,
                        "RefCount": references.get(name).copied().unwrap_or(0),
                    },
                })
            })
            .collect(),
    )
}

/// `GET /system/df`, in the contract's shape.
pub fn disk_json(df: &Value) -> Value {
    let images_raw = df["Images"].as_array().cloned().unwrap_or_default();
    let mut images: Vec<Value> = images_raw
        .iter()
        .map(|image| {
            let tags: Vec<&str> = image["RepoTags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|t| *t != "<none>:<none>")
                .collect();
            json!({
                "id": image["Id"].as_str().unwrap_or_default(),
                "dangling": tags.is_empty(),
                "tags": tags,
                "size": size(&image["Size"]).unwrap_or(0),
                "sharedSize": size(&image["SharedSize"]),
                "created": image["Created"].as_i64().unwrap_or(0) * 1000,
                "containers": size(&image["Containers"]).unwrap_or(0),
            })
        })
        .collect();
    let image_sum: u64 = images.iter().filter_map(|i| i["size"].as_u64()).sum();
    let image_size = size(&df["LayersSize"])
        .filter(|s| *s > 0)
        .unwrap_or(image_sum);
    let image_unused: u64 = images
        .iter()
        .filter(|i| i["containers"] == 0)
        .filter_map(|i| i["size"].as_u64())
        .sum();

    let containers = df["Containers"].as_array().cloned().unwrap_or_default();
    let container_size: u64 = containers.iter().filter_map(|c| size(&c["SizeRw"])).sum();
    let container_stopped: u64 = containers
        .iter()
        .filter(|c| {
            !matches!(
                state(c["State"].as_str().unwrap_or_default()),
                "running" | "paused" | "restarting"
            )
        })
        .filter_map(|c| size(&c["SizeRw"]))
        .sum();

    let mut volumes: Vec<Value> = df["Volumes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|volume| {
            let name = volume["Name"].as_str().unwrap_or_default();
            let labels = &volume["Labels"];
            json!({
                "name": name,
                "driver": volume["Driver"].as_str().unwrap_or("local"),
                "size": size(&volume["UsageData"]["Size"]),
                "refCount": size(&volume["UsageData"]["RefCount"]),
                "anonymous": valid_id(name) && labels["com.docker.compose.volume"].is_null(),
                "compose": text(&labels["com.docker.compose.project"]),
            })
        })
        .collect();
    let volume_size: u64 = volumes.iter().filter_map(|v| v["size"].as_u64()).sum();
    let volume_unused: u64 = volumes
        .iter()
        .filter(|v| v["refCount"] == 0)
        .filter_map(|v| v["size"].as_u64())
        .sum();

    let cache = df["BuildCache"].as_array().cloned().unwrap_or_default();
    let cache_size: u64 = cache.iter().filter_map(|c| size(&c["Size"])).sum();
    let cache_idle: u64 = cache
        .iter()
        .filter(|c| !c["InUse"].as_bool().unwrap_or(false))
        .filter_map(|c| size(&c["Size"]))
        .sum();

    let totals = json!({
        "images": total(images.len(), image_size, image_unused),
        "containers": total(containers.len(), container_size, container_stopped),
        "volumes": total(volumes.len(), volume_size, volume_unused),
        "buildCache": total(cache.len(), cache_size, cache_idle),
    });
    images.sort_by_key(|i| std::cmp::Reverse(i["size"].as_u64()));
    images.truncate(MAX_DISK_ROWS);
    volumes.sort_by_key(|v| std::cmp::Reverse(v["size"].as_u64()));
    volumes.truncate(MAX_DISK_ROWS);
    json!({"images": images, "volumes": volumes, "totals": totals})
}

/// What a prune reply removed: deleted ids (images count deletions, not
/// untags) and the bytes freed.
pub fn prune_json(reply: &Value) -> Value {
    let list = |key: &str| reply[key].as_array().map_or(0, Vec::len);
    let removed = list("ContainersDeleted")
        + list("VolumesDeleted")
        + list("CachesDeleted")
        + reply["ImagesDeleted"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["Deleted"].is_string())
            .count();
    json!({"removed": removed, "reclaimed": size(&reply["SpaceReclaimed"]).unwrap_or(0)})
}

/// A running container's published TCP ports for Port discovery, or None
/// when it publishes none. Compose labels name its project and directory.
pub fn published(row: &Value) -> Option<Value> {
    let name = row["Names"][0].as_str()?.trim_start_matches('/');
    let mut bindings: Vec<Value> = row["Ports"]
        .as_array()?
        .iter()
        .filter(|p| p["Type"] == "tcp")
        .filter_map(|p| {
            let port = p["PublicPort"]
                .as_u64()
                .filter(|p| (1..=65535).contains(p))?;
            let address = p["IP"]
                .as_str()
                .filter(|ip| !ip.is_empty())
                .unwrap_or("0.0.0.0");
            Some(json!({"address": address, "port": port}))
        })
        .collect();
    bindings.dedup();
    if bindings.is_empty() {
        return None;
    }
    // Docker reports a missing label as absent, or as an empty string.
    let label = |key: &str| row["Labels"][key].as_str().filter(|v| !v.is_empty());
    Some(json!({
        "name": name,
        "image": row["Image"].as_str().filter(|v| !v.is_empty()),
        "project": label("com.docker.compose.project"),
        "service": label("com.docker.compose.service"),
        "workdir": label("com.docker.compose.project.working_dir"),
        "bindings": bindings,
    }))
}

pub struct Containers {
    sockets: Box<dyn Fn() -> Vec<PathBuf> + Send + Sync>,
    engine: Mutex<Option<(Engine, Instant)>>,
    /// Whether Podman is installed, for when no engine answers.
    podman: fn() -> bool,
    inspected: Mutex<HashMap<String, (String, Summary)>>,
    cpu: Mutex<HashMap<String, (u64, Instant)>>,
    /// Compose actions (see compose.rs), which find projects through here.
    compose: crate::compose::Compose,
}

impl Containers {
    pub fn system() -> Self {
        let mut containers = Self::new(Box::new(|| {
            // SAFETY: geteuid has no preconditions and cannot fail.
            let uid = unsafe { libc::geteuid() };
            docker::candidates(
                std::env::var("DOCKER_HOST").ok().as_deref(),
                std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
                uid,
            )
        }));
        containers.podman = || {
            let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
                .map(|path| std::env::split_paths(&path).collect())
                .unwrap_or_default();
            dirs.push("/usr/bin".into());
            crate::compose::which("podman", &dirs).is_some()
        };
        containers
    }
    pub fn new(sockets: Box<dyn Fn() -> Vec<PathBuf> + Send + Sync>) -> Self {
        Self {
            sockets,
            podman: || false,
            engine: Mutex::new(None),
            inspected: Mutex::new(HashMap::new()),
            cpu: Mutex::new(HashMap::new()),
            compose: crate::compose::Compose::system(),
        }
    }

    /// A project's containers, newest first, and the engine they are on.
    fn compose_rows(
        &self,
        project: &str,
        deadline: Instant,
    ) -> Result<crate::compose::Found, String> {
        self.with_engine(|engine| {
            Ok(crate::compose::Found {
                socket: Some(engine.socket.clone()),
                podman: engine.podman,
                rows: Self::list(engine, Some(project), deadline)?,
            })
        })
    }

    /// Lets running Compose commands finish before the agent exits.
    pub fn wait_for_compose(&self, limit: Duration) {
        self.compose.wait_idle(limit);
    }

    fn engine(&self) -> Result<Engine, String> {
        let mut cached = self.engine.lock().unwrap();
        if let Some((engine, at)) = &*cached {
            if at.elapsed() < ENGINE_TTL && engine.socket.exists() {
                return Ok(engine.clone());
            }
        }
        *cached = None;
        let engine = Engine::discover(&(self.sockets)())
            .map_err(|error| docker::explain_missing(error, (self.podman)()))?;
        *cached = Some((engine.clone(), Instant::now()));
        Ok(engine)
    }
    /// A failed request may mean the engine went away; look again next time.
    fn forget_engine(&self) {
        *self.engine.lock().unwrap() = None;
    }
    fn with_engine<T>(&self, work: impl FnOnce(&Engine) -> Result<T, String>) -> Result<T, String> {
        let engine = self.engine()?;
        let result = work(&engine);
        if result
            .as_ref()
            .is_err_and(|e| e.starts_with("Docker") || e.starts_with("This account"))
        {
            self.forget_engine();
        }
        result
    }

    fn list(
        engine: &Engine,
        project: Option<&str>,
        deadline: Instant,
    ) -> Result<Vec<Value>, String> {
        let mut path = "/containers/json?all=1".to_owned();
        if let Some(project) = project {
            let filters = json!({"label": [format!("com.docker.compose.project={project}")]});
            path += &format!("&filters={}", docker::encode(&filters.to_string()));
        }
        let listed = engine.get(&path, within(deadline, LIST_TIMEOUT).ok_or(OUT_OF_TIME)?)?;
        let mut rows: Vec<Value> = listed
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row["Id"].as_str().is_some_and(valid_id))
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row["Created"].as_i64().unwrap_or(0)));
        Ok(rows)
    }

    /// Published ports of running containers. No Docker, or no access to
    /// it, is not an error for Port discovery: listeners still show.
    pub fn published_ports(&self) -> Vec<Value> {
        let deadline = Instant::now() + LIST_TIMEOUT;
        self.with_engine(|engine| {
            let timeout = within(deadline, LIST_TIMEOUT).ok_or(OUT_OF_TIME)?;
            engine.get("/containers/json", timeout)
        })
        .ok()
        .and_then(|rows| rows.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .take(MAX_CONTAINERS)
        .filter_map(published)
        .collect()
    }

    pub fn snapshot(&self) -> Result<Value, String> {
        let deadline = Instant::now() + SNAPSHOT_BUDGET;
        self.with_engine(|engine| {
            let mut rows = Self::list(engine, None, deadline)?;
            let truncated = rows.len() > MAX_CONTAINERS;
            rows.truncate(MAX_CONTAINERS);
            let id = |row: &Value| row["Id"].as_str().unwrap_or_default().to_owned();
            let key = |row: &Value| {
                format!(
                    "{}\n{}",
                    row["State"].as_str().unwrap_or_default(),
                    row["Status"].as_str().unwrap_or_default()
                )
            };

            let stale: Vec<(String, String)> = {
                let cache = self.inspected.lock().unwrap();
                rows.iter()
                    .filter(|row| cache.get(&id(row)).is_none_or(|(k, _)| *k != key(row)))
                    .map(|row| (id(row), key(row)))
                    .collect()
            };
            let fresh = bounded(&stale, CONCURRENCY, |(id, _)| {
                let timeout = within(deadline, INSPECT_TIMEOUT)?;
                engine
                    .get(&format!("/containers/{id}/json"), timeout)
                    .ok()
                    .map(|inspect| summary(&inspect))
            });
            let summaries: HashMap<String, Summary> = {
                let mut cache = self.inspected.lock().unwrap();
                for ((id, key), summary) in stale.into_iter().zip(fresh) {
                    match summary {
                        Some(summary) => {
                            cache.insert(id, (key, summary));
                        }
                        None => {
                            cache.remove(&id);
                        }
                    }
                }
                let listed: HashSet<String> = rows.iter().map(id).collect();
                cache.retain(|id, _| listed.contains(id));
                cache
                    .iter()
                    .map(|(id, (_, s))| (id.clone(), s.clone()))
                    .collect()
            };

            let running: Vec<String> = rows
                .iter()
                .filter(|row| state(row["State"].as_str().unwrap_or_default()) == "running")
                .map(id)
                .collect();
            let stats = bounded(&running, CONCURRENCY, |id| {
                let timeout = within(deadline, STATS_TIMEOUT)?;
                engine
                    .get(
                        &format!("/containers/{id}/stats?stream=false&one-shot=true"),
                        timeout,
                    )
                    .ok()
                    .map(|stats| (stats, Instant::now()))
            });
            let mut figures = HashMap::new();
            let mut cpu = self.cpu.lock().unwrap();
            let mut next = HashMap::new();
            for (id, reading) in running.into_iter().zip(stats) {
                let Some((stats, at)) = reading else { continue };
                let total = cpu_total(&stats);
                let share = match (total, cpu.get(&id)) {
                    (Some(now), Some((before, then))) => {
                        cpu_percent(*before, now, at.duration_since(*then))
                    }
                    _ => None,
                };
                if let Some(total) = total {
                    next.insert(id.clone(), (total, at));
                }
                figures.insert(
                    id,
                    Figures {
                        cpu: share,
                        memory: memory_usage(&stats),
                        limit: memory_limit(&stats, engine.mem_total),
                    },
                );
            }
            // Containers that stopped drop their baseline.
            *cpu = next;
            drop(cpu);

            let containers: Vec<Value> = rows
                .iter()
                .map(|row| {
                    let id = id(row);
                    container_json(row, summaries.get(&id), figures.get(&id))
                })
                .collect();
            Ok(json!({
                "engine": {
                    "version": engine.version,
                    "apiVersion": engine.api_version,
                    "os": engine.os,
                    "rootless": engine.rootless,
                    "podman": engine.podman,
                },
                "containers": containers,
                "truncated": truncated,
            }))
        })
    }

    pub fn inspect(&self, request: &Value) -> Result<Value, String> {
        let container = container_arg(request)?;
        self.with_engine(|engine| {
            let inspect = engine.get(&format!("/containers/{container}/json"), INSPECT_TIMEOUT)?;
            Ok(inspect_json(&inspect))
        })
    }

    fn read_logs(
        engine: &Engine,
        container: &str,
        mark: Option<Mark>,
        lines: u64,
        deadline: Instant,
    ) -> Result<Vec<Line>, String> {
        let mut path = format!("/containers/{container}/logs?stdout=1&stderr=1&timestamps=1");
        match mark {
            Some(mark) => path += &format!("&since={}&tail={MAX_LINES}", since(mark)),
            None => path += &format!("&tail={lines}"),
        }
        let timeout = within(deadline, LOGS_TIMEOUT).ok_or(OUT_OF_TIME)?;
        let response = engine.call("GET", &path, timeout)?;
        if !response.success() {
            return Err(response.message());
        }
        let frames = docker::multiplexed(response.header("content-type"), &response.body);
        Ok(docker::demux(&response.body, frames))
    }

    pub fn logs(&self, request: &Value) -> Result<Value, String> {
        let container = request["container"].as_str();
        let project = request["project"].as_str();
        let cursor_text = request["cursor"].as_str();
        let cursor = cursor_text
            .map(|c| Cursor::parse(c).ok_or("Invalid log cursor."))
            .transpose()?;
        let lines = request["lines"]
            .as_u64()
            .unwrap_or(DEFAULT_LINES)
            .clamp(1, MAX_LINES);
        match (container, project) {
            (Some(_), None) => {
                let container = container_arg(request)?;
                let mark = match cursor {
                    None => None,
                    Some(Cursor::Container(mark)) => Some(mark),
                    Some(Cursor::Project(_)) => return Err("Invalid log cursor.".into()),
                };
                self.with_engine(|engine| {
                    let deadline = Instant::now() + LOGS_BUDGET;
                    let read = Self::read_logs(engine, container, mark, lines, deadline)?;
                    let next = advance(&read, mark);
                    let fresh = after(read, mark);
                    let mut entries: Vec<Value> = fresh.iter().map(|l| entry(l, None)).collect();
                    within_budget(&mut entries);
                    let cursor = match (entries.is_empty(), cursor_text) {
                        (true, Some(old)) => Some(old.to_owned()),
                        _ => next.map(|m| Cursor::Container(m).encode()),
                    };
                    Ok(json!({"entries": entries, "cursor": cursor}))
                })
            }
            (None, Some(_)) => {
                let project = project_arg(&request["project"])?;
                let marks = match cursor {
                    None => BTreeMap::new(),
                    Some(Cursor::Project(marks)) => marks,
                    Some(Cursor::Container(_)) => return Err("Invalid log cursor.".into()),
                };
                self.with_engine(|engine| {
                    self.project_logs(engine, project, marks, cursor_text, lines)
                })
            }
            _ => Err("Logs need either a container or a project, not both.".into()),
        }
    }

    fn project_logs(
        &self,
        engine: &Engine,
        project: &str,
        marks: BTreeMap<String, Mark>,
        cursor_text: Option<&str>,
        lines: u64,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + LOGS_BUDGET;
        let mut rows = Self::list(engine, Some(project), deadline)?;
        keep_identified(&mut rows);
        // Running first, then newest.
        rows.sort_by_key(|row| state(row["State"].as_str().unwrap_or_default()) != "running");
        rows.truncate(MAX_PROJECT_CONTAINERS);
        let service = |row: &Value| text(&row["Labels"]["com.docker.compose.service"]);
        let mut counts: HashMap<String, usize> = HashMap::new();
        for row in &rows {
            if let Some(s) = service(row) {
                *counts.entry(s).or_default() += 1;
            }
        }
        // A scaled service's replicas are told apart by container name.
        let labels: Vec<String> = rows
            .iter()
            .map(|row| match service(row) {
                Some(s) if counts[&s] == 1 => s,
                _ => listed_name(row),
            })
            .collect();
        let ids: Vec<String> = rows
            .iter()
            .map(|row| row["Id"].as_str().unwrap_or_default().to_owned())
            .collect();
        let reads = bounded(&ids, CONCURRENCY, |id| {
            let mark = marks.get(&id[..12]).copied();
            Self::read_logs(engine, id, mark, lines, deadline).map(|read| (read, mark))
        });
        let mut merged: Vec<(Line, usize)> = Vec::new();
        let mut next = BTreeMap::new();
        for (index, (id, read)) in ids.iter().zip(reads).enumerate() {
            let short = id[..12].to_owned();
            let (read, mark) = match read {
                Ok(read) => read,
                // A container removed or slow mid-read keeps its place; others go on.
                Err(_) => {
                    if let Some(mark) = marks.get(&short) {
                        next.insert(short, *mark);
                    }
                    continue;
                }
            };
            if let Some(mark) = advance(&read, mark) {
                next.insert(short, mark);
            }
            merged.extend(after(read, mark).into_iter().map(|line| (line, index)));
        }
        merged.sort_by_key(|(line, _)| line.at.unwrap_or(i64::MIN));
        if cursor_text.is_none() {
            let excess = merged.len().saturating_sub(lines as usize);
            merged.drain(..excess);
        }
        let mut entries: Vec<Value> = merged
            .iter()
            .map(|(line, index)| entry(line, Some(&labels[*index])))
            .collect();
        within_budget(&mut entries);
        let cursor = match (entries.is_empty(), cursor_text) {
            (true, Some(old)) => Some(old.to_owned()),
            _ if next.is_empty() => None,
            _ => Some(Cursor::Project(next).encode()),
        };
        Ok(json!({"entries": entries, "cursor": cursor}))
    }

    fn act(engine: &Engine, container: &str, verb: &str, timeout: Duration) -> Result<(), String> {
        let path = match verb {
            "stop" | "restart" => format!("/containers/{container}/{verb}?t=10"),
            _ => format!("/containers/{container}/{verb}"),
        };
        let response = engine.call("POST", &path, timeout)?;
        // 304: already in the requested state.
        if response.success() || response.status == 304 {
            Ok(())
        } else {
            Err(response.message())
        }
    }

    pub fn action(&self, request: &Value) -> Result<Value, String> {
        let container = container_arg(request)?;
        let verb = match request["verb"].as_str() {
            Some(v @ ("start" | "stop" | "restart" | "pause" | "unpause")) => v,
            Some(other) => return Err(format!("Unknown container action: {other}")),
            None => return Err("Container actions need a verb.".into()),
        };
        self.with_engine(|engine| Self::act(engine, container, verb, ACTION_TIMEOUT))?;
        Ok(json!({"ok": true}))
    }

    fn ids(value: &Value, what: &str) -> Result<Vec<String>, String> {
        let items = value
            .as_array()
            .ok_or_else(|| format!("{what} must be a list."))?;
        if items.is_empty() || items.len() > MAX_IDS {
            return Err(format!("{what} must list 1 to {MAX_IDS} containers."));
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for item in items {
            let id = item.as_str().unwrap_or_default();
            if seen.insert(id.to_owned()) {
                out.push(id.to_owned());
            }
        }
        Ok(out)
    }

    pub fn project_action(&self, request: &Value) -> Result<Value, String> {
        let project = project_arg(&request["project"])?;
        let verb = match request["verb"].as_str() {
            Some(v @ ("start" | "stop" | "restart")) => v,
            Some(other) => return Err(format!("Unknown project action: {other}")),
            None => return Err("Project actions need a verb.".into()),
        };
        let expected = Self::ids(&request["expectedIds"], "expectedIds")?;
        if let Some(bad) = expected.iter().find(|id| !valid_id(id)) {
            return Err(format!("Not a valid container id: {bad}"));
        }
        let started = Instant::now();
        let deadline = started + PROJECT_DEADLINE;
        self.with_engine(|engine| {
            let members: Vec<Value> = Self::list(engine, Some(project), deadline)?
                .into_iter()
                .filter(|row| {
                    !row["Labels"]["com.docker.compose.oneoff"]
                        .as_str()
                        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
                })
                .collect();
            let actual: HashSet<&str> = members
                .iter()
                .filter_map(|row| row["Id"].as_str())
                .collect();
            let wanted: HashSet<&str> = expected.iter().map(String::as_str).collect();
            if actual != wanted {
                return Err(CHANGED.into());
            }
            // Creation order to the nanosecond: Compose creates dependencies first.
            let ids: Vec<String> = actual.iter().map(|id| (*id).to_owned()).collect();
            let inspected = bounded(&ids, CONCURRENCY, |id| {
                let timeout = within(deadline, INSPECT_TIMEOUT).ok_or(OUT_OF_TIME)?;
                engine.get(&format!("/containers/{id}/json"), timeout)
            });
            let mut order = Vec::new();
            for (id, inspect) in ids.iter().zip(inspected) {
                let inspect = inspect.map_err(|e| {
                    if e == OUT_OF_TIME {
                        e
                    } else {
                        CHANGED.to_owned()
                    }
                })?;
                let created = inspect["Created"]
                    .as_str()
                    .and_then(docker::parse_time)
                    .unwrap_or(0);
                let name = inspect["Name"]
                    .as_str()
                    .unwrap_or(id)
                    .trim_start_matches('/')
                    .to_owned();
                order.push((created, id.clone(), name));
            }
            order.sort();
            if verb != "start" {
                order.reverse();
            }
            let mut failed = Vec::new();
            for (_, id, name) in &order {
                let left = PROJECT_DEADLINE.saturating_sub(started.elapsed());
                if left < Duration::from_secs(1) {
                    failed.push(format!("{name} (not attempted: out of time)"));
                    continue;
                }
                if let Err(reason) = Self::act(engine, id, verb, left.min(ACTION_TIMEOUT)) {
                    failed.push(format!("{name} ({reason})"));
                }
            }
            if failed.is_empty() {
                Ok(json!({"ok": true}))
            } else {
                Err(format!(
                    "Couldn't {verb} {} of {} containers: {}. Refresh the project before retrying.",
                    failed.len(),
                    order.len(),
                    failed.join("; ")
                ))
            }
        })
    }

    pub fn remove(&self, request: &Value) -> Result<Value, String> {
        let targets = Self::ids(&request["containers"], "containers")?;
        if let Some(bad) = targets.iter().find(|t| !valid_container(t)) {
            return Err(format!("Not a valid container: {bad}"));
        }
        let deadline = Instant::now() + PROJECT_DEADLINE;
        self.with_engine(|engine| {
            let inspected = bounded(&targets, CONCURRENCY, |target| {
                let timeout = within(deadline, INSPECT_TIMEOUT).ok_or(OUT_OF_TIME)?;
                let response =
                    engine.call("GET", &format!("/containers/{target}/json"), timeout)?;
                match response.status {
                    // Already gone: nothing to remove.
                    404 => Ok(None),
                    _ if response.success() => serde_json::from_slice::<Value>(&response.body)
                        .map(Some)
                        .map_err(|e| format!("Docker didn't answer: unreadable reply: {e}")),
                    _ => Err(response.message()),
                }
            });
            let mut doomed = Vec::new();
            for inspect in inspected {
                let Some(inspect) = inspect? else { continue };
                let name = inspect["Name"]
                    .as_str()
                    .unwrap_or_default()
                    .trim_start_matches('/')
                    .to_owned();
                let current = state(inspect["State"]["Status"].as_str().unwrap_or_default());
                if !matches!(current, "exited" | "created" | "dead") {
                    return Err(format!("{name} is running. Stop it first."));
                }
                let id = inspect["Id"].as_str().unwrap_or_default().to_owned();
                if valid_id(&id) {
                    doomed.push((id, name));
                }
            }
            let mut removed = 0;
            let mut failed = Vec::new();
            for (id, name) in &doomed {
                let Some(timeout) = within(deadline, ACTION_TIMEOUT) else {
                    failed.push(format!("{name} (not attempted: out of time)"));
                    continue;
                };
                // Volumes stay (v=0) and nothing is forced.
                match engine.call("DELETE", &format!("/containers/{id}?v=0&force=0"), timeout) {
                    Ok(r) if r.success() => removed += 1,
                    Ok(r) if r.status == 404 => {}
                    Ok(r) => failed.push(format!("{name} ({})", r.message())),
                    Err(e) => failed.push(format!("{name} ({e})")),
                }
            }
            if failed.is_empty() {
                Ok(json!({"removed": removed}))
            } else {
                Err(format!(
                    "Removed {removed} of {} containers; couldn't remove {}.",
                    doomed.len(),
                    failed.join("; ")
                ))
            }
        })
    }

    /// `GET /system/df`, split so the slow volume scan never holds up the
    /// rest. Sizing volumes walks every volume on disk (seconds on a busy
    /// engine); images, containers and build cache come from the engine's own
    /// bookkeeping in milliseconds. `quick` lists volumes unmeasured,
    /// `volumes` measures only them, and no part answers everything at once.
    pub fn disk(&self, request: &Value) -> Result<Value, String> {
        self.with_engine(|engine| match request["part"].as_str() {
            Some("quick") => {
                let mut df = engine.get(
                    "/system/df?type=image&type=container&type=build-cache",
                    DF_TIMEOUT,
                )?;
                // Engines before API 1.42 ignore `type` and measure volumes anyway.
                let measured = df["Volumes"].is_array();
                if !measured {
                    let listed = engine.get("/volumes", DF_TIMEOUT)?;
                    df["Volumes"] = unmeasured_volumes(&listed, &df["Containers"]);
                }
                let mut reply = disk_json(&df);
                reply["volumesMeasured"] = measured.into();
                Ok(reply)
            }
            Some("volumes") => {
                let mut reply = disk_json(&engine.get("/system/df?type=volume", DF_TIMEOUT)?);
                reply["volumesMeasured"] = true.into();
                Ok(reply)
            }
            Some(other) => Err(format!("Unknown disk part: {other}")),
            None => Ok(disk_json(&engine.get("/system/df", DF_TIMEOUT)?)),
        })
    }

    pub fn prune(&self, request: &Value) -> Result<Value, String> {
        let path = match request["what"].as_str() {
            Some("containers") => "/containers/prune".to_owned(),
            Some("dangling-images") => format!(
                "/images/prune?filters={}",
                docker::encode(r#"{"dangling":["true"]}"#)
            ),
            // The API's spelling of `docker image prune --all`.
            Some("unused-images") => format!(
                "/images/prune?filters={}",
                docker::encode(r#"{"dangling":["false"]}"#)
            ),
            Some("build-cache") => "/build/prune".to_owned(),
            // The engine's default: anonymous volumes only.
            Some("volumes") => "/volumes/prune".to_owned(),
            Some(other) => return Err(format!("Unknown prune target: {other}")),
            None => return Err("Say what to prune.".into()),
        };
        self.with_engine(|engine| {
            let response = engine.call("POST", &path, PRUNE_TIMEOUT)?;
            if !response.success() {
                return Err(response.message());
            }
            let reply: Value = serde_json::from_slice(&response.body)
                .map_err(|e| format!("Docker didn't answer: unreadable reply: {e}"))?;
            Ok(prune_json(&reply))
        })
    }

    /// Answers a Containers request, or None when the op is not one of ours.
    pub fn handle(&self, request: &Value) -> Option<Value> {
        let find = |project: &str, deadline| self.compose_rows(project, deadline);
        let result = match request.get("op").and_then(Value::as_str)? {
            op if op.starts_with("compose_") => self.compose.handle(request, &find)?,
            "containers" => self.snapshot(),
            "container_inspect" => self.inspect(request),
            "container_logs" => self.logs(request),
            "container_action" => self.action(request),
            "project_action" => self.project_action(request),
            "containers_remove" => self.remove(request),
            "docker_disk" => self.disk(request),
            "docker_prune" => self.prune(request),
            _ => return None,
        };
        Some(result.unwrap_or_else(|error| {
            let mut reply = Map::new();
            reply.insert("error".into(), error.into());
            Value::Object(reply)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        sync::Arc,
    };

    #[test]
    fn rows_without_a_full_id_are_skipped_in_project_logs() {
        let mut rows = vec![
            serde_json::json!({ "Id": A }),
            serde_json::json!({}),
            serde_json::json!({ "Id": "" }),
            serde_json::json!({ "Id": "abc" }),
            serde_json::json!({ "Id": 7 }),
        ];
        keep_identified(&mut rows);
        assert_eq!(rows, vec![serde_json::json!({ "Id": A })]);
    }

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    type Reply = (u16, &'static str, Vec<u8>);
    /// A Docker engine on a unix socket that answers from a closure and
    /// records each request line.
    struct Fake {
        _dir: tempfile::TempDir,
        socket: PathBuf,
        requests: Arc<Mutex<Vec<String>>>,
    }
    fn fake(answer: impl Fn(&str, &str) -> Reply + Send + Sync + 'static) -> Fake {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("docker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let answer = Arc::new(answer);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let seen = seen.clone();
                let answer = answer.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                            break;
                        }
                    }
                    let mut parts = first.split_whitespace();
                    let (method, target) = (parts.next().unwrap(), parts.next().unwrap());
                    seen.lock().unwrap().push(format!("{method} {target}"));
                    let path = target
                        .strip_prefix("/v1.47")
                        .or_else(|| target.strip_prefix("/v1.41"))
                        .unwrap_or(target);
                    let (status, content_type, body) = answer(method, path);
                    // Alternate encodings so both readers see real traffic.
                    let mut out =
                        format!("HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\n");
                    if body.len() % 2 == 0 {
                        out += "Transfer-Encoding: chunked\r\n\r\n";
                        let mut bytes = out.into_bytes();
                        for chunk in body.chunks(7) {
                            bytes.extend(format!("{:x}\r\n", chunk.len()).as_bytes());
                            bytes.extend(chunk);
                            bytes.extend(b"\r\n");
                        }
                        bytes.extend(b"0\r\n\r\n");
                        stream.write_all(&bytes).unwrap();
                    } else {
                        out += &format!("Content-Length: {}\r\n\r\n", body.len());
                        let mut bytes = out.into_bytes();
                        bytes.extend(&body);
                        stream.write_all(&bytes).unwrap();
                    }
                });
            }
        });
        Fake {
            _dir: dir,
            socket,
            requests,
        }
    }
    fn json_reply(value: Value) -> Reply {
        (200, "application/json", value.to_string().into_bytes())
    }
    fn containers(fake: &Fake) -> Containers {
        let socket = fake.socket.clone();
        Containers::new(Box::new(move || {
            vec!["/nonexistent/docker.sock".into(), socket.clone()]
        }))
    }
    fn version() -> Reply {
        json_reply(
            json!({"Version": "29.4.0", "ApiVersion": "1.54", "MinAPIVersion": "1.44", "Os": "linux"}),
        )
    }
    fn info() -> Reply {
        json_reply(
            json!({"OperatingSystem": "Debian GNU/Linux 13 (trixie)", "SecurityOptions": ["name=seccomp,profile=builtin"], "MemTotal": 8_000_000_000u64}),
        )
    }
    fn frame(stream: u8, payload: &str) -> Vec<u8> {
        let mut out = vec![stream, 0, 0, 0];
        out.extend((payload.len() as u32).to_be_bytes());
        out.extend(payload.as_bytes());
        out
    }

    #[test]
    fn ids_names_and_projects_are_validated() {
        assert!(valid_id(A));
        assert!(!valid_id(&A[..63]));
        assert!(!valid_id(&format!("{}g", &A[..63])));
        for good in [&A[..12], A, "web", "my-app_web.1", "X9"] {
            assert!(valid_container(good), "{good}");
        }
        for bad in [
            "", "-x", ".x", "_x", "a b", "a/b", "a;b", "$(id)", "a\nb", "é",
        ] {
            assert!(!valid_container(bad), "{bad:?}");
        }
        assert!(!valid_container(&"a".repeat(129)));
        for good in ["app", "my-app_1", "0x"] {
            assert!(valid_project(good), "{good}");
        }
        for bad in [
            "", "-app", "_app", "App", "a b", "a;id", "a$(id)", "a\nb", "a.b",
        ] {
            assert!(!valid_project(bad), "{bad:?}");
        }
        assert!(!valid_project(&"a".repeat(257)));

        let c = Containers::new(Box::new(Vec::new));
        let error = |request: Value| {
            c.handle(&request).unwrap()["error"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(
            error(json!({"op": "container_action", "container": "a;b", "verb": "stop"})),
            "Not a valid container: a;b"
        );
        assert_eq!(
            error(json!({"op": "container_action", "container": "web", "verb": "kill"})),
            "Unknown container action: kill"
        );
        assert_eq!(
            error(json!({"op": "container_logs", "container": "web", "project": "app"})),
            "Logs need either a container or a project, not both."
        );
        assert_eq!(
            error(json!({"op": "container_logs", "project": "App"})),
            "Not a valid Compose project: App"
        );
        assert_eq!(
            error(json!({"op": "container_logs", "container": "web", "cursor": "p1.zz"})),
            "Invalid log cursor."
        );
        assert_eq!(
            error(
                json!({"op": "project_action", "project": "app", "verb": "pause", "expectedIds": [A]})
            ),
            "Unknown project action: pause"
        );
        assert_eq!(
            error(
                json!({"op": "project_action", "project": "app", "verb": "stop", "expectedIds": ["abc"]})
            ),
            "Not a valid container id: abc"
        );
        assert_eq!(
            error(json!({"op": "containers_remove", "containers": []})),
            "containers must list 1 to 500 containers."
        );
        assert_eq!(
            error(json!({"op": "docker_prune", "what": "everything"})),
            "Unknown prune target: everything"
        );
        // Valid requests reach engine discovery, which finds nothing here.
        assert_eq!(
            error(json!({"op": "docker_prune", "what": "volumes"})),
            docker::NOT_RUNNING
        );
        assert!(c.handle(&json!({"op": "services"})).is_none());
    }

    #[test]
    fn ports_lose_their_ipv6_twins_and_bare_duplicates() {
        let listed = json!([
            {"IP": "0.0.0.0", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
            {"IP": "::", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
            {"PrivatePort": 80, "Type": "tcp"},
            {"IP": "127.0.0.1", "PrivatePort": 53, "PublicPort": 5353, "Type": "udp"},
            {"PrivatePort": 9000, "Type": "tcp"},
            {"PrivatePort": 9000, "Type": "tcp"}
        ]);
        assert_eq!(
            Value::Array(ports(&listed)),
            json!([
                {"ip": "127.0.0.1", "hostPort": 5353, "containerPort": 53, "protocol": "udp"},
                {"ip": "0.0.0.0", "hostPort": 8080, "containerPort": 80, "protocol": "tcp"},
                {"ip": null, "hostPort": null, "containerPort": 9000, "protocol": "tcp"}
            ])
        );
    }

    #[test]
    fn compose_labels_and_list_rows() {
        let row = json!({
            "Id": A, "Names": ["/other/alias", "/app-web-1"], "Image": "sha256:123", "ImageID": "sha256:123",
            "Command": "nginx -g 'daemon off;'", "Created": 1_791_109_265, "State": "exited",
            "Status": "Exited (137) 2 minutes ago", "Ports": [],
            "Labels": {"com.docker.compose.project": "app", "com.docker.compose.service": "web",
                       "com.docker.compose.oneoff": "False", "com.docker.compose.container-number": "1",
                       "com.docker.compose.project.working_dir": "/srv/app"}
        });
        // Without inspect, what the list says.
        let bare = container_json(&row, None, None);
        assert_eq!(bare["name"], "app-web-1");
        assert_eq!(bare["exitCode"], 137);
        assert_eq!(bare["created"], 1_791_109_265_000i64);
        assert_eq!(bare["health"], Value::Null);
        assert_eq!(
            bare["compose"],
            json!({"project": "app", "service": "web", "oneoff": false, "number": 1,
                   "workingDir": "/srv/app", "configFiles": null})
        );
        assert_eq!(bare["restartPolicy"], Value::Null);
        let inspect = json!({
            "Config": {"Image": "nginx:1.27"},
            "State": {"Status": "exited", "ExitCode": 137, "OOMKilled": true, "Error": "",
                      "StartedAt": "2026-10-04T10:21:05.5Z", "FinishedAt": "2026-10-04T10:22:05Z",
                      "Health": {"Status": "unhealthy"}},
            "RestartCount": 3, "HostConfig": {"RestartPolicy": {"Name": ""}}
        });
        let full = container_json(&row, Some(&summary(&inspect)), None);
        assert_eq!(full["image"], "nginx:1.27");
        assert_eq!(full["oomKilled"], true);
        assert_eq!(full["error"], Value::Null);
        assert_eq!(full["restartCount"], 3);
        assert_eq!(full["restartPolicy"], "no");
        assert_eq!(full["health"], "unhealthy");
        assert_eq!(full["startedAt"], 1_791_109_265_500i64);
        assert_eq!(full["finishedAt"], 1_791_109_325_000i64);
        assert_eq!(full["cpu"], Value::Null);
        let mut plain = row.clone();
        plain["Labels"] = json!({});
        plain["State"] = "running".into();
        plain["Status"] = "Up 2 hours (health: starting)".into();
        let plain = container_json(&plain, None, None);
        assert_eq!(plain["compose"], Value::Null);
        assert_eq!(plain["exitCode"], Value::Null);
        assert_eq!(plain["health"], "starting");
        assert_eq!(state("configured"), "created");
        assert_eq!(state("stopped"), "exited");
        assert_eq!(state("weird"), "dead");
    }

    #[test]
    fn cpu_is_a_delta_and_memory_excludes_inactive_cache() {
        // 0.5 s of CPU over 2 s is a quarter of one core; 3 s over 2 s is 150%.
        assert_eq!(
            cpu_percent(1_000_000_000, 1_500_000_000, Duration::from_secs(2)),
            Some(25.0)
        );
        assert_eq!(
            cpu_percent(0, 3_000_000_000, Duration::from_secs(2)),
            Some(150.0)
        );
        // A restarted container's counter went backwards: no reading.
        assert_eq!(cpu_percent(5, 1, Duration::from_secs(1)), None);
        assert_eq!(cpu_percent(1, 5, Duration::ZERO), None);

        let v2 = json!({"memory_stats": {"usage": 100_000, "limit": 8_000_000_000u64, "stats": {"inactive_file": 30_000}}});
        assert_eq!(memory_usage(&v2), Some(70_000));
        assert_eq!(memory_limit(&v2, 8_000_000_000), None);
        assert_eq!(memory_limit(&v2, 0), Some(8_000_000_000));
        let v1 = json!({"memory_stats": {"usage": 100_000, "limit": 536_870_912, "stats": {"total_inactive_file": 40_000, "inactive_file": 1}}});
        assert_eq!(memory_usage(&v1), Some(60_000));
        assert_eq!(memory_limit(&v1, 8_000_000_000), Some(536_870_912));
        let odd = json!({"memory_stats": {"usage": 10, "stats": {"inactive_file": 50}}});
        assert_eq!(memory_usage(&odd), Some(10));
        assert_eq!(memory_usage(&json!({"memory_stats": {}})), None);
        assert_eq!(
            memory_limit(&json!({"memory_stats": {"limit": u64::MAX}}), 0),
            None
        );
    }

    #[test]
    fn disk_usage_and_what_can_be_reclaimed() {
        let df = json!({
            "LayersSize": 900,
            "Images": [
                {"Id": "sha256:1", "RepoTags": ["nginx:1.27"], "Size": 500, "SharedSize": 100, "Created": 10, "Containers": 2},
                {"Id": "sha256:2", "RepoTags": ["<none>:<none>"], "Size": 300, "SharedSize": -1, "Created": 20, "Containers": 0},
                {"Id": "sha256:3", "RepoTags": null, "Size": 100, "SharedSize": 0, "Created": 30, "Containers": 0}
            ],
            "Containers": [
                {"Id": A, "SizeRw": 10, "State": "running"},
                {"Id": B, "SizeRw": 20, "State": "exited"},
                {"Id": C, "SizeRw": 5, "State": "paused"}
            ],
            "Volumes": [
                {"Name": A, "Driver": "local", "Labels": null, "UsageData": {"Size": 1000, "RefCount": 0}},
                {"Name": B, "Driver": "local", "Labels": {"com.docker.compose.volume": "x", "com.docker.compose.project": "app"}, "UsageData": {"Size": 50, "RefCount": 1}},
                {"Name": "named", "Driver": "local", "Labels": {}, "UsageData": {"Size": -1, "RefCount": -1}}
            ],
            "BuildCache": [
                {"ID": "x", "Size": 70, "InUse": false},
                {"ID": "y", "Size": 30, "InUse": true}
            ]
        });
        let disk = disk_json(&df);
        assert_eq!(
            disk["totals"],
            json!({
                "images": {"count": 3, "size": 900, "reclaimable": 400},
                "containers": {"count": 3, "size": 35, "reclaimable": 20},
                "volumes": {"count": 3, "size": 1050, "reclaimable": 1000},
                "buildCache": {"count": 2, "size": 100, "reclaimable": 70}
            })
        );
        let images = disk["images"].as_array().unwrap();
        assert_eq!(images[0]["tags"], json!(["nginx:1.27"]));
        assert_eq!(images[0]["created"], 10_000);
        assert_eq!(images[1]["dangling"], true);
        assert_eq!(images[1]["sharedSize"], Value::Null);
        assert_eq!(images[2]["tags"], json!([]));
        let volumes = disk["volumes"].as_array().unwrap();
        assert_eq!(volumes[0]["anonymous"], true);
        assert_eq!(volumes[1]["anonymous"], false);
        assert_eq!(volumes[1]["compose"], "app");
        assert_eq!(volumes[2]["size"], Value::Null);
        assert_eq!(volumes[2]["refCount"], Value::Null);
        assert_eq!(volumes[2]["anonymous"], false);

        assert_eq!(
            prune_json(
                &json!({"ImagesDeleted": [{"Untagged": "x:1"}, {"Deleted": "sha256:1"}, {"Deleted": "sha256:2"}], "SpaceReclaimed": 99})
            ),
            json!({"removed": 2, "reclaimed": 99})
        );
        assert_eq!(
            prune_json(&json!({"ContainersDeleted": null, "SpaceReclaimed": 0})),
            json!({"removed": 0, "reclaimed": 0})
        );
    }

    #[test]
    fn quick_disk_never_waits_for_the_volume_scan() {
        let fake = fake(|_, path| match path {
            "/version" => version(),
            "/info" => info(),
            p if p.starts_with("/system/df?type=image&type=container&type=build-cache") => {
                json_reply(json!({
                    "Images": [{"Id": "sha256:1", "RepoTags": ["a:1"], "Size": 5, "Containers": 1}],
                    "Containers": [
                        {"Id": A, "SizeRw": 3, "State": "running", "Mounts": [{"Type": "volume", "Name": "data"}]},
                        {"Id": B, "SizeRw": 1, "State": "exited", "Mounts": [{"Type": "bind", "Source": "/x"}]}
                    ],
                    "BuildCache": []
                }))
            }
            "/volumes" => json_reply(json!({"Volumes": [
                {"Name": "data", "Driver": "local", "Labels": {"com.docker.compose.project": "app"}},
                {"Name": B, "Driver": "local", "Labels": null}
            ]})),
            "/system/df?type=volume" => json_reply(json!({"Volumes": [
                {"Name": "data", "Driver": "local", "Labels": {}, "UsageData": {"Size": 70, "RefCount": 1}}
            ]})),
            _ => (404, "application/json", b"{}".to_vec()),
        });
        let c = containers(&fake);
        let quick = c.disk(&json!({"part": "quick"})).unwrap();
        assert_eq!(quick["volumesMeasured"], false);
        assert_eq!(quick["totals"]["images"]["size"], 5);
        assert_eq!(quick["totals"]["containers"]["count"], 2);
        let volumes = quick["volumes"].as_array().unwrap();
        assert_eq!(volumes.len(), 2);
        let data = volumes.iter().find(|v| v["name"] == "data").unwrap();
        assert_eq!(data["size"], Value::Null, "unmeasured, not zero");
        assert_eq!(data["refCount"], 1, "counted from container mounts");
        assert_eq!(data["compose"], "app");
        assert!(
            !fake
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.contains("/system/df ") || r.contains("type=volume")),
            "the quick reading never asks the engine to size volumes"
        );
        let measured = c.disk(&json!({"part": "volumes"})).unwrap();
        assert_eq!(measured["volumesMeasured"], true);
        assert_eq!(measured["totals"]["volumes"]["size"], 70);
        assert!(c.disk(&json!({"part": "nope"})).is_err());
    }

    #[test]
    fn inspect_details() {
        let inspect = json!({
            "Id": A, "Name": "/web", "Created": "2026-10-04T10:21:05.123Z",
            "Config": {
                "Image": "nginx:1.27", "Entrypoint": null, "Cmd": ["nginx", "-g", "daemon off;"],
                "WorkingDir": "", "User": "101", "Hostname": "abc",
                "Env": ["PATH=/usr/bin", "TOKEN=a=b", "EMPTY"],
                "Labels": {"b": "2", "a": "1"},
                "Healthcheck": {"Test": ["CMD-SHELL", "true"], "Interval": 2_000_000_000u64}
            },
            "Mounts": [
                {"Type": "volume", "Name": "data", "Source": "/var/lib/docker/volumes/data/_data", "Destination": "/data", "Mode": "z", "RW": true},
                {"Type": "tmpfs", "Source": "", "Destination": "/tmp", "Mode": "", "RW": true}
            ],
            "NetworkSettings": {"Networks": {"app_default": {"IPAddress": "172.18.0.2", "Aliases": ["web"]}, "none": {"IPAddress": "", "Aliases": null}}},
            "HostConfig": {"RestartPolicy": {"Name": "unless-stopped"}},
            "State": {"Health": {"Log": (0..7).map(|i| json!({"Start": format!("2026-10-04T10:21:0{i}Z"), "ExitCode": i % 2, "Output": format!("try {i}")})).collect::<Vec<_>>()}}
        });
        let out = inspect_json(&inspect);
        assert_eq!(out["name"], "web");
        assert_eq!(out["created"], 1_791_109_265_123i64);
        assert_eq!(out["entrypoint"], Value::Null);
        assert_eq!(out["cmd"], json!(["nginx", "-g", "daemon off;"]));
        assert_eq!(out["workingDir"], Value::Null);
        assert_eq!(out["user"], "101");
        assert_eq!(
            out["env"],
            json!([{"name": "PATH", "value": "/usr/bin"}, {"name": "TOKEN", "value": "a=b"}, {"name": "EMPTY", "value": ""}])
        );
        assert_eq!(out["mounts"][0]["name"], "data");
        assert_eq!(out["mounts"][1]["source"], Value::Null);
        assert_eq!(out["mounts"][1]["mode"], Value::Null);
        assert_eq!(out["mounts"][1]["name"], Value::Null);
        assert_eq!(
            out["networks"],
            json!([{"name": "app_default", "ipAddress": "172.18.0.2", "aliases": ["web"]}, {"name": "none", "ipAddress": null, "aliases": []}])
        );
        assert_eq!(out["restartPolicy"], "unless-stopped");
        assert_eq!(out["labels"], json!({"a": "1", "b": "2"}));
        assert_eq!(
            out["healthcheck"],
            json!({"test": ["CMD-SHELL", "true"], "interval": 2000})
        );
        let last = out["lastHealth"].as_array().unwrap();
        assert_eq!(last.len(), 5);
        assert_eq!(last[4]["output"], "try 6");
        assert_eq!(last[0]["at"], 1_791_109_262_000i64);
        let mut none = inspect.clone();
        none["Config"]["Healthcheck"] = json!({"Test": ["NONE"]});
        assert_eq!(inspect_json(&none)["healthcheck"], Value::Null);
    }

    fn line(at: Option<i64>, message: &str) -> Line {
        Line {
            at,
            stderr: false,
            message: message.into(),
        }
    }

    #[test]
    fn cursors_resume_strictly_after_what_was_delivered() {
        let first = vec![
            line(Some(10), "a"),
            line(Some(20), "b"),
            line(Some(20), "c"),
        ];
        let mark = advance(&first, None).unwrap();
        assert_eq!(mark, Mark { at: 20, seen: 2 });
        let cursor = Cursor::Container(mark).encode();
        assert_eq!(cursor, "c1.20.2");
        assert_eq!(Cursor::parse(&cursor), Some(Cursor::Container(mark)));
        // The engine's since is inclusive: both lines at 20 come back.
        let again = vec![
            line(Some(20), "b"),
            line(Some(20), "c"),
            line(Some(20), "d"),
            line(Some(30), "e"),
        ];
        let next = advance(&again, Some(mark)).unwrap();
        let fresh: Vec<String> = after(again, Some(mark))
            .into_iter()
            .map(|l| l.message)
            .collect();
        assert_eq!(fresh, ["d", "e"]);
        assert_eq!(next, Mark { at: 30, seen: 1 });
        // Nothing new: the old mark stands.
        let quiet = vec![line(Some(30), "e")];
        assert!(after(quiet.clone(), Some(next)).is_empty());
        assert_eq!(advance(&quiet, Some(next)), Some(next));
        assert_eq!(advance(&[], Some(next)), Some(next));
        assert_eq!(advance(&[line(None, "x")], None), None);
        // An engine whose since is coarser than ours still never repeats.
        let coarse = vec![
            line(Some(5), "old"),
            line(Some(30), "e"),
            line(Some(31), "f"),
        ];
        let fresh: Vec<String> = after(coarse, Some(next))
            .into_iter()
            .map(|l| l.message)
            .collect();
        assert_eq!(fresh, ["f"]);
        assert_eq!(
            since(Mark {
                at: 1_791_109_265_000_000_001,
                seen: 1
            }),
            "1791109265.000000001"
        );

        let project = Cursor::Project(BTreeMap::from([
            (A[..12].to_owned(), Mark { at: 5, seen: 1 }),
            (B[..12].to_owned(), Mark { at: 7, seen: 3 }),
        ]));
        let text = project.encode();
        assert_eq!(text, "p1.aaaaaaaaaaaa-5-1,bbbbbbbbbbbb-7-3");
        assert_eq!(Cursor::parse(&text), Some(project));
        assert_eq!(Cursor::parse("p1."), Some(Cursor::Project(BTreeMap::new())));
        for bad in [
            "",
            "c1.",
            "c1.-5.1",
            "c1.5",
            "c2.5.1",
            "p1.short-1-1",
            "p1.aaaaaaaaaaaa-1",
            "p1.aaaaaaaaaaaa-1-1-1",
            "c1.5.1;rm",
        ] {
            assert_eq!(Cursor::parse(bad), None, "{bad}");
        }
        assert_eq!(Cursor::parse(&format!("c1.{}.1", "9".repeat(5000))), None);
    }

    #[test]
    fn log_pages_stay_under_the_reply_budget() {
        let big = "x".repeat(MAX_LINES as usize);
        let mut entries: Vec<Value> = (0..3000)
            .map(|i| json!({"at": i, "stream": "stdout", "message": big, "container": null}))
            .collect();
        within_budget(&mut entries);
        let size: usize = entries.iter().map(|e| e.to_string().len() + 1).sum();
        assert!(size <= LOG_BUDGET && entries.len() < 3000);
        // The newest survive.
        assert_eq!(entries.last().unwrap()["at"], 2999);
    }

    #[test]
    fn published_ports_keep_tcp_host_bindings_and_compose_labels() {
        let row = json!({
            "Names": ["/app-web-1"], "Image": "nginx:1",
            "Labels": {"com.docker.compose.project": "app", "com.docker.compose.service": "web",
                "com.docker.compose.project.working_dir": "/srv/app"},
            "Ports": [
                {"IP": "0.0.0.0", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
                {"IP": "::", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
                {"IP": "0.0.0.0", "PrivatePort": 53, "PublicPort": 5353, "Type": "udp"},
                {"PrivatePort": 9000, "Type": "tcp"}
            ]
        });
        assert_eq!(
            published(&row).unwrap(),
            json!({"name": "app-web-1", "image": "nginx:1", "project": "app", "service": "web",
                "workdir": "/srv/app", "bindings": [{"address": "0.0.0.0", "port": 8080}, {"address": "::", "port": 8080}]})
        );
        let unpublished = json!({"Names": ["/db"], "Labels": {}, "Ports": [{"PrivatePort": 5432, "Type": "tcp"}]});
        assert_eq!(published(&unpublished), None);
    }

    #[test]
    fn snapshot_figures_come_from_the_engine() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let counter = ticks.clone();
        let engine = fake(move |method, path| match (method, path) {
            ("GET", "/version") => version(),
            ("GET", "/info") => info(),
            ("GET", "/containers/json?all=1") => json_reply(json!([
                {"Id": A, "Names": ["/web"], "Image": "nginx", "ImageID": "sha256:1", "Command": "nginx",
                 "Created": 200, "State": "running", "Status": "Up 1 minute (healthy)",
                 "Ports": [{"IP": "0.0.0.0", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"}], "Labels": {}},
                {"Id": B, "Names": ["/job"], "Image": "busybox", "ImageID": "sha256:2", "Command": "true",
                 "Created": 100, "State": "exited", "Status": "Exited (3) 1 minute ago", "Ports": [], "Labels": {}},
                {"Id": "not-an-id", "Names": ["/bad"], "Created": 300, "State": "running"}
            ])),
            ("GET", p) if p == format!("/containers/{A}/json") => json_reply(json!({
                "Config": {"Image": "nginx:1.27"}, "RestartCount": 0, "HostConfig": {"RestartPolicy": {"Name": "always"}},
                "State": {"Status": "running", "ExitCode": 0, "OOMKilled": false, "Error": "",
                          "StartedAt": "2026-10-04T10:21:05Z", "FinishedAt": "0001-01-01T00:00:00Z", "Health": {"Status": "healthy"}}
            })),
            ("GET", p) if p == format!("/containers/{B}/json") => json_reply(json!({
                "Config": {"Image": "busybox"}, "RestartCount": 0, "HostConfig": {"RestartPolicy": {"Name": "no"}},
                "State": {"Status": "exited", "ExitCode": 3, "OOMKilled": false, "Error": "boom",
                          "StartedAt": "2026-10-04T10:21:05Z", "FinishedAt": "2026-10-04T10:21:06Z"}
            })),
            ("GET", p) if p.starts_with(&format!("/containers/{A}/stats")) => {
                let tick = counter.fetch_add(1, Ordering::SeqCst) as u64;
                json_reply(json!({
                    "cpu_stats": {"cpu_usage": {"total_usage": tick * 50_000_000}},
                    "memory_stats": {"usage": 10_000_000, "limit": 8_000_000_000u64, "stats": {"inactive_file": 1_000_000}}
                }))
            }
            _ => (
                404,
                "application/json",
                br#"{"message":"page not found"}"#.to_vec(),
            ),
        });
        let c = containers(&engine);
        let first = c.handle(&json!({"op": "containers"})).unwrap();
        assert_eq!(
            first["engine"],
            json!({"version": "29.4.0", "apiVersion": "1.47", "os": "Debian GNU/Linux 13 (trixie)", "rootless": false, "podman": false})
        );
        assert_eq!(first["truncated"], false);
        let rows = first["containers"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let (web, job) = (&rows[0], &rows[1]);
        assert_eq!(web["image"], "nginx:1.27");
        assert_eq!(web["health"], "healthy");
        assert_eq!(web["restartPolicy"], "always");
        assert_eq!(web["finishedAt"], Value::Null);
        assert_eq!(web["cpu"], Value::Null, "the first reading has no interval");
        assert_eq!(web["memory"], 9_000_000);
        assert_eq!(web["memoryLimit"], Value::Null);
        assert_eq!(web["ports"][0]["hostPort"], 8080);
        assert_eq!(job["exitCode"], 3);
        assert_eq!(job["error"], "boom");
        assert_eq!(job["memory"], Value::Null);
        std::thread::sleep(Duration::from_millis(50));
        let second = c.handle(&json!({"op": "containers"})).unwrap();
        assert!(
            second["containers"][0]["cpu"]
                .as_f64()
                .is_some_and(|cpu| cpu > 0.0)
        );
        // Unchanged containers are not inspected again.
        let requests = engine.requests.lock().unwrap();
        let inspects = requests
            .iter()
            .filter(|r| r.ends_with(&format!("{A}/json")))
            .count();
        assert_eq!(inspects, 1, "{requests:?}");
        // The engine was found once; the versioned prefix is used throughout.
        assert_eq!(requests.iter().filter(|r| *r == "GET /version").count(), 1);
        assert!(
            requests
                .iter()
                .any(|r| r == "GET /v1.47/containers/json?all=1")
        );
    }

    #[test]
    fn logs_page_and_tail_without_repeats() {
        let body = [
            frame(1, "2026-10-04T10:21:05.000000001Z one\n"),
            frame(2, "2026-10-04T10:21:05.000000002Z two\n"),
            frame(1, "2026-10-04T10:21:05.000000002Z three\n"),
        ]
        .concat();
        let tail = [
            frame(2, "2026-10-04T10:21:05.000000002Z two\n"),
            frame(1, "2026-10-04T10:21:05.000000002Z three\n"),
            frame(1, "2026-10-04T10:21:06Z four\n"),
        ]
        .concat();
        let engine = fake(move |_, path| match path {
            "/version" => version(),
            "/info" => info(),
            p if p.starts_with("/containers/web/logs")
                && p.contains("since=1791109265.000000002") =>
            {
                (
                    200,
                    "application/vnd.docker.multiplexed-stream",
                    tail.clone(),
                )
            }
            p if p.starts_with("/containers/web/logs")
                && p.contains("since=1791109266.000000000") =>
            {
                (
                    200,
                    "application/vnd.docker.multiplexed-stream",
                    frame(1, "2026-10-04T10:21:06Z four\n"),
                )
            }
            p if p.starts_with("/containers/web/logs") && p.contains("tail=2&")
                || p.ends_with("tail=2") =>
            {
                (200, "application/vnd.docker.raw-stream", body.clone())
            }
            _ => (
                404,
                "application/json",
                br#"{"message":"No such container: web"}"#.to_vec(),
            ),
        });
        let c = containers(&engine);
        let first = c
            .handle(&json!({"op": "container_logs", "container": "web", "lines": 2}))
            .unwrap();
        assert_eq!(
            first,
            json!({
                "entries": [
                    {"at": 1_791_109_265_000i64, "stream": "stdout", "message": "one", "container": null},
                    {"at": 1_791_109_265_000i64, "stream": "stderr", "message": "two", "container": null},
                    {"at": 1_791_109_265_000i64, "stream": "stdout", "message": "three", "container": null}
                ],
                "cursor": "c1.1791109265000000002.2"
            })
        );
        let next = c
            .handle(&json!({"op": "container_logs", "container": "web", "cursor": first["cursor"]}))
            .unwrap();
        assert_eq!(next["entries"].as_array().unwrap().len(), 1);
        assert_eq!(next["entries"][0]["message"], "four");
        assert_eq!(next["cursor"], "c1.1791109266000000000.1");
        let quiet = c
            .handle(&json!({"op": "container_logs", "container": "web", "cursor": next["cursor"]}))
            .unwrap();
        assert_eq!(
            quiet,
            json!({"entries": [], "cursor": "c1.1791109266000000000.1"})
        );
        let missing = c
            .handle(&json!({"op": "container_logs", "container": "gone"}))
            .unwrap();
        assert_eq!(missing["error"], "No such container: web");
    }

    #[test]
    fn project_actions_and_removal_check_before_acting() {
        let engine = fake(move |method, path| match (method, path) {
            (_, "/version") => version(),
            (_, "/info") => info(),
            ("GET", p) if p.starts_with("/containers/json?all=1&filters=") => json_reply(json!([
                {"Id": A, "Names": ["/app-web-1"], "Created": 2, "State": "running", "Labels": {"com.docker.compose.project": "app"}},
                {"Id": B, "Names": ["/app-db-1"], "Created": 1, "State": "running", "Labels": {"com.docker.compose.project": "app"}},
                {"Id": C, "Names": ["/app-run-1"], "Created": 3, "State": "exited", "Labels": {"com.docker.compose.project": "app", "com.docker.compose.oneoff": "True"}}
            ])),
            ("GET", p) if p == format!("/containers/{A}/json") => json_reply(
                json!({"Id": A, "Name": "/app-web-1", "Created": "2026-10-04T10:21:05.2Z", "State": {"Status": "running"}}),
            ),
            ("GET", p) if p == format!("/containers/{B}/json") => json_reply(
                json!({"Id": B, "Name": "/app-db-1", "Created": "2026-10-04T10:21:05.1Z", "State": {"Status": "running"}}),
            ),
            ("GET", p) if p == format!("/containers/{C}/json") => json_reply(
                json!({"Id": C, "Name": "/app-run-1", "Created": "2026-10-04T10:21:05.3Z", "State": {"Status": "exited"}}),
            ),
            ("POST", p) if p.contains("/stop?t=10") => (204, "text/plain", Vec::new()),
            ("POST", p) if p.ends_with("/start") && p.contains(B) => {
                (304, "text/plain", Vec::new())
            }
            ("POST", p) if p.ends_with("/start") => (
                500,
                "application/json",
                br#"{"message":"port is already allocated"}"#.to_vec(),
            ),
            ("DELETE", p) if p.contains("v=0&force=0") => (204, "text/plain", Vec::new()),
            _ => (
                404,
                "application/json",
                br#"{"message":"No such container"}"#.to_vec(),
            ),
        });
        let c = containers(&engine);
        let changed = c
            .handle(&json!({"op": "project_action", "project": "app", "verb": "stop", "expectedIds": [A]}))
            .unwrap();
        assert_eq!(changed["error"], CHANGED);
        let stopped = c
            .handle(&json!({"op": "project_action", "project": "app", "verb": "stop", "expectedIds": [B, A, A]}))
            .unwrap();
        assert_eq!(stopped, json!({"ok": true}));
        let failed = c
            .handle(&json!({"op": "project_action", "project": "app", "verb": "start", "expectedIds": [A, B]}))
            .unwrap();
        assert_eq!(
            failed["error"],
            "Couldn't start 1 of 2 containers: app-web-1 (port is already allocated). Refresh the project before retrying."
        );
        let running = c
            .handle(&json!({"op": "containers_remove", "containers": [C, A]}))
            .unwrap();
        assert_eq!(running["error"], "app-web-1 is running. Stop it first.");
        let removed = c
            .handle(&json!({"op": "containers_remove", "containers": [C, "gone-already"]}))
            .unwrap();
        assert_eq!(removed, json!({"removed": 1}));
        let requests = engine.requests.lock().unwrap();
        let acted: Vec<&str> = requests
            .iter()
            .filter(|r| r.starts_with("POST") || r.starts_with("DELETE"))
            .map(String::as_str)
            .collect();
        // Stop newest first; start oldest first; the one-off and the running
        // container are never touched; volumes are kept.
        assert_eq!(
            acted,
            [
                format!("POST /v1.47/containers/{A}/stop?t=10"),
                format!("POST /v1.47/containers/{B}/stop?t=10"),
                format!("POST /v1.47/containers/{B}/start"),
                format!("POST /v1.47/containers/{A}/start"),
                format!("DELETE /v1.47/containers/{C}?v=0&force=0"),
            ]
        );
    }

    #[test]
    fn a_socket_this_account_cannot_open() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return; // root opens anything
        }
        let engine = fake(|_, _| version());
        std::fs::set_permissions(&engine.socket, std::fs::Permissions::from_mode(0o000)).unwrap();
        let c = containers(&engine);
        let error = c.handle(&json!({"op": "containers"})).unwrap();
        assert_eq!(
            error["error"],
            format!(
                "This account can't use Docker: permission denied on {}. Add it to the docker group, or use rootless Docker.",
                engine.socket.display()
            )
        );
    }
}
