//! What the Remote apps page adds to the Start menu: the app's ID, as its
//! windows carry it, marked as added in the registry, whose worker writes
//! the shortcut and icon.
use newport_backend::remote_app_registry::{self as registry, RemoteApps, StartMenuApp};
use newport_domain::{
    remote_app_identity::{self as identity, RemoteApp},
    remote_app_registry::App,
};
use newport_remote_provider_stubs::names::display_name;

pub struct Host {
    remote_apps: RemoteApps,
}

impl Host {
    pub fn new(remote_apps: RemoteApps) -> Self {
        Self { remote_apps }
    }
}

fn app_id(server: &str, app: &str) -> Result<String, String> {
    Ok(identity::app_user_model_id(
        &registry::server_key(server)?,
        RemoteApp::Desktop(app),
    ))
}

impl newport_gpui::start_menu::StartMenu for Host {
    fn is_added(&self, server: &str, app: &str) -> bool {
        app_id(server, app).is_ok_and(|id| self.remote_apps.is_added(&id))
    }

    fn set_added(&self, app: newport_gpui::start_menu::App, added: bool) -> Result<(), String> {
        self.remote_apps.set_added(
            StartMenuApp {
                id: app_id(&app.server, &app.app)?,
                server: app.server,
                server_name: display_name(&app.server_name),
                app: App::DesktopId(app.app),
                // The name its windows record.
                name: display_name(&app.name),
                icon: newport_gpui::remote_app_tile::icon_or_tile(app.icon_png, &app.name),
                actions: app.actions,
            },
            added,
        )
    }
}
