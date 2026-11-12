use super::*;
use serde_json::json;

const ID: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "22222222-2222-4222-8222-222222222222";

#[test]
fn responses_match_both_the_call_and_the_durable_operation() {
    let mut call = Call {
        id: ID.into(),
        request: Box::new(Request::Close {
            repo_id: "repo".into(),
        }),
    };
    call.validate().unwrap();
    let mut response = Response {
        id: ID.into(),
        result: Ok(Reply::Close(Box::new(Closed { closed: true }))),
    };
    response.validate_for(&call).unwrap();
    response.id = OTHER.into();
    assert!(response.validate_for(&call).is_err());
    response.id = ID.into();
    response.result = Ok(Reply::StreamComplete(Box::new(StreamComplete {
        last_seq: 0,
        bytes: 0,
    })));
    assert!(response.validate_for(&call).is_err());
    *call.request = Request::Get {
        operation_id: OTHER.into(),
    };
    let record = Operation {
        operation_id: ID.into(),
        repository: "repo".into(),
        payload_hash: "hash".into(),
        state: OperationState::OutcomeUnknown,
        seq: 2,
        result: None,
        error: None,
    };
    response.result = Ok(Reply::Operation(Box::new(record.clone())));
    assert!(response.validate_for(&call).is_err());
    response.result = Ok(Reply::Operation(Box::new(Operation {
        operation_id: OTHER.into(),
        ..record
    })));
    response.validate_for(&call).unwrap();
    response.result = Err(Error::new(
        "OPERATION_NOT_FOUND",
        "No record for this client.",
    ));
    response.validate_for(&call).unwrap();
}

#[test]
fn binary_data_keeps_absent_distinct_from_empty_and_excludes_contents_from_debug() {
    let object = json!({"algorithm":"sha256", "hex":"a".repeat(64)});
    let absent: Blob =
        serde_json::from_value(json!({"oid":object, "size":99, "bytesB64":null, "truncated":true}))
            .unwrap();
    let empty: Blob =
        serde_json::from_value(json!({"oid":object, "size":0, "bytesB64":"", "truncated":false}))
            .unwrap();
    assert!(absent.bytes_b64.is_none());
    assert_eq!(empty.bytes_b64, Some(Binary(vec![])));
    let value = Binary(b"private blob contents\0\xff".to_vec());
    let encoded = serde_json::to_value(&value).unwrap();
    assert_eq!(serde_json::from_value::<Binary>(encoded).unwrap(), value);
    assert!(!format!("{value:?}").contains("private"));
    assert!(serde_json::from_value::<Binary>(json!("not/base64!")).is_err());
}

#[test]
fn operation_records_have_explicit_result_and_state_shapes() {
    let mut value = json!({"operationId":ID, "repository":"repo", "payloadHash":"hash",
        "state":"succeeded", "seq":2, "result":{"renamedReferences":4, "token":"token", "refreshRequired":true}, "error":null});
    let record: Operation = serde_json::from_value(value.clone()).unwrap();
    assert!(matches!(
        record.result,
        Some(OperationResult::RemoteRenamed(_))
    ));
    assert_eq!(serde_json::to_value(record).unwrap(), value);
    value["state"] = json!("unrecognized");
    assert!(serde_json::from_value::<Operation>(value.clone()).is_err());
    value["state"] = json!("succeeded");
    value["result"]["extra"] = json!(true);
    assert!(serde_json::from_value::<Operation>(value).is_err());
}

#[test]
fn diff_line_projections_keep_binary_content_and_optional_selection_identity() {
    let object = json!({"lineIndex":2, "byteOffset":5, "lineComplete":true, "origin":"+",
        "oldLine":null, "newLine":9, "contentBytesB64":"AP8=", "id":"line-id"});
    let line: LineFragment = serde_json::from_value(object.clone()).unwrap();
    let tuple: LineFragment =
        serde_json::from_value(json!([2, 5, true, "+", null, 9, "AP8=", "line-id"])).unwrap();
    assert_eq!(line, tuple);
    assert_eq!(line.content_bytes_b64.0, [0, 255]);
    assert_eq!(serde_json::to_value(line).unwrap(), object);
    let historical: LineFragment =
        serde_json::from_value(json!([2, 5, true, "+", null, 9, "AP8="])).unwrap();
    assert_eq!(historical.id, None);
    assert!(
        serde_json::from_value::<LineFragment>(json!([
            2, 5, true, "+", null, 9, "AP8=", null, "extra"
        ]))
        .is_err()
    );
    let mut unknown = object;
    unknown["extra"] = json!(1);
    assert!(serde_json::from_value::<LineFragment>(unknown).is_err());
}

#[test]
fn requests_bound_paths_collections_and_aggregate_data() {
    Request::Open {
        path: Path::new(b"/tmp/binary-\xff"),
    }
    .validate()
    .unwrap();
    for bytes in [vec![], vec![0], vec![b'x'; MAX_PATH + 1]] {
        assert!(
            Request::Open {
                path: Path::new(&bytes)
            }
            .validate()
            .is_err()
        );
    }
    let request = |entries| Request::Start {
        operation_id: ID.into(),
        repo_id: "repo".into(),
        expected_snapshot: "snapshot".into(),
        action: Action::Stage {
            entry_ids: entries,
            hunks: None,
        },
    };
    request(vec!["entry".into()]).validate().unwrap();
    assert!(
        request(vec!["entry".into(); validate::MAX_ITEMS + 1])
            .validate()
            .is_err()
    );
    assert!(
        request(vec!["x".repeat(MAX_TEXT); MAX_FRAME / MAX_TEXT + 1])
            .validate()
            .is_err()
    );
    assert!(
        Request::StatusSummary {
            repo_id: None,
            path: None
        }
        .validate()
        .is_err()
    );
    assert!(
        Request::StatusSummary {
            repo_id: Some("repo".into()),
            path: Some(Path::new(b"/tmp"))
        }
        .validate()
        .is_err()
    );
}

#[test]
fn configuration_and_stream_controls_have_bounded_identities_and_payloads() {
    let mut configure = Configure {
        id: ID.into(),
        client_id: OTHER.into(),
        client_version: "test".into(),
        command_logs: true,
    };
    ClientMessage::Configure(configure.clone())
        .validate()
        .unwrap();
    configure.client_id = "invalid".into();
    assert!(ClientMessage::Configure(configure).validate().is_err());
    assert!(
        ClientMessage::Ack(Ack {
            stream_id: ID.into(),
            seq: 0
        })
        .validate()
        .is_err()
    );
    assert!(ClientMessage::Ping("x".repeat(129)).validate().is_err());
    for length in [0, CHUNK_SIZE + 1] {
        assert!(
            ServerMessage::Chunk(Chunk {
                id: ID.into(),
                stream_id: OTHER.into(),
                seq: 1,
                data: Binary(vec![1; length])
            })
            .validate()
            .is_err()
        );
    }
    ServerMessage::Chunk(Chunk {
        id: ID.into(),
        stream_id: OTHER.into(),
        seq: 1,
        data: Binary(vec![1; CHUNK_SIZE]),
    })
    .validate()
    .unwrap();
}
