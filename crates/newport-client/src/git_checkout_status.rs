//! Explicit, bounded status reads for the loaded checkouts of one repository.
//! Dropping a batch stops queued reads; resource generations reject late results.
use crate::resources::Resources;
use serde_json::Value;
use std::collections::{HashSet, VecDeque};

pub const PREFIX: &str = "git-checkout-status:";
pub fn key(entry: &Value) -> Option<String> {
    let bytes = entry["path"]["bytesB64"]
        .as_str()
        .filter(|s| !s.is_empty())?;
    (entry["state"] == "available" && matches!(entry["kind"].as_str(), Some("main" | "linked")))
        .then(|| format!("{PREFIX}{bytes}"))
}

pub struct StatusRead {
    pub key: String,
    pub path: Value,
}
#[derive(Default)]
pub struct StatusChecks {
    queue: VecDeque<StatusRead>,
    active: HashSet<String>,
}
impl StatusChecks {
    pub fn new(entries: &[Value]) -> Self {
        let mut seen = HashSet::new();
        Self {
            queue: entries
                .iter()
                .filter_map(|entry| {
                    let key = key(entry)?;
                    seen.insert(key.clone()).then(|| StatusRead {
                        key,
                        path: entry["path"].clone(),
                    })
                })
                .collect(),
            ..Default::default()
        }
    }
    /// Failed reads finish their slot just like successful reads; retry is explicit.
    pub fn take_ready(&mut self, resources: &Resources) -> Vec<StatusRead> {
        self.active
            .retain(|key| resources.get(key).is_some_and(|r| r.pending));
        let mut ready = Vec::new();
        while self.active.len() < 2 {
            let Some(next) = self.queue.pop_front() else {
                break;
            };
            self.active.insert(next.key.clone());
            ready.push(next);
        }
        ready
    }
    pub fn is_running(&self) -> bool {
        !self.queue.is_empty() || !self.active.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn entry(path: &str) -> Value {
        json!({"kind":"linked","state":"available","path":{"display":path,"bytesB64":path}})
    }
    #[test]
    fn only_loaded_available_checkouts_are_scheduled_once_with_two_in_flight() {
        let mut missing = entry("missing");
        missing["state"] = json!("missing");
        let mut bare = entry("bare");
        bare["kind"] = json!("bare");
        let mut batch = StatusChecks::new(&[
            entry("a"),
            entry("b"),
            entry("c"),
            entry("a"),
            missing,
            bare,
            json!({}),
        ]);
        let mut resources = Resources::default();
        let ready = batch.take_ready(&resources);
        assert_eq!(ready.len(), 2);
        let a = resources.begin(&ready[0].key).unwrap();
        let b = resources.begin(&ready[1].key).unwrap();
        assert!(batch.take_ready(&resources).is_empty());
        resources.complete(&ready[0].key, a, Err("unreachable".into()));
        let next = batch.take_ready(&resources);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].path, entry("c")["path"]);
        let c = resources.begin(&next[0].key).unwrap();
        resources.complete(&ready[1].key, b, Ok(json!({})));
        assert!(batch.take_ready(&resources).is_empty());
        assert!(batch.is_running());
        resources.complete(&next[0].key, c, Ok(json!({})));
        assert!(batch.take_ready(&resources).is_empty());
        assert!(!batch.is_running());
    }
    #[test]
    fn closing_or_replacing_scope_does_not_accept_old_status_completions() {
        let mut batch = StatusChecks::new(&[entry("a"), entry("b"), entry("c")]);
        let mut resources = Resources::default();
        let ready = batch.take_ready(&resources);
        let old = resources.begin(&ready[0].key).unwrap();
        drop(batch);
        resources.invalidate_prefix(PREFIX);
        let fresh = resources.begin(&ready[0].key).unwrap();
        assert!(!resources.complete(&ready[0].key, old, Ok(json!({"totalEntries":0}))));
        assert!(resources.get(&ready[0].key).unwrap().pending);
        assert!(resources.complete(&ready[0].key, fresh, Err("failed".into())));
        assert!(resources.get(&ready[0].key).unwrap().value.is_null());
    }
}
