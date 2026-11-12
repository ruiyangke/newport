//! One lazy native presenter per backend-owned display. The idle watcher keeps
//! no client, renderer, frame timer or native window alive.
use crate::graphical::Registry;
use gpui_kit::gpui::App;
use newport_remote_provider_gpui::native_session::{
    self, NativeSession, PlacementMemory, SharedOptions,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Weak},
};

/// Each remote app's own taskbar button and icon, and what a pin of it
/// runs: an AppUserModelID per server and app (see
/// `newport_domain::remote_app_identity`), the icon the server's catalogue
/// lists for the app, and `newport [--data-dir <profile>] --remote-app <id>`
/// with the app's name and an `.ico` copy of its icon. An app's first window records it in the
/// registry those commands name apps by.
///
/// The resolver runs on the UI thread and never waits: the registry is read
/// in memory and its worker writes the file and the `.ico` files, which a
/// pin's icon joins once written.
#[cfg(windows)]
fn identities(
    candidate: &crate::graphical::Presentation,
    remote_apps: &newport_backend::remote_app_registry::RemoteApps,
) -> Result<native_session::IdentityResolver, String> {
    use newport_domain::{
        remote_app_identity::{self as identity, NamedApp, RemoteApp},
        remote_app_registry::App as Recorded,
    };
    use newport_gpui::remote_app_catalog;
    use newport_remote_provider_stubs::{names::display_name, routing::AppKey};
    use std::{cell::RefCell, collections::HashMap};
    let launcher = remote_apps.launcher().clone();
    let executable = launcher.executable.to_string_lossy().into_owned();
    let server = candidate.server.id.to_string();
    let server_name = display_name(&candidate.server.name);
    let server_key = newport_backend::remote_app_registry::server_key(&server)?;
    let remote_apps = remote_apps.clone();
    // The name and actions each app was recorded with in this presentation.
    let recorded =
        RefCell::new(HashMap::<String, (String, Vec<remote_app_catalog::Action>)>::new());
    // Icons for apps that open before the Remote apps page is shown.
    remote_app_catalog::load(&server, &server_name);
    Ok(Rc::new(move |app: &AppKey| {
        let (app, entry, kept) = match app {
            AppKey::Desktop(id) => (
                RemoteApp::Desktop(id),
                remote_app_catalog::find(&server, id),
                Recorded::DesktopId(id.clone()),
            ),
            // An app id is often its desktop file's name.
            AppKey::AppId(id) => (
                RemoteApp::AppId(id),
                remote_app_catalog::find(&server, id),
                Recorded::AppId(id.clone()),
            ),
            AppKey::Server => (RemoteApp::Server, None, Recorded::Server),
        };
        let app_id = identity::app_user_model_id(&server_key, app);
        let name = match (&entry, &kept) {
            (Some(entry), _) => display_name(&entry.name),
            (None, Recorded::DesktopId(id) | Recorded::AppId(id)) => {
                // "org.gnome.Calculator.desktop" reads as "Calculator".
                let id = id.strip_suffix(".desktop").unwrap_or(id);
                display_name(id.rsplit('.').next().unwrap_or(id))
            }
            (None, Recorded::Server) => display_name(&format!("{server_name} apps")),
        };
        // Again once the catalogue names the app, or lists its actions.
        let actions = entry
            .as_ref()
            .map(|entry| entry.actions.clone())
            .unwrap_or_default();
        let seen = (name.clone(), actions.clone());
        if recorded.borrow().get(&app_id) != Some(&seen) {
            remote_apps.record(newport_backend::remote_app_registry::RecordedApp {
                id: app_id.clone(),
                server: server.clone(),
                server_name: server_name.clone(),
                app: kept,
                name: name.clone(),
                actions,
            });
            recorded.borrow_mut().insert(app_id.clone(), seen);
        }
        // An app with no icon of its own shows its letter tile, not Newport's.
        let icon_png =
            newport_gpui::remote_app_tile::icon_or_tile(entry.and_then(|e| e.icon_png), &name);
        let others: Vec<_> = remote_apps.names();
        let others: Vec<_> = others
            .iter()
            .map(|(server, name)| NamedApp {
                server_id: server,
                server_name: "",
                name,
            })
            .collect();
        let display_name = identity::distinct_name(
            NamedApp {
                server_id: &server,
                server_name: &server_name,
                name: &name,
            },
            &others,
        );
        Some(native_session::WindowIdentity {
            relaunch: Some(native_session::Relaunch {
                command: identity::relaunch_command(
                    &executable,
                    launcher.data_directory.as_deref(),
                    &app_id,
                    None,
                ),
                display_name,
                icon: icon_png
                    .as_ref()
                    .and_then(|icon| remote_apps.icon_file(&app_id, icon)),
            }),
            app_id,
            icon_png,
        })
    }))
}

/// How long after the user opens an app its windows come to the front, when
/// a copy already running shows them or they come with the display.
const LAUNCH_FOCUS: std::time::Duration = std::time::Duration::from_secs(15);

enum Presenter {
    Windows(NativeSession),
    #[cfg(target_os = "macos")]
    Stubs(newport_remote_provider_stubs::StubPresenter),
}
impl Presenter {
    /// The user opened the app with this catalogue id just now.
    fn launching(&self, app: &str) {
        match self {
            // Newport's own windows open in front.
            Self::Windows(_) => {}
            #[cfg(target_os = "macos")]
            Self::Stubs(presenter) => presenter.launching(
                newport_remote_provider_stubs::routing::AppKey::Desktop(app.to_owned()),
            ),
        }
    }

    /// Bring the app with this catalogue id to the front. Whether this
    /// presenter shows it.
    fn bring_forward(&self, app: &str, cx: &mut App) -> bool {
        match self {
            Self::Windows(session) => session.bring_forward(app, cx),
            #[cfg(target_os = "macos")]
            Self::Stubs(presenter) => {
                presenter.bring_forward(newport_remote_provider_stubs::routing::AppKey::Desktop(
                    app.to_owned(),
                ));
                presenter.is_presenting()
            }
        }
    }

    /// Release the display with its apps open, for a presenter in the other
    /// mode to show them.
    fn detach(&self) {
        match self {
            Self::Windows(session) => session.detach(),
            #[cfg(target_os = "macos")]
            Self::Stubs(presenter) => presenter.detach(),
        }
    }
    fn is_presenting(&self) -> bool {
        match self {
            Self::Windows(session) => session.is_presenting(),
            #[cfg(target_os = "macos")]
            Self::Stubs(presenter) => presenter.is_presenting(),
        }
    }
}

#[cfg(target_os = "macos")]
fn present_with_stubs(
    registry: &Registry,
    candidate: &crate::graphical::Presentation,
) -> Result<Presenter, String> {
    use newport_remote_provider_stubs as stubs;
    let stub_binary = stubs::shipped_stub().ok_or("This build has no remote app stub")?;
    let host_bundle_id = stubs::host_bundle_id().ok_or("Newport isn't running from its app")?;
    let profile = newport_backend::profile::directory()?;
    // Per profile, so two Newports with one bundle id each keep their stubs.
    let service_name = stubs::service_name(&host_bundle_id, &profile)?;
    let bundles = profile.join("Remote Apps");
    // Before claiming the clipboard: a failure here falls back to Newport
    // windows, which claim it themselves.
    stubs::prepare(&service_name, &stub_binary)?;
    let server = candidate.server.id.to_string();
    // Names and icons for apps that open before the Remote apps page is shown.
    newport_gpui::remote_app_catalog::load(&server, &candidate.server.name);
    let clipboard = candidate.clipboard_presenter();
    let detached = registry.clone();
    let options = stubs::Options {
        host_bundle_id,
        service_name,
        stub_binary,
        bundles,
        server_key: server.clone(),
        server_name: candidate.server.name.clone(),
        launch: {
            let (server, server_name) = (server.clone(), candidate.server.name.clone());
            Arc::new(move |app, name| {
                let actions = match app {
                    stubs::routing::AppKey::Desktop(id) | stubs::routing::AppKey::AppId(id) => {
                        newport_gpui::remote_app_catalog::find(&server, id)
                            .map(|entry| entry.actions)
                            .unwrap_or_default()
                    }
                    stubs::routing::AppKey::Server => Vec::new(),
                };
                crate::launchers::launch(&server, &server_name, app, name, actions)
            })
        },
        catalog: Arc::new(move |desktop_id| {
            newport_gpui::remote_app_catalog::find(&server, desktop_id).map(|entry| {
                stubs::AppInfo {
                    name: entry.name,
                    icon: entry.icon_png.map(|icon| icon.to_vec()),
                    categories: Vec::new(),
                }
            })
        }),
        placeholder_icon: Arc::new(|name| {
            newport_gpui::remote_app_tile::png(name).map(|png| png.to_vec())
        }),
        on_focus: clipboard.map(|presenter| {
            Arc::new(move |focused| {
                presenter.set_focused(focused);
            }) as Arc<dyn Fn(bool) + Send + Sync>
        }),
        // A window arriving after the idle snapshot causes a fresh attachment.
        on_detached: Arc::new(move || detached.recheck_presentation()),
    };
    stubs::present_shared(candidate.connection.clone(), options).map(Presenter::Stubs)
}

pub fn start(
    registry: Registry,
    remote_apps: newport_backend::remote_app_registry::RemoteApps,
    cx: &mut App,
) {
    #[cfg(not(windows))]
    let _ = remote_apps;
    let mut changes = registry.observe_presentation();
    // Each presenter with the separate-apps setting it started under and its
    // server.
    let presenters = Rc::new(RefCell::new(
        BTreeMap::<String, (Presenter, bool, String)>::new(),
    ));
    let (shown, launched) = (presenters.clone(), presenters.clone());
    // Apps the user opened before their server's presenter started, and when.
    let waiting_launches = Rc::new(RefCell::new(
        Vec::<(String, String, std::time::Instant)>::new(),
    ));
    #[cfg(target_os = "macos")]
    let waiting = waiting_launches.clone();
    newport_gpui::remote_app_focus::install(
        newport_gpui::remote_app_focus::Hooks {
            bring_forward: Rc::new(move |server, app, cx| {
                shown.borrow().values().any(|(presenter, _, presented)| {
                    presented == server && presenter.bring_forward(app, cx)
                })
            }),
            launching: Rc::new(move |server, app, _| {
                let presenters = launched.borrow();
                let mut presenting = presenters
                    .values()
                    .filter(|(_, _, presented)| presented == server)
                    .peekable();
                if presenting.peek().is_none() {
                    // Its windows come with a presenter that starts soon.
                    let mut waiting = waiting_launches.borrow_mut();
                    waiting.retain(|(_, _, since)| since.elapsed() < LAUNCH_FOCUS);
                    waiting.push((server.to_owned(), app.to_owned(), std::time::Instant::now()));
                }
                for (presenter, _, _) in presenting {
                    presenter.launching(app);
                }
            }),
        },
        cx,
    );
    cx.spawn(async move |cx| {
        // Idle presentation owns no client. Retain only bounded per-window
        // positions while this exact backend session still exists.
        let mut placements =
            BTreeMap::<String, (Weak<newport_remote_client::RemoteSession>, PlacementMemory)>::new(
            );
        loop {
            // Mark before reconciling: activity or detachment during the update
            // must schedule another pass, including when window counts match.
            changes.borrow_and_update();
            if registry.is_closed() {
                break;
            }
            cx.update(|cx| {
                let mut presenters = presenters.borrow_mut();
                presenters.retain(|_, (presenter, _, _)| presenter.is_presenting());
                placements.retain(|_, (owner, _)| owner.strong_count() > 0);
                // Remote windows as their own apps. A changed setting applies
                // to open apps too: a presenter that started under the other
                // one releases its display, and a later pass presents it
                // again, windows and all.
                #[cfg(any(target_os = "macos", windows))]
                let separate_apps = registry.separate_apps();
                #[cfg(not(any(target_os = "macos", windows)))]
                let separate_apps = false;
                for (presenter, started, _) in presenters.values() {
                    if *started != separate_apps {
                        presenter.detach();
                    }
                }
                for candidate in registry.presentation_candidates() {
                    if presenters.contains_key(&candidate.key) {
                        continue;
                    }
                    // Separate apps take their names and icons from the
                    // server's catalogue, and keep them while they run: an
                    // app already open waits briefly for it rather than being
                    // named by its desktop id.
                    #[cfg(any(target_os = "macos", windows))]
                    if separate_apps {
                        let recheck = registry.clone();
                        let server = candidate.server.id.to_string();
                        let name = &candidate.server.name;
                        if !newport_gpui::remote_app_catalog::ready(&server, name, move || {
                            recheck.recheck_presentation()
                        }) {
                            continue;
                        }
                    }
                    #[cfg(target_os = "macos")]
                    if separate_apps {
                        match present_with_stubs(&registry, &candidate) {
                            Ok(presenter) => {
                                newport_gpui::separate_apps::report(None);
                                let server = candidate.server.id.to_string();
                                // Apps the user opened while it started.
                                waiting.borrow_mut().retain(|(waited, app, since)| {
                                    if *waited != server {
                                        return true;
                                    }
                                    if since.elapsed() < LAUNCH_FOCUS {
                                        presenter.launching(app);
                                    }
                                    false
                                });
                                presenters
                                    .insert(candidate.key, (presenter, separate_apps, server));
                                continue;
                            }
                            // The experiment failing must not cost the apps:
                            // they open as Newport windows instead, and the
                            // setting says why.
                            Err(error) => {
                                eprintln!("Remote apps open in Newport: {error}");
                                newport_gpui::separate_apps::report(Some(error));
                            }
                        }
                    }
                    let detached = registry.clone();
                    let clipboard = candidate.clipboard_presenter();
                    let placement = placements
                        .entry(candidate.key.clone())
                        .or_insert_with(|| {
                            (
                                Arc::downgrade(&candidate.connection),
                                PlacementMemory::default(),
                            )
                        })
                        .1
                        .clone();
                    // Without an identity the apps still open, grouped with
                    // Newport on the taskbar.
                    #[cfg(windows)]
                    let identities = separate_apps
                        .then(|| identities(&candidate, &remote_apps))
                        .and_then(|identities| {
                            identities
                                .inspect_err(|error| {
                                    eprintln!("Remote apps open as Newport windows: {error}")
                                })
                                .ok()
                        });
                    let options = SharedOptions {
                        placement,
                        presentation:
                            newport_remote_provider_gpui::settings::PresentationSettings {
                                hud: candidate.hud,
                                trace: cfg!(debug_assertions)
                                    && std::env::var_os("NEWPORT_REMOTE_TRACE")
                                        .is_some_and(|value| value == "1"),
                            },
                        on_focus: clipboard.map(|presenter| {
                            Rc::new(move |focused| {
                                presenter.set_focused(focused);
                            }) as Rc<dyn Fn(bool)>
                        }),
                        // The provider calls this after releasing its claim.
                        // A window arriving after its idle snapshot therefore
                        // causes a fresh attachment to the same client.
                        on_detached: Rc::new(move |_, _| detached.recheck_presentation()),
                        #[cfg(windows)]
                        identities,
                    };
                    match native_session::present_shared(candidate.connection.clone(), options, cx)
                    {
                        Ok(presenter) => {
                            presenters.insert(
                                candidate.key,
                                (
                                    Presenter::Windows(presenter),
                                    separate_apps,
                                    candidate.server.id.to_string(),
                                ),
                            );
                        }
                        Err(error) => abort(&candidate, &error),
                    }
                }
            });
            if changes.changed().await.is_err() {
                break;
            }
        }
    })
    .detach();
}

fn abort(candidate: &crate::graphical::Presentation, error: &str) {
    if candidate.connection.control().state() == newport_remote_client::SessionState::Running {
        eprintln!("Cannot present remote display: {error}");
        candidate
            .connection
            .abort(format!("Cannot present remote display: {error}"));
    }
}
