//! Current discovery and explicitly stale observations retained after partial scans.
use crate::monitor::{
    Invalid,
    ports::{
        MAX_LISTENERS,
        discovery::{self, Outcome, Report, Snapshot},
    },
};
use std::collections::BTreeSet;

pub const MAX_RETAINED: usize = 64;
pub const MAX_ROWS: usize = MAX_LISTENERS * 2;
pub const MAX_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_FIELDS: usize = 192 * 1024;
pub const ENVELOPE_BYTES: usize = 16 * 1024;
pub const ENVELOPE_FIELDS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub current: Snapshot,
    /// These are subsets of earlier scans, never evidence of current presence or absence.
    pub retained: Vec<Snapshot>,
    pub retention: Report,
}

impl Discovery {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.current.validate()?;
        if self.retained.len() > MAX_RETAINED {
            return Err(Invalid("retained discovery count"));
        }
        let mut generations = BTreeSet::new();
        let mut counts = [0usize; 5];
        for snapshot in std::iter::once(&self.current).chain(&self.retained) {
            snapshot.validate()?;
            for (count, next) in counts.iter_mut().zip([
                snapshot.sockets.len(),
                snapshot.processes.len(),
                snapshot.services.len(),
                snapshot.publications.len(),
                snapshot
                    .processes
                    .iter()
                    .filter(|p| p.details.is_some())
                    .count(),
            ]) {
                *count = count.saturating_add(next);
            }
        }
        if counts[..4].iter().any(|count| *count > MAX_ROWS)
            || counts[4] > discovery::MAX_DETAILS * 2
        {
            return Err(Invalid("combined discovery count"));
        }
        for snapshot in &self.retained {
            if !same_scope(&self.current, snapshot)
                || snapshot.generation >= self.current.generation
                || !generations.insert(snapshot.generation)
                || (snapshot.sockets.is_empty() && snapshot.publications.is_empty())
            {
                return Err(Invalid("retained discovery scope or generation"));
            }
        }
        if !matches!(self.retention.outcome, Outcome::Complete | Outcome::Partial)
            || self.retention.issues.len() > discovery::MAX_ISSUES
            || (self.retention.outcome == Outcome::Partial && self.retention.issues.is_empty())
            || self.retention.issues.iter().any(|issue| {
                issue.count == 0
                    || issue.message.len() > discovery::MAX_DIAGNOSTIC
                    || issue.message.contains('\0')
            })
        {
            return Err(Invalid("discovery retention report"));
        }
        Ok(())
    }

    /// A complete empty inventory is useful; an empty failed inventory is not.
    pub fn has_current_inventory(&self) -> bool {
        !self.current.sockets.is_empty()
            || !self.current.publications.is_empty()
            || [
                &self.current.coverage.ipv4,
                &self.current.coverage.ipv6,
                &self.current.coverage.containers,
            ]
            .iter()
            .any(|report| report.outcome == Outcome::Complete)
    }
}

pub fn same_scope(a: &Snapshot, b: &Snapshot) -> bool {
    a.scope.network.is_some()
        && a.scope.processes.is_some()
        && a.scope.boot_id.is_some()
        && a.scope == b.scope
}

#[cfg(feature = "protobuf")]
impl Discovery {
    pub fn validate_encoding(&self) -> Result<(), crate::monitor::wire::Error> {
        self.validate()?;
        // Reserve the wrapper and retention diagnostics before accounting for imported snapshots.
        let mut bytes = ENVELOPE_BYTES;
        let mut fields = ENVELOPE_FIELDS;
        for snapshot in std::iter::once(&self.current).chain(&self.retained) {
            let size = discovery::wire::measure(snapshot)?;
            bytes += size.bytes + 8;
            fields += size.fields + 1;
            if bytes > MAX_BYTES || fields > MAX_FIELDS {
                return Err(Invalid("combined discovery encoding budget").into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
