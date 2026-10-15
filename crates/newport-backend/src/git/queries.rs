//! Cache observations, never repository opens, operation receipts, or writes.
//! Keys include the exact repository, cursor, snapshot token and query arguments.
use super::*;
use crate::query::{Policy, Registry};

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::new("READ_CHANNEL_ERROR", message)
    }
}
pub(super) fn cacheable(request: &Request) -> bool {
    matches!(
        request,
        Request::Status { .. }
            | Request::StatusSummary { .. }
            | Request::Worktrees { .. }
            | Request::Branches { .. }
            | Request::History { .. }
            | Request::Commit { .. }
            | Request::CommitFiles { .. }
            | Request::DiffPage { .. }
            | Request::CommitDiffPage { .. }
            | Request::BlobPage { .. }
            | Request::Tags { .. }
            | Request::Tag { .. }
            | Request::Stashes { .. }
            | Request::RemoteNames { .. }
            | Request::Remotes { .. }
            | Request::Remote { .. }
    )
}
pub(super) struct Queries([Registry<String, Value, Error>; 4]);
impl Default for Queries {
    fn default() -> Self {
        Self(std::array::from_fn(|lane| {
            Registry::new(if lane == 2 { 8 } else { 32 })
        }))
    }
}
impl Queries {
    pub fn close(&self) {
        for cache in &self.0 {
            cache.close();
        }
    }
    pub fn invalidate(&self, request: &Request) {
        let value = serde_json::to_value(request).unwrap_or(Value::Null);
        self.invalidate_repository(value["params"]["repoId"].as_str());
    }
    pub fn invalidate_repository(&self, repo: Option<&str>) {
        for cache in &self.0 {
            cache.invalidate(|key| {
                repo.is_none_or(|repo| {
                    serde_json::from_str::<Value>(key).is_ok_and(|v| v["params"]["repoId"] == repo)
                })
            });
        }
    }

    fn finish_read(&self, request: &Request, result: Result<Value, Error>) -> Result<Value, Error> {
        if result
            .as_ref()
            .is_err_and(|error| matches!(error.code.as_str(), "STALE_SNAPSHOT" | "STALE_ENTRY"))
        {
            let value = serde_json::to_value(request).unwrap_or(Value::Null);
            if let Some(repo) = value["params"]["repoId"]
                .as_str()
                .filter(|id| !id.is_empty())
            {
                // A recovery status read must not reuse the page that supplied
                // the rejected comparison. All read lanes share this checkout
                // identity; other repositories and sessions retain their data.
                self.invalidate_repository(Some(repo));
            }
        }
        result
    }
}
pub(super) async fn read(
    session: &Arc<Session>,
    events: LogSink,
    state: &Shared,
    pending: &pending::PendingOperations,
    request: Request,
    read: &mut Option<reads::Read<'_>>,
) -> Result<Value, Error> {
    session
        .connection
        .require_agent_available()
        .map_err(Error::transport)?;
    let key = serde_json::to_string(&request).map_err(|e| Error::invalid(e.to_string()))?;
    let weak = Arc::downgrade(session);
    let observed_request = request.clone();
    let state = Arc::downgrade(state);
    let pending = pending.clone();
    let query = session.queries.0[request_lane(&request)].get_or_insert(
        key,
        Policy {
            stale_after: Duration::from_secs(3),
            retry_after: Duration::from_millis(250),
            timeout: DEADLINE + Duration::from_secs(5),
            // Git pages renew demand through their existing visible-page timers.
            // No hidden repository or historical page is continuously polled.
            observation_lease: Duration::from_secs(1),
        },
        move || {
            let weak = weak.clone();
            let state = state.clone();
            let pending = pending.clone();
            let events = events.clone();
            let request = request.clone();
            async move {
                let session = weak
                    .upgrade()
                    .ok_or_else(|| Error::transport("Git session closed."))?;
                let state = state
                    .upgrade()
                    .ok_or_else(|| Error::transport("Application closed."))?;
                execute(&session, events, &state, &pending, request, None).await
            }
        },
    )?;
    let mut cancelled = session.cancelled.subscribe();
    if *cancelled.borrow() || session.connection.is_closed() {
        query.close();
        return Err(Error::transport("Git session closed."));
    }
    let result = tokio::select! {
        biased;
        _ = reads::cancelled(read) => Err(reads::cancelled_error()),
        _ = cancelled.changed() => Err(Error::transport("Git session closed.")),
        result = query.read() => result,
    };
    session.queries.finish_read(&observed_request, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_observations_are_cached_and_tokens_are_part_of_the_key() {
        let status = |repo: &str| {
            serde_json::from_value::<Request>(
                json!({"method":"repo.status_summary","params":{"repoId":repo}}),
            )
            .unwrap()
        };
        assert!(cacheable(&status("one")));
        assert_ne!(
            serde_json::to_string(&status("one")).unwrap(),
            serde_json::to_string(&status("two")).unwrap()
        );
        assert!(!cacheable(&Request::Open {
            path: protocol::Path::new(b"/tmp")
        }));
        assert!(!cacheable(&Request::Get {
            operation_id: "operation".into()
        }));
        assert!(!cacheable(&Request::Close {
            repo_id: "one".into()
        }));
    }
    #[tokio::test]
    async fn repository_invalidation_does_not_discard_another_repository() {
        let queries = Queries::default();
        let policy = Policy {
            stale_after: Duration::from_secs(30),
            retry_after: Duration::from_secs(5),
            timeout: Duration::from_secs(5),
            observation_lease: Duration::from_secs(1),
        };
        let key = |repo: &str| {
            json!({"method":"repo.status_summary","params":{"repoId":repo}}).to_string()
        };
        let one = queries.0[0]
            .get_or_insert(key("one"), policy, || async { Ok(json!(1)) })
            .unwrap();
        let two = queries.0[0]
            .get_or_insert(key("two"), policy, || async { Ok(json!(2)) })
            .unwrap();
        one.read().await.unwrap();
        two.read().await.unwrap();
        queries.invalidate(&Request::Close {
            repo_id: "one".into(),
        });
        assert!(one.snapshot().stale);
        assert!(!two.snapshot().stale);
    }

    #[tokio::test]
    async fn stale_comparisons_refresh_only_their_repository_before_recovery_reads() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for code in ["STALE_SNAPSHOT", "STALE_ENTRY"] {
            let queries = Queries::default();
            let other_server = Queries::default();
            let policy = Policy {
                stale_after: Duration::from_secs(60),
                retry_after: Duration::from_secs(5),
                timeout: Duration::from_secs(5),
                observation_lease: Duration::from_secs(1),
            };
            let mut pages = Vec::new();
            for (same_server, cache) in [(true, &queries), (false, &other_server)] {
                for lane in 0..4 {
                    for repo in ["one", "two"] {
                        let calls = Arc::new(AtomicUsize::new(0));
                        let count = calls.clone();
                        let query =
                            cache.0[lane]
                                .get_or_insert(
                                    json!({"method":"fixture.page","params":{"repoId":repo}})
                                        .to_string(),
                                    policy,
                                    move || {
                                        let count = count.clone();
                                        async move {
                                            Ok(json!(count.fetch_add(1, Ordering::SeqCst) + 1))
                                        }
                                    },
                                )
                                .unwrap();
                        assert_eq!(query.read().await.unwrap(), json!(1));
                        pages.push((same_server, repo, query, calls));
                    }
                }
            }
            let request: Request = serde_json::from_value(json!({
                "method":"repo.status", "params":{"repoId":"one","pageSize":1}
            }))
            .unwrap();
            for unrelated in [
                "AUTH_FAILED",
                "READ_CANCELLED",
                "READ_CHANNEL_ERROR",
                "OUTCOME_UNKNOWN",
            ] {
                let error = queries
                    .finish_read(&request, Err(Error::new(unrelated, "failed")))
                    .unwrap_err();
                assert_eq!(error.code, unrelated);
                assert!(pages.iter().all(|(_, _, query, _)| !query.snapshot().stale));
            }
            // A request without repository scope must never widen invalidation.
            queries
                .finish_read(
                    &Request::Open {
                        path: protocol::Path::new(b"/tmp"),
                    },
                    Err(Error::new(code, "expired")),
                )
                .unwrap_err();
            assert!(pages.iter().all(|(_, _, query, _)| !query.snapshot().stale));
            let error = queries
                .finish_read(&request, Err(Error::new(code, "expired")))
                .unwrap_err();
            assert_eq!(error.code, code);
            for (same_server, repo, query, calls) in pages {
                let expected = if same_server && repo == "one" { 2 } else { 1 };
                assert_eq!(query.read().await.unwrap(), json!(expected));
                assert_eq!(calls.load(Ordering::SeqCst), expected);
            }
        }
    }

    #[tokio::test]
    async fn explicit_repository_refresh_reloads_fresh_pages_across_lanes_only_for_that_checkout() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let queries = Queries::default();
        let policy = Policy {
            stale_after: Duration::from_secs(60),
            retry_after: Duration::from_secs(5),
            timeout: Duration::from_secs(5),
            observation_lease: Duration::from_secs(1),
        };
        let mut pages = Vec::new();
        for lane in 0..3 {
            for repo in ["one", "two"] {
                let calls = Arc::new(AtomicUsize::new(0));
                let count = calls.clone();
                let query = queries.0[lane]
                    .get_or_insert(
                        json!({"method":"fixture.page","params":{"repoId":repo,"cursor":"page-2"}})
                            .to_string(),
                        policy,
                        move || {
                            let count = count.clone();
                            async move { Ok(json!(count.fetch_add(1, Ordering::SeqCst) + 1)) }
                        },
                    )
                    .unwrap();
                assert_eq!(query.read().await.unwrap(), json!(1));
                assert_eq!(query.read().await.unwrap(), json!(1));
                pages.push((repo, query, calls));
            }
        }
        queries.invalidate_repository(Some("one"));
        for (repo, query, calls) in pages {
            let expected = if repo == "one" { 2 } else { 1 };
            assert_eq!(query.read().await.unwrap(), json!(expected));
            assert_eq!(calls.load(Ordering::SeqCst), expected);
        }
    }
}
