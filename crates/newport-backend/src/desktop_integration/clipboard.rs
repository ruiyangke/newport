pub use newport_platform::clipboard::provider::Payload as ClipboardData;
pub use newport_platform::clipboard::provider::Source as ClipboardSource;
use std::{future::Future, pin::Pin, sync::Arc};

pub type ClipboardFuture<T> = Pin<Box<dyn Future<Output = Option<T>> + Send>>;

/// Native identity is distinct from clipboard contents. In particular, an
/// imported promise is not a new local copy, even if it advertises no formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardOrigin {
    Local,
    Imported { publication: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardOffer {
    pub revision: isize,
    pub formats: Vec<String>,
    pub origin: ClipboardOrigin,
}
impl ClipboardOffer {
    pub fn local(revision: isize, formats: Vec<String>) -> Self {
        Self {
            revision,
            formats,
            origin: ClipboardOrigin::Local,
        }
    }
}

/// A native promise owned by the shared router. Drop revokes it; implementations
/// must preserve a newer local clipboard when withdrawing an older promise.
/// When its source becomes invalid, the native owner withdraws the promise and
/// reports empty formats with its imported origin (or a newer owner's offer).
pub trait ClipboardPublication: Send {
    fn is_current(&self) -> bool;
    /// Matches the imported origin returned by this same native owner. The
    /// router records the remote source under this ID, including across helper
    /// restarts. The number is not transferable between different native owners.
    fn generation(&self) -> u64;
    /// Replace this publication's offer with the downloaded files in `folder`,
    /// as long as the native clipboard still holds it. Yields the new native
    /// revision, or None when refused or unsupported.
    fn attach_files(&self, _folder: &std::path::Path) -> ClipboardFuture<isize> {
        Box::pin(async { None })
    }
}

/// One native owner shared across all backend integration connections. Offers
/// enumerate metadata only. Reads must reject stale/unadvertised formats, and
/// returned data must retain its source/connection fence until delivery ends.
/// Constructing a future must not perform I/O; dropping it cancels pending work.
pub trait ClipboardAccess: Send + Sync + 'static {
    fn offer(&self, previous: Option<isize>) -> ClipboardFuture<ClipboardOffer>;
    fn read(&self, revision: isize, format: String) -> ClipboardFuture<Box<dyn ClipboardData>>;
    /// Publish metadata only, conditional on the native logical revision still
    /// being current. Source callbacks queue remote demand without doing I/O.
    /// Unsupported adapters refuse publication instead of using another owner.
    fn publish(
        &self,
        _expected: isize,
        _formats: Vec<String>,
        source: Arc<dyn ClipboardSource>,
    ) -> ClipboardFuture<Box<dyn ClipboardPublication>> {
        if source.begin_publication() {
            source.publication_revoked();
        }
        Box::pin(async { None })
    }
}

/// The macOS implementation never opens a native clipboard in this process.
/// Composition supplies the one owner and owns its teardown. The optional owner
/// represents an unavailable executable, not permission to use a UI fallback.
#[cfg(target_os = "macos")]
pub struct NativeClipboard(Option<newport_platform::clipboard::helper::owner::Owner>);
#[cfg(target_os = "macos")]
impl NativeClipboard {
    pub fn new(owner: Option<newport_platform::clipboard::helper::owner::Owner>) -> Self {
        Self(owner)
    }
    pub fn close(&self) {
        if let Some(owner) = &self.0 {
            owner.close();
        }
    }
    pub async fn shutdown(&self) {
        if let Some(owner) = &self.0 {
            owner.shutdown().await;
        }
    }
}
#[cfg(target_os = "macos")]
impl ClipboardAccess for NativeClipboard {
    fn offer(&self, previous: Option<isize>) -> ClipboardFuture<ClipboardOffer> {
        let owner = self.0.clone();
        Box::pin(async move {
            let snapshot = owner?.snapshot().await?;
            if !snapshot.is_current() || previous == Some(snapshot.revision) {
                return None;
            }
            Some(ClipboardOffer {
                revision: snapshot.revision,
                formats: snapshot.formats,
                origin: snapshot
                    .origin
                    .map_or(ClipboardOrigin::Local, |publication| {
                        ClipboardOrigin::Imported { publication }
                    }),
            })
        })
    }
    fn read(&self, revision: isize, format: String) -> ClipboardFuture<Box<dyn ClipboardData>> {
        let owner = self.0.clone();
        Box::pin(async move {
            let data = owner?.read(revision, format).await?;
            Some(Box::new(data) as Box<dyn ClipboardData>)
        })
    }
    fn publish(
        &self,
        expected: isize,
        formats: Vec<String>,
        source: Arc<dyn ClipboardSource>,
    ) -> ClipboardFuture<Box<dyn ClipboardPublication>> {
        let Some(owner) = self.0.as_ref() else {
            if source.begin_publication() {
                source.publication_revoked();
            }
            return Box::pin(async { None });
        };
        let pending = owner.publish(expected, formats, source);
        let owner = owner.clone();
        Box::pin(async move {
            let publication = pending.await?;
            Some(Box::new(NativePublication { publication, owner }) as Box<dyn ClipboardPublication>)
        })
    }
}

#[cfg(target_os = "macos")]
struct NativePublication {
    publication: newport_platform::clipboard::helper::owner::Publication,
    owner: newport_platform::clipboard::helper::owner::Owner,
}
#[cfg(target_os = "macos")]
impl ClipboardPublication for NativePublication {
    fn is_current(&self) -> bool {
        self.publication.is_current()
    }
    fn generation(&self) -> u64 {
        self.publication.generation
    }
    fn attach_files(&self, folder: &std::path::Path) -> ClipboardFuture<isize> {
        let Some(folder) = crate::clipboard_downloads::attachable(folder) else {
            return Box::pin(async { None });
        };
        Box::pin(self.owner.attach_files(self.publication.generation, folder))
    }
}
