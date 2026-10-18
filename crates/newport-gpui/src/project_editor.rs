//! Projects workspace: retained worktree buffers; Git navigation never owns editor text.
//! THESIS: edit a worktree in place, with files and code sharing one native workspace.
//! OWN-WORLD: Newport's existing GPUI theme, compact controls, quiet borders and mono code.
//! STORY: choose a worktree, expand folders, edit tabs, then explicitly save to its server.
//! FIRST VIEWPORT: one repository toolbar; Files/Changes panel beside shared source/review
//! tabs. File path and save state sit above the code.
//! INTERACTION: lazy folder disclosure and retained tabs; visible dirty state, keyboard save,
//! and explicit discard confirmation. No ornamental animation or replacement design system.
use super::*;
use gpui::base::ElementExt;
use gpui_kit::component::{
    dialog::DialogButtonProps,
    input::{Editor, EditorState, InputEvent, TabSize},
    resizable::resizable_panel,
};
use std::collections::{BTreeMap, HashSet};

#[path = "project_editor_conflict.rs"]
mod conflict;
#[path = "project_editor_document.rs"]
mod document;
#[path = "project_editor_reveal.rs"]
mod reveal;
#[path = "project_editor_tree_keys.rs"]
mod tree_keys;
#[path = "project_editor_tree_status.rs"]
mod tree_status;
#[path = "project_editor_watch.rs"]
mod watch;

pub(super) fn root_path(path: &Value) -> Option<String> {
    use base64::Engine as _;
    String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(path["bytesB64"].as_str()?)
            .ok()?,
    )
    .ok()
}
gpui_kit::actions!(
    project_editor,
    [
        SaveFile,
        ToggleFiles,
        NextTab,
        PreviousTab,
        CloseTab,
        ToggleTerminal,
        QuickOpen,
        FocusExplorer
    ]
);
pub(super) fn install_shortcuts(cx: &mut App) {
    let primary = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new(&format!("{primary}-p"), QuickOpen, Some("ProjectEditor")),
        KeyBinding::new(&format!("{primary}-s"), SaveFile, Some("ProjectEditor")),
        KeyBinding::new(
            &format!("{primary}-shift-e"),
            FocusExplorer,
            Some("ProjectEditor"),
        ),
        KeyBinding::new(&format!("{primary}-w"), CloseTab, Some("ProjectEditor")),
        KeyBinding::new("ctrl-tab", NextTab, Some("ProjectEditor")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("ProjectEditor")),
        KeyBinding::new("ctrl-`", ToggleTerminal, Some("ProjectEditor")),
    ]);
}
pub(super) struct Saved;
impl EventEmitter<Saved> for CodeWorkspace {}

struct Buffer {
    path: String,
    editor: Entity<EditorState>,
    saved: String,
    revision: String,
    crlf: bool,
    bom: bool,
    saving: bool,
    conflict: Option<conflict::Conflict>,
    changed: std::cell::Cell<bool>,
    page: Option<document::Page>,
    error: Option<String>,
    _subscription: Subscription,
}
impl Buffer {
    fn dirty(&self, cx: &App) -> bool {
        // Large documents compare on edit events, not on every repaint. Undo
        // still compares exactly against the saved content. Small buffers also
        // detect programmatic text replacement without materializing a String.
        self.page.is_none()
            && if self.saved.len() > newport_domain::text_file::HIGHLIGHT_LIMIT {
                self.changed.get()
            } else {
                self.editor.read(cx).text() != &self.saved
            }
    }
}
fn decode(content: &str) -> Result<(String, bool, bool), String> {
    newport_domain::text_file::validate(content).map_err(str::to_owned)?;
    let bom = content.starts_with('\u{feff}');
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let crlf = content.contains("\r\n");
    let normalized = content.replace("\r\n", "\n");
    if normalized.contains('\r') || crlf && content.replace("\r\n", "").contains('\n') {
        return Err("This file uses mixed or legacy line endings. Download it from Files to edit without changing its format.".into());
    }
    Ok((normalized, crlf, bom))
}
fn encode(content: &str, crlf: bool, bom: bool) -> String {
    let mut output = String::with_capacity(content.len() + usize::from(bom) * 3);
    if bom {
        output.push('\u{feff}');
    }
    if crlf {
        output.push_str(&content.replace('\n', "\r\n"));
    } else {
        output.push_str(content);
    }
    output
}

#[derive(Clone)]
struct TreeRow {
    path: String,
    name: String,
    directory: bool,
    kind: String,
    depth: usize,
}

#[cfg(test)]
type TestRpc = std::sync::Arc<dyn Fn(&str, Value) -> Result<Value, String> + Send + Sync>;

pub(super) struct CodeWorkspace {
    pub(super) server: String,
    root: String,
    connection: Value,
    pub(super) available: bool,
    visible: bool,
    pub(super) show_tree: bool,
    pub(super) files: bool,
    pub(super) reviewing: bool,
    pub(super) center_width: f32,
    pub(super) narrow: bool,
    pub(super) navigator: project_documents::Navigator,
    pub(super) terminal: Option<Entity<terminal_workspace::Workspace>>,
    pub(super) terminal_visible: bool,
    _terminal_observer: Option<Subscription>,
    directories: BTreeMap<String, Vec<Value>>,
    watch: watch::State,
    expanded: HashSet<String>,
    loading: HashSet<String>,
    opening: HashSet<String>,
    buffers: Vec<Buffer>,
    active: Option<usize>,
    requested: Option<String>,
    error: Option<String>,
    failed_read: Option<(&'static str, String)>,
    scroll: UniformListScrollHandle,
    tab_scroll: ScrollHandle,
    revealed_tab: std::cell::Cell<Option<usize>>,
    reveal_target: Option<String>,
    reveal_refresh: HashSet<String>,
    reveal_notice: Option<String>,
    focus: FocusHandle,
    tree_focus: FocusHandle,
    tree_cursor: Option<String>,
    tree_status: tree_status::State,
    #[cfg(test)]
    test_rpc: Option<TestRpc>,
}
impl CodeWorkspace {
    pub(super) fn new(
        server: String,
        root: String,
        connection: Value,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            server,
            expanded: HashSet::from([root.clone()]),
            root,
            connection,
            available: true,
            visible: true,
            show_tree: true,
            files: true,
            reviewing: false,
            center_width: 0.,
            narrow: false,
            navigator: Default::default(),
            terminal: None,
            terminal_visible: false,
            _terminal_observer: None,
            directories: BTreeMap::new(),
            watch: Default::default(),
            loading: HashSet::new(),
            opening: HashSet::new(),
            buffers: Vec::new(),
            active: None,
            requested: None,
            error: None,
            failed_read: None,
            scroll: UniformListScrollHandle::new(),
            tab_scroll: ScrollHandle::new(),
            revealed_tab: std::cell::Cell::new(None),
            reveal_target: None,
            reveal_refresh: HashSet::new(),
            reveal_notice: None,
            focus: cx.focus_handle(),
            tree_focus: cx.focus_handle(),
            tree_cursor: None,
            tree_status: Default::default(),
            #[cfg(test)]
            test_rpc: None,
        }
    }
    pub(super) fn show_review(&mut self, cx: &mut Context<Self>) {
        self.reviewing = true;
        self.reveal_target = None;
        self.reveal_notice = None;
        self.requested = None;
        if self.narrow {
            self.show_tree = false;
        }
        cx.notify();
    }
    pub(super) fn dirty(&self, cx: &App) -> bool {
        self.buffers.iter().any(|b| b.saving || b.dirty(cx))
    }
    fn args(&self, path: &str) -> Value {
        json!({"id":self.server,"operation":uuid::Uuid::new_v4(),"expectedConnection":self.connection,"path":path})
    }
    pub(super) fn rpc(
        &mut self,
        method: &'static str,
        args: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<Value, String>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        #[cfg(test)]
        let fake = self.test_rpc.clone();
        let task = cx.background_executor().spawn(async move {
            #[cfg(test)]
            if let Some(fake) = fake {
                return fake(method, args);
            }
            backend::Backend::discover().call_with_timeout(method, args, Duration::from_secs(120))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                done(this, result, window, cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn list(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.available || self.loading.len() >= 8 || !self.loading.insert(path.clone()) {
            return;
        }
        self.error = None;
        self.failed_read = None;
        let mut args = self.args(&path);
        args["refresh"] = json!(true);
        args["projectRoot"] = json!(self.root);
        let reveal_read = self
            .reveal_target
            .as_ref()
            .is_some_and(|target| target.starts_with(&format!("{}/", path.trim_end_matches('/'))));
        self.rpc(
            "files_list",
            args,
            window,
            cx,
            move |this, result, window, cx| {
                this.loading.remove(&path);
                let result = result.and_then(|value| {
                    if value["path"].as_str() == Some(path.as_str()) {
                        Ok(value)
                    } else {
                        Err(
                            "This folder is a symbolic link. Open its original project to edit it."
                                .into(),
                        )
                    }
                });
                match result {
                    Ok(value) => {
                        this.directories.insert(
                            path,
                            rows(&value["entries"])
                                .into_iter()
                                .filter(|e| e["name"] != ".git")
                                .collect(),
                        );
                    }
                    Err(error) => {
                        if this.reveal_target.as_ref().is_some_and(|target| {
                            target.starts_with(&format!("{}/", path.trim_end_matches('/')))
                        }) {
                            this.reveal_target = None;
                            this.reveal_notice =
                                Some(format!("Could not locate the file: {error}"));
                        } else if !reveal_read {
                            this.failed_read = Some(("folder", path));
                            this.error = Some(error);
                        }
                    }
                }
                this.continue_reveal(window, cx);
            },
        );
    }
    pub(super) fn open(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.requested = Some(path.clone());
        self.reveal_target = None;
        self.reveal_notice = None;
        if let Some(index) = self.buffers.iter().position(|b| b.path == path) {
            self.active = Some(index);
            self.reviewing = false;
            if self.narrow {
                self.show_tree = false;
            }
            self.buffers[index]
                .editor
                .update(cx, |editor, cx| editor.focus(window, cx));
            self.reveal_active(false, window, cx);
            cx.notify();
            return;
        }
        if !self.available || self.opening.len() >= 4 || !self.opening.insert(path.clone()) {
            return;
        }
        if self.buffers.len() + self.opening.len() > 20 {
            self.opening.remove(&path);
            self.error = Some(
                "Close an editor tab before opening another file (20 tabs per worktree).".into(),
            );
            return;
        }
        self.error = None;
        self.failed_read = None;
        let mut args = self.args(&path);
        args["root"] = json!(self.root);
        args["allowCached"] = json!(true);
        self.rpc(
            "files_read_text",
            args,
            window,
            cx,
            move |this, result, window, cx| {
                this.opening.remove(&path);
                match result {
                    Ok(value) => {
                        let retained: usize = this.buffers.iter().map(|b| b.editor.read(cx).text().len()).sum();
                        if retained.saturating_add(value["content"].as_str().map_or(0, str::len)) > 64 * 1024 * 1024 {
                            this.error = Some("Close a file tab before opening another large file (64 MiB per project).".into());
                            return;
                        }
                        let cached=value["cached"]==true;
                        let buffer = match Self::document_buffer(path.clone(), value, window, cx) {
                            Ok(buffer) => buffer,
                            Err(error) => {
                                this.failed_read = Some(("file", path.clone()));
                                this.error =
                                    Some(format!("{}: {error}", this.relative_path(&path)));
                                return;
                            }
                        };
                        let activate = this.requested.as_ref() == Some(&path);
                        this.buffers.push(buffer);
                        if activate {
                            this.active = Some(this.buffers.len() - 1);
                            this.reviewing = false;
                            if this.narrow {
                                this.show_tree = false;
                            }
                            if this.visible {
                                this.buffers
                                    .last()
                                    .unwrap()
                                    .editor
                                    .update(cx, |editor, cx| editor.focus(window, cx));
                            }
                            this.reveal_active(false, window, cx);
                        }
                        if cached { this.refresh_external(path.clone(),window,cx); }
                    }
                    Err(error) => {
                        this.error = Some(format!("{}: {error}", this.relative_path(&path)));
                        this.failed_read = Some(("file", path));
                    }
                }
            },
        );
        cx.notify();
    }
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active else {
            return;
        };
        let buffer = &self.buffers[index];
        if !self.available
            || buffer.saving
            || buffer.conflict.is_some()
            || self.opening.contains(&buffer.path)
            || !buffer.dirty(cx)
        {
            return;
        }
        let path = buffer.path.clone();
        if buffer.editor.read(cx).text().len() > newport_domain::text_file::EDIT_LIMIT {
            self.buffers[index].error = Some("This file exceeds the 16 MiB editing limit. Your edits are kept; reduce its size before saving.".into());
            cx.notify();
            return;
        }
        let sent = buffer.editor.read(cx).value().to_string();
        let mut args = self.args(&path);
        args["root"] = json!(self.root);
        args["content"] = json!(encode(&sent, buffer.crlf, buffer.bom));
        args["revision"] = json!(buffer.revision);
        self.buffers[index].saving = true;
        self.buffers[index].error = None;
        self.rpc(
            "files_write_text",
            args,
            window,
            cx,
            move |this, result, window, cx| {
                let failed = result
                    .as_ref()
                    .is_err_and(|error| error.contains("remote file changed"));
                let Some(buffer) = this.buffers.iter_mut().find(|b| b.path == path) else {
                    return;
                };
                buffer.saving = false;
                match result {
                    Ok(value) => {
                        buffer.changed.set(buffer.editor.read(cx).text() != &sent);
                        buffer.saved = sent;
                        buffer.revision = text(&value, "revision");
                        cx.emit(Saved);
                    }
                    Err(error) => buffer.error = Some(error),
                }
                if failed {
                    this.refresh_external(path, window, cx);
                }
            },
        );
        cx.notify();
    }
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active else {
            return;
        };
        let buffer = &self.buffers[index];
        if buffer.saving || !self.available {
            return;
        }
        let path = buffer.path.clone();
        if buffer.dirty(cx) {
            let owner = cx.entity().downgrade();
            window.open_alert_dialog(cx, move |dialog, _, _| {
                let owner = owner.clone(); let path = path.clone();
                dialog.title("Reload and discard local edits?").description("This replaces this tab with the latest remote file. Copy any edits you want to keep first.")
                    .button_props(DialogButtonProps::default().show_cancel(true).ok_text("Reload file").cancel_text("Keep editing"))
                    .on_ok(move |_,window,cx| {let _=owner.update(cx,|this,cx|this.read_again(path.clone(),window,cx));true})
            });
        } else {
            self.read_again(path, window, cx);
        }
    }
    fn read_again(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.read_section(path, None, window, cx);
    }

    fn close(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter().find(|b| b.path == path) else {
            return;
        };
        if buffer.saving || self.opening.contains(&buffer.path) {
            return;
        }
        if !buffer.dirty(cx) {
            self.remove(&path, window, cx);
            return;
        }
        let owner = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let path = path.clone();
            dialog
                .title("Discard unsaved edits?")
                .description("Your edits are kept until you save or explicitly discard them.")
                .button_props(
                    DialogButtonProps::default()
                        .show_cancel(true)
                        .ok_text("Discard changes")
                        .cancel_text("Keep editing"),
                )
                .on_ok(move |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        if !this.buffers.iter().any(|b| b.path == path && b.saving) {
                            this.remove(&path, window, cx);
                            this.focus_document(window, cx);
                        }
                    });
                    true
                })
        });
    }
    fn remove(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let previous_index = self.active.unwrap_or(0);
        let active_path = self
            .active
            .and_then(|i| self.buffers.get(i))
            .map(|b| b.path.clone());
        let closing_active = active_path.as_deref() == Some(path);
        self.buffers.retain(|b| b.path != path);
        self.active = active_path
            .and_then(|p| self.buffers.iter().position(|b| b.path == p))
            .or_else(|| {
                (!self.buffers.is_empty())
                    .then_some(previous_index.min(self.buffers.len().saturating_sub(1)))
            });
        if self.buffers.is_empty()
            && let Some(document) = self.navigator.documents.reviews.last().cloned()
        {
            self.review_document(document, cx);
        }
        if closing_active {
            self.reveal_active(false, window, cx);
        }
        cx.notify();
    }
    fn tree_rows(&self) -> Vec<TreeRow> {
        fn add(this: &CodeWorkspace, path: &str, depth: usize, result: &mut Vec<TreeRow>) {
            if depth > 33 || result.len() >= 20_000 {
                return;
            }
            if let Some(children) = this.directories.get(path) {
                for child in children {
                    let path = text(child, "path");
                    let directory = child["kind"] == "directory";
                    result.push(TreeRow {
                        path: path.clone(),
                        name: text(child, "name"),
                        directory,
                        kind: text(child, "kind"),
                        depth,
                    });
                    if directory && this.expanded.contains(&path) {
                        add(this, &path, depth + 1, result);
                    }
                    if result.len() >= 20_000 {
                        break;
                    }
                }
            }
        }
        let mut result = vec![TreeRow {
            path: self.root.clone(),
            name: self
                .root
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or("/")
                .to_owned(),
            directory: true,
            kind: "directory".into(),
            depth: 0,
        }];
        if self.expanded.contains(&self.root) {
            add(self, &self.root, 1, &mut result);
        }
        result
    }
    pub(super) fn refresh_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.list(self.root.clone(), window, cx);
        self.refresh_tree_status(true, window, cx);
    }
    pub(super) fn open_relative(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(
            format!("{}/{}", self.root.trim_end_matches('/'), path),
            window,
            cx,
        );
    }
    pub(super) fn loading_files(&self) -> bool {
        !self.loading.is_empty()
    }
    fn tree(&self, header: bool, cx: &mut Context<Self>) -> AnyElement {
        let tree = self.tree_rows();
        let empty = self.expanded.contains(&self.root)
            && self.directories.get(&self.root).is_some_and(Vec::is_empty)
            && self.loading.is_empty()
            && self.error.is_none();
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .when(header, |view| {
                view.child(
                    crate::project_ui::header()
                        .px_3()
                        .child(crate::project_ui::header_rule(cx))
                        .justify_between()
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_size(px(crate::project_ui::FONT_SIZE))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.root.rsplit('/').next().unwrap_or("Files").to_owned()),
                        )
                        .when(self.narrow || !header, |view| {
                            view.child(
                                crate::controls::compact_icon_button("code-return-editor")
                                    .ghost()
                                    .icon(IconName::ArrowRight)
                                    .accessibility_label("Return to editor")
                                    .tooltip("Return to editor")
                                    .disabled(self.buffers.is_empty())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_tree = false;
                                        cx.notify();
                                    })),
                            )
                        })
                        .child(
                            crate::controls::compact_icon_button("code-refresh-tree")
                                .ghost()
                                .icon(IconName::RefreshCw)
                                .tooltip("Refresh file tree")
                                .accessibility_label("Refresh file tree")
                                .loading(self.loading_files())
                                .disabled(!self.available || !self.loading.is_empty())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.directories.clear();
                                    this.expanded = HashSet::from([this.root.clone()]);
                                    this.list(this.root.clone(), window, cx);
                                })),
                        ),
                )
            })
            .when(self.narrow || !header, |view| {
                view.when_some(self.error.clone().filter(|_| self.narrow && self.show_tree || self.failed_read.as_ref().is_some_and(|(kind, _)| *kind == "folder")), |view, error| {
                    let panel = div()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_size(px(crate::project_ui::FONT_SIZE))
                        .child(
                            div()
                                .id("code-read-error")
                                .role(accesskit::Role::Alert)
                                .aria_label(error.clone())
                                .text_color(cx.theme().danger)
                                .child(error)
                                .test_support(),
                        )
                        .when_some(self.failed_read.clone(), |panel, (kind, path)| {
                            panel.child(
                                crate::project_ui::compact_button("code-retry-read")
                                    .label("Retry")
                                    .disabled(
                                        !self.available
                                            || !self.loading.is_empty()
                                            || !self.opening.is_empty(),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if kind == "folder" {
                                            this.list(path.clone(), window, cx);
                                        } else {
                                            this.open(path.clone(), window, cx);
                                        }
                                    })),
                            )
                        });
                    view.child(panel)
                })
            })
            .when_some(self.reveal_notice.clone(), |view, notice| {
                view.child(
                    div()
                        .p_3()
                        .text_size(px(crate::project_ui::FONT_SIZE))
                        .text_color(cx.theme().muted_foreground)
                        .id("code-reveal-notice")
                        .role(accesskit::Role::Status)
                        .aria_label(notice.clone())
                        .child(notice),
                )
            })
            .child(
                div()
                    .id("project-files-tree")
                    .role(accesskit::Role::Tree)
                    .aria_label("Project files")
                    .track_focus(&self.tree_focus)
                    .tab_stop(true)
                    .flex_1()
                    .min_h_0()
                    .on_key_down(cx.listener(Self::tree_key))
                    .child(
                        uniform_list(
                            "code-file-tree",
                            tree.len() + usize::from(empty),
                            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                                let focused = this.tree_focus.is_focused(window);
                                let active = this
                                    .active
                                    .and_then(|i| this.buffers.get(i))
                                    .map(|b| &b.path);
                                let selected_path = if focused {
                                    this.tree_cursor
                                        .as_ref()
                                        .filter(|path| tree.iter().any(|row| &row.path == *path))
                                        .or(active)
                                        .or_else(|| tree.first().map(|row| &row.path))
                                } else {
                                    active
                                }
                                .cloned();
                                range
                                    .map(|index| {
                                        if index == tree.len() {
                                            return div()
                                                .h(px(24.))
                                                .pl(px(28.))
                                                .text_size(px(crate::project_ui::FONT_SIZE))
                                                .text_color(cx.theme().muted_foreground)
                                                .child("No files in this folder");
                                        }
                                        let row = tree[index].clone();
                                        let selected = selected_path.as_ref() == Some(&row.path);
                                        let tint = cx
                                            .theme()
                                            .background
                                            .blend(cx.theme().foreground.opacity(0.08));
                                        let hover =
                                            if selected { tint } else { theme::row_hover(cx) };
                                        let expanded = this.expanded.contains(&row.path);
                                        let path = row.path.clone();
                                        let status = this.tree_status.status(&this.root, &path);
                                        let label_color = status
                                            .map_or(cx.theme().foreground.opacity(0.85), |state| {
                                                state.color(cx)
                                            });
                                        div().h(px(24.)).child(
                                            div()
                                                .id(SharedString::from(row.path.clone()))
                                                .test_support()
                                                .role(accesskit::Role::TreeItem)
                                                .aria_label(format!(
                                                    "{} {}",
                                                    if row.depth == 0 {
                                                        "Project"
                                                    } else if row.directory {
                                                        "Folder"
                                                    } else {
                                                        "File"
                                                    },
                                                    row.name
                                                ))
                                                .when(row.depth == 0, |item| {
                                                    item.hover_hint(this.root.clone())
                                                })
                                                .aria_selected(selected)
                                                .when_some(status, |item, state| {
                                                    item.aria_description(if row.directory {
                                                        format!(
                                                            "Contains {} files",
                                                            state.description().to_lowercase()
                                                        )
                                                    } else {
                                                        state.description().to_owned()
                                                    })
                                                })
                                                .aria_level(row.depth + 1)
                                                .when(row.directory, |item| {
                                                    item.aria_expanded(expanded)
                                                })
                                                .when(focused && selected, |item| {
                                                    item.aria_active_descendant()
                                                })
                                                .w_full()
                                                .h_full()
                                                .flex()
                                                .items_center()
                                                .relative()
                                                .cursor_default()
                                                .border_1()
                                                .border_color(if focused && selected {
                                                    cx.theme().primary
                                                } else {
                                                    gpui::transparent_black()
                                                })
                                                .when(selected, |item| item.bg(tint))
                                                .hover(move |item| item.bg(hover))
                                                .active(move |item| item.bg(tint))
                                                .pr_2()
                                                .pl(px(6. + row.depth as f32 * 16.))
                                                .children((0..row.depth).map(|depth| {
                                                    div()
                                                        .absolute()
                                                        .left(px(13. + depth as f32 * 16.))
                                                        .top_0()
                                                        .bottom_0()
                                                        .w(px(1.))
                                                        .bg(cx.theme().border.opacity(0.65))
                                                }))
                                                .child(
                                                    div()
                                                        .w_full()
                                                        .min_w_0()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(6.))
                                                        .child(if row.directory {
                                                            Icon::default()
                                                                .path(if expanded {
                                                                    "icons/folder-open.svg"
                                                                } else {
                                                                    "icons/folder.svg"
                                                                })
                                                                .size(px(16.))
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                )
                                                                .flex_shrink_0()
                                                                .into_any_element()
                                                        } else {
                                                            file_icon::tree_icon(
                                                                &row.name,
                                                                &row.kind,
                                                                cx.theme().muted_foreground,
                                                            )
                                                        })
                                                        .child(
                                                            div()
                                                                .min_w_0()
                                                                .truncate()
                                                                .text_size(px(
                                                                    crate::project_ui::FONT_SIZE,
                                                                ))
                                                                .text_color(label_color)
                                                                .child(row.name),
                                                        )
                                                        .when_some(status, |item, state| {
                                                            item.child(div().flex_1().min_w(px(4.)))
                                                                .child(
                                                                    div()
                                                                        .text_size(px(crate::project_ui::FONT_SIZE))
                                                                        .text_color(state.color(cx))
                                                                        .child(state.mark()),
                                                                )
                                                        }),
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.tree_cursor = Some(path.clone());
                                                        if row.directory {
                                                            this.tree_focus.focus(window, cx);
                                                            this.reveal_target = None;
                                                            if !this.expanded.remove(&path) {
                                                                this.expanded.insert(path.clone());
                                                                if !this
                                                                    .directories
                                                                    .contains_key(&path)
                                                                {
                                                                    this.list(
                                                                        path.clone(),
                                                                        window,
                                                                        cx,
                                                                    );
                                                                }
                                                            }
                                                        } else {
                                                            this.open(path.clone(), window, cx);
                                                        }
                                                        cx.notify();
                                                    },
                                                )),
                                        )
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(&self.scroll)
                        .size_full(),
                    ),
            )
            .into_any_element()
    }
}
impl CodeWorkspace {
    fn tab_bar(
        &self,
        reviews: Option<(Vec<AnyElement>, Option<usize>)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let (reviews, active_review) = reviews.unwrap_or_default();
        let selected = if self.reviewing {
            active_review
        } else {
            self.active.map(|i| reviews.len() + i)
        };
        if self.revealed_tab.replace(selected) != selected
            && let Some(index) = selected
        {
            self.tab_scroll.scroll_to_item(index);
        }
        crate::project_ui::header()
            .min_w_0()
            .bg(cx.theme().muted)
            .child(crate::project_ui::header_rule(cx))
            .child(
                crate::controls::compact_icon_button("code-toggle-tree")
                    .ghost()
                    .icon(IconName::PanelLeft)
                    .accessibility_label("Toggle project panel")
                    .tooltip("Toggle project panel")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_tree = !this.show_tree;
                        this.reveal_active(false, window, cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("code-tabs")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll)
                    .children(reviews)
                    .children(self.buffers.iter().enumerate().map(|(i, buffer)| {
                        let path = buffer.path.clone();
                        let close = path.clone();
                        let name = path.rsplit('/').next().unwrap_or(&path);
                        let duplicate = self
                            .buffers
                            .iter()
                            .filter(|b| b.path.rsplit('/').next() == Some(name))
                            .count()
                            > 1;
                        let label = if duplicate {
                            self.relative_path(&path)
                        } else {
                            name.to_owned()
                        };
                        let active = !self.reviewing && self.active == Some(i);
                        project_workspace::document_tab_surface(("code-tab-surface", i), active, cx)
                            .child(
                                crate::project_ui::compact_button(("code-tab", i))
                                    .text()
                                    .cursor_default()
                                    .text_color(if active {
                                        cx.theme().foreground
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .rounded_none()
                                    .label(format!(
                                        "{label}{}",
                                        if buffer.dirty(cx) { " •" } else { "" }
                                    ))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open(path.clone(), window, cx);
                                    })),
                            )
                            .child(
                                crate::controls::compact_icon_button(("code-close", i))
                                    .ghost()
                                    .icon(IconName::X)
                                    .accessibility_label(format!("Close {}", buffer.path))
                                    .tooltip("Close file")
                                    .disabled(buffer.saving)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.close(close.clone(), window, cx)
                                    })),
                            )
                    })),
            )
            .when(!self.reviewing, |view| {
                view.child(
                    div()
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .gap_1()
                        .px_1()
                        .child(
                            crate::controls::compact_icon_button("code-reveal-file")
                                .ghost()
                                .icon(IconName::FolderOpen)
                                .accessibility_label("Reveal in Explorer")
                                .tooltip("Reveal in Explorer")
                                .disabled(!self.available || self.active.is_none())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.reveal_active(true, window, cx)
                                })),
                        )
                        .child(
                            crate::controls::compact_icon_button("code-find")
                                .ghost()
                                .icon(IconName::Search)
                                .accessibility_label(
                                    if self.active.is_some_and(|i| self.buffers[i].page.is_some()) {
                                        "Find in section"
                                    } else {
                                        "Find in file"
                                    },
                                )
                                .tooltip(
                                    if self.active.is_some_and(|i| self.buffers[i].page.is_some()) {
                                        "Find in displayed section"
                                    } else {
                                        "Find in file"
                                    },
                                )
                                .disabled(self.active.is_none())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(i) = this.active {
                                        this.buffers[i]
                                            .editor
                                            .update(cx, |e, cx| e.open_search(false, cx));
                                    }
                                })),
                        )
                        .child(
                            crate::controls::compact_icon_button("code-reload")
                                .ghost()
                                .icon(IconName::RefreshCw)
                                .accessibility_label("Reload")
                                .tooltip("Reload file from server")
                                .disabled(
                                    !self.available
                                        || self.active.is_none()
                                        || !self.opening.is_empty()
                                        || self.active.is_some_and(|i| self.buffers[i].saving),
                                )
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.reload(window, cx)),
                                ),
                        )
                        .child(
                            crate::project_ui::compact_button("code-save")
                                .ghost()
                                .label(if self.active.is_some_and(|i| self.buffers[i].saving) {
                                    "Saving…"
                                } else {
                                    "Save"
                                })
                                .disabled(
                                    !self.available
                                        || !self.active.is_some_and(|i| {
                                            self.buffers[i].dirty(cx)
                                                && !self.buffers[i].saving
                                                && self.buffers[i].conflict.is_none()
                                        }),
                                )
                                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        ),
                )
            })
    }
    fn relative_path(&self, path: &str) -> String {
        path.strip_prefix(&format!("{}/", self.root.trim_end_matches('/')))
            .unwrap_or(path)
            .to_owned()
    }
    fn editor_pane(&self, tabs: bool, cx: &mut Context<Self>) -> Div {
        let active = self.active.and_then(|i| self.buffers.get(i));
        let error = active.and_then(|b| b.error.clone()).or_else(|| {
            self.error.clone().filter(|_| {
                !self
                    .failed_read
                    .as_ref()
                    .is_some_and(|(kind, _)| *kind == "folder")
            })
        });
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .when(tabs, |v| v.child(self.tab_bar(None, cx)))
            .when_some(error, |view, error| {
                view.child(
                    div()
                        .id("code-document-error")
                        .test_support()
                        .role(accesskit::Role::Alert)
                        .aria_label(error.clone())
                        .p_3()
                        .text_size(px(crate::project_ui::FONT_SIZE))
                        .text_color(cx.theme().danger)
                        .child(error)
                        .when_some(
                            self.failed_read.clone().filter(|(kind, _)| *kind == "file"),
                            |panel, (_, path)| {
                                panel.child(
                                    crate::project_ui::compact_button("code-retry-file")
                                        .label("Retry")
                                        .ghost()
                                        .disabled(!self.available || self.opening.contains(&path))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.open(path.clone(), window, cx)
                                        })),
                                )
                            },
                        ),
                )
            })
            .child(
                div()
                    .h(px(crate::project_ui::PATH_ROW_HEIGHT))
                    .min_h(px(crate::project_ui::PATH_ROW_HEIGHT))
                    .px_3()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_size(px(crate::project_ui::FONT_SIZE))
                    .line_height(px(20.))
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .id("code-current-path")
                            .text_color(cx.theme().foreground)
                            .flex_1()
                            .min_w_0()
                            .overflow_x_scroll()
                            .whitespace_nowrap()
                            .child(
                                active
                                    .map(|b| self.relative_path(&b.path))
                                    .unwrap_or_default(),
                            ),
                    )
                    .when_some(active, |view, buffer| {
                        view.when(buffer.dirty(cx), |view| {
                            view.child(div().flex_shrink_0().child("Unsaved"))
                        })
                        .child(div().flex_shrink_0().child(if buffer.crlf {
                            "UTF-8 · CRLF"
                        } else {
                            "UTF-8 · LF"
                        }))
                    }),
            )
            .when_some(active.filter(|b| b.conflict.is_some()), |view, buffer| {
                view.child(self.conflict_strip(buffer, cx))
            })
            .when_some(
                active.and_then(|b| b.page.as_ref().map(|p| (b, p))),
                |view, (buffer, page)| view.child(self.section_controls(buffer, page, cx)),
            )
            .child(if let Some(buffer) = active {
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        Editor::new(
                            buffer
                                .conflict
                                .as_ref()
                                .and_then(|c| c.preview())
                                .unwrap_or(&buffer.editor),
                        )
                        .appearance(false)
                        .bordered(false)
                        .aria_label(if buffer.conflict.as_ref().is_some_and(|c| c.show_remote) {
                            "Remote version (read only)"
                        } else if buffer.page.is_some() {
                            "Read-only file section"
                        } else {
                            "Code editor"
                        })
                        .readonly(
                            buffer.page.is_some()
                                || buffer.conflict.as_ref().is_some_and(|c| c.show_remote),
                        )
                        .h_full()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(crate::project_ui::FONT_SIZE))
                        .line_height(relative(1.6))
                        .bg(cx.theme().background)
                        .px(px(12.))
                        .pt_0()
                        .pb(px(8.)),
                    )
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(empty::compact("Select a file to start editing", cx))
                    .into_any_element()
            })
    }
}
impl CodeWorkspace {
    pub(super) fn can_quick_open(&self) -> bool {
        self.available && self.visible
    }
    pub(super) fn open_quick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_quick_open() || window.has_active_dialog(cx) {
            return;
        }
        let mut recent: Vec<String> = self
            .buffers
            .iter()
            .rev()
            .map(|b| self.relative_path(&b.path))
            .collect();
        if let Some(buffer) = self.active.and_then(|i| self.buffers.get(i)) {
            let path = self.relative_path(&buffer.path);
            recent.retain(|p| p != &path);
            recent.insert(0, path);
        }
        let operation = uuid::Uuid::new_v4();
        let owner = cx.entity().downgrade();
        let picker = cx.new(|cx| {
            project_quick_open::Picker::new(owner.clone(), recent, operation, window, cx)
        });
        let pending = picker.downgrade();
        self.rpc("files_project_index", json!({"id":self.server,"operation":operation,"root":self.root,"expectedConnection":self.connection}), window, cx, move |_, result, window, cx| {
            let _ = pending.update(cx, |p, cx| p.loaded(result, window, cx));
        });
        let focus = picker.clone();
        let previous_focus = window.focused(cx);
        window.open_dialog(cx, move |dialog, window, _| {
            let body = picker.clone();
            let close = picker.clone();
            let previous_focus = previous_focus.clone();
            dialog
                .title("Open file")
                .aria_label("Open project file")
                .w(px(620.).min(window.viewport_size().width - px(32.)))
                .on_close(move |_, window, cx| {
                    close.update(cx, |p, _| p.cancel());
                    if let Some(focus) = &previous_focus {
                        focus.focus(window, cx);
                    }
                })
                .content(move |content, _, _| content.gap_0().child(body.clone()))
        });
        window.defer(cx, move |window, cx| {
            focus.update(cx, |picker, cx| picker.focus(window, cx));
        });
    }
    fn focus_document(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.reviewing
            && let Some(buffer) = self.active.and_then(|i| self.buffers.get(i))
        {
            buffer.editor.update(cx, |e, cx| e.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
    }
    fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        if let Some(terminal) = self
            .terminal
            .clone()
            .filter(|t| self.terminal_visible && t.read(cx).contains_focus(window, cx))
        {
            terminal.update(cx, |t, cx| t.cycle_tab(forward, window, cx));
            return;
        }
        // Match the visible strip: kept/preview reviews, then source buffers.
        let reviews = self.navigator.documents.reviews.len();
        let count = reviews + self.buffers.len();
        if count == 0 {
            return;
        }
        let current = if self.reviewing {
            self.navigator
                .documents
                .reviews
                .iter()
                .position(|d| Some(&d.key) == self.navigator.documents.active.as_ref())
        } else {
            self.active.map(|i| reviews + i)
        };
        let next = match current {
            Some(i) if forward => (i + 1) % count,
            Some(i) => (i + count - 1) % count,
            None if forward => 0,
            None => count - 1,
        };
        if next < reviews {
            self.review_document(self.navigator.documents.reviews[next].clone(), cx);
            self.focus.focus(window, cx);
        } else {
            self.open(self.buffers[next - reviews].path.clone(), window, cx);
        }
    }
    pub(super) fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        if self.terminal_visible
            && self
                .terminal
                .as_ref()
                .is_some_and(|t| t.read(cx).contains_focus(window, cx))
        {
            // Close the dock, retaining its shell rather than destroying a running job.
            self.toggle_terminal(window, cx);
        } else if self.reviewing {
            if let Some(key) = self.navigator.documents.active.clone() {
                self.close_review(&key, cx);
                self.focus_document(window, cx);
            }
        } else if let Some(buffer) = self.active.and_then(|i| self.buffers.get(i)) {
            self.close(buffer.path.clone(), window, cx);
            if !window.has_active_dialog(cx) {
                self.focus_document(window, cx);
            }
        }
    }
    pub(super) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_visible {
            self.terminal_visible = false;
            if let Some(terminal) = &self.terminal {
                terminal.update(cx, |t, cx| t.deactivate(window, cx));
            }
            self.focus_document(window, cx);
        } else if self.available {
            if self.terminal.is_none() {
                let server = self.server.clone();
                let connection = self.connection.clone();
                let root = self.root.clone();
                if newport_client::terminal_command::CommandDraft::project_directory(&root).is_err()
                {
                    return;
                }
                let terminal = cx.new(|cx| {
                    terminal_workspace::Workspace::for_project(server, connection, root, cx)
                        .expect("validated root")
                });
                self._terminal_observer = Some(cx.observe(&terminal, |_, _, cx| cx.notify()));
                self.terminal = Some(terminal);
            }
            self.terminal_visible = true;
            let owner = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if this.available
                        && this.terminal_visible
                        && let Some(terminal) = &this.terminal
                    {
                        terminal.update(cx, |t, cx| t.focus_active(window, cx));
                    }
                });
            });
            if self.narrow {
                self.show_tree = false;
            }
        }
        cx.notify();
    }

    pub(super) fn workspace(
        &self,
        header: AnyElement,
        reviews: (Vec<AnyElement>, Option<usize>),
        changes: AnyElement,
        review: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let left = div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(if self.files {
                        self.tree(false, cx)
                    } else {
                        changes
                    }),
            );
        let measured = cx.entity().downgrade();
        let document = div()
            .on_prepaint(move |bounds, window, cx| {
                let width = f32::from(bounds.size.width);
                if measured
                    .upgrade()
                    .is_some_and(|e| (e.read(cx).center_width - width).abs() > 1.)
                {
                    let measured = measured.clone();
                    window.defer(cx, move |_, cx| {
                        let _ = measured.update(cx, |e, cx| {
                            e.center_width = width;
                            cx.notify();
                        });
                    });
                }
            })
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .child(self.tab_bar(Some(reviews), cx))
            .child(div().flex_1().min_h_0().min_w_0().child(if self.reviewing {
                review
            } else {
                self.editor_pane(false, cx).into_any_element()
            }));
        let center = if self.terminal_visible {
            use gpui_kit::component::resizable::v_resizable;
            v_resizable("project-terminal-split")
                .child(
                    resizable_panel()
                        .size_range(px(120.)..Pixels::MAX)
                        .child(document),
                )
                .child(
                    resizable_panel()
                        .size(px(220.))
                        .size_range(px(100.)..Pixels::MAX)
                        .flex_none()
                        .child(
                            div()
                                .size_full()
                                .border_t_1()
                                .border_color(cx.theme().border)
                                .children(self.terminal.clone().filter(|_| self.available))
                                .when(!self.available, |dock| {
                                    dock.child(div().p_3().text_size(px(crate::project_ui::FONT_SIZE)).child(
                                        "Terminal paused: this project connection is unavailable.",
                                    ))
                                }),
                        ),
                )
                .into_any_element()
        } else {
            document.into_any_element()
        };
        let content = if self.narrow && self.show_tree {
            left.into_any_element()
        } else if self.show_tree {
            git_split::group("project-unified-split", "Resize project panel", 300.)
                .child(
                    resizable_panel()
                        .size(px(260.))
                        .size_range(px(220.)..px(420.))
                        .flex_none()
                        .child(left),
                )
                .child(resizable_panel().child(center))
                .into_any_element()
        } else {
            center.into_any_element()
        };
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .when(!self.available, |v| {
                v.child(div().p_2().child(
                    "The server connection changed. Open buffers are kept; saving is disabled.",
                ))
            })
            .track_focus(&self.focus)
            .key_context("ProjectEditor")
            .on_action(cx.listener(|this, _: &QuickOpen, window, cx| this.open_quick(window, cx)))
            .on_action(
                cx.listener(|this, _: &FocusExplorer, window, cx| this.navigate(0, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.cycle_tab(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CloseTab, window, cx| this.close_active_tab(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                if !window.has_active_dialog(cx) {
                    this.toggle_terminal(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SaveFile, window, cx| {
                if !this.reviewing
                    && !this
                        .terminal
                        .as_ref()
                        .is_some_and(|t| t.read(cx).contains_focus(window, cx))
                {
                    this.save(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleFiles, window, cx| {
                this.show_tree = !this.show_tree;
                this.reveal_active(false, window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FindInView, window, cx| {
                if let Some(terminal) = this
                    .terminal
                    .clone()
                    .filter(|t| t.read(cx).contains_focus(window, cx))
                {
                    terminal.update(cx, |t, cx| t.open_find(window, cx));
                    return;
                }
                if !this.reviewing
                    && let Some(i) = this.active
                {
                    this.buffers[i]
                        .editor
                        .update(cx, |e, cx| e.open_search(false, cx));
                }
            }))
            .child(content)
            .into_any_element()
    }
}
impl Render for CodeWorkspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.editor_pane(true, cx);
        let content = if self.narrow && self.show_tree {
            self.tree(true, cx)
        } else if self.show_tree {
            git_split::group("project-code-split", "Resize the file tree", 300.)
                .child(
                    resizable_panel()
                        .size(px(240.))
                        .size_range(px(170.)..px(480.))
                        .flex_none()
                        .child(self.tree(true, cx)),
                )
                .child(resizable_panel().child(pane))
                .into_any_element()
        } else {
            pane.into_any_element()
        };
        div().size_full().min_h_0().min_w_0().flex().flex_col().track_focus(&self.focus).key_context("ProjectEditor")
            .on_action(cx.listener(|this, _: &QuickOpen, window, cx| this.open_quick(window, cx)))
            .on_action(cx.listener(|this, _: &FocusExplorer, window, cx| this.navigate(0, window, cx)))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(true, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.cycle_tab(false, window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| this.close_active_tab(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                if !window.has_active_dialog(cx) { this.toggle_terminal(window, cx); }
            }))
            .on_action(cx.listener(|this, _: &SaveFile, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleFiles, window, cx| { this.show_tree = !this.show_tree; this.reveal_active(false, window, cx); cx.notify(); }))
            .on_action(cx.listener(|this, _: &FindInView, _, cx| {
                if let Some(i) = this.active { this.buffers[i].editor.update(cx, |e, cx| e.open_search(false, cx)); }
            }))
            .when(!self.available, |view| view.child(div().p_2().child("The server connection changed. Your open buffers are kept; saving is disabled.")))
            .child(div().flex_1().min_h_0().min_w_0().child(content))
    }
}

pub(super) struct Retained {
    server: String,
    root: String,
    pub(super) editor: Entity<CodeWorkspace>,
    _subscription: Subscription,
    _observe: Subscription,
    _documents: Subscription,
}
impl Newport {
    pub(super) fn project_terminal_sessions(&self, cx: &App) -> Vec<uuid::Uuid> {
        self.code_workspaces
            .iter()
            .flat_map(|retained| {
                retained
                    .editor
                    .read(cx)
                    .terminal
                    .as_ref()
                    .map(|t| t.read(cx).session_ids(cx))
                    .unwrap_or_default()
            })
            .collect()
    }
    pub(super) fn deactivate_project_terminals(&self, window: &mut Window, cx: &mut Context<Self>) {
        for retained in &self.code_workspaces {
            let editor = retained.editor.read(cx);
            if (self.page != Page::Projects
                || self.server != retained.server
                || root_path(&self.git.repo["root"]).as_ref() != Some(&retained.root)
                || !editor.terminal_visible
                || !editor.available
                || (editor.narrow && editor.show_tree)
                || self.server_form.is_some())
                && let Some(terminal) = editor.terminal.clone()
            {
                terminal.update(cx, |t, cx| t.deactivate(window, cx));
            }
        }
    }

    pub(super) fn sync_code_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = root_path(&self.git.repo["root"]);
        for retained in &self.code_workspaces {
            let visible = self.page == Page::Projects
                && self.git.repo["bare"] != true
                && retained.server == self.server
                && current.as_ref() == Some(&retained.root);
            let narrow = self.content_width < 600.;
            if retained.editor.read(cx).visible != visible
                || retained.editor.read(cx).narrow != narrow
            {
                retained.editor.update(cx, |e, cx| {
                    let entering = visible && !e.visible;
                    e.visible = visible;
                    if !visible {
                        e.suspend_file_watch(window, cx);
                    }
                    e.narrow = narrow;
                    if entering && !window.has_active_dialog(cx) {
                        e.focus_document(window, cx);
                    }
                    cx.notify();
                });
            }
        }
        if self.page != Page::Projects
            || self.git.repo["bare"] == true
            || !self.git.repo["repoId"].is_string()
        {
            return;
        }
        let Some(root) = root_path(&self.git.repo["root"]) else {
            return;
        };
        let Some(server) = self.selected_server() else {
            return;
        };
        let connection = terminal_scope::connection(&server, &self.snapshot);
        if let Some(retained) = self
            .code_workspaces
            .iter()
            .find(|e| e.server == self.server && e.root == root)
        {
            let available =
                retained.editor.read(cx).connection == connection && self.configuration_available();
            if retained.editor.read(cx).available != available {
                retained.editor.update(cx, |e, cx| {
                    e.available = available;
                    if !available {
                        e.suspend_file_watch(window, cx);
                    }
                    if !available && let Some(terminal) = &e.terminal {
                        terminal.update(cx, |terminal, cx| {
                            terminal.sync_connection(Value::Null, e.root.clone(), cx)
                        });
                    }
                    cx.notify();
                });
            }
            let repo = self.git_repository_id();
            let writable = !self.git_mutation_blocks_write()
                && !self.project_dialog_open
                && !self.modal_open
                && self.git.repo["capabilities"]["readOnly"] == false;
            retained.editor.update(cx, |e, cx| {
                if e.navigator.repo != repo {
                    e.navigator.repo = repo.clone();
                    e.navigator.collections = Default::default();
                    e.navigator.invalidate();
                }
                e.navigator.writable = writable && available;
                e.poll_files(window, cx);
                e.refresh_tree_status(false, window, cx);
                if matches!(e.navigator.panel, 1 | 2) {
                    let index = e.navigator.panel - 1;
                    let collection = &e.navigator.collections[index];
                    if !collection.loading
                        && (e.navigator.invalidated || collection.read_at.is_none())
                        && e.navigator
                            .filter_changed
                            .is_none_or(|at| at.elapsed() >= Duration::from_millis(200))
                    {
                        e.navigator.invalidated = false;
                        e.load_collection(index, false, window, cx);
                    }
                }
            });
            self.project_restore_document(cx);
            return;
        }
        let editor =
            cx.new(|cx| CodeWorkspace::new(self.server.clone(), root.clone(), connection, cx));
        editor.update(cx, |editor, cx| {
            editor.narrow = self.content_width < 600.;
            editor.navigator.repo = self.git_repository_id();
            editor.navigator.writable = self.git.repo["capabilities"]["readOnly"] == false;
            editor.list(root.clone(), window, cx);
            editor.refresh_tree_status(false, window, cx);
        });
        let initial = editor.downgrade();
        // Focus after the new workspace is mounted; the library's old focus
        // path disappears during this render.
        window.defer(cx, move |window, cx| {
            let _ = initial.update(cx, |editor, cx| {
                if editor.visible && !window.has_active_dialog(cx) {
                    editor.focus_tree(window, cx);
                }
            });
        });
        let saved_server = self.server.clone();
        let saved_root = root.clone();
        let subscription = cx.subscribe(&editor, move |this, _, _: &Saved, cx| {
            if this.page == Page::Projects
                && this.server == saved_server
                && root_path(&this.git.repo["root"]).as_ref() == Some(&saved_root)
            {
                if let Some(editor) = this.code_workspace() {
                    editor.update(cx, |e, _| {
                        e.navigator.invalidate();
                        e.tree_status.read_at = None;
                    });
                }
                this.git_refresh();
                cx.notify();
            }
        });
        let document_server = self.server.clone();
        let document_root = root.clone();
        let documents = cx.subscribe(
            &editor,
            move |this, editor, event: &project_documents::Event, cx| {
                if this.server != document_server
                    || root_path(&this.git.repo["root"]).as_ref() != Some(&document_root)
                {
                    return;
                }
                match event {
                    project_documents::Event::Open(document) => {
                        this.project_open_document(document.clone(), cx)
                    }
                    project_documents::Event::Stage {
                        snapshot,
                        entries,
                        stage,
                    } => {
                        let allowed = editor.read(cx).navigator.can_stage(snapshot, entries);
                        if !allowed {
                            return;
                        }
                        let snapshot = snapshot.clone();
                        let ids: Vec<_> =
                            entries.iter().map(|entry| text(entry, "entryId")).collect();
                        let stage = *stage;
                        this.start_git_index_mutation(
                            this.git_repository_id(),
                            format!(
                                "{} {} files",
                                if stage { "Stage" } else { "Unstage" },
                                ids.len()
                            ),
                            move |backend, server, repo, op| {
                                backend
                                    .change_index_entries(server, repo, &snapshot, &ids, stage, op)
                            },
                            cx,
                        );
                    }
                }
            },
        );
        let observe = cx.observe(&editor, |_, _, cx| cx.notify());
        self.code_workspaces.push(Retained {
            server: self.server.clone(),
            root,
            editor,
            _subscription: subscription,
            _observe: observe,
            _documents: documents,
        });
    }
    pub(super) fn code_workspace(&self) -> Option<Entity<CodeWorkspace>> {
        let root = root_path(&self.git.repo["root"])?;
        self.code_workspaces
            .iter()
            .find(|e| e.server == self.server && e.root == root)
            .map(|e| e.editor.clone())
    }
    pub(super) fn editor_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        if self
            .code_workspaces
            .iter()
            .any(|e| e.editor.read(cx).buffers.iter().any(|b| b.saving))
        {
            self.notification = Some(Notification::info(
                "Wait for file saves to finish before quitting.",
            ));
            cx.notify();
            return;
        }
        if !self
            .code_workspaces
            .iter()
            .any(|e| e.editor.read(cx).dirty(cx))
        {
            cx.quit();
            return;
        }
        window.open_alert_dialog(cx,|dialog,_,_|dialog.title("Quit with unsaved files?").description("Return to Code to save your edits. Quitting now discards unsaved editor changes.").button_props(DialogButtonProps::default().show_cancel(true).ok_text("Quit without saving").cancel_text("Keep editing")).on_ok(|_,_,cx|{cx.quit();true}));
    }
}
#[cfg(test)]
mod tests {
    use super::{decode, encode};
    #[test]
    fn preserve_bom_and_line_endings_and_reject_lossy_conversion() {
        for source in ["", "a\nb\n", "\u{feff}a\r\nb\r\n", "a\r\nb"] {
            let (text, crlf, bom) = decode(source).unwrap();
            assert_eq!(encode(&text, crlf, bom), source);
        }
        assert!(decode("a\r\nb\nc").is_err());
        assert!(decode("a\rb").is_err());
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::CodeWorkspace;
    use crate::project_documents;
    use gpui::{Entity, TestAppContext, VisualTestContext, px, size};
    use gpui_kit as gpui;
    use gpui_kit::AppContext;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    type Calls = Arc<Mutex<Vec<(String, Value)>>>;
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    fn fixture(cx: &mut TestAppContext) -> (Entity<CodeWorkspace>, &mut VisualTestContext, Calls) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
            super::install_shortcuts(cx);
            crate::navigation::install_workspace_shortcuts(cx);
            // The application installs this fallback after project bindings.
            cx.bind_keys([gpui::KeyBinding::new("cmd-w", super::CloseTab, None)]);
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let retained = std::rc::Rc::new(std::cell::RefCell::new(None));
        let saved = retained.clone();
        let (_,cx)=cx.add_window_view(move |window,cx| {
            let view=cx.new(|cx| {
            let mut view=CodeWorkspace::new("server-a".into(),"/repo".into(),json!(["scope-a"]),cx);
            view.test_rpc=Some(Arc::new(move |method,args| {
                seen.lock().unwrap().push((method.into(),args.clone()));
                match method {
                    "files_read_text" => Ok(json!({"path":args["path"],"content":"fn main() {}\n","revision":"original"})),
                    "files_write_text" => Ok(json!({"path":args["path"],"content":args["content"],"revision":"saved"})),
                    "files_list" => Ok(json!({"path":args["path"],"entries":[{"name":"main.rs","path":"/repo/main.rs","kind":"file"}]})),
                    "files_project_index" => Ok(json!({"paths":["main.rs", "src/deep/hidden.rs", "tests/hidden.rs"], "skipped":0})),
                    _ => panic!("unexpected method {method}"),
                }
            }));view
            });
            *saved.borrow_mut()=Some(view.clone());
            gpui::base::Root::new(view,window,cx)
        });
        let view = retained.borrow_mut().take().unwrap();
        cx.simulate_resize(size(px(1000.), px(650.)));
        draw(cx);
        (view, cx, calls)
    }
    fn open(view: &Entity<CodeWorkspace>, cx: &mut VisualTestContext, path: &str) {
        view.update_in(cx, |view, window, cx| view.open(path.into(), window, cx));
        draw(cx);
    }
    fn reveal_fixture(
        cx: &mut TestAppContext,
    ) -> (Entity<CodeWorkspace>, &mut VisualTestContext, Calls) {
        let (view, cx, calls) = fixture(cx);
        let recorded = calls.clone();
        view.update(cx, |v, _| {
            v.test_rpc = Some(Arc::new(move |method, args| {
                recorded.lock().unwrap().push((method.into(), args.clone()));
                if method == "files_read_text" {
                    return Ok(json!({"path":args["path"], "content":"original source", "revision":"initial"}));
                }
                assert_eq!(method, "files_list");
                let children = match args["path"].as_str().unwrap() {
                    "/repo" => vec![("main.rs", "file"), ("src", "directory"), ("unrelated", "directory")],
                    "/repo/src" => vec![("deep", "directory")],
                    "/repo/src/deep" => vec![("hidden.rs", "file")],
                    other => panic!("must not crawl unrelated folders: {other}"),
                };
                let entries: Vec<_> = children.into_iter().map(|(name, kind)| json!({"name":name,"path":format!("{}/{name}",args["path"].as_str().unwrap()),"kind":kind})).collect();
                Ok(json!({"path":args["path"], "entries":entries}))
            }));
        });
        (view, cx, calls)
    }
    #[gpui::test]
    fn edits_and_saves_twelve_megabytes_with_undo(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        let source = Arc::new(format!("// {}\n", "x".repeat(200)).repeat(62_000));
        assert!(source.len() > 12 * 1024 * 1024);
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sent = writes.clone();
        let read = source.clone();
        view.update(cx, |v, _| {
            v.test_rpc = Some(Arc::new(move |method, args| match method {
                "files_read_text" => Ok(json!({"content":*read,"revision":"large-original"})),
                "files_list" => Ok(json!({"path":args["path"],"entries":[]})),
                "files_write_text" => {
                    assert_eq!(args["revision"], "large-original");
                    assert_eq!(
                        args["content"].as_str().unwrap(),
                        format!("// edited\n{}", read)
                    );
                    sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(json!({"revision":"large-saved"}))
                }
                _ => panic!("unexpected method {method}"),
            }))
        });
        let start = std::time::Instant::now();
        open(&view, cx, "/repo/large.rs");
        eprintln!("12 MiB editor open and first draw: {:?}", start.elapsed());
        view.read_with(cx, |v, cx| {
            assert!(v.buffers[0].page.is_none());
            assert!(!v.buffers[0].dirty(cx));
            assert_eq!(v.buffers[0].editor.read(cx).text().len(), source.len());
        });
        let start = std::time::Instant::now();
        cx.simulate_input("// edited\n");
        draw(cx);
        eprintln!("12 MiB insertion and draw: {:?}", start.elapsed());
        assert!(view.read_with(cx, |v, cx| v.buffers[0].dirty(cx)));
        crate::test_shortcuts(cx, "cmd-s");
        draw(cx);
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!view.read_with(cx, |v, cx| v.buffers[0].dirty(cx)));
        crate::test_shortcuts(cx, "cmd-z");
        draw(cx);
        assert!(view.read_with(cx, |v, cx| v.buffers[0].dirty(cx)));
        crate::test_shortcuts(cx, "cmd-shift-z");
        draw(cx);
        assert!(!view.read_with(cx, |v, cx| v.buffers[0].dirty(cx)));
    }
    #[gpui::test]
    fn large_sections_are_readonly_navigable_and_cannot_be_saved(cx: &mut TestAppContext) {
        let (view, cx, calls) = fixture(cx);
        let seen = calls.clone();
        view.update(cx, |v, _| v.test_rpc = Some(Arc::new(move |method, args| {
            seen.lock().unwrap().push((method.into(), args.clone()));
            match method {
                "files_read_text" => {
                    let offset = args["offset"].as_u64().unwrap_or(0);
                    if offset > 0 { assert_eq!(args["revision"], "page:original"); }
                    let (content, next) = if offset == 0 { ("first section", Some(13)) } else { ("last", None) };
                    Ok(json!({"content":content,"revision":"page:original","page":{"offset":offset,"next":next,"size":17}}))
                }
                "files_list" => Ok(json!({"entries":[]})),
                other => panic!("unexpected call {other}"),
            }
        })));
        open(&view, cx, "/repo/large.log");
        let first = view.read_with(cx, |v, _| v.buffers[0].editor.entity_id());
        cx.simulate_input("must not edit");
        crate::test_shortcuts(cx, "cmd-s");
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(
                v.buffers[0].editor.read(cx).value().as_str(),
                "first section"
            );
            assert!(!v.buffers[0].dirty(cx));
        });
        let next = cx.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .iter()
                .find(|e| e.label() == Some("Next"))
                .unwrap()
                .bounds()
                .center()
        });
        cx.simulate_click(next, Default::default());
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(v.buffers[0].editor.read(cx).value().as_str(), "last");
            assert_ne!(
                v.buffers[0].editor.entity_id(),
                first,
                "old undo storage is released"
            );
            assert!(v.buffers[0].page.as_ref().unwrap().next.is_none());
        });
        let previous = cx.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .iter()
                .find(|e| e.label() == Some("Previous"))
                .unwrap()
                .bounds()
                .center()
        });
        cx.simulate_click(previous, Default::default());
        draw(cx);
        assert_eq!(
            view.read_with(cx, |v, cx| v.buffers[0].editor.read(cx).value().to_string()),
            "first section"
        );
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|(m, _)| m == "files_write_text")
        );
    }
    #[gpui::test]
    fn binary_response_never_creates_a_text_buffer(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        view.update(cx, |v, _| {
            v.test_rpc = Some(Arc::new(|_, _| {
                Ok(json!({"content":"%PDF-1.7\nobject", "revision":"r"}))
            }))
        });
        open(&view, cx, "/repo/report.pdf");
        view.read_with(cx, |v, _| {
            assert!(v.buffers.is_empty());
            assert!(v.error.as_ref().unwrap().contains("binary"));
        });
    }
    #[gpui::test]
    fn project_root_disclosure_preserves_nested_expansion_and_open_buffer(cx: &mut TestAppContext) {
        let (view, cx, calls) = reveal_fixture(cx);
        // The project is present before any directory read finishes.
        view.read_with(cx, |v, _| {
            let rows = v.tree_rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].path, "/repo");
            assert_eq!(rows[0].name, "repo");
            assert_eq!(rows[0].depth, 0);
        });
        open(&view, cx, "/repo/src/deep/hidden.rs");
        view.update_in(cx, |v, window, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("keep unsaved edits", window, cx));
        });
        draw(cx);
        view.read_with(cx, |v, _| {
            let rows = v.tree_rows();
            assert_eq!(rows[0].path, "/repo");
            assert_eq!(rows.iter().filter(|r| r.depth == 0).count(), 1);
            assert_eq!(
                rows.iter().find(|r| r.path == "/repo/src").unwrap().depth,
                1
            );
            assert_eq!(
                rows.iter()
                    .find(|r| r.path == "/repo/src/deep/hidden.rs")
                    .unwrap()
                    .depth,
                3
            );
        });
        let read_count = calls.lock().unwrap().len();
        let root_bounds = cx.update(|window, _| {
            let nodes = gpui::base::test_support::snapshots(window);
            let root = nodes
                .iter()
                .find(|e| e.label() == Some("Project repo"))
                .unwrap();
            assert_eq!(root.expanded(), Some(true));
            root.bounds()
        });
        cx.simulate_click(root_bounds.center(), Default::default());
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(v.tree_rows().len(), 1);
            assert!(v.expanded.contains("/repo/src/deep"));
            assert_eq!(
                v.buffers[0].editor.read(cx).value().as_str(),
                "keep unsaved edits"
            );
            assert!(v.buffers[0].dirty(cx));
        });
        crate::test_shortcuts(cx, "right right");
        draw(cx);
        assert_eq!(
            view.read_with(cx, |v, _| v.tree_cursor.clone()).as_deref(),
            Some("/repo/main.rs")
        );
        crate::test_shortcuts(cx, "left left");
        draw(cx);
        assert_eq!(view.read_with(cx, |v, _| v.tree_rows().len()), 1);
        assert_eq!(
            calls.lock().unwrap().len(),
            read_count,
            "collapse and expand reuse loaded directories"
        );
        // Reveal must reopen the root as well as the active file's ancestors.
        view.update_in(cx, |v, window, cx| v.reveal_active(false, window, cx));
        draw(cx);
        assert!(view.read_with(cx, |v, _| {
            v.tree_rows()
                .iter()
                .any(|r| r.path == "/repo/src/deep/hidden.rs")
        }));
    }

    #[gpui::test]
    fn tree_keyboard_browses_lazily_and_opens_only_on_enter(cx: &mut TestAppContext) {
        let (view, cx, calls) = reveal_fixture(cx);
        open(&view, cx, "/repo/main.rs");
        view.update_in(cx, |v, window, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("keep edits", window, cx));
        });
        crate::test_shortcuts(cx, "cmd-shift-e");
        draw(cx);
        cx.update(|window, cx| assert!(view.read(cx).tree_focus.is_focused(window)));
        let cursor =
            |cx: &mut VisualTestContext| view.read_with(cx, |v, _| v.tree_cursor.clone().unwrap());
        assert_eq!(cursor(cx), "/repo/main.rs");
        crate::test_shortcuts(cx, "down");
        draw(cx);
        assert_eq!(cursor(cx), "/repo/src");
        crate::test_shortcuts(cx, "right");
        draw(cx);
        assert_eq!(cursor(cx), "/repo/src");
        crate::test_shortcuts(cx, "right");
        draw(cx);
        assert_eq!(cursor(cx), "/repo/src/deep");
        crate::test_shortcuts(cx, "right");
        draw(cx);
        crate::test_shortcuts(cx, "right");
        draw(cx);
        assert_eq!(cursor(cx), "/repo/src/deep/hidden.rs");
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, _)| m == "files_read_text")
                .count(),
            1,
            "browsing must not replace the document or read files"
        );
        crate::test_shortcuts(cx, "left");
        draw(cx);
        assert_eq!(cursor(cx), "/repo/src/deep");
        crate::test_shortcuts(cx, "left");
        draw(cx);
        assert!(!view.read_with(cx, |v, _| v.expanded.contains("/repo/src/deep")));
        crate::test_shortcuts(cx, "right right enter");
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(
                v.buffers[v.active.unwrap()].path,
                "/repo/src/deep/hidden.rs"
            );
            assert_eq!(v.buffers[0].editor.read(cx).value().as_str(), "keep edits");
            assert!(v.buffers[0].dirty(cx));
        });
        crate::test_shortcuts(cx, "cmd-shift-e home up");
        draw(cx);
        assert_eq!(cursor(cx), "/repo");
        crate::test_shortcuts(cx, "end down");
        draw(cx);
        assert_eq!(cursor(cx), "/repo/unrelated");
        crate::test_shortcuts(cx, "escape");
        draw(cx);
        cx.update(|window, cx| assert!(!view.read(cx).tree_focus.is_focused(window)));
        cx.simulate_input("editing again");
        draw(cx);
        assert!(view.read_with(cx, |v, cx| {
            v.buffers[v.active.unwrap()]
                .editor
                .read(cx)
                .value()
                .contains("editing again")
        }));
    }

    #[gpui::test]
    fn tree_keyboard_handles_empty_refresh_and_mouse_folder_focus(cx: &mut TestAppContext) {
        let (view, cx, calls) = reveal_fixture(cx);
        // Match project entry: its mounted workspace is the initial focus owner.
        view.update_in(cx, |v, w, cx| v.focus_document(w, cx));
        crate::test_shortcuts(cx, "cmd-shift-e");
        draw(cx);
        cx.update(|window, cx| assert!(view.read(cx).tree_focus.is_focused(window)));
        crate::test_shortcuts(cx, "up down left right home end enter");
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.buffers.is_empty()));
        // Global explorer shortcuts must not steal focus from a modal picker.
        cx.update(|w, cx| {
            use gpui::component::WindowExt as _;
            w.open_alert_dialog(cx, |d, _, _| d.title("Test modal"));
        });
        draw(cx);
        crate::test_shortcuts(cx, "cmd-shift-e");
        draw(cx);
        cx.update(|w, cx| assert!(!view.read(cx).tree_focus.is_focused(w)));
        crate::test_shortcuts(cx, "escape");
        draw(cx);
        view.update_in(cx, |v, w, cx| {
            v.expanded.insert(v.root.clone());
            v.list("/repo".into(), w, cx);
        });
        draw(cx);
        let bounds = cx.update(|w, _| {
            gpui::base::test_support::snapshots(w)
                .iter()
                .find(|e| e.label() == Some("Folder src"))
                .unwrap()
                .bounds()
        });
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|w, cx| assert!(view.read(cx).tree_focus.is_focused(w)));
        crate::test_shortcuts(cx, "right");
        draw(cx);
        assert_eq!(
            view.read_with(cx, |v, _| v.tree_cursor.clone()).as_deref(),
            Some("/repo/src/deep")
        );
        // A refresh may remove the cursor's row; navigation recovers to a visible item.
        view.update(cx, |v, cx| {
            v.directories.clear();
            v.expanded.clear();
            cx.notify();
        });
        draw(cx);
        crate::test_shortcuts(cx, "down enter");
        draw(cx);
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|(m, _)| m == "files_read_text")
        );
    }

    #[gpui::test]
    fn explorer_reveals_only_active_file_ancestors_and_preserves_edits(cx: &mut TestAppContext) {
        let (view, cx, calls) = reveal_fixture(cx);
        open(&view, cx, "/repo/main.rs");
        view.update_in(cx, |v, window, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("keep edits", window, cx))
        });
        open(&view, cx, "/repo/src/deep/hidden.rs");
        view.read_with(cx, |v, cx| {
            assert!(v.reveal_target.is_none());
            assert!(v.reveal_notice.is_none());
            assert!(v.expanded.contains("/repo/src"));
            assert!(v.expanded.contains("/repo/src/deep"));
            assert!(!v.expanded.contains("/repo/unrelated"));
            assert!(
                v.tree_rows()
                    .iter()
                    .any(|r| r.path == "/repo/src/deep/hidden.rs")
            );
            assert!(v.buffers[0].dirty(cx));
            assert_eq!(
                v.buffers[0].editor.read(cx).value().to_string(),
                "keep edits"
            );
        });
        let requests = calls.lock().unwrap();
        let folders: Vec<_> = requests
            .iter()
            .filter(|(m, _)| m == "files_list")
            .map(|(_, a)| a["path"].as_str().unwrap())
            .collect();
        assert_eq!(folders, ["/repo", "/repo/src", "/repo/src/deep"]);
        cx.update(|window, _| {
            assert!(
                gpui::base::test_support::snapshots(window)
                    .iter()
                    .any(|e| e.label() == Some("File hidden.rs"))
            )
        });
    }
    #[gpui::test]
    fn reveal_scrolls_the_active_file_into_the_virtualized_explorer(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        view.update(cx, |v,_| v.test_rpc = Some(Arc::new(|method,args| {
            match method {
                "files_read_text" => Ok(json!({"path":args["path"],"content":"visible document", "revision":"original"})),
                "files_list" => {
                    let entries: Vec<_> = (0..300).map(|n|json!({"path":format!("/repo/f{n:03}.rs"),"name":format!("f{n:03}.rs"),"kind":"file"})).collect();
                    Ok(json!({"path":args["path"],"entries":entries}))
                }
                _ => panic!("unexpected {method}"),
            }
        })));
        open(&view, cx, "/repo/f299.rs");
        draw(cx);
        cx.update(|window, _| {
            assert!(
                gpui::base::test_support::snapshots(window)
                    .iter()
                    .any(|e| e.label() == Some("File f299.rs")),
                "reveal must scroll to the file, not just expand its parents"
            )
        });
    }
    #[gpui::test]
    fn explicit_reveal_opens_explorer_without_losing_the_source_or_dirty_text(
        cx: &mut TestAppContext,
    ) {
        let (view, cx, calls) = reveal_fixture(cx);
        view.update(cx, |v, _| {
            v.files = false;
            v.navigator.panel = 1;
            v.narrow = true;
        });
        open(&view, cx, "/repo/src/deep/hidden.rs");
        assert!(!calls.lock().unwrap().iter().any(|(m, _)| m == "files_list"));
        view.update_in(cx, |v, window, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("unsaved nested", window, cx))
        });
        draw(cx);
        let at = cx.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .iter()
                .find(|e| e.label() == Some("Reveal in Explorer"))
                .expect("discoverable source action")
                .bounds()
                .center()
        });
        cx.simulate_click(at, Default::default());
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert!(v.files && v.show_tree);
            assert_eq!(v.navigator.panel, 0);
            assert_eq!(
                v.buffers[v.active.unwrap()].path,
                "/repo/src/deep/hidden.rs"
            );
            assert!(v.buffers[0].dirty(cx));
            assert!(v.reveal_notice.is_none());
        });
    }
    #[gpui::test]
    fn late_reveal_cannot_replace_git_navigation_and_missing_files_keep_buffers(
        cx: &mut TestAppContext,
    ) {
        let (view, cx, calls) = reveal_fixture(cx);
        view.update(cx, |v, _| v.files = false);
        open(&view, cx, "/repo/src/deep/hidden.rs");
        view.update_in(cx, |v, window, cx| {
            v.reveal_active(true, window, cx); // root listing pending
            v.navigate(1, window, cx); // user changes navigator before the reply
        });
        draw(cx);
        assert!(view.read_with(cx, |v, _| !v.files
            && v.navigator.panel == 1
            && v.reveal_target.is_none()));
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, _)| m == "files_list")
                .count(),
            1
        );
        view.update_in(cx, |v, window, cx| {
            v.test_rpc = Some(Arc::new(|method, args| {
                assert_eq!(method, "files_list");
                Ok(json!({"path":args["path"],"entries":[]}))
            }));
            v.reveal_active(true, window, cx);
            assert_eq!(
                v.tree_rows().len(),
                4,
                "keep existing rows while refreshing"
            );
        });
        draw(cx);
        view.read_with(cx, |v, _| {
            assert!(v.reveal_target.is_none());
            assert!(
                v.reveal_notice
                    .as_ref()
                    .unwrap()
                    .contains("open buffer is kept")
            );
            assert_eq!(v.buffers.len(), 1);
            assert_eq!(v.active, Some(0));
        });
        view.update_in(cx, |v, window, cx| {
            v.test_rpc = Some(Arc::new(|_, _| Err("Old folder read failed".into())));
            v.reveal_active(true, window, cx);
            v.navigate(1, window, cx);
        });
        draw(cx);
        view.read_with(cx, |v, _| {
            assert!(!v.files);
            assert!(
                v.error.is_none(),
                "an obsolete reveal must not replace editor feedback"
            );
            assert!(v.reveal_notice.is_none());
        });
    }
    #[gpui::test]
    fn quick_open_finds_unexpanded_files_and_preserves_dirty_buffers(cx: &mut TestAppContext) {
        use gpui::component::WindowExt as _;
        let (view, cx, calls) = fixture(cx);
        // Searching from Git does not change its navigator or crawl file folders.
        view.update(cx, |v, _| {
            v.files = false;
            v.navigator.panel = 1;
        });
        open(&view, cx, "/repo/main.rs");
        view.update_in(cx, |v, window, cx| {
            v.buffers[0].editor.update(cx, |e, cx| {
                e.set_value("unsaved source", window, cx);
                e.focus(window, cx);
            });
        });
        crate::test_shortcuts(cx, "cmd-p");
        draw(cx);
        cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
        cx.simulate_input("hidden");
        draw(cx);
        cx.update(|window, _| {
            let rows = gpui::base::test_support::snapshots(window);
            assert!(
                rows.iter()
                    .any(|e| e.label() == Some("Open src/deep/hidden.rs"))
            );
            assert!(
                rows.iter()
                    .any(|e| e.label() == Some("Open tests/hidden.rs"))
            );
        });
        crate::test_shortcuts(cx, "down");
        crate::test_shortcuts(cx, "enter");
        draw(cx);
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        assert_eq!(
            view.read_with(cx, |v, _| v.buffers[v.active.unwrap()].path.clone()),
            "/repo/src/deep/hidden.rs"
        );
        assert_eq!(
            view.read_with(cx, |v, cx| v.buffers[0].editor.read(cx).value().to_string()),
            "unsaved source"
        );
        assert!(view.read_with(cx, |v, cx| v.buffers[0].dirty(cx)));
        let requests = calls.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|(m, _)| m == "files_project_index")
                .count(),
            1
        );
        assert!(
            !requests
                .iter()
                .any(|(m, _)| m == "files_list" || m == "files_write_text")
        );
        let index = &requests
            .iter()
            .find(|(m, _)| m == "files_project_index")
            .unwrap()
            .1;
        assert_eq!(index["root"], "/repo");
        assert_eq!(index["expectedConnection"], json!(["scope-a"]));
    }
    #[gpui::test]
    fn quick_open_escape_restores_editor_without_opening_a_file(cx: &mut TestAppContext) {
        use gpui::component::WindowExt as _;
        let (view, cx, calls) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        view.update_in(cx, |v, window, cx| v.focus_document(window, cx));
        crate::test_shortcuts(cx, "cmd-p");
        draw(cx);
        cx.simulate_input("hidden");
        draw(cx);
        crate::test_shortcuts(cx, "escape");
        draw(cx);
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        cx.simulate_input("// back in editor");
        draw(cx);
        assert!(view.read_with(cx, |v, cx| {
            v.buffers[0]
                .editor
                .read(cx)
                .value()
                .contains("// back in editor")
        }));
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, _)| m == "files_read_text")
                .count(),
            1
        );
        view.update_in(cx, |v, window, cx| {
            v.available = false;
            v.open_quick(window, cx);
            assert!(!window.has_active_dialog(cx));
        });
    }
    #[gpui::test]
    fn quick_open_rejects_invalid_index_and_cannot_open_after_project_disconnect(
        cx: &mut TestAppContext,
    ) {
        let (view, cx, calls) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        view.update_in(cx, |v, window, cx| v.open_quick(window, cx));
        draw(cx);
        cx.simulate_input("hidden");
        draw(cx);
        view.update(cx, |v, _| v.available = false);
        crate::test_shortcuts(cx, "enter");
        draw(cx);
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, _)| m == "files_read_text")
                .count(),
            1
        );
        crate::test_shortcuts(cx, "escape");
        draw(cx);
        view.update_in(cx, |v, window, cx| {
            v.available = true;
            v.test_rpc = Some(Arc::new(|method, _| {
                assert_eq!(method, "files_project_index");
                Ok(json!({"paths":["../outside.rs"]}))
            }));
            v.open_quick(window, cx);
        });
        draw(cx);
        crate::test_shortcuts(cx, "enter");
        draw(cx);
        assert_eq!(view.read_with(cx, |v, _| v.buffers.len()), 1);
        cx.update(|window, _| {
            let rows = gpui::base::test_support::snapshots(window);
            assert!(!rows.iter().any(|e| e.label() == Some("Open ../outside.rs")));
        });
    }
    #[gpui::test]
    fn edit_save_undo_and_tab_switch_preserve_buffers(cx: &mut TestAppContext) {
        let (view, cx, calls) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let editor = view.read_with(cx, |v, _| v.buffers[0].editor.clone());
        cx.update(|window, cx| {
            editor.update(cx, |e, cx| {
                e.focus(window, cx);
                e.set_selected_range(0..0, cx);
            })
        });
        cx.simulate_input("// edited\n");
        draw(cx);
        assert!(view.read_with(cx, |v, cx| v.dirty(cx)));
        crate::test_shortcuts(cx, "cmd-s");
        draw(cx);
        let requests = calls.lock().unwrap().clone();
        let (_, save) = requests
            .iter()
            .find(|(m, _)| m == "files_write_text")
            .expect("Cmd/Ctrl-S sends a save");
        assert_eq!(save["expectedConnection"], json!(["scope-a"]));
        assert_eq!(save["revision"], "original");
        assert!(save["content"].as_str().unwrap().starts_with("// edited\n"));
        assert!(!view.read_with(cx, |v, cx| v.dirty(cx)));
        crate::test_shortcuts(cx, "cmd-z");
        draw(cx);
        assert!(
            view.read_with(cx, |v, cx| v.dirty(cx)),
            "saving must retain undo history"
        );
        let content = editor.read_with(cx, |e, _| e.value().to_string());
        open(&view, cx, "/repo/other.rs");
        open(&view, cx, "/repo/main.rs");
        assert_eq!(view.read_with(cx, |v, _| v.buffers.len()), 2);
        assert_eq!(editor.read_with(cx, |e, _| e.value().to_string()), content);
        assert!(view.read_with(cx, |v, cx| v.buffers[0].dirty(cx)));
    }
    #[gpui::test]
    fn keyboard_tabs_wrap_reveal_and_preserve_dirty_buffers(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        for i in 0..8 {
            open(&view, cx, &format!("/repo/long-document-name-{i}.rs"));
        }
        view.update_in(cx, |v, window, cx| {
            v.buffers[7]
                .editor
                .update(cx, |e, cx| e.set_value("unsaved", window, cx));
        });
        crate::test_shortcuts(cx, "ctrl-tab");
        draw(cx);
        assert!(view.read_with(cx, |v, cx| v.active == Some(0) && v.buffers[7].dirty(cx)));
        crate::test_shortcuts(cx, "ctrl-shift-tab");
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.active == Some(7)));
        assert!(
            view.read_with(cx, |v, _| v.tab_scroll.offset().x < px(0.)),
            "keyboard selection reveals the offscreen tab"
        );
        crate::test_shortcuts(cx, "cmd-w");
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.buffers.len() == 8));
        let cancel = cx.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .iter()
                .find(|e| e.label() == Some("Keep editing"))
                .expect("close shortcut uses dirty-buffer confirmation")
                .bounds()
                .center()
        });
        cx.simulate_click(cancel, Default::default());
        draw(cx);
        // Re-focus a clean source, then close it through the production key binding.
        open(&view, cx, "/repo/long-document-name-0.rs");
        crate::test_shortcuts(cx, "cmd-w");
        draw(cx);
        assert!(view.read_with(cx, |v, cx| v.buffers.len() == 7 && v.dirty(cx)));
    }
    #[gpui::test]
    fn keyboard_reviews_and_sources_share_tab_order(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let document = project_documents::ReviewDocument {
            key: project_documents::ReviewKey::Commit("123456789".into()),
            entry: json!({"oid":{"hex":"123456789"}}),
            status: Value::Null.into(),
            preview: false,
        };
        view.update(cx, |v, _| {
            v.navigator.documents.open(document);
        });
        crate::test_shortcuts(cx, "ctrl-shift-tab");
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.reviewing));
        crate::test_shortcuts(cx, "ctrl-tab");
        draw(cx);
        assert!(view.read_with(cx, |v, _| !v.reviewing && v.active == Some(0)));
        crate::test_shortcuts(cx, "ctrl-shift-tab");
        crate::test_shortcuts(cx, "cmd-w");
        draw(cx);
        assert!(view.read_with(cx, |v, _| !v.reviewing
            && v.buffers.len() == 1
            && v.navigator.documents.reviews.is_empty()));
    }
    #[gpui::test]
    fn typing_during_save_or_reload_never_replaces_newer_edits(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        view.update_in(cx, |v, window, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("sent", window, cx));
            v.save(window, cx);
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("typed while saving", window, cx));
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |v, _| v.buffers[0].saved.clone()),
            "sent"
        );
        assert_eq!(
            view.read_with(cx, |v, cx| v.buffers[0].editor.read(cx).value().to_string()),
            "typed while saving"
        );
        assert!(view.read_with(cx, |v, cx| v.dirty(cx)));
        view.update_in(cx, |v, window, cx| {
            v.read_again("/repo/main.rs".into(), window, cx);
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("typed while reloading", window, cx));
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |v, cx| v.buffers[0].editor.read(cx).value().to_string()),
            "typed while reloading"
        );
        assert_eq!(
            view.read_with(cx, |v, _| v.buffers[0].saved.clone()),
            "sent"
        );
        assert!(view.read_with(cx, |v, _| {
            v.buffers[0]
                .error
                .as_deref()
                .unwrap()
                .contains("while it was reloading")
        }));
    }
    #[gpui::test]
    fn dirty_close_requires_confirmation_and_pending_save_cannot_close(cx: &mut TestAppContext) {
        use gpui::component::WindowExt as _;
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        view.update_in(cx, |v, window, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("keep this", window, cx));
            v.close("/repo/main.rs".into(), window, cx);
            assert!(window.has_active_dialog(cx));
            assert_eq!(v.buffers.len(), 1);
        });
        draw(cx);
        let cancel = cx.update(|window, _| {
            let buttons = gpui::base::test_support::snapshots(window);
            assert!(buttons.iter().any(|e| e.label() == Some("Discard changes")));
            buttons
                .iter()
                .find(|e| e.label() == Some("Keep editing"))
                .expect("visible cancel button")
                .bounds()
                .center()
        });
        cx.simulate_click(cancel, Default::default());
        draw(cx);
        view.update_in(cx, |v, window, cx| {
            assert!(!window.has_active_dialog(cx));
            assert_eq!(v.buffers.len(), 1);
            v.buffers[0].saving = true;
            v.close("/repo/main.rs".into(), window, cx);
            assert!(!window.has_active_dialog(cx));
            assert_eq!(v.buffers.len(), 1);
        });
    }
    use gpui::prelude::*;
    struct WorkspaceHarness {
        owner: Entity<crate::Newport>,
    }
    impl gpui::Render for WorkspaceHarness {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div().size_full().flex().flex_col().child(
                self.owner
                    .update(cx, |owner, cx| owner.project_workspace_view(window, cx)),
            )
        }
    }
    #[gpui::test]
    fn unified_navigation_preserves_editor_and_git_selection(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let editor = view.read_with(cx, |v, _| v.buffers[0].editor.clone());
        view.update_in(cx, |v, window, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("unsaved source", window, cx))
        });
        let owner = cx.update(|window, cx| {
            let owner = cx.new(|cx| crate::page_perf_tests::owner(window, cx));
            owner.update(cx, |o, cx| {
                o.server = "server-a".into();
                o.git.repo =
                    json!({"repoId":"repo","root":{"bytesB64":"L3JlcG8=","display":"/repo"}});
                o.git.page =
                    json!({"snapshot":"status","entries":[{"entryId":"change","path":{"display":"working.rs","bytesB64":"d29ya2luZy5ycw=="},"unstaged":true}],"metadata":{"totalEntries":1}});
                o.code_workspaces.push(super::Retained {
                    server: "server-a".into(),
                    root: "/repo".into(),
                    editor: view.clone(),
                    _subscription: cx.subscribe(&view, |_, _, _: &super::Saved, _| {}),
                    _observe: cx.observe(&view, |_, _, cx| cx.notify()),
                    _documents: cx.subscribe(&view, |o, _, event: &project_documents::Event, cx| { if let project_documents::Event::Open(document) = event { o.project_open_document(document.clone(), cx); } }),
                });
            });
            let clone = owner.clone();
            window.replace_root(cx, |window, cx| {
                let h = cx.new(|_| WorkspaceHarness { owner: clone });
                gpui::base::Root::new(h, window, cx)
            });
            owner
        });
        // An unavailable retained project never mounts a terminal or starts a PTY.
        view.update_in(cx, |v, window, cx| {
            v.toggle_terminal(window, cx);
            v.available = false;
        });
        draw(cx);
        assert!(view.read_with(cx, |v, cx| {
            v.terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .session_ids(cx)
                .is_empty()
        }));
        cx.update(|window, _| {
            assert!(
                !gpui::base::test_support::snapshots(window)
                    .iter()
                    .any(|e| e.label() == Some("Remote terminal"))
            )
        });
        view.update_in(cx, |v, window, cx| {
            v.toggle_terminal(window, cx);
            v.available = true;
        });
        let click = |cx: &mut VisualTestContext, label: &str| {
            draw(cx);
            let at = cx.update(|window, _| {
                gpui::base::test_support::snapshots(window)
                    .iter()
                    .find(|e| e.label() == Some(label))
                    .unwrap_or_else(|| panic!("missing {label}"))
                    .bounds()
                    .center()
            });
            cx.simulate_click(at, Default::default());
            draw(cx);
        };
        view.update(cx, |v, cx| {
            v.navigator.collections[0].set_page(owner.read(cx).git.page.clone());
            v.navigator.collections[1].set_page(json!({"snapshot":"history","entries":[{"oid":{"hex":"abc1234"},"message":{"display":"Saved commit"}}]}));
        });
        click(cx, "Git");
        assert!(view.read_with(cx, |v, cx| !v.files && !v.reviewing && v.dirty(cx)));
        click(cx, "Open diff working.rs");
        assert!(view.read_with(cx, |v, _| v.reviewing));
        click(cx, "Keep working.rs · Diff open");
        click(cx, "History");
        assert_eq!(
            owner.read_with(cx, |o, _| o.git.tab),
            0,
            "browsing history does not replace the active comparison"
        );
        assert!(view.read_with(cx, |v, _| v.reviewing && v.navigator.panel == 2));
        click(cx, "Open commit abc1234");
        assert_eq!(
            owner.read_with(cx, |o, _| o.git.history.oid.clone()),
            "abc1234"
        );
        click(cx, "Keep Commit abc1234 open");
        click(cx, "Changes");
        assert_eq!(
            owner.read_with(cx, |o, _| o.git.history.oid.clone()),
            "abc1234"
        );
        click(cx, "Close Commit abc1234");
        assert_eq!(
            owner.read_with(cx, |o, _| crate::text(&o.git.selected["path"], "display")),
            "working.rs"
        );
        click(cx, "main.rs •");
        click(cx, "Changes");
        assert!(view.read_with(cx, |v, cx| !v.reviewing && v.dirty(cx)));
        assert_eq!(
            editor.read_with(cx, |e, _| e.value().to_string()),
            "unsaved source"
        );
        click(cx, "Review working.rs · Diff");
        click(cx, "Files");
        assert!(view.read_with(cx, |v, _| v.reviewing && v.files));
        click(cx, "Close working.rs · Diff");
        assert!(view.read_with(cx, |v, cx| !v.reviewing && v.dirty(cx)));
        // Compact navigation leaves a route back to the retained dirty source.
        cx.simulate_resize(size(px(450.), px(650.)));
        owner.update(cx, |o, _| o.content_width = 450.);
        view.update(cx, |v, _| {
            v.narrow = true;
            v.show_tree = true;
        });
        click(cx, "Return to open tab");
        assert!(view.read_with(cx, |v, cx| !v.show_tree && v.dirty(cx)));
        // History adapts to the actual center after the project panel takes its space.
        owner.update(cx, |o, _| {
            o.content_width = 850.;
            o.git.tab = 1;
        });
        view.update(cx, |v, _| {
            v.center_width = 560.;
        });
        assert!(owner.read_with(cx, |o, cx| o.project_review_compact(cx)));
        view.update(cx, |v, _| {
            v.center_width = 850.;
        });
        view.update(cx, |v, _| v.narrow = false);
        assert!(!owner.read_with(cx, |o, cx| o.project_review_compact(cx)));
        assert!(!owner.read_with(cx, |o, cx| o.project_history_stacked(false, cx)));
        view.update(cx, |v, _| v.center_width = 1100.);
        assert!(!owner.read_with(cx, |o, cx| o.project_history_stacked(false, cx)));
    }
    #[gpui::test]
    fn loading_a_directory_does_not_shift_file_rows(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        view.update_in(cx, |v, window, cx| v.list("/repo".into(), window, cx));
        draw(cx);
        let row_bounds = |cx: &mut VisualTestContext| {
            cx.update(|window, _| {
                gpui::base::test_support::snapshots(window)
                    .iter()
                    .find(|e| e.label() == Some("File main.rs"))
                    .expect("file row")
                    .bounds()
            })
        };
        let before = row_bounds(cx);
        view.update(cx, |v, cx| {
            v.loading.insert("/repo/subfolder".into());
            cx.notify();
        });
        draw(cx);
        assert_eq!(before, row_bounds(cx));
        view.update(cx, |v, cx| {
            v.loading.clear();
            cx.notify();
        });
        draw(cx);
        assert_eq!(before, row_bounds(cx));
    }
    #[gpui::test]
    fn late_file_read_cannot_interrupt_review(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        view.update_in(cx, |v, window, cx| {
            v.open("/repo/main.rs".into(), window, cx);
            v.show_review(cx);
        });
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.reviewing
            && v.active.is_none()
            && v.buffers.len() == 1));
    }
    #[gpui::test]
    fn host_exit_guards_preserve_dirty_and_saving_buffers(cx: &mut TestAppContext) {
        use gpui::component::WindowExt as _;
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        cx.update(|window, cx| {
            let owner = cx.new(|cx| crate::page_perf_tests::owner(window, cx));
            owner.update(cx, |owner, cx| {
                let subscription = cx.subscribe(&view, |_, _, _: &super::Saved, _| {});
                owner.code_workspaces.push(super::Retained {
                    server: "server-a".into(),
                    root: "/repo".into(),
                    editor: view.clone(),
                    _observe: cx.observe(&view, |_, _, cx| cx.notify()),
                    _documents: cx.subscribe(&view, |_, _, _: &project_documents::Event, _| {}),
                    _subscription: subscription,
                });
            });
            window.replace_root(cx, |window, cx| gpui::base::Root::new(owner, window, cx));
        });
        view.update_in(cx, |v, window, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("unsaved", window, cx));
        });
        // Defer host calls until the window's update lease has been released.
        cx.update(|_, cx| {
            cx.defer(|cx| {
                assert!(crate::ensure_editors_saved(cx).is_err());
                crate::request_quit(cx).unwrap();
            })
        });
        draw(cx);
        let cancel = cx.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            gpui::base::test_support::snapshots(window)
                .iter()
                .find(|e| e.label() == Some("Keep editing"))
                .unwrap()
                .bounds()
                .center()
        });
        cx.simulate_click(cancel, Default::default());
        draw(cx);
        assert!(view.read_with(cx, |v, cx| v.dirty(cx)));
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        view.update(cx, |v, cx| {
            v.buffers[0].saved = v.buffers[0].editor.read(cx).value().to_string();
            v.buffers[0].saving = true;
        });
        cx.update(|_, cx| {
            cx.defer(|cx| {
                assert!(crate::ensure_editors_saved(cx).is_err());
                crate::request_quit(cx).unwrap();
            })
        });
        draw(cx);
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        view.update(cx, |v, _| v.buffers[0].saving = false);
        cx.update(|_, cx| cx.defer(|cx| assert!(crate::ensure_editors_saved(cx).is_ok())));
        draw(cx);
    }
    #[gpui::test]
    fn narrow_read_failures_remain_visible_and_retryable(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        cx.simulate_resize(size(px(450.), px(650.)));
        let visible_feedback = |elements: &[gpui::base::test_support::ElementSnapshot],
                                label: &str| {
            let node = elements.iter().find(|e| e.label() == Some(label)).unwrap();
            let bounds = node.bounds();
            assert!(node.visible());
            assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
            assert!(bounds.origin.x >= px(0.) && bounds.origin.y >= px(0.));
            assert!(bounds.origin.x + bounds.size.width <= px(450.));
            assert!(bounds.origin.y + bounds.size.height <= px(650.));
        };
        view.update_in(cx, |v, window, cx| {
            v.narrow = true;
            v.test_rpc = Some(Arc::new(|_, _| Err("Connection interrupted".into())));
            v.list("/repo".into(), window, cx);
        });
        draw(cx);
        let retry = cx.update(|window, _| {
            let elements = gpui::base::test_support::snapshots(window);
            visible_feedback(&elements, "Connection interrupted");
            visible_feedback(&elements, "Retry");
            assert!(
                !elements
                    .iter()
                    .any(|e| e.label() == Some("No files in this folder"))
            );
            elements
                .iter()
                .find(|e| e.label() == Some("Retry"))
                .unwrap()
                .bounds()
                .center()
        });
        view.update(cx, |v, _| {
            v.test_rpc = Some(Arc::new(|_, args| {
                Ok(json!({"path":args["path"],"entries":[]}))
            }))
        });
        cx.simulate_click(retry, Default::default());
        draw(cx);
        assert!(view.read_with(cx, |v, _| v.error.is_none()));
        view.update(cx, |v, _| {
            v.test_rpc = Some(Arc::new(|_, _| Err("Cannot read this file".into())))
        });
        open(&view, cx, "/repo/main.rs");
        assert!(view.read_with(cx, |v, _| v.show_tree));
        cx.update(|window, _| {
            let elements = gpui::base::test_support::snapshots(window);
            visible_feedback(&elements, "main.rs: Cannot read this file");
            visible_feedback(&elements, "Retry");
        });
    }
    #[gpui::test]
    fn narrow_workspace_switches_between_files_and_retained_editor(cx: &mut TestAppContext) {
        let (view, cx, calls) = fixture(cx);
        view.update(cx, |v, _| v.narrow = true);
        cx.simulate_resize(size(px(450.), px(650.)));
        open(&view, cx, "/repo/main.rs");
        assert!(!view.read_with(cx, |v, _| v.show_tree));
        view.update_in(cx, |v, window, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("retained", window, cx));
            v.show_tree = true;
            v.list("/repo".into(), window, cx);
        });
        draw(cx);
        let reads = calls.lock().unwrap().len();
        open(&view, cx, "/repo/main.rs");
        assert_eq!(calls.lock().unwrap().len(), reads);
        assert!(!view.read_with(cx, |v, _| v.show_tree));
        assert_eq!(
            view.read_with(cx, |v, cx| v.buffers[0].editor.read(cx).value().to_string()),
            "retained"
        );
    }
    #[gpui::test]
    fn failed_save_keeps_edits_and_tree_is_lazy(cx: &mut TestAppContext) {
        let (view, cx, calls) = fixture(cx);
        view.update_in(cx, |v, window, cx| v.list("/repo".into(), window, cx));
        draw(cx);
        assert_eq!(view.read_with(cx, |v, _| v.tree_rows().len()), 2);
        assert_eq!(calls.lock().unwrap().len(), 1);
        open(&view, cx, "/repo/main.rs");
        view.update_in(cx, |v, window, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.set_value("unsaved", window, cx));
            v.test_rpc = Some(Arc::new(|_, _| Err("Remote file changed".into())));
            v.save(window, cx);
        });
        draw(cx);
        assert!(view.read_with(cx, |v, cx| v.dirty(cx)));
        assert_eq!(
            view.read_with(cx, |v, cx| v.buffers[0].editor.read(cx).value().to_string()),
            "unsaved"
        );
        assert_eq!(
            view.read_with(cx, |v, _| v.buffers[0].revision.clone()),
            "original"
        );
        assert!(view.read_with(cx, |v, _| v.buffers[0].error.is_some()));
    }
    #[gpui::test]
    fn explorer_status_reads_all_pages_independently_of_git_filter(cx: &mut TestAppContext) {
        use base64::Engine;
        let (view, cx, _) = fixture(cx);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        view.update_in(cx, |v, window, cx| {
            v.navigator.repo = "repo-a".into();
            v.navigator.query = "hidden-by-filter".into();
            v.test_rpc = Some(Arc::new(move |method, args| {
                assert_eq!(method, "git_request");
                let params = &args["request"]["params"];
                assert_eq!(params["filter"], json!({"text":"","group":"all"}));
                recorded.lock().unwrap().push(params.clone());
                let (path, next) = if params["cursor"].is_null() { ("src/first.rs", json!("page2")) } else { ("src/second.rs", Value::Null) };
                Ok(json!({"snapshot":"one", "entries":[{"path":{"bytesB64":base64::engine::general_purpose::STANDARD.encode(path)},"unstaged":true}], "nextCursor":next,"metadata":{"totalEntries":2}}))
            }));
            v.refresh_tree_status(false, window, cx);
        });
        draw(cx);
        view.update(cx, |v, _| {
            assert!(
                v.tree_status
                    .status("/repo", "/repo/src/first.rs")
                    .is_some()
            );
            assert!(
                v.tree_status
                    .status("/repo", "/repo/src/second.rs")
                    .is_some()
            );
            assert!(v.tree_status.status("/repo", "/repo/src").is_some());
            assert!(v.navigator.collections[0].page().is_null());
        });
        assert_eq!(calls.lock().unwrap().len(), 2);
        view.update_in(cx, |v, window, cx| v.refresh_tree_status(false, window, cx));
        draw(cx);
        assert_eq!(
            calls.lock().unwrap().len(),
            2,
            "unchanged renders must not poll again"
        );
        // A failed continuation must not publish a partial new snapshot or
        // retain colors from the old one.
        view.update_in(cx, |v, window, cx| {
            v.test_rpc = Some(Arc::new(|_, args| {
                if args["request"]["params"]["cursor"].is_string() { return Err("snapshot changed".into()); }
                Ok(json!({"snapshot":"two","entries":[],"nextCursor":"page2","metadata":{"totalEntries":2}}))
            }));
            v.refresh_tree_status(true, window, cx);
        });
        draw(cx);
        view.update(cx, |v, _| {
            assert!(v.tree_status.status("/repo", "/repo/src").is_none())
        });
    }
    struct HistoryHarness {
        editor: Entity<CodeWorkspace>,
    }
    impl gpui::Render for HistoryHarness {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
                .size_full()
                .child(self.editor.update(cx, |e, cx| e.collection_view(cx)))
        }
    }
    #[gpui::test]
    fn history_hit_targets_contain_both_lines_and_stay_inside_panel(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        view.update(cx, |v, _| {
            v.navigator.panel = 2;
            v.navigator.collections[1].set_page(json!({"entries":[
                {"oid":{"hex":"abc1234"},"message":{"display":"A very long commit subject that must truncate instead of widening the navigator"},"author":{"name":"A very long author name"},"time":0},
                {"oid":{"hex":"def5678"},"message":{"display":"Second commit"},"author":{"name":"Author"},"time":0}
            ]}));
            v.reviewing = true;
            v.navigator.documents.active = Some(project_documents::ReviewKey::Commit("abc1234".into()));
        });
        cx.update(|window, cx| {
            window.replace_root(cx, |window, cx| {
                let h = cx.new(|_| HistoryHarness {
                    editor: view.clone(),
                });
                gpui::base::Root::new(h, window, cx)
            });
        });
        for width in [220., 290., 420.] {
            cx.simulate_resize(size(px(width), px(250.)));
            draw(cx);
            let content = cx
                .debug_bounds("project-nav-content-0")
                .expect("history text");
            cx.update(|window, _| {
                let nodes = gpui::base::test_support::snapshots(window);
                let first = nodes
                    .iter()
                    .find(|e| e.label() == Some("Open commit abc1234"))
                    .unwrap()
                    .bounds();
                let second = nodes
                    .iter()
                    .find(|e| e.label() == Some("Open commit def5678"))
                    .unwrap()
                    .bounds();
                assert!(
                    first.size.height > content.size.height,
                    "both lines need vertical padding"
                );
                assert!(content.top() >= first.top() && content.bottom() <= first.bottom());
                assert!(
                    first.right() <= px(width),
                    "hit target must stay inside navigator"
                );
                assert!(
                    content.right() <= first.right(),
                    "text must fit the hit target"
                );
                assert!(second.top() >= first.bottom(), "rows must not overlap");
            });
        }
    }
    #[gpui::test]
    fn changes_checkboxes_align_and_stage_only_their_snapshot_entry(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        let staged = Arc::new(Mutex::new(Vec::new()));
        let observed = staged.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &project_documents::Event, _| {
                if let project_documents::Event::Stage {
                    snapshot,
                    entries,
                    stage,
                } = event
                {
                    observed
                        .lock()
                        .unwrap()
                        .push((snapshot.clone(), entries.clone(), *stage));
                }
            })
        });
        let first = json!({"entryId":"first", "path":{"display":"src/deep/first.rs"},"unstaged":true,"flags":256});
        let second = json!({"entryId":"second", "path":{"display":"short.rs"},"untracked":true});
        view.update(cx, |v, _| {
            v.navigator.panel = 1;
            v.navigator.writable = true;
            v.navigator.collections[0]
                .set_page(json!({"snapshot":"current","entries":[first,second]}));
        });
        cx.update(|window, cx| {
            window.replace_root(cx, |window, cx| {
                let h = cx.new(|_| HistoryHarness {
                    editor: view.clone(),
                });
                gpui::base::Root::new(h, window, cx)
            });
        });
        for width in [220., 290., 420.] {
            cx.simulate_resize(size(px(width), px(300.)));
            draw(cx);
            cx.update(|window, _| {
                let nodes = gpui::base::test_support::snapshots(window);
                let bounds = |label: &str| {
                    nodes
                        .iter()
                        .find(|e| e.label() == Some(label))
                        .unwrap()
                        .bounds()
                };
                let first = bounds("Stage src/deep/first.rs");
                let second = bounds("Stage short.rs");
                assert_eq!(first.center().x, second.center().x);
                assert!(first.right() <= px(width));
                assert!(bounds("Open diff src/deep/first.rs").right() <= first.left());
            });
        }
        let at = cx.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .iter()
                .find(|e| e.label() == Some("Stage src/deep/first.rs"))
                .unwrap()
                .bounds()
                .center()
        });
        cx.simulate_click(at, Default::default());
        draw(cx);
        assert_eq!(
            *staged.lock().unwrap(),
            vec![("current".into(), vec![first], true)]
        );
        assert!(
            view.read_with(cx, |v, _| v.navigator.documents.active.is_none()),
            "staging must not open a diff"
        );
    }
    #[gpui::test]
    fn changes_auto_page_near_end_and_groups_collapse_without_losing_review(
        cx: &mut TestAppContext,
    ) {
        let (view, cx, calls) = fixture(cx);
        let entry = |i| json!({"entryId":format!("e{i}"),"path":{"display":format!("docs/file-{i:03}.md")},"untracked":true});
        let first: Vec<_> = (0..60).map(entry).collect();
        let next: Vec<_> = (60..120).map(entry).collect();
        let seen = calls.clone();
        view.update(cx, |v, _| {
            v.navigator.repo = "repo".into();
            v.navigator.panel = 1;
            v.navigator.collections[0].set_page(
                json!({"snapshot":"s","entries":first,"nextCursor":"second","metadata":{}}),
            );
            v.test_rpc = Some(Arc::new(move |method, args| {
                assert_eq!(method, "git_request");
                seen.lock().unwrap().push((method.into(), args.clone()));
                match args["request"]["params"]["cursor"].as_str() {
                    Some("second") => Ok(
                        json!({"snapshot":"s","entries":next,"nextCursor":"third","metadata":{}}),
                    ),
                    Some("third") => Err("Page unavailable".into()),
                    other => panic!("unexpected cursor {other:?}"),
                }
            }));
        });
        cx.update(|window, cx| {
            window.replace_root(cx, |window, cx| {
                let h = cx.new(|_| HistoryHarness {
                    editor: view.clone(),
                });
                gpui::base::Root::new(h, window, cx)
            });
        });
        cx.simulate_resize(size(px(290.), px(300.)));
        draw(cx);
        assert!(
            calls.lock().unwrap().is_empty(),
            "top of a long list must not prefetch"
        );
        view.update(cx, |v, cx| {
            v.navigator.collections[0]
                .scroll
                .scroll_to_item(60, gpui::ScrollStrategy::Bottom);
            cx.notify();
        });
        for _ in 0..4 {
            draw(cx);
        }
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(
            view.read_with(cx, |v, _| v.navigator.collections[0].page()["entries"]
                .as_array()
                .unwrap()
                .len()),
            120
        );
        view.update(cx, |v, cx| {
            v.navigator.collections[0]
                .scroll
                .scroll_to_item(120, gpui::ScrollStrategy::Bottom);
            cx.notify();
        });
        for _ in 0..6 {
            draw(cx);
        }
        assert_eq!(
            calls.lock().unwrap().len(),
            2,
            "failed pages must not retry every render"
        );
        view.update(cx, |v, cx| {
            v.navigator.collections[0]
                .scroll
                .scroll_to_item(0, gpui::ScrollStrategy::Top);
            cx.notify();
        });
        draw(cx);
        let click = |cx: &mut VisualTestContext, label: &str| {
            let at = cx.update(|window, _| {
                gpui::base::test_support::snapshots(window)
                    .iter()
                    .find(|e| e.label() == Some(label))
                    .unwrap()
                    .bounds()
                    .center()
            });
            cx.simulate_click(at, Default::default());
            draw(cx);
        };
        click(cx, "Open diff docs/file-000.md");
        let selected = view.read_with(cx, |v, _| v.navigator.documents.active.clone());
        click(cx, "Collapse Untracked");
        cx.update(|window, _| {
            let rows = gpui::base::test_support::snapshots(window);
            assert!(
                rows.iter()
                    .any(|e| e.label() == Some("Expand Untracked") && e.expanded() == Some(false))
            );
            assert!(
                !rows
                    .iter()
                    .any(|e| e.label().is_some_and(|l| l.starts_with("Open diff")))
            );
        });
        assert_eq!(
            view.read_with(cx, |v, _| v.navigator.documents.active.clone()),
            selected
        );
        click(cx, "Expand Untracked");
        cx.update(|window, _| {
            assert!(
                gpui::base::test_support::snapshots(window)
                    .iter()
                    .any(|e| e.label() == Some("Open diff docs/file-000.md"))
            )
        });
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    #[gpui::test]
    fn short_files_do_not_scroll_but_overflowing_code_still_does(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let editor = view.read_with(cx, |v, _| v.buffers[0].editor.clone());
        editor.update_in(cx, |e, window, cx| {
            e.set_value("short line\n".repeat(16), window, cx);
        });
        draw(cx);
        let scroll = |cx: &mut VisualTestContext, dx: f32, dy: f32| {
            let position = cx.update(|window, _| {
                gpui::base::test_support::snapshots(window)
                    .iter()
                    .find(|e| e.label() == Some("Code editor"))
                    .expect("mounted editor")
                    .bounds()
                    .center()
            });
            cx.simulate_event(gpui::ScrollWheelEvent {
                position,
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(dx), px(dy))),
                ..Default::default()
            });
            draw(cx);
        };
        scroll(cx, 0., -500.);
        assert_eq!(editor.read_with(cx, |e, _| e.scroll_offset().y), px(0.));
        // Resizing can legitimately make those same seventeen rows overflow.
        cx.simulate_resize(size(px(1000.), px(300.)));
        draw(cx);
        scroll(cx, 0., -500.);
        assert!(editor.read_with(cx, |e, _| e.scroll_offset().y) < px(0.));
        cx.simulate_resize(size(px(1000.), px(650.)));
        draw(cx);
        assert_eq!(editor.read_with(cx, |e, _| e.scroll_offset().y), px(0.));
        // Disabling trailing blank rows must not disable actual overflow.
        editor.update_in(cx, |e, window, cx| {
            e.set_value(
                format!("{}\n{}", "wide ".repeat(100), "line\n".repeat(100)),
                window,
                cx,
            );
        });
        draw(cx);
        scroll(cx, -300., -500.);
        let offset = editor.read_with(cx, |e, _| e.scroll_offset());
        assert!(offset.y < px(0.));
        assert!(offset.x < px(0.));
    }

    #[gpui::test]
    fn returning_to_projects_reopens_retained_commit_on_the_new_repository(
        cx: &mut TestAppContext,
    ) {
        let (view, cx, _) = fixture(cx);
        let owner = cx.update(|window, cx| cx.new(|cx| crate::page_perf_tests::owner(window, cx)));
        let oid = "559718aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let commit = json!({"oid":{"hex":oid},"message":{"display":"Retained commit"},
            "messageTruncated":false,"parents":[],"author":{"name":"Author"},"time":1});
        let document = project_documents::ReviewDocument {
            key: project_documents::ReviewKey::Commit(oid.into()),
            entry: commit.clone(),
            status: Value::Null.into(),
            preview: false,
        };
        view.update(cx, |v, _| {
            v.navigator.documents.open(document.clone());
            v.reviewing = true;
        });
        owner.update(cx, |o, cx| {
            let (work, requests) = std::sync::mpsc::sync_channel(64);
            o.work = work.into();
            o.server = "server-a".into();
            o.page = crate::Page::Projects;
            o.git.tab = 1;
            o.git.selected = commit.clone();
            o.git.history.oid = oid.into();
            o.code_workspaces.push(super::Retained {
                server: "server-a".into(), root: "/repo".into(), editor: view.clone(),
                _subscription: cx.subscribe(&view, |_, _, _: &super::Saved, _| {}),
                _observe: cx.observe(&view, |_, _, cx| cx.notify()),
                _documents: cx.subscribe(&view, |_, _, _: &project_documents::Event, _| {}),
            });
            let stale = o.resources.begin("git-history-files").unwrap();
            o.navigate(crate::Page::Ports, cx);
            assert!(o.git.history.oid.is_empty());
            o.navigate(crate::Page::Projects, cx);
            assert!(!o.resources.complete("git-history-files", stale, Ok(json!({"snapshot":"old"}))));
            // Complete the reopened handle just as navigation's repo.open does.
            o.git.tab = 1;
            let repo = json!({"repoId":"reopened","root":{"bytesB64":"L3JlcG8=","display":"/repo"},"bare":false});
            let generation = o.resources.begin("repository").unwrap();
            o.resources.complete("repository", generation, Ok(repo));
            o.git_completed("repository");
            requests.try_iter().for_each(drop);
            o.project_restore_document(cx);
            assert_eq!(o.git.history.oid, oid);
            assert_eq!(o.git.selected, commit);
            assert_eq!(o.value("git-history-detail")["message"]["display"], "Retained commit");
            let reads: Vec<_> = requests.try_iter().collect();
            assert!(reads.iter().any(|r| r.key == "git-history-files"
                && r.args["request"]["params"]["repoId"] == "reopened"
                && r.args["request"]["params"]["commitOid"] == oid));
            // Rendering again must not restart in-flight reads or clear selection.
            o.git.history.path = json!({"display":"src/main.rs"});
            o.project_restore_document(cx);
            assert!(requests.try_recv().is_err());
            assert_eq!(o.git.history.path["display"], "src/main.rs");
            // A source editor in front must stay in front; inactive review tabs
            // are not a reason to replace the user's active document.
            view.update(cx, |v, _| v.reviewing = false);
            o.git.history = Default::default();
            o.git.restore_document = true;
            o.project_restore_document(cx);
            assert!(o.git.history.oid.is_empty());
            assert!(!view.read(cx).reviewing);
            assert!(requests.try_recv().is_err());
            // Retained tabs are scoped to both server and checkout root.
            view.update(cx, |v, _| v.reviewing = true);
            o.server = "other-server".into();
            o.git.restore_document = true;
            o.project_restore_document(cx);
            assert!(o.git.history.oid.is_empty());
            o.server = "server-a".into();
            o.git.repo["root"] = json!({"display":"/other","bytesB64":"L290aGVy"});
            o.project_restore_document(cx);
            assert!(o.git.history.oid.is_empty());
            assert!(requests.try_recv().is_err());
        });
    }

    #[gpui::test]
    fn reopening_review_retires_failed_or_pending_legacy_status(cx: &mut TestAppContext) {
        let (_, cx, _) = fixture(cx);
        let owner = cx.update(|window, cx| cx.new(|cx| crate::page_perf_tests::owner(window, cx)));
        let entry = json!({"entryId":"e","path":{"display":"docs/file.md"},"untracked":true});
        let status =
            json!({"snapshot":"current","entries":[entry],"nextCursor":null,"metadata":{}});
        let document = project_documents::ReviewDocument {
            key: project_documents::ReviewKey::File {
                path: "docs/file.md".into(),
                staged: false,
            },
            entry: entry.clone(),
            status: status.clone().into(),
            preview: true,
        };
        owner.update(cx,|o,cx| {
            o.server="server-a".into();
            o.git.repo=json!({"repoId":"repo","bare":false,"capabilities":{"readOnly":false,"workingTree":true}});
            o.git.tab=0;o.git.page=status.clone();o.git.selected=entry.clone();
            o.git.diff=json!({"snapshot":"old-diff"});
            let generation=o.resources.begin("git-list").unwrap();
            o.resources.complete("git-list",generation,Err("Old read failed".into()));
            assert!(!o.git_can_change_index(true));
            o.project_open_document(document.clone(),cx);
            assert!(o.git_can_change_index(true),"fresh navigator evidence must recover file staging");
            assert!(o.git.diff.is_null(),"failed same-file review must reload instead of returning early");
            o.resources.invalidate("git-list");
            let abandoned=o.resources.begin("git-list").unwrap();
            o.git.diff=json!({"snapshot":"old-diff"});
            o.project_open_document(document,cx);
            assert!(o.git_can_change_index(true));
            assert!(!o.resources.complete("git-list",abandoned,Ok(json!({"snapshot":"wrong"}))));
            assert_eq!(o.git.page,status);
        });
    }
    #[gpui::test]
    fn native_file_events_refresh_clean_buffers_without_losing_selection(cx: &mut TestAppContext) {
        let (view, cx, calls) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let original = view.read_with(cx, |v, _| v.buffers[0].editor.clone());
        original.update(cx, |e, cx| e.set_selected_range(1..4, cx));
        let seen = calls.clone();
        view.update(cx, |v, _| {
            v.test_rpc = Some(Arc::new(move |method, args| {
                seen.lock().unwrap().push((method.into(), args.clone()));
                assert_eq!(method, "files_read_text");
                Ok(json!({"path":args["path"],"content":"external update","revision":"external"}))
            }))
        });
        view.update_in(cx, |v, window, cx| {
            v.apply_file_events(
                &json!({"paths":["/repo/elsewhere.rs"],"reset":false}),
                &Default::default(),
            );
            v.refresh_watched_files(window, cx);
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |v, _| v.buffers[0].editor.entity_id()),
            original.entity_id()
        );
        let reads = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == "files_read_text")
            .count();
        view.update_in(cx, |v, window, cx| {
            v.apply_file_events(
                &json!({"paths":["/repo/main.rs"],"reset":false}),
                &Default::default(),
            );
            v.refresh_watched_files(window, cx);
        });
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(
                v.buffers[0].editor.read(cx).value().as_str(),
                "external update"
            );
            assert_eq!(v.buffers[0].editor.read(cx).selected_range(), 1..4);
            assert!(!v.buffers[0].dirty(cx));
        });
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, _)| m == "files_read_text")
                .count(),
            reads + 1
        );
        let current = view.read_with(cx, |v, _| v.buffers[0].editor.entity_id());
        view.update_in(cx, |v, window, cx| {
            v.refresh_external("/repo/main.rs".into(), window, cx)
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |v, _| v.buffers[0].editor.entity_id()),
            current,
            "same revision must retain editor and undo"
        );
    }
    #[gpui::test]
    fn external_changes_keep_dirty_buffers_and_fence_in_flight_saves(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let editor = view.read_with(cx, |v, _| v.buffers[0].editor.clone());
        view.update(cx, |v, _| {
            v.test_rpc = Some(Arc::new(|_, _| {
                Ok(json!({"content":"disk version","revision":"changed"}))
            }))
        });
        editor.update_in(cx, |e, window, cx| e.set_value("unsaved work", window, cx));
        view.update_in(cx, |v, window, cx| {
            v.refresh_external("/repo/main.rs".into(), window, cx)
        });
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(v.buffers[0].editor.entity_id(), editor.entity_id());
            assert_eq!(
                v.buffers[0].editor.read(cx).value().as_str(),
                "unsaved work"
            );
            assert!(v.buffers[0].conflict.is_some());
            assert!(v.buffers[0].error.is_none());
            assert!(v.buffers[0].dirty(cx));
        });
        view.update_in(cx, |v, window, cx| {
            v.refresh_external("/repo/main.rs".into(), window, cx);
            // A save completed after the watcher read began. Never install its stale reply.
            v.buffers[0].saved = "unsaved work".into();
            v.buffers[0].revision = "saved-after-read".into();
            v.buffers[0].error = None;
        });
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(v.buffers[0].revision, "saved-after-read");
            assert_eq!(
                v.buffers[0].editor.read(cx).value().as_str(),
                "unsaved work"
            );
            assert!(v.buffers[0].error.is_none());
        });
    }
    #[gpui::test]
    fn cached_opens_validate_and_conflicts_preserve_local_undo(cx: &mut TestAppContext) {
        let (view, cx, calls) = fixture(cx);
        let seen = calls.clone();
        view.update(cx, |v, _| {
            v.test_rpc = Some(Arc::new(move |method, args| {
                seen.lock().unwrap().push((method.into(), args.clone()));
                match method {
                    "files_read_text" if args["allowCached"] == true => {
                        Ok(json!({"content":"a\nb\nc\n","revision":"cached","cached":true}))
                    }
                    "files_read_text" => Ok(json!({"content":"A\nb\nc\n","revision":"remote"})),
                    "files_list" => Ok(json!({"path":args["path"],"entries":[]})),
                    _ => panic!("unexpected {method}"),
                }
            }));
        });
        open(&view, cx, "/repo/main.rs");
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(v.buffers[0].revision, "remote");
            assert_eq!(v.buffers[0].editor.read(cx).value().as_str(), "A\nb\nc\n");
        });
        let reads = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == "files_read_text")
            .count();
        assert_eq!(reads, 2, "cached opening must validate in the background");
        view.update_in(cx, |v, w, cx| {
            v.buffers[0]
                .editor
                .update(cx, |e, cx| e.replace_all("A\nb\nC\n", w, cx));
            v.test_rpc = Some(Arc::new(|method, _| {
                assert_eq!(method, "files_read_text");
                Ok(json!({"content":"remote A\nb\nc\n","revision":"remote-2"}))
            }));
            v.refresh_external("/repo/main.rs".into(), w, cx);
        });
        draw(cx);
        let original = view.read_with(cx, |v, _| v.buffers[0].editor.entity_id());
        view.update_in(cx, |v, w, cx| {
            assert!(v.buffers[0].conflict.is_some());
            v.save(w, cx); // unresolved conflicts cannot issue a write
            v.review_remote(w, cx);
        });
        draw(cx);
        cx.simulate_input("must not edit the remote snapshot");
        draw(cx);
        view.read_with(cx, |v, cx| {
            let preview = v.buffers[0].conflict.as_ref().unwrap().preview().unwrap();
            assert_eq!(preview.read(cx).value().as_str(), "remote A\nb\nc\n");
            assert_eq!(v.buffers[0].editor.read(cx).value().as_str(), "A\nb\nC\n");
        });
        view.update_in(cx, |v, w, cx| v.merge_remote(w, cx));
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert!(v.buffers[0].conflict.is_none());
            assert_eq!(v.buffers[0].editor.entity_id(), original);
            assert_eq!(v.buffers[0].revision, "remote-2");
            assert_eq!(v.buffers[0].saved, "remote A\nb\nc\n");
            assert_eq!(
                v.buffers[0].editor.read(cx).value().as_str(),
                "remote A\nb\nC\n"
            );
            assert!(v.buffers[0].dirty(cx));
        });
        crate::test_shortcuts(cx, "cmd-z");
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(v.buffers[0].editor.read(cx).value().as_str(), "A\nb\nC\n");
            assert!(v.buffers[0].dirty(cx));
        });
    }
}
