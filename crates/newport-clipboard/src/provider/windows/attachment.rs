//! Typed, fully prepared file copies enter the native clipboard in one commit.
use super::*;
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

#[cfg(test)]
pub(super) static FAIL_FILE_DATA: AtomicBool = AtomicBool::new(false);

struct Global(HGLOBAL);
impl Global {
    fn new(bytes: &[u8]) -> Option<Self> {
        // SAFETY: Plain allocation; a null result is rejected below and the wrapper frees it.
        let memory = Self(unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) });
        if memory.0.is_null() {
            return None;
        }
        // SAFETY: memory.0 is a live moveable allocation.
        let pointer = unsafe { GlobalLock(memory.0) };
        if pointer.is_null() {
            return None;
        }
        // SAFETY: The locked block was allocated with bytes.len() bytes and does not
        // overlap `bytes`; it is unlocked after the copy.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len());
            GlobalUnlock(memory.0);
        }
        Some(memory)
    }
    fn publish(&mut self, format: u32) -> bool {
        // SAFETY: self.0 is a live moveable allocation; on success the system owns it,
        // so the handle is cleared below.
        if unsafe { SetClipboardData(format, self.0) }.is_null() {
            return false;
        }
        self.0 = std::ptr::null_mut();
        true
    }
}
impl Drop for Global {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: The handle is still owned by this wrapper: it was never given to the
            // clipboard.
            unsafe {
                GlobalFree(self.0);
            }
        }
    }
}

struct Prepared {
    data: Global,
    bytes: usize,
    effect: (u32, Global),
    privacy: Vec<(u32, Global)>,
}
impl Prepared {
    fn new(files: &Files, concealed: bool) -> Option<Self> {
        let paths = files
            .paths()
            .iter()
            .map(|path| path.to_str())
            .collect::<Option<Vec<_>>>()?;
        let bytes = crate::windows_attachment::dropfiles(&paths, MAX_BYTES)?;
        let data = Global::new(&bytes)?;
        let effect = (
            clipboard_win::register_format("Preferred DropEffect")?.get(),
            Global::new(&1u32.to_le_bytes())?,
        );
        let privacy = if concealed {
            [
                "ExcludeClipboardContentFromMonitorProcessing",
                "CanIncludeInClipboardHistory",
                "CanUploadToCloudClipboard",
            ]
            .into_iter()
            .map(|name| {
                Some((
                    clipboard_win::register_format(name)?.get(),
                    Global::new(&[0; 4])?,
                ))
            })
            .collect::<Option<Vec<_>>>()?
        } else {
            Vec::new()
        };
        Some(Self {
            data,
            bytes: bytes.len(),
            effect,
            privacy,
        })
    }
}

struct Receipt {
    reply: Option<Reply<FileWrite>>,
    outcome: FileWrite,
}
impl Receipt {
    fn new(reply: Reply<FileWrite>) -> Self {
        Self {
            reply: Some(reply),
            outcome: FileWrite::Unchanged,
        }
    }
    fn closed(&self) -> bool {
        self.reply.as_ref().is_none_or(|reply| reply.is_closed())
    }
    fn defer(&mut self) -> Reply<FileWrite> {
        self.reply.take().unwrap()
    }
}
impl Drop for Receipt {
    fn drop(&mut self) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(Some(self.outcome));
        }
    }
}

impl Window {
    pub(super) fn attach(
        &self,
        hwnd: HWND,
        generation: u64,
        files: Files,
        commit: Arc<Commit>,
        reply: Reply<FileWrite>,
    ) {
        let mut receipt = Receipt::new(reply);
        if receipt.closed() || !commit.pending() {
            return;
        }
        let Some(snapshot) = self.observe() else {
            self.defer(Action::Attach {
                generation,
                files,
                commit,
                reply: receipt.defer(),
            });
            return;
        };
        let import = {
            let state = self.state.borrow();
            state
                .preparation
                .as_ref()
                .filter(|p| p.generation == generation)
                .or_else(|| state.import.as_ref().filter(|p| p.generation == generation))
                .cloned()
        };
        let Some(import) = import.filter(|import| import.owns_baseline()) else {
            return;
        };
        let preparing = match &*import
            .stage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            Stage::Preparing(baseline) if baseline.revision == snapshot.revision => true,
            Stage::Native { files: None } if snapshot.origin == Some(generation) => false,
            _ => return,
        };
        // Every fallible path/format/allocation step precedes native replacement.
        let Some(mut prepared) = Prepared::new(&files, preparing && import.concealed) else {
            return;
        };
        if !import
            .native_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .permits(
                clipboard_win::formats::CF_HDROP,
                prepared.bytes.saturating_add(16),
                publication_limit(MAX_BYTES),
            )
        {
            return;
        }
        let Ok(clipboard) = clipboard_win::Clipboard::new_for(hwnd as _) else {
            self.defer(Action::Attach {
                generation,
                files,
                commit,
                reply: receipt.defer(),
            });
            return;
        };
        if !self.running.load(Ordering::Acquire)
            || !snapshot.is_current()
            || !import.owns_baseline()
            || receipt.closed()
            || !commit.pending()
            || !import.source.publication_confirmed()
            || !commit.begin()
        {
            return;
        }
        // The pure commit claim wins cancellation before any foreign owner callback.
        // No source/transaction lock is held across EmptyClipboard.
        receipt.outcome = FileWrite::Uncertain;
        if preparing {
            // A worker sweep can run concurrently with EmptyClipboard. The old
            // snapshot expires during that call before the new owner is stable.
            // Keep this claim alive until its native stage is installed; source
            // revocation is still checked independently by Import::current.
            *import
                .stage
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Stage::Committing;
            self.state.borrow_mut().preparation = None;
            if clipboard_win::raw::empty().is_err() {
                import.revoke();
                return;
            }
            #[cfg(test)]
            if FAIL_FILE_DATA.load(Ordering::Acquire) && !file_transition_for_fixture(generation) {
                return;
            }
            *import
                .stage
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Stage::Native {
                files: Some(files.clone()),
            };
            self.state.borrow_mut().import = Some(import.clone());
        }
        *import
            .stage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Stage::Native { files: Some(files) };
        let privacy = prepared
            .privacy
            .iter_mut()
            .all(|(format, memory)| memory.publish(*format));
        #[cfg(test)]
        let fail_file_data = FAIL_FILE_DATA.load(Ordering::Acquire);
        #[cfg(not(test))]
        let fail_file_data = false;
        let complete = privacy
            && prepared.effect.1.publish(prepared.effect.0)
            && !fail_file_data
            && prepared.data.publish(clipboard_win::formats::CF_HDROP);
        let mut formats = if preparing {
            Vec::new()
        } else {
            snapshot.formats
        };
        if complete {
            import
                .native_budget
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .record(
                    clipboard_win::formats::CF_HDROP,
                    prepared.bytes.saturating_add(16),
                );
            if !formats.iter().any(|format| format == FILES_FORMAT) {
                formats.push(FILES_FORMAT.into());
            }
            if import.concealed && !formats.iter().any(|format| format == CONCEALED_FORMAT) {
                formats.push(CONCEALED_FORMAT.into());
            }
        }
        let revision = self
            .replace_snapshot(clipboard_sequence(), formats, Some(generation))
            .map(|snapshot| snapshot.revision);
        drop(clipboard);
        if let Some(revision) = revision {
            receipt.outcome = FileWrite::Committed { revision, complete };
        }
    }
}
