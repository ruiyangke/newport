use super::{Budget, Failure, budget::record};
mod attribution;
use crate::services::Run;
use newport_protocol::monitor::ports::{
    MAX_FIELD,
    discovery::{
        MAX_ASSOCIATIONS, MAX_SERVICES, Manager, Report, Service, valid_cgroup_path, valid_unit,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
    time::Duration,
};

const QUERY_TIME: Duration = Duration::from_secs(3);
const OUTPUT_BYTES: usize = 1 << 20;
const SHOW_BATCH: usize = 200;
const PROPERTIES: &str = "Id,Description,ControlGroup,MainPID,Triggers,LoadState";

pub(super) struct Managers {
    system: bool,
    user: Option<u32>,
}

impl Managers {
    pub fn current() -> Self {
        let uid = crate::paths::effective_uid();
        let runtime = format!("/run/user/{uid}");
        let user = fs::symlink_metadata(&runtime).is_ok_and(|m| m.is_dir() && m.uid() == uid)
            && fs::symlink_metadata(Path::new(&runtime).join("bus"))
                .is_ok_and(|m| m.file_type().is_socket() && m.uid() == uid);
        Self {
            system: Path::new("/run/systemd/system").is_dir(),
            user: user.then_some(uid),
        }
    }

    fn scopes(&self) -> impl Iterator<Item = Manager> + use<> {
        self.system
            .then_some(Manager::System)
            .into_iter()
            .chain(self.user.map(|uid| Manager::User { uid }))
    }
}

pub(super) struct Units {
    pub values: Vec<Service>,
    pub report: Report,
}

impl Units {
    pub fn collect(runner: &dyn Run, managers: &Managers, budget: &Budget<'_>) -> Self {
        let budget = budget.limited(QUERY_TIME);
        let mut remaining = OUTPUT_BYTES;
        let mut result = Self {
            values: Vec::new(),
            report: Report::not_applicable(),
        };
        for scope in managers.scopes() {
            if result.report.outcome
                == newport_protocol::monitor::ports::discovery::Outcome::NotApplicable
            {
                result.report = Report::complete();
            }
            if let Err(error) = result.manager(runner, scope, &budget, &mut remaining) {
                record(&mut result.report, &error);
            }
        }
        result
    }

    fn manager(
        &mut self,
        runner: &dyn Run,
        scope: Manager,
        budget: &Budget<'_>,
        remaining: &mut usize,
    ) -> Result<(), Failure> {
        let text = query(
            runner,
            scope,
            &[
                "list-units".into(),
                "--type=service,socket,scope".into(),
                "--all".into(),
                "--plain".into(),
                "--no-legend".into(),
                "--full".into(),
            ],
            budget,
            remaining,
        )?;
        let names = names(&text, MAX_SERVICES.saturating_sub(self.values.len()))?;
        let names = names.into_iter().collect::<Vec<_>>();
        for batch in names.chunks(SHOW_BATCH) {
            let mut args = vec![
                "show".into(),
                format!("--property={PROPERTIES}"),
                "--".into(),
            ];
            args.extend(batch.iter().cloned());
            let text = query(runner, scope, &args, budget, remaining)?;
            for record in records(&text)? {
                if let Some(unit) = unit(scope, record)? {
                    if self
                        .values
                        .iter()
                        .any(|known| known.manager == scope && known.unit == unit.unit)
                    {
                        return Err(Failure::Invalid("Duplicate service-manager unit identity"));
                    }
                    if self.values.len() >= MAX_SERVICES {
                        return Err(Failure::Exhausted("Service metadata count limit reached"));
                    }
                    self.values.push(unit);
                }
            }
        }
        Ok(())
    }

    pub fn paths(&self) -> Vec<String> {
        self.values
            .iter()
            .map(|unit| unit.control_group.clone())
            .collect()
    }
}

fn query(
    runner: &dyn Run,
    scope: Manager,
    args: &[String],
    budget: &Budget<'_>,
    remaining: &mut usize,
) -> Result<String, Failure> {
    let timeout = budget.remaining()?;
    if *remaining == 0 {
        return Err(Failure::Exhausted("Service metadata byte limit reached"));
    }
    let mut all = vec!["--no-pager".into(), "--no-ask-password".into()];
    let mut env = vec![
        ("LC_ALL".into(), "C".into()),
        ("SYSTEMD_COLORS".into(), "0".into()),
        ("SYSTEMD_URLIFY".into(), "0".into()),
    ];
    match scope {
        Manager::System => {
            all.push("--system".into());
            env.push((
                "DBUS_SYSTEM_BUS_ADDRESS".into(),
                "unix:path=/run/dbus/system_bus_socket".into(),
            ));
        }
        Manager::User { uid } => {
            all.push("--user".into());
            env.push(("XDG_RUNTIME_DIR".into(), format!("/run/user/{uid}")));
            env.push((
                "DBUS_SESSION_BUS_ADDRESS".into(),
                format!("unix:path=/run/user/{uid}/bus"),
            ));
        }
    }
    all.extend_from_slice(args);
    let output = runner
        .run_cancellable("systemctl", &all, &env, timeout, *remaining, &|| {
            budget.is_cancelled()
        })
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Failure::Unsupported("systemctl is not installed")
            } else {
                error.into()
            }
        })?;
    let used = output
        .stdout
        .len()
        .checked_add(output.stderr.len())
        .filter(|used| *used <= *remaining)
        .ok_or(Failure::Exhausted("Service metadata byte limit reached"))?;
    *remaining -= used;
    if !output.success {
        return Err(Failure::Unavailable(
            "Service-manager metadata is unavailable",
        ));
    }
    Ok(output.stdout)
}

fn names(text: &str, limit: usize) -> Result<BTreeSet<String>, Failure> {
    let mut names = BTreeSet::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let mut words = line.split_whitespace();
        let name = match words.next() {
            Some("●") => words.next(),
            name => name,
        }
        .ok_or(Failure::Invalid("Missing service-manager unit identity"))?;
        if !valid_unit(name) {
            return Err(Failure::Invalid("Invalid service-manager unit identity"));
        }
        names.insert(name.to_owned());
        if names.len() > limit {
            return Err(Failure::Exhausted("Service metadata count limit reached"));
        }
    }
    Ok(names)
}

fn records(text: &str) -> Result<Vec<BTreeMap<String, String>>, Failure> {
    let mut values = Vec::new();
    let mut current = BTreeMap::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !current.is_empty() {
                if values.len() >= SHOW_BATCH {
                    return Err(Failure::Exhausted("Service metadata batch limit reached"));
                }
                values.push(std::mem::take(&mut current));
            }
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or(Failure::Invalid("Invalid service-manager property"))?;
        if !PROPERTIES.split(',').any(|known| known == key) {
            continue;
        }
        if value.len() > MAX_FIELD || value.contains('\0') {
            return Err(Failure::Exhausted("Service property size limit reached"));
        }
        if current.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(Failure::Invalid("Duplicate service-manager property"));
        }
    }
    Ok(values)
}

fn unit(
    manager: Manager,
    mut fields: BTreeMap<String, String>,
) -> Result<Option<Service>, Failure> {
    let name = fields
        .remove("Id")
        .filter(|name| valid_unit(name))
        .ok_or(Failure::Invalid("Invalid service-manager unit identity"))?;
    let load = fields
        .remove("LoadState")
        .ok_or(Failure::Invalid("Missing service-manager load state"))?;
    if load == "not-found" {
        return Ok(None);
    }
    let control_group = fields.remove("ControlGroup").unwrap_or_default();
    if control_group.is_empty() {
        return Ok(None);
    }
    if !valid_cgroup_path(&control_group) {
        return Err(Failure::Invalid("Invalid service control-group path"));
    }
    let main_pid = match fields.remove("MainPID") {
        Some(value) => {
            let pid = value
                .parse::<u32>()
                .map_err(|_| Failure::Invalid("Invalid service main process identifier"))?;
            (pid != 0).then_some(pid)
        }
        None => None,
    };
    let triggers = fields
        .remove("Triggers")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if triggers.len() > MAX_ASSOCIATIONS || triggers.iter().any(|unit| !valid_unit(unit)) {
        return Err(Failure::Invalid("Invalid triggered service identities"));
    }
    let description = fields.remove("Description").filter(|text| !text.is_empty());
    Ok(Some(Service {
        id: 0,
        manager,
        unit: name,
        description,
        control_group,
        main_pid,
        triggers: triggers.into_iter().collect(),
    }))
}

#[cfg(test)]
mod tests;
