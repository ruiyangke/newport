//! Shared display environment contracts. Each profile has its own grammar.
//! Until v1.0, each profile is version 1 and changes in place.
//! A missing map entry inherits, Some("") sets empty, and None explicitly unsets.
use std::collections::BTreeMap;
pub type Values = BTreeMap<String, Option<String>>;

/// The account's stable display, shown on one computer at a time. Requests
/// follow the computer viewing it, named in the display's folder when they are
/// made. Its Wayland socket is a name in the runtime directory, or a path
/// relative to it, as aliases in its folder are, so the names stay the same
/// when the display restarts. Its X11 endpoint and markers are either the
/// display's own, which XWayland serves on demand, or explicitly null.
pub mod shared_terminal_v1 {
    use super::Values;
    /// The display's folder, an absolute path.
    pub const FOLDER: &str = "NEWPORT_DISPLAY";
    /// Each owned endpoint and the marker that must carry the same value.
    pub const MARKERS: [(&str, &str); 4] = [
        ("DISPLAY", "NEWPORT_GUI_DISPLAY"),
        ("XAUTHORITY", "NEWPORT_GUI_AUTHORITY"),
        ("WAYLAND_DISPLAY", "NEWPORT_GUI_WAYLAND_DISPLAY"),
        ("XDG_RUNTIME_DIR", "NEWPORT_GUI_RUNTIME_DIR"),
    ];
    /// The display's folder in the account's runtime directory.
    pub const DIRECTORY: &str = "newport-display";
    /// Socket names in the folder that remain stable across display runs.
    pub const WAYLAND: &str = "wayland-0";
    pub const BUS: &str = "bus";
    /// A display's authority file beside its directory. Snap permits regular
    /// `xauth_*` files directly in the account's runtime directory.
    pub fn authority_name(directory: &str) -> String {
        format!("xauth_{directory}")
    }
    /// The display's sink on the account's own sound server.
    pub const SINK: &str = "newport-display";
    /// The desktop's name; unregistered names carry the `X-` prefix (Desktop
    /// Menu Specification).
    pub const DESKTOP: &str = "X-Newport";
    /// Newport's data directory in the runtime directory, first in
    /// `XDG_DATA_DIRS`; its compiled schemas, `SCHEMAS` in it, go first in
    /// `GSETTINGS_SCHEMA_DIR`.
    pub const DATA: &str = "newport-remote/gsettings/share";
    pub const SCHEMAS: &str = "glib-2.0/schemas";
    /// What every program on the display gets beside its endpoints, bus and
    /// data directories, whether launched there or run from a terminal: the
    /// desktop's identity, toolkit backends, input method, accessibility and
    /// sound. None unsets. Values that depend on the computer viewing it,
    /// such as scale, are not here: Wayland clients take those from their
    /// output.
    pub const APPLICATION: [(&str, Option<&str>); 11] = [
        ("XDG_SESSION_TYPE", Some("wayland")),
        ("XDG_CURRENT_DESKTOP", Some(DESKTOP)),
        ("GDK_BACKEND", Some("wayland,x11")),
        ("QT_QPA_PLATFORM", Some("wayland;xcb")),
        ("MOZ_ENABLE_WAYLAND", Some("1")),
        ("GTK_IM_MODULE", None),
        ("QT_IM_MODULE", None),
        ("IBUS_USE_PORTAL", Some("1")),
        ("GTK_A11Y", Some("none")),
        ("NO_AT_BRIDGE", Some("1")),
        ("PULSE_SINK", Some(SINK)),
    ];
    /// `value` as a D-Bus address value: bytes outside its unescaped set
    /// as `%XX` (D-Bus specification, "Server Addresses").
    fn address_value(value: &str) -> String {
        value
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_/.*".contains(&b) {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02x}")
                }
            })
            .collect()
    }
    /// What a terminal's shell sets so the programs it runs use the display,
    /// as programs launched on it do, whether or not it runs yet. `runtime`
    /// is the account's runtime directory, `x11` the display's X11 number
    /// when it has one, and `inherited` the shell's own variables. The bus
    /// is an address list: the display's while it runs, else the login's,
    /// so command-line tools keep a session bus. The Wayland path is absolute
    /// so Snap can replace `XDG_RUNTIME_DIR` without changing the endpoint.
    pub fn terminal(
        runtime: &str,
        x11: Option<u32>,
        inherited: impl Fn(&str) -> Option<String>,
    ) -> Vec<(&'static str, Option<String>)> {
        let inherited = |name: &str| inherited(name).filter(|value| !value.is_empty());
        let folder = format!("{runtime}/{DIRECTORY}");
        let display_bus = format!("unix:path={}", address_value(&format!("{folder}/{BUS}")));
        let login_bus = inherited("DBUS_SESSION_BUS_ADDRESS")
            .map(|address| {
                address
                    .split(';')
                    .filter(|entry| !entry.is_empty() && *entry != display_bus)
                    .collect::<Vec<_>>()
                    .join(";")
            })
            .filter(|address| !address.is_empty())
            .unwrap_or_else(|| format!("unix:path={}", address_value(&format!("{runtime}/bus"))));
        let data = format!("{runtime}/{DATA}");
        let first = |value: &str, rest: Option<String>| match rest {
            Some(rest) => {
                let rest: Vec<&str> = rest.split(':').filter(|dir| *dir != value).collect();
                std::iter::once(value)
                    .chain(rest)
                    .collect::<Vec<_>>()
                    .join(":")
            }
            None => value.to_owned(),
        };
        let mut values = vec![
            ("WAYLAND_DISPLAY", Some(format!("{folder}/{WAYLAND}"))),
            ("XDG_RUNTIME_DIR", Some(runtime.to_owned())),
            ("DISPLAY", x11.map(|number| format!(":{number}"))),
            (
                "XAUTHORITY",
                x11.map(|_| format!("{runtime}/{}", authority_name(DIRECTORY))),
            ),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                Some(format!("{display_bus};{login_bus}")),
            ),
            (
                "XDG_DATA_DIRS",
                Some(first(
                    &data,
                    Some(
                        inherited("XDG_DATA_DIRS")
                            .unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
                    ),
                )),
            ),
            (
                "GSETTINGS_SCHEMA_DIR",
                Some(first(
                    &format!("{data}/{SCHEMAS}"),
                    inherited("GSETTINGS_SCHEMA_DIR"),
                )),
            ),
        ];
        values.extend(
            APPLICATION
                .iter()
                .map(|(name, value)| (*name, value.map(str::to_owned))),
        );
        // A terminal is not a program launched on the display, and requests
        // from it follow the terminal, not the display's viewer.
        values.extend(MARKERS.iter().map(|(_, marker)| (*marker, None)).chain([
            ("WAYLAND_SOCKET", None),
            (FOLDER, None),
            ("DBUS_STARTER_ADDRESS", None),
            ("DBUS_STARTER_BUS_TYPE", None),
            ("DBUS_SESSION_BUS_PID", None),
            ("DBUS_SESSION_BUS_WINDOWID", None),
        ]));
        values
    }
    pub fn allowed(name: &str) -> bool {
        name == FOLDER
            || matches!(
                name,
                "DISPLAY"
                    | "XAUTHORITY"
                    | "WAYLAND_DISPLAY"
                    | "WAYLAND_SOCKET"
                    | "GDK_BACKEND"
                    | "QT_QPA_PLATFORM"
                    | "GDK_SCALE"
                    | "GDK_DPI_SCALE"
                    | "QT_FONT_DPI"
                    | "QT_SCALE_FACTOR"
                    | "QT_AUTO_SCREEN_SCALE_FACTOR"
                    | "QT_ENABLE_HIGHDPI_SCALING"
                    | "GTK2_RC_FILES"
                    | "XMODIFIERS"
                    | "GTK_IM_MODULE"
                    | "QT_IM_MODULE"
                    | "IBUS_USE_PORTAL"
                    | "PULSE_SERVER"
                    | "PULSE_SINK"
                    | "XDG_RUNTIME_DIR"
                    | "DBUS_SESSION_BUS_ADDRESS"
                    | "DBUS_STARTER_ADDRESS"
                    | "DBUS_STARTER_BUS_TYPE"
                    | "NEWPORT_GUI_DISPLAY"
                    | "NEWPORT_GUI_AUTHORITY"
                    | "NEWPORT_GUI_WAYLAND_DISPLAY"
                    | "NEWPORT_GUI_RUNTIME_DIR"
            )
    }
    /// The folder's grammar: absolute, with plain components.
    pub fn folder(value: &str) -> bool {
        value.len() <= 4096
            && value.strip_prefix('/').is_some_and(|rest| {
                rest.split('/').all(|part| {
                    !part.is_empty()
                        && !matches!(part, "." | "..")
                        && part
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
                })
            })
    }
    pub fn validate(values: &Values) -> Result<(), String> {
        let bad = || "Invalid stable display environment".to_owned();
        if values.len() > 40
            || values.iter().any(|(key, value)| {
                !allowed(key)
                    || value
                        .as_ref()
                        .is_some_and(|v| v.len() > 4096 || v.contains('\0'))
            })
        {
            return Err(bad());
        }
        let get = |key: &str| {
            values
                .get(key)
                .and_then(Option::as_deref)
                .unwrap_or_default()
        };
        if !values
            .get(FOLDER)
            .and_then(Option::as_deref)
            .is_some_and(folder)
        {
            return Err(bad());
        }
        let component = |value: &str| {
            !value.is_empty()
                && !matches!(value, "." | "..")
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        };
        let path = |value: &str| {
            value
                .strip_prefix('/')
                .is_some_and(|v| v.split('/').all(component))
        };
        let wayland = get("WAYLAND_DISPLAY");
        let wayland_valid = if wayland.contains('/') {
            folder(&format!("/{wayland}"))
        } else {
            component(wayland)
        };
        let display = get("DISPLAY");
        let owned_x11 = display
            .strip_prefix(':')
            .is_some_and(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()))
            && path(get("XAUTHORITY"));
        let x11_valid = owned_x11
            || [
                "DISPLAY",
                "XAUTHORITY",
                "NEWPORT_GUI_DISPLAY",
                "NEWPORT_GUI_AUTHORITY",
            ]
            .iter()
            .all(|name| values.get(*name) == Some(&None));
        if !x11_valid || !path(get("XDG_RUNTIME_DIR")) || !wayland_valid {
            return Err(bad());
        }
        for name in [
            "WAYLAND_SOCKET",
            "DBUS_STARTER_ADDRESS",
            "DBUS_STARTER_BUS_TYPE",
        ] {
            if values.get(name) != Some(&None) {
                return Err(bad());
            }
        }
        for (endpoint, marker) in MARKERS {
            if get(endpoint) != get(marker) {
                return Err(bad());
            }
        }
        let address = get("DBUS_SESSION_BUS_ADDRESS")
            .strip_prefix("unix:path=")
            .ok_or_else(bad)?;
        let (socket, guid) = address
            .split_once(",guid=")
            .map_or((address, None), |(p, g)| (p, Some(g)));
        if !path(socket)
            || guid.is_some_and(|g| g.len() != 32 || !g.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(bad());
        }
        Ok(())
    }
}

/// What a Newport display sets for every program on it, launched or started
/// from one of its terminals: the desktop's identity, the data directories with
/// package export roots, toolkit backend hints, schema search paths and
/// accessibility policy. Its names are disjoint from the stable display's,
/// which never accepts them.
pub mod application_v1 {
    use super::Values;
    pub const NAMES: [&str; 7] = [
        "XDG_CURRENT_DESKTOP",
        "XDG_DATA_DIRS",
        "MOZ_ENABLE_WAYLAND",
        "ELECTRON_OZONE_PLATFORM_HINT",
        "GSETTINGS_SCHEMA_DIR",
        "GTK_A11Y",
        "NO_AT_BRIDGE",
    ];
    pub fn allowed(name: &str) -> bool {
        NAMES.contains(&name)
    }
    /// One entry in its variable's grammar; None unsets it. Empty values are
    /// not meaningful for these variables and are refused.
    pub fn valid(name: &str, value: Option<&str>) -> bool {
        let Some(value) = value else {
            return allowed(name);
        };
        let directories = |value: &str| {
            value.split(':').all(|directory| {
                directory.starts_with('/') && !directory.chars().any(char::is_control)
            })
        };
        value.len() <= 4096
            && match name {
                "XDG_CURRENT_DESKTOP" => value.split(':').all(|desktop| {
                    !desktop.is_empty()
                        && desktop
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
                }),
                "XDG_DATA_DIRS" | "GSETTINGS_SCHEMA_DIR" => directories(value),
                "MOZ_ENABLE_WAYLAND" => matches!(value, "0" | "1"),
                "ELECTRON_OZONE_PLATFORM_HINT" => matches!(value, "auto" | "wayland" | "x11"),
                "GTK_A11Y" => value == "none",
                "NO_AT_BRIDGE" => value == "1",
                _ => false,
            }
    }
    pub fn validate(values: &Values) -> Result<(), String> {
        if values
            .iter()
            .all(|(name, value)| valid(name, value.as_deref()))
        {
            Ok(())
        } else {
            Err("Invalid display application environment".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stable() -> Values {
        [
            ("DISPLAY", Some(":75")),
            ("XAUTHORITY", Some("/tmp/session/Xauthority")),
            ("WAYLAND_DISPLAY", Some("wayland-0")),
            ("WAYLAND_SOCKET", None),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                Some("unix:path=/tmp/session/bus"),
            ),
            ("DBUS_STARTER_ADDRESS", None),
            ("DBUS_STARTER_BUS_TYPE", None),
            ("NEWPORT_GUI_DISPLAY", Some(":75")),
            ("NEWPORT_GUI_AUTHORITY", Some("/tmp/session/Xauthority")),
            ("NEWPORT_GUI_WAYLAND_DISPLAY", Some("wayland-0")),
            ("NEWPORT_GUI_RUNTIME_DIR", Some("/run/user/1000")),
            (
                shared_terminal_v1::FOLDER,
                Some("/run/user/1000/newport-display"),
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.map(str::to_owned)))
        .collect()
    }
    #[test]
    fn a_terminal_keeps_its_wayland_endpoint_when_snap_replaces_the_runtime_directory() {
        let values: BTreeMap<_, _> = shared_terminal_v1::terminal("/run/user/1000", None, |_| None)
            .into_iter()
            .collect();
        let display = values["WAYLAND_DISPLAY"].as_deref().unwrap();
        let endpoint = std::path::Path::new("/run/user/1000/newport-display/wayland-0");
        for runtime in ["/run/user/1000", "/run/user/1000/snap.chromium"] {
            assert_eq!(std::path::Path::new(runtime).join(display), endpoint);
        }
    }
    #[test]
    fn a_terminal_names_the_display_and_keeps_the_logins_bus_and_data_after_it() {
        let login = |name: &str| match name {
            "DBUS_SESSION_BUS_ADDRESS" => Some("unix:path=/run/user/1000/bus".to_owned()),
            "XDG_DATA_DIRS" => Some("/usr/share:/var/lib/flatpak/exports/share".to_owned()),
            _ => None,
        };
        let values: BTreeMap<_, _> = shared_terminal_v1::terminal("/run/user/1000", Some(3), login)
            .into_iter()
            .collect();
        let get = |name: &str| values[name].as_deref();
        assert_eq!(
            get("WAYLAND_DISPLAY"),
            Some("/run/user/1000/newport-display/wayland-0")
        );
        assert_eq!(get("XDG_RUNTIME_DIR"), Some("/run/user/1000"));
        assert_eq!(get("DISPLAY"), Some(":3"));
        assert_eq!(
            get("XAUTHORITY"),
            Some("/run/user/1000/xauth_newport-display")
        );
        // The display's bus while it runs, the login's otherwise.
        assert_eq!(
            get("DBUS_SESSION_BUS_ADDRESS"),
            Some("unix:path=/run/user/1000/newport-display/bus;unix:path=/run/user/1000/bus")
        );
        assert_eq!(
            get("XDG_DATA_DIRS"),
            Some(
                "/run/user/1000/newport-remote/gsettings/share:/usr/share:/var/lib/flatpak/exports/share"
            )
        );
        assert_eq!(
            get("GSETTINGS_SCHEMA_DIR"),
            Some("/run/user/1000/newport-remote/gsettings/share/glib-2.0/schemas")
        );
        for (name, value) in shared_terminal_v1::APPLICATION {
            assert_eq!(get(name), value, "{name}");
        }
        assert_eq!(get("PULSE_SINK"), Some(shared_terminal_v1::SINK));
        for name in [
            "NEWPORT_GUI_DISPLAY",
            "NEWPORT_GUI_AUTHORITY",
            "NEWPORT_GUI_WAYLAND_DISPLAY",
            "NEWPORT_GUI_RUNTIME_DIR",
            "WAYLAND_SOCKET",
            shared_terminal_v1::FOLDER,
            "DBUS_STARTER_ADDRESS",
            "DBUS_STARTER_BUS_TYPE",
        ] {
            assert_eq!(values.get(name), Some(&None), "{name}");
        }
    }

    #[test]
    fn a_terminal_without_login_values_gets_their_defaults_and_names_each_once() {
        let values: BTreeMap<_, _> =
            shared_terminal_v1::terminal("/run/user/10 01", None, |_| None)
                .into_iter()
                .collect();
        // A display without X11 names none.
        assert_eq!(values["DISPLAY"], None);
        assert_eq!(values["XAUTHORITY"], None);
        // The account's bus where systemd puts it, with the path escaped
        // as D-Bus addresses require.
        assert_eq!(
            values["DBUS_SESSION_BUS_ADDRESS"].as_deref(),
            Some("unix:path=/run/user/10%2001/newport-display/bus;unix:path=/run/user/10%2001/bus")
        );
        assert_eq!(
            values["XDG_DATA_DIRS"].as_deref(),
            Some("/run/user/10 01/newport-remote/gsettings/share:/usr/local/share:/usr/share")
        );
        // A shell that already has them doesn't get them twice.
        let again = |name: &str| values.get(name).cloned().flatten();
        let twice: BTreeMap<_, _> = shared_terminal_v1::terminal("/run/user/10 01", None, again)
            .into_iter()
            .collect();
        assert_eq!(twice, values);
    }

    #[test]
    fn the_stable_display_accepts_only_its_own_key_set() {
        for name in [
            "PATH",
            "LD_PRELOAD",
            "XDG_SESSION_TYPE",
            "GDK_DEBUG",
            "QT_PLUGIN_PATH",
        ] {
            assert!(!shared_terminal_v1::allowed(name), "{name}");
        }
        for name in [
            "DISPLAY",
            "XAUTHORITY",
            "WAYLAND_DISPLAY",
            "WAYLAND_SOCKET",
            "GDK_BACKEND",
            "QT_QPA_PLATFORM",
            "GDK_SCALE",
            "GDK_DPI_SCALE",
            "QT_FONT_DPI",
            "QT_SCALE_FACTOR",
            "QT_AUTO_SCREEN_SCALE_FACTOR",
            "QT_ENABLE_HIGHDPI_SCALING",
            "GTK2_RC_FILES",
            "XMODIFIERS",
            "GTK_IM_MODULE",
            "QT_IM_MODULE",
            "IBUS_USE_PORTAL",
            "PULSE_SERVER",
            "PULSE_SINK",
            "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS",
            "DBUS_STARTER_ADDRESS",
            "DBUS_STARTER_BUS_TYPE",
            "NEWPORT_GUI_DISPLAY",
            "NEWPORT_GUI_AUTHORITY",
            "NEWPORT_GUI_WAYLAND_DISPLAY",
            "NEWPORT_GUI_RUNTIME_DIR",
            shared_terminal_v1::FOLDER,
        ] {
            assert!(shared_terminal_v1::allowed(name), "{name}");
        }
        assert!(shared_terminal_v1::validate(&stable()).is_ok());
    }

    #[test]
    fn stable_display_paths_keep_their_grammar() {
        for (path, stable_valid) in [
            ("/tmp/session/Xauthority", true),
            ("/tmp/a b/Xauthority", false),
            ("/tmp/../Xauthority", false),
            ("relative", false),
        ] {
            let mut second = stable();
            for name in ["XAUTHORITY", "NEWPORT_GUI_AUTHORITY"] {
                second.insert(name.into(), Some(path.into()));
            }
            assert_eq!(
                shared_terminal_v1::validate(&second).is_ok(),
                stable_valid,
                "stable {path}"
            );
        }
        let mut second = stable();
        for name in ["WAYLAND_DISPLAY", "NEWPORT_GUI_WAYLAND_DISPLAY"] {
            second.insert(name.into(), Some("/tmp/session/wayland-0".into()));
        }
        assert!(shared_terminal_v1::validate(&second).is_err());
    }

    #[test]
    fn missing_empty_and_unset_keep_distinct_meanings() {
        let mut values = stable();
        let validate = shared_terminal_v1::validate;
        // An optional toolkit key can inherit, explicitly unset, or set empty.
        assert!(!values.contains_key("GTK_IM_MODULE"));
        assert!(validate(&values).is_ok());
        for value in [None, Some(String::new())] {
            values.insert("GTK_IM_MODULE".into(), value.clone());
            assert!(validate(&values).is_ok());
            assert_eq!(values.get("GTK_IM_MODULE"), Some(&value));
        }
        values.remove("WAYLAND_SOCKET");
        assert!(validate(&values).is_err());
        values.insert("WAYLAND_SOCKET".into(), Some(String::new()));
        assert!(validate(&values).is_err());
        values.insert("WAYLAND_SOCKET".into(), None);
        assert!(validate(&values).is_ok());
    }

    #[test]
    fn the_application_profile_keeps_its_closed_wire_names() {
        assert_eq!(
            application_v1::NAMES,
            [
                "XDG_CURRENT_DESKTOP",
                "XDG_DATA_DIRS",
                "MOZ_ENABLE_WAYLAND",
                "ELECTRON_OZONE_PLATFORM_HINT",
                "GSETTINGS_SCHEMA_DIR",
                "GTK_A11Y",
                "NO_AT_BRIDGE",
            ]
        );
        for (name, value) in [
            ("XDG_DATA_DIRS", "/usr/share"),
            ("GSETTINGS_SCHEMA_DIR", "/usr/share/glib-2.0/schemas"),
            ("GTK_A11Y", "none"),
            ("NO_AT_BRIDGE", "1"),
        ] {
            for value in [None, Some(value)] {
                let values = [(name.into(), value.map(str::to_owned))].into();
                assert!(application_v1::validate(&values).is_ok(), "{name}");
            }
        }
        for (name, value) in [
            ("GSETTINGS_SCHEMA_DIR", "relative"),
            ("GTK_A11Y", "atspi"),
            ("NO_AT_BRIDGE", "0"),
        ] {
            assert!(!application_v1::valid(name, Some(value)), "{name}");
        }
    }

    #[test]
    fn application_names_are_their_own_negotiated_profile() {
        for name in application_v1::NAMES {
            assert!(!shared_terminal_v1::allowed(name), "stable {name}");
            let mut values = stable();
            values.insert(name.into(), None);
            assert!(
                shared_terminal_v1::validate(&values).is_err(),
                "stable {name}"
            );
            assert!(application_v1::valid(name, None));
            assert!(!application_v1::valid(name, Some("")), "empty {name}");
        }
        for name in [
            "PATH",
            "LD_PRELOAD",
            "DISPLAY",
            "GDK_BACKEND",
            "XDG_CONFIG_DIRS",
        ] {
            assert!(!application_v1::allowed(name));
            assert!(!application_v1::valid(name, None));
        }
        let display: Values = [
            ("XDG_CURRENT_DESKTOP", Some("X-Newport")),
            (
                "XDG_DATA_DIRS",
                Some("/usr/local/share:/usr/share:/home/a b/.local/share/flatpak/exports/share:/var/lib/snapd/desktop"),
            ),
            (
                "GSETTINGS_SCHEMA_DIR",
                Some("/run/user/1000/newport-remote/gsettings/share/glib-2.0/schemas"),
            ),
            ("MOZ_ENABLE_WAYLAND", Some("1")),
            ("ELECTRON_OZONE_PLATFORM_HINT", None),
            ("GTK_A11Y", Some("none")),
            ("NO_AT_BRIDGE", Some("1")),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.map(str::to_owned)))
        .collect();
        application_v1::validate(&display).unwrap();
        assert!(application_v1::validate(&Values::new()).is_ok());
        for (name, value) in [
            ("XDG_CURRENT_DESKTOP", "X-Newport;rm"),
            ("XDG_CURRENT_DESKTOP", "GNOME::X-Newport"),
            ("XDG_DATA_DIRS", "/usr/share:relative"),
            ("XDG_DATA_DIRS", "/usr/share::/opt/share"),
            ("XDG_DATA_DIRS", "/usr/share\n/evil"),
            ("MOZ_ENABLE_WAYLAND", "yes"),
            ("ELECTRON_OZONE_PLATFORM_HINT", "headless"),
            ("GSETTINGS_SCHEMA_DIR", "schemas"),
            ("GTK_A11Y", "atspi"),
            ("NO_AT_BRIDGE", "0"),
        ] {
            let mut invalid = display.clone();
            invalid.insert(name.into(), Some(value.into()));
            assert!(
                application_v1::validate(&invalid).is_err(),
                "{name}={value:?}"
            );
        }
        let mut long = display;
        long.insert(
            "XDG_DATA_DIRS".into(),
            Some(format!("/{}", "a".repeat(4096))),
        );
        assert!(application_v1::validate(&long).is_err());
    }

    #[test]
    fn x11_takes_owned_or_explicit_null_pairs_not_missing_or_empty() {
        let mut values = stable();
        // The display's own X11 endpoint, served by XWayland on demand.
        assert!(shared_terminal_v1::validate(&values).is_ok());
        let x11 = [
            "DISPLAY",
            "XAUTHORITY",
            "NEWPORT_GUI_DISPLAY",
            "NEWPORT_GUI_AUTHORITY",
        ];
        for name in x11 {
            values.insert(name.into(), None);
        }
        assert!(shared_terminal_v1::validate(&values).is_ok());
        for name in x11 {
            let mut invalid = values.clone();
            invalid.remove(name);
            assert!(
                shared_terminal_v1::validate(&invalid).is_err(),
                "missing {name}"
            );
            invalid.insert(name.into(), Some(String::new()));
            assert!(
                shared_terminal_v1::validate(&invalid).is_err(),
                "empty {name}"
            );
        }
    }

    #[test]
    fn the_stable_display_names_its_folder() {
        let values = stable();
        shared_terminal_v1::validate(&values).unwrap();
        for folder in [
            None,
            Some(""),
            Some("newport-display"),
            Some("/run/user/1000/../newport-display"),
            Some("/run/user/1000//newport-display"),
            Some("/run/user/1000/newport stable"),
            Some("/"),
        ] {
            let mut invalid = values.clone();
            invalid.insert(shared_terminal_v1::FOLDER.into(), folder.map(str::to_owned));
            assert!(
                shared_terminal_v1::validate(&invalid).is_err(),
                "{folder:?}"
            );
        }
        let mut missing = values.clone();
        missing.remove(shared_terminal_v1::FOLDER);
        assert!(shared_terminal_v1::validate(&missing).is_err());
        // Each endpoint keeps its grammar, and its marker names the same one.
        for (name, value) in [
            ("WAYLAND_DISPLAY", "/tmp/session/wayland-0"),
            ("NEWPORT_GUI_DISPLAY", ":76"),
            ("NEWPORT_GUI_RUNTIME_DIR", "/run/user/1001"),
            ("DBUS_SESSION_BUS_ADDRESS", "tcp:host=example"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/bus,guid=short"),
        ] {
            let mut invalid = values.clone();
            invalid.insert(name.into(), Some(value.into()));
            assert!(shared_terminal_v1::validate(&invalid).is_err(), "{name}");
        }
        for name in ["DBUS_STARTER_ADDRESS", "DBUS_STARTER_BUS_TYPE"] {
            let mut inherited = values.clone();
            inherited.remove(name);
            assert!(shared_terminal_v1::validate(&inherited).is_err(), "{name}");
        }
        // Or relative to the runtime directory, as the display's aliases are.
        let mut aliased = values.clone();
        for name in ["WAYLAND_DISPLAY", "NEWPORT_GUI_WAYLAND_DISPLAY"] {
            aliased.insert(name.into(), Some("newport-display/wayland-0".into()));
        }
        shared_terminal_v1::validate(&aliased).unwrap();
        for (wayland, marker) in [
            ("newport-display/wayland-0", "newport-display/wayland-1"),
            (
                "newport-display/../wayland-0",
                "newport-display/../wayland-0",
            ),
            ("newport-display//wayland-0", "newport-display//wayland-0"),
            ("newport-display/", "newport-display/"),
        ] {
            let mut invalid = values.clone();
            invalid.insert("WAYLAND_DISPLAY".into(), Some(wayland.into()));
            invalid.insert("NEWPORT_GUI_WAYLAND_DISPLAY".into(), Some(marker.into()));
            assert!(shared_terminal_v1::validate(&invalid).is_err(), "{wayland}");
        }
    }
}

/// The shared display's map for an owned remote shell. This is data;
/// consumers must quote assignments, never eval it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SharedEnvironment(pub BTreeMap<String, Option<String>>);
impl SharedEnvironment {
    pub fn allowed(name: &str) -> bool {
        self::shared_terminal_v1::allowed(name)
    }
    /// The display's map: its endpoints with the application policy
    /// (`self::application_v1`).
    pub fn validate(&self) -> Result<(), String> {
        let (shared, application) = self.split_application();
        self::shared_terminal_v1::validate(&shared.0)?;
        self::application_v1::validate(&application)
    }
    /// The frozen map and the application policy names, apart.
    pub fn split_application(&self) -> (Self, self::Values) {
        let (application, shared) = self
            .0
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .partition(|(name, _)| self::application_v1::allowed(name));
        (Self(shared), application)
    }
}

#[cfg(test)]
mod shared_environment_tests {
    use super::*;
    fn shared() -> SharedEnvironment {
        serde_json::from_value(serde_json::json!({
            "DISPLAY": ":75", "XAUTHORITY": "/tmp/nr-session-a/Xauthority",
            "WAYLAND_DISPLAY": "wayland-a", "XDG_RUNTIME_DIR": "/run/user/1000",
            "WAYLAND_SOCKET": null, "DBUS_STARTER_ADDRESS": null, "DBUS_STARTER_BUS_TYPE": null,
            "DBUS_SESSION_BUS_ADDRESS": "unix:path=/tmp/nr-session-a/bus,guid=0123456789abcdef0123456789abcdef",
            "NEWPORT_DISPLAY": "/run/user/1000/newport-display",
            "NEWPORT_GUI_DISPLAY": ":75", "NEWPORT_GUI_AUTHORITY": "/tmp/nr-session-a/Xauthority",
            "NEWPORT_GUI_WAYLAND_DISPLAY": "wayland-a", "NEWPORT_GUI_RUNTIME_DIR": "/run/user/1000"
        })).unwrap()
    }
    fn valid(environment: &SharedEnvironment) -> bool {
        environment.validate().is_ok()
    }
    #[test]
    fn shared_endpoints_require_complete_paired_local_identity() {
        let environment = shared();
        assert!(valid(&environment));
        for name in environment.0.keys() {
            let mut missing = environment.clone();
            missing.0.remove(name);
            assert!(!valid(&missing), "missing {name}");
        }
        for (name, value) in [
            ("DISPLAY", "host:0"),
            ("XAUTHORITY", "relative/path"),
            ("XAUTHORITY", "/tmp/../secret"),
            ("XDG_RUNTIME_DIR", "/tmp//runtime"),
            ("WAYLAND_DISPLAY", "../socket"),
            ("WAYLAND_SOCKET", "5"),
            ("NEWPORT_GUI_DISPLAY", ":76"),
            ("NEWPORT_GUI_AUTHORITY", "/tmp/foreign"),
            ("NEWPORT_GUI_WAYLAND_DISPLAY", "foreign"),
            ("NEWPORT_GUI_RUNTIME_DIR", "/tmp/foreign"),
            ("NEWPORT_DISPLAY", "relative/folder"),
            ("NEWPORT_DISPLAY", "/run/user/1000/../folder"),
            ("DBUS_STARTER_ADDRESS", "unix:path=/tmp/foreign"),
            ("DBUS_SESSION_BUS_ADDRESS", "tcp:host=example.org,port=1234"),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/tmp/bus;unix:path=/tmp/other",
            ),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/bus,guid=bad"),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/tmp/bus,abstract=other",
            ),
            ("PATH", "/tmp/evil"),
            ("LD_PRELOAD", "/tmp/evil.so"),
            ("GDK_BACKEND", "wayland\0"),
        ] {
            let mut invalid = environment.clone();
            invalid.0.insert(name.into(), Some(value.into()));
            assert!(!valid(&invalid), "{name}={value:?}");
        }
    }
    #[test]
    fn x11_is_the_displays_own_or_explicitly_removed() {
        let owned = shared();
        let mut native = owned.clone();
        let x11 = [
            "DISPLAY",
            "XAUTHORITY",
            "NEWPORT_GUI_DISPLAY",
            "NEWPORT_GUI_AUTHORITY",
        ];
        for name in x11 {
            native.0.insert(name.into(), None);
        }
        assert!(valid(&native));
        // The display's own X11 endpoint, served by XWayland on demand.
        assert!(valid(&owned));
        for name in native.0.keys() {
            let mut missing = native.clone();
            missing.0.remove(name);
            assert!(!valid(&missing), "missing {name}");
        }
        for name in x11 {
            let mut inherited = native.clone();
            inherited.0.insert(name.into(), owned.0[name].clone());
            assert!(!valid(&inherited), "inherited {name}");
            inherited.0.insert(name.into(), Some(String::new()));
            assert!(!valid(&inherited), "empty {name} must be unset");
        }
        for (name, value) in [
            ("WAYLAND_DISPLAY", "../foreign"),
            ("XDG_RUNTIME_DIR", "/tmp//foreign"),
            ("NEWPORT_GUI_WAYLAND_DISPLAY", "foreign"),
            ("DBUS_SESSION_BUS_ADDRESS", "tcp:host=foreign"),
        ] {
            let mut invalid = native.clone();
            invalid.0.insert(name.into(), Some(value.into()));
            assert!(!valid(&invalid), "{name}");
        }
    }
    #[test]
    fn the_display_names_its_aliases_relative_to_the_runtime_folder() {
        let mut environment = shared();
        for name in ["WAYLAND_DISPLAY", "NEWPORT_GUI_WAYLAND_DISPLAY"] {
            environment
                .0
                .insert(name.into(), Some("newport-display/wayland-0".into()));
        }
        for name in ["XAUTHORITY", "NEWPORT_GUI_AUTHORITY"] {
            environment.0.insert(
                name.into(),
                Some("/run/user/1000/xauth_newport-display".into()),
            );
        }
        assert!(valid(&environment));
        assert!(SharedEnvironment::allowed("NEWPORT_DISPLAY"));
        let mut mismatched = environment.clone();
        mismatched.0.insert(
            "NEWPORT_GUI_WAYLAND_DISPLAY".into(),
            Some("wayland-0".into()),
        );
        assert!(!valid(&mismatched));
        let mut escaping = environment;
        for name in ["WAYLAND_DISPLAY", "NEWPORT_GUI_WAYLAND_DISPLAY"] {
            escaping
                .0
                .insert(name.into(), Some("newport-display/../wayland-0".into()));
        }
        assert!(!valid(&escaping));
    }
    #[test]
    fn the_map_carries_the_application_policy() {
        let mut environment = shared();
        environment.0.extend(
            [
                ("XDG_CURRENT_DESKTOP", Some("X-Newport")),
                ("XDG_DATA_DIRS", Some("/usr/share:/var/lib/snapd/desktop")),
                ("MOZ_ENABLE_WAYLAND", Some("1")),
                ("ELECTRON_OZONE_PLATFORM_HINT", None),
            ]
            .map(|(name, value)| (name.to_owned(), value.map(str::to_owned))),
        );
        assert!(valid(&environment));
        let (frozen, application) = environment.split_application();
        assert_eq!(frozen, shared());
        assert_eq!(application.len(), 4);
        for (name, value) in [
            ("XDG_DATA_DIRS", "relative"),
            ("XDG_CURRENT_DESKTOP", "X Newport"),
            ("MOZ_ENABLE_WAYLAND", ""),
            ("PATH", "/tmp/evil"),
        ] {
            let mut invalid = environment.clone();
            invalid.0.insert(name.into(), Some(value.into()));
            assert!(!valid(&invalid), "{name}");
        }
        // The endpoint rules still apply in full, with or without X11.
        let mut native = environment;
        native.0.insert("XAUTHORITY".into(), None);
        assert!(!valid(&native));
        for name in [
            "DISPLAY",
            "XAUTHORITY",
            "NEWPORT_GUI_DISPLAY",
            "NEWPORT_GUI_AUTHORITY",
        ] {
            native.0.insert(name.into(), None);
        }
        assert!(valid(&native));
        for (name, value) in [
            ("GSETTINGS_SCHEMA_DIR", "/usr/share/glib-2.0/schemas"),
            ("GTK_A11Y", "none"),
            ("NO_AT_BRIDGE", "1"),
        ] {
            let mut expanded = native.clone();
            expanded.0.insert(name.into(), Some(value.into()));
            assert!(valid(&expanded), "{name}");
        }
    }
    #[test]
    fn the_shared_display_map_round_trips_as_json() {
        let value = shared();
        let serialized = serde_json::to_value(&value).unwrap();
        assert_eq!(serialized["DISPLAY"], ":75");
        assert_eq!(
            serde_json::from_value::<SharedEnvironment>(serialized.clone()).unwrap(),
            value
        );
    }
}
