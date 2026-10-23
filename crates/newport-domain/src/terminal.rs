/// Product resource bound, independent of sshd's per-connection session limit.
pub const MAX_TERMINALS: usize = 32;

use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct GuiEnvironment(pub BTreeMap<String, Option<String>>);
impl GuiEnvironment {
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
    pub fn validate(&self) -> Result<(), String> {
        if self.0.len() > 24
            || self.0.iter().any(|(key, value)| {
                !Self::allowed(key)
                    || value
                        .as_ref()
                        .is_some_and(|v| v.len() > 4096 || v.contains('\0'))
            })
        {
            return Err("Invalid terminal display environment".into());
        }
        let display = self
            .0
            .get("DISPLAY")
            .and_then(Option::as_deref)
            .unwrap_or_default();
        let authority = self
            .0
            .get("XAUTHORITY")
            .and_then(Option::as_deref)
            .unwrap_or_default();
        if !display.starts_with(':')
            || display.len() < 2
            || !display[1..].bytes().all(|b| b.is_ascii_digit())
            || !authority.starts_with('/')
            || self.0.get("WAYLAND_DISPLAY") != Some(&None)
            || self.0.get("WAYLAND_SOCKET") != Some(&None)
        {
            return Err("Invalid private X11 terminal display".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod gui_tests {
    use super::*;
    fn environment() -> GuiEnvironment {
        GuiEnvironment(
            [
                ("DISPLAY", Some(":75")),
                ("XAUTHORITY", Some("/tmp/private/Xauthority")),
                ("WAYLAND_DISPLAY", None),
                ("WAYLAND_SOCKET", None),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.map(str::to_owned)))
            .collect(),
        )
    }
    #[test]
    fn terminal_environment_rejects_executable_and_foreign_display_settings() {
        let valid = environment();
        valid.validate().unwrap();
        for (key, value) in [
            ("PATH", "/attacker"),
            ("LD_PRELOAD", "/attacker.so"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/foreign-bus"),
            ("DISPLAY", "remote:0"),
            ("DISPLAY", ":1;evil"),
            ("XAUTHORITY", "relative"),
            ("WAYLAND_DISPLAY", "clipboard-only"),
        ] {
            let mut bad = valid.clone();
            bad.0.insert(key.into(), Some(value.into()));
            assert!(bad.validate().is_err(), "accepted {key}");
        }
    }
}
