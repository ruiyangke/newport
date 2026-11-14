//! Bounded remote discovery and configuration reads.
use crate::backend::Backend;
use newport_protocol::git::{Page, Remote, RemoteName, Reply, Request, TotalEntries};
use std::collections::HashSet;

pub const PAGE_SIZE: usize = 20;
type NamePage = Page<RemoteName, TotalEntries>;
#[derive(Clone, Debug)]
pub struct Names {
    pub entries: Vec<String>,
    pub next_cursor: Option<String>,
    pub total: usize,
    snapshot: String,
}
impl Names {
    /// Checks one page: distinct nonempty names, a count that covers them,
    /// and a cursor only while names remain.
    pub fn from_page(page: &NamePage) -> Result<Self, String> {
        Self::bounded(page, PAGE_SIZE)
    }
    fn bounded(page: &NamePage, limit: usize) -> Result<Self, String> {
        let entries: Vec<_> = page.entries.iter().map(|n| n.name.clone()).collect();
        let total = usize::try_from(page.metadata.total_entries).unwrap_or(usize::MAX);
        if page.snapshot.is_empty()
            || entries.len() > limit
            || entries.len() > total
            || entries.iter().any(|n| n.is_empty())
            || entries.iter().collect::<HashSet<_>>().len() != entries.len()
            || page
                .next_cursor
                .as_ref()
                .is_some_and(|c| c.is_empty() || entries.is_empty() || entries.len() >= total)
        {
            return Err("Invalid remote names page. Reload remotes.".into());
        }
        Ok(Self {
            entries,
            next_cursor: page.next_cursor.clone(),
            total,
            snapshot: page.snapshot.clone(),
        })
    }
    /// Checks a first page, which must hold every name unless it continues.
    pub fn validate_first(page: &NamePage) -> Result<(), String> {
        let page = Self::from_page(page)?;
        if page.next_cursor.is_none() && page.entries.len() != page.total {
            return Err("Incomplete remote names page. Reload remotes.".into());
        }
        Ok(())
    }
    /// Appends a continuation page to the accepted names, keeping the
    /// incoming envelope. Errors leave the accepted page untouched.
    pub fn merge(
        previous: &NamePage,
        mut incoming: NamePage,
        cursor: &str,
    ) -> Result<NamePage, String> {
        let mut accumulated = Self::bounded(previous, usize::MAX)?;
        accumulated.append(Self::from_page(&incoming)?, cursor)?;
        incoming.entries = accumulated
            .entries
            .into_iter()
            .map(|name| RemoteName { name })
            .collect();
        Ok(incoming)
    }
    pub fn append(&mut self, next: Self, cursor: &str) -> Result<(), String> {
        let existing: HashSet<_> = self.entries.iter().collect();
        if self.next_cursor.as_deref() != Some(cursor)
            || next.snapshot != self.snapshot
            || next.total != self.total
            || next.next_cursor.as_deref() == Some(cursor)
            || next.entries.is_empty()
            || self.entries.len() + next.entries.len() > self.total
            || (next.next_cursor.is_none() && self.entries.len() + next.entries.len() != self.total)
            || next.entries.iter().any(|name| existing.contains(name))
        {
            return Err("The remote list changed. Reload remotes.".into());
        }
        self.entries.extend(next.entries);
        self.next_cursor = next.next_cursor;
        Ok(())
    }
    pub fn default_name(&self) -> Option<&str> {
        self.entries
            .iter()
            .find(|n| n.as_str() == "origin")
            .map(String::as_str)
            .or_else(|| {
                (self.next_cursor.is_none() && self.entries.len() == self.total)
                    .then(|| self.entries.first().map(String::as_str))
                    .flatten()
            })
    }
}
impl Backend {
    pub fn git_remote_names(
        &self,
        server: &str,
        repo: &str,
        query: &str,
        cursor: Option<&str>,
    ) -> Result<Names, String> {
        let server = uuid::Uuid::parse_str(server).map_err(|error| error.to_string())?;
        let reply = self
            .git(
                server,
                Request::RemoteNames {
                    repo_id: repo.into(),
                    filter: query.into(),
                    page_size: PAGE_SIZE,
                    cursor: cursor.map(str::to_owned),
                },
                false,
            )
            .map_err(|error| error.to_string())?;
        let Reply::RemoteNames(page) = reply else {
            return Err("Invalid remote names response.".into());
        };
        Names::from_page(&page)
    }
    /// The selected remote's configuration, once it names `name` and carries
    /// the token a write must present.
    pub fn git_remote(&self, server: &str, repo: &str, name: &str) -> Result<Remote, String> {
        let server = uuid::Uuid::parse_str(server).map_err(|error| error.to_string())?;
        let reply = self
            .git(
                server,
                Request::Remote {
                    repo_id: repo.into(),
                    name: name.into(),
                },
                false,
            )
            .map_err(|error| error.to_string())?;
        match reply {
            Reply::Remote(remote) if remote.name == name && !remote.token.is_empty() => Ok(*remote),
            _ => Err("Invalid remote configuration. Select the remote again.".into()),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{NamePage, Names, PAGE_SIZE};
    use newport_protocol::git::{Page, RemoteName, TotalEntries};
    fn wire(names: &[String], cursor: Option<&str>, snapshot: &str, total: u64) -> NamePage {
        Page {
            snapshot: snapshot.into(),
            entries: names
                .iter()
                .map(|name| RemoteName { name: name.clone() })
                .collect(),
            next_cursor: cursor.map(Into::into),
            metadata: TotalEntries {
                total_entries: total,
            },
        }
    }
    fn named(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }
    fn page(names: &[&str], cursor: Option<&str>, snapshot: &str, total: u64) -> Names {
        Names::from_page(&wire(&named(names), cursor, snapshot, total)).unwrap()
    }
    #[test]
    fn pages_reject_changed_snapshot_repeated_cursor_and_duplicates_without_losing_rows() {
        let mut first = page(&["origin"], Some("next"), "v1", 3);
        assert!(
            first
                .append(page(&["backup"], None, "v2", 3), "next")
                .is_err()
        );
        assert!(
            first
                .append(page(&["backup"], Some("next"), "v1", 3), "next")
                .is_err()
        );
        assert!(
            first
                .append(page(&["origin"], None, "v1", 3), "next")
                .is_err()
        );
        assert_eq!(first.entries, ["origin"]);
        first
            .append(page(&["backup", "upstream"], None, "v1", 3), "next")
            .unwrap();
        assert_eq!(first.entries, ["origin", "backup", "upstream"]);
        assert!(first.next_cursor.is_none());
    }
    #[test]
    fn bounded_pages_and_default_destination_do_not_guess_from_a_partial_prefix() {
        assert_eq!(page(&["backup"], Some("next"), "s", 2).default_name(), None);
        assert_eq!(
            page(&["backup"], None, "s", 1).default_name(),
            Some("backup")
        );
        assert_eq!(
            page(&["backup", "origin"], None, "s", 2).default_name(),
            Some("origin")
        );
        let oversized: Vec<_> = (0..=PAGE_SIZE).map(|n| format!("remote-{n}")).collect();
        assert!(Names::from_page(&wire(&oversized, None, "s", PAGE_SIZE as u64 + 1)).is_err());
    }
    #[test]
    fn full_page_merge_preserves_rows_and_rejects_incomplete_or_duplicate_results() {
        let names: Vec<_> = (0..20).map(|i| format!("remote-{i}")).collect();
        let first = wire(&names, Some("next"), "s", 21);
        assert!(Names::validate_first(&first).is_ok());
        let last = wire(&named(&["remote-20"]), None, "s", 21);
        assert!(Names::validate_first(&last).is_err());
        let merged = Names::merge(&first, last.clone(), "next").unwrap();
        assert_eq!(merged.entries.len(), 21);
        assert!(merged.next_cursor.is_none());
        let repeated = wire(&named(&["remote-0"]), None, "s", 21);
        assert!(Names::merge(&first, repeated, "next").is_err());
        assert_eq!(first.entries.len(), 20);
    }
}
