use super::*;
use protocol::clipboard::{RouteOutcome, RouteReply};

fn typed_reply(call: &Call, identity: &crate::graphical_session::Identity) -> protocol::Reply {
    let protocol::Operation::ClipboardRouteV2 { route } = &call.operation else {
        panic!("v2 capability was not selected");
    };
    protocol::Reply {
        id: "fixture".into(),
        runtime_id: identity.runtime.instance.to_string(),
        error: None,
        applied: None,
        worker_stream: None,
        clipboard_route: Some(RouteReply {
            revision: route.revision,
            worker_id: route.worker_id.clone(),
            outcome: RouteOutcome::Applied,
        }),
    }
}

#[tokio::test]
async fn capability_selects_typed_v2_and_preserves_v1_fallback_without_retry() {
    for typed in [false, true] {
        for retired in [false, true] {
            let slot = test_slot();
            let (_control, mut calls, identity) = clipboard_control_version(&slot, typed);
            let respond = async {
                let call = calls.recv().await.unwrap();
                if typed {
                    let mut reply = typed_reply(&call, &identity);
                    if retired {
                        reply.clipboard_route.as_mut().unwrap().outcome = RouteOutcome::Retired;
                    }
                    call.reply.send(Ok(reply)).unwrap();
                } else {
                    route_reply(
                        call,
                        &identity,
                        retired.then(|| "legacy provider rejection".into()),
                    );
                }
            };
            let (result, ()) =
                tokio::join!(slot.clipboard_route_outcome(&identity, true, true), respond);
            assert_eq!(
                result,
                match (typed, retired) {
                    (true, true) => Ok(ClipboardRoute::Retired),
                    (false, true) => Err("legacy provider rejection".into()),
                    _ => Ok(ClipboardRoute::Applied(1)),
                }
            );
            assert!(
                calls.try_recv().is_err(),
                "failed request retried or downgraded"
            );
        }
    }
}

#[tokio::test]
async fn typed_ack_requires_exact_worker_revision_runtime_and_response_shape() {
    for invalid in 0..7 {
        let slot = test_slot();
        let (_control, mut calls, identity) = clipboard_control_version(&slot, true);
        let respond = async {
            let call = calls.recv().await.unwrap();
            let mut reply = typed_reply(&call, &identity);
            reply.clipboard_route.as_mut().unwrap().outcome = RouteOutcome::Retired;
            match invalid {
                0 => reply.clipboard_route = None,
                1 => {
                    reply.clipboard_route.as_mut().unwrap().worker_id =
                        Some(uuid::Uuid::new_v4().to_string())
                }
                2 => reply.clipboard_route.as_mut().unwrap().worker_id = None,
                3 => reply.clipboard_route.as_mut().unwrap().revision += 1,
                4 => reply.clipboard_route.as_mut().unwrap().revision = 0,
                5 => reply.runtime_id = uuid::Uuid::new_v4().to_string(),
                _ => {
                    reply.applied = Some(protocol::integration::Applied {
                        permissions: protocol::integration::Permissions {
                            revision: 1,
                            clipboard: false,
                            browser: false,
                        },
                        error: None,
                    })
                }
            }
            call.reply.send(Ok(reply)).unwrap();
        };
        let (result, ()) =
            tokio::join!(slot.clipboard_route_outcome(&identity, true, true), respond);
        assert_eq!(result, Err("Invalid clipboard route acknowledgment".into()));
        assert!(calls.try_recv().is_err());
    }
}

#[tokio::test]
async fn typed_retirement_never_masks_permission_or_transport_failure() {
    for transport in [false, true] {
        let slot = test_slot();
        let (_control, mut calls, identity) = clipboard_control_version(&slot, true);
        let error = if transport {
            "transport read failed"
        } else {
            "Clipboard permission denied"
        };
        let respond = async {
            let call = calls.recv().await.unwrap();
            let mut reply = typed_reply(&call, &identity);
            reply.clipboard_route.as_mut().unwrap().outcome = RouteOutcome::Retired;
            reply.error = Some(error.into());
            call.reply
                .send(if transport {
                    Err(error.into())
                } else {
                    Ok(reply)
                })
                .unwrap();
        };
        let (result, ()) = tokio::join!(
            slot.clipboard_route_outcome(&identity, false, false),
            respond
        );
        assert_eq!(result, Err(error.into()));
        assert!(calls.try_recv().is_err());
    }
}

#[tokio::test]
async fn initial_preparation_does_not_accept_retired_provider_as_ready() {
    let slot = test_slot();
    let (_control, mut calls, identity) = clipboard_control_version(&slot, true);
    let respond = async {
        let call = calls.recv().await.unwrap();
        let mut reply = typed_reply(&call, &identity);
        reply.clipboard_route.as_mut().unwrap().outcome = RouteOutcome::Retired;
        call.reply.send(Ok(reply)).unwrap();
    };
    let (result, ()) = tokio::join!(slot.clipboard_route(&identity, false, false), respond);
    assert_eq!(
        result,
        Err("Graphical clipboard provider retired during display preparation".into())
    );
}
