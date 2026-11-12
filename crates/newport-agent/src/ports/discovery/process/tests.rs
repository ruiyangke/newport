use super::*;
use newport_protocol::monitor::ports::discovery::{Endpoint, Outcome, SocketSource};
use std::{
    os::unix::{ffi::OsStrExt, fs::symlink},
    time::{Duration, Instant},
};

fn fixture(root: &Path, pid: u32, ticks: u64, inode: u64) -> PathBuf {
    let directory = root.join(format!("proc/{pid}"));
    fs::create_dir_all(directory.join("fd")).unwrap();
    write_start(&directory, pid, ticks);
    fs::write(directory.join("comm"), b"worker\n").unwrap();
    fs::write(
        directory.join("status"),
        b"Name:\tworker\nUid:\t0\t0\t0\t0\n",
    )
    .unwrap();
    fs::write(
        directory.join("cmdline"),
        b"/usr/bin/worker\0--name\0two words\0\0",
    )
    .unwrap();
    symlink("/usr/bin/worker", directory.join("exe")).unwrap();
    symlink("/home/test/project", directory.join("cwd")).unwrap();
    symlink(format!("socket:[{inode}]"), directory.join("fd/4")).unwrap();
    directory
}

fn write_start(directory: &Path, pid: u32, ticks: u64) {
    fs::write(
        directory.join("stat"),
        format!(
            "{pid} (name with ) parentheses) S {} {ticks}\n",
            ["0"; 18].join(" ")
        ),
    )
    .unwrap();
}

fn socket(inode: u64) -> Socket {
    Socket {
        id: 1,
        endpoint: Endpoint {
            address: "127.0.0.1".parse().unwrap(),
            port: 8080,
            interface: None,
        },
        source: SocketSource::Diagnostic,
        inode,
        cookie: Some(inode),
        uid: 55,
        account: None,
        ipv6_only: None,
        cgroup: None,
        holders: Vec::new(),
        holder_state: HolderState::Unresolved,
        holders_truncated: false,
        services: Vec::new(),
    }
}

fn run(root: &Path, sockets: &mut [Socket], detailed: bool) -> Inspection {
    collect(
        root,
        sockets,
        &Budget::new(Instant::now() + Duration::from_secs(2), &|| false),
        detailed,
    )
}

#[test]
fn shared_socket_keeps_every_observed_process_and_deduplicates_its_descriptors() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path(), 42, 100, 7);
    let first = fixture(root.path(), 1, 3, 7);
    symlink("socket:[7]", first.join("fd/9")).unwrap();
    let mut sockets = [socket(7)];
    let result = run(root.path(), &mut sockets, true);
    assert_eq!(
        result
            .processes
            .iter()
            .map(|p| p.identity.pid)
            .collect::<Vec<_>>(),
        [1, 42]
    );
    assert_eq!(sockets[0].holders, [1, 2]);
    assert_eq!(sockets[0].uid, 55);
    assert_eq!(result.processes[0].uid, Some(0));
    assert_eq!(result.witnesses.len(), 2);
    assert_eq!(result.witnesses[0].descriptor, 4);
    assert_eq!(result.coverage.outcome, Outcome::Complete);
    assert_eq!(
        result.processes[0].details.as_ref().unwrap().arguments,
        ["/usr/bin/worker", "--name", "two words", ""]
    );
}

#[test]
fn witness_revalidation_rejects_reused_pids_and_replaced_descriptors() {
    let root = tempfile::tempdir().unwrap();
    let directory = fixture(root.path(), 12, 99, 7);
    let mut sockets = [socket(7)];
    let result = run(root.path(), &mut sockets, false);
    let witness = &result.witnesses[0];
    assert!(verify(root.path(), witness).unwrap());
    write_start(&directory, 12, 100);
    assert!(!verify(root.path(), witness).unwrap());
    write_start(&directory, 12, 99);
    fs::remove_file(directory.join("fd/4")).unwrap();
    symlink("socket:[8]", directory.join("fd/4")).unwrap();
    assert!(!verify(root.path(), witness).unwrap());
}

#[test]
fn missing_optional_metadata_does_not_hide_a_verified_holder() {
    let root = tempfile::tempdir().unwrap();
    let directory = fixture(root.path(), 12, 99, 7);
    fs::remove_file(directory.join("comm")).unwrap();
    fs::remove_file(directory.join("exe")).unwrap();
    fs::remove_file(directory.join("cwd")).unwrap();
    let mut sockets = [socket(7)];
    let result = run(root.path(), &mut sockets, true);
    assert_eq!(sockets[0].holders, [1]);
    assert_eq!(result.processes[0].identity.pid, 12);
    assert!(result.processes[0].name.is_none());
    assert_eq!(result.processes[0].metadata.outcome, Outcome::Partial);
    assert_eq!(result.coverage.outcome, Outcome::Complete);
}

#[test]
fn lightweight_scan_excludes_arguments_and_working_directory() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path(), 12, 99, 7);
    let result = run(root.path(), &mut [socket(7)], false);
    assert!(result.processes[0].details.is_none());
    assert_eq!(
        result.processes[0].executable_name.as_deref(),
        Some("worker")
    );
    assert_eq!(result.details.outcome, Outcome::NotApplicable);
}

#[test]
fn ambiguous_inode_reuse_does_not_attribute_both_sockets_to_a_process() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path(), 12, 99, 7);
    let mut sockets = [
        socket(7),
        Socket {
            id: 2,
            cookie: Some(8),
            ..socket(7)
        },
    ];
    let result = run(root.path(), &mut sockets, false);
    assert!(result.processes.is_empty());
    assert!(
        sockets
            .iter()
            .all(|socket| socket.holders.is_empty() && socket.holder_state == HolderState::Changed)
    );
}

#[test]
fn holder_limit_marks_truncation_and_never_leaves_dangling_process_references() {
    let root = tempfile::tempdir().unwrap();
    for number in 1..=MAX_HOLDERS + 1 {
        fixture(root.path(), u32::try_from(number).unwrap(), 99, 7);
    }
    let mut sockets = [socket(7)];
    let result = run(root.path(), &mut sockets, false);
    assert_eq!(result.processes.len(), MAX_HOLDERS);
    assert_eq!(sockets[0].holders.len(), MAX_HOLDERS);
    assert!(sockets[0].holders_truncated);
    assert_eq!(result.coverage.outcome, Outcome::Partial);
    assert!(
        sockets[0]
            .holders
            .iter()
            .all(|id| result.processes.iter().any(|p| p.id == *id))
    );
}

#[test]
fn deleted_and_non_utf8_paths_are_explicit_display_metadata() {
    let root = tempfile::tempdir().unwrap();
    let directory = fixture(root.path(), 12, 99, 7);
    fs::remove_file(directory.join("cwd")).unwrap();
    symlink(
        std::ffi::OsStr::from_bytes(b"/home/test/proj\xff (deleted)"),
        directory.join("cwd"),
    )
    .unwrap();
    let result = run(root.path(), &mut [socket(7)], true);
    let path = result.processes[0]
        .details
        .as_ref()
        .unwrap()
        .directory
        .as_ref()
        .unwrap();
    assert!(path.deleted);
    assert!(path.lossy);
    assert_eq!(path.value, "/home/test/proj�");
}

#[test]
fn oversized_arguments_are_reported_without_truncating_into_fake_arguments() {
    let root = tempfile::tempdir().unwrap();
    let directory = fixture(root.path(), 12, 99, 7);
    fs::write(directory.join("cmdline"), vec![b'x'; MAX_FIELD + 1]).unwrap();
    let result = run(root.path(), &mut [socket(7)], true);
    assert!(
        result.processes[0]
            .details
            .as_ref()
            .unwrap()
            .arguments
            .is_empty()
    );
    assert_eq!(result.processes[0].metadata.outcome, Outcome::Partial);
}

#[test]
fn repeated_metadata_failures_keep_their_count_beyond_the_issue_slot_limit() {
    use newport_protocol::monitor::ports::discovery::{IssueKind, MAX_ISSUES};
    let root = tempfile::tempdir().unwrap();
    let count = MAX_ISSUES * 2;
    for number in 1..=count {
        let directory = fixture(root.path(), u32::try_from(number).unwrap(), 99, 7);
        fs::write(directory.join("cmdline"), vec![b'x'; MAX_FIELD + 1]).unwrap();
    }
    let result = run(root.path(), &mut [socket(7)], true);
    assert_eq!(result.processes.len(), count);
    assert_eq!(result.details.outcome, Outcome::Partial);
    assert_eq!(
        result
            .details
            .issues
            .iter()
            .map(|issue| issue.count)
            .sum::<u64>(),
        u64::try_from(count).unwrap()
    );
    assert_eq!(result.details.issues.len(), 1);
    assert_eq!(result.details.issues[0].kind, IssueKind::BudgetExceeded);
}

#[test]
fn invalid_or_missing_process_inventory_cannot_be_empty_success() {
    let root = tempfile::tempdir().unwrap();
    let result = run(root.path(), &mut [socket(7)], true);
    assert_ne!(result.coverage.outcome, Outcome::Complete);
    let directory = fixture(root.path(), 12, 99, 7);
    write_start(&directory, 13, 99);
    let result = run(root.path(), &mut [socket(7)], true);
    assert!(result.processes.is_empty());
    assert_ne!(result.coverage.outcome, Outcome::Complete);
}

#[test]
fn cancellation_cannot_publish_unverified_holders() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path(), 12, 99, 7);
    let mut sockets = [socket(7)];
    let result = collect(
        root.path(),
        &mut sockets,
        &Budget::new(Instant::now() + Duration::from_secs(1), &|| true),
        true,
    );
    assert!(result.processes.is_empty());
    assert!(sockets[0].holders.is_empty());
    assert_eq!(result.coverage.outcome, Outcome::Partial);
}
