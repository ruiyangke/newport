//! Typed discovery rows alongside the existing saved-forward configuration.
use super::Worktree;
use crate::{port_context, port_discovery as d};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub enum Port {
    Observed(d::Row),
    Saved {
        definition: Value,
        worktree: Option<Worktree>,
    },
}

impl Port {
    pub fn saved(definition: Value) -> Self {
        Self::Saved {
            definition,
            worktree: None,
        }
    }
    pub fn definition(&self) -> Option<&Value> {
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
            .is_some_and(|value| value["direction"] == "remote")
    }
    pub fn number(&self) -> Option<u16> {
        match self {
            Self::Observed(row) => row.members.first().map(|member| member.endpoint.port),
            Self::Saved { definition, .. } => definition[if self.reverse() {
                "localPort"
            } else {
                "remotePort"
            }]
            .as_u64()
            .and_then(|port| u16::try_from(port).ok()),
        }
    }
    pub fn worktree(&self) -> Option<Worktree> {
        match self {
            Self::Saved { worktree, .. } => worktree.clone(),
            Self::Observed(row) => {
                let first = row.members.first()?.project()?.checkout.as_ref()?;
                row.members
                    .iter()
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
        let row = self.row()?;
        let first = row.members.first()?.project()?;
        row.members
            .iter()
            .all(|member| member.project() == Some(first))
            .then_some(first)
    }
    pub fn publication(&self) -> Option<&d::Publication> {
        let row = self.row()?;
        let d::Kind::Publication(first) = &row.members.first()?.kind else {
            return None;
        };
        row.members.iter().all(|member| matches!(&member.kind, d::Kind::Publication(value) if value.engine == first.engine && value.container == first.container)).then_some(first)
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
            return port_context::container_kinds(publication.image.as_deref());
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
    pub fn detail(&self, base: Option<&str>) -> String {
        let Some(row) = self.row() else {
            return String::new();
        };
        if row.stale() {
            return "Last observed".into();
        }
        if let Some(publication) = self.publication() {
            return if publication.running == Some(false) {
                "Published by Docker · Container stopped"
            } else {
                "Published by Docker"
            }
            .into();
        }
        let holders: BTreeSet<_> = row
            .members
            .iter()
            .filter_map(|member| match &member.kind {
                d::Kind::Socket(socket) => Some(socket),
                _ => None,
            })
            .flat_map(|socket| &socket.holders)
            .map(|holder| holder.identity)
            .collect();
        if holders.len() > 1 {
            return format!("{} processes", holders.len());
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
        if row
            .members
            .iter()
            .any(|member| member.endpoint.address.is_unspecified())
        {
            parts.push("all interfaces".into());
        }
        d::display(&parts.join(" · "))
    }
    pub fn search_text(&self) -> String {
        match self {
            Self::Observed(row) => row.search_text(),
            Self::Saved {
                definition,
                worktree,
            } => {
                let mut parts: Vec<_> = ["name", "localPort", "remotePort", "remoteHost"]
                    .iter()
                    .filter_map(|field| definition.get(field))
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string())
                    })
                    .collect();
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
            Self::Saved { definition, .. } => definition["id"].as_str().unwrap_or_default().into(),
            Self::Observed(row) => row
                .members
                .iter()
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
