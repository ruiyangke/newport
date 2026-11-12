use super::{Containers, MAX_CONTAINERS, valid_id};
use crate::docker::{Endpoint as Engine, Query, QueryError};
use newport_protocol::monitor::ports::{
    MAX_FIELD, MAX_LISTENERS,
    discovery::{
        Compose, ContainerState, Endpoint, Issue, IssueKind, MAX_DIAGNOSTIC, MAX_ISSUES, Outcome,
        Publication, Report,
    },
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    net::IpAddr,
    time::{Duration, Instant},
};

const RESPONSE_BYTES: usize = 4 << 20;
const PUBLICATION_TEXT: usize = 1 << 20;
const QUERY_TIME: Duration = Duration::from_secs(3);

pub(crate) struct Publications {
    pub values: Vec<Publication>,
    pub report: Report,
    pub observed_engine: bool,
    pub engine: Option<String>,
}

impl Publications {
    fn empty(report: Report, observed_engine: bool) -> Self {
        Self {
            values: Vec::new(),
            report,
            observed_engine,
            engine: None,
        }
    }
}

impl Containers {
    pub(crate) fn publication_inventory(
        &self,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
        previously_observed: bool,
    ) -> Publications {
        let query = Query::new(
            deadline.min(Instant::now() + QUERY_TIME),
            RESPONSE_BYTES,
            cancelled,
        );
        let engine = match Engine::discover(&(self.sockets)(), &query) {
            Ok(engine) => engine,
            Err(QueryError::Missing) if !previously_observed => {
                return Publications::empty(Report::not_applicable(), false);
            }
            Err(error) => return Publications::empty(failure(&error), previously_observed),
        };
        match inventory(&engine, &query) {
            Ok(values) => values,
            Err(error) => Publications::empty(failure(&error), true),
        }
    }
}

fn failure(error: &QueryError) -> Report {
    let kind = match error.kind() {
        io::ErrorKind::PermissionDenied => IssueKind::PermissionDenied,
        io::ErrorKind::TimedOut => IssueKind::TimedOut,
        io::ErrorKind::Interrupted => IssueKind::Interrupted,
        io::ErrorKind::FileTooLarge => IssueKind::BudgetExceeded,
        io::ErrorKind::InvalidData => IssueKind::InvalidResponse,
        io::ErrorKind::Unsupported => IssueKind::Unsupported,
        io::ErrorKind::NotFound => IssueKind::Unavailable,
        _ => IssueKind::Io,
    };
    let mut message = error.to_string();
    message.truncate(message.floor_char_boundary(MAX_DIAGNOSTIC));
    Report {
        outcome: Outcome::Unavailable,
        issues: vec![Issue {
            kind,
            count: 1,
            message,
        }],
    }
}

fn issue(report: &mut Report, kind: IssueKind, message: &'static str) {
    report.outcome = Outcome::Partial;
    if let Some(issue) = report.issues.iter_mut().find(|issue| issue.kind == kind) {
        issue.count = issue.count.saturating_add(1);
    } else if report.issues.len() < MAX_ISSUES {
        report.issues.push(Issue {
            kind,
            count: 1,
            message: message.into(),
        });
    }
}

#[derive(Deserialize)]
struct Info {
    #[serde(rename = "ID")]
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Container {
    id: String,
    names: Vec<String>,
    image: Option<String>,
    state: String,
    /// Engines that encode an empty Go slice send `null` for a container
    /// without ports.
    #[serde(default)]
    ports: Option<Vec<Value>>,
    labels: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Port {
    #[serde(rename = "Type")]
    transport: String,
    public_port: Option<u16>,
    private_port: u16,
    /// Empty for a port the container exposes but doesn't publish.
    #[serde(rename = "IP", default)]
    address: Option<String>,
}

fn text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_FIELD && !value.chars().any(char::is_control)
}

impl Container {
    fn name(&self) -> Option<&str> {
        self.names
            .iter()
            .map(|name| name.trim_start_matches('/'))
            .filter(|name| text(name))
            .min()
    }

    fn valid(&self) -> bool {
        valid_id(&self.id)
            && self.name().is_some()
            && text(&self.state)
            && self.image.as_deref().is_none_or(text)
            && self.labels.as_ref().is_none_or(|labels| {
                [
                    "com.docker.compose.project",
                    "com.docker.compose.service",
                    "com.docker.compose.project.working_dir",
                ]
                .iter()
                .all(|key| {
                    labels
                        .get(*key)
                        .is_none_or(|value| value.is_empty() || text(value))
                })
            })
    }

    fn compose(&self) -> Option<Compose> {
        let labels = self.labels.as_ref()?;
        let label = |key| labels.get(key).filter(|value| !value.is_empty()).cloned();
        let compose = Compose {
            project: label("com.docker.compose.project"),
            service: label("com.docker.compose.service"),
            directory: label("com.docker.compose.project.working_dir"),
            directory_project: None,
        };
        (compose.project.is_some() || compose.service.is_some() || compose.directory.is_some())
            .then_some(compose)
    }

    fn state(&self) -> ContainerState {
        match self.state.as_str() {
            "running" => ContainerState::Running,
            "created" | "exited" | "dead" | "removing" => ContainerState::Stopped,
            _ => ContainerState::Unknown,
        }
    }
}

fn inventory(engine: &Engine, query: &Query<'_>) -> Result<Publications, QueryError> {
    if engine.is_podman() {
        // Podman's compatibility API generates a fresh /info ID per request.
        let mut result = convert(None, engine.get("/containers/json", query)?, query);
        issue(
            &mut result.report,
            IssueKind::Unsupported,
            "Podman does not supply a stable engine identity; publication absence cannot be verified",
        );
        return Ok(result);
    }
    let info: Info = engine.get("/info", query)?;
    if !text(&info.id) {
        return Err(QueryError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "Docker engine identity is invalid",
        )));
    }
    let values = match engine.get::<Vec<Value>>("/containers/json", query) {
        Ok(rows) => {
            // Each API request opens a connection; an engine can be replaced between them.
            let confirmed: Info = engine.get("/info", query)?;
            if confirmed.id != info.id {
                return Err(QueryError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Docker engine changed during publication discovery",
                )));
            }
            convert(Some(&info.id), rows, query)
        }
        Err(error) => Publications {
            engine: Some(info.id),
            ..Publications::empty(failure(&error), true)
        },
    };
    Ok(values)
}

fn convert(engine: Option<&str>, rows: Vec<Value>, query: &Query<'_>) -> Publications {
    let mut result = Publications::empty(Report::complete(), true);
    result.engine = engine.map(str::to_owned);
    if rows.len() > MAX_CONTAINERS {
        issue(
            &mut result.report,
            IssueKind::BudgetExceeded,
            "Docker container count limit reached",
        );
    }
    let mut collected = Collected {
        result,
        seen: BTreeSet::new(),
        bytes: 0,
    };
    for row in rows.into_iter().take(MAX_CONTAINERS) {
        if !collected.active(query) {
            break;
        }
        let container = match serde_json::from_value::<Container>(row) {
            Ok(container) if container.valid() => container,
            _ => {
                issue(
                    &mut collected.result.report,
                    IssueKind::InvalidResponse,
                    "Docker returned invalid container metadata",
                );
                continue;
            }
        };
        if !collected.container(engine, &container, query) {
            break;
        }
    }
    collected.result.values.sort_by(|a, b| {
        (&a.engine, &a.container, a.host, a.target_port).cmp(&(
            &b.engine,
            &b.container,
            b.host,
            b.target_port,
        ))
    });
    collected.result
}

struct Collected {
    result: Publications,
    seen: BTreeSet<(String, Endpoint, u16)>,
    bytes: usize,
}

impl Collected {
    fn active(&mut self, query: &Query<'_>) -> bool {
        match query.check() {
            Ok(()) => true,
            Err(error) => {
                for failure in failure(&error).issues {
                    issue(
                        &mut self.result.report,
                        failure.kind,
                        "Docker publication collection interrupted",
                    );
                }
                false
            }
        }
    }

    fn container(
        &mut self,
        engine: Option<&str>,
        container: &Container,
        query: &Query<'_>,
    ) -> bool {
        let name = container.name().unwrap_or_default();
        let compose = container.compose();
        for raw in container.ports.iter().flatten() {
            if !self.active(query) {
                return false;
            }
            let (host, target_port) = match binding(raw) {
                Ok(Some(binding)) => binding,
                Ok(None) => continue,
                Err(message) => {
                    issue(&mut self.result.report, IssueKind::InvalidResponse, message);
                    continue;
                }
            };
            let value = Publication {
                engine: engine.map(str::to_owned),
                container: container.id.clone(),
                name: name.into(),
                image: container.image.clone(),
                state: container.state(),
                host,
                target_port,
                target_address: None,
                compose: compose.clone(),
            };
            if !self.append(value) {
                return false;
            }
        }
        true
    }

    fn append(&mut self, value: Publication) -> bool {
        if !self
            .seen
            .insert((value.container.clone(), value.host, value.target_port))
        {
            return true;
        }
        let size = text_bytes(&value);
        if self.result.values.len() == MAX_LISTENERS || size > PUBLICATION_TEXT - self.bytes {
            issue(
                &mut self.result.report,
                IssueKind::BudgetExceeded,
                "Docker publication limit reached",
            );
            return false;
        }
        self.bytes += size;
        self.result.values.push(value);
        true
    }
}

fn binding(raw: &Value) -> Result<Option<(Endpoint, u16)>, &'static str> {
    let port = Port::deserialize(raw).map_err(|_| "Docker returned an invalid port binding")?;
    if port.transport != "tcp" || port.public_port.is_none() {
        return Ok(None);
    }
    let (Some(address), Some(public)) = (
        port.address
            .as_deref()
            .filter(|address| !address.is_empty()),
        port.public_port,
    ) else {
        return Err("Docker publication has no host address");
    };
    let address: IpAddr = address
        .parse()
        .map_err(|_| "Docker returned an invalid port binding")?;
    if public == 0 || port.private_port == 0 {
        return Err("Docker publication has an invalid port");
    }
    Ok(Some((
        Endpoint {
            address,
            port: public,
            interface: None,
        },
        port.private_port,
    )))
}

fn text_bytes(value: &Publication) -> usize {
    let compose = value
        .compose
        .iter()
        .flat_map(|value| {
            [
                value.project.as_deref(),
                value.service.as_deref(),
                value.directory.as_deref(),
            ]
        })
        .flatten();
    [&value.container, &value.name]
        .into_iter()
        .map(String::as_str)
        .chain(value.engine.as_deref())
        .chain(value.image.as_deref())
        .chain(compose)
        .map(str::len)
        .sum()
}

#[cfg(test)]
mod tests;
