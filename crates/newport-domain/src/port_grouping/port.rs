//! Typed discovery rows alongside the existing saved-forward configuration.
use super::Worktree;
use crate::{port_context, port_discovery as d};
use newport_protocol::desktop::configuration::{Direction, Tunnel};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub enum Port {
    Observed(d::Row),
    Saved {
        definition: Tunnel,
        worktree: Option<Worktree>,
    },
}

impl Port {
    pub fn saved(definition: Tunnel) -> Self {
        Self::Saved {
            definition,
            worktree: None,
        }
    }
    pub fn definition(&self) -> Option<&Tunnel> {
        match self {
            Self::Saved { definition, .. } => Some(definition),
            Self::Observed(_) => None,
        }
    }
    pub fn row(&self) -> Option<&d::Row> {
        match self {
            Self::Observed(row) => Some(row),
            Self::Saved { .. } => None,
        }
    }
    pub fn reverse(&self) -> bool {
        self.definition()
            .is_some_and(|value| value.direction == Direction::Remote)
    }
    pub fn number(&self) -> Option<u16> {
        match self {
            Self::Observed(row) => row.members.first().map(|member| member.endpoint.port),
            Self::Saved { definition, .. } => Some(if self.reverse() {
                definition.local_port
            } else {
                definition.remote_port
            }),
        }
    }
    pub fn worktree(&self) -> Option<Worktree> {
        match self {
            Self::Saved { worktree, .. } => worktree.clone(),
            Self::Observed(row) => {
                let mut members = row.identifying_members();
                let first = members.clone().next()?.project()?.checkout.as_ref()?;
                members
                    .all(|member| {
                        member
                            .project()
                            .and_then(|project| project.checkout.as_ref())
                            == Some(first)
                    })
                    .then(|| Worktree {
                        path: super::trim(&first.path).into(),
                        branch: first.branch.clone(),
                        main: first.main,
                    })
            }
        }
    }
    pub fn project(&self) -> Option<&d::Project> {
        let mut members = self.row()?.identifying_members();
        let first = members.clone().next()?.project()?;
        members
            .all(|member| member.project() == Some(first))
            .then_some(first)
    }
    /// The container a row publishes. Host sockets in the row are its proxies.
    pub fn publication(&self) -> Option<&d::Publication> {
        let mut publications = self
            .row()?
            .identifying_members()
            .filter_map(|member| match &member.kind {
                d::Kind::Publication(value) => Some(value),
                d::Kind::Socket(_) => None,
            });
        let first = publications.next()?;
        publications
            .all(|value| value.engine == first.engine && value.container == first.container)
            .then_some(first)
    }
    pub fn system(&self) -> bool {
        self.row().is_some_and(|row| {
            !row.members.is_empty()
                && row
                    .members
                    .iter()
                    .all(|member| member.unit().is_some_and(|unit| unit.manager_uid.is_none()))
        })
    }
    pub fn label(&self) -> String {
        self.row()
            .map(d::Row::label)
            .unwrap_or_else(|| "Saved forward".into())
    }
    pub fn forward_name(&self) -> String {
        let name = self.label();
        match self
            .worktree()
            .filter(|worktree| worktree.main == Some(false))
            .and_then(|worktree| worktree.branch)
        {
            Some(branch) => format!("{name} · {}", d::display(&branch)),
            None => name,
        }
    }
    pub fn destination(
        &self,
        selected: Option<d::Reference>,
    ) -> Result<d::Destination, d::DestinationError> {
        self.row()
            .ok_or(d::DestinationError::Empty)?
            .destination(selected)
    }
    pub fn addresses(&self) -> Vec<String> {
        self.row()
            .into_iter()
            .flat_map(|row| &row.members)
            .map(|member| {
                let endpoint = member.endpoint;
                match endpoint.interface {
                    Some(interface) => format!("{}%{interface}", endpoint.address),
                    None => endpoint.address.to_string(),
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn kinds(&self) -> Vec<&'static str> {
        if let Some(publication) = self.publication() {
            return port_context::container_kinds(
                publication.image.as_deref(),
                publication.engine_kind,
            );
        }
        let mut kinds = Vec::new();
        for holder in self
            .row()
            .into_iter()
            .flat_map(|row| &row.members)
            .filter_map(|member| match &member.kind {
                d::Kind::Socket(socket) => Some(socket),
                _ => None,
            })
            .flat_map(|socket| &socket.holders)
        {
            for kind in &holder.kinds {
                if !kinds.contains(kind) {
                    kinds.push(*kind);
                }
            }
        }
        kinds
    }
    /// Where a container's published port goes, which names the row along
    /// with its container; [`Self::detail`] leaves it out.
    pub fn target(&self) -> Option<String> {
        self.publication()
            .map(|publication| format!("to container port {}", publication.target_port))
    }
    pub fn detail(&self, base: Option<&str>) -> String {
        let Some(row) = self.row() else {
            return String::new();
        };
        if row.stale() {
            return "Last observed".into();
        }
        if let Some(publication) = self.publication() {
            let mut parts = Vec::new();
            if publication.running == Some(false) {
                parts.push("Container stopped".into());
            }
            parts.extend(families(row));
            return parts.join(" · ");
        }
        // Processes of one program, such as an IPv4 and an IPv6 listener,
        // describe that program rather than a count.
        let programs: BTreeSet<_> = row
            .members
            .iter()
            .filter_map(|member| match &member.kind {
                d::Kind::Socket(socket) => Some(socket),
                _ => None,
            })
            .flat_map(|socket| &socket.holders)
            .map(
                |holder| match holder.name.as_ref().or(holder.executable.as_ref()) {
                    Some(name) => (Some(name), holder.uid, None),
                    None => (None, holder.uid, Some(holder.identity)),
                },
            )
            .collect();
        if programs.len() > 1 {
            return format!("{} programs", programs.len());
        }
        let mut parts = Vec::new();
        for holder in row
            .members
            .iter()
            .filter_map(|member| match &member.kind {
                d::Kind::Socket(socket) => Some(socket),
                _ => None,
            })
            .flat_map(|socket| &socket.holders)
        {
            for detail in &holder.details {
                if Some(detail.value.as_str()) == base {
                    continue;
                }
                let short = if let Some(relative) =
                    base.and_then(|base| detail.value.strip_prefix(base)?.strip_prefix('/'))
                {
                    format!("{relative}/")
                } else {
                    port_context::short_path(&detail.value, &[holder.account.as_deref()], 40)
                };
                if !parts.contains(&short) {
                    parts.push(short);
                }
            }
        }
        let label = self.label();
        for kind in self.kinds() {
            if kind != label {
                parts.push(kind.into());
            }
        }
        parts.extend(families(row));
        d::display(&parts.join(" · "))
    }
    pub fn search_text(&self) -> String {
        match self {
            Self::Observed(row) => row.search_text(),
            Self::Saved {
                definition,
                worktree,
            } => {
                let mut parts = vec![
                    definition.name.clone(),
                    definition.local_port.to_string(),
                    definition.remote_port.to_string(),
                    definition.remote_host.clone(),
                ];
                if let Some(worktree) = worktree {
                    parts.push(worktree.path.clone());
                    parts.extend(worktree.branch.iter().cloned());
                }
                d::display(&parts.join(" ")).to_lowercase()
            }
        }
    }
    pub fn key(&self) -> String {
        match self {
            Self::Saved { definition, .. } => definition.id.to_string(),
            // A container's row keeps its key while proxy sockets join or
            // leave it between scans.
            Self::Observed(row) => row
                .identifying_members()
                .map(|member| match &member.kind {
                    d::Kind::Socket(socket) => format!(
                        "{:?}:{:?}:{}:{:?}:{}",
                        member.scope,
                        member.endpoint,
                        socket.inode,
                        socket.cookie,
                        if socket.cookie.is_none() {
                            member.reference.generation
                        } else {
                            0
                        }
                    ),
                    d::Kind::Publication(publication) => format!(
                        "{:?}:{:?}:{}:{:?}:{}:{:?}:{}",
                        member.scope,
                        publication.engine,
                        publication.container,
                        member.endpoint,
                        publication.target_port,
                        publication.target_address,
                        if publication.engine.is_none() {
                            member.reference.generation
                        } else {
                            0
                        }
                    ),
                })
                .collect::<Vec<_>>()
                .join("|"),
        }
    }
}

/// Where a row that listens on a wildcard address accepts connections: a
/// family it covers entirely, or its specific addresses in that family.
fn families(row: &d::Row) -> Option<String> {
    use std::net::IpAddr;
    let addresses: BTreeSet<_> = row.members.iter().map(|m| m.endpoint.address).collect();
    if !addresses.iter().any(IpAddr::is_unspecified) {
        return None;
    }
    let ipv4_any = row.members.iter().any(|member| {
        matches!(member.endpoint.address, IpAddr::V4(ip) if ip.is_unspecified())
            || matches!(&member.kind, d::Kind::Socket(socket)
                if socket.ipv6_only == Some(false) && member.endpoint.address.is_unspecified())
    });
    let ipv6_any = addresses
        .iter()
        .any(|address| address.is_ipv6() && address.is_unspecified());
    let ipv4: Vec<_> = addresses
        .iter()
        .filter(|address| address.is_ipv4())
        .collect();
    // A `::` socket of unknown mode may or may not also accept IPv4.
    if ipv6_any && !ipv4_any && ipv4.is_empty() {
        let only = row.members.iter().all(|member| {
            matches!(&member.kind, d::Kind::Socket(socket) if socket.ipv6_only == Some(true))
                || !member.endpoint.address.is_unspecified()
        });
        return Some(if only { "IPv6" } else { "all addresses" }.into());
    }
    let describe = |any: bool, ipv6: bool| {
        if any {
            vec![if ipv6 { "IPv6" } else { "IPv4" }.to_owned()]
        } else {
            addresses
                .iter()
                .filter(|address| address.is_ipv6() == ipv6)
                .map(ToString::to_string)
                .collect()
        }
    };
    let mut parts = describe(ipv4_any, false);
    parts.extend(describe(ipv6_any, true));
    Some(parts.join(" and "))
}
