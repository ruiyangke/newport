use super::*;
use newport_protocol::clipboard::{RouteOutcome, RouteReply};

fn route_result(call: &Call, identity: &crate::graphical_session::Identity) -> protocol::Reply {
    let protocol::Operation::ClipboardRoute(route) = &call.operation else {
        panic!("not a clipboard route");
    };
    protocol::Reply {
        id: 1,
        runtime_id: identity.runtime.instance.to_string(),
        result: Ok(protocol::ReplyValue::ClipboardRoute(RouteReply {
            revision: route.revision,
            worker_id: route.worker_id.clone(),
            outcome: RouteOutcome::Applied,
        })),
    }
}

fn route(reply: &mut protocol::Reply) -> &mut RouteReply {
    match &mut reply.result {
        Ok(protocol::ReplyValue::ClipboardRoute(route)) => route,
        _ => panic!("Expected clipboard route"),
    }
}

#[tokio::test]
async fn a_route_reports_whether_it_was_applied_or_its_provider_retired() {
    for retired in [false, true] {
        let slot = test_slot();
        let (_control, mut calls, identity) = clipboard_control(&slot);
        let respond = async {
            let call = calls.recv().await.unwrap();
            let mut reply = route_result(&call, &identity);
            if retired {
                route(&mut reply).outcome = RouteOutcome::Retired;
            }
            call.reply.send(Ok(reply)).unwrap();
        };
        let (result, ()) =
            tokio::join!(slot.clipboard_route_outcome(&identity, true, true), respond);
        assert_eq!(
            result,
            Ok(if retired {
                ClipboardRoute::Retired
            } else {
                ClipboardRoute::Applied(1)
            })
        );
        assert!(calls.try_recv().is_err(), "failed request retried");
    }
}

#[tokio::test]
async fn route_ack_requires_exact_worker_revision_runtime_and_response_shape() {
    for invalid in 0..7 {
        let slot = test_slot();
        let (_control, mut calls, identity) = clipboard_control(&slot);
        let respond = async {
            let call = calls.recv().await.unwrap();
            let mut reply = route_result(&call, &identity);
            route(&mut reply).outcome = RouteOutcome::Retired;
            match invalid {
                0 => reply.result = Ok(protocol::ReplyValue::Accepted),
                1 => {
                    route(&mut reply).worker_id =
                        Some(uuid::Uuid::new_v4().to_string())
                }
                2 => route(&mut reply).worker_id = None,
                3 => route(&mut reply).revision += 1,
                4 => route(&mut reply).revision = 0,
                5 => reply.runtime_id = uuid::Uuid::new_v4().to_string(),
                _ => {
                    reply.result = Ok(protocol::ReplyValue::Permissions(newport_protocol::runtime::Applied {
                        permissions: newport_protocol::runtime::Permissions {
                            revision: 1,
                            clipboard: false,
                            browser: false,
                        },
                        error: None,
                    }))
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
async fn retirement_never_masks_permission_or_transport_failure() {
    for transport in [false, true] {
        let slot = test_slot();
        let (_control, mut calls, identity) = clipboard_control(&slot);
        let error = if transport {
            "transport read failed"
        } else {
            "Clipboard permission denied"
        };
        let respond = async {
            let call = calls.recv().await.unwrap();
            let mut reply = route_result(&call, &identity);
            route(&mut reply).outcome = RouteOutcome::Retired;
            reply.result = Err(newport_protocol::common::Error { code: "denied".into(), message: error.into(), retry: newport_protocol::common::Retry::No });
            call.reply
                .send(if transport {
                    Err(Error::Interrupted(error.into()))
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
    let (_control, mut calls, identity) = clipboard_control(&slot);
    let respond = async {
        let call = calls.recv().await.unwrap();
        let mut reply = route_result(&call, &identity);
        route(&mut reply).outcome = RouteOutcome::Retired;
        call.reply.send(Ok(reply)).unwrap();
    };
    let (result, ()) = tokio::join!(slot.clipboard_route(&identity, false, false), respond);
    assert_eq!(
        result,
        Err("Graphical clipboard provider retired during display preparation".into())
    );
}
