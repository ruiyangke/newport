//! Explicit OS integration boundary. The backend owns protocol and recovery;
//! application composition provides desktop thread dispatch and OS access.
use std::sync::Arc;
mod clipboard;
#[cfg(target_os = "macos")]
pub use clipboard::NativeClipboard;
pub use clipboard::{
    ClipboardAccess, ClipboardData, ClipboardFuture, ClipboardOffer, ClipboardOrigin,
    ClipboardPublication, ClipboardSource,
};

pub type Desktop = Arc<dyn DesktopIntegration>;
pub type GraphicalMaintenance = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
/// Setup/account guidance only. The backend acquires the display and returns
/// its environment to the exact shell; UI replies never supply socket paths.
pub type GuiReply = Box<dyn FnOnce(Result<(), String>) + Send>;
/// Whether to download the files a server copy holds.
pub type ClipboardFilesReply = Box<dyn FnOnce(bool) + Send>;

/// The files a server copy would download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardFiles {
    /// Files, folders and links, counted recursively.
    pub items: u64,
    pub bytes: u64,
    /// Some copied item resolves outside the server user's home folder.
    pub outside_home: bool,
    /// Why the person is asked besides where the files are and how many.
    pub hidden: bool,
    pub executable: bool,
    pub installer: bool,
    pub large: bool,
    /// The first few top-level names, long ones cut short.
    pub names: Vec<String>,
    /// How many items were copied at the top level.
    pub top_level: u64,
}

/// How the transfer of copied files is going.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardFilesStatus {
    /// Still running after a few seconds.
    Copying,
    /// Ended without an error to show: done, refused or superseded.
    Finished,
    Failed(String),
    /// A paste of files copied on the Mac got no files on the server.
    UploadFailed(String),
}

pub trait DesktopIntegration: Send + Sync + 'static {
    fn supports_shared_graphical(&self) -> bool {
        false
    }
    /// Bind account policy before returning the worker, then maintain retained
    /// displays independently of UI windows. The backend owns cancellation.
    fn maintain_graphical(&self, _account: crate::account::Account) -> GraphicalMaintenance {
        Box::pin(async {})
    }
    /// Prepare the private client at the application composition boundary.
    /// Await before acquiring graphical SSH demand. Implementations keep local
    /// license/machine checks off executor threads and do not open a display.
    fn graphical_factory(
        &self,
        _license: crate::account::RemoteLicense,
        _settings: crate::preferences::remote_apps::Settings,
    ) -> crate::graphical_session::FactoryFuture {
        Box::pin(async { Err("Shared graphical sessions are unavailable in this build".into()) })
    }
    /// Shared asynchronous native owner, when available. Failure of this owner
    /// must not fall back to UI-thread access. Dropped futures cancel reads.
    fn clipboard_access(&self) -> Option<Arc<dyn ClipboardAccess>> {
        None
    }
    /// The policy router around that same native owner. Integration and GUI
    /// drivers must share this instance; unavailable platforms refuse export.
    fn clipboard_router(&self) -> Option<crate::clipboard_router::Router> {
        None
    }
    /// Queue, rather than block waiting on, the desktop thread. Callers own
    /// deadlines and must not execute cancelled clipboard reads later.
    fn on_main_thread(&self, action: Box<dyn FnOnce() + Send>) -> Result<(), String>;
    /// Legacy access, used only when clipboard_access is absent and called
    /// only from the dispatched desktop closure.
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
    /// Ask before downloading the files of a server copy. `cancelled` turns
    /// true once the copy is no longer current or the question has expired,
    /// and the question should then close; an unanswered question is a no.
    fn request_clipboard_files(
        &self,
        _server: &str,
        _files: ClipboardFiles,
        reply: ClipboardFilesReply,
        _cancelled: Arc<std::sync::atomic::AtomicBool>,
    ) {
        reply(false);
    }
    /// Progress and failures of the download of a server copy's files.
    fn clipboard_files_status(&self, _server: &str, _status: ClipboardFilesStatus) {}
    /// The shipping application's version, reported to the Newport account.
    fn app_version(&self) -> String {
        env!("CARGO_PKG_VERSION").into()
    }
}
