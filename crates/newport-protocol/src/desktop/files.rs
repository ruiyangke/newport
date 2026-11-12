//! Typed desktop files contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    Symlink,
    File,
    Other,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub kind: crate::desktop::files::EntryKind,
    pub size: Option<u64>,
    pub modified: Option<u32>,
    pub permissions: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Listing {
    pub path: String,
    pub entries: Vec<crate::desktop::files::Entry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    Text,
    Image,
}

#[derive(Clone, Debug)]
pub enum Content {
    Text(Box<String>),
    Binary(Box<bytes::Bytes>),
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub kind: crate::desktop::files::PreviewKind,
    pub mime: String,
    pub content: crate::desktop::files::Content,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageReason {
    Size,
    LongLines,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub offset: u64,
    pub next: Option<u64>,
    pub size: u64,
    pub reason: crate::desktop::files::PageReason,
}

#[derive(Clone, Debug)]
pub struct Document {
    pub path: String,
    pub content: String,
    pub revision: String,
    pub page: Option<crate::desktop::files::Page>,
    pub cached: bool,
}

#[derive(Clone, Debug)]
pub struct Index {
    pub paths: Vec<String>,
    pub skipped: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchMode {
    Native,
    Poll,
}

#[derive(Clone, Debug)]
pub struct WatchReply {
    pub mode: crate::desktop::files::WatchMode,
    pub cursor: Option<String>,
    pub paths: Vec<String>,
    pub reset: bool,
}

#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub name: String,
    pub completed: u64,
    pub total: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct ProgressLookup {
    pub progress: Option<crate::desktop::files::TransferProgress>,
}

#[derive(Clone, Debug)]
pub enum NativePath {
    Unix(Box<bytes::Bytes>),
    Windows(Box<Vec<u16>>),
}

#[derive(Clone, Debug)]
pub enum TransferPath {
    Remote(Box<String>),
    Local(Box<crate::desktop::files::NativePath>),
}

#[derive(Clone, Debug)]
pub struct TransferResult {
    pub path: Option<crate::desktop::files::TransferPath>,
}

#[derive(Clone, Debug)]
pub struct ListArgs {
    pub server: uuid::Uuid,
    pub operation: uuid::Uuid,
    pub path: String,
    pub expected: Option<crate::desktop::configuration::ConnectionScope>,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
    pub project_root: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WatchArgs {
    pub server: uuid::Uuid,
    pub operation: uuid::Uuid,
    pub expected: crate::desktop::configuration::ConnectionScope,
    pub root: String,
    pub watch: uuid::Uuid,
    pub directories: Vec<String>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PreviewArgs {
    pub server: uuid::Uuid,
    pub operation: uuid::Uuid,
    pub path: String,
    pub expected: Option<crate::desktop::configuration::ConnectionScope>,
}

#[derive(Clone, Debug)]
pub struct ReadTextArgs {
    pub server: uuid::Uuid,
    pub operation: uuid::Uuid,
    pub root: String,
    pub path: String,
    pub expected: crate::desktop::configuration::ConnectionScope,
    pub revision: Option<String>,
    pub offset: Option<u64>,
    /// Defaults to false when absent on the wire.
    pub allow_cached: bool,
}

#[derive(Clone, Debug)]
pub struct ProjectIndexArgs {
    pub server: uuid::Uuid,
    pub operation: uuid::Uuid,
    pub root: String,
    pub expected: crate::desktop::configuration::ConnectionScope,
}

#[derive(Clone, Debug)]
pub struct WriteTextArgs {
    pub server: uuid::Uuid,
    pub operation: uuid::Uuid,
    pub root: String,
    pub path: String,
    pub expected: crate::desktop::configuration::ConnectionScope,
    pub content: String,
    pub revision: String,
}

#[derive(Clone, Debug)]
pub struct CancelArgs {
    pub operation: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct ProgressArgs {
    pub operation: uuid::Uuid,
}

#[derive(Clone, Debug)]
pub struct UploadArgs {
    pub server: uuid::Uuid,
    pub operation: uuid::Uuid,
    pub path: String,
    pub local_path: crate::desktop::files::NativePath,
    pub expected: crate::desktop::configuration::ConnectionScope,
}

#[derive(Clone, Debug)]
pub struct DownloadArgs {
    pub server: uuid::Uuid,
    pub operation: uuid::Uuid,
    pub path: String,
    pub local_path: crate::desktop::files::NativePath,
    pub expected: crate::desktop::configuration::ConnectionScope,
}

#[derive(Clone, Debug)]
pub enum Operation {
    List(Box<crate::desktop::files::ListArgs>),
    Watch(Box<crate::desktop::files::WatchArgs>),
    Preview(Box<crate::desktop::files::PreviewArgs>),
    ReadText(Box<crate::desktop::files::ReadTextArgs>),
    ProjectIndex(Box<crate::desktop::files::ProjectIndexArgs>),
    WriteText(Box<crate::desktop::files::WriteTextArgs>),
    Cancel(Box<crate::desktop::files::CancelArgs>),
    Progress(Box<crate::desktop::files::ProgressArgs>),
    Upload(Box<crate::desktop::files::UploadArgs>),
    Download(Box<crate::desktop::files::DownloadArgs>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    List(Box<crate::desktop::files::Listing>),
    Watch(Box<crate::desktop::files::WatchReply>),
    Preview(Box<crate::desktop::files::Preview>),
    ReadText(Box<crate::desktop::files::Document>),
    ProjectIndex(Box<crate::desktop::files::Index>),
    WriteText(Box<crate::desktop::files::Document>),
    Cancel(Box<crate::common::Present>),
    Progress(Box<crate::desktop::files::ProgressLookup>),
    Upload(Box<crate::desktop::files::TransferResult>),
    Download(Box<crate::desktop::files::TransferResult>),
}
