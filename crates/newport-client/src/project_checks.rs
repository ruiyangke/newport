//! Explicit, sequential status checks for a captured project-library scope.
//! Dropping a batch stops scheduling; it never replays or cancels a shared read.
use crate::resources::Resources;
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};

pub fn status_key(project: &Value) -> String {
    format!(
        "project-status:{}",
        json!([
            project["serverId"],
            project["id"],
            project["path"]["bytesB64"]
        ])
    )
}

pub struct Batch {
    server: String,
    query: String,
    queue: VecDeque<Value>,
    waiting: Option<String>,
    stopped: bool,
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, PartialEq)]
pub enum Advance {
    Read { key: String, arguments: Value },
    Waiting,
    Finished,
    Cancelled,
}

impl Batch {
    pub fn new(server: String, query: String, projects: Vec<Value>) -> Self {
        let mut seen = HashSet::new();
        let queue: VecDeque<_> = projects
            .into_iter()
            .filter(|p| p["serverId"].as_str() == Some(&server))
            .filter(|p| seen.insert(status_key(p)))
            .collect();
        Self {
            server,
            query,
            total: queue.len(),
            queue,
            waiting: None,
            stopped: false,
            completed: 0,
        }
    }

    /// The caller supplies current view scope, not the captured click closure.
    /// A scope change is terminal even if the same view is reopened later.
    pub fn advance(
        &mut self,
        resources: &Resources,
        server: &str,
        query: &str,
        library: bool,
    ) -> Advance {
        if self.stopped || !library || self.server != server || self.query != query {
            self.stopped = true;
            return Advance::Cancelled;
        }
        if let Some(key) = &self.waiting {
            let Some(resource) = resources.get(key) else {
                // A reset/invalidation is not a completed remote check.
                self.stopped = true;
                return Advance::Cancelled;
            };
            if resource.pending {
                return Advance::Waiting;
            }
            self.waiting = None;
            self.completed += 1;
        }
        let Some(project) = self.queue.pop_front() else {
            return Advance::Finished;
        };
        let key = status_key(&project);
        self.waiting = Some(key.clone());
        Advance::Read {
            key,
            arguments: json!({"serverId":self.server,"request":{
                "method":"repo.status_summary","params":{"path":project["path"]}
            }}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn project(id: &str) -> Value {
        json!({"id":id,"serverId":"s","path":{"display":format!("/{id}"),"bytesB64":id}})
    }
    fn advance(batch: &mut Batch, resources: &Resources) -> Advance {
        batch.advance(resources, "s", "filter", true)
    }
    #[test]
    fn checks_are_sequential_deduplicated_and_coalesce_with_row_reads() {
        let a = project("a");
        let b = project("b");
        let mut foreign = project("c");
        foreign["serverId"] = json!("other");
        let mut batch = Batch::new(
            "s".into(),
            "filter".into(),
            vec![a.clone(), foreign, a.clone(), b.clone()],
        );
        let mut resources = Resources::default();
        let row_read = resources.begin(&status_key(&a)).unwrap();
        assert_eq!(batch.total, 2);
        let Advance::Read { key, arguments } = advance(&mut batch, &resources) else {
            panic!("first read")
        };
        assert_eq!(key, status_key(&a));
        assert_eq!(arguments["serverId"], "s");
        assert_eq!(arguments["request"]["params"]["path"], a["path"]);
        assert!(
            resources.begin(&key).is_none(),
            "existing row read is reused"
        );
        assert_eq!(advance(&mut batch, &resources), Advance::Waiting);
        assert_eq!(batch.completed, 0);
        resources.complete(&key, row_read, Err("Not a repository".into()));
        let Advance::Read { key, .. } = advance(&mut batch, &resources) else {
            panic!("second read")
        };
        assert_eq!(key, status_key(&b));
        assert_eq!(batch.completed, 1, "a failed check is settled, not retried");
        let request = resources.begin(&key).unwrap();
        assert_eq!(advance(&mut batch, &resources), Advance::Waiting);
        resources.complete(&key, request, Ok(json!({})));
        assert_eq!(advance(&mut batch, &resources), Advance::Finished);
        assert_eq!(advance(&mut batch, &resources), Advance::Finished);
        assert_eq!(batch.completed, 2);
    }
    #[test]
    fn changed_scope_is_terminal_and_does_not_cancel_an_inflight_shared_read() {
        for (server, query, library) in [
            ("other", "filter", true),
            ("s", "edited", true),
            ("s", "filter", false),
        ] {
            let mut resources = Resources::default();
            let mut batch = Batch::new(
                "s".into(),
                "filter".into(),
                vec![project("a"), project("b")],
            );
            let Advance::Read { key, .. } = advance(&mut batch, &resources) else {
                panic!("read")
            };
            let request = resources.begin(&key).unwrap();
            assert_eq!(
                batch.advance(&resources, server, query, library),
                Advance::Cancelled
            );
            assert_eq!(advance(&mut batch, &resources), Advance::Cancelled);
            assert_eq!(batch.completed, 0);
            assert!(resources.complete(&key, request, Ok(json!({}))));
        }
    }
    #[test]
    fn invalidated_requests_are_not_counted_as_completed() {
        let mut resources = Resources::default();
        let mut batch = Batch::new(
            "s".into(),
            "filter".into(),
            vec![project("a"), project("b")],
        );
        let Advance::Read { key, .. } = advance(&mut batch, &resources) else {
            panic!("read")
        };
        let generation = resources.begin(&key).unwrap();
        resources.invalidate(&key);
        assert_eq!(advance(&mut batch, &resources), Advance::Cancelled);
        assert!(!resources.complete(&key, generation, Ok(json!({}))));
        assert_eq!(batch.completed, 0);
    }
}
