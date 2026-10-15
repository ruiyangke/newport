//! Plan local forwards for an explicit project/worktree scope.
//! Callers supply the intended rows: whole-project actions include all rows,
//! while worktree actions use their filtered selection resolved against current data.
use crate::{
    forwarding::discovered_tunnel,
    model::{Direction, Tunnel},
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

const MAX_BATCH: usize = 256;

#[derive(Debug, PartialEq)]
pub struct ResolvedBatch {
    /// The complete proposed configuration, including unchanged other servers.
    pub tunnels: Vec<Tunnel>,
    /// Existing forwards first, followed by resolved creations, without repeats.
    pub connect: Vec<Uuid>,
}

/// Resolve a batch against authoritative configuration before persisting anything.
/// Revalidate resolution against the committed configuration revision, then
/// admit only those exact saved definitions. Transport retries are not safe.
pub fn resolve_batch(
    server: Uuid,
    create: &[Tunnel],
    connect: &[Tunnel],
    latest: &[Tunnel],
) -> Result<ResolvedBatch, String> {
    validate_batch(server, create, connect)?;
    let mut result = ResolvedBatch {
        tunnels: latest.to_vec(),
        connect: Vec::new(),
    };
    let mut connecting = HashSet::new();
    for requested in connect {
        let saved = latest
            .iter()
            .find(|t| t.id == requested.id)
            .ok_or("A selected forward no longer exists; refresh the project")?;
        if saved.server_id != server || saved.direction != Direction::Local {
            return Err(
                "A selected forward changed server or direction; refresh the project".into(),
            );
        }
        if connecting.insert(saved.id) {
            result.connect.push(saved.id);
        }
    }
    let mut creating = HashSet::new();
    for requested in create {
        if !creating.insert(requested.id) {
            continue;
        }
        let mut resolved = discovered_tunnel(
            &result.tunnels,
            server,
            &requested.remote_host,
            requested.remote_port,
        )?;
        if result
            .tunnels
            .iter()
            .any(|t| t.id == requested.id && t.id != resolved.id)
        {
            return Err("A new forward ID already belongs to another saved destination".into());
        }
        if !result.tunnels.iter().any(|t| t.id == resolved.id) {
            resolved.id = requested.id;
            resolved.name = requested.name.clone();
            resolved.project = requested.project.clone();
            resolved.auto_connect = requested.auto_connect;
            resolved.auto_reconnect = requested.auto_reconnect;
            resolved.validate_unique(&result.tunnels)?;
            result.tunnels.push(resolved.clone());
        }
        if connecting.insert(resolved.id) {
            result.connect.push(resolved.id);
        }
    }
    Ok(result)
}

fn validate_batch(server: Uuid, create: &[Tunnel], connect: &[Tunnel]) -> Result<(), String> {
    if server.is_nil() {
        return Err("A server is required".into());
    }
    let mut seen: HashMap<Uuid, (&Tunnel, bool)> = HashMap::new();
    for (t, creating) in create
        .iter()
        .map(|t| (t, true))
        .chain(connect.iter().map(|t| (t, false)))
    {
        if t.server_id != server || t.id.is_nil() || t.direction != Direction::Local {
            return Err("All forwards must be local forwards for the selected server".into());
        }
        if creating {
            if t.local_port_end.is_some() || t.remote_port_end.is_some() {
                return Err("New batch forwards must describe a single discovered port".into());
            }
            t.pairs()?;
        }
        if let Some((previous, was_creating)) = seen.get(&t.id) {
            if *was_creating != creating || *previous != t {
                return Err(
                    "A forward ID must have one definition and cannot be both created and connected"
                        .into(),
                );
            }
        } else {
            seen.insert(t.id, (t, creating));
            if seen.len() > MAX_BATCH {
                return Err("Choose between 1 and 256 forwards".into());
            }
        }
    }
    if seen.is_empty() {
        return Err("Choose between 1 and 256 forwards".into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Destination {
    pub address: String,
    pub port: u16,
    pub name: String,
}
#[derive(Clone, Debug)]
pub struct Entry {
    /// None for a saved forward with no discovered listener.
    pub destination: Option<Destination>,
    pub forwards: Vec<Tunnel>,
}
#[derive(Default, Debug)]
pub struct Plan {
    pub create: Vec<Tunnel>,
    pub connect: Vec<Tunnel>,
    pub stop: Vec<Tunnel>,
    /// A failed creation plan must not hide the ability to stop active forwards.
    pub creation_error: Option<String>,
}
impl Plan {
    pub fn can_forward(&self) -> bool {
        !self.create.is_empty() || !self.connect.is_empty() || self.creation_error.is_some()
    }
}

/// Include in-flight creations in `allocated` to avoid proposing their local
/// ports to another concurrent batch. The backend still validates on save.
pub fn plan(
    server: Uuid,
    project: &str,
    entries: &[Entry],
    allocated: &[Tunnel],
    active: &HashSet<Uuid>,
) -> Result<Plan, String> {
    if project.is_empty() {
        return Err("Choose a named project for batch forwarding".into());
    }
    plan_entries(server, Some(project), entries, allocated, active)
}

/// Plan one row, including an unattributed listener in Other services. An empty
/// project leaves a newly saved forward unfiled; existing forwards retain their
/// own metadata. Multi-row controls must continue to use a named project.
pub fn plan_single(
    server: Uuid,
    project: &str,
    entry: &Entry,
    allocated: &[Tunnel],
    active: &HashSet<Uuid>,
) -> Result<Plan, String> {
    plan_entries(
        server,
        (!project.is_empty()).then_some(project),
        std::slice::from_ref(entry),
        allocated,
        active,
    )
}

fn plan_entries(
    server: Uuid,
    project: Option<&str>,
    entries: &[Entry],
    allocated: &[Tunnel],
    active: &HashSet<Uuid>,
) -> Result<Plan, String> {
    if server.is_nil() {
        return Err("A server is required for forwarding".into());
    }
    let mut result = Plan::default();
    let mut taken = allocated.to_vec();
    let mut seen = HashSet::new();
    let mut include = |t: &Tunnel, result: &mut Plan| -> Result<(), String> {
        if t.server_id != server {
            return Err("A forward belongs to another server; refresh the project".into());
        }
        // Project controls do not stop shares from this computer.
        if t.direction == Direction::Local && seen.insert(t.id) {
            if active.contains(&t.id) {
                result.stop.push(t.clone());
            } else {
                result.connect.push(t.clone());
            }
        }
        Ok(())
    };
    for entry in entries {
        for tunnel in &entry.forwards {
            include(tunnel, &mut result)?;
        }
        if !entry.forwards.is_empty() {
            continue;
        }
        let Some(destination) = &entry.destination else {
            continue;
        };
        let mut tunnel =
            match discovered_tunnel(&taken, server, &destination.address, destination.port) {
                Ok(tunnel) => tunnel,
                Err(error) => {
                    result.creation_error =
                        Some(format!("Cannot forward port {}: {error}", destination.port));
                    continue;
                }
            };
        if taken.iter().any(|t| t.id == tunnel.id) {
            // A range or another row may already cover this destination.
            if !result.create.iter().any(|t| t.id == tunnel.id) {
                include(&tunnel, &mut result)?;
            }
            continue;
        }
        tunnel.name = destination.name.clone();
        tunnel.project = project.map(str::to_owned);
        taken.push(tunnel.clone());
        result.create.push(tunnel);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn template(server: Uuid, port: u16) -> Tunnel {
        discovered_tunnel(&[], server, "localhost", port).unwrap()
    }
    fn listener(port: u16) -> Entry {
        Entry {
            destination: Some(Destination {
                address: "0.0.0.0".into(),
                port,
                name: "Vite".into(),
            }),
            forwards: vec![],
        }
    }

    #[test]
    fn single_other_service_keeps_observed_name_and_is_saved_without_project() {
        let server = Uuid::new_v4();
        let entry = listener(4173);
        let result = plan_single(server, "", &entry, &[], &HashSet::new()).unwrap();
        assert_eq!(result.create.len(), 1);
        let created = &result.create[0];
        assert_eq!(created.name, "Vite");
        assert_eq!(created.project, None);
        assert_eq!(created.server_id, server);
        assert_eq!(created.remote_host, "127.0.0.1");
        assert_eq!((created.local_port, created.remote_port), (4173, 4173));
        assert!(result.connect.is_empty() && result.stop.is_empty());
        assert!(result.creation_error.is_none());
        let resolved = resolve_batch(server, &result.create, &result.connect, &[]).unwrap();
        assert_eq!(resolved.tunnels, result.create);
        assert_eq!(resolved.connect, vec![created.id]);
        assert!(plan(server, "", &[entry], &[], &HashSet::new()).is_err());
    }

    #[test]
    fn single_named_and_linked_rows_preserve_project_and_requested_name() {
        let server = Uuid::new_v4();
        for (project, name) in [
            ("/srv/atlas", "Vite"),
            ("/srv/atlas", "Vite · feat/search"),
            ("compose:shop", "db"),
        ] {
            let mut entry = listener(4173);
            entry.destination.as_mut().unwrap().name = name.into();
            let result = plan_single(server, project, &entry, &[], &HashSet::new()).unwrap();
            assert_eq!(result.create.len(), 1);
            assert_eq!(result.create[0].name, name);
            assert_eq!(result.create[0].project.as_deref(), Some(project));
        }
    }

    #[test]
    fn single_rows_reuse_ranges_without_overwriting_saved_metadata() {
        let server = Uuid::new_v4();
        let mut saved = template(server, 3000);
        saved.name = "Existing range".into();
        saved.project = Some("/srv/original".into());
        saved.local_port = 4100;
        saved.local_port_end = Some(4102);
        saved.remote_port_end = Some(3002);
        let entry = listener(3001);
        let allocated = std::slice::from_ref(&saved);
        for project in ["", "/srv/new"] {
            let idle = plan_single(server, project, &entry, allocated, &HashSet::new()).unwrap();
            assert!(idle.create.is_empty() && idle.stop.is_empty());
            assert_eq!(idle.connect, vec![saved.clone()]);
            let active = plan_single(
                server,
                project,
                &entry,
                allocated,
                &HashSet::from([saved.id]),
            )
            .unwrap();
            assert!(active.create.is_empty() && active.connect.is_empty());
            assert_eq!(active.stop, vec![saved.clone()]);
            // The backend also reuses a range saved after this row was planned.
            let earlier = plan_single(server, project, &entry, &[], &HashSet::new()).unwrap();
            let resolved =
                resolve_batch(server, &earlier.create, &earlier.connect, allocated).unwrap();
            assert_eq!(resolved.tunnels, vec![saved.clone()]);
            assert_eq!(resolved.connect, vec![saved.id]);
        }
    }

    #[test]
    fn single_rows_require_a_server_even_without_a_project() {
        let entry = listener(4173);
        for project in ["", "/srv/atlas"] {
            assert!(plan_single(Uuid::nil(), project, &entry, &[], &HashSet::new()).is_err());
            assert!(plan(
                Uuid::nil(),
                project,
                std::slice::from_ref(&entry),
                &[],
                &HashSet::new()
            )
            .is_err());
        }
    }

    #[test]
    fn scopes_plans_and_allocates_across_servers_and_pending_creations() {
        let server = Uuid::new_v4();
        let mut occupied = discovered_tunnel(&[], Uuid::new_v4(), "localhost", 4173).unwrap();
        occupied.local_port_end = Some(4174);
        let pending = discovered_tunnel(&[occupied.clone()], server, "localhost", 9000).unwrap();
        let entries = [listener(4173), listener(4174), listener(4174)];
        let full = plan(
            server,
            "/srv/atlas",
            &entries,
            &[occupied.clone(), pending.clone()],
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(
            full.create
                .iter()
                .map(|t| (t.local_port, t.remote_port))
                .collect::<Vec<_>>(),
            vec![(4175, 4173), (4176, 4174)]
        );
        assert!(full
            .create
            .iter()
            .all(|t| t.project.as_deref() == Some("/srv/atlas") && t.remote_host == "127.0.0.1"));
        let scope = plan(
            server,
            "/srv/atlas",
            &entries[1..2],
            &[occupied, pending],
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(scope.create.len(), 1);
        assert_eq!(scope.create[0].remote_port, 4174);
    }
    #[test]
    fn reconnects_idle_deduplicates_ranges_and_excludes_reverse_shares() {
        let server = Uuid::new_v4();
        let mut range = discovered_tunnel(&[], server, "localhost", 3000).unwrap();
        range.remote_port_end = Some(3002);
        let idle = discovered_tunnel(&[], server, "localhost", 7000).unwrap();
        let mut reverse = idle.clone();
        reverse.id = Uuid::new_v4();
        reverse.direction = Direction::Remote;
        let entries = [
            Entry {
                destination: None,
                forwards: vec![range.clone(), idle.clone(), reverse],
            },
            Entry {
                destination: None,
                forwards: vec![range.clone()],
            },
            listener(3001),
        ];
        let result = plan(
            server,
            "/srv/atlas",
            &entries,
            &[range.clone(), idle.clone()],
            &HashSet::from([range.id]),
        )
        .unwrap();
        assert!(result.create.is_empty());
        assert_eq!(
            result.stop.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![range.id]
        );
        assert_eq!(
            result.connect.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![idle.id]
        );
        assert!(result.can_forward());
    }
    #[test]
    fn rejects_other_services_cross_server_rows_and_exhausted_local_ports() {
        let server = Uuid::new_v4();
        assert!(plan(server, "", &[listener(8000)], &[], &HashSet::new()).is_err());
        let foreign = discovered_tunnel(&[], Uuid::new_v4(), "localhost", 65535).unwrap();
        let live = discovered_tunnel(&[], server, "localhost", 8000).unwrap();
        let result = plan(
            server,
            "/srv/app",
            &[
                listener(65535),
                Entry {
                    destination: None,
                    forwards: vec![live.clone()],
                },
            ],
            std::slice::from_ref(&foreign),
            &HashSet::from([live.id]),
        )
        .unwrap();
        assert!(result.creation_error.is_some());
        assert!(result.create.is_empty());
        assert_eq!(result.stop[0].id, live.id);
        assert!(plan(
            server,
            "/srv/app",
            &[Entry {
                destination: None,
                forwards: vec![foreign]
            }],
            &[],
            &HashSet::new()
        )
        .is_err());
    }

    #[test]
    fn resolution_reallocates_ports_claimed_since_planning_and_keeps_template_metadata() {
        let server = Uuid::new_v4();
        let mut first = template(server, 3000);
        first.name = "Web · feature".into();
        first.project = Some("/srv/app".into());
        first.auto_connect = true;
        first.auto_reconnect = false;
        let second = template(server, 3001);
        let existing = template(server, 8000);
        let mut occupied = template(Uuid::new_v4(), 3000);
        occupied.local_port_end = Some(3001);
        occupied.remote_port_end = Some(3001);
        let latest = vec![occupied, existing.clone()];
        let resolved = resolve_batch(
            server,
            &[first.clone(), second.clone()],
            std::slice::from_ref(&existing),
            &latest,
        )
        .unwrap();
        let mut expected_first = first.clone();
        expected_first.local_port = 3002;
        let mut expected_second = second.clone();
        expected_second.local_port = 3003;
        assert_eq!(resolved.tunnels[..latest.len()], latest);
        assert_eq!(resolved.tunnels[2..], [expected_first, expected_second]);
        assert_eq!(resolved.connect, vec![existing.id, first.id, second.id]);
        assert_eq!(first.local_port, 3000);
    }

    #[test]
    fn resolution_reuses_just_saved_destinations_ranges_and_ids_without_overwriting() {
        let server = Uuid::new_v4();
        let mut saved = template(server, 3000);
        saved.name = "Keep saved name".into();
        saved.project = Some("/srv/original".into());
        saved.local_port = 4100;
        saved.local_port_end = Some(4101);
        saved.remote_port_end = Some(3001);
        let mut first = template(server, 3001);
        first.remote_host = "0.0.0.0".into();
        first.name = "Stale creation name".into();
        first.project = Some("/srv/stale".into());
        let duplicate_destination = template(server, 3000);
        let mut same_id = first.clone();
        same_id.id = saved.id;
        let latest = vec![saved.clone()];
        let resolved = resolve_batch(
            server,
            &[first.clone(), first, duplicate_destination],
            &[saved.clone(), saved.clone()],
            &latest,
        )
        .unwrap();
        assert_eq!(resolved.tunnels, latest);
        assert_eq!(resolved.connect, vec![saved.id]);
        let resolved = resolve_batch(server, &[same_id], &[], &latest).unwrap();
        assert_eq!(resolved.tunnels, latest);
        assert_eq!(resolved.connect, vec![saved.id]);
    }

    #[test]
    fn resolution_merges_new_duplicate_destinations_and_preserves_first_definition() {
        let server = Uuid::new_v4();
        let first = template(server, 3000);
        let mut duplicate = template(server, 3000);
        duplicate.remote_host = "0.0.0.0".into();
        duplicate.name = "Another row".into();
        let resolved = resolve_batch(server, &[first.clone(), duplicate], &[], &[]).unwrap();
        assert_eq!(resolved.tunnels, vec![first.clone()]);
        assert_eq!(resolved.connect, vec![first.id]);
    }

    #[test]
    fn resolution_rejects_conflicting_preexisting_ids_instead_of_overwriting() {
        let server = Uuid::new_v4();
        let requested = template(server, 3000);
        let mut changed_destination = requested.clone();
        changed_destination.remote_port = 4000;
        let mut changed_server = requested.clone();
        changed_server.server_id = Uuid::new_v4();
        let mut changed_direction = requested.clone();
        changed_direction.direction = Direction::Remote;
        for existing in [changed_destination, changed_server, changed_direction] {
            let latest = vec![existing];
            let before = latest.clone();
            assert!(resolve_batch(server, std::slice::from_ref(&requested), &[], &latest).is_err());
            assert_eq!(latest, before);
        }
        // A different saved ID at the destination does not excuse an ID collision.
        let mut conflicting_id = template(server, 4000);
        conflicting_id.id = requested.id;
        let destination = template(server, 3000);
        assert!(resolve_batch(server, &[requested], &[], &[conflicting_id, destination]).is_err());
    }

    #[test]
    fn resolution_validates_every_connect_against_latest_configuration() {
        let server = Uuid::new_v4();
        let requested = template(server, 3000);
        let create = template(server, 4000);
        let mut moved_server = requested.clone();
        moved_server.server_id = Uuid::new_v4();
        let mut moved_direction = requested.clone();
        moved_direction.direction = Direction::Remote;
        for latest in [vec![], vec![moved_server], vec![moved_direction]] {
            assert!(resolve_batch(
                server,
                std::slice::from_ref(&create),
                std::slice::from_ref(&requested),
                &latest,
            )
            .is_err());
        }
        let mut edited = requested.clone();
        edited.local_port = 5000;
        edited.remote_port = 6000;
        let resolved = resolve_batch(
            server,
            &[],
            &[requested.clone(), requested.clone()],
            std::slice::from_ref(&edited),
        )
        .unwrap();
        assert_eq!(resolved.tunnels, vec![edited]);
        assert_eq!(resolved.connect, vec![requested.id]);
    }

    #[test]
    fn resolution_rejects_invalid_templates_and_ambiguous_ids_atomically() {
        let server = Uuid::new_v4();
        let valid = template(server, 3000);
        let mut nil_id = valid.clone();
        nil_id.id = Uuid::nil();
        let mut zero_port = valid.clone();
        zero_port.remote_port = 0;
        let mut invalid_host = valid.clone();
        invalid_host.remote_host = "bad\nhost".into();
        let mut range = valid.clone();
        range.local_port_end = Some(3001);
        range.remote_port_end = Some(3001);
        let mut single_range = valid.clone();
        single_range.local_port_end = Some(3000);
        let mut remote = valid.clone();
        remote.direction = Direction::Remote;
        let mut conflicting = valid.clone();
        conflicting.name = "Conflicting definition".into();
        let foreign = template(Uuid::new_v4(), 3001);
        for (create, connect) in [
            (vec![], vec![]),
            (vec![nil_id.clone()], vec![]),
            (vec![], vec![nil_id]),
            (vec![zero_port], vec![]),
            (vec![invalid_host], vec![]),
            (vec![range], vec![]),
            (vec![single_range], vec![]),
            (vec![remote.clone()], vec![]),
            (vec![], vec![remote]),
            (vec![foreign.clone()], vec![]),
            (vec![], vec![foreign]),
            (vec![valid.clone(), conflicting.clone()], vec![]),
            (vec![], vec![valid.clone(), conflicting]),
            (vec![valid.clone()], vec![valid.clone()]),
        ] {
            assert!(resolve_batch(server, &create, &connect, &[]).is_err());
        }
        assert!(resolve_batch(Uuid::nil(), &[template(Uuid::nil(), 3000)], &[], &[]).is_err());

        let occupied = template(Uuid::new_v4(), 65535);
        let latest = vec![occupied];
        let before = latest.clone();
        // Even a failure after resolving an earlier valid creation yields no batch.
        assert!(resolve_batch(server, &[valid, template(server, 65535)], &[], &latest).is_err());
        assert_eq!(latest, before);
    }

    #[test]
    fn resolution_limits_unique_ids_across_creation_and_connect_lists() {
        let server = Uuid::new_v4();
        let all: Vec<_> = (0..MAX_BATCH)
            .map(|i| template(server, 3000 + i as u16))
            .collect();
        let mut create = all[..128].to_vec();
        create.extend(all[..128].iter().cloned());
        create.extend(all[..128].iter().cloned());
        let connect = &all[128..];
        let resolved = resolve_batch(server, &create, connect, connect).unwrap();
        assert_eq!(resolved.tunnels.len(), MAX_BATCH);
        assert_eq!(resolved.connect.len(), MAX_BATCH);
        assert_eq!(
            resolved.connect.iter().collect::<HashSet<_>>().len(),
            MAX_BATCH
        );
        create.push(template(server, 4000));
        assert!(resolve_batch(server, &create, connect, connect).is_err());
    }
}
