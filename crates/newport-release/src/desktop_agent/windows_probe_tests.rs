use super::*;
use std::{
    io::Write,
    os::windows::process::CommandExt,
    process::Stdio,
    sync::{Mutex, PoisonError},
};

const FIXTURE: &str = "signing::desktop_agent::windows_probe::tests::probe_fixture";
const MODE: &str = "NEWPORT_IDENTITY_FIXTURE_MODE";
const ROOT: &str = "NEWPORT_IDENTITY_FIXTURE_ROOT";
const EXECUTABLE: &str = "NEWPORT_IDENTITY_FIXTURE_EXECUTABLE";
const ARGUMENTS: [&str; 5] = [
    FIXTURE,
    "--exact",
    "--ignored",
    "--nocapture",
    "--test-threads=1",
];
const TIMEOUT: Duration = Duration::from_secs(3);
static TESTS: Mutex<()> = Mutex::new(());

#[derive(Debug, Serialize)]
pub(super) struct ProcessReceipt {
    pid: u32,
    image: String,
    exited: bool,
}

impl Processes {
    pub(super) fn receipt(&self, parent: u32, process: &OwnedHandle) -> Vec<ProcessReceipt> {
        std::iter::once((parent, process))
            .chain(self.0.iter().map(|(&pid, process)| (pid, process)))
            .map(|(pid, process)| {
                let mut image = [0u16; 1024];
                let mut length = image.len() as u32;
                // SAFETY: The handle is live, and `image` holds the stated number of u16s.
                let name = if unsafe {
                    QueryFullProcessImageNameW(
                        process.as_raw_handle(),
                        0,
                        image.as_mut_ptr(),
                        &mut length,
                    )
                } != 0
                {
                    std::path::PathBuf::from(String::from_utf16_lossy(&image[..length as usize]))
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                } else {
                    "unavailable".to_owned()
                };
                ProcessReceipt {
                    pid,
                    image: name,
                    // SAFETY: The handle is live.
                    exited: unsafe { WaitForSingleObject(process.as_raw_handle(), 0) }
                        == WAIT_OBJECT_0,
                }
            })
            .collect()
    }
}

struct Fixture {
    root: Profile,
    command: Command,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let root = tempfile::Builder::new()
            .prefix("newport-identity-fixture-")
            .tempdir()
            .unwrap();
        let path = root.path().canonicalize().unwrap();
        fs::write(path.join("fixture-owner"), b"owned identity probe fixture").unwrap();
        let home = path.join("home");
        fs::create_dir(&home).unwrap();
        let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
        let system = std::env::var_os("SystemRoot").unwrap();
        let production = windows_probe_command(&executable, &home, Path::new(&system)).unwrap();
        let mut command = Command::new(&executable);
        command.args(ARGUMENTS).current_dir(home).env_clear();
        for (key, value) in production.get_envs() {
            command.env(key, value.unwrap());
        }
        command
            .env(MODE, mode)
            .env(ROOT, path)
            .env(EXECUTABLE, executable);
        Self {
            root: Profile(Some(root)),
            command,
        }
    }
    fn capture(&self) -> std::result::Result<Output, Failure> {
        let result = capture(&self.command, TIMEOUT);
        if let Err(failure) = &result {
            // Keep both PID markers and job/pipe cleanup proof if this root is
            // quarantined. Never save captured output from the fixture.
            let bytes = serde_json::to_vec_pretty(&failure.cleanup).unwrap();
            fs::write(self.root.path().join("probe-cleanup.json"), bytes).unwrap();
        }
        result
    }
    fn assert_cleanup(&self, receipt: &Cleanup) {
        assert!(receipt.confirmed(), "{receipt:?}");
        assert!(receipt.process_id > 0);
        assert!(receipt.members_checked);
        assert!(receipt.observed_members_exited);
        assert!(receipt.member_observation_error.is_none());
        assert!(
            receipt.processes.iter().all(|process| process.exited),
            "{receipt:?}"
        );
        let pid: u32 = fs::read_to_string(self.root.path().join("parent.pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(receipt.process_id, pid);
        assert_eq!(STATE.load(Ordering::Acquire), IDLE);
    }
}

#[test]
fn empty_job_does_not_confirm_unfinished_observed_members() {
    let mut receipt = Cleanup::no_child();
    assert!(receipt.confirmed());
    receipt.observed_members_exited = false;
    assert!(!receipt.confirmed());
    receipt.observed_members_exited = true;
    receipt.members_checked = false;
    assert!(!receipt.confirmed());
}

#[test]
fn output_is_collected_after_the_exact_job_and_pipes_close() {
    let _serial = TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    let fixture = Fixture::new("normal");
    let output = fixture.capture().unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("identity fixture output")
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("identity fixture error stream")
    );
    assert_eq!(STATE.load(Ordering::Acquire), IDLE);
    let path = fixture.root.path().to_owned();
    drop(fixture);
    assert!(!path.exists());
}

#[test]
fn descendant_holding_output_cannot_outlive_the_probe_deadline() {
    let _serial = TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    let fixture = Fixture::new("parent");
    let started = Instant::now();
    let failure = fixture.capture().unwrap_err();
    assert!(failure.error.to_string().contains("deadline"));
    assert!(started.elapsed() < TIMEOUT + CLEANUP_TIMEOUT + Duration::from_secs(2));
    fixture.assert_cleanup(&failure.cleanup);
    assert!(failure.cleanup.process_exited_before_cleanup);
    assert_eq!(failure.cleanup.total_processes, 2, "{:?}", failure.cleanup);
    let descendant: u32 = fs::read_to_string(fixture.root.path().join("descendant.pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(descendant > 0 && descendant != failure.cleanup.process_id);
    assert!(
        failure
            .cleanup
            .processes
            .iter()
            .any(|process| process.pid == descendant && process.exited),
        "{:?}",
        failure.cleanup
    );
    assert!(fixture.root.path().join("descendant-ready").is_file());
    let path = fixture.root.path().to_owned();
    drop(fixture);
    assert!(!path.exists());
}

#[test]
fn output_cap_terminates_the_exact_job_and_cancels_the_other_pipe() {
    let _serial = TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    for mode in ["stdout-limit", "stderr-limit"] {
        let fixture = Fixture::new(mode);
        let started = Instant::now();
        let failure = fixture.capture().unwrap_err();
        assert!(failure.error.to_string().contains("output limit"));
        assert!(started.elapsed() < TIMEOUT + CLEANUP_TIMEOUT + Duration::from_secs(2));
        fixture.assert_cleanup(&failure.cleanup);
        assert_eq!(failure.cleanup.total_processes, 1);
        let path = fixture.root.path().to_owned();
        drop(fixture);
        assert!(!path.exists());
    }
}

/// This entry only runs under the runner's exact arguments and owned TEMP root.
#[test]
#[ignore = "Child process fixture for the Windows identity-probe tests"]
fn probe_fixture() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(arguments, ARGUMENTS);
    let root = std::path::PathBuf::from(std::env::var_os(ROOT).expect("owned fixture root"));
    let root = root.canonicalize().unwrap();
    let temporary = std::env::temp_dir().canonicalize().unwrap();
    // TEMP is the fixture's private home/Temp, while the root was created below
    // the caller's system temporary directory. Both must share the owned root.
    assert!(temporary.starts_with(&root) && temporary != root);
    assert!(
        root.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("newport-identity-fixture-")
    );
    assert_eq!(
        fs::read(root.join("fixture-owner")).unwrap(),
        b"owned identity probe fixture"
    );
    assert_eq!(
        std::env::current_dir().unwrap().canonicalize().unwrap(),
        root.join("home").canonicalize().unwrap()
    );
    let expected = std::path::PathBuf::from(std::env::var_os(EXECUTABLE).unwrap());
    assert_eq!(
        std::env::current_exe().unwrap().canonicalize().unwrap(),
        expected.canonicalize().unwrap()
    );
    assert_eq!(
        std::path::PathBuf::from(std::env::var_os("NEWPORT_DATA_DIR").unwrap()),
        root.join("home/profile")
    );
    let mode = std::env::var(MODE).unwrap();
    if mode == "descendant" {
        fs::write(root.join("descendant-ready"), b"ready").unwrap();
        std::thread::sleep(Duration::from_secs(60));
        std::process::exit(98);
    }
    fs::write(root.join("parent.pid"), std::process::id().to_string()).unwrap();
    match mode.as_str() {
        "normal" => {
            println!("identity fixture output");
            eprintln!("identity fixture error stream");
        }
        "parent" => {
            let child = Command::new(&expected)
                .args(ARGUMENTS)
                .env(MODE, "descendant")
                // Explicit standard handles work without creating a console.
                .creation_flags(DETACHED_PROCESS)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            fs::write(root.join("descendant.pid"), child.id().to_string()).unwrap();
            println!("identity fixture output");
            io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        "stdout-limit" | "stderr-limit" => {
            let data = vec![b'x'; OUTPUT_LIMIT as usize + 16 * 1024];
            if mode == "stdout-limit" {
                io::stdout().write_all(&data).unwrap();
            } else {
                io::stderr().write_all(&data).unwrap();
            }
            std::thread::sleep(Duration::from_secs(60));
            panic!("probe did not enforce its output cap");
        }
        _ => panic!("unknown identity fixture mode"),
    }
}
