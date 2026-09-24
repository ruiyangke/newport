//! Desktop startup, single-instance ownership, background workers and shutdown.
use crate::{
    commands,
    config::Store,
    manager::{self, Manager, Shared},
    model::Status,
};
use anyhow::Context;
use fs2::FileExt;
use std::{
    fs::OpenOptions,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager as _,
};

pub(crate) fn show(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
pub fn run() -> anyhow::Result<()> {
    let store = Store::open()
        .map_err(anyhow::Error::msg)
        .context("Cannot open Porthop profile directory")?;
    // Same lock location as the Swift app, unless running an isolated test profile.
    let lock_path = if std::env::var_os("PORTHOP_DATA_DIR").is_some() {
        store.directory.join("porthop.lock")
    } else {
        std::env::temp_dir().join("porthop.lock")
    };
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .context("Cannot open Porthop instance lock")?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            eprintln!("Porthop is already running. Open it from the Dock or menu bar.");
            return Ok(());
        }
        Err(error) => return Err(error).context("Cannot acquire Porthop instance lock"),
    }
    let logging = crate::plugins::logging(&store.directory);
    let profile_directory = store.directory.clone();
    let manager = Manager::new(store);
    crate::credentials::initialize(manager.store.clone());
    let key = match &manager.load_error {
        Some(error) => Err(error.clone()),
        None => manager.store.vault_key().map(|key| key.to_vec()),
    };
    let metrics_url = if key.is_ok() {
        crate::metrics_db::database_url(&profile_directory.join("metrics.sqlite3"))
            .map_err(anyhow::Error::msg)?
    } else {
        String::new()
    };
    let state: Shared = Arc::new(tokio::sync::Mutex::new(manager));
    let background_state = state.clone();
    let sampler = crate::metrics_sampler::Sampler::default();
    let sampling_state = state.clone();
    let sampling_worker = sampler.clone();
    let shutdown_complete = Arc::new(AtomicBool::new(false));
    let shutdown_started = Arc::new(AtomicBool::new(false));
    let workers = Arc::new(std::sync::Mutex::new(Vec::<
        tauri::async_runtime::JoinHandle<()>,
    >::new()));
    let setup_workers = workers.clone();
    let window_state = tauri_plugin_window_state::Builder::default()
        .with_state_flags(
            tauri_plugin_window_state::StateFlags::SIZE
                | tauri_plugin_window_state::StateFlags::POSITION
                | tauri_plugin_window_state::StateFlags::MAXIMIZED,
        )
        .build();
    let mut context = tauri::generate_context!();
    // Preload in Rust: SQL initialization must not depend on a visible webview.
    context.config_mut().plugins.0.insert(
        "sql".into(),
        serde_json::json!({
            "preload": if key.is_ok() { vec![metrics_url.clone()] } else { vec![] }
        }),
    );
    if std::env::args().any(|arg| arg == "--autostart") {
        for window in &mut context.config_mut().app.windows {
            window.visible = false;
        }
    }
    let app = tauri::Builder::default()
        .plugin(logging)
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_sql::Builder::default()
            .add_migrations(&metrics_url, crate::metrics_db::migrations()).build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_stronghold::Builder::with_argon2(&profile_directory.join("vault.salt")).build())
        .plugin(tauri_plugin_autostart::Builder::new()
            .app_name("Porthop")
            .macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent)
            .arg("--autostart")
            .build())
        .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
        .plugin(window_state)
        .manage(state.clone())
        .manage(sampler)
        .manage(crate::updates::Updates::new(env!("CARGO_PKG_VERSION").into()))
        .manage(crate::terminal::Sessions::default())
        .manage(crate::files::Operations::default())
        .invoke_handler(tauri::generate_handler![
            crate::updates::update_status,
            crate::updates::check_for_updates,
            crate::updates::install_update,
            crate::files::files_list,
            crate::files::files_preview,
            crate::files::transfer::files_upload,
            crate::files::transfer::files_download,
            crate::files::operations::files_cancel,
            crate::files::operations::files_progress,
            crate::preferences::get_startup_settings,
            crate::preferences::set_launch_at_login,
            crate::preferences::get_sidebar_width,
            crate::preferences::set_sidebar_width,
            commands::ssh_agent_keys,
            commands::cockpit_collect,
            commands::cockpit_history,
            commands::get_metrics_cache,
            commands::clear_metrics_cache,
            commands::cockpit_project_action,
            commands::cockpit_logs,
            commands::terminal_open,
            commands::terminal_read,
            commands::terminal_write,
            commands::terminal_resize,
            commands::terminal_close,
            commands::snapshot,
            commands::save_server,
            commands::delete_server,
            commands::save_tunnel,
            commands::delete_tunnel,
            commands::set_tunnel_connected,
            commands::set_clipboard_enabled,
            commands::set_integration_enabled,
            commands::test_connection,
            commands::discover_ports,
            commands::install_clipboard_helper,
            commands::reinstall_agent,
            commands::open_tunnel
        ])
        .setup(move |app| {
            let sampling_db = tauri::async_runtime::block_on(
                crate::metrics_db::MetricsDb::from_plugin(app.handle(), &metrics_url, key)
            ).map_err(anyhow::Error::msg)?;
            app.manage(sampling_db.clone());
            app.set_activation_policy(tauri::ActivationPolicy::Regular);
            crate::preferences::install(app, &profile_directory)?;
            crate::desktop::install(app)?;
            if let Err(error) = crate::system_events::install() {
                log::warn!("System event monitoring unavailable; using timed recovery: {error}");
            }
            let open = MenuItem::with_id(app, "open", "Open Porthop", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Porthop", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "app-settings", "Settings…", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &settings, &quit])?;
            TrayIconBuilder::with_id("porthop")
                .tooltip("Porthop · SSH tunnels")
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/tray.png"
                ))?)
                .icon_as_template(true)
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "open" => show(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            let observations = crate::connectivity::subscribe();
            setup_workers.lock().unwrap().push(tauri::async_runtime::spawn(
                manager::connection_events(state.clone(), observations),
            ));
            setup_workers
                .lock()
                .unwrap()
                .push(tauri::async_runtime::spawn(manager::background(
                    background_state, app.handle().clone(),
                )));
            setup_workers.lock().unwrap().push(tauri::async_runtime::spawn(
                sampling_worker.run(sampling_state, sampling_db.clone()),
            ));
            setup_workers.lock().unwrap().push(tauri::async_runtime::spawn(
                sampling_db.maintain(),
            ));
            let signal_app = app.handle().clone();
            setup_workers.lock().unwrap().push(tauri::async_runtime::spawn(async move {
                if let Ok(mut terminate) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    tokio::select! { _ = terminate.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
                    signal_app.exit(0);
                }
            }));
            let status_state = state.clone();
            let handle = app.handle().clone();
            setup_workers
                .lock()
                .unwrap()
                .push(tauri::async_runtime::spawn(async move {
                    let mut published = 0;
                    loop {
                        let snapshot = status_state.lock().await.snapshot();
                        if snapshot.revision != published {
                            published = snapshot.revision;
                            let _ = handle.emit("state-changed", serde_json::json!({
                                "instanceId": snapshot.instance_id, "revision": snapshot.revision
                            }));
                        }
                        let connected = snapshot
                            .runtime
                            .tunnels
                            .values()
                            .filter(|s| s.status == Status::Connected)
                            .count();
                        let errors = snapshot
                            .runtime
                            .tunnels
                            .values()
                            .any(|s| s.status == Status::Error);
                        if let Some(tray) = handle.tray_by_id("porthop") {
                            let _ = tray.set_title(if connected > 0 {
                                Some(connected.to_string())
                            } else if errors {
                                Some("!".into())
                            } else {
                                None
                            });
                            let _ = tray.set_tooltip(Some(format!(
                                "Porthop · {connected} connected tunnels"
                            )));
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }));
            if std::env::args().any(|arg| arg == "--autostart") {
                if let Some(window) = app.get_webview_window("main") { window.hide()?; }
            }
            setup_workers.lock().unwrap().push(tauri::async_runtime::spawn(
                crate::updates::background(app.handle().clone()),
            ));
            log::info!("Desktop and background workers initialized");
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(context)
        .context("Cannot initialize the desktop app")?;
    let lock = Arc::new(lock);
    let restart_lock = lock.clone();
    app.run(move |app, event| {
        if let tauri::RunEvent::ExitRequested { ref api, .. } = event {
            if !shutdown_complete.load(Ordering::SeqCst) {
                api.prevent_exit();
                if !shutdown_started.swap(true, Ordering::SeqCst) {
                    log::info!("Graceful shutdown started");
                    let app = app.clone();
                    let state = app.state::<Shared>().inner().clone();
                    let workers = std::mem::take(&mut *workers.lock().unwrap());
                    let done = shutdown_complete.clone();
                    let restart_lock = restart_lock.clone();
                    tauri::async_runtime::spawn(async move {
                        for worker in &workers {
                            worker.abort();
                        }
                        for worker in workers {
                            let _ = worker.await;
                        }
                        app.state::<crate::terminal::Sessions>()
                            .close_server(None)
                            .await;
                        app.state::<crate::files::Operations>().cancel_server(None);
                        state.lock().await.shutdown().await;
                        log::info!("Background workers and SSH sessions stopped");
                        done.store(true, Ordering::SeqCst);
                        if app
                            .state::<crate::updates::Updates>()
                            .restart
                            .load(Ordering::SeqCst)
                        {
                            // Tauri spawns the replacement before exiting. Release
                            // ownership first so the new process cannot lose the lock race.
                            match FileExt::unlock(&*restart_lock) {
                                Ok(()) => app.request_restart(),
                                Err(error) => {
                                    log::error!(
                                        "Could not release instance lock for restart: {error}"
                                    );
                                    app.exit(0);
                                }
                            }
                        } else {
                            app.exit(0);
                        }
                    });
                }
            }
        }
        if let tauri::RunEvent::Reopen { .. } = event {
            show(app);
        }
    });
    drop(lock);
    Ok(())
}
