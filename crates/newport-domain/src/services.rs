//! Systemd service rules over the protocol's service types, with no UI or
//! runtime dependencies.
use newport_protocol::monitor::services::{ExitKind, Privilege, Scope, Service, Timer, Verb};

pub fn short_name(name: &str) -> &str {
    name.strip_suffix(".service")
        .or_else(|| name.strip_suffix(".timer"))
        .unwrap_or(name)
}

pub fn is_problem(unit: &Service) -> bool {
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
pub fn is_owned(unit: &Service) -> bool {
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
    pub problems: Vec<&'a Service>,
    pub owned: Vec<&'a Service>,
    pub system: Vec<&'a Service>,
}

/// Stable, case-insensitive name order; scoped identities remain separate.
/// Browser localeCompare is locale-dependent. The core uses deterministic
/// Unicode lowercase ordering so every client receives the same grouping.
pub fn group_services(services: &[Service]) -> ServiceGroups<'_> {
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

pub fn state_of(unit: &Service) -> ServiceState {
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

pub fn failure_reason(unit: &Service) -> Option<String> {
    if unit.active != "failed" && unit.sub != "auto-restart" {
        return None;
    }
    let code = if matches!(unit.exit_kind, Some(ExitKind::Killed | ExitKind::Dumped)) {
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

pub fn list_note(unit: &Service, now: i64) -> String {
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
    if unit.active == "active"
        && unit.sub == "running"
        && let Some(since) = unit.since
    {
        return duration(now.saturating_sub(since));
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
    pub const fn allows(self, verb: Verb) -> bool {
        match verb {
            Verb::Start => self.start,
            Verb::Stop => self.stop,
            Verb::Restart => self.restart,
            Verb::Reload => self.reload,
            Verb::Enable => self.enable,
            Verb::Disable => self.disable,
        }
    }
}
pub fn available_verbs(unit: &Service) -> AvailableVerbs {
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
pub fn timer_note(timer: &Timer, now: i64) -> String {
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

    fn unit(name: &str) -> Service {
        Service {
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
    fn failed() -> Service {
        Service {
            active: "failed".into(),
            sub: "failed".into(),
            result: Some("exit-code".into()),
            exit_code: Some(1),
            exit_kind: Some(ExitKind::Exited),
            ..unit("atlas-api.service")
        }
    }

    #[test]
    fn grouping_separates_problems_owned_and_operating_system_units() {
        let services = vec![
            unit("systemd-journald.service"),
            unit("nginx.service"),
            Service {
                active: "activating".into(),
                sub: "auto-restart".into(),
                ..unit("atlas-worker.service")
            },
            failed(),
            unit("getty@tty1.service"),
            Service {
                scope: Scope::User,
                enabled: Some("disabled".into()),
                ..unit("dev-tunnel.service")
            },
            Service {
                active: "inactive".into(),
                sub: "dead".into(),
                enabled: Some("static".into()),
                ..unit("certbot.service")
            },
            Service {
                active: "inactive".into(),
                sub: "dead".into(),
                enabled: Some("disabled".into()),
                fragment: Some("/etc/systemd/system/backup.service".into()),
                ..unit("backup.service")
            },
        ];
        let groups = group_services(&services);
        fn names(list: Vec<&Service>) -> Vec<&str> {
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
            Service {
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
            Timer {
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
        assert!(is_owned(&Service {
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
            let u = Service {
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
                failure_reason(&Service {
                    result: Some(result.into()),
                    ..failed()
                })
                .as_deref(),
                Some(expected)
            );
        }
        assert_eq!(
            failure_reason(&Service {
                result: Some("signal".into()),
                exit_kind: Some(ExitKind::Killed),
                exit_code: Some(9),
                ..failed()
            })
            .as_deref(),
            Some("Stopped: killed by signal 9")
        );
        assert_eq!(
            failure_reason(&Service {
                result: Some("core-dump".into()),
                exit_kind: Some(ExitKind::Dumped),
                exit_code: Some(11),
                ..failed()
            })
            .as_deref(),
            Some("Crashed with a core dump (killed by signal 11)")
        );
        assert_eq!(
            failure_reason(&Service {
                result: Some("signal".into()),
                exit_kind: Some(ExitKind::Killed),
                exit_code: None,
                ..failed()
            })
            .as_deref(),
            Some("Stopped: killed by signal ?")
        );
        assert_eq!(
            failure_reason(&Service {
                result: None,
                exit_code: Some(0),
                ..failed()
            }),
            None
        );
        assert_eq!(
            failure_reason(&Service {
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
                &Service {
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
                &Service {
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
                &Service {
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
                &Service {
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
        let verbs = available_verbs(&Service {
            enabled: Some("disabled".into()),
            ..failed()
        });
        assert!(verbs.start && verbs.enable && !verbs.stop && !verbs.reload);
        let looping = available_verbs(&Service {
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
            let verbs = available_verbs(&Service {
                enabled: enabled.map(str::to_owned),
                ..unit("a")
            });
            assert!(!verbs.enable && !verbs.disable);
            assert!(verbs.allows(Verb::Restart));
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
        let mut timer = Timer {
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
