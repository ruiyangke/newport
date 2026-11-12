use super::{
    Failure, Family, Inventory,
    budget::{Budget, record},
    cgroup, elevation, process, projects, sockets,
    units::{Managers, Units},
};
use newport_protocol::monitor::ports::{
    MAX_FIELD, MAX_LISTENERS,
    discovery::{HolderState, Namespace, Report, Scope, Snapshot},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SCAN_TIME: Duration = Duration::from_secs(20);
const ACCOUNT_BYTES: usize = 1 << 20;

pub struct Scan {
    pub snapshot: Snapshot,
    pub visited_processes: usize,
    pub visited_descriptors: usize,
    pub visited_cgroups: usize,
}

#[derive(Clone, Copy)]
pub enum Mode<'a> {
    Full(&'a crate::containers::Containers),
    Identity,
}

#[derive(Default)]
pub struct Discovery {
    cgroups: cgroup::Cache,
    container_engine_seen: bool,
    elevation: elevation::State,
    projects: projects::State,
}

impl Discovery {
    pub fn invalidate(&mut self) {
        self.cgroups.invalidate();
        self.elevation.refresh();
    }

    /// Collects facts in the current namespaces, within the caller's remaining time.
    pub fn collect(
        &mut self,
        generation: u64,
        deadline: Instant,
        mode: Mode<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Scan, Failure> {
        let started = Instant::now();
        let deadline = deadline.min(started + SCAN_TIME);
        let budget = Budget::new(deadline, cancelled);
        let root = Path::new("/");
        let mut snapshot = Snapshot {
            generation,
            started_at_ms: u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| Failure::Invalid("System clock predates the Unix epoch"))?
                    .as_millis(),
            )
            .map_err(|_| Failure::Invalid("System clock exceeds the discovery timestamp range"))?,
            scope: scope(root),
            ..Snapshot::default()
        };
        append(
            &mut snapshot,
            Family::Ipv4,
            sockets(Family::Ipv4, deadline, cancelled),
        )?;
        append(
            &mut snapshot,
            Family::Ipv6,
            sockets(Family::Ipv6, deadline, cancelled),
        )?;
        let inspection = process::collect(
            root,
            &mut snapshot.sockets,
            &budget,
            matches!(mode, Mode::Full(_)),
        );
        snapshot.processes = inspection.processes;
        snapshot.coverage.processes = inspection.coverage;
        snapshot.coverage.details = inspection.details;
        revalidate(
            &mut snapshot,
            &inspection.witnesses,
            deadline,
            cancelled,
            root,
        );
        let mut visited_cgroups = 0;
        if let Mode::Full(containers) = mode {
            self.elevation.enrich(&mut snapshot, deadline, cancelled);
            accounts(root, &mut snapshot);
            let units = if snapshot.sockets.is_empty() {
                Units {
                    values: Vec::new(),
                    report: Report::not_applicable(),
                }
            } else {
                Units::collect(&crate::services::System, &Managers::current(), &budget)
            };
            let wanted = snapshot
                .sockets
                .iter()
                .filter_map(|socket| socket.cgroup.as_ref().map(|group| group.id))
                .collect();
            let groups =
                self.cgroups
                    .resolve(root, &snapshot.scope, &wanted, &units.paths(), &budget);
            for socket in &mut snapshot.sockets {
                if let Some(group) = &mut socket.cgroup {
                    group.path = groups.paths.get(&group.id).cloned();
                }
            }
            snapshot.coverage.cgroups = groups.report;
            visited_cgroups = groups.visited;
            units.apply(&mut snapshot, root, &budget);
            let publications =
                containers.publication_inventory(deadline, cancelled, self.container_engine_seen);
            self.container_engine_seen |= publications.observed_engine;
            snapshot.publications = publications.values;
            snapshot.container_engine = publications.engine;
            snapshot.coverage.containers = publications.report;
            super::fit::fit(&mut snapshot)?;
            self.projects.enrich(&mut snapshot, deadline, cancelled);
        }
        snapshot.elapsed_ms = u64::try_from(started.elapsed().as_millis())
            .map_err(|_| Failure::Exhausted("Discovery elapsed time limit reached"))?;
        super::fit::fit(&mut snapshot)?;
        Ok(Scan {
            snapshot,
            visited_processes: inspection.visited_processes,
            visited_descriptors: inspection.visited_descriptors,
            visited_cgroups,
        })
    }
}

fn append(
    snapshot: &mut Snapshot,
    family: Family,
    mut inventory: Inventory,
) -> Result<(), Failure> {
    let available = MAX_LISTENERS.saturating_sub(snapshot.sockets.len());
    if inventory.sockets.len() > available {
        inventory.sockets.truncate(available);
        record(
            &mut inventory.report,
            &Failure::Exhausted("Socket count limit reached"),
        );
    }
    for mut socket in inventory.sockets {
        socket.id = u32::try_from(snapshot.sockets.len() + 1)
            .map_err(|_| Failure::Exhausted("Socket identity limit reached"))?;
        snapshot.sockets.push(socket);
    }
    match family {
        Family::Ipv4 => {
            snapshot.coverage.ipv4 = inventory.report;
            snapshot.diagnostics.ipv4_source = Some(inventory.source);
        }
        Family::Ipv6 => {
            snapshot.coverage.ipv6 = inventory.report;
            snapshot.diagnostics.ipv6_source = Some(inventory.source);
        }
    }
    Ok(())
}

pub(super) fn scope(root: &Path) -> Scope {
    let namespace = |path: &str| {
        fs::metadata(root.join(path)).ok().map(|m| Namespace {
            device: m.dev(),
            inode: m.ino(),
        })
    };
    let boot_id = process::read(&root.join("proc/sys/kernel/random/boot_id"), MAX_FIELD)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| uuid::Uuid::parse_str(text).is_ok());
    Scope {
        network: namespace("proc/self/ns/net"),
        processes: namespace("proc/1/ns/pid").or_else(|| {
            let status = process::read(&root.join("proc/self/status"), MAX_FIELD).ok()?;
            let status = std::str::from_utf8(&status).ok()?;
            current_proc_namespace(status, std::process::id())
                .then(|| namespace("proc/self/ns/pid"))
                .flatten()
        }),
        boot_id,
    }
}

fn current_proc_namespace(status: &str, pid: u32) -> bool {
    let Some(line) = status.lines().find_map(|line| line.strip_prefix("NSpid:")) else {
        return false;
    };
    let mut ids = line.split_whitespace();
    ids.next().and_then(|id| id.parse::<u32>().ok()) == Some(pid) && ids.next().is_none()
}

fn accounts(root: &Path, snapshot: &mut Snapshot) {
    let Ok(bytes) = process::read(&root.join("etc/passwd"), ACCOUNT_BYTES) else {
        return;
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return;
    };
    let accounts = super::super::users(&text);
    let account = |uid: u32| {
        accounts
            .get(&uid)
            .filter(|name| {
                !name.is_empty() && name.len() <= MAX_FIELD && !name.chars().any(char::is_control)
            })
            .cloned()
    };
    for socket in &mut snapshot.sockets {
        socket.account = account(socket.uid);
    }
    for process in &mut snapshot.processes {
        process.account = process.uid.and_then(account);
    }
}

fn revalidate(
    snapshot: &mut Snapshot,
    witnesses: &[process::Witness],
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
    root: &Path,
) {
    if witnesses.is_empty() {
        return;
    }
    let mut confirmed = BTreeSet::new();
    let mut report = Report::complete();
    for family in [Family::Ipv4, Family::Ipv6] {
        if !snapshot
            .sockets
            .iter()
            .any(|s| family.contains(s.endpoint.address) && !s.holders.is_empty())
        {
            continue;
        }
        let current = sockets(family, deadline, cancelled);
        if current.report.outcome != newport_protocol::monitor::ports::discovery::Outcome::Complete
        {
            record(&mut report, &Failure::Interrupted);
        }
        let identities = current
            .sockets
            .iter()
            .map(|socket| (socket.endpoint, socket.inode, socket.cookie, socket.uid))
            .collect::<BTreeSet<_>>();
        for socket in &snapshot.sockets {
            if identities.contains(&(socket.endpoint, socket.inode, socket.cookie, socket.uid)) {
                confirmed.insert(socket.id);
            }
        }
    }
    let mut holders: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
    let budget = Budget::new(deadline, cancelled);
    for witness in witnesses {
        if let Err(error) = budget.remaining() {
            record(&mut report, &error);
            break;
        }
        match process::verify(root, witness) {
            Ok(true) => {
                holders
                    .entry(witness.inode)
                    .or_default()
                    .insert(witness.process);
            }
            Ok(false) => {}
            // An exited holder cannot hold the socket. The retain below reports
            // it as a changed holder, not as incomplete inspection.
            Err(error) if process::vanished(&error) => {}
            Err(error) => record(&mut report, &error),
        }
    }
    let same_scope = snapshot.scope == scope(root);
    if !same_scope {
        record(&mut report, &Failure::Interrupted);
    }
    let mut referenced = BTreeSet::new();
    for socket in &mut snapshot.sockets {
        if socket.holders.is_empty() {
            continue;
        }
        socket.holders.retain(|id| {
            same_scope
                && confirmed.contains(&socket.id)
                && holders
                    .get(&socket.inode)
                    .is_some_and(|ids| ids.contains(id))
        });
        if socket.holders.is_empty() {
            socket.holder_state = HolderState::Changed;
        }
        referenced.extend(socket.holders.iter().copied());
    }
    snapshot
        .processes
        .retain(|process| referenced.contains(&process.id));
    for issue in report.issues {
        snapshot.coverage.processes.outcome =
            newport_protocol::monitor::ports::discovery::Outcome::Partial;
        if snapshot.coverage.processes.issues.len()
            < newport_protocol::monitor::ports::discovery::MAX_ISSUES
        {
            snapshot.coverage.processes.issues.push(issue);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::monitor::ports::discovery::{Outcome, wire};
    use std::net::TcpListener;

    #[test]
    fn a_holder_that_exits_before_revalidation_leaves_process_coverage_complete() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let mut snapshot = Snapshot {
            scope: scope(Path::new("/")),
            ..Snapshot::default()
        };
        let witness = process::Witness {
            process: 1,
            identity: newport_protocol::monitor::ports::discovery::ProcessIdentity {
                pid,
                start_ticks: 1,
            },
            descriptor: 3,
            inode: 1,
        };
        revalidate(
            &mut snapshot,
            &[witness],
            Instant::now() + Duration::from_secs(5),
            &|| false,
            Path::new("/"),
        );
        assert!(snapshot.coverage.processes.issues.is_empty());
    }

    #[test]
    fn process_namespace_fallback_requires_the_proc_view_to_match_the_current_namespace() {
        assert!(current_proc_namespace("Name:\ttest\nNSpid:\t42\n", 42));
        for status in [
            "NSpid:\t400 42\n",
            "NSpid:\t43\n",
            "NSpid:\t4294967296\n",
            "NSpid:\n",
            "Name:\ttest\n",
        ] {
            assert!(!current_proc_namespace(status, 42), "{status}");
        }
    }

    #[test]
    fn live_snapshot_correlates_its_process_and_roundtrips_generated_protobuf() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        let result = Discovery::default()
            .collect(
                1,
                Instant::now() + Duration::from_secs(5),
                Mode::Full(&crate::containers::Containers::new(Box::new(Vec::new))),
                &|| false,
            )
            .unwrap();
        let socket = result
            .snapshot
            .sockets
            .iter()
            .find(|socket| {
                socket.endpoint.address == endpoint.ip() && socket.endpoint.port == endpoint.port()
            })
            .unwrap();
        assert_eq!(socket.holder_state, HolderState::Observed);
        assert!(socket.holders.iter().any(|id| {
            result
                .snapshot
                .processes
                .iter()
                .any(|process| process.id == *id && process.identity.pid == std::process::id())
        }));
        assert!(result.visited_processes > 0);
        assert!(result.visited_descriptors > 0);
        assert_eq!(result.snapshot.coverage.ipv4.outcome, Outcome::Complete);
        let bytes = wire::encode(&result.snapshot).unwrap();
        assert_eq!(wire::read(&mut &bytes[..]).unwrap(), result.snapshot);
    }

    #[test]
    fn cancelled_inventory_stays_explicitly_unavailable() {
        let result = Discovery::default()
            .collect(
                1,
                Instant::now() + Duration::from_secs(5),
                Mode::Identity,
                &|| true,
            )
            .unwrap();
        assert!(result.snapshot.sockets.is_empty());
        assert_eq!(result.snapshot.coverage.ipv4.outcome, Outcome::Unavailable);
        assert_eq!(result.snapshot.coverage.ipv6.outcome, Outcome::Unavailable);
    }

    #[test]
    fn a_previously_observed_engine_stays_relevant_when_it_disappears() {
        let fixture = crate::docker::testing::Fixture::new(|path| match path {
            "/version" => (200, r#"{"ApiVersion":"1.47"}"#.into()),
            "/v1.47/info" => (200, r#"{"ID":"engine-a"}"#.into()),
            "/v1.47/containers/json" => (200, "[]".into()),
            _ => (404, "{}".into()),
        });
        let path = fixture.socket.clone();
        let containers = crate::containers::Containers::new(Box::new(move || vec![path.clone()]));
        let mut discovery = Discovery::default();
        let first = discovery
            .collect(
                1,
                Instant::now() + Duration::from_secs(5),
                Mode::Full(&containers),
                &|| false,
            )
            .unwrap();
        assert_eq!(
            first.snapshot.coverage.containers.outcome,
            Outcome::Complete
        );
        drop(fixture);
        let next = discovery
            .collect(
                2,
                Instant::now() + Duration::from_secs(5),
                Mode::Full(&containers),
                &|| false,
            )
            .unwrap();
        assert_eq!(
            next.snapshot.coverage.containers.outcome,
            Outcome::Unavailable
        );
        assert!(!next.snapshot.publications_complete_for("engine-a"));
    }
}
