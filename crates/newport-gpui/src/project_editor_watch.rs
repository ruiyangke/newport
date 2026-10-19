//! Incremental VFS observation. No recursive scans, loading banners, or timer per
//! document. Hidden workspaces stop renewing their remote watch lease.
use super::*;
use std::{collections::BTreeSet, time::Instant};

pub(super) struct State {
    id: uuid::Uuid,
    cursor: Option<String>,
    busy: bool,
    active: bool,
    next: Option<Instant>,
    folders: BTreeSet<String>,
    files: BTreeSet<String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            cursor: None,
            busy: false,
            active: false,
            next: None,
            folders: Default::default(),
            files: Default::default(),
        }
    }
}
impl CodeWorkspace {
    pub(super) fn suspend_file_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.watch.busy || !self.watch.active {
            return;
        }
        self.watch.active = false;
        self.watch.cursor = None;
        self.watch.next = None;
        self.rpc(
            "files_watch",
            json!({"id":self.server,"operation":uuid::Uuid::new_v4().to_string(),
            "expectedConnection":self.connection,"root":self.root,"watch":self.watch.id,
            "directories":[],"cursor":null}),
            window,
            cx,
            |_, _, _, _| {},
        );
    }
    fn watch_directories(&self) -> BTreeSet<String> {
        let mut directories = BTreeSet::from([self.root.clone()]);
        // Open documents have priority over off-screen expanded directories.
        for buffer in &self.buffers {
            if let Some(parent) = std::path::Path::new(&buffer.path)
                .parent()
                .and_then(|p| p.to_str())
            {
                directories.insert(parent.into());
            }
        }
        for row in self
            .tree_rows()
            .into_iter()
            .filter(|row| row.directory && self.expanded.contains(&row.path))
        {
            if directories.len() >= 64 {
                break;
            }
            directories.insert(row.path);
        }
        directories
    }
    pub(super) fn poll_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.available
            || !self.visible
            || self.watch.busy
            || self.watch.next.is_some_and(|at| at > Instant::now())
        {
            return;
        }
        self.watch.busy = true;
        self.watch.active = true;
        let directories = self.watch_directories();
        let args = json!({"id":self.server,"operation":uuid::Uuid::new_v4().to_string(),
            "expectedConnection":self.connection,"root":self.root,"watch":self.watch.id,
            "directories":directories,"cursor":self.watch.cursor});
        self.rpc(
            "files_watch",
            args,
            window,
            cx,
            move |this, result, window, cx| {
                this.watch.busy = false;
                let delay = match &result {
                    Ok(value) if value["mode"] == "native" => 2,
                    _ => 10,
                };
                this.watch.next = Some(Instant::now() + Duration::from_secs(delay));
                if let Ok(value) = result {
                    this.watch.cursor = value["cursor"].as_str().map(str::to_owned);
                    if this.available && this.visible {
                        this.apply_file_events(&value, &directories);
                        this.refresh_watched_files(window, cx);
                    } else {
                        // Events drained while hidden must be reconciled on return.
                        this.watch.cursor = None;
                    }
                } else {
                    this.watch.cursor = None;
                }
                if !this.available || !this.visible {
                    this.suspend_file_watch(window, cx);
                }
                if this.available && this.visible {
                    cx.spawn_in(window, async move |view, cx| {
                        cx.background_executor()
                            .timer(Duration::from_secs(delay))
                            .await;
                        let _ = view.update_in(cx, |view, window, cx| view.poll_files(window, cx));
                    })
                    .detach();
                }
            },
        );
    }
    pub(super) fn apply_file_events(&mut self, value: &Value, directories: &BTreeSet<String>) {
        let paths: BTreeSet<_> = value["paths"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let reset = value["reset"] == true;
        for directory in directories {
            if (reset
                || paths.iter().any(|p| {
                    *p == directory
                        || std::path::Path::new(p).parent().and_then(|p| p.to_str())
                            == Some(directory)
                }))
                && self.directories.contains_key(directory)
            {
                self.watch.folders.insert(directory.clone());
            }
        }
        for buffer in &self.buffers {
            if reset
                || paths.contains(buffer.path.as_str())
                || paths
                    .iter()
                    .any(|p| buffer.path.starts_with(&format!("{p}/")))
            {
                self.watch.files.insert(buffer.path.clone());
            }
        }
        if reset || !paths.is_empty() {
            self.tree_status.read_at = None;
        }
    }
    pub(super) fn refresh_watched_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Keep below the backend's eight-operation limit, including foreground work.
        for folder in self.watch.folders.clone() {
            if self.loading.len() + self.opening.len() >= 4 {
                break;
            }
            if self.loading.contains(&folder) {
                continue;
            }
            self.watch.folders.remove(&folder);
            self.list(folder, window, cx);
        }
        for path in self.watch.files.clone() {
            if self.loading.len() + self.opening.len() >= 4 {
                break;
            }
            if self.opening.contains(&path)
                || self.buffers.iter().any(|b| b.path == path && b.saving)
            {
                continue;
            }
            self.watch.files.remove(&path);
            self.refresh_external(path, window, cx);
        }
    }
    pub(super) fn refresh_external(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(buffer) = self.buffers.iter().find(|b| b.path == path) else {
            return;
        };
        let identity = buffer.editor.entity_id();
        let revision = buffer.revision.clone();
        if !self.opening.insert(path.clone()) {
            return;
        }
        let mut args = self.args(&path);
        args["root"] = json!(self.root);
        self.rpc(
            "files_read_text",
            args,
            window,
            cx,
            move |this, result, window, cx| {
                this.opening.remove(&path);
                let Some(index) = this
                    .buffers
                    .iter()
                    .position(|b| b.path == path && b.editor.entity_id() == identity)
                else {
                    return;
                };
                let buffer = &mut this.buffers[index];
                if buffer.revision != revision {
                    this.watch.files.insert(path);
                    return;
                }
                // Compare against the *current* saved revision: an atomic local save
                // may complete between the event and this read.
                if result
                    .as_ref()
                    .is_ok_and(|v| v["revision"].as_str() == Some(&buffer.revision))
                {
                    buffer.conflict = None;
                    return;
                }
                if buffer.saving {
                    this.watch.files.insert(path);
                    return;
                }
                if let Err(error) = &result {
                    buffer.error = Some(format!(
                        "Cannot refresh file: {error}. The open buffer is kept."
                    ));
                    return;
                }
                if buffer.dirty(cx) {
                    if let Err(error) = this.retain_conflict(index, result.unwrap()) {
                        this.buffers[index].error = Some(error);
                    }
                    return;
                }
                if buffer.page.is_some() {
                    buffer.error = Some(
                        "This file changed outside Newport. Reload to view its current contents."
                            .into(),
                    );
                    return;
                }
                let selection = buffer.editor.read(cx).selected_range();
                let scroll = buffer.editor.read(cx).scroll_offset();
                let focused = buffer.editor.focus_handle(cx).contains_focused(window, cx);
                match result.and_then(|v| Self::document_buffer(path, v, window, cx)) {
                    Ok(loaded) => {
                        loaded.editor.update(cx, |e, cx| {
                            e.set_selected_range(selection, cx);
                            e.set_scroll_offset(scroll, cx);
                            if focused {
                                e.focus(window, cx);
                            }
                        });
                        this.buffers[index] = loaded;
                    }
                    Err(error) => {
                        this.buffers[index].error = Some(format!(
                            "Cannot refresh file: {error}. The open buffer is kept."
                        ))
                    }
                }
            },
        );
    }
}
