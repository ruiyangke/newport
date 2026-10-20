//! Systemd service contracts and domain rules, with no UI, transport, or
//! runtime dependencies.
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    System,
    User,
}
impl Scope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
        }
    }
}
impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Privilege {
    Root,
    Sudo,
    Password,
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceVerb {
    Start,
    Stop,
    Restart,
    Reload,
    Enable,
    Disable,
}
impl ServiceVerb {
    pub const ALL: [Self; 6] = [
        Self::Start,
        Self::Stop,
        Self::Restart,
        Self::Reload,
        Self::Enable,
        Self::Disable,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Reload => "reload",
            Self::Enable => "enable",
            Self::Disable => "disable",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Restart => "Restart",
            Self::Reload => "Reload",
            Self::Enable => "Enable",
            Self::Disable => "Disable",
        }
    }
}
impl fmt::Display for ServiceVerb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceUnit {
    pub name: String,
    pub scope: Scope,
    pub description: String,
    pub load: String,
    pub active: String,
    pub sub: String,
    pub enabled: Option<String>,
    pub main_pid: Option<u32>,
    /// Epoch milliseconds of the last state change.
    pub since: Option<i64>,
    pub memory: Option<u64>,
    /// Percent of one core since the agent's previous reading.
    pub cpu: Option<f64>,
    pub restarts: Option<u64>,
    pub result: Option<String>,
    pub exit_code: Option<i64>,
    pub exit_kind: Option<String>,
    pub fragment: Option<String>,
    pub drop_ins: Vec<String>,
    pub triggered_by: Vec<String>,
    pub user: Option<String>,
}
impl ServiceUnit {
    pub fn key(&self) -> String {
        unit_key(self.scope, &self.name)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerUnit {
    pub name: String,
    pub scope: Scope,
    pub description: String,
    pub active: String,
    pub next: Option<i64>,
    pub last: Option<i64>,
    pub unit: Option<String>,
}
impl TimerUnit {
    pub fn key(&self) -> String {
        unit_key(self.scope, &self.name)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserAvailability {
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicesSnapshot {
    pub services: Vec<ServiceUnit>,
    pub timers: Vec<TimerUnit>,
    pub user: UserAvailability,
    pub privilege: Privilege,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub at: Option<i64>,
    pub priority: Option<u8>,
    pub pid: Option<u32>,
    pub identifier: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalPage {
    pub entries: Vec<JournalEntry>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitFile {
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_password: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

pub fn unit_key(scope: Scope, name: &str) -> String {
    format!("{scope}:{name}")
}
pub fn short_name(name: &str) -> &str {
    name.strip_suffix(".service")
        .or_else(|| name.strip_suffix(".timer"))
        .unwrap_or(name)
}

/// The backend and agent accept systemd unit names, never arbitrary shell
/// arguments. Timers are allowed for inspection, not service mutations.
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

pub fn is_problem(unit: &ServiceUnit) -> bool {
    unit.active == "failed"
        || unit.sub == "auto-restart"
        || (unit.active == "activating" && unit.restarts.unwrap_or(0) > 0)
}

// Prefixes deliberately match name prefixes, not only complete service names. User units always remain the user's services.
const PLUMBING: &[&str] = &[
    "systemd-",
    "getty@",
    "serial-getty@",
    "user@",
    "user-runtime-dir@",
    "dbus",
    "polkit",
    "udev",
    "snapd",
    "apt-",
    "unattended-upgrades",
    "networkd-dispatcher",
    "ModemManager",
    "NetworkManager",
    "accounts-daemon",
    "cloud-",
    "multipathd",
    "apparmor",
    "ufw",
    "irqbalance",
    "thermald",
    "fwupd",
    "packagekit",
    "plymouth",
    "e2scrub",
    "fstrim",
    "logrotate",
    "man-db",
    "motd-news",
    "rsyslog",
    "cron",
    "atd",
    "kmod",
    "keyboard-setup",
    "console-setup",
    "setvtrgb",
    "rc-local",
    "ifup@",
    "networking",
    "open-iscsi",
    "iscsid",
    "lvm2",
    "mdmonitor",
    "blk-availability",
    "finalrd",
    "grub",
    "secureboot",
    "ua-",
    "ubuntu-",
    "emergency",
    "rescue",
    "sysstat",
    "dmesg",
    "upower",
    "udisks2",
    "avahi",
    "bluetooth",
    "cups",
    "wpa_supplicant",
    "rtkit",
    "chrony",
    "ntp",
    "ssh",
    "sshd",
    "haveged",
    "qemu-guest-agent",
    "open-vm-tools",
    "vgauth",
    "lxd-agent",
    "google-",
    "walinuxagent",
    "hv-",
    "serial-",
    "modprobe@",
    "dpkg-",
    "pollinate",
    "smartd",
    "lm-sensors",
    "kerneloops",
    "whoopsie",
    "apport",
    "systemd",
];
pub fn is_owned(unit: &ServiceUnit) -> bool {
    if unit.scope == Scope::User {
        return true;
    }
    if PLUMBING.iter().any(|prefix| unit.name.starts_with(prefix)) {
        return false;
    }
    if unit
        .fragment
        .as_deref()
        .filter(|path| !path.is_empty())
        .is_some_and(|path| {
            !path.starts_with("/lib/systemd/") && !path.starts_with("/usr/lib/systemd/")
        })
    {
        return true;
    }
    matches!(unit.active.as_str(), "active" | "reloading")
        || unit.enabled.as_deref() == Some("enabled")
}

#[derive(Debug, Default)]
pub struct ServiceGroups<'a> {
    pub problems: Vec<&'a ServiceUnit>,
    pub owned: Vec<&'a ServiceUnit>,
    pub system: Vec<&'a ServiceUnit>,
}

/// Stable, case-insensitive name order; scoped identities remain separate.
/// Browser localeCompare is locale-dependent. The core uses deterministic
/// Unicode lowercase ordering so every client receives the same grouping.
pub fn group_services(services: &[ServiceUnit]) -> ServiceGroups<'_> {
    let mut sorted: Vec<_> = services.iter().collect();
    sorted.sort_by_cached_key(|unit| (short_name(&unit.name).to_lowercase(), unit.scope));
    let mut groups = ServiceGroups::default();
    for unit in sorted {
        if is_problem(unit) {
            groups.problems.push(unit);
        } else if is_owned(unit) {
            groups.owned.push(unit);
        } else {
            groups.system.push(unit);
        }
    }
    groups
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Success,
    Warning,
    Danger,
    Muted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceState {
    pub label: &'static str,
    pub tone: Tone,
}

pub fn state_of(unit: &ServiceUnit) -> ServiceState {
    let (label, tone) = if unit.active == "failed" {
        ("Failed", Tone::Danger)
    } else if unit.sub == "auto-restart" {
        ("Restarting", Tone::Warning)
    } else {
        match unit.active.as_str() {
            "activating" => ("Starting", Tone::Warning),
            "deactivating" => ("Stopping", Tone::Warning),
            "reloading" => ("Reloading", Tone::Warning),
            "active" => match unit.sub.as_str() {
                "exited" => ("Completed", Tone::Muted),
                "waiting" => ("Waiting", Tone::Muted),
                _ => ("Running", Tone::Success),
            },
            _ => ("Stopped", Tone::Muted),
        }
    };
    ServiceState { label, tone }
}

pub fn failure_reason(unit: &ServiceUnit) -> Option<String> {
    if unit.active != "failed" && unit.sub != "auto-restart" {
        return None;
    }
    let code = if matches!(unit.exit_kind.as_deref(), Some("killed" | "dumped")) {
        Some(format!(
            "killed by signal {}",
            unit.exit_code
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into())
        ))
    } else {
        unit.exit_code
            .filter(|code| *code != 0)
            .map(|code| format!("exit code {code}"))
    };
    match unit.result.as_deref() {
        Some("timeout") => Some("Timed out while starting or stopping".into()),
        Some("start-limit-hit") => Some("Restarted too often; systemd gave up".into()),
        Some("core-dump") => Some(format!(
            "Crashed with a core dump{}",
            code.map(|code| format!(" ({code})")).unwrap_or_default()
        )),
        Some("oom-kill") => Some("Killed for running out of memory".into()),
        Some("watchdog") => Some("Stopped answering its watchdog".into()),
        Some("signal") => Some(
            code.map(|code| format!("Stopped: {code}"))
                .unwrap_or_else(|| "Stopped by a signal".into()),
        ),
        Some("exit-code") => Some(
            code.map(|code| format!("Exited with {code}"))
                .unwrap_or_else(|| "Exited with an error".into()),
        ),
        _ => code.map(|code| format!("Stopped: {code}")),
    }
}

/// Epoch deltas rounded to the nearest second, matching the source's compact
/// duration. Negative clock skew reads as zero instead of a negative uptime.
pub fn duration(ms: i64) -> String {
    let seconds = (ms.max(0) as u64 + 500) / 1000;
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let minutes = (seconds % 3600) / 60;
    if days > 0 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

pub fn list_note(unit: &ServiceUnit, now: i64) -> String {
    if unit.active == "failed" {
        return unit
            .exit_code
            .filter(|code| *code != 0)
            .map(|code| format!("exit {code}"))
            .unwrap_or_else(|| unit.result.clone().unwrap_or_else(|| "failed".into()));
    }
    if unit.sub == "auto-restart" {
        return unit
            .restarts
            .filter(|count| *count != 0)
            .map(|count| format!("restart ×{count}"))
            .unwrap_or_else(|| "restarting".into());
    }
    if unit.active == "active" && unit.sub == "running" {
        if let Some(since) = unit.since {
            return duration(now.saturating_sub(since));
        }
    }
    state_of(unit).label.to_lowercase()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AvailableVerbs {
    pub start: bool,
    pub stop: bool,
    pub restart: bool,
    pub reload: bool,
    pub enable: bool,
    pub disable: bool,
}
impl AvailableVerbs {
    pub const fn allows(self, verb: ServiceVerb) -> bool {
        match verb {
            ServiceVerb::Start => self.start,
            ServiceVerb::Stop => self.stop,
            ServiceVerb::Restart => self.restart,
            ServiceVerb::Reload => self.reload,
            ServiceVerb::Enable => self.enable,
            ServiceVerb::Disable => self.disable,
        }
    }
}
pub fn available_verbs(unit: &ServiceUnit) -> AvailableVerbs {
    let running = matches!(unit.active.as_str(), "active" | "reloading" | "activating");
    let bootable = !matches!(
        unit.enabled.as_deref().unwrap_or("static"),
        "static" | "masked" | "generated" | "transient" | "alias"
    );
    AvailableVerbs {
        start: !running,
        stop: running || unit.sub == "auto-restart",
        restart: true,
        reload: unit.active == "active",
        enable: bootable && unit.enabled.as_deref() != Some("enabled"),
        disable: bootable && unit.enabled.as_deref() == Some("enabled"),
    }
}

/// Password privilege remains actionable: the backend can request a terminal
/// handover instead of a direct privileged operation.
pub fn action_blocked_reason(scope: Scope, privilege: Privilege) -> Option<&'static str> {
    (scope == Scope::System && privilege == Privilege::None)
        .then_some("This account can’t manage system services: it isn’t root and has no sudo.")
}
pub fn priority_tone(priority: Option<u8>) -> Tone {
    match priority {
        Some(priority) if priority <= 3 => Tone::Danger,
        Some(4) => Tone::Warning,
        _ => Tone::Muted,
    }
}
pub fn timer_note(timer: &TimerUnit, now: i64) -> String {
    if let Some(next) = timer.next.filter(|next| *next != 0 && *next > now) {
        return format!("in {}", duration(next.saturating_sub(now)));
    }
    if let Some(last) = timer.last.filter(|last| *last != 0) {
        return format!("{} ago", duration(now.saturating_sub(last)));
    }
    if timer.active == "active" {
        "waiting".into()
    } else {
        "off".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn malformed_required_contract_fields_are_not_successful_empty_readings() {
        assert!(serde_json::from_value::<ServicesSnapshot>(json!({})).is_err());
        assert!(serde_json::from_value::<JournalPage>(json!({})).is_err());
        assert!(serde_json::from_value::<UnitFile>(json!({})).is_err());
        assert!(serde_json::from_value::<ActionResult>(json!({})).is_err());
        assert!(serde_json::from_value::<ServiceUnit>(json!({"name":"app.service"})).is_err());
    }

    #[test]
    fn unit_names_preserve_backend_validation_and_timer_read_only_boundary() {
        for name in [
            "app.service",
            "worker@tenant.service",
            "app\\x20name.service",
        ] {
            assert!(valid_unit(name, false));
        }
        assert!(valid_unit("cleanup.timer", true));
        assert!(!valid_unit("cleanup.timer", false));
        for name in [
            ".service",
            "-app.service",
            "app.service;reboot",
            "app name.service",
            "../app.service",
            "app.socket",
        ] {
            assert!(!valid_unit(name, true));
        }
        assert!(valid_unit(&format!("{}.service", "a".repeat(255)), false));
        assert!(!valid_unit(&format!("{}.service", "a".repeat(256)), false));
    }
    fn unit(name: &str) -> ServiceUnit {
        ServiceUnit {
            name: name.into(),
            load: "loaded".into(),
            active: "active".into(),
            sub: "running".into(),
            enabled: Some("enabled".into()),
            main_pid: Some(100),
            since: Some(0),
            restarts: Some(0),
            result: Some("success".into()),
            fragment: Some(format!("/lib/systemd/system/{name}")),
            ..Default::default()
        }
    }
    fn failed() -> ServiceUnit {
        ServiceUnit {
            active: "failed".into(),
            sub: "failed".into(),
            result: Some("exit-code".into()),
            exit_code: Some(1),
            exit_kind: Some("exited".into()),
            ..unit("atlas-api.service")
        }
    }
    #[test]
    fn contract_roundtrips_camel_case_nulls_and_lowercase_enums() {
        let value = json!({"name":"app.service","scope":"user","description":"Application","load":"loaded","active":"failed","sub":"failed","mainPid":42,"since":123,"memory":2048,"cpu":12.5,"restarts":2,"exitCode":9,"exitKind":"killed","dropIns":["/etc/app.conf"],"triggeredBy":["app.timer"]});
        let service: ServiceUnit = serde_json::from_value(value).unwrap();
        assert_eq!(service.scope, Scope::User);
        assert_eq!(service.main_pid, Some(42));
        let value = serde_json::to_value(&service).unwrap();
        assert_eq!(value["exitCode"], 9);
        assert_eq!(value["exitKind"], "killed");
        assert_eq!(value["triggeredBy"], json!(["app.timer"]));
        assert_eq!(value["enabled"], Value::Null);
        for verb in ServiceVerb::ALL {
            assert_eq!(serde_json::to_value(verb).unwrap(), verb.as_str());
        }
        for (value, privilege) in [
            ("root", Privilege::Root),
            ("sudo", Privilege::Sudo),
            ("password", Privilege::Password),
            ("none", Privilege::None),
        ] {
            assert_eq!(
                serde_json::from_value::<Privilege>(json!(value)).unwrap(),
                privilege
            );
        }
        let result: ActionResult = serde_json::from_value(json!({"ok":true})).unwrap();
        assert!(result.ok);
        assert_eq!(result.needs_password, None);
        let result: ActionResult = serde_json::from_value(
            json!({"ok":false,"needsPassword":true,"command":"sudo systemctl stop app.service"}),
        )
        .unwrap();
        assert_eq!(result.needs_password, Some(true));
        assert!(result.command.is_some());
        let snapshot:ServicesSnapshot=serde_json::from_value(json!({"services":[],"timers":[],"user":{"available":false,"reason":"No user manager"},"privilege":"none"})).unwrap();
        assert!(!snapshot.user.available);
        let page:JournalPage=serde_json::from_value(json!({"entries":[{"at":null,"priority":null,"pid":null,"identifier":null,"message":"message"}],"cursor":null})).unwrap();
        assert_eq!(page.entries[0].message, "message");
    }
    use serde_json::Value;
    #[test]
    fn grouping_separates_problems_owned_and_operating_system_units() {
        let services = vec![
            unit("systemd-journald.service"),
            unit("nginx.service"),
            ServiceUnit {
                active: "activating".into(),
                sub: "auto-restart".into(),
                ..unit("atlas-worker.service")
            },
            failed(),
            unit("getty@tty1.service"),
            ServiceUnit {
                scope: Scope::User,
                enabled: Some("disabled".into()),
                ..unit("dev-tunnel.service")
            },
            ServiceUnit {
                active: "inactive".into(),
                sub: "dead".into(),
                enabled: Some("static".into()),
                ..unit("certbot.service")
            },
            ServiceUnit {
                active: "inactive".into(),
                sub: "dead".into(),
                enabled: Some("disabled".into()),
                fragment: Some("/etc/systemd/system/backup.service".into()),
                ..unit("backup.service")
            },
        ];
        let groups = group_services(&services);
        fn names(list: Vec<&ServiceUnit>) -> Vec<&str> {
            list.into_iter()
                .map(|u| u.name.as_str())
                .collect::<Vec<_>>()
        }
        assert_eq!(
            names(groups.problems),
            ["atlas-api.service", "atlas-worker.service"]
        );
        assert_eq!(
            names(groups.owned),
            ["backup.service", "dev-tunnel.service", "nginx.service"]
        );
        assert_eq!(
            names(groups.system),
            [
                "certbot.service",
                "getty@tty1.service",
                "systemd-journald.service"
            ]
        );
    }
    #[test]
    fn scoped_identity_and_tie_order_keep_same_named_units_separate() {
        let services = vec![
            ServiceUnit {
                scope: Scope::User,
                ..unit("app.service")
            },
            unit("app.service"),
        ];
        assert_ne!(services[0].key(), services[1].key());
        let groups = group_services(&services);
        assert_eq!(groups.owned.len(), 2);
        assert_eq!(groups.owned[0].scope, Scope::System);
        assert_eq!(groups.owned[1].scope, Scope::User);
        assert_eq!(
            TimerUnit {
                name: "app.timer".into(),
                scope: Scope::User,
                ..Default::default()
            }
            .key(),
            "user:app.timer"
        );
    }
    #[test]
    fn ownership_and_restart_problems_match_the_source_boundaries() {
        for name in [
            "ssh.service",
            "systemd-resolved.service",
            "NetworkManager.service",
        ] {
            assert!(!is_owned(&unit(name)));
        }
        assert!(is_owned(&unit("redis-server.service")));
        assert!(is_owned(&ServiceUnit {
            scope: Scope::User,
            ..unit("systemd-example.service")
        }));
        let mut value = unit("app.service");
        value.active = "activating".into();
        value.restarts = Some(2);
        assert!(is_problem(&value));
        value.restarts = Some(0);
        assert!(!is_problem(&value));
        value.active = "inactive".into();
        value.enabled = Some("disabled".into());
        assert!(!is_owned(&value));
        for path in [
            "/lib/systemd/system/app.service",
            "/usr/lib/systemd/system/app.service",
        ] {
            value.fragment = Some(path.into());
            assert!(!is_owned(&value));
        }
        value.fragment = Some("/etc/systemd/system/app.service".into());
        assert!(is_owned(&value));
        value.fragment = Some("".into());
        assert!(!is_owned(&value));
    }
    #[test]
    fn readable_states_preserve_precedence_and_oneshot_semantics() {
        for (active, sub, label, tone) in [
            ("failed", "auto-restart", "Failed", Tone::Danger),
            ("activating", "auto-restart", "Restarting", Tone::Warning),
            ("activating", "start", "Starting", Tone::Warning),
            ("deactivating", "stop", "Stopping", Tone::Warning),
            ("reloading", "reload", "Reloading", Tone::Warning),
            ("active", "exited", "Completed", Tone::Muted),
            ("active", "waiting", "Waiting", Tone::Muted),
            ("active", "running", "Running", Tone::Success),
            ("inactive", "dead", "Stopped", Tone::Muted),
        ] {
            let u = ServiceUnit {
                active: active.into(),
                sub: sub.into(),
                ..unit("app")
            };
            assert_eq!(state_of(&u), ServiceState { label, tone });
        }
    }
    #[test]
    fn failure_reasons_use_result_code_and_signal_without_stale_healthy_errors() {
        assert_eq!(
            failure_reason(&failed()).as_deref(),
            Some("Exited with exit code 1")
        );
        for (result, expected) in [
            ("oom-kill", "Killed for running out of memory"),
            ("timeout", "Timed out while starting or stopping"),
            ("watchdog", "Stopped answering its watchdog"),
            ("start-limit-hit", "Restarted too often; systemd gave up"),
        ] {
            assert_eq!(
                failure_reason(&ServiceUnit {
                    result: Some(result.into()),
                    ..failed()
                })
                .as_deref(),
                Some(expected)
            );
        }
        assert_eq!(
            failure_reason(&ServiceUnit {
                result: Some("signal".into()),
                exit_kind: Some("killed".into()),
                exit_code: Some(9),
                ..failed()
            })
            .as_deref(),
            Some("Stopped: killed by signal 9")
        );
        assert_eq!(
            failure_reason(&ServiceUnit {
                result: Some("core-dump".into()),
                exit_kind: Some("dumped".into()),
                exit_code: Some(11),
                ..failed()
            })
            .as_deref(),
            Some("Crashed with a core dump (killed by signal 11)")
        );
        assert_eq!(
            failure_reason(&ServiceUnit {
                result: Some("signal".into()),
                exit_kind: Some("killed".into()),
                exit_code: None,
                ..failed()
            })
            .as_deref(),
            Some("Stopped: killed by signal ?")
        );
        assert_eq!(
            failure_reason(&ServiceUnit {
                result: None,
                exit_code: Some(0),
                ..failed()
            }),
            None
        );
        assert_eq!(
            failure_reason(&ServiceUnit {
                active: "active".into(),
                sub: "running".into(),
                ..failed()
            }),
            None
        );
    }
    #[test]
    fn row_notes_handle_zero_and_missing_fields_and_clock_skew() {
        assert_eq!(list_note(&failed(), 0), "exit 1");
        assert_eq!(
            list_note(
                &ServiceUnit {
                    active: "activating".into(),
                    sub: "auto-restart".into(),
                    restarts: Some(14),
                    ..unit("a")
                },
                0
            ),
            "restart ×14"
        );
        assert_eq!(list_note(&unit("a"), 2 * 86400000 + 3 * 3600000), "2d 3h");
        assert_eq!(
            list_note(
                &ServiceUnit {
                    active: "inactive".into(),
                    sub: "dead".into(),
                    ..unit("a")
                },
                0
            ),
            "stopped"
        );
        assert_eq!(
            list_note(
                &ServiceUnit {
                    exit_code: Some(0),
                    result: None,
                    ..failed()
                },
                0
            ),
            "failed"
        );
        assert_eq!(
            list_note(
                &ServiceUnit {
                    since: Some(1000),
                    ..unit("a")
                },
                0
            ),
            "0s"
        );
    }
    #[test]
    fn duration_rounding_and_compact_units_match_reference() {
        for (ms, expected) in [
            (42000, "42s"),
            (90 * 60000, "1h 30m"),
            (3 * 3600000, "3h"),
            (-5, "0s"),
            (499, "0s"),
            (500, "1s"),
            (59999, "1m"),
            (86400000, "1d"),
        ] {
            assert_eq!(duration(ms), expected);
        }
    }
    #[test]
    fn action_availability_and_privilege_are_independent() {
        assert_eq!(
            available_verbs(&unit("a")),
            AvailableVerbs {
                start: false,
                stop: true,
                restart: true,
                reload: true,
                enable: false,
                disable: true
            }
        );
        let verbs = available_verbs(&ServiceUnit {
            enabled: Some("disabled".into()),
            ..failed()
        });
        assert!(verbs.start && verbs.enable && !verbs.stop && !verbs.reload);
        let looping = available_verbs(&ServiceUnit {
            active: "inactive".into(),
            sub: "auto-restart".into(),
            ..unit("a")
        });
        assert!(looping.stop);
        for enabled in [
            None,
            Some("static"),
            Some("masked"),
            Some("generated"),
            Some("transient"),
            Some("alias"),
        ] {
            let verbs = available_verbs(&ServiceUnit {
                enabled: enabled.map(str::to_owned),
                ..unit("a")
            });
            assert!(!verbs.enable && !verbs.disable);
            assert!(verbs.allows(ServiceVerb::Restart));
        }
        assert!(action_blocked_reason(Scope::System, Privilege::None).is_some());
        for privilege in [Privilege::Root, Privilege::Sudo, Privilege::Password] {
            assert!(action_blocked_reason(Scope::System, privilege).is_none());
        }
        assert!(action_blocked_reason(Scope::User, Privilege::None).is_none());
    }
    #[test]
    fn priorities_and_timer_notes_preserve_missing_and_zero_timestamp_behavior() {
        for (priority, tone) in [
            (Some(0), Tone::Danger),
            (Some(3), Tone::Danger),
            (Some(4), Tone::Warning),
            (Some(6), Tone::Muted),
            (None, Tone::Muted),
        ] {
            assert_eq!(priority_tone(priority), tone);
        }
        let mut timer = TimerUnit {
            active: "active".into(),
            next: Some(120000),
            last: Some(30000),
            ..Default::default()
        };
        assert_eq!(timer_note(&timer, 60000), "in 1m");
        timer.next = Some(60000);
        assert_eq!(timer_note(&timer, 60000), "30s ago");
        timer.last = Some(0);
        assert_eq!(timer_note(&timer, 60000), "waiting");
        timer.active = "inactive".into();
        assert_eq!(timer_note(&timer, 60000), "off");
    }
}
