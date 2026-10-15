//! Standalone application composition. The UI crate owns no backend runtime.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod desktop;
mod health;
mod startup;
#[cfg(any(target_os = "macos", windows))]
mod tray;
mod updates;

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    if let Err(error) = run() {
        eprintln!("Could not start Newport: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    // Preview must branch before profile access, backend startup, or SSH agent relay.
    if std::env::args().any(|arg| arg == "--setup-preview") {
        return newport_gpui::run_setup_preview();
    }
    if std::env::args().any(|arg| arg == "--build-info") {
        println!(
            "{}",
            serde_json::json!({
                "schema": 1, "productName": "Newport", "identifier": "app.newport",
                "version": env!("CARGO_PKG_VERSION"),
                "debugAssertions": cfg!(debug_assertions),
                "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            })
        );
        return Ok(());
    }
    if std::env::args().any(|arg| arg == "--check-needle") {
        newport_backend::agent_lifecycle::check_resources()?;
        println!("Bundled Needle resources validated");
        return Ok(());
    }
    if std::env::args().any(|arg| arg == "--check-grammars") {
        newport_gpui::check_grammars()?;
        println!("Bundled grammars validated");
        return Ok(());
    }
    if std::env::args().any(|arg| arg == "--check-pdf") {
        newport_platform::pdf::check_renderer()?;
        println!("Bundled PDF renderer validated");
        return Ok(());
    }
    let launch = startup::Launch::from_args(std::env::args().skip(1))?;
    if launch.check == Some(startup::Check::UpdateHealth) {
        newport_backend::agent_lifecycle::check_resources()?;
        newport_gpui::check_grammars()?;
        newport_platform::pdf::check_renderer()?;
    }
    let isolated = startup::profile_policy(
        cfg!(debug_assertions),
        newport_backend::profile::data_directory_override().as_deref(),
    )?;
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    let (desktop, requests) = desktop::Integration::new();
    let updates = updates::Service::for_host()?;
    #[cfg(target_os = "macos")]
    let startup_guard = if updates.enabled() && launch.check.is_none() {
        Some(updates::recovery::guard()?)
    } else {
        // The install parent's health probe deliberately skips this lock: the
        // parent holds it throughout verification. Development stays isolated.
        None
    };
    let store = newport_backend::profile::open()?;
    let endpoint = newport_platform::ipc::profile_endpoint(&store.directory);
    let Some(reserved) = newport_backend::application::Application::reserve(store, isolated)?
    else {
        #[cfg(target_os = "macos")]
        drop(startup_guard);
        if launch.check.is_some() {
            return Err("Another Newport process owns this profile. Quit that process before starting the backend check.".into());
        }
        if !launch.activate_existing() {
            return Ok(());
        }
        return newport_client::backend::Backend::at_path(endpoint).activate_window()
            .map_err(|error| format!("Another Newport process owns this profile, but its window could not be activated: {error}"));
    };
    #[cfg(target_os = "macos")]
    let recovery = match startup_guard.and_then(|guard| guard.into_transaction()) {
        Some(transaction) => match updates::recovery::resolve(transaction)? {
            updates::recovery::Startup::Continue(transaction) => Some(transaction),
            updates::recovery::Startup::Restart(installation) => {
                // Do not load credentials or migrate a profile using an old
                // executable. The health subprocess needs ownership released.
                drop(reserved);
                return runtime.block_on(installation.run(Ok(())));
            }
        },
        None => None,
    };
    reserved.prepare_upgrade(env!("CARGO_PKG_VERSION"))?;
    let mut backend = runtime.block_on(
        reserved.start(
            std::sync::Arc::new(desktop.clone()),
            desktop::Commands::new(desktop.clone(), startup::registration()?)
                .with_updates(updates.clone()),
        ),
    )?;
    if let Some(check) = launch.check {
        let report = runtime.block_on(health::check(backend))?;
        match check {
            startup::Check::Backend => println!(
                "Backend check passed ({} servers, {} tunnels).",
                report.servers, report.tunnels
            ),
            startup::Check::UpdateHealth => println!("{}", report.update_receipt()),
        }
        return Ok(());
    }
    let (stop, stopped) = tokio::sync::oneshot::channel();
    #[cfg(target_os = "macos")]
    let (ready, initialized) = tokio::sync::oneshot::channel();
    #[cfg(target_os = "macos")]
    let acceptance = recovery.map(|transaction| {
        let _entered = runtime.enter();
        updates::recovery::Acceptance::start(transaction, initialized, endpoint.clone())
    });
    let update_background = runtime.spawn(updates.clone().background());
    let failed_desktop = desktop.clone();
    let services = runtime.spawn(async move {
        tokio::select! {
            _ = stopped => backend.stop().await,
            result = backend.wait() => {
                failed_desktop.quit().await;
                result.and(Err("The backend stopped unexpectedly.".into()))
            }
        }
    });
    let signal_desktop = desktop.clone();
    let signal = runtime.spawn(async move {
        if let Err(error) = shutdown_signal().await {
            eprintln!("Cannot observe shutdown signals: {error}");
            return;
        }
        signal_desktop.quit().await;
    });
    #[cfg(any(target_os = "macos", windows))]
    let tray_monitor = runtime.spawn(tray::monitor(
        newport_client::backend::Backend::at_path(endpoint),
        desktop.clone(),
    ));
    #[cfg(any(target_os = "macos", windows))]
    let tray_desktop = desktop.clone();
    #[cfg(any(target_os = "macos", windows))]
    let install_request = std::sync::Arc::new(std::sync::Mutex::new(None));
    #[cfg(any(target_os = "macos", windows))]
    let requested_install = install_request.clone();
    let ui_result = newport_gpui::run_with_options_and_ready(
        newport_gpui::LaunchOptions {
            start_hidden: launch.autostart,
        },
        move |cx| {
            if let Err(error) = newport_platform::events::install(newport_backend::recovery::report)
            {
                eprintln!("System event monitoring unavailable; using timed recovery: {error}");
            }
            // AppKit requires a running event loop before creating a status item.
            cx.defer(move |cx| {
                #[cfg(any(target_os = "macos", windows))]
                let tray = match newport_platform::tray::Tray::new(move |action| {
                    tray_desktop.tray_action(action)
                }) {
                    Ok(tray) => Some(tray),
                    Err(error) => {
                        eprintln!("Tray unavailable: {error}");
                        // A hidden startup without a tray would strand the user.
                        if launch.autostart {
                            let _ = newport_gpui::activate(cx);
                        }
                        None
                    }
                };
                cx.spawn(async move |cx| {
                    while let Ok(action) = requests.recv().await {
                        match action {
                            #[cfg(any(target_os = "macos", windows))]
                            desktop::Action::Install(request) => {
                                *requested_install
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner()) = Some(*request);
                                cx.update(|cx| cx.quit());
                                break;
                            }
                            desktop::Action::Run(action) => action(),
                            desktop::Action::Activate(acknowledge) => {
                                if !acknowledge.is_closed() {
                                    let result = cx.update(newport_gpui::activate);
                                    let _ = acknowledge.send(result);
                                }
                            }
                            desktop::Action::Quit => {
                                cx.update(|cx| cx.quit());
                                break;
                            }
                            #[cfg(any(target_os = "macos", windows))]
                            desktop::Action::Tray(action) => {
                                use newport_platform::tray::Action;
                                let result = cx.update(|cx| match action {
                                    Action::Open => newport_gpui::activate(cx),
                                    Action::Settings => newport_gpui::open_settings(cx),
                                    Action::Quit => {
                                        cx.quit();
                                        Ok(())
                                    }
                                });
                                if let Err(error) = result {
                                    eprintln!("Tray action failed: {error}");
                                }
                                if action == Action::Quit {
                                    break;
                                }
                            }
                            #[cfg(any(target_os = "macos", windows))]
                            desktop::Action::TrayStatus(status) => {
                                if let Some(tray) = &tray
                                    && let Err(error) = tray.set_status(status)
                                {
                                    eprintln!("Tray status failed: {error}");
                                }
                            }
                        }
                    }
                })
                .detach();
            });
        },
        move || {
            #[cfg(target_os = "macos")]
            let _ = ready.send(());
        },
    );
    #[cfg(target_os = "macos")]
    if let Some(acceptance) = acceptance
        && let Err(error) = runtime.block_on(acceptance.finish())
    {
        eprintln!("Update backup retained: {error}");
    }
    #[cfg(any(target_os = "macos", windows))]
    tray_monitor.abort();
    signal.abort();
    updates.shutdown();
    update_background.abort();
    desktop.close();
    runtime.block_on(updates.finish_preparation());
    #[cfg(any(target_os = "macos", windows))]
    let installation = if ui_result.is_ok() {
        // The UI loop has ended, but backend ownership is still held. Transfer
        // preparation (and persist macOS recovery) before requesting drain.
        install_request
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .map(|request| request.begin())
    } else {
        None
    };
    let _ = stop.send(());
    let backend_result = runtime
        .block_on(services)
        .map_err(|error| format!("Backend shutdown failed: {error}"))
        .and_then(|result| result);
    let result = ui_result.and(backend_result);
    #[cfg(any(target_os = "macos", windows))]
    let result = match installation {
        Some(Ok(installation)) => runtime.block_on(installation.run(result)),
        Some(Err(error)) => result.and(Err(error)),
        None => result,
    };
    // Backend shutdown has completed. A cancelled blocking HTTPS read must not
    // keep Quit waiting for its download deadline; it cannot publish a package
    // after shutdown, and these workers never install or write profile data.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}

async fn shutdown_signal() -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|error| error.to_string())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(|error| error.to_string()),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| error.to_string())
}
