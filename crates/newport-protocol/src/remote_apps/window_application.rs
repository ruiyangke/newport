//! Which Linux application owns a window, so a client can group the windows of
//! one app.
use serde::{Deserialize, Serialize};

/// Longest `app_id`, in bytes. Reverse-DNS application IDs and D-Bus names
/// stay within it.
pub const MAX_APP_ID: usize = 255;
/// Longest `desktop_id`, in bytes: one file name.
pub const MAX_DESKTOP_ID: usize = 255;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Later fields are optional additions, so unknown ones are ignored as in
/// the window they belong to.
pub struct WindowApplication {
    /// The identity the app gives its window: Wayland `xdg_toplevel.set_app_id`;
    /// on X11 `_GTK_APPLICATION_ID`, else the `WM_CLASS` class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// The catalogue entry the agent attributes the window to, such as
    /// `org.gnome.Calculator.desktop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_id: Option<String>,
    /// The launch request whose processes own the window's
    /// client. Absent for a window of an instance that was already running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<u64>,
}
impl WindowApplication {
    /// None when nothing is known, so an unknown owner has one encoding.
    pub fn new(
        app_id: Option<String>,
        desktop_id: Option<String>,
        request: Option<u64>,
    ) -> Option<Self> {
        let application = Self {
            app_id,
            desktop_id,
            request,
        };
        (application != Self::default()).then_some(application)
    }
    pub fn validate(&self) -> Result<(), String> {
        if *self == Self::default() {
            return Err("Window application carries no identity".into());
        }
        if let Some(app_id) = &self.app_id {
            validate_app_id(app_id)?;
        }
        if let Some(desktop_id) = &self.desktop_id {
            validate_desktop_id(desktop_id)?;
        }
        if self.request == Some(0) {
            return Err("Invalid window application request".into());
        }
        Ok(())
    }
}
/// A window's application as a hint: one that doesn't parse, such as a later
/// version's of another shape, is no application rather than no window.
pub(crate) fn hint<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<WindowApplication>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok().flatten())
}
pub fn validate_app_id(app_id: &str) -> Result<(), String> {
    if app_id.is_empty() || app_id.len() > MAX_APP_ID || app_id.chars().any(char::is_control) {
        return Err("Invalid window application ID".into());
    }
    Ok(())
}
/// Desktop IDs become app names and file names on the client, so control
/// characters are refused here as for app IDs.
pub fn validate_desktop_id(desktop_id: &str) -> Result<(), String> {
    if desktop_id.len() > MAX_DESKTOP_ID || desktop_id.chars().any(char::is_control) {
        return Err("Invalid window desktop ID".into());
    }
    crate::remote_apps::apps::validate_id(desktop_id)
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    #[test]
    fn applications_round_trip_and_accept_later_fields() {
        let full = WindowApplication {
            app_id: Some("org.gnome.Calculator".into()),
            desktop_id: Some("org.gnome.Calculator.desktop".into()),
            request: Some(7),
        };
        full.validate().unwrap();
        let encoded = serde_json::to_value(&full).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({
                "app_id": "org.gnome.Calculator",
                "desktop_id": "org.gnome.Calculator.desktop",
                "request": 7,
            })
        );
        assert_eq!(
            serde_json::from_value::<WindowApplication>(encoded).unwrap(),
            full
        );
        let only_class = WindowApplication::new(Some("xterm".into()), None, None).unwrap();
        assert_eq!(
            serde_json::to_string(&only_class).unwrap(),
            r#"{"app_id":"xterm"}"#
        );
        assert_eq!(WindowApplication::new(None, None, None), None);
        // A later version's fields don't break this one.
        assert_eq!(
            serde_json::from_str::<WindowApplication>(r#"{"app_id":"a","pid":1}"#).unwrap(),
            WindowApplication::new(Some("a".into()), None, None).unwrap()
        );
    }
    #[test]
    fn desktop_ids_with_control_characters_are_refused() {
        assert!(validate_desktop_id("calc.desktop").is_ok());
        assert!(validate_desktop_id("calc\n.desktop").is_err());
        assert!(validate_desktop_id("\u{1b}[31mcalc.desktop").is_err());
    }
    #[test]
    fn window_metadata_carries_the_application_only_when_known() {
        use crate::remote_apps::v1::Window;
        let without_application = serde_json::json!({
            "key": {"id": 1, "generation": 1},
            "parent": null,
            "kind": "normal",
            "modal": false,
            "position": [0, 0],
            "title": "Calculator",
            "decoration": "client",
            "actions": {"close": true, "minimize": true, "maximize": true, "resize": true},
            "constraints": {"min": null, "max": null},
        });
        let mut window: Window = serde_json::from_value(without_application).unwrap();
        assert_eq!(window.application, None);
        window.validate().unwrap();
        assert!(
            serde_json::to_value(&window)
                .unwrap()
                .get("application")
                .is_none()
        );
        window.application = WindowApplication::new(
            Some("org.gnome.Calculator".into()),
            Some("org.gnome.Calculator.desktop".into()),
            Some(3),
        );
        window.validate().unwrap();
        let decoded: Window =
            serde_json::from_value(serde_json::to_value(&window).unwrap()).unwrap();
        assert_eq!(decoded, window);
        window.application = Some(WindowApplication {
            request: Some(0),
            ..Default::default()
        });
        assert!(window.validate().is_err());
        // An application of another shape leaves the window without one.
        let mut later = serde_json::to_value(&window).unwrap();
        later["application"] = serde_json::json!({"app_id": ["a", "b"], "request": "7"});
        let decoded: Window = serde_json::from_value(later).unwrap();
        assert_eq!(decoded.application, None);
        assert_eq!(decoded.title, window.title);
    }
    #[test]
    fn identities_are_bounded() {
        let app = |app_id: String| WindowApplication {
            app_id: Some(app_id),
            ..Default::default()
        };
        let desktop = |desktop_id: String| WindowApplication {
            desktop_id: Some(desktop_id),
            ..Default::default()
        };
        app("a".repeat(MAX_APP_ID)).validate().unwrap();
        let suffix = ".desktop";
        desktop(format!(
            "{}{suffix}",
            "a".repeat(MAX_DESKTOP_ID - suffix.len())
        ))
        .validate()
        .unwrap();
        for invalid in [
            WindowApplication::default(),
            app(String::new()),
            app("a".repeat(MAX_APP_ID + 1)),
            app("line\nbreak".into()),
            app("nul\0".into()),
            desktop(format!("{}{suffix}", "a".repeat(MAX_DESKTOP_ID))),
            desktop("calculator".into()),
            desktop("sub/dir.desktop".into()),
            WindowApplication {
                request: Some(0),
                ..Default::default()
            },
        ] {
            assert!(invalid.validate().is_err(), "{invalid:?}");
        }
    }
}
