//! Remote apps the person adds to the Start menu, from the Remote apps
//! page. The host that owns the registry and the shortcuts installs itself
//! here; without one the page offers nothing.
use std::sync::{Arc, OnceLock};

/// An app on a server, as the page lists it.
#[derive(Clone, Debug)]
pub struct App {
    pub server: String,
    pub server_name: String,
    /// The app's desktop id.
    pub app: String,
    pub name: String,
    pub icon_png: Option<Arc<[u8]>>,
    /// Its desktop actions, which its jump list runs.
    pub actions: Vec<newport_domain::remote_app_registry::Action>,
}

pub trait StartMenu: Send + Sync {
    fn is_added(&self, server: &str, app: &str) -> bool;
    /// Never waits on the shortcut, which is written in the background.
    fn set_added(&self, app: App, added: bool) -> Result<(), String>;
}

static HOST: OnceLock<Box<dyn StartMenu>> = OnceLock::new();

/// Called once, before the window opens.
pub fn install(host: Box<dyn StartMenu>) {
    let _ = HOST.set(host);
}

pub(crate) fn host() -> Option<&'static dyn StartMenu> {
    HOST.get().map(Box::as_ref)
}
