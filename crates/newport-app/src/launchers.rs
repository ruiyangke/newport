//! The Mac's launchers for Linux apps: one for each app a server lists, in a
//! folder named after the server, so every app is in Spotlight, Launchpad
//! and the Dock with its own name and icon. A launcher opens its app with
//! `--remote-app <id>`, whether or not Newport is running, and the registry
//! keeps the app that ID names.
use newport_backend::remote_app_registry::{RecordedApp, RemoteApps};
use newport_domain::{
    remote_app_identity::{self as identity, DATA_DIR_OPTION, REMOTE_APP_OPTION, RemoteApp},
    remote_app_registry::{Action, App as Recorded},
};
use newport_remote_provider_stubs::{
    self as stubs, launchers, names::display_name, routing::AppKey,
};
use std::{collections::HashSet, sync::OnceLock};

struct Launchers {
    writer: launchers::Writer,
    remote_apps: RemoteApps,
}

static LAUNCHERS: OnceLock<Launchers> = OnceLock::new();

/// Keep the launchers in line with what each server lists.
/// Called once, at startup.
pub fn install(remote_apps: RemoteApps) -> Result<(), String> {
    let stub_binary = stubs::shipped_stub().ok_or("This build has no remote app stub")?;
    let host_bundle_id = stubs::host_bundle_id().ok_or("Newport isn't running from its app")?;
    let profile = newport_backend::profile::directory()?;
    let service_name = stubs::service_name(&host_bundle_id, &profile)?;
    let writer = launchers::Writer {
        root: profile.join(newport_backend::profile::LAUNCHERS),
        stub_binary,
        host_bundle_id,
        service_name,
    };
    let refreshed = writer.clone();
    if LAUNCHERS
        .set(Launchers {
            writer,
            remote_apps,
        })
        .is_err()
    {
        return Err("The launchers are already kept".into());
    }
    // A new build's launchers run its stub from the start, not only once
    // their server's apps are read again.
    std::thread::spawn(move || {
        launchers::remove_leftovers(&refreshed.root);
        launchers::refresh(&refreshed);
        keep_entries();
    });
    newport_gpui::remote_app_catalog::observe(Box::new(|server, name, apps| {
        let (server, name) = (server.to_owned(), name.to_owned());
        std::thread::spawn(move || reconcile(&server, &name, apps));
    }));
    Ok(())
}

/// What `app`'s launcher runs Newport with to open it, recording the app so
/// the ID it names finds it.
pub fn launch(
    server: &str,
    server_name: &str,
    app: &AppKey,
    name: &str,
    actions: Vec<Action>,
) -> Vec<String> {
    let Some(launchers) = LAUNCHERS.get() else {
        return Vec::new();
    };
    let Ok(server_key) = newport_backend::remote_app_registry::server_key(server) else {
        return Vec::new();
    };
    let id = app_id(&server_key, app);
    let recorded = match app {
        AppKey::Desktop(id) => Recorded::DesktopId(id.clone()),
        AppKey::AppId(id) => Recorded::AppId(id.clone()),
        AppKey::Server => Recorded::Server,
    };
    launchers.remote_apps.record(RecordedApp {
        id: id.clone(),
        server: server.to_owned(),
        server_name: display_name(server_name),
        app: recorded,
        name: display_name(name),
        actions,
    });
    let mut arguments = Vec::new();
    if let Some(directory) = &launchers.remote_apps.launcher().data_directory {
        arguments.extend([DATA_DIR_OPTION.to_owned(), directory.clone()]);
    }
    arguments.extend([REMOTE_APP_OPTION.to_owned(), id]);
    arguments
}

/// The ID `app` is recorded by, which its launcher opens it with.
fn app_id(server_key: &str, app: &AppKey) -> String {
    let app = match app {
        AppKey::Desktop(id) => RemoteApp::Desktop(id),
        AppKey::AppId(id) => RemoteApp::AppId(id),
        AppKey::Server => RemoteApp::Server,
    };
    identity::app_user_model_id(server_key, app)
}

/// A launcher for each app `server` lists, with the app's name and icon; the
/// launchers of apps it no longer lists go.
fn reconcile(
    server: &str,
    server_name: &str,
    apps: Vec<(String, newport_gpui::remote_app_catalog::Entry)>,
) {
    let Some(launchers) = LAUNCHERS.get() else {
        return;
    };
    let apps: Vec<launchers::App> = apps
        .into_iter()
        .map(|(id, entry)| {
            let key = AppKey::Desktop(id);
            let icon = newport_gpui::remote_app_tile::icon_or_tile(entry.icon_png, &entry.name);
            launchers::App {
                launch: launch(server, server_name, &key, &entry.name, entry.actions),
                key,
                name: entry.name,
                icon_png: icon.map(|icon| icon.to_vec()),
                categories: Vec::new(),
            }
        })
        .collect();
    if let Err(error) = launchers::reconcile(&launchers.writer, server, server_name, &apps) {
        eprintln!("Remote app launchers for {server_name}: {error}");
    }
    keep_entries();
}

/// The registry keeps every app a launcher names, however long unused.
fn keep_entries() {
    let Some(launchers) = LAUNCHERS.get() else {
        return;
    };
    let ids: HashSet<String> = launchers::all(&launchers.writer.root)
        .into_iter()
        .filter_map(|(server, key)| {
            let server_key = newport_backend::remote_app_registry::server_key(&server).ok()?;
            Some(app_id(&server_key, &AppKey::from_stable_id(&key)?))
        })
        .collect();
    launchers.remote_apps.set_launchers(ids);
}

/// The server took a new name: so does its launchers' folder.
pub fn server_renamed(server: &str, name: &str) {
    let Some(launchers) = LAUNCHERS.get() else {
        return;
    };
    let (root, server, name) = (
        launchers.writer.root.clone(),
        server.to_owned(),
        name.to_owned(),
    );
    std::thread::spawn(move || {
        if let Err(error) = launchers::server_folder(&root, &server, &name) {
            eprintln!("Remote app launchers for {name}: {error}");
        }
    });
}

/// The server was removed from Newport: its launchers go.
pub fn server_removed(server: &str) {
    let Some(launchers) = LAUNCHERS.get() else {
        return;
    };
    let (root, server) = (launchers.writer.root.clone(), server.to_owned());
    std::thread::spawn(move || {
        launchers::remove_server(&root, &server);
        keep_entries();
    });
}
