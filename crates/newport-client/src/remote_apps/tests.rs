use super::*;
use crate::backend::exchange::Cause;
use newport_protocol::{
    common,
    desktop::{
        Kind,
        applications::{Identity, Launched, Observation, Runtime},
    },
};
use serde_json::json;
use std::{collections::BTreeSet, sync::Condvar, thread};

fn observations(rows: Vec<Observation>) -> protocol::Reply {
    protocol::Reply::States(Box::new(protocol::Observations { observations: rows }))
}
fn remote_error(
    code: &str,
    message: &str,
    license: Option<desktop::account::RefusalReason>,
) -> Error {
    Error::new(
        &Backend::at_path("fixture".into()),
        ExchangePhase::Response,
        true,
        Cause::Remote(desktop::Failure {
            error: common::Error {
                code: code.into(),
                message: message.into(),
                retry: common::Retry::No,
            },
            detail: license.map(|value| desktop::FailureDetail::LicenseRefusal(Box::new(value))),
        }),
    )
}
fn interrupted(message: &str) -> Error {
    Error::new(
        &Backend::at_path("fixture".into()),
        ExchangePhase::Response,
        true,
        std::io::Error::new(std::io::ErrorKind::ConnectionReset, message.to_owned()),
    )
}

fn scope() -> Scope {
    let id = uuid::Uuid::new_v4().to_string();
    // As the app makes it, including a server reached through a jump host.
    let server = json!({"id":id,"sshHost":"host","sshUser":"user","sshPort":22,"authMethod":"publicKey","proxy":{"jump":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"}});
    let expected = newport_domain::connection_scope::connection(
        &server,
        &json!({"instanceId":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"}),
    );
    Scope::new(&id, expected).unwrap()
}
fn handle(request: u64) -> Handle {
    Handle {
        display: Identity {
            runtime: Runtime {
                instance: uuid::Uuid::from_u128(1),
                build_sha256: "a".repeat(64),
                transport_generation: 1,
            },
            session: uuid::Uuid::from_u128(2),
            worker: uuid::Uuid::from_u128(3),
            display_generation: 1,
        },
        request: request.try_into().unwrap(),
    }
}
fn target() -> Target {
    Target::Command(Box::new(protocol::Command {
        argv: vec!["app".into(), "literal; $(text)".into()],
    }))
}
fn wait(check: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < until, "application controller stalled");
        thread::sleep(Duration::from_millis(2));
    }
}
fn existing(scope: Scope, request: u64) -> Application {
    Application(Arc::new(Shared {
        scope,
        state: Mutex::new(State {
            phase: Status::Running,
            handle: Some(handle(request)),
            stop_requested: false,
            stop_sent: false,
            stop_error: None,
        }),
    }))
}

#[test]
fn a_refused_launch_never_replays_or_claims_a_running_app() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mut controller = Controller::with_exchange(Arc::new(move |operation, _| {
        assert_eq!(operation.kind(), Kind::ApplicationsLaunch);
        seen.fetch_add(1, Ordering::SeqCst);
        Err(remote_error(
            "APPLICATION_NOT_SUBMITTED",
            "Remote apps isn’t set up for this server",
            None,
        ))
    }));
    let application = controller.launch(scope(), target()).unwrap();
    wait(|| matches!(application.status(), Status::Refused { .. }));
    assert!(application.0.state.lock().unwrap().handle.is_none());
    application.stop();
    controller.poll(vec![application.clone()]);
    wait(|| !controller.polling.load(Ordering::Acquire));
    assert!(matches!(
        application.status(),
        Status::Refused { action: None, .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn quit_during_launch_waits_for_its_receipt_and_preserves_other_apps() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let stopped = Arc::new(Mutex::new(BTreeSet::new()));
    let batches = Arc::new(Mutex::new(Vec::new()));
    let exchange = {
        let (gate, calls, stopped, batches) = (
            gate.clone(),
            calls.clone(),
            stopped.clone(),
            batches.clone(),
        );
        Arc::new(
            move |operation: protocol::Operation, _: Duration| match operation {
                protocol::Operation::Launch(args) => {
                    assert!(
                        matches!(&args.target, Target::Command(value) if value.argv == ["app", "literal; $(text)"])
                    );
                    let request = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if request == 1 {
                        let (lock, changed) = &*gate;
                        drop(
                            changed
                                .wait_while(lock.lock().unwrap(), |ready| !*ready)
                                .unwrap(),
                        );
                    }
                    Ok(protocol::Reply::Launch(Box::new(Launched {
                        application: handle(request as u64),
                        outcome: Outcome::Accepted(Box::new(common::Present {})),
                    })))
                }
                protocol::Operation::States(args) => {
                    let handles = args.applications;
                    batches.lock().unwrap().push(handles.len());
                    Ok(observations(
                        handles
                            .into_iter()
                            .map(|application| Observation {
                                state: Some(
                                    if stopped.lock().unwrap().contains(&application.request.get())
                                    {
                                        Phase::Ended
                                    } else {
                                        Phase::Running
                                    },
                                ),
                                application,
                                error: None,
                                ending: None,
                            })
                            .collect::<Vec<_>>(),
                    ))
                }
                protocol::Operation::Stop(args) => {
                    let application = args.application;
                    assert!(
                        stopped.lock().unwrap().insert(application.request.get()),
                        "duplicate stop"
                    );
                    Ok(protocol::Reply::Stop(Box::new(protocol::Stopped {
                        stopped: true,
                    })))
                }
                _ => panic!("unexpected application operation"),
            },
        ) as Arc<Exchange>
    };
    let mut controller = Controller::with_exchange(exchange);
    let scope = scope();
    let one = controller.launch(scope.clone(), target()).unwrap();
    wait(|| calls.load(Ordering::SeqCst) == 1);
    one.stop();
    assert_eq!(one.status(), Status::Stopping);
    let two = controller.launch(scope, target()).unwrap();
    wait(|| two.0.state.lock().unwrap().handle.is_some());
    assert_eq!(two.status(), Status::Starting);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    wait(|| one.0.state.lock().unwrap().handle.is_some());
    controller.poll(vec![one.clone(), two.clone()]);
    wait(|| !controller.polling.load(Ordering::Acquire));
    assert_eq!(*stopped.lock().unwrap(), BTreeSet::from([1]));
    assert_eq!(*batches.lock().unwrap(), [2]);
    assert_eq!(two.status(), Status::Running);
    one.stop(); // An acknowledged stop is coalesced.
    controller.next_poll = None;
    controller.poll(vec![one.clone(), two.clone()]);
    wait(|| one.status() == Status::Ended);
    assert_eq!(two.status(), Status::Running);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn launch_receipt_and_other_apps_do_not_clear_startup_and_quit_still_works() {
    let phase = Arc::new(Mutex::new(Phase::Opening));
    let stopped = Arc::new(AtomicBool::new(false));
    let exchange = {
        let (phase, stopped) = (phase.clone(), stopped.clone());
        Arc::new(
            move |operation: protocol::Operation, _: Duration| match operation {
                protocol::Operation::Launch(_) => Ok(protocol::Reply::Launch(Box::new(Launched {
                    application: handle(1),
                    outcome: Outcome::Accepted(Box::new(common::Present {})),
                }))),
                protocol::Operation::States(args) => {
                    let handles = args.applications;
                    Ok(observations(
                        handles
                            .into_iter()
                            .map(|application| Observation {
                                state: Some(if application.request.get() == 1 {
                                    *phase.lock().unwrap()
                                } else {
                                    Phase::Running
                                }),
                                application,
                                error: None,
                                ending: None,
                            })
                            .collect::<Vec<_>>(),
                    ))
                }
                protocol::Operation::Stop(args) => {
                    assert_eq!(args.application, handle(1));
                    stopped.store(true, Ordering::Release);
                    Ok(protocol::Reply::Stop(Box::new(protocol::Stopped {
                        stopped: true,
                    })))
                }
                _ => panic!("unexpected request"),
            },
        ) as Arc<Exchange>
    };
    let controller = Controller::with_exchange(exchange.clone());
    let scope = scope();
    let app = controller.launch(scope.clone(), target()).unwrap();
    wait(|| app.0.state.lock().unwrap().handle.is_some());
    let other = existing(scope.clone(), 2);
    let group = [(app.clone(), handle(1)), (other.clone(), handle(2))];
    assert_eq!(app.status(), Status::Starting);
    for _ in 0..3 {
        observe(&*exchange, &scope, &group);
        assert_eq!(app.status(), Status::Starting);
        assert_eq!(other.status(), Status::Running);
    }
    *phase.lock().unwrap() = Phase::Running;
    observe(&*exchange, &scope, &group);
    assert_eq!(app.status(), Status::Running);

    // Quit before a window exists must target the owned launcher too.
    *phase.lock().unwrap() = Phase::Opening;
    app.stop();
    observe(&*exchange, &scope, &group);
    assert!(stopped.load(Ordering::Acquire));
    assert_eq!(app.status(), Status::Stopping);
    *phase.lock().unwrap() = Phase::Ended;
    observe(&*exchange, &scope, &group);
    assert_eq!(app.status(), Status::Ended);
    assert_eq!(other.status(), Status::Running);
}

#[test]
fn lost_launch_reply_is_never_replayed_or_misreported_as_cancelled() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut controller = Controller::with_exchange(Arc::new(move |operation, _| {
        assert_eq!(operation.kind(), Kind::ApplicationsLaunch);
        observed.fetch_add(1, Ordering::SeqCst);
        Err(interrupted("reply lost after submission"))
    }));
    let application = controller.launch(scope(), target()).unwrap();
    wait(|| matches!(application.status(), Status::Unknown(_)));
    application.stop();
    for _ in 0..3 {
        controller.next_poll = None;
        controller.poll(vec![application.clone()]);
        wait(|| !controller.polling.load(Ordering::Acquire));
    }
    assert!(matches!(application.status(), Status::Unknown(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn mismatched_observations_cannot_authorize_quit_on_another_receipt() {
    let scope = scope();
    let one = existing(scope.clone(), 1);
    let two = existing(scope.clone(), 2);
    one.stop();
    let exchange = |operation: protocol::Operation, _: Duration| {
        assert_eq!(operation.kind(), Kind::ApplicationsStates);
        Ok(observations(vec![
            Observation {
                application: handle(2),
                state: Some(Phase::Running),
                error: None,
                ending: None,
            },
            Observation {
                application: handle(1),
                state: Some(Phase::Running),
                error: None,
                ending: None,
            },
        ]))
    };
    observe(
        &exchange,
        &scope,
        &[(one.clone(), handle(1)), (two.clone(), handle(2))],
    );
    assert!(matches!(one.status(), Status::Unknown(_)));
    assert!(matches!(two.status(), Status::Unknown(_)));
}

#[test]
fn uncertain_stop_requires_an_explicit_retry_and_never_relaunches() {
    let scope = scope();
    let application = existing(scope.clone(), 1);
    let stops = Arc::new(AtomicUsize::new(0));
    let observed = stops.clone();
    let exchange = move |operation: protocol::Operation, _: Duration| match operation {
        protocol::Operation::States(_) => Ok(observations(vec![Observation {
            application: handle(1),
            state: Some(Phase::Running),
            error: None,
            ending: None,
        }])),
        protocol::Operation::Stop(_) => {
            observed.fetch_add(1, Ordering::SeqCst);
            Err(interrupted("stop response lost"))
        }
        _ => panic!("unexpected request"),
    };
    application.stop();
    for _ in 0..3 {
        observe(&exchange, &scope, &[(application.clone(), handle(1))]);
    }
    assert!(matches!(application.status(), Status::Unknown(_)));
    assert_eq!(stops.load(Ordering::SeqCst), 1);
    application.stop();
    observe(&exchange, &scope, &[(application.clone(), handle(1))]);
    assert_eq!(stops.load(Ordering::SeqCst), 2);
}

#[test]
fn known_admission_refusals_preserve_account_actions() {
    assert_eq!(
        refusal(remote_error(
            "APPLICATION_REFUSED",
            "Sign in",
            Some(desktop::account::RefusalReason::SignedOut)
        )),
        Status::Refused {
            message: "Sign in".into(),
            action: Some(account::Action::SignIn)
        }
    );
    assert!(matches!(
        refusal(remote_error(
            "APPLICATION_NOT_SUBMITTED",
            "Stale connection",
            None
        )),
        Status::Refused { .. }
    ));
    assert!(Scope::new(&uuid::Uuid::new_v4().to_string(), Value::Null).is_err());
}

#[test]
fn changed_connection_retires_observation_without_quitting_or_adopting_a_new_display() {
    let scope = scope();
    let application = existing(scope, 1);
    application.stop();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut controller = Controller::with_exchange(Arc::new(move |operation, _| {
        assert_eq!(
            operation.kind(),
            Kind::ApplicationsStates,
            "must not send Quit or relaunch"
        );
        let call = observed.fetch_add(1, Ordering::SeqCst);
        Err(if call == 0 {
            interrupted("temporary local IPC failure")
        } else {
            remote_error(
                "APPLICATION_CONNECTION_CHANGED",
                "Original connection was replaced",
                None,
            )
        })
    }));
    controller.poll(vec![application.clone()]);
    wait(|| !controller.polling.load(Ordering::Acquire));
    assert!(matches!(application.status(), Status::Unknown(_)));
    controller.next_poll = None;
    controller.poll(vec![application.clone()]);
    wait(|| !controller.polling.load(Ordering::Acquire));
    assert!(matches!(application.status(), Status::Unavailable(_)));
    application.stop();
    controller.next_poll = None;
    controller.poll(vec![application.clone()]);
    wait(|| !controller.polling.load(Ordering::Acquire));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(matches!(application.status(), Status::Unavailable(_)));
}

#[test]
fn ended_rows_carry_why_there_was_no_window() {
    use newport_protocol::desktop::applications::Ending;
    let scope = scope();
    let applications: Vec<_> = (1..=6)
        .map(|request| existing(scope.clone(), request))
        .collect();
    let endings = [
        Some(Ending::Failed(Box::new("boom".into()))),
        Some(Ending::NoWindow(Box::new(protocol::OptionalDetail {
            detail: Some("done".into()),
        }))),
        Some(Ending::NoWindow(Box::new(protocol::OptionalDetail {
            detail: None,
        }))),
        Some(Ending::HandedOff(Box::new(common::Present {}))),
        None,
        Some(Ending::DisplayStopped(Box::new(common::Present {}))),
    ];
    let exchange = move |operation: protocol::Operation, _: Duration| {
        assert_eq!(operation.kind(), Kind::ApplicationsStates);
        Ok(observations(
            endings
                .iter()
                .enumerate()
                .map(|(index, ending)| Observation {
                    application: handle(index as u64 + 1),
                    state: Some(Phase::Ended),
                    error: None,
                    ending: ending.clone(),
                })
                .collect::<Vec<_>>(),
        ))
    };
    let group: Vec<_> = applications
        .iter()
        .enumerate()
        .map(|(index, application)| (application.clone(), handle(index as u64 + 1)))
        .collect();
    observe(&exchange, &scope, &group);
    let statuses: Vec<_> = applications.iter().map(Application::status).collect();
    assert_eq!(
        statuses,
        [
            Status::Failed("boom".into()),
            Status::EndedWithoutWindow(Some("done".into())),
            Status::EndedWithoutWindow(None),
            Status::HandedOff,
            Status::Ended,
            Status::DisplayStopped,
        ]
    );
    assert!(statuses.iter().all(Status::ended));
}

#[test]
fn an_ending_after_the_person_asked_to_quit_is_a_plain_end() {
    use newport_protocol::desktop::applications::Ending;
    let scope = scope();
    let applications: Vec<_> = (1..=5)
        .map(|request| existing(scope.clone(), request))
        .collect();
    for application in applications[..3].iter().chain(&applications[4..]) {
        application.stop();
        application.0.state.lock().unwrap().stop_sent = true;
    }
    let endings = [
        Some(Ending::Failed(Box::new("killed".into()))),
        Some(Ending::NoWindow(Box::new(protocol::OptionalDetail {
            detail: Some("terminated".into()),
        }))),
        Some(Ending::HandedOff(Box::new(common::Present {}))),
        Some(Ending::Failed(Box::new("boom".into()))),
        // The last app's display closes as it quits.
        Some(Ending::DisplayStopped(Box::new(common::Present {}))),
    ];
    let exchange = move |_: protocol::Operation, _: Duration| {
        Ok(observations(
            endings
                .iter()
                .enumerate()
                .map(|(index, ending)| Observation {
                    application: handle(index as u64 + 1),
                    state: Some(Phase::Ended),
                    error: None,
                    ending: ending.clone(),
                })
                .collect::<Vec<_>>(),
        ))
    };
    let group: Vec<_> = applications
        .iter()
        .enumerate()
        .map(|(index, application)| (application.clone(), handle(index as u64 + 1)))
        .collect();
    observe(&exchange, &scope, &group);
    let statuses: Vec<_> = applications.iter().map(Application::status).collect();
    assert_eq!(
        statuses,
        [
            Status::Ended,
            Status::Ended,
            Status::HandedOff,
            Status::Failed("boom".into()),
            Status::Ended,
        ]
    );
}

#[test]
fn an_app_without_a_window_yet_runs_in_the_background_and_can_quit() {
    let scope = scope();
    let application = existing(scope.clone(), 1);
    let phase = Arc::new(Mutex::new(Phase::Background));
    let stops = Arc::new(AtomicUsize::new(0));
    let exchange = {
        let (phase, stops) = (phase.clone(), stops.clone());
        move |operation: protocol::Operation, _: Duration| match operation {
            protocol::Operation::States(_) => Ok(observations(vec![Observation {
                application: handle(1),
                state: Some(*phase.lock().unwrap()),
                error: None,
                ending: None,
            }])),
            protocol::Operation::Stop(_) => {
                stops.fetch_add(1, Ordering::SeqCst);
                Ok(protocol::Reply::Stop(Box::new(protocol::Stopped {
                    stopped: true,
                })))
            }
            other => panic!("{other:?}"),
        }
    };
    let group = [(application.clone(), handle(1))];
    observe(&exchange, &scope, &group);
    assert_eq!(application.status(), Status::Background);
    assert!(!application.status().ended());

    // A window that opens later is a normal running app.
    *phase.lock().unwrap() = Phase::Running;
    observe(&exchange, &scope, &group);
    assert_eq!(application.status(), Status::Running);

    *phase.lock().unwrap() = Phase::Background;
    observe(&exchange, &scope, &group);
    application.stop();
    assert_eq!(application.status(), Status::Stopping);
    observe(&exchange, &scope, &group);
    assert_eq!(stops.load(Ordering::SeqCst), 1);
}

#[test]
fn an_app_shown_elsewhere_keeps_running_there_and_runs_here_once_shown_here_again() {
    use newport_protocol::desktop::applications::Ending;
    let scope = scope();
    let application = existing(scope.clone(), 1);
    let elsewhere = || {
        Ok((
            Phase::Ended,
            Some(Ending::ShownElsewhere(Box::new(protocol::OptionalDetail {
                detail: Some("studio-mac".into()),
            }))),
        ))
    };
    type ObservedState = Result<(Phase, Option<Ending>), Option<String>>;
    let row: Arc<Mutex<ObservedState>> = Arc::new(Mutex::new(elsewhere()));
    let exchange = {
        let row = row.clone();
        move |operation: protocol::Operation, _: Duration| {
            assert_eq!(operation.kind(), Kind::ApplicationsStates);
            match row.lock().unwrap().clone() {
                Ok((state, ending)) => Ok(observations(vec![Observation {
                    application: handle(1),
                    state: Some(state),
                    error: None,
                    ending,
                }])),
                Err(code) => Err(code.map_or_else(
                    || interrupted("The connection is busy."),
                    |code| remote_error(&code, "The connection is busy.", None),
                )),
            }
        }
    };
    let group = [(application.clone(), handle(1))];
    observe(&exchange, &scope, &group);
    let shown = Status::ShownElsewhere(Some("studio-mac".into()));
    assert_eq!(application.status(), shown);
    // Still observed, and never a failure or uncertain.
    assert!(!shown.ended());
    *row.lock().unwrap() = Err(None);
    observe(&exchange, &scope, &group);
    assert_eq!(application.status(), shown);
    // Shown here again: it runs here, and is still observed.
    *row.lock().unwrap() = Ok((Phase::Running, None));
    observe(&exchange, &scope, &group);
    assert_eq!(application.status(), Status::Running);
    // Another computer takes it over again.
    *row.lock().unwrap() = elsewhere();
    observe(&exchange, &scope, &group);
    assert_eq!(application.status(), shown);
    // Its display ending there ends it here with nothing more to say.
    *row.lock().unwrap() = Ok((
        Phase::Ended,
        Some(Ending::DisplayStopped(Box::new(common::Present {}))),
    ));
    observe(&exchange, &scope, &group);
    assert_eq!(application.status(), Status::Ended);
    // Its connection changing while it is shown elsewhere ends it here too.
    let moved = existing(scope.clone(), 1);
    moved.0.state.lock().unwrap().phase = shown;
    *row.lock().unwrap() = Err(Some("APPLICATION_CONNECTION_CHANGED".into()));
    observe(&exchange, &scope, &[(moved.clone(), handle(1))]);
    assert_eq!(moved.status(), Status::Ended);
}

#[test]
fn show_asks_for_the_servers_display_and_reports_the_result() {
    let (send, receive) = std::sync::mpsc::channel();
    let scope = scope();
    let expected = scope.clone();
    let controller = Controller::with_exchange(Arc::new(move |operation, _| {
        let protocol::Operation::Show(args) = operation else {
            panic!("expected show");
        };
        assert_eq!(args.server, expected.server);
        assert_eq!(args.expected, expected.expected);
        Err(remote_error(
            "APPLICATION_NOT_SUBMITTED",
            "Remote apps isn’t set up for this server",
            None,
        ))
    }));
    controller
        .show(scope, move |result| send.send(result).unwrap())
        .unwrap();
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(5)).unwrap(),
        Err("Remote apps isn’t set up for this server".into())
    );
}
