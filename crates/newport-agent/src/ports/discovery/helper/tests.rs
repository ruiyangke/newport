use super::*;
use newport_protocol::monitor::ports::discovery::{Report, helper::Request};
use std::{
    io::{Read, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    sync::mpsc,
    thread,
};

fn pipe() -> (UnixStream, File) {
    let (client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    (client, File::from(OwnedFd::from(server)))
}

fn request(contract_id: Option<String>) -> Request {
    Request {
        projects: None,
        generation: 7,
        contract_id,
    }
}

fn current() -> Request {
    request(Some(monitor::CONTRACT_ID.into()))
}

fn snapshot(generation: u64) -> Snapshot {
    Snapshot {
        generation,
        ..Default::default()
    }
}

#[test]
fn one_scan_returns_one_frame_and_does_not_wait_for_control_eof() {
    let (mut input, reader) = pipe();
    let (mut output, writer) = pipe();
    let worker = thread::spawn(move || {
        exchange(
            &reader,
            &writer,
            Instant::now() + Duration::from_secs(2),
            false,
            |request, _, _| Ok(snapshot(request.generation)),
        )
    });
    input
        .write_all(&wire::encode_request(&current()).unwrap())
        .unwrap();
    let reply = wire::read_reply(&mut output, &current()).unwrap();
    assert_eq!(reply.result.unwrap().generation, current().generation);
    worker.join().unwrap().unwrap();
    assert_eq!(output.read(&mut [0]).unwrap(), 0);
    drop(input);
}

#[test]
fn listener_command_refuses_project_filesystem_requests() {
    let (mut input, reader) = pipe();
    let (mut output, writer) = pipe();
    let request = Request {
        projects: Some(Box::new(snapshot(7))),
        ..current()
    };
    input
        .write_all(&wire::encode_request(&request).unwrap())
        .unwrap();
    exchange(
        &reader,
        &writer,
        Instant::now() + Duration::from_secs(2),
        false,
        |_, _, _| panic!("Listener helper must not inspect project paths"),
    )
    .unwrap();
    assert_eq!(
        wire::read_reply(&mut output, &request)
            .unwrap()
            .result
            .unwrap_err()
            .code,
        "invalid_request"
    );
}

#[test]
fn incompatible_request_never_calls_the_collector() {
    for contract_id in [None, Some("0".repeat(64))] {
        let (mut input, reader) = pipe();
        let (mut output, writer) = pipe();
        let worker = thread::spawn(move || {
            exchange(
                &reader,
                &writer,
                Instant::now() + Duration::from_secs(2),
                false,
                |_, _, _| panic!("An incompatible caller reached the scan"),
            )
        });
        let request = request(contract_id);
        input
            .write_all(&wire::encode_request(&request).unwrap())
            .unwrap();
        let error = wire::read_reply(&mut output, &request)
            .unwrap()
            .result
            .unwrap_err();
        assert_eq!(error.code, "incompatible");
        assert_eq!(error.retry, common::Retry::No);
        worker.join().unwrap().unwrap();
    }
}

#[test]
fn control_disconnect_cancels_an_in_progress_scan() {
    let (mut input, reader) = pipe();
    let (_output, writer) = pipe();
    let (started, ready) = mpsc::channel();
    let worker = thread::spawn(move || {
        exchange(
            &reader,
            &writer,
            Instant::now() + Duration::from_secs(2),
            false,
            |_, _, cancelled| {
                started.send(()).unwrap();
                while !cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(Failure::Cancelled)
            },
        )
    });
    input
        .write_all(&wire::encode_request(&current()).unwrap())
        .unwrap();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let ended = Instant::now();
    drop(input);
    assert_eq!(
        worker.join().unwrap().unwrap_err().kind(),
        io::ErrorKind::ConnectionAborted
    );
    assert!(ended.elapsed() < Duration::from_secs(1));
}

#[test]
fn helper_deadline_includes_partial_request_and_scan_time() {
    let (mut input, reader) = pipe();
    let (_output, writer) = pipe();
    input.write_all(&[100, 1]).unwrap();
    let started = Instant::now();
    let error = exchange(
        &reader,
        &writer,
        started + Duration::from_millis(30),
        false,
        |_, _, _| panic!("Partial request reached collection"),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(1));

    let (mut input, reader) = pipe();
    input
        .write_all(&wire::encode_request(&current()).unwrap())
        .unwrap();
    let error = exchange(
        &reader,
        &writer,
        Instant::now() + Duration::from_millis(30),
        false,
        |_, _, cancelled| {
            while !cancelled() {
                thread::sleep(Duration::from_millis(2));
            }
            Err(Failure::TimedOut)
        },
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}

#[test]
fn extra_input_is_rejected_before_scanning_and_during_collection() {
    for extra_before in [true, false] {
        let (mut input, reader) = pipe();
        let (_output, writer) = pipe();
        input
            .write_all(&wire::encode_request(&current()).unwrap())
            .unwrap();
        if extra_before {
            input.write_all(&[1]).unwrap();
        }
        let error = exchange(
            &reader,
            &writer,
            Instant::now() + Duration::from_secs(2),
            false,
            |_, _, cancelled| {
                assert!(!extra_before);
                input.write_all(&[1]).unwrap();
                assert!(cancelled());
                // Cancellation remains sticky after the unexpected byte was consumed.
                assert!(cancelled());
                Err(Failure::Cancelled)
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}

#[test]
fn backpressured_output_has_a_deadline_and_observes_control_disconnect() {
    for close in [false, true] {
        let (mut input, reader) = pipe();
        let (_output, writer) = pipe();
        let flags = Nonblocking::pipe(writer.as_fd()).unwrap();
        loop {
            match (&writer).write(&[0; 65536]) {
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("{error}"),
            }
        }
        drop(flags);
        input
            .write_all(&wire::encode_request(&current()).unwrap())
            .unwrap();
        let (started, ready) = mpsc::channel();
        let (finished, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            let outcome = exchange(
                &reader,
                &writer,
                Instant::now() + Duration::from_millis(150),
                false,
                |request, _, _| {
                    started.send(()).unwrap();
                    Ok(snapshot(request.generation))
                },
            );
            let _ = finished.send(outcome);
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        if close {
            drop(input);
        }
        let kind = result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap_err()
            .kind();
        worker.join().unwrap();
        assert_eq!(
            kind,
            if close {
                io::ErrorKind::ConnectionAborted
            } else {
                io::ErrorKind::TimedOut
            }
        );
    }
}

#[test]
fn regular_files_are_rejected_and_process_identity_mode_is_enforced() {
    let file = tempfile::tempfile().unwrap();
    assert_eq!(
        Nonblocking::pipe(file.as_fd()).err().unwrap().kind(),
        io::ErrorKind::InvalidInput
    );
    let (mut input, reader) = pipe();
    let (_output, writer) = pipe();
    input
        .write_all(&wire::encode_request(&current()).unwrap())
        .unwrap();
    let error = exchange(
        &reader,
        &writer,
        Instant::now() + Duration::from_secs(2),
        false,
        |request, _, _| {
            let mut value = snapshot(request.generation);
            value.coverage.details = Report::complete();
            Ok(value)
        },
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn scan_failure_is_a_typed_reply() {
    let (mut input, reader) = pipe();
    let (mut output, writer) = pipe();
    let worker = thread::spawn(move || {
        exchange(
            &reader,
            &writer,
            Instant::now() + Duration::from_secs(2),
            false,
            |_, _, _| Err(Failure::Io(io::Error::from_raw_os_error(libc::EACCES))),
        )
    });
    input
        .write_all(&wire::encode_request(&current()).unwrap())
        .unwrap();
    let error = wire::read_reply(&mut output, &current())
        .unwrap()
        .result
        .unwrap_err();
    assert_eq!(error.code, "permission_denied");
    assert_eq!(error.retry, common::Retry::No);
    worker.join().unwrap().unwrap();
}

#[test]
fn oversized_project_reply_returns_a_typed_budget_error() {
    use newport_protocol::monitor::ports::{
        Project,
        discovery::{Details, ObservedPath, Process, ProcessIdentity},
    };
    let mut value = snapshot(7);
    value.coverage.processes = Report::complete();
    value.coverage.details = Report::complete();
    let path = format!("/{}", "p".repeat(3999));
    let observed = ObservedPath {
        value: path.clone(),
        deleted: false,
        lossy: false,
    };
    value.processes = (1..=256)
        .map(|id| Process {
            id,
            identity: ProcessIdentity {
                pid: id,
                start_ticks: 1,
            },
            name: None,
            executable_name: None,
            uid: Some(1000),
            account: None,
            details: Some(Details {
                executable: Some(observed.clone()),
                directory: Some(observed.clone()),
                arguments: vec!["a".repeat(4000)],
                project: None,
            }),
            metadata: Report::complete(),
        })
        .collect();
    let request = Request {
        projects: Some(Box::new(value.clone())),
        ..current()
    };
    let frame = wire::encode_request(&request).unwrap();
    for process in &mut value.processes {
        process.details.as_mut().unwrap().project = Some(Project {
            root: path.clone(),
            marker: ".git".into(),
            worktree: Some(path.clone()),
            branch: Some("main".into()),
            main: Some(true),
        });
    }
    assert!(value.validate_records().unwrap() > monitor::ports::MAX_REPLY_TEXT);
    let (mut input, reader) = pipe();
    let (mut output, writer) = pipe();
    let worker = thread::spawn(move || {
        exchange(
            &reader,
            &writer,
            Instant::now() + Duration::from_secs(5),
            true,
            |_, _, _| Ok(value),
        )
    });
    input.write_all(&frame).unwrap();
    let error = wire::read_reply(&mut output, &request)
        .expect("oversized enrichment must return a framed failure")
        .result
        .unwrap_err();
    assert_eq!(error.code, "budget_exceeded");
    assert_eq!(error.retry, common::Retry::No);
    worker.join().unwrap().unwrap();
}

#[test]
fn a_persistent_helper_serves_each_request_and_stops_when_the_pipe_closes() {
    let (mut input, reader) = pipe();
    let (mut output, writer) = pipe();
    let worker = thread::spawn(move || {
        serve_on(&reader, &writer, |request, _, _| {
            Ok(snapshot(request.generation))
        })
    });
    for generation in [1, 2, 3] {
        let request = Request {
            generation,
            ..current()
        };
        input
            .write_all(&wire::encode_request(&request).unwrap())
            .unwrap();
        let reply = wire::read_reply(&mut output, &request).unwrap();
        assert_eq!(reply.result.unwrap().generation, generation);
    }
    drop(input);
    worker.join().unwrap().unwrap();
    assert_eq!(output.read(&mut [0]).unwrap(), 0);
}

#[test]
fn a_persistent_helper_that_gets_no_request_exits_cleanly() {
    let (input, reader) = pipe();
    let (_output, writer) = pipe();
    drop(input);
    serve_on(&reader, &writer, |_, _, _| panic!("No request was sent")).unwrap();
}
