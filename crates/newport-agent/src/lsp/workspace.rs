//! Debounced recursive worktree notifications.
use super::*;
use newport_protocol::lsp::{FileChange, FileChangeKind, FilesChanged, WatchError};
use notify::{
    EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{ModifyKind, RenameMode},
};
use std::collections::BTreeMap;

#[derive(Default)]
struct Pending {
    events: BTreeMap<PathBuf, FileChangeKind>,
    error: Option<String>,
}
impl Pending {
    fn receive(&mut self, root: &Path, event: notify::Result<notify::Event>) {
        let event = match event {
            Ok(event) if event.need_rescan() => {
                self.error = Some(
                    "File watching lost events. Restart the language server to rescan.".into(),
                );
                return;
            }
            Ok(event) => event,
            Err(error) => {
                self.error = Some(format!("File watching failed: {error}"));
                return;
            }
        };
        for (index, path) in event.paths.into_iter().enumerate() {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            if relative.components().any(|c| c.as_os_str() == ".git") {
                continue;
            }
            use FileChangeKind::{Changed, Created, Deleted};
            let kind = match event.kind {
                EventKind::Create(_) => Created,
                EventKind::Remove(_) => Deleted,
                EventKind::Modify(ModifyKind::Name(RenameMode::From)) => Deleted,
                EventKind::Modify(ModifyKind::Name(RenameMode::To)) => Created,
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                    if index == 0 {
                        Deleted
                    } else {
                        Created
                    }
                }
                EventKind::Modify(_) => Changed,
                _ => continue,
            };
            if self.events.len() >= 4096 && !self.events.contains_key(&path) {
                self.error = Some(
                    "Too many file changes to track. Restart the language server to rescan.".into(),
                );
                continue;
            }
            // Preserve creates through writes; delete/recreate means changed.
            let kind = match (self.events.get(&path), kind) {
                (Some(Created), Changed) => Created,
                (Some(Deleted), Created) => Changed,
                _ => kind,
            };
            self.events.insert(path, kind);
        }
    }
}

pub(super) fn watch(
    root: &Path,
    output: Output,
    stopping: Arc<AtomicBool>,
) -> io::Result<RecommendedWatcher> {
    let pending = Arc::new(Mutex::new(Pending::default()));
    let callback = pending.clone();
    let root = root.to_owned();
    let watched = root.clone();
    let mut watcher = notify::recommended_watcher(move |event| {
        callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .receive(&watched, event)
    })
    .map_err(io::Error::other)?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(io::Error::other)?;
    std::thread::spawn(move || {
        while !stopping.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(200));
            if stopping.load(Ordering::Acquire) {
                break;
            }
            let pending = std::mem::take(
                &mut *pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            if let Some(error) = pending.error {
                let _ = send(&output, &WatchError { error });
            }
            let changes: Vec<_> = pending
                .events
                .into_iter()
                .filter_map(|(path, kind)| {
                    let uri = url::Url::from_file_path(path).ok()?;
                    Some(FileChange {
                        uri: uri.into(),
                        kind,
                    })
                })
                .collect();
            if !changes.is_empty() && send(&output, &FilesChanged { changes }).is_err() {
                break;
            }
        }
    });
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watcher_preserves_create_delete_and_rename_kinds() {
        let mut pending = Pending::default();
        let root = Path::new("/repo");
        let event = |kind, path: &str| Ok(notify::Event::new(kind).add_path(path.into()));
        pending.receive(
            root,
            event(
                EventKind::Create(notify::event::CreateKind::File),
                "/repo/a",
            ),
        );
        pending.receive(root, event(EventKind::Modify(ModifyKind::Any), "/repo/a"));
        assert_eq!(
            pending.events[Path::new("/repo/a")],
            FileChangeKind::Created
        );
        pending.receive(
            root,
            Ok(
                notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                    .add_path("/repo/a".into())
                    .add_path("/repo/b".into()),
            ),
        );
        assert_eq!(
            pending.events[Path::new("/repo/a")],
            FileChangeKind::Deleted
        );
        assert_eq!(
            pending.events[Path::new("/repo/b")],
            FileChangeKind::Created
        );
        pending.receive(
            root,
            event(EventKind::Modify(ModifyKind::Any), "/repo/.git/index"),
        );
        pending.receive(
            root,
            event(EventKind::Modify(ModifyKind::Any), "/elsewhere/a"),
        );
        assert_eq!(pending.events.len(), 2);
    }
}
