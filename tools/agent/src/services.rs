//! systemd services and timers for the Services tab: inventory with live
//! figures, journal reads, unit files and lifecycle actions. Everything shells
//! out to systemctl/journalctl with argument vectors -- never a shell -- and
//! every child is bounded in time and output.
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    io::{self, Read},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

const MAX_UNITS: usize = 2000;
const SHOW_BATCH: usize = 200;
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const ACTION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_OUTPUT: usize = 16 << 20;
const MAX_UNIT_FILE: usize = 256 << 10;
const MAX_MESSAGE: usize = 16 << 10;
const DEFAULT_LINES: u64 = 200;
const MAX_LINES: u64 = 1000;
const MAX_TAIL: u64 = 500;

const SERVICE_PROPERTIES: &str = "Id,Description,LoadState,ActiveState,SubState,UnitFileState,MainPID,StateChangeTimestamp,ActiveEnterTimestamp,MemoryCurrent,CPUUsageNSec,NRestarts,Result,ExecMainStatus,ExecMainCode,FragmentPath,DropInPaths,TriggeredBy,User";
const TIMER_PROPERTIES: &str =
    "Id,Description,ActiveState,NextElapseUSecRealtime,LastTriggerUSec,Unit";

/// What a child process left behind.
#[derive(Debug, Default)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a program; tests substitute canned output.
pub trait Run: Send + Sync {
    fn run(
        &self,
        program: &str,
        args: &[String],
        env: &[(String, String)],
        timeout: Duration,
        cap: usize,
    ) -> io::Result<Output>;
}

/// The real thing: piped, capped, killed at the deadline.
pub struct System;
impl Run for System {
    fn run(
        &self,
        program: &str,
        args: &[String],
        env: &[(String, String)],
        timeout: Duration,
        cap: usize,
    ) -> io::Result<Output> {
        let mut child = Command::new(program)
            .args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let capped = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut kept = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = (&mut pipe).take(cap as u64).read_to_end(&mut kept);
                    // Drain the rest so a chatty child never blocks on a full pipe.
                    let _ = io::copy(&mut pipe, &mut io::sink());
                }
                String::from_utf8_lossy(&kept).into_owned()
            })
        };
        let stdout = capped(
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        );
        let stderr = capped(
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        );
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{program} did not finish within {} seconds",
                        timeout.as_secs()
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        Ok(Output {
            success: status.success(),
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    System,
    User,
}
impl Scope {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            Some("system") | None => Ok(Self::System),
            Some("user") => Ok(Self::User),
            Some(other) => Err(format!("Unknown service scope: {other}")),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
        }
    }
}

/// What system-scope actions run as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Privilege {
    Root,
    Sudo,
    Password,
    None,
}
impl Privilege {
    fn name(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Sudo => "sudo",
            Self::Password => "password",
            Self::None => "none",
        }
    }
}

/// A unit name that is safe as a single argument: systemd's own character set
/// (including `\` escapes), no leading dash, and the expected suffix.
pub fn valid_unit(name: &str, timers: bool) -> bool {
    let stem = name
        .strip_suffix(".service")
        .or_else(|| timers.then(|| name.strip_suffix(".timer")).flatten());
    stem.is_some_and(|stem| {
        !stem.is_empty()
            && stem.len() <= 255
            && !name.starts_with('-')
            && stem
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b":_.@-\\".contains(&b))
    })
}

/// `systemctl show` output: `Key=Value` lines, one blank line between units.
pub fn parse_show(text: &str) -> Vec<HashMap<String, String>> {
    let mut records = Vec::new();
    let mut current = HashMap::new();
    for line in text.lines() {
        if line.is_empty() {
            if !current.is_empty() {
                records.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            current.insert(key.to_owned(), value.to_owned());
        }
    }
    if !current.is_empty() {
        records.push(current);
    }
    records
}

/// Unit names from `systemctl list-units --plain --no-legend`, whose first
/// column may carry a failure marker.
pub fn parse_list(text: &str, limit: usize) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            line.split_whitespace()
                .find(|token| {
                    token
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric())
                })
                .map(str::to_owned)
        })
        .take(limit)
        .collect()
}

pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // Howard Hinnant's algorithm: days since 1970-01-01 in the Gregorian calendar.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// A systemd timestamp as epoch milliseconds. Children run with `TZ=UTC`, so
/// "Sun 2026-10-04 10:21:04 UTC" is the usual form; `@seconds` and numeric
/// offsets are accepted too. Empty, `n/a` and zero mean "never".
pub fn parse_timestamp(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Some(seconds) = value.strip_prefix('@') {
        return seconds
            .parse::<f64>()
            .ok()
            .filter(|s| *s > 0.0)
            .map(|s| (s * 1000.0) as i64);
    }
    let tokens: Vec<&str> = value.split_whitespace().collect();
    let date = tokens
        .iter()
        .position(|t| t.len() == 10 && t.as_bytes()[4] == b'-')?;
    let mut ymd = tokens[date].split('-').map(|p| p.parse::<i64>());
    let (year, month, day) = (ymd.next()?.ok()?, ymd.next()?.ok()?, ymd.next()?.ok()?);
    let mut hms = tokens.get(date + 1)?.split(':');
    let hour = hms.next()?.parse::<i64>().ok()?;
    let minute = hms.next()?.parse::<i64>().ok()?;
    let second = hms.next()?.parse::<f64>().ok()?;
    let offset = match tokens.get(date + 2) {
        Some(zone) if zone.starts_with('+') || zone.starts_with('-') => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let digits = zone[1..].replace(':', "");
            let hours = digits.get(..2)?.parse::<i64>().ok()?;
            let minutes = digits.get(2..4).unwrap_or("00").parse::<i64>().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
        _ => 0,
    };
    let seconds = days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 - offset;
    Some(seconds * 1000 + (second * 1000.0) as i64)
}

/// systemd prints "unset" numbers as `[not set]`, empty, or u64::MAX.
fn number(record: &HashMap<String, String>, key: &str) -> Option<u64> {
    record
        .get(key)?
        .parse::<u64>()
        .ok()
        .filter(|n| *n != u64::MAX)
}
fn text(record: &HashMap<String, String>, key: &str) -> Option<String> {
    record
        .get(key)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}
fn list(record: &HashMap<String, String>, key: &str) -> Vec<String> {
    record
        .get(key)
        .map(|v| v.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// One service record in the Services contract, before CPU is known.
pub fn service_json(record: &HashMap<String, String>, scope: Scope) -> Value {
    let code = number(record, "ExecMainCode").unwrap_or(0);
    let status = number(record, "ExecMainStatus");
    let result = text(record, "Result");
    // A clean exit, or a process that never ran, has no code worth showing.
    let exit_code = match (code, status) {
        (0, _) => None,
        (_, Some(0)) if result.as_deref() == Some("success") => None,
        (_, status) => status,
    };
    let since = text(record, "StateChangeTimestamp")
        .and_then(|t| parse_timestamp(&t))
        .or_else(|| text(record, "ActiveEnterTimestamp").and_then(|t| parse_timestamp(&t)));
    json!({
        "name": text(record, "Id").unwrap_or_default(),
        "scope": scope.name(),
        "description": text(record, "Description").unwrap_or_default(),
        "load": text(record, "LoadState").unwrap_or_default(),
        "active": text(record, "ActiveState").unwrap_or_default(),
        "sub": text(record, "SubState").unwrap_or_default(),
        "enabled": text(record, "UnitFileState"),
        "mainPid": number(record, "MainPID").filter(|p| *p > 0),
        "since": since,
        "memory": number(record, "MemoryCurrent"),
        "cpu": Value::Null,
        "restarts": number(record, "NRestarts"),
        "result": result,
        "exitCode": exit_code,
        "exitKind": match code { 1 => Some("exited"), 2 => Some("killed"), 3 => Some("dumped"), _ => None },
        "fragment": text(record, "FragmentPath"),
        "dropIns": list(record, "DropInPaths"),
        "triggeredBy": list(record, "TriggeredBy"),
        "user": text(record, "User"),
    })
}

pub fn timer_json(record: &HashMap<String, String>, scope: Scope) -> Value {
    json!({
        "name": text(record, "Id").unwrap_or_default(),
        "scope": scope.name(),
        "description": text(record, "Description").unwrap_or_default(),
        "active": text(record, "ActiveState").unwrap_or_default(),
        "next": text(record, "NextElapseUSecRealtime").and_then(|t| parse_timestamp(&t)),
        "last": text(record, "LastTriggerUSec").and_then(|t| parse_timestamp(&t)),
        "unit": text(record, "Unit"),
    })
}

/// Control characters would corrupt the log view; tabs are kept, newlines in
/// multi-line messages become spaces.
fn clean(message: &str) -> String {
    let mut out: String = message
        .chars()
        .filter_map(|c| match c {
            '\t' => Some('\t'),
            '\n' | '\r' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    if out.len() > MAX_MESSAGE {
        let mut end = MAX_MESSAGE;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push('…');
    }
    out
}

/// `journalctl -o json`: one object per line. MESSAGE is a byte array when it
/// is not valid UTF-8. Unparsable lines are skipped.
pub fn parse_journal(text: &str) -> (Vec<Value>, Option<String>) {
    let mut entries = Vec::new();
    let mut cursor = None;
    for line in text.lines() {
        let Ok(Value::Object(entry)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let field = |key: &str| entry.get(key).and_then(Value::as_str);
        let message = match entry.get("MESSAGE") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(bytes)) => String::from_utf8_lossy(
                &bytes
                    .iter()
                    .filter_map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
                    .collect::<Vec<_>>(),
            )
            .into_owned(),
            _ => String::new(),
        };
        if let Some(c) = field("__CURSOR") {
            cursor = Some(c.to_owned());
        }
        entries.push(json!({
            "at": field("__REALTIME_TIMESTAMP").and_then(|t| t.parse::<i64>().ok()).map(|us| us / 1000),
            "priority": field("PRIORITY").and_then(|p| p.parse::<u8>().ok()).filter(|p| *p <= 7),
            "pid": field("_PID").and_then(|p| p.parse::<u32>().ok()),
            "identifier": field("SYSLOG_IDENTIFIER").or_else(|| field("_COMM")),
            "message": clean(&message),
        }));
    }
    (entries, cursor)
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// A command the user can paste into a terminal: quoted only when needed.
fn shell_word(word: &str) -> String {
    if word
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"._@:-".contains(&b))
    {
        word.to_owned()
    } else {
        format!("'{word}'")
    }
}

pub struct Services {
    runner: Box<dyn Run>,
    euid: u32,
    env: Vec<(String, String)>,
    cpu: Mutex<HashMap<(Scope, String), (u64, Instant)>>,
    // Probed once: a sudo check on every refresh would fill the auth log.
    privilege: Mutex<Option<Privilege>>,
    system_journal: fn() -> bool,
}

/// Whether this account may open the system journal. journalctl quietly
/// returns nothing once the account has a journal file of its own.
fn system_journal_readable() -> bool {
    let mut found = false;
    for root in ["/var/log/journal", "/run/log/journal"] {
        for dir in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let file = dir.path().join("system.journal");
            if file.exists() {
                found = true;
                if std::fs::File::open(file).is_ok() {
                    return true;
                }
            }
        }
    }
    // No journal files at all is journalctl's own error to report.
    !found
}

impl Services {
    pub fn system() -> Self {
        // SAFETY: geteuid has no preconditions and cannot fail.
        let euid = unsafe { libc::geteuid() };
        let mut env = vec![("LC_ALL".into(), "C".into()), ("TZ".into(), "UTC".into())];
        // The user manager is reached through the session's runtime directory,
        // which a non-login SSH command may not have exported.
        let runtime = std::env::var("XDG_RUNTIME_DIR")
            .ok()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| format!("/run/user/{euid}"));
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() && std::path::Path::new(&runtime).is_dir()
        {
            env.push(("XDG_RUNTIME_DIR".into(), runtime.clone()));
        }
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
            && std::path::Path::new(&runtime).join("bus").exists()
        {
            env.push((
                "DBUS_SESSION_BUS_ADDRESS".into(),
                format!("unix:path={runtime}/bus"),
            ));
        }
        Self {
            system_journal: system_journal_readable,
            ..Self::new(Box::new(System), euid, env)
        }
    }
    pub fn new(runner: Box<dyn Run>, euid: u32, env: Vec<(String, String)>) -> Self {
        Self {
            runner,
            euid,
            env,
            cpu: Mutex::new(HashMap::new()),
            privilege: Mutex::new(None),
            system_journal: || true,
        }
    }

    fn run(
        &self,
        program: &str,
        args: Vec<String>,
        timeout: Duration,
        cap: usize,
    ) -> Result<Output, String> {
        self.runner
            .run(program, &args, &self.env, timeout, cap)
            .map_err(|e| format!("Cannot run {program}: {e}"))
    }
    fn systemctl(
        &self,
        scope: Scope,
        args: &[&str],
        timeout: Duration,
        cap: usize,
    ) -> Result<Output, String> {
        let mut all: Vec<String> = Vec::new();
        if scope == Scope::User {
            all.push("--user".into());
        }
        all.extend(["--no-pager".into()]);
        all.extend(args.iter().map(|a| (*a).to_owned()));
        self.run("systemctl", all, timeout, cap)
    }

    pub fn privilege(&self) -> Privilege {
        let mut cached = self.privilege.lock().unwrap();
        if let Some(known) = *cached {
            return known;
        }
        let found = if self.euid == 0 {
            Privilege::Root
        } else {
            // Exactly what actions need: systemctl through non-interactive sudo.
            match self.runner.run(
                "sudo",
                &["-n".into(), "systemctl".into(), "--version".into()],
                &self.env,
                Duration::from_secs(5),
                64 << 10,
            ) {
                Ok(out) if out.success => Privilege::Sudo,
                Ok(out) if out.stderr.contains("password") => Privilege::Password,
                _ => Privilege::None,
            }
        };
        *cached = Some(found);
        found
    }

    /// Properties for many units, a batch at a time to keep argv modest.
    fn show(
        &self,
        scope: Scope,
        names: &[String],
        properties: &str,
    ) -> Result<Vec<HashMap<String, String>>, String> {
        let mut records = Vec::new();
        for batch in names.chunks(SHOW_BATCH) {
            let mut args = vec!["show".to_owned(), format!("--property={properties}")];
            args.extend(batch.iter().cloned());
            let mut all = vec![];
            if scope == Scope::User {
                all.push("--user".to_owned());
            }
            all.push("--no-pager".into());
            all.extend(args);
            let out = self.run("systemctl", all, LIST_TIMEOUT, MAX_OUTPUT)?;
            if !out.success {
                return Err(first_line(&out.stderr));
            }
            records.extend(parse_show(&out.stdout));
        }
        Ok(records)
    }
    fn units(&self, scope: Scope, kind: &str) -> Result<Vec<String>, String> {
        let out = self.systemctl(
            scope,
            &[
                "list-units",
                &format!("--type={kind}"),
                "--all",
                "--plain",
                "--no-legend",
                "--full",
            ],
            LIST_TIMEOUT,
            MAX_OUTPUT,
        )?;
        if !out.success {
            let reason = first_line(&out.stderr);
            return Err(if reason.is_empty() {
                "systemctl could not list units".into()
            } else {
                reason
            });
        }
        Ok(parse_list(&out.stdout, MAX_UNITS))
    }

    pub fn snapshot(&self) -> Value {
        let mut services = Vec::new();
        let mut timers = Vec::new();
        let mut user = json!({"available": true, "reason": null});
        let now = Instant::now();
        let mut seen = HashMap::new();
        for scope in [Scope::System, Scope::User] {
            let read = (|| -> Result<(), String> {
                let names = self.units(scope, "service")?;
                for record in self.show(scope, &names, SERVICE_PROPERTIES)? {
                    let mut service = service_json(&record, scope);
                    let name = service["name"].as_str().unwrap_or_default().to_owned();
                    if name.is_empty() || services.len() >= MAX_UNITS {
                        continue;
                    }
                    if let Some(ns) = number(&record, "CPUUsageNSec") {
                        seen.insert((scope, name), (ns, now));
                    }
                    let key = (
                        scope,
                        service["name"].as_str().unwrap_or_default().to_owned(),
                    );
                    if let (Some((ns, _)), Some((before, at))) =
                        (seen.get(&key), self.cpu.lock().unwrap().get(&key))
                    {
                        let elapsed = now.duration_since(*at).as_nanos() as f64;
                        if *ns >= *before && elapsed > 0.0 {
                            let share = (*ns - *before) as f64 / elapsed * 100.0;
                            service["cpu"] = json!((share * 10.0).round() / 10.0);
                        }
                    }
                    services.push(service);
                }
                let timer_names = self.units(scope, "timer")?;
                for record in self.show(scope, &timer_names, TIMER_PROPERTIES)? {
                    timers.push(timer_json(&record, scope));
                }
                Ok(())
            })();
            if let Err(reason) = read {
                match scope {
                    Scope::System => {
                        return json!({"error": format!("Cannot read system services: {reason}")})
                    }
                    Scope::User => user = json!({"available": false, "reason": reason}),
                }
            }
        }
        // Units that went away drop their baseline.
        *self.cpu.lock().unwrap() = seen;
        json!({
            "services": services,
            "timers": timers,
            "user": user,
            "privilege": self.privilege().name(),
        })
    }

    pub fn logs(&self, request: &Value) -> Result<Value, String> {
        let scope = Scope::parse(request["scope"].as_str())?;
        let unit = request["unit"].as_str().filter(|u| !u.is_empty());
        if let Some(unit) = unit {
            if !valid_unit(unit, true) {
                return Err(format!("Invalid unit name: {unit}"));
            }
        }
        let priority = match request["priority"].as_str() {
            None => None,
            Some(level @ ("err" | "warning")) => Some(level),
            Some(other) => return Err(format!("Unknown log priority: {other}")),
        };
        let cursor = request["cursor"].as_str().filter(|c| !c.is_empty());
        if cursor.is_some_and(|c| c.len() > 512 || c.chars().any(char::is_control)) {
            return Err("Invalid journal cursor.".into());
        }
        let lines = request["lines"]
            .as_u64()
            .unwrap_or(DEFAULT_LINES)
            .clamp(1, MAX_LINES);
        let read = |cursor: Option<&str>| -> Result<Output, String> {
            let mut args: Vec<String> = vec!["-o".into(), "json".into(), "--no-pager".into()];
            match (scope, unit) {
                (Scope::System, Some(unit)) => args.extend(["-u".into(), unit.into()]),
                // A user unit's entries may sit in the user or the system journal.
                (Scope::User, Some(unit)) => args.push(format!("--user-unit={unit}")),
                (Scope::User, None) => args.push("--user".into()),
                (Scope::System, None) => {}
            }
            if let Some(level) = priority {
                args.extend(["-p".into(), level.into()]);
            }
            match cursor {
                Some(cursor) => {
                    args.push(format!("--after-cursor={cursor}"));
                    args.extend(["-n".into(), MAX_TAIL.to_string()]);
                }
                None => args.extend(["-n".into(), lines.to_string()]),
            }
            self.run("journalctl", args, LIST_TIMEOUT, MAX_OUTPUT)
        };
        let mut out = read(cursor)?;
        // A cursor from a rotated-away journal cannot be resumed; start over.
        if !out.success && cursor.is_some() {
            out = read(None)?;
        }
        let (entries, last) = parse_journal(&out.stdout);
        // journalctl exits 1 when it could open no journal file at all.
        if entries.is_empty() && cursor.is_none() && out.stderr.contains("insufficient permissions")
        {
            return Err(format!(
                "This account cannot read that journal: {}",
                first_line(&out.stderr)
            ));
        }
        if entries.is_empty()
            && cursor.is_none()
            && scope == Scope::System
            && self.euid != 0
            && !(self.system_journal)()
        {
            return Err("This account cannot read that journal: the system journal needs root or the systemd-journal or adm group.".into());
        }
        if !out.success {
            return Err(first_line(&out.stderr));
        }
        Ok(json!({"entries": entries, "cursor": last.or_else(|| cursor.map(str::to_owned))}))
    }

    pub fn unit_file(&self, request: &Value) -> Result<Value, String> {
        let scope = Scope::parse(request["scope"].as_str())?;
        let unit = request["unit"].as_str().unwrap_or_default();
        if !valid_unit(unit, true) {
            return Err(format!("Invalid unit name: {unit}"));
        }
        let out = self.systemctl(scope, &["cat", unit], LIST_TIMEOUT, MAX_UNIT_FILE)?;
        if !out.success {
            return Err(first_line(&out.stderr));
        }
        Ok(json!({"text": out.stdout}))
    }

    pub fn action(&self, request: &Value) -> Result<Value, String> {
        let scope = Scope::parse(request["scope"].as_str())?;
        let unit = request["unit"].as_str().unwrap_or_default();
        if !valid_unit(unit, false) {
            return Err(format!("Invalid unit name: {unit}"));
        }
        let verb = match request["verb"].as_str() {
            Some(v @ ("start" | "stop" | "restart" | "reload" | "enable" | "disable")) => v,
            Some(other) => return Err(format!("Unknown service action: {other}")),
            None => return Err("Service actions need a verb.".into()),
        };
        let needs_password = || json!({"ok": false, "needsPassword": true, "command": format!("sudo systemctl {verb} {}", shell_word(unit))});
        let privilege = match scope {
            Scope::User => None,
            Scope::System => Some(self.privilege()),
        };
        let out = match privilege {
            None | Some(Privilege::Root) => {
                self.systemctl(scope, &[verb, unit], ACTION_TIMEOUT, 64 << 10)?
            }
            Some(Privilege::Sudo) => self.run(
                "sudo",
                vec![
                    "-n".into(),
                    "systemctl".into(),
                    "--no-pager".into(),
                    verb.into(),
                    unit.into(),
                ],
                ACTION_TIMEOUT,
                64 << 10,
            )?,
            Some(Privilege::Password) => return Ok(needs_password()),
            Some(Privilege::None) => {
                return Err("Controlling system services needs root or sudo on this server.".into())
            }
        };
        if out.success {
            return Ok(json!({"ok": true}));
        }
        if out.stderr.contains("password is required") {
            *self.privilege.lock().unwrap() = Some(Privilege::Password);
            return Ok(needs_password());
        }
        let reason = first_line(&out.stderr);
        Err(if reason.is_empty() {
            format!("systemctl {verb} {unit} failed.")
        } else {
            reason
        })
    }

    /// Answers a Services request, or None when the op is not one of ours.
    pub fn handle(&self, request: &Value) -> Option<Value> {
        let result = match request.get("op").and_then(Value::as_str)? {
            "services" => return Some(self.snapshot()),
            "logs" => self.logs(request),
            "unit" => self.unit_file(request),
            "action" => self.action(request),
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
    use std::sync::Arc;

    /// Answers by program and first matching argument; records every call.
    #[derive(Default)]
    struct Fake {
        answers: Vec<(&'static str, &'static str, Output)>,
        calls: Arc<Mutex<Vec<String>>>,
    }
    impl Fake {
        fn on(
            mut self,
            program: &'static str,
            contains: &'static str,
            success: bool,
            stdout: &str,
            stderr: &str,
        ) -> Self {
            self.answers.push((
                program,
                contains,
                Output {
                    success,
                    stdout: stdout.into(),
                    stderr: stderr.into(),
                },
            ));
            self
        }
    }
    impl Run for Fake {
        fn run(
            &self,
            program: &str,
            args: &[String],
            _: &[(String, String)],
            _: Duration,
            _: usize,
        ) -> io::Result<Output> {
            let line = format!("{program} {}", args.join(" "));
            self.calls.lock().unwrap().push(line.clone());
            self.answers
                .iter()
                .find(|(p, needle, _)| *p == program && line.contains(needle))
                .map(|(_, _, out)| Output {
                    success: out.success,
                    stdout: out.stdout.clone(),
                    stderr: out.stderr.clone(),
                })
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, format!("no fake for {line}"))
                })
        }
    }
    fn services(fake: Fake, euid: u32) -> (Services, Arc<Mutex<Vec<String>>>) {
        let calls = fake.calls.clone();
        (Services::new(Box::new(fake), euid, Vec::new()), calls)
    }

    const SHOW: &str = "Id=atlas-api.service\nDescription=Atlas API\nLoadState=loaded\nActiveState=failed\nSubState=failed\nUnitFileState=enabled\nMainPID=0\nStateChangeTimestamp=Sun 2026-10-04 10:21:05 UTC\nActiveEnterTimestamp=\nMemoryCurrent=[not set]\nCPUUsageNSec=[not set]\nNRestarts=14\nResult=exit-code\nExecMainStatus=3\nExecMainCode=1\nFragmentPath=/etc/systemd/system/atlas-api.service\nDropInPaths=/etc/systemd/system/atlas-api.service.d/env.conf /run/x.conf\nTriggeredBy=\nUser=atlas\n\nId=nginx.service\nDescription=A high performance web server\nLoadState=loaded\nActiveState=active\nSubState=running\nUnitFileState=enabled\nMainPID=812\nStateChangeTimestamp=\nActiveEnterTimestamp=Fri 2026-10-02 06:00:00 UTC\nMemoryCurrent=43122688\nCPUUsageNSec=2000000000\nNRestarts=0\nResult=success\nExecMainStatus=0\nExecMainCode=0\nFragmentPath=/lib/systemd/system/nginx.service\nDropInPaths=\nTriggeredBy=\nUser=\n";

    #[test]
    fn unit_names_are_systemd_characters_with_the_right_suffix() {
        for good in [
            "nginx.service",
            "postgresql@16-main.service",
            "dev-sda1\\x2d.service",
            "a:b_c.service",
        ] {
            assert!(valid_unit(good, false), "{good}");
        }
        assert!(valid_unit("certbot.timer", true));
        for bad in [
            "certbot.timer",
            "-x.service",
            ".service",
            "a b.service",
            "x;rm.service",
            "x.socket",
            "x.service\n",
            "$(id).service",
        ] {
            assert!(!valid_unit(bad, false), "{bad:?}");
        }
        assert!(!valid_unit(&format!("{}.service", "a".repeat(256)), false));
    }

    #[test]
    fn timestamps_in_every_form_systemd_prints() {
        assert_eq!(
            parse_timestamp("Sun 2026-10-04 10:21:05 UTC"),
            Some(1_791_109_265_000)
        );
        assert_eq!(
            parse_timestamp("2026-10-04 10:21:05 +0200"),
            Some(1_791_102_065_000)
        );
        assert_eq!(parse_timestamp("@1791109265"), Some(1_791_109_265_000));
        assert_eq!(parse_timestamp("Thu 1970-01-01 00:00:00 UTC"), Some(0));
        for never in ["", "n/a", "@0", "0"] {
            assert_eq!(parse_timestamp(never), None, "{never}");
        }
    }

    #[test]
    fn show_records_become_services() {
        let records = parse_show(SHOW);
        assert_eq!(records.len(), 2);
        let failed = service_json(&records[0], Scope::System);
        assert_eq!(failed["active"], "failed");
        assert_eq!(failed["exitCode"], 3);
        assert_eq!(failed["exitKind"], "exited");
        assert_eq!(failed["restarts"], 14);
        assert_eq!(failed["mainPid"], Value::Null);
        assert_eq!(failed["memory"], Value::Null);
        assert_eq!(failed["since"], 1_791_109_265_000i64);
        assert_eq!(
            failed["dropIns"],
            json!([
                "/etc/systemd/system/atlas-api.service.d/env.conf",
                "/run/x.conf"
            ])
        );
        assert_eq!(failed["user"], "atlas");
        let running = service_json(&records[1], Scope::User);
        assert_eq!(running["scope"], "user");
        assert_eq!(running["mainPid"], 812);
        assert_eq!(running["memory"], 43_122_688);
        assert_eq!(running["exitCode"], Value::Null);
        assert_eq!(running["exitKind"], Value::Null);
        assert_eq!(running["since"], 1_790_920_800_000i64);
        assert_eq!(running["user"], Value::Null);
        assert_eq!(running["triggeredBy"], json!([]));
        // An unlimited counter is as good as unset.
        let mut infinite = records[1].clone();
        infinite.insert("MemoryCurrent".into(), u64::MAX.to_string());
        assert_eq!(
            service_json(&infinite, Scope::System)["memory"],
            Value::Null
        );
    }

    #[test]
    fn list_units_lines_lose_their_failure_marker() {
        let text = "● atlas-api.service loaded failed failed Atlas API\nnginx.service loaded active running A high performance web server\n\n";
        assert_eq!(
            parse_list(text, 10),
            vec!["atlas-api.service", "nginx.service"]
        );
        assert_eq!(parse_list(text, 1), vec!["atlas-api.service"]);
    }

    #[test]
    fn journal_entries_decode_bytes_strip_controls_and_keep_the_cursor() {
        let text = concat!(
            r#"{"__CURSOR":"s=1;i=1","__REALTIME_TIMESTAMP":"1791109264000123","PRIORITY":"6","_PID":"902","SYSLOG_IDENTIFIER":"atlas","MESSAGE":"listening\ton :8080"}"#,
            "\n",
            "not json\n",
            r#"{"__CURSOR":"s=1;i=2","__REALTIME_TIMESTAMP":"1791109265000000","PRIORITY":"3","_COMM":"atlas","MESSAGE":[100,98,32,255,10,27,91,51,49,109,100,111,119,110]}"#,
            "\n",
        );
        let (entries, cursor) = parse_journal(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["at"], 1_791_109_264_000i64);
        assert_eq!(entries[0]["priority"], 6);
        assert_eq!(entries[0]["pid"], 902);
        assert_eq!(entries[0]["message"], "listening\ton :8080");
        assert_eq!(entries[1]["identifier"], "atlas");
        assert_eq!(entries[1]["pid"], Value::Null);
        assert_eq!(entries[1]["message"], "db \u{fffd} [31mdown");
        assert_eq!(cursor.as_deref(), Some("s=1;i=2"));
        assert_eq!(
            clean(&"x".repeat(MAX_MESSAGE + 10)).chars().count(),
            MAX_MESSAGE + 1
        );
    }

    #[test]
    fn snapshot_reads_both_scopes_and_reports_cpu_from_the_previous_request() {
        let (services, calls) = services(
            Fake::default()
                .on("systemctl", "--user --no-pager list-units", false, "", "Failed to connect to bus: No medium found\n")
                .on("systemctl", "list-units --type=service", true, "● atlas-api.service loaded failed failed Atlas API\nnginx.service loaded active running nginx\n", "")
                .on("systemctl", "list-units --type=timer", true, "certbot.timer loaded active waiting Run certbot twice daily\n", "")
                .on("systemctl", "show --property=Id,Description,ActiveState,NextElapseUSecRealtime", true, "Id=certbot.timer\nDescription=Run certbot twice daily\nActiveState=active\nNextElapseUSecRealtime=Sun 2026-10-04 12:00:00 UTC\nLastTriggerUSec=n/a\nUnit=certbot.service\n", "")
                .on("systemctl", "show --property=Id,Description,LoadState", true, SHOW, "")
                .on("sudo", "-n systemctl --version", false, "", "sudo: a password is required\n"),
            1000,
        );
        let first = services.snapshot();
        assert_eq!(first["services"].as_array().unwrap().len(), 2);
        assert_eq!(first["services"][1]["cpu"], Value::Null, "no baseline yet");
        assert_eq!(first["timers"][0]["unit"], "certbot.service");
        assert_eq!(first["timers"][0]["next"], 1_791_115_200_000i64);
        assert_eq!(first["timers"][0]["last"], Value::Null);
        assert_eq!(
            first["user"],
            json!({"available": false, "reason": "Failed to connect to bus: No medium found"})
        );
        assert_eq!(first["privilege"], "password");
        // Pretend nginx used a full core for the elapsed time.
        services.cpu.lock().unwrap().insert(
            (Scope::System, "nginx.service".into()),
            (1_000_000_000, Instant::now() - Duration::from_secs(1)),
        );
        let second = services.snapshot();
        let cpu = second["services"][1]["cpu"].as_f64().unwrap();
        assert!((90.0..=110.0).contains(&cpu), "{cpu}");
        // The sudo probe ran once, not on every refresh.
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.starts_with("sudo"))
                .count(),
            1
        );
    }

    #[test]
    fn system_failures_are_errors_and_root_needs_no_probe() {
        let (services, calls) = services(
            Fake::default().on(
                "systemctl",
                "list-units",
                false,
                "",
                "System has not been booted with systemd as init system (PID 1).\n",
            ),
            0,
        );
        assert_eq!(
            services.snapshot()["error"],
            "Cannot read system services: System has not been booted with systemd as init system (PID 1)."
        );
        assert_eq!(services.privilege(), Privilege::Root);
        assert!(calls.lock().unwrap().iter().all(|c| !c.starts_with("sudo")));
    }

    #[test]
    fn logs_build_journalctl_arguments_and_resume_after_rotation() {
        let line = r#"{"__CURSOR":"c2","__REALTIME_TIMESTAMP":"1000","MESSAGE":"hi"}"#;
        let (services, calls) = services(
            Fake::default()
                .on(
                    "journalctl",
                    "--after-cursor=gone",
                    false,
                    "",
                    "Failed to seek to cursor: Invalid argument\n",
                )
                .on("journalctl", "-o json", true, line, ""),
            1000,
        );
        let reply = services
            .logs(&json!({"unit": "nginx.service", "scope": "system", "priority": "err", "lines": 50}))
            .unwrap();
        assert_eq!(reply["cursor"], "c2");
        assert!(calls.lock().unwrap()[0].ends_with("-u nginx.service -p err -n 50"));
        services
            .logs(&json!({"unit": "app.service", "scope": "user", "cursor": "c2"}))
            .unwrap();
        assert!(
            calls.lock().unwrap()[1].ends_with("--user-unit=app.service --after-cursor=c2 -n 500")
        );
        services.logs(&json!({"scope": "user"})).unwrap();
        assert!(calls.lock().unwrap()[2].contains("--no-pager --user -n 200"));
        // A rotated-away cursor starts over from the latest lines.
        let reply = services
            .logs(&json!({"scope": "system", "cursor": "gone"}))
            .unwrap();
        assert_eq!(reply["entries"].as_array().unwrap().len(), 1);
        assert!(services.logs(&json!({"unit": "x;y.service"})).is_err());
        assert!(services.logs(&json!({"priority": "debug"})).is_err());
        assert!(services.logs(&json!({"cursor": "a\nb"})).is_err());
        // An empty tail keeps the caller's cursor.
        let (quiet, _) = super::tests::services(
            Fake::default().on("journalctl", "-o json", true, "", ""),
            1000,
        );
        assert_eq!(
            quiet.logs(&json!({"cursor": "keep"})).unwrap()["cursor"],
            "keep"
        );
        // With a journal of its own, journalctl shows an outsider nothing
        // rather than an error; nothing plus an unreadable system journal is one.
        let (mut outsider, _) = super::tests::services(
            Fake::default().on("journalctl", "-o json", true, "", ""),
            1000,
        );
        outsider.system_journal = || false;
        let error = outsider
            .logs(&json!({"unit": "nginx.service", "scope": "system"}))
            .unwrap_err();
        assert!(error.contains("systemd-journal"), "{error}");
        assert!(outsider.logs(&json!({"scope": "user"})).is_ok());
        assert!(outsider.logs(&json!({"cursor": "keep"})).is_ok());
    }

    #[test]
    fn actions_pick_the_right_privilege_and_report_password_needs() {
        let (root, calls) = services(
            Fake::default().on("systemctl", "restart nginx.service", true, "", ""),
            0,
        );
        assert_eq!(
            root.action(&json!({"unit": "nginx.service", "verb": "restart"}))
                .unwrap(),
            json!({"ok": true})
        );
        assert!(calls.lock().unwrap()[0].starts_with("systemctl --no-pager restart nginx.service"));

        let (sudo, calls) = services(
            Fake::default()
                .on("sudo", "--version", true, "systemd 257", "")
                .on("sudo", "stop nginx.service", true, "", ""),
            1000,
        );
        assert_eq!(
            sudo.action(&json!({"unit": "nginx.service", "verb": "stop", "scope": "system"}))
                .unwrap()["ok"],
            true
        );
        assert!(
            calls.lock().unwrap()[1].starts_with("sudo -n systemctl --no-pager stop nginx.service")
        );

        let (password, _) = services(
            Fake::default().on(
                "sudo",
                "--version",
                false,
                "",
                "sudo: a password is required\n",
            ),
            1000,
        );
        assert_eq!(
            password
                .action(&json!({"unit": "dev-sda1\\x2d.service", "verb": "enable"}))
                .unwrap(),
            json!({"ok": false, "needsPassword": true, "command": "sudo systemctl enable 'dev-sda1\\x2d.service'"})
        );

        let (user, calls) = services(
            Fake::default().on("systemctl", "--user", true, "", ""),
            1000,
        );
        assert_eq!(
            user.action(&json!({"unit": "app.service", "verb": "start", "scope": "user"}))
                .unwrap()["ok"],
            true
        );
        assert!(
            calls.lock().unwrap().iter().all(|c| !c.starts_with("sudo")),
            "user scope never uses sudo"
        );

        let (failing, _) = services(
            Fake::default().on(
                "systemctl",
                "start",
                false,
                "",
                "Failed to start ghost.service: Unit ghost.service not found.\n",
            ),
            0,
        );
        assert_eq!(
            failing
                .action(&json!({"unit": "ghost.service", "verb": "start"}))
                .unwrap_err(),
            "Failed to start ghost.service: Unit ghost.service not found."
        );
        for bad in [
            json!({"unit": "x.service", "verb": "mask"}),
            json!({"unit": "x.timer", "verb": "start"}),
            json!({"unit": "x.service"}),
        ] {
            assert!(failing.action(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn unit_files_and_dispatch() {
        let (services, _) = services(
            Fake::default().on(
                "systemctl",
                "cat certbot.timer",
                true,
                "# /lib/systemd/system/certbot.timer\n[Timer]\n",
                "",
            ),
            0,
        );
        assert_eq!(
            services
                .handle(&json!({"op": "unit", "unit": "certbot.timer"}))
                .unwrap()["text"],
            "# /lib/systemd/system/certbot.timer\n[Timer]\n"
        );
        assert!(services
            .handle(&json!({"op": "unit", "unit": "bad name.service"}))
            .unwrap()["error"]
            .is_string());
        assert!(
            services.handle(&json!({"op": "snapshot"})).is_none(),
            "not ours"
        );
    }
}
