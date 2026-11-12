pub use newport_platform::clipboard::owner::{ClipboardFuture, ClipboardOffer, ClipboardOrigin};
#[cfg(any(target_os = "macos", windows))]
pub use newport_platform::clipboard::owner::{NativeOwner, NativePublication};
pub use newport_platform::clipboard::provider::Payload as ClipboardData;
pub use newport_platform::clipboard::provider::Source as ClipboardSource;
use std::sync::Arc;

/// A native promise owned by the shared router. Drop revokes it; implementations
/// must preserve a newer local clipboard when withdrawing an older promise.
/// When its source becomes invalid, the native owner withdraws the promise and
/// reports empty formats with its imported origin (or a newer owner's offer).
pub trait ClipboardPublication: Send {
    fn is_current(&self) -> bool;
    /// A file preparation reserves this unchanged revision without becoming
    /// its native owner. None means a published native promise.
    fn preparing_revision(&self) -> Option<isize> {
        None
    }
    /// Atomically retire an uncommitted file reservation. False means that no
    /// reservation exists or native admission already won; dropping the download
    /// must not then revoke the publication or discard its retained files.
    fn cancel_preparation(&self) -> bool {
        false
    }
    /// Matches the imported origin returned by this same native owner. The
    /// router records the remote source under this ID, including across helper
    /// restarts. The number is not transferable between different native owners.
    fn generation(&self) -> u64;
    /// Replace this publication's offer with the downloaded files in `folder`,
    /// as long as the native clipboard still holds it. Yields the new native
    /// revision when complete. None can also report a partial or uncertain native
    /// outcome; callers must use cancel_preparation rather than infer cancellation.
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

#[cfg(target_os = "macos")]
type PlatformOwner = newport_platform::clipboard::helper::owner::Owner;
#[cfg(windows)]
type PlatformOwner = newport_platform::clipboard::provider::windows::Owner;

/// Never opens a native clipboard in this process. Composition supplies the
/// one owner and owns its teardown. The optional owner represents an
/// unavailable executable, not permission to use a UI fallback.
#[cfg(any(target_os = "macos", windows))]
pub struct NativeClipboard<O: NativeOwner = PlatformOwner>(Option<O>);
#[cfg(any(target_os = "macos", windows))]
impl<O: NativeOwner> NativeClipboard<O> {
    pub fn new(owner: Option<O>) -> Self {
        Self(owner)
    }
    pub fn close(&self) {
        if let Some(owner) = &self.0 {
            owner.close();
        }
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        match &self.0 {
            Some(owner) => owner.shutdown().await,
            None => Ok(()),
        }
    }
}
#[cfg(any(target_os = "macos", windows))]
impl<O: NativeOwner> ClipboardAccess for NativeClipboard<O> {
    fn offer(&self, previous: Option<isize>) -> ClipboardFuture<ClipboardOffer> {
        let pending = self.0.as_ref().map(|owner| owner.snapshot());
        Box::pin(async move {
            pending?
                .await
                .filter(|offer| previous != Some(offer.revision))
        })
    }
    fn read(&self, revision: isize, format: String) -> ClipboardFuture<Box<dyn ClipboardData>> {
        let pending = self.0.as_ref().map(|owner| owner.read(revision, format));
        Box::pin(async move { pending?.await })
    }
    fn publish(
        &self,
        expected: isize,
        formats: Vec<String>,
        source: Arc<dyn ClipboardSource>,
    ) -> ClipboardFuture<Box<dyn ClipboardPublication>> {
        let Some(owner) = self.0.clone() else {
            if source.begin_publication() {
                source.publication_revoked();
            }
            return Box::pin(async { None });
        };
        let pending = owner.publish(expected, formats, source);
        Box::pin(async move {
            let publication = pending.await?;
            Some(Box::new(OwnedPublication { publication, owner }) as Box<dyn ClipboardPublication>)
        })
    }
}

#[cfg(any(target_os = "macos", windows))]
struct OwnedPublication<O: NativeOwner> {
    publication: O::Publication,
    owner: O,
}
#[cfg(any(target_os = "macos", windows))]
impl<O: NativeOwner> ClipboardPublication for OwnedPublication<O> {
    fn is_current(&self) -> bool {
        self.publication.is_current()
    }
    fn generation(&self) -> u64 {
        self.publication.generation()
    }
    fn preparing_revision(&self) -> Option<isize> {
        self.publication.preparing_revision()
    }
    fn cancel_preparation(&self) -> bool {
        self.publication.cancel_preparation()
    }
    fn attach_files(&self, folder: &std::path::Path) -> ClipboardFuture<isize> {
        self.owner
            .attach_files(self.publication.generation(), folder)
    }
}

#[cfg(all(test, any(target_os = "macos", windows)))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Claims {
        begun: AtomicUsize,
        revoked: AtomicUsize,
    }
    impl ClipboardSource for Claims {
        fn is_current(&self) -> bool {
            true
        }
        fn request(&self, _: &str, _: newport_platform::clipboard::provider::Reply) {}
        fn begin_publication(&self) -> bool {
            self.begun.fetch_add(1, Ordering::AcqRel);
            true
        }
        fn publication_revoked(&self) {
            self.revoked.fetch_add(1, Ordering::AcqRel);
        }
    }

    #[tokio::test]
    async fn publish_without_owner_revokes_source() {
        let source = Arc::new(Claims::default());
        let clipboard: NativeClipboard = NativeClipboard::new(None);
        let publication = clipboard.publish(1, vec!["text/plain".into()], source.clone());
        assert!(publication.await.is_none());
        assert_eq!(source.begun.load(Ordering::Acquire), 1);
        assert_eq!(source.revoked.load(Ordering::Acquire), 1);
    }
}
