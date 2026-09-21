use tauri::{
    menu::{Menu, MenuItemBuilder, PredefinedMenuItem, Submenu},
    Emitter, Manager,
};

pub fn install(app: &tauri::App) -> tauri::Result<()> {
    let menu = Menu::default(app.handle())?;
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
    let server = Submenu::with_items(
        app,
        "Server",
        true,
        &[
            &MenuItemBuilder::with_id("server-new", "Add Server…")
                .accelerator("CmdOrCtrl+N")
                .build(app)?,
            &MenuItemBuilder::with_id("server-edit", "Edit Server…")
                .accelerator("CmdOrCtrl+Shift+E")
                .build(app)?,
            &MenuItemBuilder::with_id("server-test", "Test Connection")
                .accelerator("CmdOrCtrl+Shift+T")
                .build(app)?,
        ],
    )?;
    let workspace = Submenu::new(app, "Workspace", true)?;
    for (index, (id, title)) in [
        ("overview", "Overview"),
        ("connections", "Connections"),
        ("services", "Services"),
        ("containers", "Containers"),
        ("commands", "Commands"),
        ("files", "Files"),
    ]
    .iter()
    .enumerate()
    {
        workspace.append(
            &MenuItemBuilder::with_id(format!("view-{id}"), *title)
                .accelerator(format!("CmdOrCtrl+{}", index + 1))
                .build(app)?,
        )?;
    }
    workspace.append(&PredefinedMenuItem::separator(app)?)?;
    workspace.append(
        &MenuItemBuilder::with_id("sidebar-toggle", "Show/Hide Sidebar")
            .accelerator("CmdOrCtrl+Shift+L")
            .build(app)?,
    )?;
    menu.insert(&server, 2)?;
    menu.insert(&workspace, 4)?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        let id = event.id.as_ref();
        if id.starts_with("server-")
            || id.starts_with("view-")
            || id == "sidebar-toggle"
            || id == "app-settings"
        {
            if let Some(window) = app.get_webview_window("main") {
                crate::app::show(app);
                let _ = window.emit("workspace-action", id);
            }
        }
    });
    Ok(())
}
