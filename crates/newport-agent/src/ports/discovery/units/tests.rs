use super::*;
use crate::services::Output;
use newport_protocol::monitor::ports::discovery::{
    Cgroup, Coverage, Endpoint, HolderState, Outcome, Scope, ServiceRelation, Snapshot, Socket,
    SocketSource,
};
use std::{collections::VecDeque, sync::Mutex, time::Instant};

const SSH: &str = "ControlGroup=/system.slice/ssh.socket\nId=ssh.socket\nTriggers=ssh.service\nDescription=SSH socket\nLoadState=loaded\n";

type Call = (Vec<String>, Vec<(String, String)>, Duration);

struct Runner {
    outputs: Mutex<VecDeque<Output>>,
    calls: Mutex<Vec<Call>>,
}
impl Runner {
    fn new(outputs: &[&str]) -> Self {
        Self {
            outputs: Mutex::new(
                outputs
                    .iter()
                    .map(|text| Output {
                        success: true,
                        stdout: (*text).into(),
                        stderr: String::new(),
                    })
                    .collect(),
            ),
            calls: Mutex::default(),
        }
    }
}
impl Run for Runner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        env: &[(String, String)],
        timeout: Duration,
        _: usize,
    ) -> io::Result<Output> {
        assert_eq!(program, "systemctl");
        self.calls
            .lock()
            .unwrap()
            .push((args.to_vec(), env.to_vec(), timeout));
        Ok(self.outputs.lock().unwrap().pop_front().unwrap())
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        generation: 1,
        started_at_ms: 0,
        elapsed_ms: 0,
        scope: Scope::default(),
        sockets: vec![Socket {
            id: 1,
            endpoint: Endpoint {
                address: "0.0.0.0".parse().unwrap(),
                port: 22,
                interface: None,
            },
            source: SocketSource::Diagnostic,
            inode: 12,
            cookie: Some(1),
            uid: 0,
            account: None,
            ipv6_only: None,
            cgroup: Some(Cgroup {
                id: 88,
                path: Some("/system.slice/ssh.socket".into()),
            }),
            holders: Vec::new(),
            holder_state: HolderState::Unresolved,
            holders_truncated: false,
            services: Vec::new(),
        }],
        processes: Vec::new(),
        services: Vec::new(),
        publications: Vec::new(),
        container_engine: None,
        diagnostics: Default::default(),
        coverage: Coverage {
            ipv4: Report::complete(),
            cgroups: Report::complete(),
            ..Coverage::default()
        },
    }
}

#[test]
fn socket_unit_identification_needs_no_pid_or_cgroup_member() {
    let runner = Runner::new(&["ssh.socket loaded active listening SSH\n", SSH]);
    let budget = Budget::new(Instant::now() + Duration::from_secs(2), &|| false);
    let units = Units::collect(
        &runner,
        &Managers {
            system: true,
            user: None,
        },
        &budget,
    );
    assert_eq!(units.paths(), ["/system.slice/ssh.socket"]);
    let mut snapshot = snapshot();
    units.apply(&mut snapshot, Path::new("/unused"), &budget);
    snapshot.validate().unwrap();
    assert_eq!(snapshot.services[0].unit, "ssh.socket");
    assert_eq!(snapshot.services[0].triggers, ["ssh.service"]);
    assert!(snapshot.sockets[0].holders.is_empty());
    assert!(snapshot.processes.is_empty());
    assert_eq!(
        snapshot.sockets[0].services[0].relation,
        ServiceRelation::SocketCgroup
    );
    let calls = runner.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .all(|(args, _, time)| args.contains(&"--no-ask-password".into())
                && *time <= Duration::from_secs(2))
    );
}

#[test]
fn service_main_pid_never_creates_a_socket_holder() {
    let value = unit(Manager::System, records("Id=cups.service\nControlGroup=/system.slice/cups.service\nMainPID=914\nLoadState=loaded\n").unwrap().remove(0)).unwrap().unwrap();
    let mut snapshot = snapshot();
    snapshot.sockets[0].cgroup.as_mut().unwrap().path =
        Some("/system.slice/cups.service/worker".into());
    Units {
        values: vec![value],
        report: Report::complete(),
    }
    .apply(
        &mut snapshot,
        Path::new("/unused"),
        &Budget::new(Instant::now() + QUERY_TIME, &|| false),
    );
    snapshot.validate().unwrap();
    assert_eq!(snapshot.services[0].main_pid, Some(914));
    assert!(snapshot.sockets[0].holders.is_empty());
}

#[test]
fn similar_cgroup_names_are_not_unit_membership() {
    let value = unit(Manager::System, records(SSH).unwrap().remove(0))
        .unwrap()
        .unwrap();
    let mut snapshot = snapshot();
    snapshot.sockets[0].cgroup.as_mut().unwrap().path =
        Some("/system.slice/ssh.socket-other".into());
    Units {
        values: vec![value],
        report: Report::complete(),
    }
    .apply(
        &mut snapshot,
        Path::new("/unused"),
        &Budget::new(Instant::now() + QUERY_TIME, &|| false),
    );
    assert!(snapshot.services.is_empty());
    assert!(snapshot.sockets[0].services.is_empty());
}

#[test]
fn duplicate_properties_and_invalid_paths_cannot_establish_service_identity() {
    assert!(records(&format!("{SSH}Id=other.service\n")).is_err());
    assert!(
        unit(
            Manager::System,
            records("Id=ssh.socket\nControlGroup=/../escape\nLoadState=loaded\n")
                .unwrap()
                .remove(0)
        )
        .is_err()
    );
    assert!(
        names(
            "bad-name loaded active description ssh.service\n",
            MAX_SERVICES
        )
        .is_err()
    );
    assert!(names("ssh.socket loaded active\n", 0).is_err());
}

#[test]
fn user_manager_queries_stay_on_the_current_users_bus() {
    let runner = Runner::new(&["ssh.socket loaded active\n", SSH]);
    let result = Units::collect(
        &runner,
        &Managers {
            system: false,
            user: Some(1001),
        },
        &Budget::new(Instant::now() + QUERY_TIME, &|| false),
    );
    assert_eq!(result.values[0].manager, Manager::User { uid: 1001 });
    assert!(runner.calls.lock().unwrap().iter().all(
        |(args, env, _)| args.contains(&"--user".into())
            && env.contains(&(
                "DBUS_SESSION_BUS_ADDRESS".into(),
                "unix:path=/run/user/1001/bus".into()
            ))
    ));
}

#[test]
fn unavailable_manager_keeps_successful_other_manager_records() {
    let runner = Runner::new(&["ssh.socket loaded active\n", SSH, ""]);
    runner.outputs.lock().unwrap().back_mut().unwrap().success = false;
    let result = Units::collect(
        &runner,
        &Managers {
            system: true,
            user: Some(1001),
        },
        &Budget::new(Instant::now() + QUERY_TIME, &|| false),
    );
    assert_eq!(result.values.len(), 1);
    assert_eq!(result.report.outcome, Outcome::Partial);
}

#[test]
fn missing_managers_and_expired_budget_never_spawn_queries() {
    let runner = Runner::new(&[]);
    let result = Units::collect(
        &runner,
        &Managers {
            system: false,
            user: None,
        },
        &Budget::new(Instant::now() + QUERY_TIME, &|| false),
    );
    assert_eq!(result.report.outcome, Outcome::NotApplicable);
    let result = Units::collect(
        &runner,
        &Managers {
            system: true,
            user: Some(1001),
        },
        &Budget::new(Instant::now(), &|| false),
    );
    assert_eq!(result.report.outcome, Outcome::Partial);
    assert!(runner.calls.lock().unwrap().is_empty());
}
