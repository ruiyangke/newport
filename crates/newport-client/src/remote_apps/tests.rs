use super::*;
use newport_platform::ipc::applications::{Identity, Runtime};
use std::{collections::BTreeSet, sync::Condvar, thread};

fn scope() -> Scope {
    let id = uuid::Uuid::new_v4().to_string();
    // As the app makes it, including a server reached through a jump host.
    let server =
        json!({"id":id,"sshHost":"host","sshUser":"user","sshPort":22,"proxy":{"jump":"bastion"}});
    let expected =
        newport_domain::connection_scope::connection(&server, &json!({"instanceId":"backend"}));
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
    Target::Command {
        argv: vec!["app".into(), "literal; $(text)".into()],
    }
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
            notice: None,
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
    let mut controller = Controller::with_exchange(Arc::new(move |method, _, _| {
        assert_eq!(method, "remote_apps_launch");
        seen.fetch_add(1, Ordering::SeqCst);
        Err(RequestError {
            code: Some("APPLICATION_NOT_SUBMITTED".into()),
            message: "Remote apps isn’t set up for this server".into(),
        })
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
        Arc::new(move |method: &str, args: Value, _: Duration| match method {
            "remote_apps_launch" => {
                assert_eq!(args["target"], serde_json::to_value(target()).unwrap());
                let request = calls.fetch_add(1, Ordering::SeqCst) + 1;
                if request == 1 {
                    let (lock, changed) = &*gate;
                    drop(
                        changed
                            .wait_while(lock.lock().unwrap(), |ready| !*ready)
                            .unwrap(),
                    );
                }
                Ok(json!(Launched {
                    application: handle(request as u64),
                    outcome: Outcome::Accepted
                }))
            }
            "remote_apps_states" => {
                let handles: Vec<Handle> =
                    serde_json::from_value(args["applications"].clone()).unwrap();
                batches.lock().unwrap().push(handles.len());
                Ok(json!(
                    handles
                        .into_iter()
                        .map(|application| Observation {
                            state: Some(
                                if stopped.lock().unwrap().contains(&application.request.get()) {
                                    Phase::Ended
                                } else {
                                    Phase::Running
                                }
                            ),
                            application,
                            error: None,
                            ending: None,
                            notice: None,
                        })
                        .collect::<Vec<_>>()
                ))
            }
            "remote_apps_stop" => {
                let application: Handle =
                    serde_json::from_value(args["application"].clone()).unwrap();
                assert!(
                    stopped.lock().unwrap().insert(application.request.get()),
                    "duplicate stop"
                );
                Ok(json!(true))
            }
            _ => panic!("unexpected method {method}"),
        }) as Arc<Exchange>
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
        Arc::new(move |method: &str, args: Value, _: Duration| match method {
            "remote_apps_launch" => Ok(json!(Launched {
                application: handle(1),
                outcome: Outcome::Accepted,
            })),
            "remote_apps_states" => {
                let handles: Vec<Handle> =
                    serde_json::from_value(args["applications"].clone()).unwrap();
                Ok(json!(
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
                            notice: None,
                        })
                        .collect::<Vec<_>>()
                ))
            }
            "remote_apps_stop" => {
                assert_eq!(args["application"], json!(handle(1)));
                stopped.store(true, Ordering::Release);
                Ok(json!(true))
            }
            _ => panic!("unexpected request"),
        }) as Arc<Exchange>
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
    let mut controller = Controller::with_exchange(Arc::new(move |method, _, _| {
        assert_eq!(method, "remote_apps_launch");
        observed.fetch_add(1, Ordering::SeqCst);
        Err(RequestError::from("reply lost after submission"))
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
    let exchange = |method: &str, _: Value, _: Duration| {
        assert_eq!(method, "remote_apps_states");
        Ok(json!([
            Observation {
                application: handle(2),
                state: Some(Phase::Running),
                error: None,
                ending: None,
                notice: None
            },
            Observation {
                application: handle(1),
                state: Some(Phase::Running),
                error: None,
                ending: None,
                notice: None
            }
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
    let exchange = move |method: &str, _: Value, _: Duration| match method {
        "remote_apps_states" => Ok(json!([Observation {
            application: handle(1),
            state: Some(Phase::Running),
            error: None,
            ending: None,
            notice: None
        }])),
        "remote_apps_stop" => {
            observed.fetch_add(1, Ordering::SeqCst);
            Err("stop response lost".into())
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
        refusal(RequestError {
            code: Some("APPLICATION_REFUSED:signed_out".into()),
            message: "Sign in".into()
        }),
        Status::Refused {
            message: "Sign in".into(),
            action: Some(account::Action::SignIn)
        }
    );
    assert!(matches!(
        refusal(RequestError {
            code: Some("APPLICATION_NOT_SUBMITTED".into()),
            message: "Stale connection".into()
        }),
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
    let mut controller = Controller::with_exchange(Arc::new(move |method, _, _| {
        assert_eq!(
            method, "remote_apps_states",
            "must not send Quit or relaunch"
        );
        let call = observed.fetch_add(1, Ordering::SeqCst);
        Err(if call == 0 {
            RequestError::from("temporary local IPC failure")
        } else {
            RequestError {
                code: Some("APPLICATION_CONNECTION_CHANGED".into()),
                message: "Original connection was replaced".into(),
            }
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
    use newport_platform::ipc::applications::Ending;
    let scope = scope();
    let applications: Vec<_> = (1..=6)
        .map(|request| existing(scope.clone(), request))
        .collect();
    let endings = [
        Some(Ending::Failed("boom".into())),
        Some(Ending::NoWindow(Some("done".into()))),
        Some(Ending::NoWindow(None)),
        Some(Ending::HandedOff),
        None,
        Some(Ending::DisplayStopped),
    ];
    let exchange = move |method: &str, _: Value, _: Duration| {
        assert_eq!(method, "remote_apps_states");
        Ok(json!(
            endings
                .iter()
                .enumerate()
                .map(|(index, ending)| Observation {
                    application: handle(index as u64 + 1),
                    state: Some(Phase::Ended),
                    error: None,
                    ending: ending.clone(),
                    notice: None,
                })
                .collect::<Vec<_>>()
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
fn a_running_app_keeps_running_with_a_notice_until_it_ends() {
    let scope = scope();
    let application = existing(scope.clone(), 1);
    let notice = Notice::DialogsElsewhere(Elsewhere::ServerScreen);
    let phase = Arc::new(Mutex::new(Phase::Running));
    let exchange = {
        let phase = phase.clone();
        move |_: &str, _: Value, _: Duration| {
            let phase = *phase.lock().unwrap();
            Ok(json!([Observation {
                application: handle(1),
                state: Some(phase),
                error: None,
                ending: None,
                notice: (phase != Phase::Ended).then_some(notice),
            }]))
        }
    };
    let group = [(application.clone(), handle(1))];
    assert_eq!(application.notice(), None);
    observe(&exchange, &scope, &group);
    assert_eq!(application.status(), Status::Running);
    assert_eq!(application.notice(), Some(notice));
    *phase.lock().unwrap() = Phase::Ended;
    observe(&exchange, &scope, &group);
    assert_eq!(application.status(), Status::Ended);
    assert_eq!(application.notice(), None);
}

#[test]
fn an_ending_after_the_person_asked_to_quit_is_a_plain_end() {
    use newport_platform::ipc::applications::Ending;
    let scope = scope();
    let applications: Vec<_> = (1..=5)
        .map(|request| existing(scope.clone(), request))
        .collect();
    for application in applications[..3].iter().chain(&applications[4..]) {
        application.stop();
        application.0.state.lock().unwrap().stop_sent = true;
    }
    let endings = [
        Some(Ending::Failed("killed".into())),
        Some(Ending::NoWindow(Some("terminated".into()))),
        Some(Ending::HandedOff),
        Some(Ending::Failed("boom".into())),
        // The last app's display closes as it quits.
        Some(Ending::DisplayStopped),
    ];
    let exchange = move |_: &str, _: Value, _: Duration| {
        Ok(json!(
            endings
                .iter()
                .enumerate()
                .map(|(index, ending)| Observation {
                    application: handle(index as u64 + 1),
                    state: Some(Phase::Ended),
                    error: None,
                    ending: ending.clone(),
                    notice: None,
                })
                .collect::<Vec<_>>()
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
        move |method: &str, _: Value, _: Duration| match method {
            "remote_apps_states" => Ok(json!([Observation {
                application: handle(1),
                state: Some(*phase.lock().unwrap()),
                error: None,
                ending: None,
                notice: None,
            }])),
            "remote_apps_stop" => {
                stops.fetch_add(1, Ordering::SeqCst);
                Ok(json!(true))
            }
            other => panic!("{other}"),
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
