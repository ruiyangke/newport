//! Bounded recovery of snapshot-bound diff reads. Never applies to writes.
use serde_json::Value;

/// Resolve a selected comparison against a refreshed status page. Identity is
/// the exact path, never lossy display text or the old snapshot's entry token.
pub fn matching_entry(page: &Value, path: &Value, staged: bool) -> Option<Value> {
    let bytes = path["bytesB64"]
        .as_str()
        .filter(|bytes| !bytes.is_empty())?;
    page["entries"]
        .as_array()?
        .iter()
        .find(|entry| {
            entry["path"]["bytesB64"].as_str() == Some(bytes)
                && entry["entryId"].as_str().is_some_and(|id| !id.is_empty())
                && entry["conflicted"] != true
                && if staged {
                    entry["staged"] == true
                } else {
                    entry["unstaged"] == true || entry["untracked"] == true
                }
        })
        .cloned()
}

#[derive(Default)]
pub struct Recovery {
    attempts: u8,
    target: Option<(Value, bool)>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Ignore,
    Refresh,
    Exhausted,
}
impl Recovery {
    pub fn begin(&mut self, code: Option<&str>, entry: &Value, staged: bool) -> Decision {
        if !matches!(code, Some("STALE_SNAPSHOT" | "STALE_ENTRY"))
            || self.target.is_some()
            || entry["path"]["bytesB64"].as_str().is_none_or(str::is_empty)
        {
            return Decision::Ignore;
        }
        if self.attempts >= 3 {
            return Decision::Exhausted;
        }
        self.attempts += 1;
        self.target = Some((entry["path"].clone(), staged));
        Decision::Refresh
    }
    pub fn pending(&self) -> bool {
        self.target.is_some()
    }
    /// Rebind only to a current, exact byte path and the same comparison side.
    /// A removed/clean/filtered file is not evidence for a different selection.
    /// Do not eagerly fetch every status page to find a disappeared row.
    pub fn resolve(&mut self, page: &Value) -> Option<(Value, bool)> {
        let (path, staged) = self.target.take()?;
        matching_entry(page, &path, staged).map(|entry| (entry, staged))
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn entry(id: &str) -> Value {
        json!({"entryId":id,"path":{"display":"a","bytesB64":"YQ=="},"unstaged":true})
    }
    #[test]
    fn moving_repository_is_bounded_even_when_snapshot_stays_the_same() {
        let mut recovery = Recovery::default();
        for _ in 0..3 {
            assert_eq!(
                recovery.begin(Some("STALE_ENTRY"), &entry("old"), false),
                Decision::Refresh
            );
            assert_eq!(
                recovery.begin(Some("STALE_ENTRY"), &entry("old"), false),
                Decision::Ignore
            );
            assert_eq!(
                recovery.resolve(&json!({"snapshot":"same","entries":[entry("new")]})),
                Some((entry("new"), false))
            );
        }
        assert_eq!(
            recovery.begin(Some("STALE_SNAPSHOT"), &entry("new"), false),
            Decision::Exhausted
        );
        recovery.reset();
        assert_eq!(
            recovery.begin(Some("STALE_SNAPSHOT"), &entry("new"), false),
            Decision::Refresh
        );
    }
    #[test]
    fn errors_and_display_paths_cannot_redirect_recovery() {
        let mut recovery = Recovery::default();
        for code in [
            None,
            Some("CONNECTION_LOST"),
            Some("OUTCOME_UNKNOWN"),
            Some("AUTH_FAILED"),
        ] {
            assert_eq!(recovery.begin(code, &entry("old"), false), Decision::Ignore);
        }
        let mut other = entry("other");
        other["path"]["bytesB64"] = json!("/w==");
        for rows in [
            json!([]),
            json!([other]),
            json!([{"entryId":"staged","path":{"bytesB64":"YQ=="},"staged":true}]),
        ] {
            assert_eq!(
                recovery.begin(Some("STALE_ENTRY"), &entry("old"), false),
                Decision::Refresh
            );
            assert!(recovery.resolve(&json!({"entries":rows})).is_none());
            assert!(!recovery.pending());
        }
    }
}
