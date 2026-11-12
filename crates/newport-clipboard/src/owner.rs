//! The one lazy native owner interface. Each desktop OS implements it with its
//! supervised helper, so callers hold one owner type whatever the OS.
use crate::provider::{Payload, Source};
use std::{future::Future, path::Path, pin::Pin, sync::Arc};

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

/// A lazy native owner: one per desktop OS, running its native calls outside
/// this process. Constructing a future does no I/O; dropping it cancels.
pub trait NativeOwner: Clone + Send + Sync + 'static {
    type Publication: NativePublication;
    /// The current native offer, None once it is no longer current.
    fn snapshot(&self) -> ClipboardFuture<ClipboardOffer>;
    fn read(&self, revision: isize, format: String) -> ClipboardFuture<Box<dyn Payload>>;
    /// Claims the source before the future is first polled; the owner then
    /// confirms or revokes it.
    fn publish(
        &self,
        expected: isize,
        formats: Vec<String>,
        source: Arc<dyn Source>,
    ) -> ClipboardFuture<Self::Publication>;
    /// Offers the downloaded files in `folder` in place of publication
    /// `generation`. Yields the new native revision.
    fn attach_files(&self, generation: u64, folder: &Path) -> ClipboardFuture<isize>;
    /// Fence and stop the owner permanently.
    fn close(&self);
    /// Await the helper's teardown.
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
}

/// The native owner's handle for one publication. Dropping it revokes the
/// publication.
pub trait NativePublication: Send + 'static {
    fn is_current(&self) -> bool;
    /// Matches the imported origin this owner reports for the publication.
    fn generation(&self) -> u64;
    /// A file preparation reserves this unchanged revision without becoming
    /// its native owner. None means a published native promise.
    fn preparing_revision(&self) -> Option<isize> {
        None
    }
    /// Atomically retire an uncommitted file reservation. False means that no
    /// reservation exists or native admission already won.
    fn cancel_preparation(&self) -> bool {
        false
    }
}

#[cfg(any(target_os = "macos", windows))]
fn offer(revision: isize, formats: Vec<String>, origin: Option<u64>) -> ClipboardOffer {
    ClipboardOffer {
        revision,
        formats,
        origin: origin.map_or(ClipboardOrigin::Local, |publication| {
            ClipboardOrigin::Imported { publication }
        }),
    }
}

#[cfg(target_os = "macos")]
impl NativeOwner for crate::helper::owner::Owner {
    type Publication = crate::helper::owner::Publication;
    fn snapshot(&self) -> ClipboardFuture<ClipboardOffer> {
        let owner = self.clone();
        Box::pin(async move {
            let snapshot = owner.snapshot().await?;
            snapshot
                .is_current()
                .then(|| offer(snapshot.revision, snapshot.formats, snapshot.origin))
        })
    }
    fn read(&self, revision: isize, format: String) -> ClipboardFuture<Box<dyn Payload>> {
        let owner = self.clone();
        Box::pin(async move {
            let data = owner.read(revision, format).await?;
            Some(Box::new(data) as Box<dyn Payload>)
        })
    }
    fn publish(
        &self,
        expected: isize,
        formats: Vec<String>,
        source: Arc<dyn Source>,
    ) -> ClipboardFuture<Self::Publication> {
        Box::pin(self.publish(expected, formats, source))
    }
    fn attach_files(&self, generation: u64, folder: &Path) -> ClipboardFuture<isize> {
        let Some(folder) = crate::helper::attachable(folder) else {
            return Box::pin(async { None });
        };
        Box::pin(self.attach_files(generation, folder))
    }
    fn close(&self) {
        self.close();
    }
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let owner = self.clone();
        Box::pin(async move {
            owner.shutdown().await;
            Ok(())
        })
    }
}
#[cfg(target_os = "macos")]
impl NativePublication for crate::helper::owner::Publication {
    fn is_current(&self) -> bool {
        self.is_current()
    }
    fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(windows)]
impl NativeOwner for crate::provider::windows::Owner {
    type Publication = crate::provider::windows::Publication;
    fn snapshot(&self) -> ClipboardFuture<ClipboardOffer> {
        let owner = self.clone();
        Box::pin(async move {
            let snapshot = owner.snapshot().await?;
            snapshot
                .is_current()
                .then(|| offer(snapshot.revision, snapshot.formats, snapshot.origin))
        })
    }
    fn read(&self, revision: isize, format: String) -> ClipboardFuture<Box<dyn Payload>> {
        let owner = self.clone();
        Box::pin(
            async move { Some(Box::new(owner.read(revision, format).await?) as Box<dyn Payload>) },
        )
    }
    fn publish(
        &self,
        expected: isize,
        formats: Vec<String>,
        source: Arc<dyn Source>,
    ) -> ClipboardFuture<Self::Publication> {
        Box::pin(self.publish(expected, formats, source))
    }
    fn attach_files(&self, generation: u64, folder: &Path) -> ClipboardFuture<isize> {
        let Some(open) = self.files() else {
            return Box::pin(async { None });
        };
        let folder = folder.to_owned();
        let owner = self.clone();
        Box::pin(async move {
            let files = tokio::task::spawn_blocking(move || open(&folder))
                .await
                .ok()?
                .ok()?;
            owner.attach_files(generation, files).await
        })
    }
    fn close(&self) {
        self.close();
    }
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let owner = self.clone();
        Box::pin(async move { owner.shutdown().await })
    }
}
#[cfg(windows)]
impl NativePublication for crate::provider::windows::Publication {
    fn is_current(&self) -> bool {
        self.is_current()
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    fn preparing_revision(&self) -> Option<isize> {
        self.preparing_revision()
    }
    fn cancel_preparation(&self) -> bool {
        self.cancel_preparation()
    }
}
