use super::Fixture;
use newport_protocol::monitor::ports::{
    MAX_FIELD, MAX_REPLY_TEXT, Project,
    discovery::{self, Details, IssueKind, ObservedPath, Outcome, Snapshot},
};
use serde::Serialize;
use std::{collections::BTreeMap, fs};

/// Independent input measurements from the fixture's own live processes.
#[derive(Serialize)]
pub struct Metadata {
    pub command_bytes_max: usize,
    pub path_bytes_max: usize,
    pub argument_fields: usize,
    pub text_bytes_lower_bound: usize,
    #[serde(skip)]
    details: BTreeMap<u32, Details>,
}

impl Metadata {
    pub fn read(fixture: &Fixture) -> Self {
        assert_eq!(fixture.load.root_processes, 0);
        let status = fs::read_to_string("/proc/self/status").unwrap();
        let uid = status
            .lines()
            .find_map(|line| line.strip_prefix("Uid:")?.split_whitespace().next())
            .unwrap();
        let passwd = fs::read_to_string("/etc/passwd").unwrap();
        let account = passwd
            .lines()
            .find_map(|line| {
                let fields: Vec<_> = line.split(':').collect();
                (fields.get(2) == Some(&uid)).then_some(fields[0])
            })
            .unwrap();
        let mut result = Self {
            command_bytes_max: 0,
            path_bytes_max: 0,
            argument_fields: 0,
            text_bytes_lower_bound: 0,
            details: BTreeMap::new(),
        };
        for worker in &fixture.workers {
            let root = std::path::PathBuf::from(format!("/proc/{}", worker.pid));
            let command = fs::read(root.join("cmdline")).unwrap();
            result.command_bytes_max = result.command_bytes_max.max(command.len());
            let arguments: Vec<_> = command
                .strip_suffix(&[0])
                .unwrap()
                .split(|b| *b == 0)
                .map(|arg| String::from_utf8(arg.to_vec()).unwrap())
                .collect();
            let executable = fs::read_link(root.join("exe")).unwrap();
            let directory = fs::read_link(root.join("cwd")).unwrap();
            result.path_bytes_max = result
                .path_bytes_max
                .max(executable.as_os_str().len())
                .max(directory.as_os_str().len());
            let name = fs::read_to_string(root.join("comm")).unwrap();
            result.argument_fields += arguments.len();
            result.text_bytes_lower_bound += arguments.iter().map(String::len).sum::<usize>()
                + executable.as_os_str().len()
                + directory.as_os_str().len()
                + name.trim_end_matches('\n').len()
                + executable.file_name().unwrap().len()
                + account.len() * (1 + worker.ports.len());
            let path = |value: &std::path::Path| ObservedPath {
                value: value.to_str().unwrap().into(),
                deleted: false,
                lossy: false,
            };
            let project = fixture.load.project.then(|| Project {
                root: directory.to_str().unwrap().into(),
                marker: ".git".into(),
                worktree: Some(directory.to_str().unwrap().into()),
                branch: Some("main".into()),
                main: Some(true),
            });
            result.details.insert(
                worker.pid,
                Details {
                    executable: Some(path(&executable)),
                    directory: Some(path(&directory)),
                    arguments,
                    project,
                },
            );
        }
        result
    }

    pub fn candidate(&self, observed: &Snapshot) -> Snapshot {
        let mut candidate = observed.clone();
        for process in &mut candidate.processes {
            if let Some(details) = self.details.get(&process.identity.pid) {
                process.details = Some(details.clone());
            }
        }
        candidate
    }

    pub fn check_response(&self, name: &str, observed: &Snapshot) {
        assert!(self.command_bytes_max <= MAX_FIELD);
        assert!(self.path_bytes_max <= MAX_FIELD);
        let candidate = self.candidate(observed);
        let within = name == "project-enrichment-within-budget";
        match name {
            "protobuf-field-budget" => {
                assert!(candidate.validate_records().unwrap() < MAX_REPLY_TEXT);
                assert!(matches!(
                    discovery::wire::measure(&candidate),
                    Err(newport_protocol::monitor::wire::Error::FieldBudget)
                ));
            }
            "project-enrichment-within-budget" => {
                assert!(discovery::wire::fits_response(&candidate).unwrap());
                assert_eq!(observed.coverage.details.outcome, Outcome::Complete);
            }
            "aggregate-text-budget" | "project-enrichment-budget" => {
                assert!(candidate.validate_records().unwrap() > MAX_REPLY_TEXT);
                if name == "aggregate-text-budget" {
                    assert!(self.text_bytes_lower_bound > MAX_REPLY_TEXT);
                }
            }
            _ => panic!("Unknown metadata fixture: {name}"),
        }
        if !within {
            assert_eq!(observed.coverage.details.outcome, Outcome::Partial);
            assert!(
                observed
                    .coverage
                    .details
                    .issues
                    .iter()
                    .any(|issue| issue.kind == IssueKind::BudgetExceeded),
                "{name}: {:?}",
                observed.coverage
            );
        }
        for (pid, expected) in &self.details {
            let process = observed
                .processes
                .iter()
                .find(|p| p.identity.pid == *pid)
                .expect("fixture holder survives optional metadata omission");
            if name.starts_with("project-enrichment-") {
                let mut expected = expected.clone();
                if !within {
                    expected.project = None;
                }
                assert_eq!(process.details.as_ref(), Some(&expected));
            }
        }
    }
}
