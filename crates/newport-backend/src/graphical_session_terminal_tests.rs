//! Real local shells consume the same validated leases used by terminal tasks.
use super::*;
use crate::{ssh::terminal_command_with_identity, terminal::display::Prepared as Display};
use std::os::unix::{fs::PermissionsExt, net::UnixListener};

#[tokio::test]
async fn owned_terminal_bus_is_explicit_retained_and_precedes_user_overrides() {
    for x11 in [false, true] {
        let home = tempfile::tempdir_in("/tmp").unwrap();
        let socket = home.path().join("wayland owned");
        let bus = home.path().join("bus");
        let authority = home.path().join("authority");
        let _display = UnixListener::bind(&socket).unwrap();
        let _bus = UnixListener::bind(&bus).unwrap();
        std::fs::write(&authority, "fixture").unwrap();
        let address = format!("unix:path={},guid={}", bus.display(), "a".repeat(32));
        let shell = home.path().join("login-shell");
        let child = home.path().join("child");
        std::fs::write(&child, r#"#!/bin/sh
printf '%s\n' "$DBUS_SESSION_BUS_ADDRESS" "${DBUS_STARTER_ADDRESS-unset}" "${DBUS_STARTER_BUS_TYPE-unset}" "${DBUS_SESSION_BUS_PID-unset}" "${DBUS_SESSION_BUS_WINDOWID-unset}" "$WAYLAND_DISPLAY" "${DISPLAY-unset}" "$XDG_SESSION_TYPE" "$XDG_RUNTIME_DIR" "$LANG" "$PATH" "${WAYLAND_SOCKET-unset}"
"#).unwrap();
        // Model the login profile and a literal command override in actual
        // child processes, after Newport's one-time initialization.
        std::fs::write(&shell, r#"#!/bin/sh
test "$1" = -l || exit 9
if [ "$OVERRIDE" = profile ]; then export DBUS_SESSION_BUS_ADDRESS=unix:path=/profile/bus; fi
if [ "$OVERRIDE" = command ]; then exec env DBUS_SESSION_BUS_ADDRESS=unix:path=/command/bus "$HOME/child"; fi
exec "$HOME/child"
"#).unwrap();
        for path in [&shell, &child] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let slot = Slot::default();
        let fixture = Fixture::new(&slot);
        let prepared = fixture.clone();
        let moved_socket = socket.to_str().unwrap().to_owned();
        let moved_authority = authority.to_str().unwrap().to_owned();
        let moved_address = address.clone();
        let lease = slot
            .acquire_policy(
                fixture.runtime.clone(),
                terminal_bus::Policy::OwnedNative,
                Demand::Terminal,
                move |generation| async move {
                    let mut prepared = prepared.prepared_x11(generation, x11);
                    let environment = &mut prepared.attached.environment;
                    environment.wayland_socket = moved_socket;
                    if x11 {
                        environment.authority = Some(moved_authority);
                    }
                    environment.fallback_bus = Some(moved_address);
                    prepared.attached.terminal_bus = terminal_bus::Attachment::owned(environment)?;
                    Ok(prepared)
                },
            )
            .await
            .unwrap();
        assert!(
            slot.acquire(fixture.runtime.clone(), Demand::Terminal, |_| async {
                panic!("must not attach another policy")
            })
            .await
            .err()
            .unwrap()
            .contains("terminal bus policy")
        );
        let display = Display::Shared(lease);
        let command = terminal_command_with_identity(fixture.session, &display, None).unwrap();
        let run = |override_kind: &str| {
            std::process::Command::new("sh")
                .args(["-c", &command])
                .env("HOME", home.path())
                .env("SHELL", &shell)
                .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/ambient/bus")
                .env("DBUS_STARTER_ADDRESS", "unix:path=/stale/bus")
                .env("DBUS_STARTER_BUS_TYPE", "session")
                .env("DBUS_SESSION_BUS_PID", "123")
                .env("DBUS_SESSION_BUS_WINDOWID", "456")
                .env("WAYLAND_SOCKET", "99")
                .env("XDG_RUNTIME_DIR", "/user/runtime")
                .env("LANG", "C")
                .env("PATH", "/usr/bin:/bin")
                .env("OVERRIDE", override_kind)
                .output()
                .unwrap()
        };
        for (kind, expected) in [
            ("none", address.as_str()),
            ("profile", "unix:path=/profile/bus"),
            ("command", "unix:path=/command/bus"),
        ] {
            let output = run(kind);
            assert!(output.status.success(), "{output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            assert_eq!(
                text.lines().collect::<Vec<_>>(),
                [
                    expected,
                    "unset",
                    "unset",
                    "unset",
                    "unset",
                    socket.to_str().unwrap(),
                    if x11 { ":77" } else { "unset" },
                    "wayland",
                    "/user/runtime",
                    "C",
                    "/usr/bin:/bin",
                    "unset"
                ]
            );
        }
        // A missing bus leaves an ordinary shell with its own environment.
        std::fs::remove_file(&bus).unwrap();
        let ordinary = run("none");
        assert!(ordinary.status.success(), "{ordinary:?}");
        let text = String::from_utf8(ordinary.stdout).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(
            lines[..5],
            [
                "unix:path=/ambient/bus",
                "unix:path=/stale/bus",
                "session",
                "123",
                "456"
            ]
        );
        assert_ne!(lines[5], socket.to_str().unwrap());
        assert_eq!(lines[11], "99");
        fixture.valid.store(false, Ordering::SeqCst);
        assert!(terminal_command_with_identity(fixture.session, &display, None).is_err());
    }
}

#[tokio::test]
async fn owned_terminal_bus_rejects_unacknowledged_or_changed_attachment() {
    for mismatch in [false, true] {
        let slot = Slot::default();
        let fixture = Fixture::new(&slot);
        let runtime = fixture.runtime.clone();
        let result = slot
            .acquire_policy(
                runtime,
                terminal_bus::Policy::OwnedNative,
                Demand::Terminal,
                move |generation| async move {
                    let mut prepared = fixture.prepared(generation);
                    prepared.attached.environment.fallback_bus =
                        Some("unix:path=/tmp/owned/bus".into());
                    if mismatch {
                        prepared.attached.terminal_bus =
                            terminal_bus::Attachment::owned(&prepared.attached.environment)?;
                        prepared.attached.environment.fallback_bus =
                            Some("unix:path=/tmp/different/bus".into());
                    }
                    Ok(prepared)
                },
            )
            .await;
        assert!(
            result
                .err()
                .unwrap()
                .contains("selected terminal bus policy")
        );
    }
}

#[tokio::test]
async fn native_terminal_clears_inherited_x11_and_checks_owned_sockets_before_ready() {
    let home = tempfile::tempdir_in("/tmp").unwrap();
    let socket = home.path().join("wayland socket");
    let bus = home.path().join("bus");
    let _listener = UnixListener::bind(&socket).unwrap();
    let _bus = UnixListener::bind(&bus).unwrap();
    let shell = home.path().join("login-shell");
    std::fs::write(&shell, r#"#!/bin/sh
printf '%s\n' "${DISPLAY-unset}" "${XAUTHORITY-unset}" "${NEWPORT_GUI_DISPLAY-unset}" "${NEWPORT_GUI_AUTHORITY-unset}" "$WAYLAND_DISPLAY" "$NEWPORT_GUI_WAYLAND_DISPLAY" "${WAYLAND_SOCKET-unset}" "$XDG_RUNTIME_DIR" "$DBUS_SESSION_BUS_ADDRESS"
"#).unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    let bin = home.path().join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let agent = bin.join("newport-agent");
    // Simulate a display disappearing during agent environment refresh,
    // after the initial check but before the readiness acknowledgment.
    std::fs::write(&agent, "#!/bin/sh\nif [ -n \"${REMOVE_ENDPOINT-}\" ]; then /bin/rm -f -- \"$REMOVE_ENDPOINT\"; fi\n").unwrap();
    std::fs::set_permissions(agent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let prepared = fixture.clone();
    let moved_socket = socket.to_str().unwrap().to_owned();
    let address = format!("unix:path={}", bus.display());
    let moved_bus = address.clone();
    let lease = slot
        .acquire(
            fixture.runtime.clone(),
            Demand::Terminal,
            move |generation| async move {
                let mut prepared = prepared.prepared(generation);
                let environment = &mut prepared.attached.environment;
                environment.display = None;
                environment.authority = None;
                environment.wayland_socket = moved_socket;
                environment.fallback_bus = Some(moved_bus);
                Ok(prepared)
            },
        )
        .await
        .unwrap();
    let display = Display::Shared(lease);
    let normal = terminal_command_with_identity(fixture.session, &display, None).unwrap();
    let checked = crate::ssh::checked_terminal_command(fixture.session, &display).unwrap();
    let run = |command: &str, remove: &str| {
        std::process::Command::new("sh")
            .args(["-c", command])
            .env("HOME", home.path())
            .env("SHELL", &shell)
            .env("DISPLAY", ":999")
            .env("XAUTHORITY", "/foreign/authority")
            .env("NEWPORT_GUI_DISPLAY", ":999")
            .env("NEWPORT_GUI_AUTHORITY", "/foreign/authority")
            .env("WAYLAND_SOCKET", "99")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("NEWPORT_GUI_WAYLAND_DISPLAY")
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("DBUS_STARTER_ADDRESS")
            .env("REMOVE_ENDPOINT", remove)
            .output()
            .unwrap()
    };
    let result = run(&normal, "");
    assert!(result.status.success(), "{result:?}");
    let expected = format!(
        "unset\nunset\nunset\nunset\n{0}\n{0}\nunset\n{1}\n{address}\n",
        socket.display(),
        home.path().display()
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), expected);
    let result = run(&checked, "");
    assert!(result.status.success(), "{result:?}");
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.starts_with("\u{1e}NEWPORT_GUI_READY:"));
    assert_eq!(output.split_once('\u{1f}').unwrap().1, expected);
    // A failed check starts the login shell with the environment it had,
    // and the acknowledgment says the display is unavailable.
    let ordinary = ":999\n/foreign/authority\n:999\n/foreign/authority\n\n\n99\n\n\n";
    let unavailable = |output: Vec<u8>| {
        let output = String::from_utf8(output).unwrap();
        assert!(output.starts_with("\u{1e}NEWPORT_GUI_UNAVAILABLE:"));
        assert_eq!(output.split_once('\u{1f}').unwrap().1, ordinary);
    };
    for endpoint in [&bus, &socket] {
        let result = run(&checked, endpoint.to_str().unwrap());
        assert!(result.status.success(), "{result:?}");
        unavailable(result.stdout);
        let _replacement = UnixListener::bind(endpoint).unwrap();
    }
    std::fs::remove_file(&socket).unwrap();
    let result = run(&normal, "");
    assert!(result.status.success(), "{result:?}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), ordinary);
    let result = run(&checked, "");
    assert!(result.status.success(), "{result:?}");
    unavailable(result.stdout);
}

#[tokio::test]
async fn terminal_gets_the_display_application_policy_and_keeps_the_user_bus() {
    let home = tempfile::tempdir_in("/tmp").unwrap();
    let socket = home.path().join("wayland socket");
    let bus = home.path().join("bus");
    let _listener = UnixListener::bind(&socket).unwrap();
    let _bus = UnixListener::bind(&bus).unwrap();
    let shell = home.path().join("login-shell");
    // The login profile runs after Newport's assignments and stays final.
    std::fs::write(&shell, r#"#!/bin/sh
test "$1" = -l || exit 9
if [ -n "${PROFILE-}" ]; then export XDG_DATA_DIRS="$XDG_DATA_DIRS:/profile/share"; fi
printf '%s\n' "$XDG_CURRENT_DESKTOP" "$XDG_DATA_DIRS" "$MOZ_ENABLE_WAYLAND" "${ELECTRON_OZONE_PLATFORM_HINT-unset}" "$DBUS_SESSION_BUS_ADDRESS" "$XDG_SESSION_TYPE"
"#).unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let prepared = fixture.clone();
    let moved_socket = socket.to_str().unwrap().to_owned();
    let address = format!("unix:path={}", bus.display());
    let lease = slot
        .acquire(
            fixture.runtime.clone(),
            Demand::Terminal,
            move |generation| async move {
                let mut prepared = prepared.prepared_x11(generation, false);
                let environment = &mut prepared.attached.environment;
                environment.wayland_socket = moved_socket;
                environment.fallback_bus = Some(address);
                environment.application = [
                    ("XDG_CURRENT_DESKTOP".into(), Some("X-Newport".into())),
                    (
                        "XDG_DATA_DIRS".into(),
                        Some("/usr/share:/var/lib/snapd/desktop".into()),
                    ),
                    ("MOZ_ENABLE_WAYLAND".into(), Some("1".into())),
                    ("ELECTRON_OZONE_PLATFORM_HINT".into(), None),
                ]
                .into();
                Ok(prepared)
            },
        )
        .await
        .unwrap();
    let display = Display::Shared(lease);
    let command = terminal_command_with_identity(fixture.session, &display, None).unwrap();
    for profile in [false, true] {
        let mut run = std::process::Command::new("sh");
        run.args(["-c", &command])
            .env("HOME", home.path())
            .env("SHELL", &shell)
            .env("XDG_CURRENT_DESKTOP", "GNOME")
            .env("XDG_DATA_DIRS", "/stale")
            .env("ELECTRON_OZONE_PLATFORM_HINT", "x11")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/user/bus");
        if profile {
            run.env("PROFILE", "1");
        }
        let output = run.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let data_dirs = if profile {
            "/usr/share:/var/lib/snapd/desktop:/profile/share"
        } else {
            "/usr/share:/var/lib/snapd/desktop"
        };
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            [
                "X-Newport",
                data_dirs,
                "1",
                "unset",
                "unix:path=/user/bus",
                "wayland"
            ]
        );
    }
}

#[tokio::test]
async fn shared_terminal_shells_keep_both_endpoints_and_release_only_their_demand() {
    let home = tempfile::tempdir_in("/tmp").unwrap();
    let authority = home.path().join("authority ' $(touch INJECTED)");
    let socket = home.path().join("wayland ' socket");
    let bus = home.path().join("bus");
    let _bus_listener = UnixListener::bind(&bus).unwrap();
    let bus_address = format!("unix:path={}", bus.display());
    std::fs::write(&authority, "fixture").unwrap();
    let _listener = UnixListener::bind(&socket).unwrap();
    let shell = home.path().join("login-shell");
    std::fs::write(&shell, r#"#!/bin/sh
printf '%s\n' "$DISPLAY" "$XAUTHORITY" "$WAYLAND_DISPLAY" "$NEWPORT_GUI_DISPLAY" "$NEWPORT_GUI_AUTHORITY" "$NEWPORT_GUI_WAYLAND_DISPLAY" "${WAYLAND_SOCKET-unset}" "${NEWPORT_GUI_RUNTIME_DIR-unset}" "$DBUS_SESSION_BUS_ADDRESS" "$XDG_RUNTIME_DIR" "$NEWPORT_SESSION" "$NEWPORT_TERMINAL" "$XDG_SESSION_TYPE" "$1"
"#).unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let prepared = fixture.clone();
    let expected_authority = authority.to_str().unwrap().to_owned();
    let expected_socket = socket.to_str().unwrap().to_owned();
    let moved_authority = expected_authority.clone();
    let moved_socket = expected_socket.clone();
    let moved_bus = bus_address.clone();
    let first = slot
        .acquire(
            fixture.runtime.clone(),
            Demand::Terminal,
            move |generation| async move {
                let mut prepared = prepared.prepared(generation);
                prepared.attached.environment.authority = Some(moved_authority);
                prepared.attached.environment.wayland_socket = moved_socket;
                prepared.attached.environment.fallback_bus = Some(moved_bus);
                Ok(prepared)
            },
        )
        .await
        .unwrap();
    let second = fixture.acquire(&slot, Demand::Terminal).await;
    assert_eq!(first.identity().unwrap(), second.identity().unwrap());
    assert_eq!(slot.snapshot()[0].terminals, 2);
    assert_eq!(slot.snapshot()[0].windows, 0);
    assert_eq!(slot.snapshot()[0].applications, 0);
    let first = Display::Shared(first);
    let second = Display::Shared(second);
    let terminal = Uuid::new_v4();
    let command = terminal_command_with_identity(
        fixture.session,
        &first,
        Some(crate::terminal::shell::Identity::new(terminal)),
    )
    .unwrap();
    let run = |runtime: Option<&str>, bus: Option<&str>, starter: Option<&str>| {
        let mut process = std::process::Command::new("sh");
        process
            .args(["-c", &command])
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("SHELL", &shell)
            .env("WAYLAND_SOCKET", "99")
            .env("NEWPORT_GUI_RUNTIME_DIR", "/old/runtime")
            .env("XDG_SESSION_TYPE", "tty")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("DBUS_STARTER_ADDRESS");
        if let Some(bus) = bus {
            process.env("DBUS_SESSION_BUS_ADDRESS", bus);
        }
        if let Some(starter) = starter {
            process.env("DBUS_STARTER_ADDRESS", starter);
        }
        match runtime {
            Some(value) => process.env("XDG_RUNTIME_DIR", value),
            None => process.env_remove("XDG_RUNTIME_DIR"),
        };
        process.output().unwrap()
    };
    let result = run(Some("/user/runtime"), Some("unix:path=/user/bus"), None);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let values = String::from_utf8(result.stdout).unwrap();
    assert_eq!(
        values.lines().collect::<Vec<_>>(),
        [
            ":77",
            &expected_authority,
            &expected_socket,
            ":77",
            &expected_authority,
            &expected_socket,
            "unset",
            "unset",
            "unix:path=/user/bus",
            "/user/runtime",
            &fixture.session.to_string(),
            &terminal.to_string(),
            "wayland",
            "-l",
        ]
    );
    assert!(!home.path().join("INJECTED").exists());
    for runtime in [None, Some("")] {
        let result = run(runtime, Some("unix:path=/user/bus"), None);
        assert!(result.status.success());
        let values = String::from_utf8(result.stdout).unwrap();
        let values = values.lines().collect::<Vec<_>>();
        assert_eq!(values[2], expected_socket);
        assert_eq!(values[8], "unix:path=/user/bus");
        assert_eq!(values[9], home.path().to_str().unwrap());
        assert_eq!(values[10], fixture.session.to_string());
    }
    for inherited in [None, Some("")] {
        let result = run(None, inherited, None);
        assert!(result.status.success(), "{result:?}");
        let values = String::from_utf8(result.stdout).unwrap();
        assert_eq!(values.lines().nth(8), Some(bus_address.as_str()));
    }
    // An explicit activation bus is also user-owned environment, even when
    // DBUS_SESSION_BUS_ADDRESS is absent. Do not silently replace it.
    let result = run(None, None, Some("unix:path=/starter/bus"));
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().lines().nth(8),
        Some("")
    );
    // A missing endpoint leaves an ordinary shell, never a failed one.
    let ordinary = |result: std::process::Output| {
        assert!(result.status.success(), "{result:?}");
        let values = String::from_utf8(result.stdout).unwrap();
        let values = values.lines().map(str::to_owned).collect::<Vec<_>>();
        assert_ne!(values[2], expected_socket);
        assert_eq!(values[10], fixture.session.to_string());
        assert_eq!(values[12..], ["tty", "-l"]);
        values
    };
    std::fs::remove_file(&bus).unwrap();
    assert_eq!(ordinary(run(None, None, None))[8], "");
    // A shell with its own bus does not need the display's.
    let result = run(None, Some("unix:path=/user/bus"), None);
    assert!(result.status.success(), "{result:?}");
    let values = String::from_utf8(result.stdout).unwrap();
    let values = values.lines().collect::<Vec<_>>();
    assert_eq!(values[2], expected_socket);
    assert_eq!(values[8], "unix:path=/user/bus");
    assert!(
        terminal_command_with_identity(
            Uuid::new_v4(),
            &first,
            Some(crate::terminal::shell::Identity::new(terminal))
        )
        .is_err()
    );
    // A disappearing endpoint between local validation and remote exec must
    // not start a GUI-enabled shell with stale or clipboard-only endpoints.
    std::fs::remove_file(&socket).unwrap();
    ordinary(run(None, Some("unix:path=/user/bus"), None));
    let _listener = UnixListener::bind(&socket).unwrap();
    std::fs::remove_file(&authority).unwrap();
    ordinary(run(None, Some("unix:path=/user/bus"), None));
    drop(first);
    assert_eq!(slot.snapshot()[0].terminals, 1);
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
    fixture.valid.store(false, Ordering::SeqCst);
    assert!(
        terminal_command_with_identity(
            fixture.session,
            &second,
            Some(crate::terminal::shell::Identity::new(terminal))
        )
        .is_err()
    );
    drop(second);
    eventually(|| fixture.driver.stops.load(Ordering::SeqCst) > 0).await;
}

#[tokio::test]
async fn approved_shell_retains_one_display_across_requests_and_reconnects() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let registry = crate::terminal::shell::Registry::default();
    let id = Uuid::new_v4();
    let shell = registry.register(id).unwrap();
    shell.initialize(Arc::new(Display::None)).unwrap();
    let first = shell.begin().unwrap();
    let first_epoch = first.identity.epoch;
    let approval = registry.capture(id, Some(first_epoch)).unwrap();
    let first_lease = fixture.acquire(&slot, Demand::Terminal).await;
    let environment = first_lease.environment().unwrap();
    let concurrent = fixture.acquire(&slot, Demand::Terminal).await;
    assert_eq!(slot.snapshot()[0].terminals, 2);
    assert_eq!(
        approval
            .retain(Display::Shared(first_lease), fixture.session)
            .unwrap(),
        environment
    );
    assert_eq!(
        approval
            .retain(Display::Shared(concurrent), fixture.session)
            .unwrap(),
        environment
    );
    assert_eq!(
        slot.snapshot()[0].terminals,
        1,
        "concurrent approvals must not retain duplicate demand"
    );
    drop(first);
    assert!(approval.validate().is_err());
    let second = shell.begin().unwrap();
    assert_ne!(second.identity.epoch, first_epoch);
    // Reconnection takes the retained environment; it does not create a display.
    let retained = shell.display().unwrap().unwrap();
    assert_eq!(
        retained.assignments(fixture.session).unwrap(),
        environment.shell_assignments().unwrap()
    );
    drop(retained);
    fixture.driver.publish(3, 3);
    shell.close();
    assert!(shell.display().is_err());
    assert_eq!(slot.snapshot()[0].terminals, 0);
    assert_eq!(slot.snapshot()[0].windows, 3);
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 0);
    fixture.driver.publish(0, 0);
    slot.drained().await;
    assert_eq!(fixture.driver.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_approval_releases_candidate_without_attaching_to_replacement_shell() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let clipboard = fixture.acquire(&slot, Demand::Clipboard).await;
    let registry = crate::terminal::shell::Registry::default();
    let id = Uuid::new_v4();
    let shell = registry.register(id).unwrap();
    shell.initialize(Arc::new(Display::None)).unwrap();
    let first = shell.begin().unwrap();
    let approval = registry.capture(id, Some(first.identity.epoch)).unwrap();
    let candidate = fixture.acquire(&slot, Demand::Terminal).await;
    drop(first);
    let second = shell.begin().unwrap();
    assert!(
        approval
            .retain(Display::Shared(candidate), fixture.session)
            .is_err()
    );
    assert_eq!(slot.snapshot()[0].terminals, 0);
    assert!(matches!(
        shell.display().unwrap().as_deref(),
        Some(Display::None)
    ));
    let current = registry.capture(id, Some(second.identity.epoch)).unwrap();
    let candidate = fixture.acquire(&slot, Demand::Terminal).await;
    shell.close();
    assert!(
        current
            .retain(Display::Shared(candidate), fixture.session)
            .is_err()
    );
    assert_eq!(slot.snapshot()[0].terminals, 0);
    assert!(clipboard.environment().is_ok());
    drop(clipboard);
    slot.drained().await;
}

#[tokio::test]
async fn approval_cannot_replace_a_retained_display_even_with_identical_endpoints() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let registry = crate::terminal::shell::Registry::default();
    let id = Uuid::new_v4();
    let shell = registry.register(id).unwrap();
    let attempt = shell.begin().unwrap();
    let approval = registry.capture(id, Some(attempt.identity.epoch)).unwrap();
    let first = fixture.acquire(&slot, Demand::Terminal).await;
    let expected = first.identity().unwrap();
    approval
        .retain(Display::Shared(first), fixture.session)
        .unwrap();
    let other_slot = Slot::default();
    let mut other = Fixture::new(&other_slot);
    other.session = fixture.session;
    let candidate = other.acquire(&other_slot, Demand::Terminal).await;
    assert!(
        approval
            .retain(Display::Shared(candidate), fixture.session)
            .is_err()
    );
    other_slot.drained().await;
    let retained = shell.display().unwrap().unwrap();
    let Display::Shared(lease) = retained.as_ref() else {
        panic!()
    };
    assert_eq!(lease.identity().unwrap(), expected);
    assert_eq!(slot.snapshot()[0].terminals, 1);
    drop(retained);
    shell.close();
    slot.drained().await;
}

#[tokio::test]
async fn shell_observes_a_late_handoff_failure_without_revoking_ssh_identity() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let registry = crate::terminal::shell::Registry::default();
    let id = Uuid::new_v4();
    let shell = registry.register(id).unwrap();
    shell.initialize(Arc::new(Display::None)).unwrap();
    let attempt = shell.begin().unwrap();
    let approval = registry.capture(id, Some(attempt.identity.epoch)).unwrap();
    let watched = shell.clone();
    let notice = tokio::spawn(async move { watched.gui_failure().await });
    tokio::task::yield_now().await;
    assert!(
        !notice.is_finished(),
        "an ordinary shell has no GUI failure"
    );
    let lease = fixture.acquire(&slot, Demand::Terminal).await;
    approval
        .retain(Display::Shared(lease), fixture.session)
        .unwrap();
    fixture.driver.publish(3, 3);
    changed_until(&slot, |state| state[0].windows == 3).await;
    assert!(
        !notice.is_finished(),
        "window updates must not become failures"
    );
    fixture
        .driver
        .updates
        .send_replace(DriverState::Failed("Display worker exited".into()));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), notice)
            .await
            .unwrap()
            .unwrap(),
        "Display worker exited"
    );
    approval.validate().unwrap();
    assert!(
        shell
            .display()
            .unwrap()
            .unwrap()
            .assignments(fixture.session)
            .is_err()
    );
    let replacement = Fixture::new(&slot);
    let candidate = replacement.acquire(&slot, Demand::Terminal).await;
    assert!(
        approval
            .retain(Display::Shared(candidate), replacement.session)
            .is_err()
    );
    approval.validate().unwrap();
    shell.close();
}

#[tokio::test]
async fn gui_failure_watch_does_not_keep_a_retired_owner_alive() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let lease = fixture.acquire(&slot, Demand::Terminal).await;
    let notice = tokio::spawn(async move { lease.unavailable().await });
    tokio::task::yield_now().await;
    assert!(!notice.is_finished());
    drop(slot);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), notice)
            .await
            .unwrap()
            .is_ok()
    );
}

fn display_owner(
    port: u16,
) -> (
    crate::server_session::Registry,
    Arc<crate::server_session::ServerSession>,
) {
    let server: crate::model::Server = serde_json::from_value(serde_json::json!({
        "id":Uuid::new_v4(),"name":"Implicit display","sshHost":"127.0.0.1",
        "sshUser":"fixture","sshPort":port
    }))
    .unwrap();
    let registry = crate::server_session::Registry::default();
    let owner = registry.session(&server, 0);
    (registry, owner)
}
/// A display running for a server session, as Remote apps or an earlier GUI
/// launch leaves it. Its endpoints are never created on disk.
async fn running_display(owner: &crate::server_session::ServerSession) -> (Fixture, Lease) {
    let mut fixture = Fixture::new(&owner.graphical);
    fixture.session = owner.integration_session_id();
    let held = fixture.acquire(&owner.graphical, Demand::Clipboard).await;
    assert_eq!(owner.graphical_sessions()[0].phase, Phase::Ready);
    (fixture, held)
}
type Asked = Arc<AtomicUsize>;
fn source(
    asked: &Asked,
    answer: impl Fn() -> crate::graphical_session::FactoryFuture + Send + Sync + 'static,
) -> crate::terminal::display::Source {
    let asked = asked.clone();
    Arc::new(move || {
        asked.fetch_add(1, Ordering::SeqCst);
        answer()
    })
}
fn never() -> crate::graphical_session::FactoryFuture {
    Box::pin(std::future::pending())
}
/// What a source refuses with when the agent, account, update, channel limit
/// or display start gets in the way.
const REFUSALS: [&str; 6] = [
    "Needle worker is not registered",
    "This computer has no Remote apps access",
    "Needle is updating; try again when it finishes",
    "SSH channel refused: the server's MaxSessions limit is reached",
    "Remote apps worker exited before the display was ready",
    "Remote apps isn’t set up for this server",
];
struct Granted;
impl Factory for Granted {
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }
    fn attach(&self, _: transport::Channel, _: Identity) -> AttachFuture {
        panic!("joining a running display never starts another")
    }
}

#[tokio::test(start_paused = true)]
async fn a_new_shell_waits_only_briefly_and_only_for_a_running_display() {
    use crate::terminal::display::{JOIN_WAIT, join};
    // Accepts SSH but never answers, like a stalled server.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (registry, owner) = display_owner(silent.local_addr().unwrap().port());
    let asked = Asked::default();
    let started = tokio::time::Instant::now();
    assert!(matches!(
        join(&owner, source(&asked, never), JOIN_WAIT).await,
        Display::None
    ));
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "no display: nothing to ask"
    );

    let (_fixture, held) = running_display(&owner).await;
    // A source that never answers, and one whose display acquisition stalls
    // on the server, cost the shell at most JOIN_WAIT.
    for answer in [
        never as fn() -> crate::graphical_session::FactoryFuture,
        || Box::pin(async { Ok(Arc::new(Granted) as Arc<dyn Factory>) }),
    ] {
        let started = tokio::time::Instant::now();
        assert!(matches!(
            join(&owner, source(&asked, answer), JOIN_WAIT).await,
            Display::None
        ));
        assert_eq!(started.elapsed(), JOIN_WAIT);
    }
    for refusal in REFUSALS {
        let started = tokio::time::Instant::now();
        let refused = source(&asked, move || Box::pin(async move { Err(refusal.into()) }));
        assert!(matches!(
            join(&owner, refused, JOIN_WAIT).await,
            Display::None
        ));
        assert_eq!(started.elapsed(), Duration::ZERO, "{refusal}");
    }
    assert_eq!(asked.load(Ordering::SeqCst), 2 + REFUSALS.len());
    assert_eq!(
        owner.graphical_sessions()[0].terminals,
        0,
        "nothing retained"
    );
    drop(held);
    registry.shutdown().await;
}

#[tokio::test]
async fn a_display_that_failed_a_shell_is_forgotten_and_no_other() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let registry = crate::terminal::shell::Registry::default();
    let shell = registry.register(Uuid::new_v4()).unwrap();
    let failed = Arc::new(Display::Shared(
        fixture.acquire(&slot, Demand::Terminal).await,
    ));
    shell.initialize(failed.clone()).unwrap();
    shell.forget_display(&Arc::new(Display::None));
    assert!(Arc::ptr_eq(&shell.display().unwrap().unwrap(), &failed));
    shell.forget_display(&failed);
    assert!(matches!(
        shell.display().unwrap().as_deref(),
        Some(Display::None)
    ));
    drop(failed);
    assert!(slot.snapshot().iter().all(|display| display.terminals == 0));
    shell.close();
    slot.drained().await;
}

fn ssh_fixture() -> crate::model::Server {
    crate::model::Server {
        id: Uuid::new_v4(),
        name: "Terminal fixture".into(),
        ssh_user: "fixture".into(),
        ssh_host: "127.0.0.1".into(),
        ssh_port: std::env::var("NEWPORT_TEST_SSH_PORT")
            .unwrap()
            .parse()
            .unwrap(),
        identity_file: Some(std::env::var("NEWPORT_TEST_IDENTITY").unwrap()),
        agent_source: None,
        agent_key_fingerprint: None,
        auth_method: crate::model::AuthMethod::PublicKey,
        clipboard_enabled: false,
        browser_enabled: false,
        agent_install_approved: true,
        setup_complete: true,
        proxy: None,
    }
}
/// Reads until `needle`; any error, exit or reconnect fails the test.
async fn shell_output(io: &mut crate::terminal::Io, needle: &str) -> String {
    use crate::terminal::Event;
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut text = String::new();
        loop {
            match io.output.recv().await {
                Some(Event::Data(bytes)) => {
                    text.push_str(&String::from_utf8_lossy(&bytes));
                    if text.contains(needle) {
                        return text;
                    }
                }
                Some(Event::Ready) => {}
                event => panic!("Unexpected {event:?}: {text}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("missing {needle}"))
}
/// Runs a command once the shell prompts; `started` skips that wait for a
/// shell already shown to be running.
async fn usable(io: &mut crate::terminal::Io, marker: &str, started: bool) -> String {
    if !started {
        shell_output(io, "fixture> ").await;
    }
    io.input
        .send(crate::terminal::Input::Data(
            // The echoed command line never contains "{marker}=".
            format!("printf '%s=%s\\n' {marker} \"${{WAYLAND_DISPLAY-none}}\"\r").into_bytes(),
        ))
        .await
        .unwrap();
    shell_output(io, &format!("{marker}=")).await
}

#[tokio::test]
#[ignore = "Run with cargo xtask test ssh; runs a real Bash PTY in an isolated HOME"]
async fn the_shell_is_ready_and_usable_whatever_its_display_does() {
    use crate::terminal::display::Display as Startup;
    let server = ssh_fixture();
    let registry = crate::server_session::Registry::default();
    let owner = registry.session(&server, 0);
    let sessions = crate::terminal::Sessions::default();
    let (fixture, held) = running_display(&owner).await;
    let asked = Asked::default();
    let mut answers: Vec<Box<dyn Fn() -> crate::graphical_session::FactoryFuture + Send + Sync>> =
        vec![Box::new(never)];
    for refusal in REFUSALS {
        answers.push(Box::new(move || {
            Box::pin(async move { Err(refusal.into()) })
        }));
    }
    // GUI preparation that never answers or refuses: the shell starts plain,
    // with no error, and a later GUI launch is refused for that program only.
    for (index, answer) in answers.into_iter().enumerate() {
        let id = Uuid::new_v4();
        let started = std::time::Instant::now();
        let mut io = sessions
            .open_bound_with_handoff(
                id,
                server.clone(),
                (80, 24),
                owner.clone(),
                Startup::Implicit(source(&asked, answer)),
                Some(source(&asked, || {
                    Box::pin(async { Err(REFUSALS[0].into()) })
                })),
            )
            .unwrap();
        let text = usable(&mut io, &format!("plain{index}"), false).await;
        assert!(text.contains(&format!("plain{index}=none")), "{text}");
        assert!(started.elapsed() < Duration::from_secs(8));
        sessions.close(id).await;
    }
    // A running display whose sockets fail the shell's startup check: the
    // shell starts without it and releases its demand.
    let id = Uuid::new_v4();
    let lease = fixture.acquire(&owner.graphical, Demand::Terminal).await;
    assert_eq!(owner.graphical_sessions()[0].terminals, 1);
    let mut io = sessions
        .open_bound_with_display(
            id,
            server.clone(),
            80,
            24,
            owner.clone(),
            Startup::Ready(Display::Shared(lease)),
        )
        .unwrap();
    let text = usable(&mut io, "fallback", false).await;
    assert!(text.contains("fallback=none"), "{text}");
    assert_eq!(owner.graphical_sessions()[0].terminals, 0);
    // A display adopted after the shell started, then lost: the shell keeps
    // running and accepts input, with no event besides its output.
    let approval = owner.terminal_shells.capture(id, None).unwrap();
    approval
        .retain(
            Display::Shared(fixture.acquire(&owner.graphical, Demand::Terminal).await),
            owner.integration_session_id(),
        )
        .unwrap();
    fixture
        .driver
        .updates
        .send_replace(DriverState::Failed("Display worker exited".into()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let text = usable(&mut io, "after", true).await;
    assert!(text.contains("after=none"), "{text}");
    sessions.close(id).await;
    drop(held);
    sessions.close_server(None).await;
    registry.shutdown().await;
}

#[tokio::test]
async fn a_shell_whose_display_was_ended_lets_it_go_for_the_next_launch() {
    let slot = Slot::default();
    let fixture = Fixture::new(&slot);
    let registry = crate::terminal::shell::Registry::default();
    let id = Uuid::new_v4();
    let shell = registry.register(id).unwrap();
    shell.initialize(Arc::new(Display::None)).unwrap();
    let attempt = shell.begin().unwrap();
    let approval = registry.capture(id, Some(attempt.identity.epoch)).unwrap();
    let lease = fixture.acquire(&slot, Demand::Terminal).await;
    let environment = approval
        .retain(Display::Shared(lease), fixture.session)
        .unwrap();
    assert_eq!(
        shell.held_environment(fixture.session).unwrap(),
        Some(environment)
    );
    // As when Needle is uninstalled with this terminal open.
    slot.end_all();
    assert_eq!(shell.held_environment(fixture.session).unwrap(), None);
    assert!(matches!(
        shell.display().unwrap().as_deref(),
        Some(Display::None)
    ));
    let mut reinstalled = Fixture::new(&slot);
    reinstalled.session = fixture.session;
    let replacement = reinstalled.acquire(&slot, Demand::Terminal).await;
    let current = replacement.environment().unwrap();
    assert_eq!(
        approval
            .retain(Display::Shared(replacement), fixture.session)
            .unwrap(),
        current
    );
    shell.close();
}
