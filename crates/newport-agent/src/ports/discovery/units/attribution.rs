use super::super::process;
use super::*;
use newport_protocol::monitor::ports::discovery::{Association, ServiceRelation, Snapshot};

impl Units {
    pub fn apply(mut self, snapshot: &mut Snapshot, root: &Path, budget: &Budget<'_>) {
        if self.values.is_empty() {
            snapshot.coverage.services = self.report;
            return;
        }
        self.values
            .sort_by(|a, b| (a.manager, &a.unit).cmp(&(b.manager, &b.unit)));
        for (index, unit) in self.values.iter_mut().enumerate() {
            let Ok(id) = u32::try_from(index + 1) else {
                record(
                    &mut self.report,
                    &Failure::Exhausted("Service identity limit reached"),
                );
                break;
            };
            unit.id = id;
        }
        let mut holders = BTreeMap::new();
        for process in &snapshot.processes {
            match process::cgroup(root, process.identity, budget) {
                Ok(Some(path)) => {
                    holders.insert(process.id, self.matching(&path));
                }
                Ok(None) => {}
                Err(error) => record(&mut self.report, &error),
            }
        }
        for socket in &mut snapshot.sockets {
            let mut associations = BTreeSet::new();
            if let Some(path) = socket
                .cgroup
                .as_ref()
                .and_then(|group| group.path.as_deref())
            {
                associations.extend(self.matching(path).into_iter().map(|service| Association {
                    service,
                    relation: ServiceRelation::SocketCgroup,
                }));
            }
            for process in &socket.holders {
                if let Some(services) = holders.get(process) {
                    associations.extend(services.iter().map(|service| Association {
                        service: *service,
                        relation: ServiceRelation::Holder { process: *process },
                    }));
                }
            }
            if associations.len() > MAX_ASSOCIATIONS {
                record(
                    &mut self.report,
                    &Failure::Exhausted("Socket service association limit reached"),
                );
            }
            socket.services = associations.into_iter().take(MAX_ASSOCIATIONS).collect();
        }
        let used = snapshot
            .sockets
            .iter()
            .flat_map(|socket| socket.services.iter().map(|a| a.service))
            .collect::<BTreeSet<_>>();
        snapshot.services = self
            .values
            .into_iter()
            .filter(|service| used.contains(&service.id))
            .collect();
        snapshot.coverage.services = self.report;
    }

    fn matching(&self, path: &str) -> Vec<u32> {
        let matches = self.values.iter().filter(|unit| {
            unit.control_group != "/" && Path::new(path).starts_with(&unit.control_group)
        });
        let depth = matches.clone().map(|unit| unit.control_group.len()).max();
        matches
            .filter(|unit| Some(unit.control_group.len()) == depth)
            .map(|unit| unit.id)
            .collect()
    }
}
