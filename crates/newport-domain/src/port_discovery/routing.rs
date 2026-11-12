use super::*;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Destination {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationError {
    Empty,
    ChooseMember,
    Changed,
    Stale,
    RemoteInterface,
}
impl std::fmt::Display for DestinationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Empty => "This row has no destination.",
            Self::ChooseMember => "Choose which server address to forward.",
            Self::Changed => {
                "This listener changed. Review its current addresses before forwarding."
            }
            Self::Stale => "Refresh this listener before creating a forward.",
            Self::RemoteInterface => {
                "This listener needs a server network interface that forwarding cannot select yet."
            }
        })
    }
}
impl std::error::Error for DestinationError {}

impl Endpoint {
    pub fn destination(self) -> Result<Destination, DestinationError> {
        if self.port == 0 {
            return Err(DestinationError::Empty);
        }
        // A local interface index is never a substitute for the remote index.
        // Retain the observation but disable creation until routing supports it.
        if self.interface.is_some()
            || matches!(self.address, IpAddr::V6(ip) if ip.is_unicast_link_local())
        {
            return Err(DestinationError::RemoteInterface);
        }
        let host = match self.address {
            IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".into(),
            IpAddr::V6(ip) if ip.is_unspecified() => "::1".into(),
            ip => ip.to_string(),
        };
        Ok(Destination {
            host,
            port: self.port,
        })
    }
}

/// Loopback in whichever family the server reaches: a forward to `localhost`
/// tries `127.0.0.1`, then `::1`. Where the server has IPv6 off, or its SSH
/// server is IPv4-only, only the first works; an IPv6-only server needs the
/// second.
const BOTH_LOOPBACKS: &str = "localhost";

impl Row {
    /// A listener that accepts both families, a dual-stack socket on `::` or a
    /// verified loopback pair, defaults to `localhost`. Other distinct
    /// destinations require a choice; explicit selections retain their family.
    pub fn destination(
        &self,
        selected: Option<Reference>,
    ) -> Result<Destination, DestinationError> {
        if let Some(selected) = selected {
            let member = self
                .members
                .iter()
                .find(|member| member.reference == selected)
                .ok_or(DestinationError::Changed)?;
            if member.stale {
                return Err(DestinationError::Stale);
            }
            return member.endpoint.destination();
        }
        if self.stale() {
            return Err(DestinationError::Stale);
        }
        let endpoints: BTreeSet<_> = self.members.iter().map(|member| member.endpoint).collect();
        if endpoints.len() > 1 {
            if !super::grouping::shares_attribution(&self.members) {
                return Err(DestinationError::ChooseMember);
            }
            let destinations = endpoints
                .into_iter()
                .map(Endpoint::destination)
                .collect::<Result<BTreeSet<_>, _>>()
                .map_err(|_| DestinationError::ChooseMember)?;
            let mut destinations = destinations.into_iter();
            let first = destinations.next().ok_or(DestinationError::Empty)?;
            match destinations.next() {
                None => return Ok(first),
                Some(second)
                    if first.host == "127.0.0.1"
                        && second.host == "::1"
                        && first.port == second.port
                        && destinations.next().is_none() =>
                {
                    return Ok(Destination {
                        host: BOTH_LOOPBACKS.into(),
                        port: first.port,
                    });
                }
                Some(_) => return Err(DestinationError::ChooseMember),
            }
        }
        let destination = endpoints
            .into_iter()
            .next()
            .ok_or(DestinationError::Empty)?
            .destination()?;
        Ok(if self.members.iter().all(Member::dual_stack) {
            Destination {
                host: BOTH_LOOPBACKS.into(),
                ..destination
            }
        } else {
            destination
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Identity {
    Socket {
        scope: Arc<Scope>,
        endpoint: Endpoint,
        cookie: u64,
        inode: u64,
        uid: u32,
        holders: Vec<Process>,
    },
    Publication {
        scope: Arc<Scope>,
        endpoint: Endpoint,
        engine: String,
        container: String,
        target: u16,
        address: Option<IpAddr>,
    },
}

fn identity(member: &Member) -> Option<Identity> {
    if member.stale {
        return None;
    }
    let scope = member.scope.clone()?;
    Some(match &member.kind {
        Kind::Socket(socket) => Identity::Socket {
            scope,
            endpoint: member.endpoint,
            cookie: socket.cookie?,
            inode: socket.inode,
            uid: socket.uid,
            holders: socket
                .holders
                .iter()
                .map(|holder| holder.identity)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        },
        Kind::Publication(publication) => Identity::Publication {
            scope,
            endpoint: member.endpoint,
            engine: publication.engine.clone()?,
            container: publication.container.clone(),
            target: publication.target_port,
            address: publication.target_address,
        },
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProjectIdentity {
    root: String,
    checkout: Option<String>,
}

fn project_identity(member: &Member) -> Option<ProjectIdentity> {
    let project = member.project()?;
    let path = |path: &str| {
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            "/".into()
        } else {
            trimmed.to_owned()
        }
    };
    Some(ProjectIdentity {
        root: path(&project.root),
        checkout: project
            .checkout
            .as_ref()
            .map(|checkout| path(&checkout.path)),
    })
}

/// Strong choices survive refresh only while socket, holder and project
/// identity agree. Weaker observations remain selectable within their original
/// immutable view, but a refresh always requires another explicit choice.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    identities: HashSet<(Identity, Option<ProjectIdentity>)>,
    observations: HashMap<(Observation, Reference), Member>,
}
impl Selection {
    pub fn capture<'a>(members: impl IntoIterator<Item = &'a Member>) -> Self {
        let mut selection = Self::default();
        for member in members.into_iter().filter(|member| !member.stale) {
            if let Some(identity) = identity(member) {
                selection
                    .identities
                    .insert((identity, project_identity(member)));
            } else {
                selection.observations.insert(
                    (member.observation.clone(), member.reference),
                    member.clone(),
                );
            }
        }
        selection
    }
    pub fn contains(&self, member: &Member) -> bool {
        identity(member).is_some_and(|identity| {
            self.identities
                .contains(&(identity, project_identity(member)))
        }) || self
            .observations
            .get(&(member.observation.clone(), member.reference))
            == Some(member)
    }
}

/// What a scan covers.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub fresh: bool,
}
