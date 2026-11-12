use super::*;
use newport_platform::clipboard::files::{
    DownloadDirectory, RetainedFiles, cache_base, sweep_cache,
};

/// Windows launches these formats even without a Unix executable permission bit.
pub(super) fn risk(name: &str) -> (bool, bool) {
    let lower = name.to_ascii_lowercase();
    let extension = lower.rsplit('.').next().unwrap_or_default();
    let installer = matches!(
        extension,
        "msi" | "msp" | "msix" | "msixbundle" | "appx" | "appxbundle"
    );
    let executable = installer
        || matches!(
            extension,
            "exe"
                | "com"
                | "scr"
                | "bat"
                | "cmd"
                | "ps1"
                | "psm1"
                | "vbs"
                | "vbe"
                | "js"
                | "jse"
                | "wsf"
                | "wsh"
                | "hta"
                | "lnk"
                | "url"
                | "reg"
                | "cpl"
                | "msc"
                | "chm"
        );
    (executable, installer)
}

/// Windows cache ownership is represented by a validated lease, not a pathname.
/// A released lease leaves files on disk through the native paste grace period.
struct Job {
    id: u64,
    cancel: Arc<AtomicBool>,
    files: Option<RetainedFiles>,
}
#[derive(Default)]
struct State {
    next: u64,
    current: Option<Job>,
}
impl State {
    fn supersede(&mut self) -> Option<Job> {
        let previous = self.current.take();
        if let Some(job) = &previous {
            job.cancel.store(true, Ordering::Release);
        }
        previous
    }
}
#[derive(Clone)]
pub(crate) struct Ticket {
    id: u64,
    cancel: Arc<AtomicBool>,
    fence: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl Ticket {
    pub(crate) fn is_current(&self) -> bool {
        !self.cancel.load(Ordering::Acquire) && (self.fence)()
    }
}
pub(crate) struct Cache {
    root: PathBuf,
    state: Mutex<State>,
    swept: Mutex<Option<std::time::Instant>>,
}
impl Cache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            state: Mutex::default(),
            swept: Mutex::default(),
        }
    }
    pub(crate) fn default_root(profile: &Path) -> Option<PathBuf> {
        use sha2::Digest;
        let profile = profile
            .canonicalize()
            .or_else(|_| std::path::absolute(profile))
            .unwrap_or_else(|_| profile.to_owned());
        let name: String = sha2::Sha256::digest(profile.as_os_str().as_encoded_bytes())[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Some(cache_base()?.join(name))
    }
    pub(crate) fn begin(&self, fence: impl Fn() -> bool + Send + Sync + 'static) -> Ticket {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = state.supersede();
        state.next += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let id = state.next;
        state.current = Some(Job {
            id,
            cancel: cancel.clone(),
            files: None,
        });
        let ticket = Ticket {
            id,
            cancel,
            fence: Arc::new(fence),
        };
        drop(state);
        drop(previous);
        ticket
    }
    pub(crate) fn retire(&self, ticket: &Ticket) {
        ticket.cancel.store(true, Ordering::Release);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = if state
            .current
            .as_ref()
            .is_some_and(|job| job.id == ticket.id)
        {
            state.supersede()
        } else {
            None
        };
        drop(state);
        drop(previous);
    }
    pub(crate) async fn fetch<R: RemoteFs>(
        &self,
        fs: &R,
        plan: &Plan,
        ticket: &Ticket,
    ) -> Result<PathBuf> {
        let files = fetch(fs, plan, &self.root, ticket).await?;
        let path = files.folder().to_owned();
        let current = ticket.is_current();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(job) = state
            .current
            .as_mut()
            .filter(|job| job.id == ticket.id && current)
        else {
            bail!(SUPERSEDED);
        };
        let previous = job.files.replace(files);
        drop(state);
        drop(previous);
        Ok(path)
    }
    pub(crate) fn tick(&self) {
        let now = std::time::Instant::now();
        let mut swept = self
            .swept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if swept.is_some_and(|last| now.duration_since(last) < Duration::from_secs(60)) {
            return;
        }
        *swept = Some(now);
        drop(swept);
        let root = self.root.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn_blocking(move || {
                let _ = sweep_cache(&root, SystemTime::now());
            }));
        }
    }
    pub(crate) fn clear(&self) {
        // Existing Explorer pastes can outlive this process. Releasing leases
        // updates last use; future admission reclaims only after the grace.
        let previous = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .supersede();
        drop(previous);
    }
}

async fn fetch<R: RemoteFs>(
    fs: &R,
    plan: &Plan,
    root: &Path,
    ticket: &Ticket,
) -> Result<RetainedFiles> {
    if decide(plan) == Decision::Refuse {
        bail!(TOO_LARGE);
    }
    if !ticket.is_current() {
        bail!(SUPERSEDED);
    }
    let (root, bytes, entries) = (root.to_owned(), plan.bytes, plan.count);
    let staging = Arc::new(
        tokio::task::spawn_blocking(move || DownloadDirectory::create(&root, bytes, entries))
            .await??,
    );
    let mut budget = MAX_BYTES;
    for entry in &plan.entries {
        if !ticket.is_current() {
            bail!(SUPERSEDED);
        }
        let entry = entry.clone();
        match &entry.kind {
            EntryKind::Directory => {
                let stage = staging.clone();
                tokio::task::spawn_blocking(move || stage.create_dir(&entry.path)).await??;
            }
            EntryKind::File { size, mtime } => {
                let (stage, path) = (staging.clone(), entry.path.clone());
                let file = tokio::task::spawn_blocking(move || stage.create_file(&path)).await??;
                let mut file = tokio::fs::File::from_std(file);
                let handle = fs.open_read(&entry.source).await?;
                let open = &handle;
                let copied = crate::files::transfer::read_file_capped(
                    &mut file,
                    |offset, len| async move {
                        if !ticket.is_current() {
                            bail!(SUPERSEDED);
                        }
                        fs.read(open, offset, len).await
                    },
                    Some(*size),
                    Some((*size).min(budget)),
                    |_| {},
                )
                .await;
                let closed = fs.close(handle).await;
                let copied = match copied {
                    Err(error) if error.is::<LimitExceeded>() => {
                        bail!(if budget < *size { TOO_LARGE } else { CHANGED })
                    }
                    other => other?,
                };
                closed?;
                file.flush().await?;
                drop(file.into_std().await);
                let after = fs.lstat(&entry.source).await?;
                if copied != *size
                    || after.is_none_or(|after| {
                        after.kind != Kind::File || after.size != *size || after.mtime != *mtime
                    })
                {
                    bail!(CHANGED);
                }
                budget -= copied;
                let (stage, path, mtime) = (staging.clone(), entry.path.clone(), *mtime);
                tokio::task::spawn_blocking(move || stage.finish_file(&path, mtime)).await??;
            }
        }
    }
    if !ticket.is_current() {
        bail!(SUPERSEDED);
    }
    let staging = Arc::try_unwrap(staging)
        .map_err(|_| anyhow!("A clipboard cache writer is still running"))?;
    let files = tokio::task::spawn_blocking(move || staging.finish()).await??;
    if !ticket.is_current() {
        bail!(SUPERSEDED);
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_launchable_formats_require_confirmation_without_unix_execute_bits() {
        for name in [
            "program.EXE",
            "setup.msi",
            "shortcut.lnk",
            "page.url",
            "script.ps1",
            "document.chm",
            "settings.reg",
        ] {
            assert!(risk(name).0, "{name}");
        }
        assert!(risk("bundle.MSIXBUNDLE").1);
        assert!(!risk("photo.png").0);
        assert!(!risk("program.exe.txt").0);
    }
}
