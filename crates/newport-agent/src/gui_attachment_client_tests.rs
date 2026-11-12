use super::*;
use std::os::unix::net::UnixListener;

fn values() -> BTreeMap<&'static str, OsString> {
    [
        ("DISPLAY", ":75"),
        ("NEWPORT_GUI_DISPLAY", ":75"),
        ("XAUTHORITY", "/tmp/owned/Xauthority"),
        ("NEWPORT_GUI_AUTHORITY", "/tmp/owned/Xauthority"),
        ("WAYLAND_DISPLAY", "wayland-0"),
        ("NEWPORT_GUI_WAYLAND_DISPLAY", "wayland-0"),
        ("XDG_RUNTIME_DIR", "/tmp/owned"),
        ("NEWPORT_GUI_RUNTIME_DIR", "/tmp/owned"),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.into()))
    .collect()
}
fn claim(values: &BTreeMap<&'static str, OsString>) -> Claim {
    Claim::capture(|name| values.get(name).cloned())
}
#[test]
fn exact_pairs_and_relative_wayland_are_not_reinterpreted() {
    let mut values = values();
    let snapshot = claim(&values);
    assert_eq!(
        snapshot.endpoints().unwrap().wayland_socket,
        "/tmp/owned/wayland-0"
    );
    for key in [
        "DISPLAY",
        "XAUTHORITY",
        "NEWPORT_GUI_DISPLAY",
        "NEWPORT_GUI_AUTHORITY",
        "WAYLAND_DISPLAY",
        "NEWPORT_GUI_WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "NEWPORT_GUI_RUNTIME_DIR",
    ] {
        let mut changed = values.clone();
        changed.insert(key, "/foreign/value".into());
        assert!(claim(&changed).endpoints().is_err(), "{key}");
    }
    for value in ["", "99"] {
        let mut changed = values.clone();
        changed.insert("WAYLAND_SOCKET", value.into());
        assert!(claim(&changed).endpoints().is_err());
    }
    for name in [
        "DISPLAY",
        "XAUTHORITY",
        "NEWPORT_GUI_DISPLAY",
        "NEWPORT_GUI_AUTHORITY",
    ] {
        values.remove(name);
    }
    let native = claim(&values).endpoints().unwrap();
    assert_eq!((native.display, native.authority), (None, None));
    values.insert("DISPLAY", OsString::new());
    assert!(claim(&values).endpoints().is_err(), "empty is not absent");
    values.remove("DISPLAY");
    values.insert("WAYLAND_DISPLAY", "/tmp/owned/socket".into());
    values.insert("NEWPORT_GUI_WAYLAND_DISPLAY", "/tmp/owned/socket".into());
    values.remove("XDG_RUNTIME_DIR");
    values.remove("NEWPORT_GUI_RUNTIME_DIR");
    assert_eq!(
        claim(&values).endpoints().unwrap().wayland_socket,
        "/tmp/owned/socket"
    );
}

#[test]
fn a_shell_on_the_display_names_its_endpoints_without_markers() {
    let mut values = values();
    for (_, marker) in shared_terminal_v1::MARKERS {
        values.remove(marker);
    }
    let endpoints = claim(&values).endpoints().unwrap();
    assert_eq!(endpoints.display.as_deref(), Some(":75"));
    assert_eq!(endpoints.wayland_socket, "/tmp/owned/wayland-0");
    // Its X11 endpoint is a pair.
    let mut half = values.clone();
    half.remove("XAUTHORITY");
    assert!(claim(&half).endpoints().is_err());
    // One marker makes it a launched program's claim, held to all of them.
    let mut marked = values;
    marked.insert("NEWPORT_GUI_WAYLAND_DISPLAY", "wayland-0".into());
    assert!(claim(&marked).endpoints().is_err());
}

#[test]
fn a_claim_on_the_stable_display_is_current_while_it_names_the_running_display() {
    use std::os::unix::fs::PermissionsExt;
    let folder = tempfile::tempdir_in("/tmp").unwrap();
    fs::set_permissions(folder.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = folder.path().to_str().unwrap().to_owned();
    let wayland = format!("{path}/wayland-0");
    let authority = format!("{path}/Xauthority");
    let mut values = values();
    for (name, value) in [
        ("NEWPORT_DISPLAY", path.as_str()),
        ("WAYLAND_DISPLAY", wayland.as_str()),
        ("NEWPORT_GUI_WAYLAND_DISPLAY", wayland.as_str()),
        ("XAUTHORITY", authority.as_str()),
        ("NEWPORT_GUI_AUTHORITY", authority.as_str()),
    ] {
        values.insert(name, value.into());
    }
    let stable = claim(&values);
    assert_eq!(stable.stable().unwrap(), Some(path.as_str()));
    // Not running: nothing to be current on.
    assert!(stable.check_stable(folder.path()).is_err());
    let _display = UnixListener::bind(folder.path().join("display.sock")).unwrap();
    let running = |wayland: &str| {
        let environment = serde_json::json!({
            "DISPLAY": ":75", "XAUTHORITY": authority,
            "NEWPORT_GUI_DISPLAY": ":75", "NEWPORT_GUI_AUTHORITY": authority,
            "WAYLAND_DISPLAY": wayland, "NEWPORT_GUI_WAYLAND_DISPLAY": wayland,
            "WAYLAND_SOCKET": null, "XDG_RUNTIME_DIR": "/tmp/owned",
            "NEWPORT_DISPLAY": path,
        });
        fs::write(
            folder.path().join("environment.json"),
            environment.to_string(),
        )
        .unwrap();
    };
    running(&wayland);
    stable.check_stable(folder.path()).unwrap();
    // A shell on the display, with no folder or markers, is current too.
    let mut shell = values.clone();
    shell.remove("NEWPORT_DISPLAY");
    for (_, marker) in shared_terminal_v1::MARKERS {
        shell.remove(marker);
    }
    assert_eq!(claim(&shell).stable().unwrap(), None);
    claim(&shell).check_stable(folder.path()).unwrap();
    // Another display's endpoints, or ones that don't match their markers.
    running(&format!("{path}/wayland-1"));
    assert!(stable.check_stable(folder.path()).is_err());
    assert!(claim(&shell).check_stable(folder.path()).is_err());
    running(&wayland);
    let mut foreign = values.clone();
    foreign.insert("WAYLAND_DISPLAY", "/tmp/owned/wayland-9".into());
    assert!(claim(&foreign).check_stable(folder.path()).is_err());
    // Only a folder private to this account is the account's display.
    fs::set_permissions(folder.path(), fs::Permissions::from_mode(0o750)).unwrap();
    assert!(stable.check_stable(folder.path()).is_err());
    fs::set_permissions(folder.path(), fs::Permissions::from_mode(0o700)).unwrap();
    stable.check_stable(folder.path()).unwrap();
    let link = tempfile::tempdir_in("/tmp").unwrap();
    let linked = link.path().join("newport-display");
    std::os::unix::fs::symlink(folder.path(), &linked).unwrap();
    assert!(stable.check_stable(&linked).is_err());
    for folder in ["", "relative", "/tmp/../x"] {
        let mut invalid = values.clone();
        invalid.insert("NEWPORT_DISPLAY", folder.into());
        assert!(claim(&invalid).stable().is_err(), "{folder:?}");
    }
}

#[test]
fn the_live_stable_display_names_its_viewer_only_while_it_runs_in_a_private_folder() {
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    let folder = tempfile::tempdir_in("/tmp").unwrap();
    fs::set_permissions(folder.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let live = || json!(live_stable_display(Some(folder.path())));
    let stopped = json!({"running": false, "viewer": null});
    assert_eq!(json!(live_stable_display(None)), stopped);
    // A viewer left behind by a display that isn't running is no viewer.
    let view = |computer: &str| {
        fs::write(
            folder.path().join("viewer.json"),
            json!({"session": "c0ffee", "computer": computer}).to_string(),
        )
        .unwrap();
    };
    view("studio-mac");
    assert_eq!(live(), stopped);
    let path = folder.path().to_str().unwrap();
    let wayland = format!("{path}/wayland-0");
    fs::write(
        folder.path().join("environment.json"),
        json!({
            "DISPLAY": null, "XAUTHORITY": null,
            "NEWPORT_GUI_DISPLAY": null, "NEWPORT_GUI_AUTHORITY": null,
            "WAYLAND_DISPLAY": wayland, "NEWPORT_GUI_WAYLAND_DISPLAY": wayland,
            "WAYLAND_SOCKET": null, "XDG_RUNTIME_DIR": "/tmp/owned",
            "NEWPORT_DISPLAY": path,
        })
        .to_string(),
    )
    .unwrap();
    let display = UnixListener::bind(folder.path().join("display.sock")).unwrap();
    assert_eq!(
        live(),
        json!({"running": true, "viewer": {"session": "c0ffee", "computer": "studio-mac"}})
    );
    // A computer that gave no name views it.
    view("");
    assert_eq!(
        live(),
        json!({"running": true, "viewer": {"session": "c0ffee", "computer": null}})
    );
    fs::remove_file(folder.path().join("viewer.json")).unwrap();
    assert_eq!(live(), json!({"running": true, "viewer": null}));
    // Only a folder private to this account is the account's display.
    view("studio-mac");
    fs::set_permissions(folder.path(), fs::Permissions::from_mode(0o750)).unwrap();
    assert_eq!(live(), stopped);
    fs::set_permissions(folder.path(), fs::Permissions::from_mode(0o700)).unwrap();
    // The display ended, taking its socket with it.
    drop(display);
    fs::remove_file(folder.path().join("display.sock")).unwrap();
    assert_eq!(live(), stopped);
}
