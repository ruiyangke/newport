//! Remote apps preferences: streaming settings for every server, and the
//! servers last seen with the agent installed.
//! Nothing here is secret.
use super::Preferences;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use uuid::Uuid;

const SETTINGS: &str = "remoteAppsSettings";
/// Servers last seen with the agent installed, so a first visit after
/// starting Newport can show their apps loading instead of the welcome screen.
const INSTALLED: &str = "remoteAppsInstalled";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    #[default]
    Auto,
    Lossless,
    Fast,
}

/// Stored, so it reads what an older or newer build wrote: unknown fields
/// are ignored and missing ones take their default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub audio: bool,
    pub codec: Codec,
    /// 15, 30 or 60.
    pub fps_goal: u32,
    pub hud: bool,
    /// Each remote app is an app of its own: a Mac app, or a Windows taskbar
    /// button with the app's icon. On where the desktop supports it.
    pub separate_apps: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            audio: true,
            codec: Codec::Auto,
            fps_goal: 60,
            hud: false,
            separate_apps: cfg!(any(target_os = "macos", windows)),
        }
    }
}

fn validate_settings(settings: &Settings) -> Result<(), String> {
    if ![15, 30, 60].contains(&settings.fps_goal) {
        return Err("Frame rate must be 15, 30 or 60 fps.".into());
    }
    Ok(())
}

fn decode_settings(value: Option<serde_json::Value>) -> Result<Settings, String> {
    let Some(value) = value else {
        return Ok(Settings::default());
    };
    let settings: Settings = serde_json::from_value(value)
        .map_err(|e| format!("Remote apps settings could not be read: {e}"))?;
    validate_settings(&settings)?;
    Ok(settings)
}

/// The settings in freshly read preferences, or the defaults when they can't
/// be read: a session that needs them reports why when it starts.
pub(super) fn stored(values: &mut serde_json::Map<String, serde_json::Value>) -> Settings {
    decode_settings(values.remove(SETTINGS)).unwrap_or_default()
}

/// Drops a deleted server's installed-agent record.
pub(super) fn forget_server(
    values: &mut serde_json::Map<String, serde_json::Value>,
    server_id: Uuid,
) -> Result<(), String> {
    let mut installed = decode_installed(values.get(INSTALLED).cloned())?;
    installed.retain(|server| *server != server_id);
    values.insert(
        INSTALLED.into(),
        serde_json::to_value(&installed).map_err(|e| e.to_string())?,
    );
    Ok(())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

pub async fn remote_apps_settings(state: &Preferences) -> Result<Settings, String> {
    let state = state.clone();
    blocking(move || decode_settings(state.get(SETTINGS)?)).await
}

pub async fn remote_apps_settings_save(
    state: &Preferences,
    settings: Settings,
) -> Result<Settings, String> {
    validate_settings(&settings)?;
    let stored = state.clone();
    let saved = blocking(move || {
        stored.update(|values| {
            values.insert(
                SETTINGS.into(),
                serde_json::to_value(&settings).map_err(|e| e.to_string())?,
            );
            Ok(settings)
        })
    })
    .await?;
    state.remote_apps.send_replace(saved.clone());
    Ok(saved)
}

/// The stored settings now and after each save.
pub fn remote_apps_settings_changes(state: &Preferences) -> watch::Receiver<Settings> {
    state.remote_apps.subscribe()
}

/// Tells the desktop the stored settings: before returning, then after each
/// save until the preferences are gone.
pub fn publish(
    state: &Preferences,
    desktop: crate::desktop_integration::Desktop,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    let mut settings = remote_apps_settings_changes(state);
    desktop.remote_apps_settings(&settings.borrow_and_update().clone());
    async move {
        while settings.changed().await.is_ok() {
            let current = settings.borrow_and_update().clone();
            desktop.remote_apps_settings(&current);
        }
    }
}

fn decode_installed(value: Option<serde_json::Value>) -> Result<Vec<Uuid>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    serde_json::from_value(value).map_err(|e| format!("Remote apps servers could not be read: {e}"))
}

/// Servers last seen with the agent installed.
pub async fn remote_apps_installed(state: &Preferences) -> Result<Vec<Uuid>, String> {
    let state = state.clone();
    blocking(move || decode_installed(state.get(INSTALLED)?)).await
}

/// Record whether a server has the agent; returns every such server.
pub async fn remote_apps_installed_save(
    state: &Preferences,
    server_id: Uuid,
    installed: bool,
) -> Result<Vec<Uuid>, String> {
    let state = state.clone();
    blocking(move || {
        state.update(|values| {
            let mut servers = decode_installed(values.get(INSTALLED).cloned())?;
            servers.retain(|server| *server != server_id);
            if installed {
                servers.push(server_id);
            }
            values.insert(
                INSTALLED.into(),
                serde_json::to_value(&servers).map_err(|e| e.to_string())?,
            );
            Ok(servers)
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_from_another_build_still_read() {
        let settings: Settings =
            serde_json::from_value(serde_json::json!({"audio": false, "future": 1})).unwrap();
        assert_eq!(
            settings,
            Settings {
                audio: false,
                ..Settings::default()
            }
        );
    }

    #[tokio::test]
    async fn installed_servers_are_remembered_and_forgotten() {
        let directory = tempfile::tempdir().unwrap();
        let state = Preferences::open(directory.path()).unwrap();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(remote_apps_installed(&state).await.unwrap().is_empty());
        remote_apps_installed_save(&state, a, true).await.unwrap();
        // Saving twice keeps one entry.
        assert_eq!(
            remote_apps_installed_save(&state, a, true).await.unwrap(),
            [a]
        );
        remote_apps_installed_save(&state, b, true).await.unwrap();
        assert_eq!(
            remote_apps_installed_save(&state, a, false).await.unwrap(),
            [b]
        );
        let reopened = Preferences::open(directory.path()).unwrap();
        assert_eq!(remote_apps_installed(&reopened).await.unwrap(), [b]);
    }

    #[tokio::test]
    async fn saved_settings_reach_observers() {
        let directory = tempfile::tempdir().unwrap();
        let state = Preferences::open(directory.path()).unwrap();
        let mut changes = remote_apps_settings_changes(&state);
        assert_eq!(
            changes.borrow_and_update().separate_apps,
            cfg!(any(target_os = "macos", windows))
        );
        let separate = Settings {
            separate_apps: !Settings::default().separate_apps,
            ..Settings::default()
        };
        remote_apps_settings_save(&state, separate.clone())
            .await
            .unwrap();
        assert!(changes.has_changed().unwrap());
        assert_eq!(*changes.borrow_and_update(), separate);
        // A rejected save changes nothing.
        let invalid = Settings {
            fps_goal: 24,
            ..Settings::default()
        };
        assert!(remote_apps_settings_save(&state, invalid).await.is_err());
        assert!(!changes.has_changed().unwrap());
        // Opening the profile again starts from what was stored.
        let reopened = Preferences::open(directory.path()).unwrap();
        assert_eq!(*remote_apps_settings_changes(&reopened).borrow(), separate);
    }

    #[tokio::test]
    async fn settings_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let state = Preferences::open(directory.path()).unwrap();
        assert_eq!(
            remote_apps_settings(&state).await.unwrap(),
            Settings::default()
        );
        let quiet = Settings {
            audio: false,
            fps_goal: 30,
            separate_apps: true,
            ..Settings::default()
        };
        remote_apps_settings_save(&state, quiet.clone())
            .await
            .unwrap();
        assert_eq!(remote_apps_settings(&state).await.unwrap(), quiet);
        assert!(
            remote_apps_settings_save(
                &state,
                Settings {
                    fps_goal: 24,
                    ..Settings::default()
                }
            )
            .await
            .is_err()
        );
    }
}
