//! Explicit, sequential status checks for a captured project-library scope.
//! Dropping a batch stops scheduling; it never replays or cancels a shared read.
use crate::{request_error::RequestError, resources::Resources};
use newport_protocol::{desktop::preferences::Project, git::Request};
use std::collections::{HashSet, VecDeque};

pub fn status_key(project: &Project) -> String {
    format!(
        "project-status:{:?}",
        (
            project.server.to_string(),
            &project.id,
            &project.path.bytes_b64
        )
    )
}

pub struct Batch {
    server: String,
    query: String,
    queue: VecDeque<Project>,
    waiting: Option<String>,
    stopped: bool,
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, PartialEq)]
pub enum Advance {
    Read {
        key: String,
        request: Result<Box<Request>, RequestError>,
    },
    Waiting,
    Finished,
    Cancelled,
}

impl Batch {
    pub fn new(server: String, query: String, projects: Vec<Project>) -> Self {
        let mut seen = HashSet::new();
        let queue: VecDeque<_> = projects
            .into_iter()
            .filter(|p| p.server.to_string() == server)
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
            request: status_request(&project).map(Box::new),
        }
    }
}

/// A stored path that does not decode fails here, before anything is sent.
pub fn status_request(project: &Project) -> Result<Request, RequestError> {
    project
        .path
        .decode()
        .map_err(|error| RequestError::from(error.to_string()))?;
    Ok(Request::StatusSummary {
        repo_id: None,
        path: Some(project.path.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::git::Path;
    const SERVER: uuid::Uuid = uuid::Uuid::from_u128(1);
    const OTHER: uuid::Uuid = uuid::Uuid::from_u128(2);
    fn project(id: &str) -> Project {
        Project {
            id: uuid::Uuid::from_u128(id.bytes().fold(0, |id, byte| id << 8 | u128::from(byte))),
            server: SERVER,
            name: id.into(),
            path: Path::new(format!("/{id}").as_bytes()),
        }
    }
    fn advance(batch: &mut Batch, resources: &Resources) -> Advance {
        batch.advance(resources, &SERVER.to_string(), "filter", true)
    }
    fn batch(projects: Vec<Project>) -> Batch {
        Batch::new(SERVER.to_string(), "filter".into(), projects)
    }
    #[test]
    fn checks_are_sequential_deduplicated_and_coalesce_with_row_reads() {
        let a = project("a");
        let b = project("b");
        let mut foreign = project("c");
        foreign.server = OTHER;
        let mut batch = batch(vec![a.clone(), foreign, a.clone(), b.clone()]);
        let mut resources = Resources::default();
        let row_read = resources.begin(&status_key(&a)).unwrap();
        assert_eq!(batch.total, 2);
        let Advance::Read { key, request } = advance(&mut batch, &resources) else {
            panic!("first read")
        };
        assert_eq!(key, status_key(&a));
        assert_eq!(
            *request.unwrap(),
            Request::StatusSummary {
                repo_id: None,
                path: Some(Path::new(b"/a"))
            }
        );
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
        resources.complete_owned(&key, request, Ok(true));
        assert_eq!(advance(&mut batch, &resources), Advance::Finished);
        assert_eq!(advance(&mut batch, &resources), Advance::Finished);
        assert_eq!(batch.completed, 2);
    }
    #[test]
    fn changed_scope_is_terminal_and_does_not_cancel_an_inflight_shared_read() {
        for (server, query, library) in [
            (OTHER, "filter", true),
            (SERVER, "edited", true),
            (SERVER, "filter", false),
        ] {
            let mut resources = Resources::default();
            let mut batch = batch(vec![project("a"), project("b")]);
            let Advance::Read { key, .. } = advance(&mut batch, &resources) else {
                panic!("read")
            };
            let request = resources.begin(&key).unwrap();
            assert_eq!(
                batch.advance(&resources, &server.to_string(), query, library),
                Advance::Cancelled
            );
            assert_eq!(advance(&mut batch, &resources), Advance::Cancelled);
            assert_eq!(batch.completed, 0);
            assert!(resources.complete_owned(&key, request, Ok(true)));
        }
    }
    #[test]
    fn invalidated_requests_are_not_counted_as_completed() {
        let mut resources = Resources::default();
        let mut batch = batch(vec![project("a"), project("b")]);
        let Advance::Read { key, .. } = advance(&mut batch, &resources) else {
            panic!("read")
        };
        let generation = resources.begin(&key).unwrap();
        resources.invalidate(&key);
        assert_eq!(advance(&mut batch, &resources), Advance::Cancelled);
        assert!(!resources.complete_owned(&key, generation, Ok(true)));
        assert_eq!(batch.completed, 0);
    }
    #[test]
    fn malformed_bookmark_paths_fail_locally_and_the_batch_can_continue() {
        let mut malformed = project("bad");
        malformed.path.bytes_b64 = "not base64!".into();
        let mut batch = batch(vec![malformed, project("next")]);
        let mut resources = Resources::default();
        let Advance::Read { key, request } = advance(&mut batch, &resources) else {
            panic!("first read");
        };
        let error = request.unwrap_err();
        let generation = resources.begin(&key).unwrap();
        resources.complete_detailed(&key, generation, Err(error));
        let Advance::Read { request, .. } = advance(&mut batch, &resources) else {
            panic!("next read");
        };
        assert_eq!(batch.completed, 1);
        assert_eq!(
            *request.unwrap(),
            Request::StatusSummary {
                repo_id: None,
                path: Some(Path::new(b"/next"))
            }
        );
    }
}
