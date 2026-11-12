//! Systemd inventory, journal reads and service lifecycle operations.
use super::Invalid;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

pub const MAX_SERVICES: usize = 2000;
pub const MAX_TIMERS: usize = 4000;
pub const MAX_LOG_ENTRIES: usize = 1000;
pub const MAX_TEXT: usize = 16 * 1024;
pub const MAX_LIST: usize = 256;
pub const MAX_CURSOR: usize = 512;
pub const MAX_UNIT_FILE: usize = 256 * 1024;
pub const MAX_REPLY_TEXT: usize = 3 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidRequest {
    Unit,
    Cursor,
    Lines,
}
impl fmt::Display for InvalidRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unit => "Not a valid unit name.",
            Self::Cursor => "Invalid journal cursor.",
            Self::Lines => "Journal line count is outside the supported range.",
        })
    }
}
impl std::error::Error for InvalidRequest {}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    System,
    User,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Privilege {
    Root,
    Sudo,
    Password,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verb {
    Start,
    Stop,
    Restart,
    Reload,
    Enable,
    Disable,
}
impl Verb {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Err,
    Warning,
}
impl Priority {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Err => "err",
            Self::Warning => "warning",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExitKind {
    Exited,
    Killed,
    Dumped,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsRequest {
    #[serde(default)]
    pub scope: Scope,
    pub unit: Option<String>,
    pub priority: Option<Priority>,
    pub cursor: Option<String>,
    pub lines: Option<u32>,
}
impl LogsRequest {
    pub fn validate(&self) -> Result<(), InvalidRequest> {
        if self
            .unit
            .as_ref()
            .is_some_and(|unit| !valid_unit(unit, true))
        {
            return Err(InvalidRequest::Unit);
        }
        if self.cursor.as_ref().is_some_and(|value| !cursor(value)) {
            return Err(InvalidRequest::Cursor);
        }
        if self
            .lines
            .is_some_and(|lines| lines == 0 || lines > MAX_LOG_ENTRIES as u32)
        {
            return Err(InvalidRequest::Lines);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Request {
    #[serde(rename = "services")]
    Snapshot,
    #[serde(rename = "logs")]
    Logs(LogsRequest),
    #[serde(rename = "unit")]
    Unit {
        #[serde(default)]
        scope: Scope,
        unit: String,
    },
    #[serde(rename = "action")]
    Action {
        #[serde(default)]
        scope: Scope,
        unit: String,
        verb: Verb,
    },
}
impl Request {
    pub fn validate(&self) -> Result<(), InvalidRequest> {
        match self {
            Self::Snapshot => Ok(()),
            Self::Logs(request) => request.validate(),
            Self::Unit { unit, .. } if valid_unit(unit, true) => Ok(()),
            Self::Action { unit, .. } if valid_unit(unit, false) => Ok(()),
            _ => Err(InvalidRequest::Unit),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub name: String,
    pub scope: Scope,
    pub description: String,
    pub load: String,
    pub active: String,
    pub sub: String,
    pub enabled: Option<String>,
    pub main_pid: Option<u32>,
    pub since: Option<i64>,
    pub memory: Option<u64>,
    pub cpu: Option<f64>,
    pub restarts: Option<u64>,
    pub result: Option<String>,
    pub exit_code: Option<i64>,
    pub exit_kind: Option<ExitKind>,
    pub fragment: Option<String>,
    pub drop_ins: Vec<String>,
    pub triggered_by: Vec<String>,
    pub user: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timer {
    pub name: String,
    pub scope: Scope,
    pub description: String,
    pub active: String,
    pub next: Option<i64>,
    pub last: Option<i64>,
    pub unit: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserManager {
    pub available: bool,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub services: Vec<Service>,
    pub timers: Vec<Timer>,
    pub user: UserManager,
    pub privilege: Privilege,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub at: Option<i64>,
    pub priority: Option<u8>,
    pub pid: Option<u32>,
    pub identifier: Option<String>,
    pub message: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Logs {
    pub entries: Vec<Entry>,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitFile {
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_password: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Snapshot(Snapshot),
    Logs(Logs),
    Unit(UnitFile),
    Action(ActionReply),
}

/// Unit-name arguments accept systemd escapes, with no leading option or shell syntax.
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
fn text(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.contains('\0')
}
fn cursor(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_CURSOR && !value.chars().any(char::is_control)
}
fn add_text<'a>(
    values: impl IntoIterator<Item = &'a str>,
    bytes: &mut usize,
) -> Result<(), Invalid> {
    for value in values {
        *bytes = bytes.saturating_add(value.len());
        if !text(value, MAX_TEXT) || *bytes > MAX_REPLY_TEXT {
            return Err(Invalid("service text budget"));
        }
    }
    Ok(())
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.services.len() > MAX_SERVICES || self.timers.len() > MAX_TIMERS {
            return Err(Invalid("unit count"));
        }
        let mut bytes = 0;
        let mut units = BTreeSet::new();
        add_text(self.user.reason.as_deref(), &mut bytes)?;
        for service in &self.services {
            if !valid_unit(&service.name, false)
                || !units.insert((service.scope, &service.name))
                || service.main_pid == Some(0)
                || service.cpu.is_some_and(|v| !v.is_finite() || v < 0.0)
                || service.drop_ins.len() > MAX_LIST
                || service.triggered_by.len() > MAX_LIST
            {
                return Err(Invalid("service observation"));
            }
            add_text(
                [
                    service.name.as_str(),
                    &service.description,
                    &service.load,
                    &service.active,
                    &service.sub,
                ],
                &mut bytes,
            )?;
            add_text(
                [
                    &service.enabled,
                    &service.result,
                    &service.fragment,
                    &service.user,
                ]
                .into_iter()
                .filter_map(|v| v.as_deref()),
                &mut bytes,
            )?;
            add_text(
                service
                    .drop_ins
                    .iter()
                    .chain(&service.triggered_by)
                    .map(String::as_str),
                &mut bytes,
            )?;
        }
        for timer in &self.timers {
            if !timer.name.ends_with(".timer")
                || !valid_unit(&timer.name, true)
                || !units.insert((timer.scope, &timer.name))
            {
                return Err(Invalid("timer observation"));
            }
            add_text(
                [timer.name.as_str(), &timer.description, &timer.active]
                    .into_iter()
                    .chain(timer.unit.as_deref()),
                &mut bytes,
            )?;
        }
        Ok(())
    }
}
impl Logs {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.entries.len() > MAX_LOG_ENTRIES || self.cursor.as_ref().is_some_and(|v| !cursor(v))
        {
            return Err(Invalid("journal page"));
        }
        let mut bytes = self.cursor.as_ref().map_or(0, String::len);
        for entry in &self.entries {
            if entry.priority.is_some_and(|v| v > 7) {
                return Err(Invalid("journal priority"));
            }
            add_text(
                [entry.message.as_str()]
                    .into_iter()
                    .chain(entry.identifier.as_deref()),
                &mut bytes,
            )?;
        }
        Ok(())
    }
}
impl UnitFile {
    pub fn validate(&self) -> Result<(), Invalid> {
        if text(&self.text, MAX_UNIT_FILE) {
            Ok(())
        } else {
            Err(Invalid("unit file"))
        }
    }
}
impl ActionReply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match (self.ok, self.needs_password, self.command.as_deref()) {
            (true, None | Some(false), None) => Ok(()),
            (false, Some(true), Some(command))
                if !command.is_empty() && text(command, MAX_TEXT) =>
            {
                Ok(())
            }
            _ => Err(Invalid("service action result")),
        }
    }
}
impl Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Snapshot(v) => v.validate(),
            Self::Logs(v) => v.validate(),
            Self::Unit(v) => v.validate(),
            Self::Action(v) => v.validate(),
        }
    }
    pub fn validate_for(&self, request: &Request) -> Result<(), Invalid> {
        self.validate()?;
        if matches!(
            (self, request),
            (Self::Snapshot(_), Request::Snapshot)
                | (Self::Logs(_), Request::Logs(_))
                | (Self::Unit(_), Request::Unit { .. })
                | (Self::Action(_), Request::Action { .. })
        ) {
            Ok(())
        } else {
            Err(Invalid("service result operation"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_validates_scoped_identity_cpu_and_total_text() {
        let service: Service = serde_json::from_value(serde_json::json!({
            "name": "worker.service", "scope": "system", "description": "worker",
            "load": "loaded", "active": "active", "sub": "running", "cpu": 250.0,
            "dropIns": [], "triggeredBy": []
        }))
        .unwrap();
        let mut snapshot = Snapshot {
            services: vec![service.clone()],
            timers: Vec::new(),
            user: UserManager {
                available: true,
                reason: None,
            },
            privilege: Privilege::Root,
        };
        snapshot.validate().unwrap();
        snapshot.services.push(service.clone());
        assert!(snapshot.validate().is_err());
        snapshot.services[1].scope = Scope::User;
        snapshot.validate().unwrap();
        snapshot.services[0].cpu = Some(f64::NAN);
        assert!(snapshot.validate().is_err());
        snapshot.services = (0..=MAX_REPLY_TEXT / MAX_TEXT)
            .map(|n| Service {
                name: format!("worker{n}.service"),
                description: "x".repeat(MAX_TEXT),
                ..service.clone()
            })
            .collect();
        assert!(snapshot.validate().is_err());
    }

    #[test]
    fn requests_reject_argument_injection_and_unbounded_journal_reads() {
        for unit in [
            "-x.service",
            "a b.service",
            "$(id).service",
            ".service",
            "x.socket",
            "x.service\n",
        ] {
            assert!(!valid_unit(unit, true), "{unit}");
        }
        for unit in [
            "nginx.service",
            "postgresql@16-main.service",
            "dev-sda1\\x2d.service",
        ] {
            assert!(valid_unit(unit, false));
        }
        Request::Unit {
            scope: Scope::User,
            unit: "cleanup.timer".into(),
        }
        .validate()
        .unwrap();
        assert!(
            Request::Action {
                scope: Scope::User,
                unit: "cleanup.timer".into(),
                verb: Verb::Start
            }
            .validate()
            .is_err()
        );
        for query in [
            LogsRequest {
                cursor: Some("a\nb".into()),
                ..Default::default()
            },
            LogsRequest {
                lines: Some(0),
                ..Default::default()
            },
            LogsRequest {
                lines: Some(MAX_LOG_ENTRIES as u32 + 1),
                ..Default::default()
            },
        ] {
            assert!(query.validate().is_err());
        }
        for request in [
            serde_json::json!({"op":"logs", "priority":"debug"}),
            serde_json::json!({"op":"action", "unit":"x.service", "verb":"mask"}),
            serde_json::json!({"op":"unit", "unit":"x.service", "scope":"global"}),
        ] {
            assert!(serde_json::from_value::<Request>(request).is_err());
        }
    }
    #[test]
    fn action_results_and_reply_operations_cannot_contradict_each_other() {
        let completed = ActionReply {
            ok: true,
            needs_password: None,
            command: None,
        };
        completed.validate().unwrap();
        let request = Request::Action {
            scope: Scope::System,
            unit: "nginx.service".into(),
            verb: Verb::Stop,
        };
        Reply::Action(completed.clone())
            .validate_for(&request)
            .unwrap();
        assert!(
            Reply::Action(completed)
                .validate_for(&Request::Snapshot)
                .is_err()
        );
        for value in [
            ActionReply {
                ok: true,
                needs_password: Some(true),
                command: Some("sudo systemctl stop nginx.service".into()),
            },
            ActionReply {
                ok: false,
                needs_password: None,
                command: None,
            },
        ] {
            assert!(value.validate().is_err());
        }
    }
    #[test]
    fn journal_pages_bound_rows_text_and_priorities() {
        let entry = Entry {
            at: Some(0),
            priority: Some(0),
            pid: Some(0),
            identifier: None,
            message: "line".into(),
        };
        let mut page = Logs {
            entries: vec![entry.clone()],
            cursor: Some("s=1;i=1".into()),
        };
        page.validate().unwrap();
        page.entries[0].priority = Some(8);
        assert!(page.validate().is_err());
        page.entries = vec![entry; MAX_LOG_ENTRIES + 1];
        assert!(page.validate().is_err());
        page.entries.truncate(1);
        page.entries[0].message = "x".repeat(MAX_TEXT + 1);
        assert!(page.validate().is_err());
    }
}
