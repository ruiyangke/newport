//! Standalone application composition. The UI crate owns no backend runtime.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod desktop;
mod health;
mod shutdown;
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
    startup::profile_policy(
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
    // The window keeps its own preferences beside the backend's profile.
    newport_client::profile::set_directory(store.directory.clone());
    let endpoint = newport_platform::ipc::profile_endpoint(&store.directory);
    #[cfg(target_os = "macos")]
    let profile_directory = store.directory.clone();
    let Some(reserved) = newport_backend::application::Application::reserve(store)? else {
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
        Some(transaction) => match updates::recovery::resolve(transaction, &profile_directory)? {
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
    // Recovery restored this version after a failed install. The health probe
    // never consumes the reason; only an app that shows Settings does.
    #[cfg(target_os = "macos")]
    if launch.check.is_none()
        && let Some(failure) = updates::recovery::take_failure(&profile_directory)
    {
        updates.report_failure(failure);
    }
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
    // One backend subscription for the whole UI, independent of visible pages
    // and the slower configuration/feature snapshot refresh.
    let mut connections = backend.connections();
    let initial_connections = connections.borrow_and_update().clone();
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
    #[cfg(windows)]
    let install_request = std::sync::Arc::new(std::sync::Mutex::new(None));
    #[cfg(windows)]
    let requested_install = install_request.clone();
    let teardown = std::rc::Rc::new(std::cell::Cell::new(Some(shutdown::Teardown {
        #[cfg(target_os = "macos")]
        acceptance,
        #[cfg(any(target_os = "macos", windows))]
        tray_monitor,
        signal,
        updates: updates.clone(),
        update_background,
        desktop: desktop.clone(),
        backend: shutdown::Backend { stop, services },
    })));
    #[cfg(target_os = "macos")]
    let quit_teardown = teardown.clone();
    #[cfg(target_os = "macos")]
    let quit_runtime = runtime.handle().clone();
    let ui_result = newport_gpui::run_with_options_and_ready(
        newport_gpui::LaunchOptions {
            start_hidden: launch.autostart,
        },
        move |cx| {
            // AppKit calls exit() right after quit observers, so the loop never
            // returns. Drain here, before the window's own observer: backend
            // shutdown closes every terminal, and GPUI caps that observer's now
            // redundant close requests at its quit timeout.
            #[cfg(target_os = "macos")]
            cx.on_app_quit(move |cx| {
                if let Some(teardown) = quit_teardown.take() {
                    // The main thread blocks until the drain ends; don't leave
                    // a frozen window on screen.
                    cx.hide();
                    match teardown.finish_within(&quit_runtime, shutdown::LIMIT) {
                        Ok(()) => eprintln!("Newport shut down."),
                        Err(error) => eprintln!("Newport shutdown: {error}"),
                    }
                }
                async {}
            })
            .detach();
            newport_gpui::connectivity::publish(initial_connections, cx);
            cx.spawn(async move |cx| {
                while connections.changed().await.is_ok() {
                    let snapshot = connections.borrow_and_update().clone();
                    cx.update(|cx| newport_gpui::connectivity::publish(snapshot, cx));
                }
            })
            .detach();
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
                            desktop::Action::Install(request, reply) => {
                                // AppKit exits inside terminate:, so nothing after the UI
                                // loop runs on macOS. Persist intent first, then check
                                // buffers and quit with no await in between; the
                                // relaunched app replaces the bundle during recovery.
                                #[cfg(target_os = "macos")]
                                {
                                    use updates::install::{Begun, Handoff, handoff};
                                    let begun = cx
                                        .background_executor()
                                        .spawn(async move { (*request).begin() })
                                        .await;
                                    match handoff(
                                        begun,
                                        || cx.update(newport_gpui::ensure_editors_saved),
                                        Begun::relaunch,
                                        |result| reply.send(result).is_ok(),
                                    ) {
                                        Handoff::Quit(relaunch) => {
                                            relaunch.release();
                                            cx.update(|cx| cx.quit());
                                            break;
                                        }
                                        Handoff::Abandon(begun) => {
                                            let abandoned = cx
                                                .background_executor()
                                                .spawn(async move { begun.abandon() })
                                                .await;
                                            if let Err(error) = abandoned {
                                                eprintln!(
                                                    "Couldn’t discard the staged update: {error}"
                                                );
                                            }
                                        }
                                        Handoff::Refused => {}
                                    }
                                }
                                #[cfg(windows)]
                                {
                                    if let Err(error) =
                                        cx.update(newport_gpui::ensure_editors_saved)
                                    {
                                        let _ = reply.send(Err(error));
                                        continue;
                                    }
                                    // A timed-out/cancelled requester must never arm an install.
                                    if reply.send(Ok(())).is_err() {
                                        continue;
                                    }
                                    *requested_install
                                        .lock()
                                        .unwrap_or_else(|error| error.into_inner()) =
                                        Some(*request);
                                    cx.update(|cx| cx.quit());
                                    break;
                                }
                            }
                            desktop::Action::Run(action) => action(),
                            desktop::Action::Activate(acknowledge) => {
                                if !acknowledge.is_closed() {
                                    let result = cx.update(newport_gpui::activate);
                                    let _ = acknowledge.send(result);
                                }
                            }
                            desktop::Action::Quit => {
                                let _ = cx.update(newport_gpui::request_quit);
                            }
                            #[cfg(any(target_os = "macos", windows))]
                            desktop::Action::Tray(action) => {
                                use newport_platform::tray::Action;
                                let result = cx.update(|cx| match action {
                                    Action::Open => newport_gpui::activate(cx),
                                    Action::Settings => newport_gpui::open_settings(cx),
                                    Action::Quit => newport_gpui::request_quit(cx),
                                });
                                if let Err(error) = result {
                                    eprintln!("Tray action failed: {error}");
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
    // On macOS this is reached only when the UI never ran its event loop.
    let Some(teardown) = teardown.take() else {
        return ui_result;
    };
    let backend = runtime.block_on(teardown.release_host());
    #[cfg(windows)]
    let installation = if ui_result.is_ok() {
        // The UI loop has ended, but backend ownership is still held. Transfer
        // preparation before requesting drain.
        install_request
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .map(|request| request.begin())
    } else {
        None
    };
    let backend_result = runtime.block_on(backend.stop());
    let result = ui_result.and(backend_result);
    #[cfg(windows)]
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
