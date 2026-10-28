//! Public environment validation contracts. Source owner: ruiyangke/newport.
//! Private consumers vendor this exact file and verify its hash; change the public
//! source first. These versioned profiles deliberately have different grammars.
//! Existing profiles are frozen: additions require explicit protocol negotiation.
//! A missing map entry inherits, Some("") sets empty, and None explicitly unsets.
// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Ruiyang Ke (ruiyangke)
use std::collections::BTreeMap;
pub type Values = BTreeMap<String, Option<String>>;

/// Legacy X11 terminal map: do not add shared-session or process-launch keys.
pub mod legacy_x11_v1 {
    use super::Values;
    pub fn allowed(name: &str) -> bool {
        matches!(
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
        )
    }
    pub fn validate(values: &Values) -> Result<(), String> {
        if values.len() > 24
            || values.iter().any(|(key, value)| {
                !allowed(key)
                    || value
                        .as_ref()
                        .is_some_and(|v| v.len() > 4096 || v.contains('\0'))
            })
        {
            return Err("Invalid terminal display environment".into());
        }
        let display = values
            .get("DISPLAY")
            .and_then(Option::as_deref)
            .unwrap_or_default();
        let authority = values
            .get("XAUTHORITY")
            .and_then(Option::as_deref)
            .unwrap_or_default();
        if !display.starts_with(':')
            || display.len() < 2
            || !display[1..].bytes().all(|b| b.is_ascii_digit())
            || !authority.starts_with('/')
            || values.get("WAYLAND_DISPLAY") != Some(&None)
            || values.get("WAYLAND_SOCKET") != Some(&None)
        {
            return Err("Invalid private X11 terminal display".into());
        }
        Ok(())
    }
}

/// Owned shared terminal map. Native-only mode requires separately negotiated
/// terminal-wayland-v1 and explicit null X11 endpoint/marker entries.
pub mod shared_terminal_v2 {
    use super::Values;
    /// Each owned endpoint and the marker that must carry the same value.
    pub const MARKERS: [(&str, &str); 4] = [
        ("DISPLAY", "NEWPORT_GUI_DISPLAY"),
        ("XAUTHORITY", "NEWPORT_GUI_AUTHORITY"),
        ("WAYLAND_DISPLAY", "NEWPORT_GUI_WAYLAND_DISPLAY"),
        ("XDG_RUNTIME_DIR", "NEWPORT_GUI_RUNTIME_DIR"),
    ];
    pub fn allowed(name: &str) -> bool {
        super::legacy_x11_v1::allowed(name)
            || matches!(
                name,
                "XDG_RUNTIME_DIR"
                    | "DBUS_SESSION_BUS_ADDRESS"
                    | "DBUS_STARTER_ADDRESS"
                    | "DBUS_STARTER_BUS_TYPE"
                    | "NEWPORT_SESSION"
                    | "NEWPORT_GUI_DISPLAY"
                    | "NEWPORT_GUI_AUTHORITY"
                    | "NEWPORT_GUI_WAYLAND_DISPLAY"
                    | "NEWPORT_GUI_RUNTIME_DIR"
            )
    }
    pub fn validate(values: &Values, native_only: bool) -> Result<(), String> {
        let bad = || "Invalid shared terminal display environment".to_owned();
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
        let display = get("DISPLAY");
        let x11_valid = if native_only {
            [
                "DISPLAY",
                "XAUTHORITY",
                "NEWPORT_GUI_DISPLAY",
                "NEWPORT_GUI_AUTHORITY",
            ]
            .iter()
            .all(|name| values.get(*name) == Some(&None))
        } else {
            display
                .strip_prefix(':')
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()))
                && path(get("XAUTHORITY"))
        };
        if !x11_valid || !path(get("XDG_RUNTIME_DIR")) || !component(get("WAYLAND_DISPLAY")) {
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
        let session = get("NEWPORT_SESSION");
        if session.len() != 36
            || !session.bytes().enumerate().all(|(i, c)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
        {
            return Err(bad());
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
/// package export roots, and toolkit backend hints. Negotiated on its own
/// (`terminal-application-v1`, GUI launch v5); its names are disjoint from the
/// endpoint profiles, which never accept them.
pub mod application_v1 {
    use super::Values;
    pub const NAMES: [&str; 4] = [
        "XDG_CURRENT_DESKTOP",
        "XDG_DATA_DIRS",
        "MOZ_ENABLE_WAYLAND",
        "ELECTRON_OZONE_PLATFORM_HINT",
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
        value.len() <= 4096
            && match name {
                "XDG_CURRENT_DESKTOP" => value.split(':').all(|desktop| {
                    !desktop.is_empty()
                        && desktop
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
                }),
                "XDG_DATA_DIRS" => value.split(':').all(|directory| {
                    directory.starts_with('/') && !directory.chars().any(char::is_control)
                }),
                "MOZ_ENABLE_WAYLAND" => matches!(value, "0" | "1"),
                "ELECTRON_OZONE_PLATFORM_HINT" => matches!(value, "auto" | "wayland" | "x11"),
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

/// Schema search paths and accessibility policy require their own negotiation.
pub mod application_v2 {
    use super::{Values, application_v1};
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
    pub fn valid(name: &str, value: Option<&str>) -> bool {
        if application_v1::allowed(name) {
            return application_v1::valid(name, value);
        }
        let Some(value) = value else {
            return allowed(name);
        };
        value.len() <= 4096
            && match name {
                "GSETTINGS_SCHEMA_DIR" => value.split(':').all(|directory| {
                    directory.starts_with('/') && !directory.chars().any(char::is_control)
                }),
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

/// Typed public shell attachment. UUID/request/epoch/capability identity checks
/// belong to its envelope; this validates bounded endpoints and toolkit data.
pub mod shared_attachment_v2 {
    use super::Values;
    pub fn validate(
        display: Option<&str>,
        authority: Option<&str>,
        wayland_socket: &str,
        fallback_bus: Option<&str>,
        toolkit: &Values,
    ) -> Result<(), String> {
        let local_path = |path: &str| {
            path.starts_with('/')
                && path.len() <= 4096
                && !path.chars().any(char::is_control)
                && path[1..]
                    .split('/')
                    .all(|part| !matches!(part, "" | "." | ".."))
        };
        let valid_x11 = match (display, authority) {
            (None, None) => true,
            (Some(display), Some(authority)) => {
                let number = display.strip_prefix(':').unwrap_or_default();
                !number.is_empty()
                    && number.len() <= 10
                    && number.bytes().all(|byte| byte.is_ascii_digit())
                    && local_path(authority)
            }
            _ => false,
        };
        let reserved = ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET"];
        if !valid_x11
            || !local_path(wayland_socket)
            || fallback_bus.is_some_and(|address| {
                let Some(rest) = address.strip_prefix("unix:path=") else {
                    return true;
                };
                let (path, guid) = rest
                    .split_once(",guid=")
                    .map_or((rest, None), |(p, g)| (p, Some(g)));
                !local_path(path)
                    || !path
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/_.-".contains(&b))
                    || guid
                        .is_some_and(|g| g.len() != 32 || !g.bytes().all(|b| b.is_ascii_hexdigit()))
            })
            || toolkit.len() > 24
            || toolkit.iter().any(|(name, value)| {
                !super::legacy_x11_v1::allowed(name)
                    || reserved.contains(&name.as_str())
                    || value
                        .as_ref()
                        .is_some_and(|value| value.len() > 4096 || value.contains('\0'))
            })
        {
            return Err("Invalid shared graphical environment".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn legacy() -> Values {
        [
            ("DISPLAY", Some(":75")),
            ("XAUTHORITY", Some("/tmp/session/Xauthority")),
            ("WAYLAND_DISPLAY", None),
            ("WAYLAND_SOCKET", None),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.map(str::to_owned)))
        .collect()
    }
    fn shared() -> Values {
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
                "NEWPORT_SESSION",
                Some("12345678-1234-1234-1234-123456789abc"),
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.map(str::to_owned)))
        .collect()
    }
    fn attachment(authority: &str, toolkit: &Values) -> Result<(), String> {
        shared_attachment_v2::validate(
            Some(":75"),
            Some(authority),
            "/tmp/session/wayland-0",
            None,
            toolkit,
        )
    }

    #[test]
    fn frozen_profiles_accept_only_their_own_key_sets() {
        for name in [
            "PATH",
            "LD_PRELOAD",
            "XDG_SESSION_TYPE",
            "GDK_DEBUG",
            "QT_PLUGIN_PATH",
        ] {
            assert!(!legacy_x11_v1::allowed(name), "legacy {name}");
            assert!(!shared_terminal_v2::allowed(name), "shared {name}");
            assert!(
                attachment("/tmp/session/Xauthority", &[(name.into(), None)].into()).is_err(),
                "attachment {name}"
            );
        }
        for name in [
            "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS",
            "NEWPORT_SESSION",
        ] {
            assert!(!legacy_x11_v1::allowed(name));
            assert!(shared_terminal_v2::allowed(name));
            assert!(attachment("/tmp/session/Xauthority", &[(name.into(), None)].into()).is_err());
        }
        assert!(legacy_x11_v1::validate(&legacy()).is_ok());
        assert!(shared_terminal_v2::validate(&legacy(), false).is_err());
        assert!(shared_terminal_v2::validate(&shared(), false).is_ok());
        assert!(legacy_x11_v1::validate(&shared()).is_err());
    }

    #[test]
    fn version_specific_path_grammars_are_not_silently_strengthened_or_widened() {
        let toolkit = Values::new();
        for (path, legacy_valid, shared_valid, attachment_valid) in [
            ("/tmp/session/Xauthority", true, true, true),
            ("/tmp/a b/Xauthority", true, false, true),
            ("/tmp/../Xauthority", true, false, false),
            ("relative", false, false, false),
        ] {
            let mut first = legacy();
            first.insert("XAUTHORITY".into(), Some(path.into()));
            assert_eq!(
                legacy_x11_v1::validate(&first).is_ok(),
                legacy_valid,
                "legacy {path}"
            );
            let mut second = shared();
            for name in ["XAUTHORITY", "NEWPORT_GUI_AUTHORITY"] {
                second.insert(name.into(), Some(path.into()));
            }
            assert_eq!(
                shared_terminal_v2::validate(&second, false).is_ok(),
                shared_valid,
                "shared {path}"
            );
            assert_eq!(
                attachment(path, &toolkit).is_ok(),
                attachment_valid,
                "attachment {path}"
            );
        }
        let mut second = shared();
        for name in ["WAYLAND_DISPLAY", "NEWPORT_GUI_WAYLAND_DISPLAY"] {
            second.insert(name.into(), Some("/tmp/session/wayland-0".into()));
        }
        assert!(shared_terminal_v2::validate(&second, false).is_err());
        assert!(attachment("/tmp/session/Xauthority", &toolkit).is_ok());
    }

    #[test]
    fn missing_empty_and_unset_keep_distinct_meanings() {
        for mut values in [legacy(), shared()] {
            let is_shared = values.contains_key("NEWPORT_SESSION");
            let validate = |values: &Values| {
                if is_shared {
                    shared_terminal_v2::validate(values, false)
                } else {
                    legacy_x11_v1::validate(values)
                }
            };
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
    }

    #[test]
    fn application_profiles_keep_their_closed_wire_names() {
        assert_eq!(
            application_v1::NAMES,
            [
                "XDG_CURRENT_DESKTOP",
                "XDG_DATA_DIRS",
                "MOZ_ENABLE_WAYLAND",
                "ELECTRON_OZONE_PLATFORM_HINT",
            ]
        );
        for (name, value) in [
            ("GSETTINGS_SCHEMA_DIR", "/usr/share/glib-2.0/schemas"),
            ("GTK_A11Y", "none"),
            ("NO_AT_BRIDGE", "1"),
        ] {
            for value in [None, Some(value)] {
                let values = [(name.into(), value.map(str::to_owned))].into();
                assert!(application_v1::validate(&values).is_err());
                assert!(application_v2::validate(&values).is_ok());
            }
        }
    }

    #[test]
    fn application_names_are_their_own_negotiated_profile() {
        for name in application_v2::NAMES {
            assert!(!legacy_x11_v1::allowed(name), "legacy {name}");
            assert!(!shared_terminal_v2::allowed(name), "shared {name}");
            assert!(
                attachment("/tmp/session/Xauthority", &[(name.into(), None)].into()).is_err(),
                "attachment {name}"
            );
            let mut values = shared();
            values.insert(name.into(), None);
            assert!(
                shared_terminal_v2::validate(&values, false).is_err(),
                "{name}"
            );
            assert!(application_v2::valid(name, None));
            assert!(!application_v2::valid(name, Some("")), "empty {name}");
        }
        for name in [
            "PATH",
            "LD_PRELOAD",
            "DISPLAY",
            "GDK_BACKEND",
            "XDG_CONFIG_DIRS",
        ] {
            assert!(!application_v2::allowed(name));
            assert!(!application_v2::valid(name, None));
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
        application_v2::validate(&display).unwrap();
        assert!(application_v2::validate(&Values::new()).is_ok());
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
                application_v2::validate(&invalid).is_err(),
                "{name}={value:?}"
            );
        }
        let mut long = display;
        long.insert(
            "XDG_DATA_DIRS".into(),
            Some(format!("/{}", "a".repeat(4096))),
        );
        assert!(application_v2::validate(&long).is_err());
    }

    #[test]
    fn native_only_requires_explicit_null_pairs_not_missing_or_empty() {
        let mut values = shared();
        assert!(shared_terminal_v2::validate(&values, true).is_err());
        let x11 = [
            "DISPLAY",
            "XAUTHORITY",
            "NEWPORT_GUI_DISPLAY",
            "NEWPORT_GUI_AUTHORITY",
        ];
        for name in x11 {
            values.insert(name.into(), None);
        }
        assert!(shared_terminal_v2::validate(&values, true).is_ok());
        assert!(shared_terminal_v2::validate(&values, false).is_err());
        for name in x11 {
            let mut invalid = values.clone();
            invalid.remove(name);
            assert!(
                shared_terminal_v2::validate(&invalid, true).is_err(),
                "missing {name}"
            );
            invalid.insert(name.into(), Some(String::new()));
            assert!(
                shared_terminal_v2::validate(&invalid, true).is_err(),
                "empty {name}"
            );
        }
        assert!(
            shared_attachment_v2::validate(
                None,
                None,
                "/tmp/session/wayland-0",
                None,
                &Values::new()
            )
            .is_ok()
        );
        assert!(
            shared_attachment_v2::validate(
                Some(":75"),
                None,
                "/tmp/session/wayland-0",
                None,
                &Values::new()
            )
            .is_err()
        );
    }
}
