pub fn configure(app: &mut tauri::App) {
    app.set_activation_policy(tauri::ActivationPolicy::Regular);
}
pub fn handle_event(app: &tauri::AppHandle, event: &tauri::RunEvent) {
    if let tauri::RunEvent::Reopen { .. } = event {
        crate::app::show(app);
    }
}
pub fn autostart() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_autostart::Builder::new()
        .app_name("Porthop")
        .macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent)
        .arg("--autostart")
        .build()
}

pub async fn wait_for_termination(app: tauri::AppHandle) {
    if let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! { _ = terminate.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
        app.exit(0);
    }
}

pub fn configure_menu(
    app: &tauri::App,
    menu: &tauri::menu::Menu<tauri::Wry>,
    server: &tauri::menu::Submenu<tauri::Wry>,
    workspace: &tauri::menu::Submenu<tauri::Wry>,
) -> tauri::Result<()> {
    use tauri::menu::MenuItemBuilder;
    if let Some(app_menu) = menu
        .items()?
        .first()
        .and_then(|item| item.as_submenu())
        .cloned()
    {
        app_menu.insert(
            &MenuItemBuilder::with_id("app-settings", "Settings…")
                .accelerator("CmdOrCtrl+,")
                .build(app)?,
            2,
        )?;
    }
    menu.insert(server, 2)?;
    menu.insert(workspace, 4)?;
    Ok(())
}
