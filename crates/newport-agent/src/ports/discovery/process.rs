use super::{
    Budget,
    budget::{Failure, record},
};
use newport_protocol::monitor::ports::{
    MAX_FIELD,
    discovery::{
        Details, HolderState, MAX_DETAILS, MAX_HOLDER_REFERENCES, MAX_HOLDERS, MAX_IDENTITIES,
        ObservedPath, Process, ProcessIdentity, Report, Socket,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const PROC_ENTRIES: usize = 65536;
const DESCRIPTORS: usize = 1_000_000;

pub(super) struct Inspection {
    pub processes: Vec<Process>,
    pub coverage: Report,
    pub details: Report,
    pub witnesses: Vec<Witness>,
    pub visited_processes: usize,
    pub visited_descriptors: usize,
}

pub(super) struct Witness {
    pub process: u32,
    pub identity: ProcessIdentity,
    pub descriptor: u32,
    pub inode: u64,
}

struct Collector<'a, 'b> {
    root: &'a Path,
    sockets: &'a mut [Socket],
    wanted: BTreeMap<u64, Vec<usize>>,
    budget: &'a Budget<'b>,
    result: Inspection,
    references: usize,
    include_details: bool,
}

pub(super) fn collect(
    root: &Path,
    sockets: &mut [Socket],
    budget: &Budget<'_>,
    include_details: bool,
) -> Inspection {
    let mut wanted: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (index, socket) in sockets.iter().enumerate() {
        if socket.inode != 0 {
            wanted.entry(socket.inode).or_default().push(index);
        }
    }
    let mut collector = Collector {
        root,
        sockets,
        wanted,
        budget,
        include_details,
        references: 0,
        result: Inspection {
            processes: Vec::new(),
            coverage: Report::complete(),
            details: if include_details {
                Report::complete()
            } else {
                Report::not_applicable()
            },
            witnesses: Vec::new(),
            visited_processes: 0,
            visited_descriptors: 0,
        },
    };
    if let Err(error) = collector.run() {
        record(&mut collector.result.coverage, &error);
    }
    collector.result
}

impl Collector<'_, '_> {
    fn run(&mut self) -> Result<(), Failure> {
        self.budget.remaining()?;
        if self.wanted.is_empty() {
            return Ok(());
        }
        let pids = self.pids()?;
        for pid in pids {
            self.budget.remaining()?;
            self.result.visited_processes += 1;
            if let Err(error) = self.inspect(pid) {
                if !vanished(&error) {
                    record(&mut self.result.coverage, &error);
                }
                if matches!(
                    error,
                    Failure::Exhausted(_) | Failure::TimedOut | Failure::Cancelled
                ) {
                    break;
                }
            }
        }
        Ok(())
    }

    fn pids(&mut self) -> Result<BTreeSet<u32>, Failure> {
        let mut pids = BTreeSet::new();
        let entries = fs::read_dir(self.root.join("proc"))?;
        for (count, entry) in entries.enumerate() {
            self.budget.remaining()?;
            if count >= PROC_ENTRIES {
                record(
                    &mut self.result.coverage,
                    &Failure::Exhausted("Process enumeration limit reached"),
                );
                break;
            }
            match entry {
                Ok(entry) => {
                    if let Some(pid) = number(&entry.file_name()).filter(|pid| *pid > 0) {
                        pids.insert(pid);
                    }
                }
                Err(error) => record(&mut self.result.coverage, &error.into()),
            }
        }
        Ok(pids)
    }

    fn inspect(&mut self, pid: u32) -> Result<(), Failure> {
        let directory = self.root.join(format!("proc/{pid}"));
        let identity = ProcessIdentity {
            pid,
            start_ticks: start(&directory, pid)?,
        };
        let descriptors = self.descriptors(&directory)?;
        if descriptors.is_empty() {
            return Ok(());
        }
        if start(&directory, pid)? != identity.start_ticks {
            self.changed(descriptors.keys().copied());
            return Ok(());
        }
        let mut verified = Vec::new();
        for (inode, descriptor) in descriptors {
            self.budget.remaining()?;
            if socket_link(&directory.join(format!("fd/{descriptor}")))? == Some(inode) {
                verified.push((inode, descriptor));
            } else {
                self.changed([inode]);
            }
        }
        if verified.is_empty() {
            return Ok(());
        }
        let detailed = self.include_details && self.result.processes.len() < MAX_DETAILS;
        let mut process = metadata(&directory, identity, detailed, self.budget);
        if self.include_details && !detailed {
            record(
                &mut self.result.details,
                &Failure::Exhausted("Detailed process limit reached"),
            );
        }
        if detailed
            && process.metadata.outcome
                != newport_protocol::monitor::ports::discovery::Outcome::Complete
        {
            for issue in &process.metadata.issues {
                self.result.details.outcome =
                    newport_protocol::monitor::ports::discovery::Outcome::Partial;
                if let Some(previous) = self
                    .result
                    .details
                    .issues
                    .iter_mut()
                    .find(|previous| previous.kind == issue.kind)
                {
                    previous.count = previous.count.saturating_add(issue.count);
                } else if self.result.details.issues.len()
                    < newport_protocol::monitor::ports::discovery::MAX_ISSUES
                {
                    self.result.details.issues.push(issue.clone());
                }
            }
        }
        if start(&directory, pid)? != identity.start_ticks {
            self.changed(verified.iter().map(|(inode, _)| *inode));
            return Ok(());
        }
        process.id = u32::try_from(self.result.processes.len() + 1)
            .map_err(|_| Failure::Exhausted("Process identity limit reached"))?;
        let mut used = false;
        for (inode, descriptor) in verified {
            if let Err(error) = self.budget.remaining() {
                record(&mut self.result.coverage, &error);
                break;
            }
            let witness = Witness {
                process: process.id,
                identity,
                descriptor,
                inode,
            };
            match verify(self.root, &witness) {
                Ok(true) => {
                    if self.attach(&witness) {
                        self.result.witnesses.push(witness);
                        used = true;
                    }
                }
                Ok(false) => self.changed([inode]),
                Err(error) => {
                    self.changed([inode]);
                    if !vanished(&error) {
                        record(&mut self.result.coverage, &error);
                    }
                }
            }
        }
        if used {
            self.result.processes.push(process);
        }
        Ok(())
    }

    fn descriptors(&mut self, directory: &Path) -> Result<BTreeMap<u64, u32>, Failure> {
        let mut found = BTreeMap::new();
        for entry in fs::read_dir(directory.join("fd"))? {
            self.budget.remaining()?;
            if self.result.visited_descriptors >= DESCRIPTORS {
                return Err(Failure::Exhausted("Process descriptor limit reached"));
            }
            self.result.visited_descriptors += 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    record(&mut self.result.coverage, &error.into());
                    continue;
                }
            };
            let Some(descriptor) = number(&entry.file_name()) else {
                continue;
            };
            match socket_link(&entry.path()) {
                Ok(Some(inode)) if self.wanted.contains_key(&inode) => {
                    found
                        .entry(inode)
                        .and_modify(|previous: &mut u32| *previous = (*previous).min(descriptor))
                        .or_insert(descriptor);
                }
                Ok(_) => {}
                Err(error) if vanished(&error) => {}
                Err(error) => record(&mut self.result.coverage, &error),
            }
        }
        Ok(found)
    }

    fn attach(&mut self, witness: &Witness) -> bool {
        let Some(indices) = self.wanted.get(&witness.inode) else {
            return false;
        };
        if indices.len() != 1 {
            self.changed([witness.inode]);
            return false;
        }
        let socket = &mut self.sockets[indices[0]];
        if socket.holders.len() >= MAX_HOLDERS
            || self.references >= MAX_HOLDER_REFERENCES
            || self.result.processes.len() >= MAX_IDENTITIES
        {
            socket.holders_truncated = true;
            record(
                &mut self.result.coverage,
                &Failure::Exhausted("Socket holder limit reached"),
            );
            return false;
        }
        socket.holders.push(witness.process);
        socket.holder_state = HolderState::Observed;
        self.references += 1;
        true
    }

    fn changed(&mut self, inodes: impl IntoIterator<Item = u64>) {
        for inode in inodes {
            if let Some(indices) = self.wanted.get(&inode) {
                for index in indices {
                    if self.sockets[*index].holders.is_empty() {
                        self.sockets[*index].holder_state = HolderState::Changed;
                    }
                }
            }
        }
    }
}

fn number(name: &std::ffi::OsStr) -> Option<u32> {
    name.to_str()?.parse().ok()
}

pub(super) fn vanished(error: &Failure) -> bool {
    matches!(error, Failure::Io(error) if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)))
}

fn socket_link(path: &Path) -> Result<Option<u64>, Failure> {
    let value = fs::read_link(path)?;
    Ok(value
        .to_str()
        .and_then(|text| text.strip_prefix("socket:["))
        .and_then(|text| text.strip_suffix(']'))
        .and_then(|text| text.parse().ok()))
}

pub(super) fn read(path: &Path, limit: usize) -> Result<Vec<u8>, Failure> {
    let limit64 = u64::try_from(limit)
        .map_err(|_| Failure::Exhausted("Process metadata size limit reached"))?;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Failure::Exhausted("Process metadata size limit reached"));
    }
    Ok(bytes)
}

fn start(directory: &Path, expected_pid: u32) -> Result<u64, Failure> {
    let bytes = read(&directory.join("stat"), MAX_FIELD)?;
    let start = || {
        let space = bytes.iter().position(|byte| *byte == b' ')?;
        let pid = std::str::from_utf8(&bytes[..space])
            .ok()?
            .parse::<u32>()
            .ok()?;
        if pid != expected_pid {
            return None;
        }
        let close = bytes.iter().rposition(|byte| *byte == b')')?;
        std::str::from_utf8(&bytes[close + 1..])
            .ok()?
            .split_whitespace()
            .nth(19)?
            .parse()
            .ok()
    };
    start().ok_or(Failure::Invalid("Invalid process identity"))
}

pub(super) fn verify(root: &Path, witness: &Witness) -> Result<bool, Failure> {
    let directory = root.join(format!("proc/{}", witness.identity.pid));
    if start(&directory, witness.identity.pid)? != witness.identity.start_ticks {
        return Ok(false);
    }
    if socket_link(&directory.join(format!("fd/{}", witness.descriptor)))? != Some(witness.inode) {
        return Ok(false);
    }
    Ok(start(&directory, witness.identity.pid)? == witness.identity.start_ticks)
}

pub(super) fn verify_directory(
    root: &Path,
    identity: ProcessIdentity,
    observed: &ObservedPath,
    budget: &Budget<'_>,
) -> Result<bool, Failure> {
    use std::os::unix::fs::MetadataExt;
    budget.remaining()?;
    if observed.deleted || observed.lossy {
        return Ok(false);
    }
    let directory = root.join(format!("proc/{}", identity.pid));
    if start(&directory, identity.pid)? != identity.start_ticks {
        return Ok(false);
    }
    let same = |first: &Path, second: &Path| -> Result<bool, Failure> {
        let first = fs::metadata(first)?;
        let second = fs::metadata(second)?;
        Ok(first.dev() == second.dev() && first.ino() == second.ino())
    };
    // A pathname inside another mount view or chroot can name a different host project.
    if !same(&directory.join("ns/mnt"), &root.join("proc/self/ns/mnt"))?
        || !same(&directory.join("root"), root)?
    {
        return Ok(false);
    }
    let (current, _) = observed_path(&directory.join("cwd"))?;
    budget.remaining()?;
    Ok(current == *observed && start(&directory, identity.pid)? == identity.start_ticks)
}

pub(super) fn cgroup(
    root: &Path,
    identity: ProcessIdentity,
    budget: &Budget<'_>,
) -> Result<Option<String>, Failure> {
    budget.remaining()?;
    let directory = root.join(format!("proc/{}", identity.pid));
    if start(&directory, identity.pid)? != identity.start_ticks {
        return Err(Failure::Interrupted);
    }
    let bytes = read(&directory.join("cgroup"), MAX_FIELD)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Failure::Invalid("Invalid process cgroup metadata"))?;
    let mut paths = text.lines().filter_map(|line| line.strip_prefix("0::"));
    let path = paths.next().map(str::to_owned);
    if paths.next().is_some()
        || path.as_ref().is_some_and(|path| {
            !newport_protocol::monitor::ports::discovery::valid_cgroup_path(path)
        })
    {
        return Err(Failure::Invalid("Invalid process cgroup path"));
    }
    if start(&directory, identity.pid)? != identity.start_ticks {
        return Err(Failure::Interrupted);
    }
    Ok(path)
}

fn optional<T>(result: Result<T, Failure>, report: &mut Report) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            record(report, &error);
            None
        }
    }
}

fn display(bytes: &[u8]) -> String {
    let mut value = String::from_utf8_lossy(bytes).replace('\0', "�");
    super::super::truncate(&mut value, MAX_FIELD);
    value
}

fn observed_path(path: &Path) -> Result<(ObservedPath, PathBuf), Failure> {
    let raw = fs::read_link(path)?;
    let value = raw.to_string_lossy();
    let lossy = matches!(value, std::borrow::Cow::Owned(_));
    let (value, deleted) = match value.strip_suffix(" (deleted)") {
        Some(value) => (value, true),
        None => (value.as_ref(), false),
    };
    if !value.starts_with('/') {
        return Err(Failure::Invalid("Process path is not absolute"));
    }
    if value.len() > MAX_FIELD {
        return Err(Failure::Exhausted("Process path size limit reached"));
    }
    Ok((
        ObservedPath {
            value: value.to_owned(),
            deleted,
            lossy,
        },
        raw,
    ))
}

fn metadata(
    directory: &Path,
    identity: ProcessIdentity,
    detailed: bool,
    budget: &Budget<'_>,
) -> Process {
    let mut report = Report::complete();
    let name = optional(read(&directory.join("comm"), MAX_FIELD), &mut report)
        .map(|mut bytes| {
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            display(&bytes)
        })
        .filter(|name| !name.is_empty());
    let executable = optional(observed_path(&directory.join("exe")), &mut report);
    let executable_name = executable
        .as_ref()
        .and_then(|(path, _)| Path::new(&path.value).file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let uid = optional(read(&directory.join("status"), MAX_FIELD), &mut report).and_then(|bytes| {
        let uid = super::super::status_uid(&String::from_utf8_lossy(&bytes));
        if uid.is_none() {
            record(
                &mut report,
                &Failure::Invalid("Invalid process credentials"),
            );
        }
        uid
    });
    let mut details = None;
    if detailed {
        match budget.remaining() {
            Ok(_) => {
                let working_directory =
                    optional(observed_path(&directory.join("cwd")), &mut report)
                        .map(|(path, _)| path);
                let arguments = optional(read(&directory.join("cmdline"), MAX_FIELD), &mut report)
                    .map(|bytes| {
                        if bytes.is_empty() {
                            Vec::new()
                        } else {
                            bytes
                                .strip_suffix(&[0])
                                .unwrap_or(&bytes)
                                .split(|byte| *byte == 0)
                                .map(display)
                                .collect()
                        }
                    })
                    .unwrap_or_default();
                details = Some(Details {
                    executable: executable.map(|(path, _)| path),
                    directory: working_directory,
                    arguments,
                    project: None,
                });
            }
            Err(error) => record(&mut report, &error),
        }
    }
    Process {
        id: 0,
        identity,
        name,
        executable_name,
        uid,
        account: None,
        details,
        metadata: report,
    }
}

#[cfg(test)]
mod tests;
