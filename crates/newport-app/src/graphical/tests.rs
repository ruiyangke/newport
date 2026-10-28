use super::*;
use serde_json::json;

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
pub(super) fn environment(session: &str) -> SharedEnvironment {
    serde_json::from_value(json!({
        "DISPLAY": ":75", "XAUTHORITY": "/tmp/owned/Xauthority",
        "WAYLAND_DISPLAY": "wayland-owned", "XDG_RUNTIME_DIR": "/run/user/1000",
        "WAYLAND_SOCKET": null, "DBUS_STARTER_ADDRESS": null, "DBUS_STARTER_BUS_TYPE": null,
        "DBUS_SESSION_BUS_ADDRESS": "unix:path=/tmp/owned/bus",
        "NEWPORT_SESSION": session,
        "NEWPORT_GUI_DISPLAY": ":75", "NEWPORT_GUI_AUTHORITY": "/tmp/owned/Xauthority",
        "NEWPORT_GUI_WAYLAND_DISPLAY": "wayland-owned", "NEWPORT_GUI_RUNTIME_DIR": "/run/user/1000",
        "GTK_IM_MODULE": null
    }))
    .unwrap()
}
#[test]
fn shared_projection_preserves_user_bus_and_runtime_and_requires_exact_session() {
    let identity = identity();
    let env = environment(&identity.session.to_string());
    let projection = project_environment(&env, &identity, SharedDisplayMode::Mixed).unwrap();
    assert_eq!(projection.wayland_socket, "/run/user/1000/wayland-owned");
    assert_eq!(
        projection.fallback_bus.as_deref(),
        Some("unix:path=/tmp/owned/bus")
    );
    let assignments = projection.shell_assignments().unwrap();
    assert!(!assignments.contains_key("DBUS_SESSION_BUS_ADDRESS"));
    assert!(!assignments.contains_key("XDG_RUNTIME_DIR"));
    assert_eq!(assignments["GTK_IM_MODULE"], None);
    assert_eq!(assignments["NEWPORT_GUI_RUNTIME_DIR"], None);
    assert!(
        project_environment(
            &environment(&identity.worker.to_string()),
            &identity,
            SharedDisplayMode::Mixed
        )
        .is_err()
    );
    let mut malformed = env;
    malformed
        .0
        .insert("NEWPORT_GUI_DISPLAY".into(), Some(":76".into()));
    assert!(project_environment(&malformed, &identity, SharedDisplayMode::Mixed).is_err());
}
#[test]
fn application_policy_projects_apart_from_toolkit_settings() {
    let identity = identity();
    let mut env = environment(&identity.session.to_string());
    let plain = project_environment(&env, &identity, SharedDisplayMode::Mixed).unwrap();
    assert!(plain.application.is_empty());
    for (name, value) in [
        ("XDG_CURRENT_DESKTOP", Some("X-Newport")),
        ("XDG_DATA_DIRS", Some("/usr/share:/var/lib/snapd/desktop")),
        ("MOZ_ENABLE_WAYLAND", Some("1")),
        ("ELECTRON_OZONE_PLATFORM_HINT", None),
    ] {
        env.0.insert(name.into(), value.map(str::to_owned));
    }
    let projected = project_environment(&env, &identity, SharedDisplayMode::Mixed).unwrap();
    assert_eq!(projected.application.len(), 4);
    assert!(!projected.toolkit.contains_key("XDG_CURRENT_DESKTOP"));
    assert_eq!(projected.toolkit, plain.toolkit);
    let assignments = projected.shell_assignments().unwrap();
    assert_eq!(
        assignments["XDG_CURRENT_DESKTOP"].as_deref(),
        Some("X-Newport")
    );
    assert_eq!(assignments["ELECTRON_OZONE_PLATFORM_HINT"], None);
    assert!(!assignments.contains_key("DBUS_SESSION_BUS_ADDRESS"));
    env.0
        .insert("XDG_DATA_DIRS".into(), Some("relative".into()));
    assert!(project_environment(&env, &identity, SharedDisplayMode::Mixed).is_err());
}
#[test]
fn pending_launches_retain_demand_and_late_activity_cannot_revive_stopped_driver() {
    let publication = Publication {
        state: watch::channel(backend::DriverState::Closed).0,
        stopped: AtomicBool::new(false),
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
fn native_projection_requires_explicit_mode_and_clears_x11() {
    let identity = identity();
    let mut env = environment(&identity.session.to_string());
    assert!(project_environment(&env, &identity, SharedDisplayMode::Wayland).is_err());
    for name in [
        "DISPLAY",
        "XAUTHORITY",
        "NEWPORT_GUI_DISPLAY",
        "NEWPORT_GUI_AUTHORITY",
    ] {
        env.0.insert(name.into(), None);
    }
    let projected = project_environment(&env, &identity, SharedDisplayMode::Wayland).unwrap();
    assert!(projected.native_wayland());
    assert!(project_environment(&env, &identity, SharedDisplayMode::Mixed).is_err());
    let assignments = projected.shell_assignments().unwrap();
    assert_eq!(assignments["DISPLAY"], None);
    assert_eq!(assignments["XAUTHORITY"], None);
    env.0.remove("DISPLAY");
    assert!(project_environment(&env, &identity, SharedDisplayMode::Wayland).is_err());
}

#[test]
fn failure_keeps_its_cause_and_cannot_be_revived_by_late_activity() {
    let publication = Publication {
        state: watch::channel(backend::DriverState::Closed).0,
        stopped: AtomicBool::new(false),
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
            0 => publication.publish(Ok(None)),
            1 => publication.stop(),
            _ => publication.publish(Err("Display endpoint disappeared".into())),
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
    assert!(registry.factory(license, Settings::default()).is_err());
    assert!(registry.0.clients.lock().unwrap().is_empty());
}
