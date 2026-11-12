use super::*;
use std::fs;

fn request(generation: u64) -> Request {
    Request {
        contract_id: Some(monitor::CONTRACT_ID.into()),
        generation,
        projects: None,
    }
}

/// The frame the helper answers `generation` with.
fn reply(generation: u64) -> Vec<u8> {
    let request = request(generation);
    wire::encode_reply(
        &Reply {
            contract_id: request.contract_id.clone(),
            generation,
            result: Ok(Box::new(Snapshot {
                generation,
                ..Default::default()
            })),
        },
        &request,
    )
    .unwrap()
}

/// A stand-in helper: `script` runs in `directory` with `/bin/sh`.
fn helper(directory: &Path, script: &str) -> Elevated {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", script, "discovery-helper-test"])
        .current_dir(directory);
    Elevated::launch(command).unwrap()
}

fn frame_length(generation: u64) -> usize {
    wire::encode_request(&request(generation)).unwrap().len()
}

#[test]
fn one_helper_answers_every_scan_until_it_is_dropped() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("first"), reply(1)).unwrap();
    fs::write(directory.path().join("second"), reply(2)).unwrap();
    let script = format!(
        "head -c {} >/dev/null; cat first; head -c {} >/dev/null; cat second; cat >/dev/null",
        frame_length(1),
        frame_length(2)
    );
    let mut elevated = helper(directory.path(), &script);
    let deadline = || Instant::now() + Duration::from_secs(5);
    assert_eq!(
        elevated.scan(1, deadline(), &|| false).unwrap().generation,
        1
    );
    assert!(elevated.answered());
    assert_eq!(
        elevated.scan(2, deadline(), &|| false).unwrap().generation,
        2
    );
}

#[test]
fn a_helper_that_exits_before_answering_was_refused() {
    let directory = tempfile::tempdir().unwrap();
    let mut elevated = helper(
        directory.path(),
        "printf 'sudo: a password is required' >&2; exit 1",
    );
    let error = elevated
        .scan(1, Instant::now() + Duration::from_secs(5), &|| false)
        .unwrap_err();
    match error {
        Error::Refused { diagnostic } => assert_eq!(diagnostic, "sudo: a password is required"),
        other => panic!("expected a refusal, got {other}"),
    }
    assert!(!elevated.answered());
}

#[test]
fn a_helper_that_stops_after_answering_has_exited() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("first"), reply(1)).unwrap();
    let script = format!("head -c {} >/dev/null; cat first; exit 3", frame_length(1));
    let mut elevated = helper(directory.path(), &script);
    let deadline = || Instant::now() + Duration::from_secs(5);
    elevated.scan(1, deadline(), &|| false).unwrap();
    assert!(matches!(
        elevated.scan(2, deadline(), &|| false),
        Err(Error::Exited { .. })
    ));
}

#[test]
fn a_silent_helper_cannot_hold_the_scan_past_its_deadline_or_cancellation() {
    // `; true` keeps the shell, and with it the reply pipe, open: BusyBox's
    // shell would otherwise exec `cat`, whose redirection closes the pipe.
    const SILENT: &str = "cat >/dev/null; true";
    let directory = tempfile::tempdir().unwrap();
    let mut elevated = helper(directory.path(), SILENT);
    let started = Instant::now();
    assert!(matches!(
        elevated.scan(1, started + Duration::from_millis(200), &|| false),
        Err(Error::TimedOut)
    ));
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut elevated = helper(directory.path(), SILENT);
    let started = Instant::now();
    assert!(matches!(
        elevated.scan(1, started + Duration::from_secs(10), &|| started.elapsed()
            > Duration::from_millis(100)),
        Err(Error::Cancelled)
    ));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_relative_agent_path_never_starts_sudo() {
    assert!(matches!(
        Elevated::start(Path::new("relative-agent")),
        Err(Error::Invalid(_))
    ));
}
