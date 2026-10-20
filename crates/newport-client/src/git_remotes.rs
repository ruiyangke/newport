//! Bounded remote discovery and configuration reads.
use crate::backend::Backend;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

pub const PAGE_SIZE: usize = 20;
#[derive(Clone, Debug)]
pub struct Names {
    pub entries: Vec<String>,
    pub next_cursor: Option<String>,
    pub total: usize,
    snapshot: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePage {
    entries: Vec<Name>,
    next_cursor: Option<String>,
    snapshot: String,
    metadata: Metadata,
}
#[derive(Deserialize)]
struct Name {
    name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    total_entries: usize,
}
impl Names {
    pub fn decode(value: Value) -> Result<Self, String> {
        Self::decode_bounded(value, PAGE_SIZE)
    }
    fn decode_bounded(value: Value, limit: usize) -> Result<Self, String> {
        let page: WirePage =
            serde_json::from_value(value).map_err(|_| "Invalid remote names response.")?;
        let entries: Vec<_> = page.entries.into_iter().map(|n| n.name).collect();
        if page.snapshot.is_empty()
            || entries.len() > limit
            || entries.len() > page.metadata.total_entries
            || entries.iter().any(|n| n.is_empty())
            || entries.iter().collect::<HashSet<_>>().len() != entries.len()
            || page.next_cursor.as_ref().is_some_and(|c| {
                c.is_empty() || entries.is_empty() || entries.len() >= page.metadata.total_entries
            })
        {
            return Err("Invalid remote names page. Reload remotes.".into());
        }
        Ok(Self {
            entries,
            next_cursor: page.next_cursor,
            total: page.metadata.total_entries,
            snapshot: page.snapshot,
        })
    }
    pub fn validate_first(value: &Value) -> Result<(), String> {
        let page = Self::decode(value.clone())?;
        if page.next_cursor.is_none() && page.entries.len() != page.total {
            return Err("Incomplete remote names page. Reload remotes.".into());
        }
        Ok(())
    }
    pub fn merge_values(
        previous: &Value,
        mut incoming: Value,
        cursor: &str,
    ) -> Result<Value, String> {
        let mut accumulated = Self::decode_bounded(previous.clone(), usize::MAX)?;
        accumulated.append(Self::decode(incoming.clone())?, cursor)?;
        incoming["entries"] = json!(
            accumulated
                .entries
                .into_iter()
                .map(|name| json!({"name":name}))
                .collect::<Vec<_>>()
        );
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
        let mut params = json!({"repoId":repo,"filter":query,"pageSize":PAGE_SIZE});
        if let Some(cursor) = cursor {
            params["cursor"] = json!(cursor);
        }
        Names::decode(self.call(
            "git_request",
            json!({"serverId":server,"request":{"method":"repo.remote_names","params":params}}),
        )?)
    }
    pub fn git_remote(&self, server: &str, repo: &str, name: &str) -> Result<Value, String> {
        let result = self.call("git_request", json!({"serverId":server,"request":{"method":"repo.remote","params":{"repoId":repo,"name":name}}}))?;
        if result["name"].as_str() != Some(name)
            || result["token"].as_str().is_none_or(str::is_empty)
            || ["url", "pushUrl"]
                .iter()
                .any(|key| !result[key].is_null() && !result[key].is_string())
        {
            return Err("Invalid remote configuration. Select the remote again.".into());
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::{Names, PAGE_SIZE};
    use serde_json::json;
    fn page(names: &[&str], cursor: Option<&str>, snapshot: &str, total: usize) -> Names {
        Names::decode(json!({"entries":names.iter().map(|n|json!({"name":n})).collect::<Vec<_>>(),"nextCursor":cursor,"snapshot":snapshot,"metadata":{"totalEntries":total}})).unwrap()
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
        assert!(Names::decode(json!({"entries":(0..=PAGE_SIZE).map(|n|json!({"name":format!("remote-{n}")})).collect::<Vec<_>>(),"nextCursor":null,"snapshot":"s","metadata":{"totalEntries":PAGE_SIZE+1}})).is_err());
    }
    #[test]
    fn full_page_merge_preserves_rows_and_rejects_incomplete_or_duplicate_results() {
        let first = json!({"entries":(0..20).map(|i|json!({"name":format!("remote-{i}")})).collect::<Vec<_>>(),"snapshot":"s","nextCursor":"next","metadata":{"totalEntries":21}});
        assert!(Names::validate_first(&first).is_ok());
        let last = json!({"entries":[{"name":"remote-20"}],"snapshot":"s","nextCursor":null,"metadata":{"totalEntries":21}});
        assert!(Names::validate_first(&last).is_err());
        let merged = Names::merge_values(&first, last.clone(), "next").unwrap();
        assert_eq!(merged["entries"].as_array().unwrap().len(), 21);
        assert!(merged["nextCursor"].is_null());
        let mut repeated = last;
        repeated["entries"][0]["name"] = json!("remote-0");
        assert!(Names::merge_values(&first, repeated, "next").is_err());
        assert_eq!(first["entries"].as_array().unwrap().len(), 20);
    }
}
