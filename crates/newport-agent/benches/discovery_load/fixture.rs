use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File},
    net::TcpListener,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub mod metadata;

#[derive(Clone, Copy, Default, Serialize)]
pub struct Load {
    pub processes: usize,
    pub root_processes: usize,
    pub sockets_per_process: usize,
    pub extra_descriptors: usize,
    pub long_arguments: usize,
    pub short_arguments: usize,
    pub command_bytes: usize,
    pub directory_bytes: usize,
    pub project: bool,
}

#[derive(Deserialize, Serialize)]
pub struct Ready {
    pub pid: u32,
    pub ports: Vec<u16>,
}

pub struct Fixture {
    pub load: Load,
    pub workers: Vec<Ready>,
    children: Vec<Child>,
    directory: tempfile::TempDir,
}

impl Fixture {
    pub fn new(load: Load) -> Self {
        assert!(load.processes <= 256 && load.root_processes <= load.processes);
        let mut fixture = Self {
            load,
            workers: Vec::with_capacity(load.processes),
            children: Vec::with_capacity(load.processes),
            directory: tempfile::tempdir().unwrap(),
        };
        let (executable, directory) = fixture.paths();
        for index in 0..load.processes {
            let ready = fixture.directory.path().join(index.to_string());
            let mut command = if index < load.root_processes {
                let mut command = Command::new("/usr/bin/sudo");
                command.args(["-n", "--"]).arg(&executable);
                command
            } else {
                let mut command = Command::new(&executable);
                command.arg0("w");
                command
            };
            command
                .arg("--fixture-worker")
                .arg(load.sockets_per_process.to_string())
                .arg(load.extra_descriptors.to_string())
                .arg(&ready)
                .args(vec!["a".repeat(4000); load.long_arguments])
                .args(vec!["x"; load.short_arguments]);
            if load.command_bytes > 0 {
                assert_eq!(load.root_processes, 0);
                let occupied = 2 + command.get_args().map(|arg| arg.len() + 1).sum::<usize>();
                let padding = load.command_bytes.checked_sub(occupied + 1).unwrap();
                command.arg("a".repeat(padding));
            }
            let child = command
                .current_dir(&directory)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            fixture.children.push(child);
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ready.exists() {
                assert!(
                    fixture
                        .children
                        .last_mut()
                        .unwrap()
                        .try_wait()
                        .unwrap()
                        .is_none()
                );
                assert!(
                    Instant::now() < deadline,
                    "Listener fixture did not become ready"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            fixture
                .workers
                .push(serde_json::from_slice(&fs::read(ready).unwrap()).unwrap());
        }
        fixture
    }

    fn paths(&self) -> (PathBuf, PathBuf) {
        let executable = std::env::current_exe().unwrap();
        let mut directory = self.directory.path().to_owned();
        if self.load.directory_bytes == 0 {
            return (executable, directory);
        }
        assert!(self.load.directory_bytes <= newport_protocol::monitor::ports::MAX_FIELD - 6);
        while directory.as_os_str().len() < self.load.directory_bytes {
            let remaining = self.load.directory_bytes - directory.as_os_str().len();
            assert!(remaining >= 2);
            let mut component = (remaining - 1).min(200);
            if remaining == component + 2 {
                component -= 1;
            }
            directory.push("p".repeat(component));
            fs::create_dir(&directory).unwrap();
        }
        let worker = directory.join("work");
        fs::copy(executable, &worker).unwrap();
        if self.load.project {
            fs::create_dir(directory.join(".git")).unwrap();
            fs::write(directory.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        }
        (worker, directory)
    }

    pub fn root_pids(&self) -> impl Iterator<Item = u32> + '_ {
        self.workers
            .iter()
            .take(self.load.root_processes)
            .map(|w| w.pid)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for child in &mut self.children {
            drop(child.stdin.take());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        for child in &mut self.children {
            while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn worker(args: &[OsString]) {
    let sockets: usize = args[0].to_str().unwrap().parse().unwrap();
    let descriptors: usize = args[1].to_str().unwrap().parse().unwrap();
    assert!(sockets <= 32 && descriptors <= 4096);
    let listeners: Vec<_> = (0..sockets)
        .map(|_| TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    let files: Vec<_> = (0..descriptors)
        .map(|_| File::open("/dev/null").unwrap())
        .collect();
    let ready = Ready {
        pid: std::process::id(),
        ports: listeners
            .iter()
            .map(|l| l.local_addr().unwrap().port())
            .collect(),
    };
    let destination = Path::new(&args[2]);
    let pending = destination.with_extension("pending");
    fs::write(&pending, serde_json::to_vec(&ready).unwrap()).unwrap();
    fs::rename(pending, destination).unwrap();
    let mut descriptor = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    // The parent owns stdin; EOF releases the listeners even after a parent crash.
    // SAFETY: descriptor is valid for one pollfd and stdin remains open.
    unsafe { libc::poll(&mut descriptor, 1, 300_000) };
    drop((files, listeners));
}
