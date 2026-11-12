//! Pure presentation and selection rules for typed discovery observations.
//!
//! These are application inputs, not a second transport schema. The client keeps
//! the original evidence and maps its references to these smaller row inputs.
use std::{collections::BTreeSet, net::IpAddr, sync::Arc};

mod grouping;
pub use grouping::{Group, GroupKind, group};
mod routing;
pub use routing::{Destination, DestinationError, Inventory, Selection};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scope {
    pub network: (u64, u64),
    pub processes: (u64, u64),
    pub boot: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Endpoint {
    pub address: IpAddr,
    pub port: u16,
    /// An interface on the server. It must never be resolved on the desktop.
    pub interface: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Record {
    Socket(u32),
    Publication(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reference {
    pub generation: u64,
    pub record: Record,
}

/// Identity of one immutable observation, independent of remotely supplied IDs.
/// Clones keep it; constructing a new observation creates a distinct identity.
#[derive(Clone, Debug, Default)]
pub struct Observation(Arc<()>);

impl PartialEq for Observation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Observation {}
impl std::hash::Hash for Observation {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Project {
    pub root: String,
    pub marker: String,
    pub checkout: Option<Checkout>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Checkout {
    pub path: String,
    pub branch: Option<String>,
    pub main: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Process {
    pub pid: u32,
    pub start: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub identity: Process,
    pub name: Option<String>,
    pub executable: Option<String>,
    pub label: Option<String>,
    pub account: Option<String>,
    pub uid: Option<u32>,
    pub project: Option<Project>,
    pub kinds: Vec<&'static str>,
    pub details: Vec<crate::port_context::Detail>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unit {
    /// None means the system manager; Some identifies a specific user manager.
    pub manager_uid: Option<u32>,
    pub name: String,
    /// Socket-cgroup evidence is distinct from a verified holder's membership.
    pub socket_cgroup: bool,
}

impl Unit {
    pub fn dedicated(&self) -> bool {
        (self.name.ends_with(".service") || self.name.ends_with(".socket"))
            && !(self.name.starts_with("user@") && self.name.ends_with(".service"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HolderCoverage {
    Complete,
    /// Some processes could not be inspected; every observed holder is retained.
    Partial,
    /// Some observed holders were omitted from this socket's evidence.
    Truncated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Socket {
    pub inode: u64,
    pub cookie: Option<u64>,
    pub uid: u32,
    pub account: Option<String>,
    pub ipv6_only: Option<bool>,
    pub holders: Vec<Arc<Holder>>,
    pub holder_coverage: HolderCoverage,
    pub units: Vec<Unit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Publication {
    pub engine: Option<String>,
    pub container: String,
    pub name: String,
    pub image: Option<String>,
    pub service: Option<String>,
    pub compose: Option<String>,
    pub directory: Option<String>,
    pub project: Option<Project>,
    pub target_port: u16,
    pub target_address: Option<IpAddr>,
    pub running: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Socket(Socket),
    Publication(Publication),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub observation: Observation,
    pub reference: Reference,
    pub scope: Option<Arc<Scope>>,
    pub endpoint: Endpoint,
    pub stale: bool,
    pub kind: Kind,
}

impl Member {
    /// A socket on `::` without IPV6_V6ONLY also accepts IPv4 connections.
    pub fn dual_stack(&self) -> bool {
        matches!(self.endpoint.address, IpAddr::V6(ip) if ip.is_unspecified())
            && matches!(&self.kind, Kind::Socket(socket) if socket.ipv6_only != Some(true))
    }

    /// Project grouping describes agreeing observed holders. Unreadable processes
    /// elsewhere do not erase those facts; omitted or conflicting holders do.
    pub fn project(&self) -> Option<&Project> {
        match &self.kind {
            Kind::Publication(value) => value.project.as_ref(),
            Kind::Socket(value) if value.holder_coverage != HolderCoverage::Truncated => {
                let first = value.holders.first()?.project.as_ref()?;
                value
                    .holders
                    .iter()
                    .all(|holder| holder.project.as_ref() == Some(first))
                    .then_some(first)
            }
            Kind::Socket(_) => None,
        }
    }

    pub fn unit(&self) -> Option<&Unit> {
        let Kind::Socket(socket) = &self.kind else {
            return None;
        };
        let preferred = socket
            .units
            .iter()
            .any(|unit| unit.dedicated() && unit.socket_cgroup);
        let mut units = socket
            .units
            .iter()
            .filter(|unit| unit.dedicated() && (!preferred || unit.socket_cgroup));
        let first = units.next()?;
        units
            .all(|unit| unit.manager_uid == first.manager_uid && unit.name == first.name)
            .then_some(first)
    }

    pub fn label(&self) -> String {
        let raw = match &self.kind {
            Kind::Publication(publication) => publication
                .service
                .as_deref()
                .filter(|v| !v.is_empty())
                .unwrap_or(&publication.name),
            Kind::Socket(socket) => {
                if let Some(unit) = self.unit() {
                    &unit.name
                } else if socket.holders.len() > 1 {
                    "Shared listener"
                } else {
                    socket
                        .holders
                        .first()
                        .and_then(|holder| holder.label.as_deref())
                        .unwrap_or("TCP listener")
                }
            }
        };
        display(raw)
    }

    /// Curated search context includes every known holder, never complete argv.
    pub fn search_text(&self) -> String {
        let mut values = vec![
            self.endpoint.address.to_string(),
            self.endpoint.port.to_string(),
            self.label(),
        ];
        if let Some(interface) = self.endpoint.interface {
            values.push(format!("%{interface}"));
        }
        match &self.kind {
            Kind::Socket(socket) => {
                values.push(socket.uid.to_string());
                values.extend(socket.account.iter().cloned());
                for holder in &socket.holders {
                    values.extend(
                        holder
                            .name
                            .iter()
                            .chain(&holder.executable)
                            .chain(&holder.label)
                            .chain(&holder.account)
                            .cloned(),
                    );
                    values.push(holder.identity.pid.to_string());
                    values.extend(holder.kinds.iter().map(|s| (*s).to_owned()));
                    values.extend(holder.details.iter().map(|detail| detail.value.clone()));
                    if let Some(project) = &holder.project {
                        project.search_into(&mut values);
                    }
                }
                values.extend(socket.units.iter().map(|unit| unit.name.clone()));
            }
            Kind::Publication(publication) => {
                values.push(publication.name.clone());
                values.extend(
                    publication
                        .image
                        .iter()
                        .chain(&publication.service)
                        .chain(&publication.compose)
                        .chain(&publication.directory)
                        .cloned(),
                );
                if let Some(project) = &publication.project {
                    project.search_into(&mut values);
                }
            }
        }
        display(&values.join(" ")).to_lowercase()
    }
}

impl Project {
    fn search_into(&self, values: &mut Vec<String>) {
        values.push(self.root.clone());
        if let Some(checkout) = &self.checkout {
            values.push(checkout.path.clone());
            values.extend(checkout.branch.iter().cloned());
        }
    }
}

/// Escape controls for display without rewriting identity or routing inputs.
pub fn display(value: &str) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(value.len());
    for c in value.chars() {
        if c.is_control()
            || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            let _ = write!(result, "\\u{{{:x}}}", u32::from(c));
        } else {
            result.push(c);
        }
    }
    result
}

/// Each row retains its constituent observations, including separate sockets on
/// an identical endpoint. A row is not evidence of one process or one socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub members: Vec<Member>,
}

impl Row {
    pub fn label(&self) -> String {
        let labels: BTreeSet<_> = self.members.iter().map(Member::label).collect();
        if labels.len() == 1 {
            labels.into_iter().next().unwrap_or_default()
        } else {
            "Shared listener".into()
        }
    }

    pub fn stale(&self) -> bool {
        self.members.iter().any(|member| member.stale)
    }

    pub fn search_text(&self) -> String {
        self.members
            .iter()
            .map(Member::search_text)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests;
