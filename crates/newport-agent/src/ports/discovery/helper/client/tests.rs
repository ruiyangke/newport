use super::*;
use newport_protocol::monitor::ports::discovery::helper::Reply;
use std::{fs, os::fd::AsRawFd};

fn shell(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script, "discovery-client-test"]);
    command
}
fn request() -> Request {
    Request {
        contract_id: Some(monitor::CONTRACT_ID.into()),
        generation: 1,
        projects: None,
    }
}
fn reply() -> Vec<u8> {
    let request = Request {
        projects: None,
        contract_id: Some(monitor::CONTRACT_ID.into()),
        generation: 1,
    };
    wire::encode_reply(
        &Reply {
            contract_id: request.contract_id.clone(),
            generation: 1,
            result: Ok(Box::new(Snapshot {
                generation: 1,
                ..Default::default()
            })),
        },
        &request,
    )
    .unwrap()
}

#[test]
fn a_finite_reply_requires_successful_exit_and_clean_eof() {
    let temporary = tempfile::tempdir().unwrap();
    let response = temporary.path().join("response");
    fs::write(&response, reply()).unwrap();
    let mut command = shell("cat \"$1\"");
    command.arg(&response);
    let mut client = Client::default();
    let value = client
        .exchange(
            &mut command,
            request(),
            Instant::now() + Duration::from_secs(2),
            &|| false,
        )
        .unwrap();
    assert_eq!(value.generation, 1);
    assert!(client.running.is_none());
    fs::write(&response, [reply(), reply()].concat()).unwrap();
    let error = client
        .exchange(
            &mut command,
            request(),
            Instant::now() + Duration::from_secs(2),
            &|| false,
        )
        .unwrap_err();
    assert!(matches!(error, Error::Invalid(_)), "{error}");
    assert!(client.running.is_none());
    fs::write(&response, &reply()[..reply().len() - 1]).unwrap();
    assert!(matches!(
        client.exchange(
            &mut command,
            request(),
            Instant::now() + Duration::from_secs(2),
            &|| false
        ),
        Err(Error::Wire(_))
    ));
    assert!(client.running.is_none());
}

#[test]
fn nonzero_exit_preserves_bounded_diagnostics_without_parsing_english() {
    let mut client = Client::default();
    let error = client
        .exchange(
            &mut shell("printf 'arbitrary diagnostic' >&2; exit 17"),
            request(),
            Instant::now() + Duration::from_secs(2),
            &|| false,
        )
        .unwrap_err();
    match error {
        Error::Exited { status, diagnostic } => {
            assert_eq!(status.code(), Some(17));
            assert_eq!(diagnostic, "arbitrary diagnostic");
        }
        other => panic!("{other}"),
    }
    assert!(client.running.is_none());
    let error = client
        .exchange(
            &mut shell("head -c 8192 /dev/zero >&2"),
            request(),
            Instant::now() + Duration::from_secs(2),
            &|| false,
        )
        .unwrap_err();
    assert!(matches!(error, Error::Invalid(_)), "{error}");
}

#[test]
fn cancellation_and_stalled_output_reap_the_child_and_allow_another_scan() {
    let temporary = tempfile::tempdir().unwrap();
    let pid = temporary.path().join("pid");
    let ready = temporary.path().join("ready");
    let mut command = shell("printf '%s' $$ > \"$1\"; : > \"$2\"; exec sleep 30");
    command.arg(&pid).arg(&ready);
    let mut client = Client::default();
    let error = client
        .exchange(
            &mut command,
            request(),
            Instant::now() + Duration::from_secs(2),
            &|| ready.exists(),
        )
        .unwrap_err();
    assert!(matches!(error, Error::Cancelled), "{error}");
    assert!(client.running.is_none());
    let child: libc::pid_t = fs::read_to_string(&pid).unwrap().parse().unwrap();
    // SAFETY: signal 0 only checks that the process exists.
    assert_eq!(unsafe { libc::kill(child, 0) }, -1);
    let error = client
        .exchange(
            &mut shell("printf '\\377'; sleep 30"),
            request(),
            Instant::now() + Duration::from_millis(100),
            &|| false,
        )
        .unwrap_err();
    assert!(matches!(error, Error::TimedOut), "{error}");
    assert!(client.running.is_none());
}

#[test]
fn unrelated_inheritable_descriptors_do_not_reach_the_helper() {
    let sentinel = tempfile::NamedTempFile::new().unwrap();
    let fd = sentinel.as_file().as_raw_fd();
    // SAFETY: fcntl takes scalar arguments on a descriptor the temporary file holds open.
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, 0);
    let mut command = shell("test ! -e \"/proc/self/fd/$1\" || exit 88; exit 17");
    command.arg(fd.to_string());
    let error = Client::default()
        .exchange(
            &mut command,
            request(),
            Instant::now() + Duration::from_secs(2),
            &|| false,
        )
        .unwrap_err();
    assert_eq!(
        // SAFETY: as above.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
        0
    );
    match error {
        Error::Exited { status, .. } => assert_eq!(status.code(), Some(17)),
        other => panic!("{other}"),
    }
}

#[test]
fn sudo_is_found_where_nixos_installs_it() {
    assert!(SUDO.iter().all(|path| Path::new(path).is_absolute()));
    assert!(SUDO.contains(&"/run/wrappers/bin/sudo"));
    let directory = tempfile::tempdir().unwrap();
    let usual = directory.path().join("usr/bin/sudo");
    let wrapper = directory.path().join("run/wrappers/bin/sudo");
    let candidates = [usual.as_path(), wrapper.as_path()];
    assert_eq!(installed(&candidates), None);
    std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
    std::fs::write(&wrapper, "").unwrap();
    assert_eq!(installed(&candidates), Some(wrapper.as_path()));
    std::fs::create_dir_all(usual.parent().unwrap()).unwrap();
    std::fs::write(&usual, "").unwrap();
    assert_eq!(installed(&candidates), Some(usual.as_path()));
}

#[test]
fn cancellation_and_relative_agent_paths_never_spawn_a_command() {
    let mut client = Client::default();
    assert!(matches!(
        client.exchange(
            &mut Command::new("/missing/program"),
            request(),
            Instant::now() + Duration::from_secs(1),
            &|| true
        ),
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        client.projects(
            Path::new("relative-agent"),
            &Snapshot::default(),
            Instant::now() + Duration::from_secs(1),
            &|| false
        ),
        Err(Error::Invalid(_))
    ));
    assert!(client.running.is_none());
}

#[test]
fn stopped_project_lookup_cannot_hold_the_callers_deadline() {
    let snapshot = Snapshot {
        generation: 1,
        ..Default::default()
    };
    let input = Request {
        projects: Some(Box::new(snapshot.clone())),
        ..request()
    };
    let started = Instant::now();
    let mut client = Client::default();
    let error = client
        .exchange(
            &mut shell("kill -STOP $$"),
            input,
            started + Duration::from_millis(150),
            &|| false,
        )
        .unwrap_err();
    assert!(matches!(error, Error::TimedOut), "{error}");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(client.running.is_none());
    assert_eq!(snapshot.generation, 1);
}
