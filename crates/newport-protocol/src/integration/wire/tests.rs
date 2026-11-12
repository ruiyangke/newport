use super::*;
use crate::worker::{ClientControl, wire as worker};
use prost::Message as _;

fn call(request: Request) -> ClientMessage {
    ClientMessage::Call(Call { id: 7, request })
}
fn failure() -> common::Error {
    common::Error {
        code: "clipboard_unavailable".into(),
        message: "No clipboard backend".into(),
        retry: common::Retry::No,
    }
}
fn round_trip<T: Message + PartialEq + fmt::Debug>(value: T) {
    let encoded = value.encode().unwrap();
    assert_eq!(read::<T>(&mut encoded.as_slice()).unwrap(), value);
    for end in [0, 1, encoded.len() - 1] {
        assert!(read::<T>(&mut &encoded[..end]).is_err());
    }
}
#[test]
fn directional_messages_preserve_revisions_optionality_and_reply_identity() {
    for message in [
        call(Request::Permissions(Permissions {
            revision: 1,
            clipboard: true,
            browser: false,
        })),
        call(Request::ClipboardOffer(imports::Offer {
            revision: -1,
            formats: vec!["text/plain".into()],
        })),
        call(Request::Snapshot(Bytes::from_static(b"archive"))),
        call(Request::GuiNotices),
        call(Request::Heartbeat),
        call(Request::Export(exports::Control::Enable { version: 1 })),
        ClientMessage::Clipboard(imports::Reply {
            id: 1,
            revision: -1,
            encoding: imports::Encoding::Raw,
            done: false,
            bytes: vec![],
        }),
        ClientMessage::Clipboard(imports::Reply {
            id: 2,
            revision: -1,
            encoding: imports::Encoding::Failed,
            done: true,
            bytes: vec![],
        }),
        ClientMessage::Browser(BrowserReply {
            id: 3,
            success: true,
            message: None,
        }),
        ClientMessage::Browser(BrowserReply {
            id: 3,
            success: false,
            message: Some(String::new()),
        }),
        ClientMessage::Quit,
    ] {
        round_trip(message);
    }
    for message in [
        ServerMessage::Response(Response {
            id: 7,
            reply: Reply::Acknowledged {
                message: "available".into(),
            },
        }),
        ServerMessage::Response(Response {
            id: 7,
            reply: Reply::Permissions(Applied {
                permissions: Permissions {
                    revision: 1,
                    clipboard: false,
                    browser: true,
                },
                error: Some(failure()),
            }),
        }),
        ServerMessage::Response(Response {
            id: 7,
            reply: Reply::Failed(failure()),
        }),
        ServerMessage::ClipboardRequest(imports::Request {
            id: 1,
            revision: -1,
            format: "text/plain".into(),
        }),
        ServerMessage::Browser(BrowserRequest {
            id: 2,
            url: "https://example.com/a?b=c#original".into(),
        }),
        ServerMessage::GuiNotice,
        ServerMessage::Export(exports::Event::Offer { offer: None }),
        ServerMessage::Failed(failure()),
    ] {
        round_trip(message);
    }
}
#[test]
fn maximum_offer_and_chunks_fit_and_snapshot_payload_is_not_metadata() {
    round_trip(call(Request::ClipboardOffer(imports::Offer {
        revision: i64::MIN,
        formats: (0..imports::MAX_FORMATS)
            .map(|n| format!("application/{n:03}{}", "x".repeat(241)))
            .collect(),
    })));
    round_trip(ClientMessage::Clipboard(imports::Reply {
        id: u64::MAX,
        revision: i64::MAX,
        encoding: imports::Encoding::Zlib,
        done: true,
        bytes: vec![255; imports::CHUNK],
    }));
    round_trip(call(Request::Snapshot(
        vec![0; CLIENT_LIMITS.metadata + 1].into(),
    )));
    assert_eq!(ClientMessage::Quit.encode().unwrap(), [2, 34, 0]);
    let encoded = call(Request::Snapshot(
        vec![0; CLIENT_LIMITS.metadata + 1].into(),
    ))
    .encode()
    .unwrap();
    assert!(read_limited::<ClientMessage>(&mut encoded.as_slice(), SERVER_LIMITS.message).is_err());
}
fn body<T: Message>(message: T) -> Bytes {
    protobuf::read_frame(&mut message.encode().unwrap().as_slice(), T::LIMITS.message)
        .unwrap()
        .unwrap()
}
#[test]
fn operation_ambiguity_duplicates_and_noncanonical_scalars_fail() {
    let original = body(call(Request::Heartbeat));
    let mut duplicated = original.to_vec();
    duplicated.extend_from_slice(&original);
    assert!(ClientMessage::decode(duplicated.into()).is_err());
    for value in [
        vec![10, 6, 8, 7, 8, 8, 50, 0],
        vec![10, 6, 8, 7, 50, 0, 42, 0],
        vec![26, 4, 8, 1, 16, 2],
        vec![18, 8, 8, 1, 24, 129, 128, 128, 128, 16],
    ] {
        assert!(ClientMessage::decode(value.into()).is_err());
    }
    let mut unknown = original.to_vec();
    unknown.extend([0xa0, 0x06, 1]);
    assert_eq!(
        ClientMessage::decode(unknown.into()).unwrap(),
        call(Request::Heartbeat)
    );
    assert!(ClientMessage::decode(Bytes::from_static(&[0xa0, 0x06, 1])).is_err());
}
#[test]
fn predecode_bounds_and_worker_reservations_are_enforced() {
    let encoded = pb::ClientFrame {
        message: Some(pb::client_frame::Message::Call(pb::Call {
            id: 1,
            request: Some(pb::call::Request::ClipboardOffer(pb::ClipboardOffer {
                revision: 1,
                formats: vec!["a/b".into(); imports::MAX_FORMATS + 1],
            })),
        })),
    }
    .encode_to_vec();
    assert!(scan::client(&encoded).is_err());
    let control = worker::encode_client(ClientControl::InputComplete).unwrap();
    let control = protobuf::read_frame(&mut control.as_slice(), 1024)
        .unwrap()
        .unwrap();
    assert!(ClientMessage::decode(control.clone()).is_err());
    let mut mixed = body(ClientMessage::Quit).to_vec();
    mixed.extend(control);
    assert!(ClientMessage::decode(mixed.into()).is_err());
    assert!(
        ServerMessage::decode(body(ClientMessage::Clipboard(imports::Reply {
            id: 1,
            revision: 0,
            encoding: imports::Encoding::Raw,
            done: true,
            bytes: vec![1]
        })))
        .is_err()
    );
}
#[test]
fn acknowledgments_match_calls_and_stale_permissions_return_the_current_revision() {
    let call = Call {
        id: 7,
        request: Request::Permissions(Permissions {
            revision: 2,
            clipboard: false,
            browser: false,
        }),
    };
    let response = Response {
        id: 7,
        reply: Reply::Permissions(Applied {
            permissions: Permissions {
                revision: 3,
                clipboard: true,
                browser: false,
            },
            error: None,
        }),
    };
    response.validate_for(&call).unwrap();
    assert!(
        Response {
            id: 8,
            ..response.clone()
        }
        .validate_for(&call)
        .is_err()
    );
    assert!(
        response
            .validate_for(&Call {
                id: 7,
                request: Request::Heartbeat
            })
            .is_err()
    );
    assert!(
        Response {
            id: 7,
            reply: Reply::Acknowledged {
                message: String::new()
            }
        }
        .validate_for(&call)
        .is_err()
    );
    let same = Response {
        id: 7,
        reply: Reply::Permissions(Applied {
            permissions: Permissions {
                revision: 2,
                clipboard: true,
                browser: false,
            },
            error: None,
        }),
    };
    assert!(same.validate_for(&call).is_err());
}
#[test]
fn urls_and_semantic_payloads_validate_before_writing() {
    for url in [
        "file:///tmp/a",
        "https://user:secret@example.com",
        "https://example.com/\n",
    ] {
        assert!(
            ServerMessage::Browser(BrowserRequest {
                id: 1,
                url: url.into()
            })
            .encode()
            .is_err()
        );
    }
    assert!(
        ClientMessage::Clipboard(imports::Reply {
            id: 1,
            revision: 0,
            encoding: imports::Encoding::Failed,
            done: false,
            bytes: vec![]
        })
        .encode()
        .is_err()
    );
    assert!(
        ClientMessage::Clipboard(imports::Reply {
            id: 0,
            revision: 0,
            encoding: imports::Encoding::Raw,
            done: true,
            bytes: vec![]
        })
        .encode()
        .is_err()
    );
    assert!(
        call(Request::ClipboardOffer(imports::Offer {
            revision: 0,
            formats: vec!["a/b".into(), "a/b".into()]
        }))
        .encode()
        .is_err()
    );
}
