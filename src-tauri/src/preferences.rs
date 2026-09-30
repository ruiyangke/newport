//! Typed preference commands keep profile data out of the unencrypted Store plugin.
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tauri::{Manager, State};

pub(crate) mod projects;
// Serialize edits and saves, including project rollback after a failed save.
static ACCESS: Mutex<()> = Mutex::new(());

pub struct Preferences(Arc<tauri_plugin_store::Store<tauri::Wry>>);

pub fn install(app: &tauri::App, directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let path = directory.join("preferences.json");
    // The plugin silently defaults after a load error; preserve malformed files instead.
    if path.exists() {
        let _: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&std::fs::read(&path)?)?;
    }
    let store = tauri_plugin_store::StoreBuilder::new(app, path)
        .disable_auto_save()
        .build()?;
    app.manage(Preferences(store));
    Ok(())
}

#[tauri::command]
pub fn get_sidebar_width(state: State<'_, Preferences>) -> Option<f64> {
    state
        .0
        .get("sidebarWidth")
        .and_then(|v| v.as_f64())
        .filter(|v| (176.0..=300.0).contains(v))
}

#[tauri::command]
pub async fn set_sidebar_width(state: State<'_, Preferences>, width: f64) -> Result<(), String> {
    if !width.is_finite() || !(176.0..=300.0).contains(&width) {
        return Err("Sidebar width must be between 176 and 300".into());
    }
    let store = state.0.clone();
    tokio::task::spawn_blocking(move || {
        let _guard = ACCESS
            .lock()
            .map_err(|_| "Preference storage is unavailable.")?;
        store.set("sidebarWidth", serde_json::json!(width));
        store.save().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupSettings {
    enabled: bool,
    available: bool,
}

#[tauri::command]
pub fn get_startup_settings(app: tauri::AppHandle) -> Result<StartupSettings, String> {
    use tauri_plugin_autostart::ManagerExt;
    Ok(StartupSettings {
        enabled: app.autolaunch().is_enabled().map_err(|e| e.to_string())?,
        available: !cfg!(debug_assertions) && crate::migration::data_directory_override().is_none(),
    })
}

#[tauri::command]
pub fn set_launch_at_login(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<StartupSettings, String> {
    use tauri_plugin_autostart::ManagerExt;
    if !get_startup_settings(app.clone())?.available {
        return Err("Launch at login is available in the installed app.".into());
    }
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|e| e.to_string())?;
    get_startup_settings(app)
}

impl Preferences {
    /// Caller serializes initial creation. Always save before exposing this identity.
    pub(crate) fn git_client_id(&self) -> Result<String, String> {
        let _guard = ACCESS
            .lock()
            .map_err(|_| "Preference storage is unavailable.")?;
        let id = match self.0.get("gitClientId") {
            Some(value) => value
                .as_str()
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
                .ok_or("Stored Git client identity is invalid.")?,
            None => uuid::Uuid::new_v4(),
        };
        self.0.set("gitClientId", serde_json::json!(id.to_string()));
        self.0.save().map_err(|e| e.to_string())?;
        Ok(id.to_string())
    }
}
