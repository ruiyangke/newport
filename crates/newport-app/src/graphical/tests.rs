use super::*;

pub(super) fn identity() -> backend::Identity {
    backend::Identity {
        runtime: backend::Runtime {
            instance: "12345678-1234-1234-1234-123456789abc".parse().unwrap(),
            build_sha256: "a".repeat(64),
            transport_generation: 1,
        },
        session: "12345678-1234-1234-1234-123456789def".parse().unwrap(),
        worker: "12345678-1234-1234-1234-123456789fed".parse().unwrap(),
        display_generation: 1,
    }
}
#[test]
fn pending_launches_retain_demand_and_late_activity_cannot_revive_stopped_driver() {
    let publication = Publication {
        state: watch::channel(backend::DriverState::Closed).0,
        stopped: AtomicBool::new(false),
        ending: AtomicBool::new(false),
        presentation: watch::channel(()).0,
        clipboard: Default::default(),
    };
    let activity = newport_remote_client::session_activity::Activity {
        windows: 2,
        applications: 3,
        pending_launches: 1,
    };
    publication.publish(Ok(Some(activity)));
    assert_eq!(
        *publication.state.borrow(),
        backend::DriverState::Ready {
            windows: 2,
            applications: 4
        }
    );
    publication.stop();
    publication.publish(Ok(Some(activity)));
    assert_eq!(*publication.state.borrow(), backend::DriverState::Closed);
}

#[test]
fn failure_keeps_its_cause_and_cannot_be_revived_by_late_activity() {
    let publication = Publication {
        state: watch::channel(backend::DriverState::Closed).0,
        stopped: AtomicBool::new(false),
        ending: AtomicBool::new(false),
        presentation: watch::channel(()).0,
        clipboard: Default::default(),
    };
    let activity = newport_remote_client::session_activity::Activity::default();
    publication.publish(Ok(Some(activity)));
    let reason = "Wayland socket endpoint disappeared or is inaccessible";
    publication.publish(Err(reason.into()));
    for update in [
        Ok(None),
        Err("later transport error".into()),
        Ok(Some(activity)),
    ] {
        publication.publish(update);
        assert_eq!(
            *publication.state.borrow(),
            backend::DriverState::Failed(reason.into())
        );
    }
    publication.stop();
    publication.publish(Err(reason.into()));
    assert_eq!(*publication.state.borrow(), backend::DriverState::Closed);
}

#[test]
fn a_takeover_ends_the_display_as_shown_elsewhere_and_other_endings_as_before() {
    let publication = || Publication {
        state: watch::channel(backend::DriverState::Closed).0,
        stopped: AtomicBool::new(false),
        ending: AtomicBool::new(false),
        presentation: watch::channel(()).0,
        clipboard: Default::default(),
    };
    let activity = newport_remote_client::session_activity::Activity {
        windows: 1,
        applications: 1,
        pending_launches: 0,
    };
    let shown = backend::DriverState::ShownElsewhere(backend::launch::ShownOn {
        computer: Some("studio-mac".into()),
        session: Some("studio-session".into()),
    });
    let taken_over = || {
        SessionOutcome::TakenOver(newport_remote_client::Takeover {
            computer: Some("studio-mac".into()),
            session: Some("studio-session".into()),
        })
    };
    let display = publication();
    assert!(!display.publish(Ok(Some(activity))));
    // Nothing is left, and only the outcome says how it ended.
    assert!(display.publish(Ok(None)));
    assert!(matches!(
        *display.state.borrow(),
        backend::DriverState::Ready { .. }
    ));
    assert!(!display.publish(Ok(Some(activity))));
    assert!(matches!(
        *display.state.borrow(),
        backend::DriverState::Ready { .. }
    ));
    display.finish(Some(taken_over()));
    assert_eq!(*display.state.borrow(), shown);
    display.publish(Err("Remote output ended".into()));
    display.finish(Some(SessionOutcome::Failed("Remote output ended".into())));
    assert_eq!(*display.state.borrow(), shown);
    display.stop();
    assert_eq!(*display.state.borrow(), backend::DriverState::Closed);
    // Every other ending, or none in time, closes or fails as before.
    for (outcome, state) in [
        (None, backend::DriverState::Closed),
        (
            Some(SessionOutcome::RemoteClosed),
            backend::DriverState::Closed,
        ),
        (
            Some(SessionOutcome::Failed("boom".into())),
            backend::DriverState::Failed("boom".into()),
        ),
    ] {
        let display = publication();
        display.publish(Ok(Some(activity)));
        assert!(display.publish(Ok(None)));
        display.finish(outcome);
        assert_eq!(*display.state.borrow(), state);
    }
    // A stop wins, and a display that was never ready closes at once.
    let display = publication();
    display.publish(Ok(Some(activity)));
    assert!(display.publish(Ok(None)));
    display.stop();
    display.finish(Some(taken_over()));
    assert_eq!(*display.state.borrow(), backend::DriverState::Closed);
    let display = publication();
    assert!(!display.publish(Ok(None)));
    assert_eq!(*display.state.borrow(), backend::DriverState::Closed);
    // Taken over while it started, it says where it went.
    assert_eq!(
        startup_failure(Some(taken_over()), &Transport::default()),
        "Shown on studio-mac."
    );
}

#[test]
fn clipboard_attachment_requires_ready_and_stop_retires_presenter_handles() {
    use newport_backend::desktop_integration::{
        ClipboardAccess, ClipboardData, ClipboardFuture, ClipboardOffer,
    };
    struct Native;
    impl ClipboardAccess for Native {
        fn offer(&self, _: Option<isize>) -> ClipboardFuture<ClipboardOffer> {
            Box::pin(async { None })
        }
        fn read(&self, _: isize, _: String) -> ClipboardFuture<Box<dyn ClipboardData>> {
            Box::pin(async { None })
        }
    }
    for loss in 0..3 {
        let router = Router::new(Arc::new(Native));
        let publication = Publication {
            state: watch::channel(backend::DriverState::Closed).0,
            stopped: AtomicBool::new(false),
            ending: AtomicBool::new(false),
            presentation: watch::channel(()).0,
            clipboard: Default::default(),
        };
        assert!(publication.attach_clipboard(&router, identity()).is_err());
        assert!(publication.clipboard.get().is_none());
        let activity = newport_remote_client::session_activity::Activity {
            windows: 3,
            applications: 3,
            pending_launches: 0,
        };
        publication.publish(Ok(Some(activity)));
        publication.attach_clipboard(&router, identity()).unwrap();
        let display = publication.clipboard.get().unwrap();
        let presenter = display.presenter().unwrap();
        assert!(presenter.set_focused(true));
        match loss {
            0 => {
                publication.publish(Ok(None));
            }
            1 => publication.stop(),
            _ => {
                publication.publish(Err("Display endpoint disappeared".into()));
            }
        }
        assert!(!presenter.set_focused(true));
        assert!(display.presenter().is_none());
        publication.publish(Ok(Some(activity)));
        assert!(!presenter.set_focused(true));
        assert!(publication.attach_clipboard(&router, identity()).is_err());
    }
}
#[test]
fn invalid_license_is_rejected_before_a_factory_can_admit_remote_work() {
    let registry = Registry::default();
    let license = RemoteLicense {
        token: "not-a-license".to_owned().into(),
        machine: String::new(),
        trusted_time: 0,
        expires_at: 0,
    };
    assert!(
        registry
            .factory(license, Settings::default(), None)
            .is_err()
    );
    assert!(registry.0.clients.lock().unwrap().is_empty());
}
