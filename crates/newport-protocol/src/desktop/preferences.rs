//! Typed desktop preferences contracts.
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use std::ops::RangeInclusive;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Auto,
    Lossless,
    Fast,
}

/// The sidebar widths a window may keep.
pub const SIDEBAR_WIDTH: RangeInclusive<f32> = 176.0..=300.0;

/// The window's own choices, under the keys preferences.json keeps them with.
/// An absent field is unset: a patch leaves it as it is, and a window reading
/// the file uses its default. Reading takes each key on its own, so a value of
/// the wrong type, or a width outside [`SIDEBAR_WIDTH`], reads as unset
/// without losing the others.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "stored_width"
    )]
    pub sidebar_width: Option<f32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "stored"
    )]
    pub sidebar_compact: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "stored"
    )]
    pub appearance: Option<crate::desktop::preferences::Appearance>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "stored"
    )]
    pub terminal_option_as_meta: Option<bool>,
}

fn stored<'de, D: Deserializer<'de>, T: DeserializeOwned>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Ok(serde_json::from_value(serde_json::Value::deserialize(deserializer)?).ok())
}

/// The width is checked before narrowing, so a stored value just outside the
/// range can't round into it.
fn stored_width<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f32>, D::Error> {
    let range = f64::from(*SIDEBAR_WIDTH.start())..=f64::from(*SIDEBAR_WIDTH.end());
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_f64()
        .filter(|width| range.contains(width))
        .map(|width| width as f32))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: uuid::Uuid,
    pub server: uuid::Uuid,
    pub name: String,
    pub path: crate::git::Path,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projects {
    pub projects: Vec<crate::desktop::preferences::Project>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub audio: bool,
    pub codec: crate::desktop::preferences::Codec,
    pub fps_goal: u32,
    pub hud: bool,
    pub separate_apps: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub servers: Vec<uuid::Uuid>,
}

#[derive(Clone, Debug)]
pub struct SetWindowArgs {
    pub patch: crate::desktop::preferences::WindowPatch,
}

#[derive(Clone, Debug)]
pub struct ProjectsListArgs {
    pub server: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct ProjectsSaveArgs {
    pub project: crate::desktop::preferences::Project,
}

#[derive(Clone, Debug)]
pub struct ProjectsRemoveArgs {
    pub server: uuid::Uuid,
    pub project: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct SettingsSaveArgs {
    pub settings: crate::desktop::preferences::Settings,
}

#[derive(Clone, Debug)]
pub struct InstalledSaveArgs {
    pub server: uuid::Uuid,
    pub installed: bool,
}

#[derive(Clone, Debug)]
pub enum Operation {
    SetWindow(Box<crate::desktop::preferences::SetWindowArgs>),
    ProjectsList(Box<crate::desktop::preferences::ProjectsListArgs>),
    ProjectsSave(Box<crate::desktop::preferences::ProjectsSaveArgs>),
    ProjectsRemove(Box<crate::desktop::preferences::ProjectsRemoveArgs>),
    Settings(Box<crate::common::Present>),
    SettingsSave(Box<crate::desktop::preferences::SettingsSaveArgs>),
    Installed(Box<crate::common::Present>),
    InstalledSave(Box<crate::desktop::preferences::InstalledSaveArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    SetWindow(Box<crate::common::Present>),
    ProjectsList(Box<crate::desktop::preferences::Projects>),
    ProjectsSave(Box<crate::desktop::preferences::Project>),
    ProjectsRemove(Box<crate::common::Present>),
    Settings(Box<crate::desktop::preferences::Settings>),
    SettingsSave(Box<crate::desktop::preferences::Settings>),
    Installed(Box<crate::desktop::preferences::Installed>),
    InstalledSave(Box<crate::desktop::preferences::Installed>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn read(file: Value) -> WindowPatch {
        serde_json::from_value(file).unwrap()
    }

    #[test]
    fn a_window_patch_writes_the_stored_keys() {
        let patch = WindowPatch {
            sidebar_width: Some(245.5),
            sidebar_compact: Some(true),
            appearance: Some(Appearance::Dark),
            terminal_option_as_meta: Some(false),
        };
        assert_eq!(
            serde_json::to_string(&patch).unwrap(),
            r#"{"sidebarWidth":245.5,"sidebarCompact":true,"appearance":"dark","terminalOptionAsMeta":false}"#
        );
        assert_eq!(
            serde_json::to_string(&WindowPatch::default()).unwrap(),
            "{}"
        );
        for (appearance, stored) in [
            (Appearance::System, "system"),
            (Appearance::Light, "light"),
            (Appearance::Dark, "dark"),
        ] {
            let patch = WindowPatch {
                appearance: Some(appearance),
                ..Default::default()
            };
            assert_eq!(
                serde_json::to_value(&patch).unwrap(),
                json!({"appearance": stored})
            );
        }
    }

    #[test]
    fn an_existing_preferences_file_reads_its_window_keys() {
        let file = json!({
            "sidebarWidth": 240,
            "sidebarCompact": true,
            "appearance": "light",
            "terminalOptionAsMeta": false,
            "gitClientId": "1c0e5a4e-7f63-4a8a-9d8a-2f3b9c1d4e5f",
            "gitProjects": [],
            "remoteAppsSettings": {"audio": true},
            "remoteAppsInstalled": [],
        });
        assert_eq!(
            read(file),
            WindowPatch {
                sidebar_width: Some(240.0),
                sidebar_compact: Some(true),
                appearance: Some(Appearance::Light),
                terminal_option_as_meta: Some(false),
            }
        );
        assert_eq!(read(json!({})), WindowPatch::default());
    }

    #[test]
    fn invalid_stored_values_read_as_unset_one_key_at_a_time() {
        let valid = json!({"sidebarWidth": 200.0, "sidebarCompact": true,
            "appearance": "dark", "terminalOptionAsMeta": false});
        for (key, invalid) in [
            ("sidebarWidth", json!(175.9)),
            ("sidebarWidth", json!(300.000001)),
            ("sidebarWidth", json!("200")),
            ("sidebarWidth", Value::Null),
            ("sidebarCompact", json!("yes")),
            ("sidebarCompact", json!(1)),
            ("appearance", json!("blue")),
            ("appearance", json!("Dark")),
            ("terminalOptionAsMeta", json!("yes")),
            ("terminalOptionAsMeta", Value::Null),
        ] {
            let mut file = valid.clone();
            file[key] = invalid.clone();
            let patch = read(file);
            let mut expected = read(valid.clone());
            match key {
                "sidebarWidth" => expected.sidebar_width = None,
                "sidebarCompact" => expected.sidebar_compact = None,
                "appearance" => expected.appearance = None,
                _ => expected.terminal_option_as_meta = None,
            }
            assert_eq!(patch, expected, "{key}: {invalid}");
        }
        for width in [176, 300] {
            assert_eq!(
                read(json!({"sidebarWidth": width})).sidebar_width,
                Some(width as f32)
            );
        }
    }
}
