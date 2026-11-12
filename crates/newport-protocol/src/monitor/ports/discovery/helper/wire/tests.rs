use super::*;
use crate::monitor::ports::discovery::{self, Details, Report};

fn request() -> Request {
    Request {
        projects: None,
        contract_id: Some(monitor::CONTRACT_ID.into()),
        generation: 1,
    }
}

fn snapshot() -> Snapshot {
    let mut value = discovery::tests::snapshot();
    value.services.clear();
    value.coverage.services = Report::default();
    for socket in &mut value.sockets {
        socket.account = None;
        socket.services.clear();
        if let Some(group) = &mut socket.cgroup {
            group.path = None;
        }
    }
    for process in &mut value.processes {
        process.account = None;
    }
    value
}

fn reply() -> Reply {
    Reply {
        contract_id: request().contract_id,
        generation: 1,
        result: Ok(Box::new(snapshot())),
    }
}

fn body(value: &Reply) -> Vec<u8> {
    let frame = encode_reply(value, &request()).unwrap();
    protobuf::read_frame(&mut frame.as_slice(), monitor::SERVER_LIMITS.message)
        .unwrap()
        .unwrap()
        .to_vec()
}

#[test]
fn finite_scan_keeps_multiple_holders_and_correlates_its_generation() {
    let frame = encode_request(&request()).unwrap();
    assert_eq!(read_request(&mut frame.as_slice()).unwrap(), request());
    let expected = reply();
    let frame = encode_reply(&expected, &request()).unwrap();
    assert_eq!(
        read_reply(&mut frame.as_slice(), &request()).unwrap(),
        expected
    );
    let mut other = request();
    other.generation += 1;
    assert!(read_reply(&mut frame.as_slice(), &other).is_err());
    let mut changed = reply();
    changed.result.as_mut().unwrap().generation += 1;
    assert!(encode_reply(&changed, &request()).is_err());
}

#[test]
fn project_enrichment_roundtrips_without_changing_observed_facts() {
    let mut input = discovery::tests::snapshot();
    input.processes[0].details = Some(Details {
        directory: Some(discovery::ObservedPath {
            value: "/srv/app".into(),
            deleted: false,
            lossy: false,
        }),
        ..Default::default()
    });
    let request = Request {
        projects: Some(Box::new(input.clone())),
        ..request()
    };
    let frame = encode_request(&request).unwrap();
    assert_eq!(read_request(&mut frame.as_slice()).unwrap(), request);
    let mut output = input;
    output.processes[0].details.as_mut().unwrap().project = Some(crate::monitor::ports::Project {
        root: "/srv/app".into(),
        marker: "Cargo.toml".into(),
        ..Default::default()
    });
    let response = Reply {
        result: Ok(Box::new(output)),
        ..reply()
    };
    let frame = encode_reply(&response, &request).unwrap();
    assert_eq!(
        read_reply(&mut frame.as_slice(), &request).unwrap(),
        response
    );
    for change in [
        "holder",
        "endpoint",
        "identity",
        "directory",
        "coverage",
        "count",
    ] {
        let mut changed = response.clone();
        let value = changed.result.as_mut().unwrap();
        match change {
            "holder" => value.sockets[0].holders.pop().map(|_| ()).unwrap(),
            "endpoint" => value.sockets[0].endpoint.port += 1,
            "identity" => value.processes[0].identity.start_ticks += 1,
            "directory" => {
                value.processes[0]
                    .details
                    .as_mut()
                    .unwrap()
                    .directory
                    .as_mut()
                    .unwrap()
                    .value = "/srv/other".into()
            }
            "coverage" => value.coverage.ipv4 = Report::default(),
            _ => {
                value.processes.pop();
            }
        }
        assert!(encode_reply(&changed, &request).is_err(), "{change}");
    }
    assert!(encode_reply(&response, &super::tests::request()).is_err());
}

#[test]
fn incompatible_project_payload_is_refused_before_nested_decoding() {
    let input = pb::HelperRequest {
        contract_id: Some("0".repeat(64)),
        generation: 1,
        projects: Some(Box::new(pb::Snapshot::default())),
    };
    assert!(matches!(
        decode_request(input.encode_to_vec().into()),
        Err(Error::Refused(_))
    ));
}

#[test]
fn project_enrichment_cannot_clear_existing_metadata_failures() {
    let mut input = snapshot();
    input.coverage.details = Report {
        outcome: discovery::Outcome::Partial,
        issues: vec![discovery::Issue {
            kind: discovery::IssueKind::PermissionDenied,
            count: 2,
            message: "Restricted process".into(),
        }],
    };
    let request = Request {
        projects: Some(Box::new(input.clone())),
        ..request()
    };
    let mut output = input;
    output.coverage.details = Report::complete();
    assert!(
        encode_reply(
            &Reply {
                result: Ok(Box::new(output)),
                ..reply()
            },
            &request
        )
        .is_err()
    );
}

#[test]
fn missing_or_different_contract_is_a_typed_refusal() {
    for contract_id in [None, Some("0".repeat(64))] {
        let caller = Request {
            projects: None,
            contract_id,
            ..request()
        };
        let frame = encode_request(&caller).unwrap();
        let decoded = read_request(&mut frame.as_slice()).unwrap();
        let error = require_contract_id(decoded.contract_id.as_deref()).unwrap_err();
        assert_eq!(error.code, "incompatible");
        assert_eq!(error.retry, common::Retry::No);
        let refused = Reply {
            result: Err(error),
            ..reply()
        };
        let frame = encode_reply(&refused, &caller).unwrap();
        assert_eq!(read_reply(&mut frame.as_slice(), &caller).unwrap(), refused);
    }
}

#[test]
fn reply_admission_does_not_decode_an_incompatible_snapshot() {
    for contract in [None, Some("0".repeat(64))] {
        let mut raw = pb::HelperReply {
            contract_id: contract,
            generation: 1,
            result: None,
        }
        .encode_to_vec();
        // The nested payload is intentionally not a valid protobuf message.
        raw.extend_from_slice(&[0x1a, 1, 0xff]);
        match decode_reply(raw.into(), &request()).unwrap_err() {
            Error::Refused(error) => assert_eq!(error.code, "incompatible"),
            other => panic!("Payload was decoded before admission: {other}"),
        }
    }
}

#[test]
fn identity_helper_cannot_return_privileged_details_or_enrichment() {
    let base = snapshot();
    let mut variants = Vec::new();
    let mut value = base.clone();
    value.processes[0].details = Some(Details {
        arguments: vec!["private argument".into()],
        ..Default::default()
    });
    variants.push(value);
    let mut value = base.clone();
    value.processes[0].account = Some("root".into());
    variants.push(value);
    let mut value = base.clone();
    value.sockets[0].account = Some("root".into());
    variants.push(value);
    let mut value = base.clone();
    value.sockets[0].cgroup.as_mut().unwrap().path = Some("/system.slice/ssh.socket".into());
    variants.push(value);
    let mut value = base;
    value.coverage.details = Report::complete();
    variants.push(value);
    for value in variants {
        value.validate().unwrap();
        let raw = pb::HelperReply {
            contract_id: request().contract_id,
            generation: 1,
            result: Some(pb::helper_reply::Result::Snapshot(Box::new(
                value.to_wire(),
            ))),
        }
        .encode_to_vec();
        assert!(decode_reply(raw.into(), &request()).is_err());
        assert!(
            encode_reply(
                &Reply {
                    result: Ok(Box::new(value)),
                    ..reply()
                },
                &request()
            )
            .is_err()
        );
    }
}

#[test]
fn helper_frames_bound_allocation_and_reject_duplicates_and_truncation() {
    let mut duplicate = body(&reply());
    duplicate.extend_from_slice(&[0x10, 1]);
    assert!(decode_reply(duplicate.into(), &request()).is_err());
    let mut duplicate = body(&reply());
    duplicate.extend_from_slice(&[0x22, 0]);
    assert!(decode_reply(duplicate.into(), &request()).is_err());
    let mut frame = Vec::new();
    prost::encoding::encode_varint((REQUEST_LIMITS.message + 1) as u64, &mut frame);
    assert!(matches!(
        read_request(&mut frame.as_slice()),
        Err(Error::Wire(monitor::wire::Error::Frame(
            FrameError::TooLarge { .. }
        )))
    ));
    let frame = encode_reply(&reply(), &request()).unwrap();
    for length in [0, 1, frame.len() - 1] {
        assert!(read_reply(&mut &frame[..length], &request()).is_err());
    }
    for bad in [
        String::new(),
        "a".repeat(63),
        "A".repeat(64),
        "x".repeat(64),
    ] {
        assert!(
            encode_request(&Request {
                projects: None,
                contract_id: Some(bad),
                ..request()
            })
            .is_err()
        );
    }
    assert!(
        encode_request(&Request {
            generation: 0,
            ..request()
        })
        .is_err()
    );
}

#[test]
fn structured_scan_failures_preserve_retry_information() {
    let expected = Reply {
        result: Err(common::Error {
            code: "timed_out".into(),
            message: "Discovery time limit reached".into(),
            retry: common::Retry::Yes,
        }),
        ..reply()
    };
    let frame = encode_reply(&expected, &request()).unwrap();
    assert_eq!(
        read_reply(&mut frame.as_slice(), &request()).unwrap(),
        expected
    );
}
