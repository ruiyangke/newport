use super::*;
use newport_protocol::monitor::ports::discovery::*;

pub(crate) mod pipeline;

pub(crate) fn snapshot(generation: u64) -> Snapshot {
    Snapshot {
        generation,
        scope: Scope {
            network: Some(Namespace {
                device: 1,
                inode: 2,
            }),
            processes: Some(Namespace {
                device: 1,
                inode: 3,
            }),
            boot_id: Some("18f68fcb-71e9-47fe-a226-e9da27d82953".into()),
        },
        sockets: vec![Socket {
            id: 1,
            endpoint: Endpoint {
                address: std::net::Ipv4Addr::LOCALHOST.into(),
                port: 8080,
                interface: None,
            },
            source: SocketSource::Diagnostic,
            inode: 5,
            cookie: Some(7),
            uid: 1000,
            account: None,
            ipv6_only: None,
            cgroup: None,
            holders: Vec::new(),
            holder_state: HolderState::Unresolved,
            holders_truncated: false,
            services: Vec::new(),
        }],
        coverage: Coverage {
            ipv4: Report::complete(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn policy() -> crate::query::Policy {
    crate::query::Policy {
        stale_after: Duration::from_secs(30),
        retry_after: Duration::from_secs(10),
        timeout: Duration::from_secs(5),
        observation_lease: Duration::from_secs(1),
    }
}

#[tokio::test]
async fn monitor_failure_keeps_transport_cause_until_presentation() {
    use newport_protocol::encoding::protobuf::FrameError;
    use std::io::{Error, ErrorKind};

    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let query = Query::new(policy(), uuid::Uuid::nil(), move |_| {
        let first = attempts.fetch_add(1, Ordering::AcqRel) == 0;
        async move {
            if first {
                Err(monitor::Error::Transport(FrameError::Io(Error::new(
                    ErrorKind::ConnectionReset,
                    "Fixture connection reset",
                )))
                .into())
            } else {
                Ok(snapshot(1))
            }
        }
    });
    let failure = query.refresh().await.unwrap_err();
    let retained = query.inner.snapshot().error.unwrap();
    for failure in [&failure, &retained] {
        let mut source = Some(failure as &(dyn std::error::Error + 'static));
        let mut found = false;
        while let Some(error) = source {
            if let Some(error) = error.downcast_ref::<Error>() {
                assert_eq!(error.kind(), ErrorKind::ConnectionReset);
                found = true;
            }
            source = error.source();
        }
        assert!(found, "monitor I/O cause was discarded: {failure}");
    }
    assert!(
        query
            .snapshot()
            .error
            .unwrap()
            .contains("Fixture connection reset")
    );
    query.refresh().await.unwrap();
    assert!(query.snapshot().error.is_none());
    assert!(query.snapshot().data.is_some());
}

#[tokio::test]
async fn manual_refresh_reaches_collector_and_scope_failure_cannot_resurrect_cached_data() {
    let step = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let query = Query::new(policy(), uuid::Uuid::nil(), move |refresh| {
        assert!(refresh);
        let step = step.fetch_add(1, Ordering::AcqRel);
        async move {
            match step {
                0 => Ok(snapshot(1)),
                1 => {
                    let mut failed = snapshot(2);
                    failed.sockets.clear();
                    failed.scope.network = None;
                    failed.coverage.ipv4 = Report {
                        outcome: Outcome::Unavailable,
                        issues: vec![Issue {
                            kind: IssueKind::TimedOut,
                            count: 1,
                            message: "Scan timed out.".into(),
                        }],
                    };
                    Ok(failed)
                }
                2 => Err(monitor::Error::Closed.into()),
                _ => Ok(snapshot(1)),
            }
        }
    });
    query.refresh().await.unwrap();
    assert!(query.snapshot().data.is_some());
    let copied_before_invalidation = query.inner.snapshot();
    assert!(query.refresh().await.is_err());
    let failed = query.snapshot().data.unwrap();
    assert_eq!(failed.current.generation, 2);
    assert!(failed.current.scope.network.is_none());
    assert!(!failed.has_current_inventory() && failed.retained.is_empty());
    let copied_failure = query.inner.snapshot();
    assert!(query.refresh().await.is_err());
    assert!(query.snapshot().data.is_none());
    query.refresh().await.unwrap();
    assert!(query.snapshot().data.is_some());
    assert!(query.project(copied_before_invalidation).data.is_none());
    assert!(query.project(copied_failure).data.is_none());
    let copied_before_close = query.inner.snapshot();
    query.close();
    assert!(query.snapshot().data.is_none());
    assert!(query.project(copied_before_close).data.is_none());
}

#[tokio::test]
async fn source_failure_exposes_stale_observations_and_a_failure_together() {
    let step = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let query = Query::new(policy(), uuid::Uuid::nil(), move |_| {
        let generation = step.fetch_add(1, Ordering::AcqRel);
        async move {
            let mut value = snapshot(generation);
            if generation > 1 {
                value.sockets.clear();
                value.coverage.ipv4 = Report {
                    outcome: Outcome::Unavailable,
                    issues: vec![Issue {
                        kind: IssueKind::TimedOut,
                        count: 1,
                        message: "Scan timed out.".into(),
                    }],
                };
            }
            Ok(value)
        }
    });
    query.refresh().await.unwrap();
    query.refresh().await.unwrap();
    let value = query.snapshot();
    assert!(value.stale && value.error.is_some());
    let data = value.data.unwrap();
    assert!(data.current.sockets.is_empty());
    assert_eq!(data.retained[0].sockets[0].endpoint.port, 8080);
}

#[tokio::test]
async fn successful_scope_changes_and_generation_resets_fence_already_copied_results() {
    let step = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let query = Query::new(policy(), uuid::Uuid::nil(), move |_| {
        let step = step.fetch_add(1, Ordering::AcqRel);
        async move {
            let mut value = snapshot(if step == 4 { 1 } else { step });
            if step >= 3 {
                value.scope.network.as_mut().unwrap().inode += 1;
            }
            Ok(value)
        }
    });
    query.refresh().await.unwrap();
    let old = query.inner.snapshot();
    query.refresh().await.unwrap();
    // A normal scan does not withdraw last-good data while its successor publishes.
    assert!(query.project(old.clone()).data.is_some());
    query.refresh().await.unwrap();
    assert!(query.project(old).data.is_none());
    let before_reset = query.inner.snapshot();
    query.refresh().await.unwrap();
    assert!(query.project(before_reset).data.is_none());
    assert!(query.snapshot().data.is_some());
}
