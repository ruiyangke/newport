pub fn configure(_: &mut tauri::App) {}
pub fn handle_event(_: &tauri::AppHandle, _: &tauri::RunEvent) {}
pub fn autostart() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_autostart::Builder::new()
        .app_name("Porthop")
        .arg("--autostart")
        .build()
}
pub async fn wait_for_termination(app: tauri::AppHandle) {
    if tokio::signal::ctrl_c().await.is_ok() {
        app.exit(0);
    }
}
pub fn configure_menu(
    app: &tauri::App,
    menu: &tauri::menu::Menu<tauri::Wry>,
    server: &tauri::menu::Submenu<tauri::Wry>,
    workspace: &tauri::menu::Submenu<tauri::Wry>,
) -> tauri::Result<()> {
    use tauri::menu::{MenuItemBuilder, Submenu};
    menu.append(server)?;
    menu.append(workspace)?;
    menu.append(&Submenu::with_items(
        app,
        "Settings",
        true,
        &[&MenuItemBuilder::with_id("app-settings", "Settings…")
            .accelerator("Ctrl+,")
            .build(app)?],
    )?)?;
    Ok(())
}

pub fn install_menu(_: &tauri::App, _: tauri::menu::Menu<tauri::Wry>) -> tauri::Result<()> {
    // Windows actions and shortcuts live in the shared webview shell. Attaching
    // a hidden menu disables its accelerators and adds inaccessible menu items.
    Ok(())
}
