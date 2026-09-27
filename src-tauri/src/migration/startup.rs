use std::path::Path;

/// Move an enabled legacy login item only after the new item is verified.
pub fn migrate(app: &tauri::App, directory: &Path) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    if cfg!(debug_assertions) || super::data_directory_override().is_some() {
        return Ok(());
    }
    let marker = directory.join("newport-startup-migrated");
    if marker.exists() {
        return Ok(());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut legacy = auto_launch::AutoLaunchBuilder::new();
    legacy
        .set_app_name(super::legacy::APP_NAME)
        .set_app_path(&executable.to_string_lossy())
        .set_args(&["--autostart"]);
    #[cfg(target_os = "macos")]
    legacy.set_use_launch_agent(true);
    let legacy = legacy.build().map_err(|e| e.to_string())?;
    if legacy.is_enabled().map_err(|e| e.to_string())? {
        app.autolaunch().enable().map_err(|e| e.to_string())?;
        if !app.autolaunch().is_enabled().map_err(|e| e.to_string())? {
            return Err(
                "Cannot verify Newport’s login item; the previous item was preserved.".into(),
            );
        }
        legacy.disable().map_err(|e| e.to_string())?;
    }
    std::fs::write(marker, b"newport-v1\n").map_err(|e| e.to_string())
}
