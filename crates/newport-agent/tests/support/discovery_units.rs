use newport_protocol::monitor::ports::discovery::Manager;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    time::{Duration, Instant},
};

pub struct Fixture {
    manager: Manager,
    name: String,
    units: PathBuf,
    directory: tempfile::TempDir,
    cleaned: bool,
}

impl Fixture {
    pub fn new(manager: Manager, port: u16) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("newport-discovery-systemd-")
            .tempdir_in("/tmp")
            .unwrap();
        let fixture = Self {
            manager,
            name: format!("newport-discovery-{}", uuid::Uuid::new_v4().simple()),
            units: match manager {
                Manager::System => PathBuf::from("/run/systemd/system"),
                Manager::User { uid } => PathBuf::from(format!("/run/user/{uid}/systemd/user")),
            },
            directory,
            cleaned: false,
        };
        let script = fixture.directory.path().join("hold.py");
        fs::write(
            &script,
            "import socket, time\ns = socket.socket(fileno=3)\ns.getsockname()\ntime.sleep(180)\n",
        )
        .unwrap();
        let socket = format!(
            "[Unit]\nDescription=Newport discovery test socket\n[Socket]\nListenStream=127.0.0.1:{port}\n"
        );
        let service = format!(
            "[Unit]\nDescription=Newport discovery test holder\n[Service]\nType=exec\nExecStart=/usr/bin/python3 {}\nRuntimeMaxSec=180\nTimeoutStopSec=2\n",
            script.display()
        );
        for (name, contents) in [(fixture.socket(), socket), (fixture.service(), service)] {
            let path = fixture.directory.path().join(&name);
            fs::write(&path, contents).unwrap();
            fixture.run(
                "install",
                &[
                    "-D",
                    "-m",
                    "0600",
                    path.to_str().unwrap(),
                    fixture.units.join(name).to_str().unwrap(),
                ],
            );
        }
        fixture.control(&["daemon-reload"]);
        fixture
    }

    fn command(&self, program: &str, args: &[&str]) -> Command {
        let mut command = Command::new("timeout");
        command.args(["--signal=TERM", "--kill-after=2", "15"]);
        if self.manager == Manager::System {
            command.args(["sudo", "-n", "--"]);
        }
        if let Manager::User { uid } = self.manager {
            command.env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"));
            command.env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path=/run/user/{uid}/bus"),
            );
        }
        command.arg(program).args(args);
        command
    }

    fn run(&self, program: &str, args: &[&str]) -> String {
        let output = self.command(program, args).output().unwrap();
        assert!(output.status.success(), "{program} {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn control_args<'a>(&self, args: &[&'a str]) -> Vec<&'a str> {
        let mut all = vec![
            "--no-pager",
            "--no-ask-password",
            match self.manager {
                Manager::System => "--system",
                Manager::User { .. } => "--user",
            },
        ];
        all.extend_from_slice(args);
        all
    }

    fn control(&self, args: &[&str]) -> String {
        self.run("systemctl", &self.control_args(args))
    }

    pub fn socket(&self) -> String {
        format!("{}.socket", self.name)
    }
    pub fn service(&self) -> String {
        format!("{}.service", self.name)
    }
    pub fn start(&self) {
        self.control(&["start", &self.socket()]);
    }
    pub fn stop(&self) {
        self.control(&["stop", &self.socket(), &self.service()]);
    }
    pub fn main_pid(&self) -> u32 {
        self.control(&["show", "--property=MainPID", "--value", &self.service()])
            .parse()
            .unwrap()
    }

    pub fn socket_cgroup(&self) -> String {
        self.control(&["show", "--property=ControlGroup", "--value", &self.socket()])
    }

    pub fn wait_active(&self) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let pid = self.main_pid();
            if pid != 0 {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "socket did not activate its service"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn finish(mut self) {
        self.cleaned = self.remove();
        assert!(self.cleaned, "temporary runtime units must be removed");
    }

    fn remove(&self) -> bool {
        let mut success = true;
        let mut check = |result: std::io::Result<Output>| {
            if !result.as_ref().is_ok_and(|output| output.status.success()) {
                success = false;
                eprintln!("systemd fixture cleanup failed: {result:?}");
            }
        };
        check(
            self.command(
                "systemctl",
                &self.control_args(&["stop", &self.socket(), &self.service()]),
            )
            .output(),
        );
        check(
            self.command(
                "rm",
                &[
                    "-f",
                    "--",
                    self.units.join(self.socket()).to_str().unwrap(),
                    self.units.join(self.service()).to_str().unwrap(),
                ],
            )
            .output(),
        );
        check(
            self.command("systemctl", &self.control_args(&["daemon-reload"]))
                .output(),
        );
        success
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.cleaned {
            self.remove();
        }
    }
}
