//! Application composition: the backend and the UI in one process. The UI
//! crate owns no backend runtime.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod build_stamp;
mod desktop;
#[cfg(newport_remote_apps)]
mod graphical;
#[cfg(newport_remote_apps)]
mod graphical_presentation;
mod health;
#[cfg(all(target_os = "macos", newport_remote_apps))]
mod launchers;
mod shutdown;
#[cfg(all(windows, newport_remote_apps))]
mod start_menu;
mod startup;
#[cfg(any(target_os = "macos", windows))]
mod tray;
mod updates;
#[cfg(target_os = "macos")]
mod webview_leftovers;

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

/// Arguments that aren't valid UTF-8 must not abort startup: only the flags
/// Newport knows are matched, and they are ASCII.
fn lossy_arguments(args: impl Iterator<Item = std::ffi::OsString>) -> Vec<String> {
    args.map(|arg| arg.to_string_lossy().into_owned()).collect()
}

fn run() -> Result<(), String> {
    let args = lossy_arguments(std::env::args_os());
    if args.get(1).map(String::as_str) == Some("--version") {
        println!("Newport {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    if args.get(1).map(String::as_str) == Some(newport_platform::clipboard::helper::ARGUMENT) {
        if args.len() != 2 {
            return Err("Invalid clipboard helper arguments".into());
        }
        return newport_platform::clipboard::helper::run_from_stdio();
    }
    #[cfg(windows)]
    if args.get(1).map(String::as_str)
        == Some(newport_platform::clipboard::provider::windows::process::ARGUMENT)
    {
        if args.len() != 2 {
            return Err("Invalid clipboard helper arguments".into());
        }
        return newport_platform::clipboard::provider::windows::process::run_from_stdio(
            newport_platform::clipboard::files::open_retained,
        );
    }
    // Preview must branch before profile access or backend startup.
    if args.iter().any(|arg| arg == "--setup-preview") {
        return newport_gpui::run_setup_preview();
    }
    if args.iter().any(|arg| arg == "--build-info") {
        println!(
            "{}",
            serde_json::json!({
                "schema": 1, "productName": "Newport", "identifier": "app.newport",
                "version": env!("CARGO_PKG_VERSION"),
                "debugAssertions": cfg!(debug_assertions), "commit": build_stamp::COMMIT,
                "remoteApps": cfg!(newport_remote_apps),
                "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            })
        );
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--agent-identity") {
        println!("{}", newport_backend::agent_lifecycle::agent_identity()?);
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-needle") {
        newport_backend::agent_lifecycle::check_resources()?;
        println!("Bundled Needle resources validated");
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-grammars") {
        newport_gpui::check_grammars()?;
        println!("Bundled grammars validated");
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-pdf") {
        newport_platform::pdf::check_renderer()?;
        println!("Bundled PDF renderer validated");
        return Ok(());
    }
    let launch = startup::Launch::from_args(args.iter().skip(1).cloned())?;
    if let Some(directory) = &launch.data_directory {
        // SAFETY: no other thread runs yet. The variable carries the
        // profile to everything that reads it, child processes included.
        unsafe { std::env::set_var("NEWPORT_DATA_DIR", directory) };
    }
    // As the profile's IDs hash it: children started in other folders open
    // the same profile.
    if let Some(directory) = newport_backend::profile::data_directory_override()
        && !directory.as_os_str().is_empty()
    {
        // SAFETY: as above.
        unsafe { std::env::set_var("NEWPORT_DATA_DIR", directory) };
    }
    let launcher = newport_backend::remote_app_registry::Launcher::current()?;
    // Before any window or tray icon: Newport's windows group under its own
    // taskbar ID, never under the pin or shortcut that started it.
    if let Err(error) = newport_platform::app_shortcuts::set_process_app_id(&launcher.app_id()) {
        eprintln!("{error}");
    }
    if launch.check == Some(startup::Check::UpdateHealth) {
        newport_backend::agent_lifecycle::check_resources()?;
        newport_gpui::check_grammars()?;
        newport_platform::pdf::check_renderer()?;
    }
    startup::profile_policy(
        cfg!(debug_assertions),
        newport_backend::profile::data_directory_override().as_deref(),
    )?;
    // The uninstaller's: the profile itself is never opened.
    if launch.remove_remote_apps {
        let (shortcuts, icons) = newport_backend::remote_app_registry::remove_shortcuts(
            &newport_backend::profile::directory()?,
            &launcher,
        );
        println!("Removed {shortcuts} remote app shortcuts and {icons} icons.");
        return Ok(());
    }
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
    let profile_directory = store.directory.clone();
    let Some(reserved) = newport_backend::application::Application::reserve(store)? else {
        #[cfg(target_os = "macos")]
        drop(startup_guard);
        if launch.check.is_some() {
            return Err("Another Newport process owns this profile. Quit that process before starting the backend check.".into());
        }
        // A taskbar pin or shortcut: the running Newport opens the app.
        if let Some(app) = &launch.remote_app {
            return newport_client::backend::Backend::at_path(endpoint)
                .open_remote_app(&app.id, app.action.as_deref())
                .map_err(|error| {
                    let message = format!("Newport is running, but couldn’t open the app: {error}");
                    #[cfg(windows)]
                    newport_platform::window::alert("Newport", &message);
                    message
                });
        }
        if !launch.activate_existing() {
            return Ok(());
        }
        return newport_client::backend::Backend::at_path(endpoint).activate_window()
            .map_err(|error| format!("Another Newport process owns this profile, but its window could not be activated: {error}"));
    };
    // Owning the profile: also clear the web view data a Tauri build left
    // outside it (only the macOS build had a web view).
    #[cfg(target_os = "macos")]
    webview_leftovers::remove(
        &launch,
        cfg!(debug_assertions),
        newport_backend::profile::data_directory_override().is_some(),
    );
    // Each self-update leaves its installer in the temp folder.
    #[cfg(windows)]
    if updates.enabled() {
        std::thread::spawn(newport_platform::updates::windows::remove_stale_installers);
    }
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
    // A pin of the window opens this profile again.
    let relaunch = launcher.data_directory.as_ref().map(|directory| {
        let executable = launcher.executable.to_string_lossy();
        newport_gpui::Relaunch {
            app_id: launcher.app_id(),
            command: newport_domain::remote_app_identity::profile_command(&executable, directory),
            icon: format!("{executable},0"),
        }
    });
    // A startup check leaves the shell alone.
    let launcher = if launch.check.is_some() {
        launcher.without_shell()
    } else {
        launcher
    };
    let remote_apps =
        newport_backend::remote_app_registry::RemoteApps::open(&profile_directory, launcher);
    desktop.set_remote_apps(remote_apps.clone());
    // Each server's apps get Mac launchers; a startup check leaves them alone.
    #[cfg(all(target_os = "macos", newport_remote_apps))]
    if launch.check.is_none()
        && let Err(error) = launchers::install(remote_apps.clone())
    {
        eprintln!("Remote app launchers: {error}");
    }
    #[cfg(all(windows, newport_remote_apps))]
    newport_gpui::start_menu::install(Box::new(start_menu::Host::new(remote_apps.clone())));
    let mut backend = runtime.block_on(
        reserved.start(
            std::sync::Arc::new(desktop.clone()),
            desktop::Commands::new(desktop.clone(), startup::registration(&launch)?)
                .with_updates(updates.clone())
                .with_remote_apps(remote_apps.clone()),
        ),
    )?;
    if let Some(check) = launch.check {
        let report = runtime.block_on(health::check(backend));
        runtime.block_on(desktop.shutdown())?;
        let report = report?;
        match check {
            startup::Check::Backend => println!(
                "Backend check passed ({} servers, {} tunnels).",
                report.servers, report.tunnels
            ),
            startup::Check::UpdateHealth => println!("{}", report.update_receipt()),
        }
        return Ok(());
    }
    // Opened once the window is up, which stays hidden: Newport waits in
    // the tray.
    if let Some(app) = &launch.remote_app {
        desktop.open_remote_app(remote_apps.find(&app.id), app.action.clone());
    }
    let hidden = launch.hidden();
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
        // Quit can be declined (unsaved files), so every signal asks again.
        loop {
            if let Err(error) = shutdown_signal().await {
                eprintln!("Cannot observe shutdown signals: {error}");
                return;
            }
            signal_desktop.quit().await;
        }
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
    let quit_teardown = teardown.clone();
    let quit_runtime = runtime.handle().clone();
    #[cfg(windows)]
    let armed_install = install_request.clone();
    #[cfg(newport_remote_apps)]
    let graphical = desktop.graphical();
    let ui_result = newport_gpui::run_with_options_and_ready(
        newport_gpui::LaunchOptions {
            start_hidden: hidden,
            relaunch,
        },
        move |cx| {
            let hidden_startup = hidden || newport_platform::startup::launched_at_login();
            // AppKit, and Windows when the session ends, exit right after quit
            // observers, so the loop doesn't return. Drain here, before the
            // window's own observer: backend shutdown closes every terminal,
            // and GPUI caps that observer's now redundant close requests at
            // its quit timeout.
            cx.on_app_quit(move |cx| {
                #[cfg(windows)]
                let update_armed = armed_install
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .is_some();
                #[cfg(not(windows))]
                let update_armed = false;
                if let Some(teardown) = shutdown::take_on_quit(&quit_teardown, update_armed) {
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
            #[cfg(newport_remote_apps)]
            graphical_presentation::start(graphical, remote_apps, cx);
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
                        if hidden_startup {
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
                            desktop::Action::GuiSetup(server) => {
                                cx.update(|cx| {
                                    newport_gpui::request_terminal_gui_setup(&server, cx)
                                });
                            }
                            desktop::Action::ClipboardFiles {
                                server,
                                files,
                                reply,
                                cancelled,
                            } => {
                                cx.update(|cx| {
                                    newport_gpui::request_clipboard_files(
                                        newport_gpui::ClipboardFilesQuestion {
                                            server,
                                            items: files.items,
                                            bytes: files.bytes,
                                            outside_home: files.outside_home,
                                            hidden: files.hidden,
                                            executable: files.executable,
                                            installer: files.installer,
                                            large: files.large,
                                            names: files.names,
                                            top_level: files.top_level,
                                        },
                                        reply,
                                        cancelled,
                                        cx,
                                    )
                                });
                            }
                            desktop::Action::ClipboardFilesStatus { server, status } => {
                                use newport_backend::desktop_integration::ClipboardFilesStatus;
                                let notice = match status {
                                    ClipboardFilesStatus::Copying => {
                                        newport_gpui::ClipboardFilesNotice::Copying
                                    }
                                    ClipboardFilesStatus::Finished => {
                                        newport_gpui::ClipboardFilesNotice::Finished
                                    }
                                    ClipboardFilesStatus::Failed(error) => {
                                        newport_gpui::ClipboardFilesNotice::Failed(error)
                                    }
                                    ClipboardFilesStatus::UploadFailed(error) => {
                                        newport_gpui::ClipboardFilesNotice::UploadFailed(error)
                                    }
                                };
                                cx.update(|cx| {
                                    newport_gpui::clipboard_files_notice(server, notice, cx)
                                });
                            }
                            desktop::Action::Activate(acknowledge) => {
                                if !acknowledge.is_closed() {
                                    let result = cx.update(newport_gpui::activate);
                                    let _ = acknowledge.send(result);
                                }
                            }
                            desktop::Action::OpenRemoteApp {
                                entry,
                                action,
                                acknowledge,
                            } => {
                                // The requester gave up: it said nothing opened.
                                if acknowledge
                                    .as_ref()
                                    .is_some_and(|acknowledge| acknowledge.is_closed())
                                {
                                    continue;
                                }
                                cx.update(|cx| newport_gpui::open_remote_app(entry, action, cx));
                                if let Some(acknowledge) = acknowledge {
                                    let _ = acknowledge.send(Ok(()));
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
    // The quit observer took the teardown unless the UI never ran its event
    // loop or an armed update left it here.
    let result = match teardown.take() {
        None => ui_result,
        Some(teardown) => {
            let (backend, host_result) = runtime.block_on(teardown.release_host());
            #[cfg(windows)]
            let installation = if ui_result.is_ok() && host_result.is_ok() {
                // The UI loop has ended, but backend ownership is still held.
                // Transfer preparation before requesting drain.
                install_request
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take()
                    .map(|request| request.begin())
            } else {
                None
            };
            let backend_result = runtime.block_on(backend.stop());
            let result = ui_result.and(host_result).and(backend_result);
            #[cfg(windows)]
            let result = match installation {
                Some(Ok(installation)) => runtime.block_on(installation.run(result)),
                Some(Err(error)) => result.and(Err(error)),
                None => result,
            };
            result
        }
    };
    // Backend shutdown has completed or run out of time. A cancelled blocking
    // HTTPS read must not keep Quit waiting for its download deadline; it
    // cannot publish a package after shutdown, and these workers never install
    // or write profile data.
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

#[cfg(test)]
mod tests {
    use super::lossy_arguments;
    use std::ffi::OsString;

    #[test]
    fn arguments_that_are_not_utf8_do_not_panic() {
        #[cfg(unix)]
        let odd = {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![b'/', 0xff, b'x'])
        };
        #[cfg(not(unix))]
        let odd = OsString::from("/x");
        let args = lossy_arguments(
            [
                OsString::from("newport"),
                odd,
                OsString::from("--build-info"),
            ]
            .into_iter(),
        );
        assert_eq!(args.len(), 3);
        assert_eq!(args[2], "--build-info");
    }
}
