use super::*;
use newport_protocol::monitor::ports::discovery::{
    Compose, ContainerState, Details, Endpoint, ObservedPath, Process, ProcessIdentity,
    Publication, Report,
};
use std::{fs, os::unix::fs::symlink, time::Duration};

fn process(root: &Path, id: u32, directory: &str) -> Process {
    let path = root.join(format!("proc/{id}"));
    fs::create_dir_all(path.join("ns")).unwrap();
    fs::create_dir_all(root.join("proc/self/ns")).unwrap();
    if !root.join("proc/self/ns/mnt").exists() {
        fs::write(root.join("proc/self/ns/mnt"), "namespace").unwrap();
    }
    fs::hard_link(root.join("proc/self/ns/mnt"), path.join("ns/mnt")).unwrap();
    symlink(root, path.join("root")).unwrap();
    symlink(directory, path.join("cwd")).unwrap();
    fs::write(
        path.join("stat"),
        format!("{id} (server) S {} 7\n", ["0"; 18].join(" ")),
    )
    .unwrap();
    Process {
        id,
        identity: ProcessIdentity {
            pid: id,
            start_ticks: 7,
        },
        name: Some("server".into()),
        executable_name: None,
        uid: None,
        account: None,
        details: Some(Details {
            directory: Some(ObservedPath {
                value: directory.into(),
                deleted: false,
                lossy: false,
            }),
            ..Details::default()
        }),
        metadata: Report::complete(),
    }
}

fn project(root: &Path, directory: &str) {
    let path = root.join(directory.trim_start_matches('/'));
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("Cargo.toml"), "").unwrap();
}

fn snapshot(processes: Vec<Process>) -> Snapshot {
    let mut snapshot = Snapshot {
        generation: 1,
        processes,
        ..Snapshot::default()
    };
    snapshot.coverage.processes = Report::complete();
    snapshot.coverage.details = Report::complete();
    snapshot
}

fn run(root: &Path, value: &mut Snapshot) {
    enrich(
        root,
        value,
        Instant::now() + Duration::from_secs(2),
        &|| false,
    );
}

#[test]
fn different_holder_projects_remain_separate_process_evidence() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), "/srv/first");
    project(root.path(), "/srv/second");
    let mut value = snapshot(vec![
        process(root.path(), 42, "/srv/first"),
        process(root.path(), 43, "/srv/second"),
    ]);
    run(root.path(), &mut value);
    let projects: Vec<_> = value
        .processes
        .iter()
        .map(|process| {
            process
                .details
                .as_ref()
                .unwrap()
                .project
                .as_ref()
                .unwrap()
                .root
                .as_str()
        })
        .collect();
    assert_eq!(projects, ["/srv/first", "/srv/second"]);
    value.validate().unwrap();
}

#[test]
fn deleted_and_lossy_paths_are_preserved_without_project_walks() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), "/srv/project");
    let mut value = snapshot(vec![
        process(root.path(), 42, "/srv/project"),
        process(root.path(), 43, "/srv/project"),
    ]);
    value.processes[0]
        .details
        .as_mut()
        .unwrap()
        .directory
        .as_mut()
        .unwrap()
        .deleted = true;
    value.processes[1]
        .details
        .as_mut()
        .unwrap()
        .directory
        .as_mut()
        .unwrap()
        .lossy = true;
    run(root.path(), &mut value);
    assert!(
        value
            .processes
            .iter()
            .all(|process| process.details.as_ref().unwrap().project.is_none())
    );
    assert!(
        value.processes[0]
            .details
            .as_ref()
            .unwrap()
            .directory
            .as_ref()
            .unwrap()
            .deleted
    );
    assert!(
        value.processes[1]
            .details
            .as_ref()
            .unwrap()
            .directory
            .as_ref()
            .unwrap()
            .lossy
    );
}

#[test]
fn pid_reuse_mount_namespace_and_chroot_changes_refuse_host_project_attribution() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), "/srv/project");
    let mut value = snapshot(vec![
        process(root.path(), 42, "/srv/project"),
        process(root.path(), 43, "/srv/project"),
        process(root.path(), 44, "/srv/project"),
    ]);
    value.processes[0].identity.start_ticks = 6;
    fs::remove_file(root.path().join("proc/43/ns/mnt")).unwrap();
    fs::write(root.path().join("proc/43/ns/mnt"), "different").unwrap();
    fs::remove_file(root.path().join("proc/44/root")).unwrap();
    symlink(
        root.path().join("srv/project"),
        root.path().join("proc/44/root"),
    )
    .unwrap();
    run(root.path(), &mut value);
    assert!(
        value
            .processes
            .iter()
            .all(|process| process.details.as_ref().unwrap().project.is_none())
    );
    assert_eq!(value.coverage.details.outcome, Outcome::Partial);
    assert!(
        value
            .processes
            .iter()
            .all(|process| process.metadata.outcome == Outcome::Partial)
    );
}

#[test]
fn directory_change_during_resolution_discards_the_project_only() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), "/srv/project");
    let mut value = snapshot(vec![process(root.path(), 42, "/srv/project")]);
    let calls = std::cell::Cell::new(0);
    let cancelled = || {
        let count = calls.get() + 1;
        calls.set(count);
        if count == 5 {
            fs::remove_file(root.path().join("proc/42/cwd")).unwrap();
            symlink("/srv/another", root.path().join("proc/42/cwd")).unwrap();
        }
        false
    };
    enrich(
        root.path(),
        &mut value,
        Instant::now() + Duration::from_secs(2),
        &cancelled,
    );
    assert!(
        value.processes[0]
            .details
            .as_ref()
            .unwrap()
            .project
            .is_none()
    );
    assert_eq!(value.processes[0].name.as_deref(), Some("server"));
    assert_eq!(value.coverage.details.outcome, Outcome::Partial);
}

#[test]
fn cancellation_preserves_process_identity_and_reports_incomplete_details() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), "/srv/project");
    let mut value = snapshot(vec![process(root.path(), 42, "/srv/project")]);
    enrich(
        root.path(),
        &mut value,
        Instant::now() + Duration::from_secs(2),
        &|| true,
    );
    assert_eq!(value.processes[0].identity.pid, 42);
    assert!(
        value.processes[0]
            .details
            .as_ref()
            .unwrap()
            .project
            .is_none()
    );
    assert_eq!(value.coverage.details.outcome, Outcome::Partial);
    value.validate().unwrap();
}

#[test]
fn compose_workdir_enrichment_is_independent_of_host_socket_ownership() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), "/srv/project");
    let mut value = snapshot(Vec::new());
    value.publications.push(Publication {
        engine: Some("engine-a".into()),
        container: "container-a".into(),
        name: "web".into(),
        image: None,
        state: ContainerState::Running,
        host: Endpoint {
            address: "127.0.0.1".parse().unwrap(),
            port: 8080,
            interface: None,
        },
        target_port: 80,
        target_address: None,
        compose: Some(Compose {
            directory: Some("/srv/project".into()),
            ..Compose::default()
        }),
    });
    value.container_engine = Some("engine-a".into());
    value.coverage.containers = Report::complete();
    run(root.path(), &mut value);
    assert_eq!(
        value.publications[0]
            .compose
            .as_ref()
            .unwrap()
            .directory_project
            .as_ref()
            .unwrap()
            .root,
        "/srv/project"
    );
    assert!(value.sockets.is_empty());
    assert!(value.processes.is_empty());
    value.validate().unwrap();
}
