//! Public display attachment data, independent of any GUI engine wire format.
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedGuiEnvironment {
    pub session: uuid::Uuid,
    /// Both X11 fields are present strings for a mixed display, or explicit
    /// nulls for a native Wayland display. Only version-four helpers accept
    /// the latter; missing fields never mean “inherit the caller's display”.
    #[serde(deserialize_with = "explicit_option")]
    pub display: Option<String>,
    #[serde(deserialize_with = "explicit_option")]
    pub authority: Option<String>,
    /// Absolute remote Unix socket path. Existing XDG_RUNTIME_DIR is retained;
    /// the shell uses this socket's parent when that variable is unset or empty.
    pub wayland_socket: String,
    /// The display's owned bus, used only when no explicit session/starter bus
    /// is inherited. Omit this field for version-two handoff readers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_bus: Option<String>,
    pub toolkit: BTreeMap<String, Option<String>>,
    /// The display's application policy: v5 helpers accept `application_v1`,
    /// v6 helpers may request `application_v2`. Omit it for earlier helpers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub application: BTreeMap<String, Option<String>>,
}

fn explicit_option<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    serde::Deserialize::deserialize(deserializer)
}
#[path = "contract.rs"]
pub mod contract;

pub fn allowed(name: &str) -> bool {
    contract::legacy_x11_v1::allowed(name)
}

impl SharedGuiEnvironment {
    pub fn validate(&self) -> Result<(), String> {
        if self.session.is_nil() {
            return Err("Invalid shared graphical environment".into());
        }
        contract::shared_attachment_v2::validate(
            self.display.as_deref(),
            self.authority.as_deref(),
            &self.wayland_socket,
            self.fallback_bus.as_deref(),
            &self.toolkit,
        )?;
        contract::application_v2::validate(&self.application)
    }
    pub fn native_wayland(&self) -> bool {
        self.display.is_none() && self.authority.is_none()
    }
    /// Call after validation. The wire address is a single local path, with an
    /// optional bus GUID; alternate transports and address lists are rejected.
    pub fn fallback_bus_socket(&self) -> Option<&str> {
        self.fallback_bus
            .as_deref()?
            .strip_prefix("unix:path=")?
            .split(',')
            .next()
    }
    /// This map is assignment data, never shell source. Shell adapters quote
    /// each value and explicitly unset None entries. Conditional bus/runtime
    /// defaults are applied separately, preserving explicit user values.
    pub fn shell_assignments(&self) -> Result<BTreeMap<String, Option<String>>, String> {
        self.validate()?;
        let mut values = self.toolkit.clone();
        values.extend(self.application.clone());
        for (name, value) in [
            ("DISPLAY", self.display.clone()),
            ("XAUTHORITY", self.authority.clone()),
            ("WAYLAND_DISPLAY", Some(self.wayland_socket.clone())),
            ("NEWPORT_GUI_DISPLAY", self.display.clone()),
            ("NEWPORT_GUI_AUTHORITY", self.authority.clone()),
            (
                "NEWPORT_GUI_WAYLAND_DISPLAY",
                Some(self.wayland_socket.clone()),
            ),
            ("NEWPORT_SESSION", Some(self.session.to_string())),
        ] {
            values.insert(name.into(), value);
        }
        values.insert("WAYLAND_SOCKET".into(), None);
        values.insert("NEWPORT_GUI_RUNTIME_DIR".into(), None);
        // SSH logins report a tty session. Like a desktop Wayland session,
        // declare Wayland so Chromium, Electron and Qt choose it themselves;
        // a mixed display still offers X11 to applications that need it.
        values.insert("XDG_SESSION_TYPE".into(), Some("wayland".into()));
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn environment() -> SharedGuiEnvironment {
        SharedGuiEnvironment {
            session: uuid::Uuid::new_v4(),
            display: Some(":75".into()),
            authority: Some("/tmp/session/Xauthority".into()),
            wayland_socket: "/run/user/1000/wayland-session".into(),
            fallback_bus: None,
            toolkit: [("GTK_IM_MODULE".into(), None)].into(),
            application: BTreeMap::new(),
        }
    }
    #[test]
    fn display_attachment_preserves_user_bus_and_runtime_and_clears_inherited_fd() {
        let env = environment();
        let assignments = env.shell_assignments().unwrap();
        for name in [
            "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS",
            "DBUS_STARTER_ADDRESS",
            "DBUS_STARTER_BUS_TYPE",
        ] {
            assert!(!assignments.contains_key(name), "{name}");
        }
        assert_eq!(assignments["WAYLAND_SOCKET"], None);
        assert_eq!(assignments["GTK_IM_MODULE"], None);
        assert_eq!(assignments["XDG_SESSION_TYPE"].as_deref(), Some("wayland"));
        assert_eq!(
            assignments["WAYLAND_DISPLAY"].as_deref(),
            Some(env.wayland_socket.as_str())
        );
        assert_eq!(assignments["DISPLAY"], assignments["NEWPORT_GUI_DISPLAY"]);
        assert_eq!(
            assignments["XAUTHORITY"],
            assignments["NEWPORT_GUI_AUTHORITY"]
        );
    }
    #[test]
    fn reject_foreign_display_paths_and_toolkit_injection() {
        for path in [
            "relative",
            "/tmp/../secret",
            "/tmp//socket",
            "/tmp/sock\0et",
            "/",
        ] {
            let mut env = environment();
            env.wayland_socket = path.into();
            assert!(env.validate().is_err(), "{path:?}");
        }
        for name in [
            "DISPLAY",
            "WAYLAND_SOCKET",
            "XAUTHORITY",
            "DBUS_SESSION_BUS_ADDRESS",
            "PATH",
            "LD_PRELOAD",
            "NEWPORT_SESSION",
        ] {
            let mut env = environment();
            env.toolkit.insert(name.into(), Some("other".into()));
            assert!(env.validate().is_err(), "{name}");
        }
        let mut env = environment();
        env.display = Some("host:0".into());
        assert!(env.validate().is_err());
        env = environment();
        env.session = uuid::Uuid::nil();
        assert!(env.validate().is_err());
    }
    #[test]
    fn native_wayland_requires_explicit_paired_nulls_and_unsets_x11() {
        let mut env = environment();
        let mixed = serde_json::to_value(&env).unwrap();
        assert_eq!(mixed["display"], ":75");
        assert_eq!(mixed["authority"], "/tmp/session/Xauthority");
        env.display = None;
        assert!(env.validate().is_err());
        env.authority = None;
        env.validate().unwrap();
        let assignments = env.shell_assignments().unwrap();
        for name in [
            "DISPLAY",
            "XAUTHORITY",
            "NEWPORT_GUI_DISPLAY",
            "NEWPORT_GUI_AUTHORITY",
        ] {
            assert_eq!(assignments.get(name), Some(&None), "{name}");
        }
        assert_eq!(assignments["XDG_SESSION_TYPE"].as_deref(), Some("wayland"));
        let native = serde_json::to_value(&env).unwrap();
        assert_eq!(native["display"], serde_json::Value::Null);
        assert_eq!(native["authority"], serde_json::Value::Null);
        for name in ["display", "authority"] {
            for value in [&mixed, &native] {
                let mut missing = value.clone();
                missing.as_object_mut().unwrap().remove(name);
                assert!(serde_json::from_value::<SharedGuiEnvironment>(missing).is_err());
                let mut empty = value.clone();
                empty[name] = "".into();
                assert!(
                    serde_json::from_value::<SharedGuiEnvironment>(empty)
                        .unwrap()
                        .validate()
                        .is_err()
                );
            }
        }
    }
    #[test]
    fn bus_fallback_is_bounded_local_data_and_absent_from_legacy_serialization() {
        let mut env = environment();
        assert!(
            serde_json::to_value(&env)
                .unwrap()
                .get("fallbackBus")
                .is_none()
        );
        for address in [
            "unix:path=/tmp/owned/bus",
            "unix:path=/tmp/owned/bus,guid=0123456789abcdef0123456789abcdef",
        ] {
            env.fallback_bus = Some(address.into());
            env.validate().unwrap();
            assert_eq!(env.fallback_bus_socket(), Some("/tmp/owned/bus"));
            assert!(
                !env.shell_assignments()
                    .unwrap()
                    .contains_key("DBUS_SESSION_BUS_ADDRESS")
            );
        }
        for address in [
            "",
            "tcp:host=example",
            "unix:abstract=bus",
            "unix:path=relative",
            "unix:path=/tmp/../bus",
            "unix:path=/tmp/bus;unix:path=/other",
            "unix:path=/tmp/bus,guid=bad",
            "unix:path=/tmp/a$(bad)",
        ] {
            env.fallback_bus = Some(address.into());
            assert!(env.validate().is_err(), "{address}");
        }
    }

    #[test]
    fn application_policy_reaches_shells_and_is_absent_for_older_readers() {
        let mut env = environment();
        assert!(
            serde_json::to_value(&env)
                .unwrap()
                .get("application")
                .is_none()
        );
        env.application = [
            ("XDG_CURRENT_DESKTOP".into(), Some("X-Newport".into())),
            (
                "XDG_DATA_DIRS".into(),
                Some("/usr/share:/var/lib/snapd/desktop".into()),
            ),
            ("MOZ_ENABLE_WAYLAND".into(), Some("1".into())),
            ("ELECTRON_OZONE_PLATFORM_HINT".into(), None),
        ]
        .into();
        let assignments = env.shell_assignments().unwrap();
        assert_eq!(
            assignments["XDG_CURRENT_DESKTOP"].as_deref(),
            Some("X-Newport")
        );
        assert_eq!(assignments["MOZ_ENABLE_WAYLAND"].as_deref(), Some("1"));
        assert_eq!(assignments["ELECTRON_OZONE_PLATFORM_HINT"], None);
        assert_eq!(assignments["GTK_IM_MODULE"], None);
        let wire = serde_json::to_value(&env).unwrap();
        assert_eq!(wire["application"]["XDG_CURRENT_DESKTOP"], "X-Newport");
        assert_eq!(
            serde_json::from_value::<SharedGuiEnvironment>(wire).unwrap(),
            env
        );
        for (name, value) in [
            ("PATH", Some("/tmp/evil")),
            ("DISPLAY", Some(":9")),
            ("GTK_IM_MODULE", None),
            ("XDG_DATA_DIRS", Some("relative")),
            ("MOZ_ENABLE_WAYLAND", Some("")),
        ] {
            let mut invalid = env.clone();
            invalid
                .application
                .insert(name.into(), value.map(str::to_owned));
            assert!(invalid.validate().is_err(), "{name}");
        }
        // Application names are not toolkit settings on older wires.
        let mut toolkit = environment();
        toolkit
            .toolkit
            .insert("XDG_CURRENT_DESKTOP".into(), Some("X-Newport".into()));
        assert!(toolkit.validate().is_err());
    }

    #[test]
    fn frozen_version_two_reader_rejects_new_bus_fields_and_native_endpoints() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        #[allow(dead_code)]
        struct VersionTwo {
            session: uuid::Uuid,
            display: String,
            authority: String,
            wayland_socket: String,
            toolkit: BTreeMap<String, Option<String>>,
        }
        let mut env = environment();
        let original = serde_json::to_value(&env).unwrap();
        assert!(serde_json::from_value::<VersionTwo>(original.clone()).is_ok());
        env.fallback_bus = Some("unix:path=/tmp/session/bus".into());
        assert!(serde_json::from_value::<VersionTwo>(serde_json::to_value(&env).unwrap()).is_err());
        env.fallback_bus = None;
        env.application = [("MOZ_ENABLE_WAYLAND".into(), Some("1".into()))].into();
        assert!(serde_json::from_value::<VersionTwo>(serde_json::to_value(&env).unwrap()).is_err());
        env.application.clear();
        env.display = None;
        env.authority = None;
        assert!(serde_json::from_value::<VersionTwo>(serde_json::to_value(&env).unwrap()).is_err());
        let mut unknown = original;
        unknown["futurePolicy"] = "never silently accepted".into();
        assert!(serde_json::from_value::<SharedGuiEnvironment>(unknown).is_err());
    }
}
