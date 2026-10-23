//! Explicit OS integration boundary. The backend owns protocol and recovery;
//! application composition provides desktop thread dispatch and OS access.
use std::sync::Arc;

pub type Desktop = Arc<dyn DesktopIntegration>;
pub type GuiReply =
    Box<dyn FnOnce(Result<newport_domain::terminal::GuiEnvironment, String>) + Send>;

pub trait DesktopIntegration: Send + Sync + 'static {
    /// Queue, rather than block waiting on, the desktop thread. Callers own
    /// deadlines and must not execute cancelled clipboard reads later.
    fn on_main_thread(&self, action: Box<dyn FnOnce() + Send>) -> Result<(), String>;
    /// Clipboard methods are called only from the dispatched desktop closure.
    fn clipboard_revision(&self) -> Option<isize>;
    fn clipboard_offer(&self, previous: Option<isize>) -> Option<(isize, Vec<String>)>;
    fn read_clipboard_format(&self, revision: isize, format: &str) -> Option<Vec<u8>>;
    /// Called on a blocking worker after URL validation and consent checks.
    fn open_url(&self, url: &str) -> Result<(), String>;
    /// GUI guidance is separate from clipboard and browser permissions.
    fn supports_gui_notices(&self) -> bool {
        false
    }
    fn request_gui_setup(&self, _server: &str) {}
    fn request_gui_launch(
        &self,
        _server: &str,
        _terminal: &str,
        _application: &str,
        reply: GuiReply,
        _cancelled: Arc<std::sync::atomic::AtomicBool>,
    ) {
        reply(Err("GUI launch is unavailable in this application.".into()));
    }
    /// The shipping application's version, reported to the Newport account.
    fn app_version(&self) -> String {
        env!("CARGO_PKG_VERSION").into()
    }
}
