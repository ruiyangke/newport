use super::*;
use serde_json::json;

use crate::port_discovery as d;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

fn listener(port: u16, pid: u32, address: &str, project: &str) -> d::Member {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let mut identity = DefaultHasher::new();
    (port, pid, address).hash(&mut identity);
    d::Member {
        observation: d::Observation::default(),
        reference: d::Reference {
            generation: 1,
            record: d::Record::Socket(NEXT.fetch_add(1, Ordering::Relaxed)),
        },
        scope: Some(Arc::new(d::Scope {
            network: (1, 2),
            processes: (1, 3),
            boot: "boot".into(),
        })),
        endpoint: d::Endpoint {
            port,
            address: address.parse().unwrap(),
            interface: None,
        },
        stale: false,
        kind: d::Kind::Socket(d::Socket {
            inode: identity.finish(),
            cookie: Some(identity.finish()),
            uid: 1000,
            account: None,
            ipv6_only: None,
            holder_coverage: d::HolderCoverage::Complete,
            units: vec![],
            holders: vec![Arc::new(d::Holder {
                identity: d::Process { pid, start: 100 },
                name: Some("worker".into()),
                executable: None,
                label: Some("worker".into()),
                account: None,
                uid: Some(1000),
                kinds: vec![],
                details: vec![],
                project: (!project.is_empty()).then(|| d::Project {
                    root: project.into(),
                    marker: String::new(),
                    checkout: None,
                }),
            })],
        }),
    }
}

fn socket(member: &mut d::Member) -> &mut d::Socket {
    let d::Kind::Socket(socket) = &mut member.kind else {
        panic!("socket fixture")
    };
    socket
}
fn owner(member: &mut d::Member) -> &mut d::Holder {
    Arc::make_mut(&mut socket(member).holders[0])
}
fn project(member: &mut d::Member) -> &mut d::Project {
    owner(member).project.as_mut().unwrap()
}
fn published(port: u16, name: &str, address: &str, root: &str, compose: Option<&str>) -> d::Member {
    let mut member = listener(port, 1, address, root);
    let d::Record::Socket(id) = member.reference.record else {
        unreachable!()
    };
    member.reference.record = d::Record::Publication(usize::try_from(id).unwrap());
    let mut project = owner(&mut member).project.clone();
    if let Some(project) = project.as_mut() {
        project.marker = "compose".into();
    }
    member.kind = d::Kind::Publication(d::Publication {
        engine: Some("engine".into()),
        container: format!("container:{name}"),
        name: name.into(),
        image: None,
        service: None,
        compose: compose.map(str::to_owned),
        directory: None,
        project,
        target_port: port,
        target_address: None,
        running: Some(true),
    });
    member
}
fn publication(member: &mut d::Member) -> &mut d::Publication {
    let d::Kind::Publication(publication) = &mut member.kind else {
        panic!("publication fixture")
    };
    publication
}
fn observed(member: d::Member) -> Port {
    Port::Observed(d::Row {
        members: vec![member],
    })
}

#[test]
fn publication_without_engine_identity_requires_reselection_after_refresh() {
    for verified in [false, true] {
        let mut member = published(8080, "web", "127.0.0.1", "", None);
        if !verified {
            publication(&mut member).engine = None;
        }
        let selection = d::Selection::capture([&member]);
        assert!(selection.contains(&member));
        let key = observed(member.clone()).key();
        member.reference.generation += 1;
        assert_eq!(selection.contains(&member), verified);
        assert_eq!(key == observed(member).key(), verified);
    }
}
fn pid(port: &Port) -> u32 {
    let d::Kind::Socket(socket) = &port.row().unwrap().members[0].kind else {
        panic!("socket fixture")
    };
    socket.holders[0].identity.pid
}
fn branch(port: &Port) -> Option<String> {
    port.worktree().and_then(|worktree| worktree.branch)
}
fn forward(id: &str, port: u16, project: &str) -> Value {
    json!({"id":id,"serverId":"s","remoteHost":"127.0.0.1","remotePort":port,"project":project})
}
fn checkout_listener(port: u16, path: &str, branch: &str) -> d::Member {
    let mut member = listener(port, u32::from(port), "127.0.0.1", "/repo");
    project(&mut member).checkout = Some(d::Checkout {
        path: path.into(),
        branch: Some(branch.into()),
        main: Some(false),
    });
    member
}

#[test]
fn saved_forward_keeps_observed_checkout_and_custom_name_when_listener_disappears() {
    let mut associations = WorktreeAssociations::default();
    let mut tunnel = forward("web", 3000, "/repo");
    tunnel["name"] = json!("My custom name");
    let saved = [tunnel.clone()];
    associations.group(
        vec![checkout_listener(3000, "/checkouts/ui/", "ui")],
        &saved,
        "s",
        &[],
    );
    let projects = associations.group(vec![], &saved, "s", &[]);
    let project = &projects[0];
    assert_eq!(project.key, "/repo");
    assert_eq!(project.worktrees[0].path, "/checkouts/ui");
    assert_eq!(project.worktrees[0].branch.as_deref(), Some("ui"));
    assert!(project.entries[0].is_saved());
    assert_eq!(
        project.entries[0].port.definition().unwrap()["name"],
        "My custom name"
    );
    assert_eq!(project.entries[0].forwards, [tunnel]);

    associations.group(
        vec![checkout_listener(3000, "/checkouts/ui", "renamed")],
        &saved,
        "s",
        &[],
    );
    let project = associations.group(vec![], &saved, "s", &[]).remove(0);
    assert_eq!(project.worktrees[0].branch.as_deref(), Some("renamed"));
}

#[test]
fn checkout_associations_are_server_scoped_and_invalidated_by_destination_changes() {
    let mut associations = WorktreeAssociations::default();
    let tunnel = forward("web", 3000, "/repo");
    let mut foreign = tunnel.clone();
    foreign["serverId"] = json!("other");
    let saved = [tunnel.clone(), foreign];
    associations.group(
        vec![checkout_listener(3000, "/checkouts/ui", "ui")],
        &saved,
        "s",
        &[],
    );
    assert!(
        associations.group(vec![], &saved, "other", &[])[0]
            .worktrees
            .is_empty()
    );
    assert_eq!(
        associations.group(vec![], &saved, "s", &[])[0]
            .worktrees
            .len(),
        1
    );

    for (field, replacement) in [
        ("remotePort", json!(3001)),
        ("remotePortEnd", json!(3002)),
        ("remoteHost", json!("192.0.2.1")),
        ("direction", json!("remote")),
        ("project", json!("/another")),
    ] {
        associations.group(
            vec![checkout_listener(3000, "/checkouts/ui", "ui")],
            &saved,
            "s",
            &[],
        );
        let mut edited = tunnel.clone();
        edited[field] = replacement;
        assert!(
            associations.group(vec![], &[edited], "s", &[])[0]
                .worktrees
                .is_empty(),
            "{field}"
        );
    }
    associations.group(
        vec![checkout_listener(3000, "/checkouts/ui", "ui")],
        &saved,
        "s",
        &[],
    );
    associations.group(vec![], &[], "s", &[]);
    assert!(associations.entries.is_empty());
    assert!(
        associations.group(vec![], &saved, "s", &[])[0]
            .worktrees
            .is_empty()
    );
}

#[test]
fn live_checkout_rename_updates_stopped_forwards_and_survives_all_listeners_stopping() {
    let mut associations = WorktreeAssociations::default();
    let saved = [forward("web", 3000, "/repo"), forward("api", 3001, "/repo")];
    associations.group(
        vec![
            checkout_listener(3000, "/checkouts/ui", "old"),
            checkout_listener(3001, "/checkouts/ui", "old"),
        ],
        &saved,
        "s",
        &[],
    );
    // Even an unsaved listener carries authoritative checkout metadata.
    let live = associations.group(
        vec![checkout_listener(3002, "/checkouts/ui", "new")],
        &saved,
        "s",
        &[],
    );
    assert_eq!(live[0].entries.len(), 3);
    assert!(
        live[0]
            .entries
            .iter()
            .all(|entry| branch(&entry.port).as_deref() == Some("new"))
    );
    assert_eq!(live[0].worktrees.len(), 1);
    let stopped = associations.group(vec![], &saved, "s", &[]);
    assert_eq!(stopped[0].entries.len(), 2);
    assert!(
        stopped[0]
            .entries
            .iter()
            .all(|entry| entry.is_saved() && branch(&entry.port).as_deref() == Some("new"))
    );
    assert_eq!(stopped[0].worktrees[0].branch.as_deref(), Some("new"));
}

#[test]
fn ambiguous_range_or_unknown_replacement_drops_checkout_association() {
    let mut associations = WorktreeAssociations::default();
    let mut tunnel = forward("range", 3000, "/repo");
    tunnel["remotePortEnd"] = json!(3002);
    let saved = [tunnel];
    let first = checkout_listener(3000, "/checkouts/ui", "ui");
    associations.group(vec![first.clone()], &saved, "s", &[]);
    assert_eq!(
        associations.group(vec![], &saved, "s", &[])[0]
            .worktrees
            .len(),
        1
    );
    associations.group(
        vec![
            first.clone(),
            checkout_listener(3001, "/checkouts/api", "api"),
            first.clone(),
        ],
        &saved,
        "s",
        &[],
    );
    assert!(
        associations.group(vec![], &saved, "s", &[])[0]
            .worktrees
            .is_empty()
    );
    associations.group(vec![first], &saved, "s", &[]);
    associations.group(
        vec![listener(3000, 9000, "127.0.0.1", "/repo")],
        &saved,
        "s",
        &[],
    );
    assert!(
        associations.group(vec![], &saved, "s", &[])[0]
            .worktrees
            .is_empty()
    );
}

#[test]
fn listener_selection_does_not_expand_to_hidden_new_or_saved_only_rows() {
    let ports = (8000..8003)
        .map(|port| listener(port, u32::from(port), "127.0.0.1", "/repo"))
        .collect();
    let original = group(ports, &[], "s", &[]).remove(0);
    let selection = ListenerSelection::capture(&original.entries[..2]);
    // The first selected listener disappeared. The hidden listener, a new
    // discovery, and a saved-only forward must not replace it in the batch.
    let latest = group(
        (8001..8004)
            .map(|port| listener(port, u32::from(port), "127.0.0.1", "/repo"))
            .collect(),
        &[forward("saved-only", 8000, "/repo")],
        "s",
        &[],
    )
    .remove(0);
    let selected = selection.resolve(latest.entries.clone());
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].port.number(), Some(8001));
    assert!(
        ListenerSelection::capture(&[])
            .resolve(latest.entries)
            .is_empty()
    );
}

#[test]
fn listener_selection_rejects_replaced_process_destination_and_checkout() {
    let mut port = checkout_listener(8000, "/repo/feature/", "feature");
    let original = group(vec![port.clone()], &[], "s", &[]).remove(0);
    let selection = ListenerSelection::capture(&original.entries);
    for case in 0..5 {
        let mut changed = port.clone();
        match case {
            0 => owner(&mut changed).identity.pid = 456,
            1 => changed.endpoint.port = 8001,
            2 => changed.endpoint.address = "192.0.2.1".parse().unwrap(),
            3 => project(&mut changed).checkout.as_mut().unwrap().path = "/repo/other".into(),
            _ => project(&mut changed).root = "/different".into(),
        }
        let latest = group(vec![changed], &[], "s", &[]).remove(0);
        assert!(selection.resolve(latest.entries).is_empty(), "case {case}");
    }
    let checkout = project(&mut port).checkout.as_mut().unwrap();
    checkout.path = "/repo/feature".into();
    checkout.branch = Some("renamed-branch".into());
    owner(&mut port).name = Some("node".into());
    let latest = group(vec![port], &[], "s", &[]).remove(0);
    assert_eq!(selection.resolve(latest.entries.clone()), latest.entries);
}

#[test]
fn listener_selection_plans_with_latest_saved_forward_and_runtime() {
    use crate::{forwarding::discovered_tunnel, forwarding_batch, model::Tunnel};
    use uuid::Uuid;

    let server = Uuid::new_v4();
    let ports: Vec<_> = (8000..8003)
        .map(|port| listener(port, u32::from(port), "127.0.0.1", "/repo"))
        .collect();
    let original_forward = discovered_tunnel(&[], server, "127.0.0.1", 8000).unwrap();
    let original = group(
        ports.clone(),
        &[serde_json::to_value(&original_forward).unwrap()],
        &server.to_string(),
        &[],
    )
    .remove(0);
    let selection = ListenerSelection::capture(&original.entries[..2]);
    let mut latest_forward = original_forward.clone();
    latest_forward.id = Uuid::new_v4();
    latest_forward.local_port = 18000;
    latest_forward.name = "Changed after render".into();
    let latest = group(
        ports,
        &[serde_json::to_value(&latest_forward).unwrap()],
        &server.to_string(),
        &[],
    )
    .remove(0);
    let entries = selection
        .resolve(latest.entries)
        .into_iter()
        .map(|entry| forwarding_batch::Entry {
            destination: Some(forwarding_batch::Destination {
                address: entry.port.destination(None).unwrap().host,
                port: entry.port.number().unwrap(),
                name: "Discovered listener".into(),
            }),
            forwards: entry
                .forwards
                .into_iter()
                .map(|value| serde_json::from_value::<Tunnel>(value).unwrap())
                .collect(),
        })
        .collect::<Vec<_>>();
    let allocated = [latest_forward.clone()];
    let connected = forwarding_batch::plan(
        server,
        "/repo",
        &entries,
        &allocated,
        &HashSet::from([latest_forward.id]),
    )
    .unwrap();
    assert!(connected.connect.is_empty());
    assert_eq!(connected.stop, [latest_forward.clone()]);
    assert_eq!(connected.create.len(), 1);
    assert_eq!(connected.create[0].remote_port, 8001);
    let disconnected =
        forwarding_batch::plan(server, "/repo", &entries, &allocated, &HashSet::new()).unwrap();
    assert_eq!(disconnected.connect, [latest_forward]);
    assert!(disconnected.stop.is_empty());
    assert_eq!(disconnected.create.len(), 1);
    assert_eq!(disconnected.create[0].remote_port, 8001);
}

#[test]
fn project_identity_prefers_repository_root_then_compose_directory_and_name() {
    let mut member = published(5432, "database", "0.0.0.0", "/srv/repo/", Some("database"));
    publication(&mut member).directory = Some("/srv/repo/db/".into());
    assert_eq!(project_key(&observed(member.clone())), "/srv/repo");
    publication(&mut member).project = None;
    assert_eq!(project_key(&observed(member.clone())), "/srv/repo/db");
    publication(&mut member).directory = None;
    assert_eq!(project_key(&observed(member.clone())), "compose:database");
    publication(&mut member).compose = None;
    assert_eq!(project_key(&observed(member)), OTHER);
    assert_eq!(
        project_key(&observed(listener(1, 1, "127.0.0.1", "/"))),
        "/"
    );
}

#[test]
fn project_names_sources_and_catch_all() {
    let mut repository = listener(3000, 3, "127.0.0.1", "/srv/atlas.git/");
    project(&mut repository).marker = ".git".into();
    let mut package = listener(7000, 5, "127.0.0.1", "/srv/package");
    project(&mut package).marker = "Cargo.toml".into();
    let ports = vec![
        listener(22, 1, "127.0.0.1", ""),
        published(
            5432,
            "ledger-db-1",
            "0.0.0.0",
            "/srv/odd-directory",
            Some("ledger"),
        ),
        repository,
        published(6379, "cache", "0.0.0.0", "", Some("cache")),
        package,
    ];
    let projects = group(ports, &[], "s", &[]);
    assert_eq!(
        projects
            .iter()
            .map(|p| (p.name.as_str(), p.source))
            .collect::<Vec<_>>(),
        vec![
            ("atlas", Source::Vcs),
            ("cache", Source::Compose),
            ("ledger", Source::Compose),
            ("package", Source::Marker),
            ("Other services", Source::Other)
        ]
    );
    assert_eq!(projects[0].marker.as_deref(), Some(".git"));
    assert_eq!(projects[1].path, None);
    assert_eq!(projects[2].path.as_deref(), Some("/srv/odd-directory"));
    assert_eq!(projects[4].marker, None);
}

#[test]
fn known_names_fold_nested_roots_at_the_longest_path_boundary() {
    let ports = vec![
        listener(1, 1, "127.0.0.1", "/srv/repo/packages/api"),
        listener(2, 2, "127.0.0.1", "/srv/repo/ui"),
        listener(3, 3, "127.0.0.1", "/srv/repository"),
    ];
    let known = vec![
        json!({"name":"Repository", "path":"/srv/repo/"}),
        json!({"name":"API", "path":{"display":"/srv/repo/packages/","bytesB64":""}, "serverId":"s"}),
        json!({"name":"Wrong server", "path":"/srv/repository", "serverId":"elsewhere"}),
    ];
    let projects = group(ports, &[], "s", &known);
    assert_eq!(
        projects
            .iter()
            .map(|p| (p.key.as_str(), p.name.as_str(), p.source))
            .collect::<Vec<_>>(),
        vec![
            ("/srv/repo/packages", "API", Source::Saved),
            ("/srv/repo", "Repository", Source::Saved),
            ("/srv/repository", "repository", Source::Marker),
        ]
    );
}

#[test]
fn known_root_slash_absorbs_absolute_paths_but_not_other_or_compose_names() {
    let projects = group(
        vec![
            listener(1, 1, "127.0.0.1", "/"),
            listener(2, 2, "127.0.0.1", "/srv/app"),
            listener(3, 3, "127.0.0.1", ""),
            published(4, "named", "0.0.0.0", "", Some("named")),
        ],
        &[],
        "s",
        &[json!({"name":"Root", "path":"/"})],
    );
    let root = projects.iter().find(|p| p.key == "/").unwrap();
    assert_eq!(root.name, "Root");
    assert_eq!(root.entries.len(), 2);
    assert_eq!(projects.len(), 3);
    assert_eq!(
        group(vec![listener(1, 1, "127.0.0.1", "/")], &[], "s", &[])[0].name,
        "/"
    );
}

#[test]
fn dual_stack_preserves_all_addresses_and_owner_isolation_without_choosing_a_destination() {
    let ports = vec![
        listener(3000, 7, "::", ""),
        listener(3000, 7, "192.0.2.2", ""),
        listener(3000, 8, "::1", ""),
        listener(3000, 7, "0.0.0.0", ""),
        listener(3000, 7, "127.0.0.1", ""),
    ];
    let mut ipv6 = forward("v6", 3000, "");
    ipv6["remoteHost"] = json!("::1");
    let projects = group(ports, &[ipv6, forward("v4", 3000, "")], "s", &[]);
    let entries = &projects[0].entries;
    assert_eq!(entries.len(), 2);
    assert_eq!(pid(&entries[0].port), 7);
    assert_eq!(
        entries[0].port.destination(None),
        Err(d::DestinationError::ChooseMember)
    );
    assert_eq!(
        entries[0].addresses,
        ["0.0.0.0", "127.0.0.1", "192.0.2.2", "::"]
    );
    assert_eq!(
        entries[0]
            .forwards
            .iter()
            .map(|t| text(t, "id"))
            .collect::<Vec<_>>(),
        ["v4", "v6"]
    );
    assert_eq!(pid(&entries[1].port), 8);
    assert_eq!(entries[1].forwards.len(), 1);
}

#[test]
fn publications_group_by_container_and_unknown_socket_holders_stay_separate() {
    let mut a = listener(3001, 1, "::", "");
    socket(&mut a).holders.clear();
    let mut b = listener(3001, 2, "0.0.0.0", "");
    socket(&mut b).holders.clear();
    let ports = vec![
        published(3000, "api", "::", "", None),
        published(3000, "api", "0.0.0.0", "", None),
        published(3000, "web", "0.0.0.0", "", None),
        a,
        b,
    ];
    let groups = group(ports, &[], "s", &[]);
    assert_eq!(groups[0].entries.len(), 4);
    let api = groups[0]
        .entries
        .iter()
        .find(|entry| entry.port.label() == "api")
        .unwrap();
    assert_eq!(api.addresses.len(), 2);
    assert!(
        groups[0]
            .entries
            .iter()
            .filter(|entry| entry.port.label() != "api")
            .all(|entry| entry.addresses.len() == 1)
    );
}

#[test]
fn saved_leftovers_keep_filed_projects_and_exclude_foreign_servers() {
    let mut reverse = forward("reverse", 5432, "");
    reverse["direction"] = json!("remote");
    let mut foreign = forward("foreign", 9999, "/srv/foreign");
    foreign["serverId"] = json!("elsewhere");
    let projects = group(
        vec![listener(5432, 1, "0.0.0.0", "/srv/db")],
        &[
            forward("db", 5432, "/srv/other"),
            forward("filed", 9999, "/srv/repo/packages/api"),
            reverse.clone(),
            foreign,
        ],
        "s",
        &[json!({"path":"/srv/repo", "name":"My application"})],
    );
    assert_eq!(projects.len(), 3);
    assert_eq!(projects[0].key, "/srv/db");
    assert!(!projects[0].entries[0].is_saved());
    assert_eq!(projects[0].entries[0].forwards[0]["id"], "db");
    assert_eq!(projects[1].name, "My application");
    assert!(projects[1].entries[0].is_saved());
    assert!(projects[1].entries[0].addresses.is_empty());
    assert_eq!(
        projects[1].entries[0].port.definition().unwrap()["id"],
        "filed"
    );
    assert_eq!(projects[2].entries[0].forwards, [reverse]);
}

#[test]
fn filed_forward_attributes_unknown_listener_and_ranges_attach_to_each_reached_port() {
    let mut range = forward("range", 3000, "/srv/repo/pkg");
    range["remotePortEnd"] = json!(3002);
    let projects = group(
        vec![
            listener(3002, 2, "127.0.0.1", ""),
            listener(3000, 1, "127.0.0.1", ""),
        ],
        &[range.clone()],
        "s",
        &[json!({"name":"Repo", "path":"/srv/repo"})],
    );
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].key, "/srv/repo");
    assert_eq!(projects[0].entries.len(), 2);
    assert_eq!(projects[0].entries[0].port.number(), Some(3000));
    assert!(
        projects[0]
            .entries
            .iter()
            .all(|e| !e.is_saved() && e.forwards == [range.clone()])
    );
}

#[test]
fn linked_worktrees_share_repository_and_main_checkout_sorts_first() {
    let mut ports = vec![
        checkout_listener(3001, "/srv/search/", "feature/search"),
        checkout_listener(3000, "/srv/repo", "main"),
        checkout_listener(3002, "/srv/search", "feature/search"),
    ];
    for port in &mut ports {
        project(port).root = "/srv/repo".into();
    }
    project(&mut ports[1]).checkout.as_mut().unwrap().main = Some(true);
    let projects = group(ports, &[], "s", &[]);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].entries.len(), 3);
    assert_eq!(
        projects[0].worktrees,
        [
            Worktree {
                path: "/srv/repo".into(),
                branch: Some("main".into()),
                main: Some(true)
            },
            Worktree {
                path: "/srv/search".into(),
                branch: Some("feature/search".into()),
                main: Some(false)
            }
        ]
    );
}

#[test]
fn narrowed_selection_removes_forwards_of_unselected_siblings() {
    let a = listener(3000, 7, "0.0.0.0", "/repo");
    let b = listener(3000, 7, "::", "/repo");
    let original = group(vec![a.clone()], &[], "s", &[]).remove(0);
    let selection = ListenerSelection::capture(&original.entries);
    let v4 = forward("v4", 3000, "/repo");
    let mut v6 = forward("v6", 3000, "/repo");
    v6["remoteHost"] = json!("::1");
    let latest = group(vec![a, b], &[v4.clone(), v6], "s", &[]).remove(0);
    assert_eq!(latest.entries.len(), 1);
    assert_eq!(latest.entries[0].forwards.len(), 2);
    let selected = selection.resolve(latest.entries);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].addresses, ["0.0.0.0"]);
    assert_eq!(selected[0].forwards, [v4]);
}

#[test]
fn system_services_are_separate_from_saved_projects_and_unknown_owners() {
    let mut service = listener(22, 1, "0.0.0.0", "");
    socket(&mut service).holders.clear();
    socket(&mut service).units.push(d::Unit {
        manager_uid: None,
        name: "ssh.socket".into(),
        socket_cgroup: true,
    });
    let mut unknown = listener(53, 2, "127.0.0.1", "");
    socket(&mut unknown).holders.clear();
    socket(&mut unknown).uid = 0;
    let projects = group(
        vec![service, unknown],
        &[],
        "s",
        &[json!({"name":"Root", "path":"/"})],
    );
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].key, SYSTEM);
    assert_eq!(projects[0].source, Source::System);
    assert_eq!(projects[0].entries[0].port.label(), "ssh.socket");
    assert!(projects[0].path.is_none());
    assert_eq!(projects[1].key, OTHER);
    assert_eq!(projects[1].entries[0].port.label(), "TCP listener");
}

#[test]
fn a_dual_stack_listener_on_any_ipv6_address_is_reached_through_ipv4_loopback() {
    let saved = |listener: d::Member| {
        group(vec![listener], &[forward("web", 3000, "")], "s", &[])
            .into_iter()
            .flat_map(|project| project.entries)
            .find(|entry| entry.forwards.iter().any(|t| t["id"] == "web"))
            .unwrap()
            .is_saved()
    };
    // Node, Java and others listen on `[::]`, which also accepts IPv4.
    assert!(!saved(listener(3000, 1, "::", "")));
    let mut only_ipv6 = listener(3000, 1, "::", "");
    socket(&mut only_ipv6).ipv6_only = Some(true);
    assert!(saved(only_ipv6));
    assert!(saved(listener(3000, 1, "::1", "")));
}
