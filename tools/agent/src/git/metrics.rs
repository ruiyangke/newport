//! Opt-in benchmark timings. Never put diagnostics into the Git wire protocol.
use std::{
    cell::Cell,
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::Instant,
};

#[derive(Clone, Copy, Default)]
struct Processes {
    helpers: u64,
    git: u64,
}
thread_local! {
    // Git RPC execution and its credential callbacks run on the request thread.
    static PROCESSES: Cell<Option<Processes>> = const { Cell::new(None) };
}
/// Count successful helper shell launches, including known `git credential-*`
/// dispatches. Commands launched internally by arbitrary helpers are not visible.
pub(super) fn helper_started(uses_git: bool) {
    PROCESSES.with(|cell| {
        if let Some(mut processes) = cell.get() {
            processes.helpers += 1;
            processes.git += u64::from(uses_git);
            cell.set(Some(processes));
        }
    });
}

/// Count a successful direct Git CLI launch separately from credential helpers.
pub(super) fn git_started() {
    PROCESSES.with(|cell| {
        if let Some(mut processes) = cell.get() {
            processes.git += 1;
            cell.set(Some(processes));
        }
    });
}

pub(super) struct Metrics(Option<File>);
impl Metrics {
    pub(super) fn from_env() -> Self {
        Self(
            std::env::var_os("NEWPORT_GIT_TIMINGS")
                .and_then(|path| Self::open(Path::new(&path)).ok()),
        )
    }
    fn open(path: &Path) -> std::io::Result<File> {
        // A benchmark supplies a fresh path. Refuse overwrites and symlinks,
        // and do not create parent directories in an agent's environment.
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }
    pub(super) fn start(&self) -> Option<Instant> {
        PROCESSES.with(|cell| cell.set(self.0.as_ref().map(|_| Processes::default())));
        self.0.as_ref().map(|_| Instant::now())
    }
    pub(super) fn finish(&mut self, id: &str, start: Option<Instant>) {
        let processes = PROCESSES.with(Cell::take).unwrap_or_default();
        let Some(start) = start else { return };
        let elapsed = start.elapsed().as_nanos();
        // Only the validated request UUID, duration and counters are recorded.
        // No methods, parameters, results, repository paths or credentials.
        if let Some(file) = self.0.as_mut() {
            if writeln!(file, "{{\"requestId\":\"{id}\",\"executionNs\":{elapsed},\"credentialHelperProcesses\":{},\"gitCommands\":{}}}", processes.helpers, processes.git).is_err() {
                self.0 = None; // Diagnostic failures must not fail Git writes.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    #[test]
    fn helper_processes_are_measured_per_request_without_sensitive_fields() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path().join("repo")).unwrap();
        let config_path = dir.path().join("config");
        std::fs::write(&config_path, "").unwrap();
        let mut config = git2::Config::open(&config_path).unwrap();
        config
            .set_str(
                "credential.helper",
                "!f() { printf 'username=fixture\\npassword=fixture\\n'; }; f",
            )
            .unwrap();
        repo.set_config(&config).unwrap();
        let path = dir.path().join("measurements");
        let mut metrics = Metrics(Some(Metrics::open(&path).unwrap()));
        for (id, named) in [
            ("helper", false),
            ("named-helper", true),
            ("next-read", false),
        ] {
            let started = metrics.start();
            if id != "next-read" {
                if named {
                    config
                        .set_str("credential.helper", "newport-nonexistent-test-helper")
                        .unwrap();
                    repo.set_config(&config).unwrap();
                }
                let result = super::super::credentials::https(
                    &repo,
                    "https://example.test/repo",
                    None,
                    Instant::now() + std::time::Duration::from_secs(2),
                );
                assert_eq!(result.is_ok(), !named);
            }
            metrics.finish(id, started);
        }
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(rows[0]["credentialHelperProcesses"], 1);
        assert_eq!(rows[0]["gitCommands"], 0);
        assert_eq!(rows[1]["credentialHelperProcesses"], 1);
        assert_eq!(rows[1]["gitCommands"], 1);
        assert_eq!(rows[2]["credentialHelperProcesses"], 0);
        assert_eq!(rows[2]["gitCommands"], 0);
        assert!(rows.iter().all(|row| row.as_object().unwrap().len() == 4));
        assert!(PROCESSES.with(Cell::get).is_none());
    }
    #[test]
    fn timings_are_private_correlated_and_do_not_replace_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timings");
        let mut metrics = Metrics(Some(Metrics::open(&path).unwrap()));
        let id = uuid::Uuid::new_v4().to_string();
        let started = metrics.start();
        metrics.finish(&id, started);
        let row: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(row["requestId"], id);
        assert!(row["executionNs"].as_u64().is_some());
        assert_eq!(row["credentialHelperProcesses"], 0);
        assert_eq!(row["gitCommands"], 0);
        assert_eq!(row.as_object().unwrap().len(), 4);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(Metrics::open(&path).is_err());
        let link = dir.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(Metrics::open(&link).is_err());
        assert!(Metrics(None).start().is_none());
    }
}
