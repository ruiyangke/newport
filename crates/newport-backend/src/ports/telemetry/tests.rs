use super::*;
use newport_protocol::monitor::ports::discovery::{
    Details, Diagnostics, Issue, Process, ProcessIdentity,
};

fn limited(kind: IssueKind, count: u64) -> Report {
    Report {
        outcome: Outcome::Partial,
        issues: vec![Issue {
            kind,
            count,
            message: "SECRET-REMOTE-ERROR".into(),
        }],
    }
}

#[test]
fn summaries_include_typed_counts_and_fallback_causes_without_remote_text() {
    let mut snapshot = crate::ports::tests::snapshot(1);
    snapshot.elapsed_ms = 25;
    snapshot.scope.boot_id = Some("SECRET-BOOT-ID".into());
    snapshot.sockets[0].account = Some("SECRET-ACCOUNT".into());
    snapshot.processes.push(Process {
        id: 1,
        identity: ProcessIdentity {
            pid: 1234,
            start_ticks: 8,
        },
        name: Some("SECRET-COMMAND".into()),
        executable_name: Some("SECRET-EXECUTABLE".into()),
        uid: Some(1000),
        account: Some("SECRET-ACCOUNT".into()),
        details: Some(Details {
            arguments: vec!["SECRET-ARGV".into()],
            ..Default::default()
        }),
        metadata: limited(IssueKind::PermissionDenied, 2),
    });
    snapshot.sockets[0].holders.push(1);
    snapshot.coverage.ipv4 = limited(IssueKind::Unsupported, 1);
    snapshot.coverage.ipv4.outcome = Outcome::Complete;
    snapshot.coverage.processes = limited(IssueKind::PermissionDenied, u64::MAX);
    snapshot.coverage.processes.issues.push(Issue {
        kind: IssueKind::PermissionDenied,
        count: 2,
        message: "SECRET-EXTRA".into(),
    });
    snapshot.diagnostics = Diagnostics {
        ipv4_source: Some(SocketSource::Proc),
        ..Default::default()
    };
    let text = Summary::new(&snapshot, true).to_string();
    assert!(text.contains("elapsed_ms=25 sockets=1 processes=1 holder_references=1"));
    assert!(text.contains("ipv4_source=Some(Proc)"));
    assert!(text.contains("ipv4_coverage=Complete ipv4_unsupported_issues=1"));
    assert!(text.contains(&format!("processes_permission_denied_issues={}", u64::MAX)));
    assert!(!text.contains("SECRET"));
    assert!(!text.contains("1234"));
    let mut telemetry = Telemetry::default();
    for transition in telemetry.transitions(&snapshot) {
        assert!(!transition.to_string().contains("SECRET"));
    }
}

#[test]
fn repeated_failures_ignore_message_count_and_order_but_log_recovery_and_new_causes() {
    let mut telemetry = Telemetry::default();
    let mut snapshot = crate::ports::tests::snapshot(1);
    snapshot.coverage.processes = limited(IssueKind::PermissionDenied, 1);
    assert_eq!(telemetry.transitions(&snapshot).len(), 1);
    snapshot.generation += 1;
    snapshot.coverage.processes.issues[0].count = 20;
    snapshot.coverage.processes.issues[0].message = "Changed remote text".into();
    assert!(telemetry.transitions(&snapshot).is_empty());
    snapshot.generation += 1;
    snapshot.coverage.processes.issues.push(Issue {
        kind: IssueKind::TimedOut,
        count: 1,
        message: "Timeout".into(),
    });
    assert_eq!(telemetry.transitions(&snapshot).len(), 1);
    snapshot.generation += 1;
    snapshot.coverage.processes.issues.reverse();
    assert!(telemetry.transitions(&snapshot).is_empty());
    snapshot.generation += 1;
    snapshot.coverage.processes = Report::complete();
    let recovery = telemetry.transitions(&snapshot);
    assert_eq!(recovery.len(), 1);
    assert!(
        recovery[0]
            .to_string()
            .contains("outcome=Complete causes=none")
    );
    snapshot.generation += 1;
    assert!(telemetry.transitions(&snapshot).is_empty());
}

#[test]
fn elevation_failure_and_recovery_each_log_one_stage_change() {
    let mut telemetry = Telemetry::default();
    let mut snapshot = crate::ports::tests::snapshot(1);
    snapshot.coverage.elevation = limited(IssueKind::PermissionDenied, 1);
    assert_eq!(telemetry.transitions(&snapshot).len(), 1);
    snapshot.generation += 1;
    assert!(telemetry.transitions(&snapshot).is_empty());
    snapshot.generation += 1;
    snapshot.coverage.elevation = Report::complete();
    let recovery = telemetry.transitions(&snapshot);
    assert_eq!(recovery.len(), 1);
    assert!(
        recovery[0]
            .to_string()
            .contains("stage=elevation outcome=Complete")
    );
}

#[test]
fn scope_changes_and_generation_resets_start_new_diagnostic_episodes() {
    let mut telemetry = Telemetry::default();
    let mut snapshot = crate::ports::tests::snapshot(10);
    snapshot.coverage.cgroups = limited(IssueKind::PermissionDenied, 1);
    assert_eq!(telemetry.transitions(&snapshot).len(), 1);
    snapshot.generation = 11;
    assert!(telemetry.transitions(&snapshot).is_empty());
    snapshot.scope.processes.as_mut().unwrap().inode += 1;
    snapshot.generation = 12;
    assert_eq!(telemetry.transitions(&snapshot).len(), 1);
    snapshot.generation = 1;
    assert_eq!(telemetry.transitions(&snapshot).len(), 1);
}

#[test]
fn query_failures_are_deduplicated_until_recovery_or_a_new_failure_kind() {
    let mut telemetry = Telemetry::default();
    assert!(telemetry.replace_failure(Failure::Query));
    assert!(!telemetry.replace_failure(Failure::Query));
    assert!(telemetry.replace_failure(Failure::InvalidSnapshot));
    assert!(!telemetry.replace_failure(Failure::InvalidSnapshot));
    telemetry.succeeded(uuid::Uuid::nil());
    assert!(telemetry.failure.is_none());
    assert!(telemetry.replace_failure(Failure::InvalidSnapshot));
}

#[test]
fn skipped_stages_preserve_failure_episodes() {
    let mut telemetry = Telemetry::default();
    let mut snapshot = crate::ports::tests::snapshot(1);
    snapshot.coverage.services = limited(IssueKind::Unavailable, 1);
    assert_eq!(telemetry.transitions(&snapshot).len(), 1);
    snapshot.generation += 1;
    snapshot.coverage.services = Report::default();
    assert!(telemetry.transitions(&snapshot).is_empty());
    snapshot.generation += 1;
    snapshot.coverage.services = limited(IssueKind::Unavailable, 2);
    assert!(telemetry.transitions(&snapshot).is_empty());
}

static LOGS: std::sync::Mutex<Option<(String, Vec<String>)>> = std::sync::Mutex::new(None);
static LOGGER: std::sync::Once = std::sync::Once::new();

struct Capture;

impl tracing::Subscriber for Capture {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "newport::discovery"
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if !self.enabled(event.metadata()) {
            return;
        }
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        let text = fields.0;
        if let Some((server, records)) = LOGS.lock().unwrap().as_mut()
            && text.contains(server.as_str())
        {
            assert!(records.len() < 64);
            records.push(text);
        }
    }
}

struct Fields(String);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        write!(self.0, "{}={value:?} ", field.name()).unwrap();
    }
}

#[tokio::test]
async fn live_query_path_emits_redacted_summaries_and_deduplicates_failures() {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };
    LOGGER.call_once(|| {
        tracing::subscriber::set_global_default(Capture).unwrap();
    });
    let server = uuid::Uuid::new_v4();
    *LOGS.lock().unwrap() = Some((server.to_string(), Vec::new()));
    let step = Arc::new(AtomicU64::new(0));
    let query = crate::ports::Query::new(
        crate::query::Policy {
            stale_after: Duration::from_secs(30),
            retry_after: Duration::from_secs(10),
            timeout: Duration::from_secs(5),
            observation_lease: Duration::from_secs(1),
        },
        server,
        move |_| {
            let step = step.fetch_add(1, Ordering::SeqCst);
            async move {
                if step < 2 {
                    return Err(crate::monitor::Error::Transport(
                        newport_protocol::encoding::protobuf::FrameError::Io(
                            std::io::Error::other(format!("SECRET-QUERY-{step}")),
                        ),
                    )
                    .into());
                }
                let mut snapshot = crate::ports::tests::snapshot(step - 1);
                snapshot.diagnostics.ipv4_source = Some(SocketSource::Proc);
                snapshot.sockets[0].source = SocketSource::Proc;
                snapshot.coverage.processes = if step == 2 {
                    limited(IssueKind::PermissionDenied, 9)
                } else {
                    Report::complete()
                };
                Ok(snapshot)
            }
        },
    );
    assert!(query.refresh().await.is_err());
    assert!(query.refresh().await.is_err());
    query.refresh().await.unwrap();
    query.refresh().await.unwrap();
    query.close();
    let (_, records) = LOGS.lock().unwrap().take().unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|line| line.contains("event=discovery_query_failed"))
            .count(),
        1
    );
    assert_eq!(
        records
            .iter()
            .filter(|line| line.contains("event=discovery_scan "))
            .count(),
        2
    );
    assert_eq!(
        records
            .iter()
            .filter(|line| line.contains("kind=query_recovered"))
            .count(),
        1
    );
    assert!(
        records
            .iter()
            .any(|line| line.contains("stage=processes outcome=Complete causes=none"))
    );
    assert!(
        records
            .iter()
            .any(|line| line.contains("processes_permission_denied_issues=9"))
    );
    assert!(records.iter().all(|line| !line.contains("SECRET")));
}
