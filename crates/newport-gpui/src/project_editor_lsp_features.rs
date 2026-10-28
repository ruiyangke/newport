//! Discoverable language tools. Server edits are staged in undoable, unsaved
//! buffers, only after every file and every captured document has been checked.
use super::*;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use lsp_types::{OneOf, request};
#[path = "project_editor_lsp_actions.rs"]
mod actions;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in super::super) enum Tool {
    Format,
    References,
    Rename,
    Signature,
    Actions,
    OrganizeImports,
}
impl Tool {
    fn label(self) -> &'static str {
        match self {
            Self::Format => "Format document",
            Self::References => "Find references",
            Self::Rename => "Rename symbol…",
            Self::Signature => "Signature help",
            Self::Actions => "Code actions…",
            Self::OrganizeImports => "Organize imports",
        }
    }
    fn supported(self, caps: &lsp_types::ServerCapabilities) -> bool {
        match self {
            Self::Format => caps
                .document_formatting_provider
                .as_ref()
                .is_some_and(|c| !matches!(c, OneOf::Left(false))),
            Self::References => caps
                .references_provider
                .as_ref()
                .is_some_and(|c| !matches!(c, OneOf::Left(false))),
            Self::Rename => caps
                .rename_provider
                .as_ref()
                .is_some_and(|c| !matches!(c, OneOf::Left(false))),
            Self::Signature => caps.signature_help_provider.is_some(),
            Self::Actions | Self::OrganizeImports => {
                caps.code_action_provider.as_ref().is_some_and(|c| {
                    !matches!(c, lsp_types::CodeActionProviderCapability::Simple(false))
                })
            }
        }
    }
}

#[derive(Default)]
pub(in super::super) struct State {
    task: Option<Task<()>>,
    generation: u64,
    busy: bool,
    panel: Option<Panel>,
    rename_subscription: Option<Subscription>,
    command: Option<actions::CommandState>,
}
enum Panel {
    Actions {
        snapshot: Snapshot,
        items: Vec<lsp_types::CodeActionOrCommand>,
    },
    Message(String),
    References {
        source: String,
        server: &'static str,
        locations: Vec<lsp_types::Location>,
        total: usize,
    },
    Signature {
        label: String,
        documentation: Option<String>,
    },
    Rename {
        snapshot: Snapshot,
        input: Entity<InputState>,
    },
}
#[derive(Clone)]
struct Snapshot {
    path: String,
    server: Rc<Server>,
    document: Rc<Document>,
    text: Rope,
    cursor: usize,
    selection: std::ops::Range<usize>,
    /// All open files can contribute to a workspace edit.
    buffers: Vec<(String, Entity<EditorState>, Rope, Option<i32>)>,
}
impl Snapshot {
    fn valid(&self, workspace: &CodeWorkspace, cx: &App) -> bool {
        workspace.available
            && self.server.connection().is_some_and(|c| !c.is_closed())
            && workspace
                .language_servers
                .get(self.server.id)
                .is_some_and(|s| Rc::ptr_eq(s, &self.server))
            && self.buffers.iter().all(|(path, editor, text, version)| {
                workspace
                    .buffers
                    .iter()
                    .find(|b| &b.path == path)
                    .is_some_and(|b| {
                        &b.editor == editor
                            && b.editor.read(cx).text() == text
                            && b.lsp.as_ref().map(|a| a.document.version.get()) == *version
                            && !b.saving
                            && b.conflict.is_none()
                            && b.failure.is_none()
                            && b.page.is_none()
                    })
            })
    }
    fn position(&self) -> lsp_types::TextDocumentPositionParams {
        text_position(
            &self.document,
            to_server(self.server.encoding.get(), &self.text, self.cursor),
        )
    }
}

struct FileEdits {
    path: String,
    version: Option<i32>,
    edits: Vec<lsp_types::TextEdit>,
}
type EditDone = Box<
    dyn FnOnce(&mut CodeWorkspace, Result<(), String>, &mut Window, &mut Context<CodeWorkspace>),
>;
struct PendingEdits {
    snapshot: Snapshot,
    files: Vec<FileEdits>,
    loaded: HashMap<String, Value>,
    generation: u64,
    done: Option<EditDone>,
}

fn workspace_edits(edit: lsp_types::WorkspaceEdit, root: &str) -> Result<Vec<FileEdits>, String> {
    let mut files = Vec::new();
    if let Some(changes) = edit.document_changes {
        let changes = match changes {
            lsp_types::DocumentChanges::Edits(changes) => changes,
            lsp_types::DocumentChanges::Operations(_) => {
                return Err(
                    "This operation includes file operations that Newport cannot apply yet.".into(),
                );
            }
        };
        for change in changes {
            let edits = change
                .edits
                .into_iter()
                .map(|e| match e {
                    OneOf::Left(edit) => Ok(edit),
                    OneOf::Right(_) => Err(
                        "This operation requires annotated edits that Newport cannot apply yet."
                            .to_owned(),
                    ),
                })
                .collect::<Result<_, _>>()?;
            files.push(FileEdits {
                path: uri_path(&change.text_document.uri)
                    .ok_or("This operation contains a non-file location.")?,
                version: change.text_document.version,
                edits,
            });
        }
    } else if let Some(changes) = edit.changes {
        for (uri, edits) in changes {
            files.push(FileEdits {
                path: uri_path(&uri).ok_or("This operation contains a non-file location.")?,
                version: None,
                edits,
            });
        }
    }
    let mut seen = HashSet::new();
    for file in &files {
        if !within_root(&file.path, root) {
            return Err("This operation changes a file outside the worktree.".into());
        }
        if !seen.insert(file.path.clone()) {
            return Err("The server returned repeated edits for one document.".into());
        }
    }
    files.retain(|f| !f.edits.is_empty());
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}
pub(super) fn within_root(path: &str, root: &str) -> bool {
    path.starts_with(&format!("{}/", root.trim_end_matches('/')))
        && !path.split('/').any(|p| matches!(p, "." | ".." | ".git"))
        && !path.chars().any(char::is_control)
}

impl CodeWorkspace {
    fn language_snapshot(&self, tool: Tool, cx: &App) -> Option<Snapshot> {
        if !self.available || self.reviewing {
            return None;
        }
        let buffer = self.active.and_then(|i| self.buffers.get(i))?;
        let attached = buffer.lsp.as_ref()?;
        if buffer.saving
            || buffer.conflict.is_some()
            || !tool.supported(&attached.server.capabilities_for(&attached.document))
        {
            return None;
        }
        let connection = attached.server.connection()?;
        if connection.is_closed() {
            return None;
        }
        // A rename can refer to another dirty tab. Synchronize every document first.
        for buffer in &self.buffers {
            if let Some(a) = &buffer.lsp {
                a.server.flush(&a.document, buffer.editor.read(cx).text());
            }
        }
        let editor = buffer.editor.read(cx);
        Some(Snapshot {
            path: buffer.path.clone(),
            server: attached.server.clone(),
            document: attached.document.clone(),
            text: editor.text().clone(),
            cursor: editor.cursor(),
            selection: editor.selected_range(),
            buffers: self
                .buffers
                .iter()
                .filter(|b| b.page.is_none() && b.failure.is_none())
                .map(|b| {
                    (
                        b.path.clone(),
                        b.editor.clone(),
                        b.editor.read(cx).text().clone(),
                        b.lsp.as_ref().map(|a| a.document.version.get()),
                    )
                })
                .collect(),
        })
    }
    pub(super) fn language_message(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.language_tools.busy = false;
        self.language_tools.panel = Some(Panel::Message(message.into()));
        cx.notify();
    }
    fn language_request<R: request::Request>(
        &mut self,
        snapshot: Snapshot,
        params: R::Params,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Snapshot, R::Result, &mut Window, &mut Context<Self>) + 'static,
    ) where
        R::Result: Send + 'static,
    {
        let Some(connection) = snapshot.server.connection() else {
            return;
        };
        self.language_tools.generation += 1;
        let generation = self.language_tools.generation;
        self.language_tools.busy = true;
        self.language_tools.panel = Some(Panel::Message("Asking the language server…".into()));
        let response = connection.request::<R>(params);
        // Dropping the request on timeout sends $/cancelRequest.
        let timer = cx.background_executor().timer(Duration::from_secs(30));
        let request = cx.background_executor().spawn(async move {
            futures_lite::future::race(response, async move {
                timer.await;
                Err("The language server took too long. Try again.".to_owned())
            })
            .await
        });
        self.language_tools.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.language_tools.generation != generation {
                    return;
                }
                if !snapshot.valid(this, cx) {
                    this.language_message(
                        "The files changed while the request was running. Try again.",
                        cx,
                    );
                    return;
                }
                this.language_tools.busy = false;
                match result {
                    Ok(result) => done(this, snapshot, result, window, cx),
                    Err(error) => this.language_message(error, cx),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub(in super::super) fn run_language_shortcut(
        &mut self,
        tool: Tool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .active
            .and_then(|i| self.buffers.get(i))
            .is_some_and(|b| b.editor.read(cx).focus_handle(cx).is_focused(window))
        {
            self.run_language_tool(tool, window, cx);
        }
    }

    pub(in super::super) fn run_language_tool(
        &mut self,
        tool: Tool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) || self.language_tools.busy {
            return;
        }
        let Some(snapshot) = self.language_snapshot(tool, cx) else {
            return;
        };
        match tool {
            Tool::Actions | Tool::OrganizeImports => {
                self.request_code_actions(snapshot, tool == Tool::OrganizeImports, window, cx)
            }
            Tool::Format => {
                let editor = self.buffers[self.active.unwrap()].editor.read(cx);
                let hard_tabs = editor
                    .text()
                    .to_string()
                    .lines()
                    .any(|line| line.starts_with('\t'));
                let params = lsp_types::DocumentFormattingParams {
                    text_document: lsp_types::TextDocumentIdentifier::new(
                        snapshot.document.uri.clone(),
                    ),
                    options: lsp_types::FormattingOptions {
                        tab_size: 4,
                        insert_spaces: !hard_tabs,
                        ..Default::default()
                    },
                    work_done_progress_params: Default::default(),
                };
                self.language_request::<request::Formatting>(
                    snapshot,
                    params,
                    window,
                    cx,
                    |this, snapshot, edits, window, cx| {
                        let file = FileEdits {
                            path: snapshot.path.clone(),
                            version: Some(snapshot.document.version.get()),
                            edits: edits.unwrap_or_default(),
                        };
                        this.stage_language_edits(snapshot, vec![file], window, cx);
                    },
                );
            }
            Tool::References => {
                let params = lsp_types::ReferenceParams {
                    text_document_position: snapshot.position(),
                    context: lsp_types::ReferenceContext {
                        include_declaration: true,
                    },
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };
                self.language_request::<request::References>(
                    snapshot,
                    params,
                    window,
                    cx,
                    |this, snapshot, locations, _, cx| {
                        let mut locations = locations.unwrap_or_default();
                        locations.sort_by(|a, b| {
                            a.uri.as_str().cmp(b.uri.as_str()).then_with(|| {
                                (a.range.start.line, a.range.start.character)
                                    .cmp(&(b.range.start.line, b.range.start.character))
                            })
                        });
                        locations.dedup();
                        let total = locations.len();
                        locations.truncate(500);
                        this.language_tools.panel = Some(Panel::References {
                            source: snapshot.path,
                            server: snapshot.server.id,
                            locations,
                            total,
                        });
                        cx.notify();
                    },
                );
            }
            Tool::Signature => {
                let params = lsp_types::SignatureHelpParams {
                    text_document_position_params: snapshot.position(),
                    work_done_progress_params: Default::default(),
                    context: None,
                };
                self.language_request::<request::SignatureHelpRequest>(
                    snapshot,
                    params,
                    window,
                    cx,
                    |this, _, help, _, cx| {
                        let Some(help) = help else {
                            this.language_message("No signature at this position.", cx);
                            return;
                        };
                        let signature = help
                            .signatures
                            .get(help.active_signature.unwrap_or(0) as usize)
                            .or_else(|| help.signatures.first());
                        let Some(signature) = signature else {
                            this.language_message("No signature at this position.", cx);
                            return;
                        };
                        let documentation = signature.documentation.as_ref().map(|d| match d {
                            lsp_types::Documentation::String(s) => s.clone(),
                            lsp_types::Documentation::MarkupContent(m) => m.value.clone(),
                        });
                        this.language_tools.panel = Some(Panel::Signature {
                            label: signature.label.clone(),
                            documentation,
                        });
                        cx.notify();
                    },
                );
            }
            Tool::Rename => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("New symbol name"));
                self.language_tools.rename_subscription =
                    Some(
                        cx.subscribe_in(&input, window, |this, _, event, window, cx| {
                            if matches!(event, InputEvent::PressEnter { .. }) {
                                this.submit_language_rename(window, cx);
                            }
                        }),
                    );
                self.language_tools.panel = Some(Panel::Rename {
                    snapshot,
                    input: input.clone(),
                });
                crate::controls::focus_editor(&input, window, cx);
                cx.notify();
            }
        }
    }
    fn submit_language_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Panel::Rename { snapshot, input }) = &self.language_tools.panel else {
            return;
        };
        let name = input.read(cx).value().to_string();
        if name.trim().is_empty() || name.chars().any(char::is_control) {
            return;
        }
        let snapshot = snapshot.clone();
        if !snapshot.valid(self, cx) {
            self.language_message("The files changed. Start Rename again.", cx);
            return;
        }
        let params = lsp_types::RenameParams {
            text_document_position: snapshot.position(),
            new_name: name,
            work_done_progress_params: Default::default(),
        };
        self.language_request::<request::Rename>(
            snapshot,
            params,
            window,
            cx,
            |this, snapshot, edit, window, cx| {
                let files = edit
                    .map(|edit| workspace_edits(edit, &this.root))
                    .unwrap_or_else(|| Ok(Vec::new()));
                match files {
                    Ok(files) => this.stage_language_edits(snapshot, files, window, cx),
                    Err(error) => this.language_message(error, cx),
                }
            },
        );
    }
    fn stage_language_edits(
        &mut self,
        snapshot: Snapshot,
        files: Vec<FileEdits>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stage_language_edits_then(snapshot, files, None, window, cx);
    }
    fn stage_language_edits_then(
        &mut self,
        snapshot: Snapshot,
        files: Vec<FileEdits>,
        done: Option<EditDone>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let failure = if files.iter().any(|file| self.opening.contains(&file.path)) {
            Some("An affected file is still opening. Wait for it to load, then try again.")
        } else if self.buffers.len()
            + self.opening.len()
            + files
                .iter()
                .filter(|f| !self.buffers.iter().any(|b| b.path == f.path))
                .count()
            > 20
        {
            Some("These edits need more than 20 open tabs. Close other tabs and try again.")
        } else {
            None
        };
        if let Some(error) = failure {
            self.finish_language_edits(done, Err(error.into()), window, cx);
            return;
        }
        if files.iter().all(|f| f.edits.is_empty()) {
            self.language_message("No changes needed.", cx);
            self.finish_language_edits(done, Ok(()), window, cx);
            return;
        }
        self.language_tools.busy = true;
        self.load_language_edits(
            PendingEdits {
                snapshot,
                files,
                loaded: HashMap::new(),
                generation: self.language_tools.generation,
                done,
            },
            window,
            cx,
        );
    }
    fn finish_language_edits(
        &mut self,
        done: Option<EditDone>,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = &result {
            self.language_message(error.clone(), cx);
        }
        if let Some(done) = done {
            done(self, result, window, cx);
        }
    }
    fn load_language_edits(
        &mut self,
        mut pending: PendingEdits,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if pending.generation != self.language_tools.generation {
            return;
        }
        if !pending.snapshot.valid(self, cx) {
            self.finish_language_edits(
                pending.done.take(),
                Err("The files changed. No edits were applied; try again.".into()),
                window,
                cx,
            );
            return;
        }
        if let Some(file) = pending.files.iter().find(|f| {
            !pending
                .snapshot
                .buffers
                .iter()
                .any(|(p, _, _, _)| p == &f.path)
                && !pending.loaded.contains_key(&f.path)
        }) {
            if file.version.is_some() {
                self.finish_language_edits(pending.done.take(), Err("The server requested a version of a file that is not open. Open it and try again.".into()), window, cx);
                return;
            }
            let path = file.path.clone();
            let mut args = self.args(&path);
            args["root"] = json!(self.root);
            self.rpc(
                "files_read_text",
                args,
                window,
                cx,
                move |this, result, window, cx| {
                    if pending.generation != this.language_tools.generation {
                        return;
                    }
                    match result {
                        Ok(value) => {
                            if value["path"].as_str() != Some(&path) {
                                this.finish_language_edits(pending.done.take(), Err("The server returned a different file. No edits were applied.".into()), window, cx);
                                return;
                            }
                            let mut pending = pending;
                            pending.loaded.insert(path, value);
                            this.load_language_edits(pending, window, cx);
                        }
                        Err(error) => {
                            this.finish_language_edits(pending.done.take(), Err(format!("No edits were applied: {error}")), window, cx)
                        }
                    }
                },
            );
            return;
        }
        let done = pending.done.take();
        let result = self.apply_language_edits(pending, window, cx);
        self.finish_language_edits(done, result, window, cx);
    }
    fn apply_language_edits(
        &mut self,
        pending: PendingEdits,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !pending.snapshot.valid(self, cx) {
            return Err("The files changed. No edits were applied.".into());
        }
        if pending
            .files
            .iter()
            .any(|file| self.opening.contains(&file.path))
        {
            return Err("An affected file is still opening. No edits were applied; try again after it loads.".into());
        }
        let mut new_buffers = Vec::new();
        let mut prepared = Vec::new();
        let mut retained: usize = self
            .buffers
            .iter()
            .map(|b| b.editor.read(cx).text().len())
            .sum();
        for file in pending.files {
            let existing = self.buffers.iter().find(|b| b.path == file.path);
            let buffer = if let Some(buffer) = existing {
                let captured = pending
                    .snapshot
                    .buffers
                    .iter()
                    .find(|(p, _, _, _)| p == &file.path)
                    .ok_or("A target file was opened while the request was running. Try again.")?;
                if file.version.is_some() && file.version != captured.3 {
                    return Err("The server returned edits for an older document version.".into());
                }
                buffer
            } else {
                let value = pending
                    .loaded
                    .get(&file.path)
                    .ok_or("An edit target could not be loaded.")?
                    .clone();
                new_buffers.push(Self::document_buffer(file.path.clone(), value, window, cx)?);
                new_buffers.last().unwrap()
            };
            if buffer.page.is_some()
                || buffer.failure.is_some()
                || buffer.conflict.is_some()
                || buffer.saving
            {
                return Err(
                    "An edit target cannot be changed right now. No edits were applied.".into(),
                );
            }
            let editor = buffer.editor.read(cx);
            let changes = edits::validate(
                pending.snapshot.server.encoding.get(),
                editor.text(),
                file.edits,
            )?;
            let after = edits::apply(editor.text(), &changes)?;
            retained = retained
                .saturating_sub(if existing.is_some() {
                    editor.text().len()
                } else {
                    0
                })
                .saturating_add(after.len());
            if retained > 64 * 1024 * 1024 {
                return Err("These edits exceed the 64 MiB project buffer limit.".into());
            }
            let caret = edits::map_offset(editor.cursor(), &changes);
            prepared.push((file.path, buffer.editor.clone(), after, caret));
        }
        if self.buffers.len() + self.opening.len() + new_buffers.len() > 20 {
            return Err("Close some editor tabs and try again.".into());
        }
        // All validation completes before the first buffer is changed.
        for buffer in new_buffers {
            self.place_buffer(buffer, window, cx);
        }
        let count = prepared.len();
        for (path, editor, after, caret) in prepared {
            editor.update(cx, |e, cx| {
                let scroll = e.scroll_offset();
                e.replace_all(after, window, cx);
                e.set_selected_range(caret..caret, cx);
                e.set_scroll_offset(scroll, cx);
            });
            self.schedule_language_sync(&path, cx);
        }
        self.language_message(format!("Updated {count} file{}. Review the unsaved tabs, then save. Undo is available in each file.", if count == 1 { "" } else { "s" }), cx);
        Ok(())
    }
    pub(in super::super) fn language_tools_button(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let owner = cx.entity().downgrade();
        let capabilities = self
            .active
            .and_then(|i| self.buffers.get(i))
            .filter(|b| {
                b.failure.is_none() && b.conflict.is_none() && !b.saving && b.page.is_none()
            })
            .and_then(|b| b.lsp.as_ref())
            .filter(|a| a.server.ready.get() && a.server.connection().is_some())
            .map(|a| a.server.capabilities_for(&a.document));
        let enabled = self.available
            && !self.reviewing
            && !self.language_tools.busy
            && capabilities.is_some();
        crate::controls::compact_button("code-language-tools")
            .ghost()
            .label("Code")
            .accessibility_label("Language tools")
            .tooltip("Language tools")
            .disabled(!enabled)
            .dropdown_menu(move |mut menu, _, _| {
                for tool in [
                    Tool::Actions,
                    Tool::OrganizeImports,
                    Tool::References,
                    Tool::Rename,
                    Tool::Format,
                    Tool::Signature,
                ] {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(tool.label())
                            .disabled(!capabilities.as_ref().is_some_and(|c| tool.supported(c)))
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.run_language_tool(tool, window, cx)
                                });
                            }),
                    );
                }
                let owner = owner.clone();
                menu.item(PopupMenuItem::new("Restart language server").on_click(
                    move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if let Some(attached) = this
                                .active
                                .and_then(|i| this.buffers.get(i))
                                .and_then(|b| b.lsp.as_ref())
                            {
                                let (id, envrc) = (attached.server.id, attached.server.envrc);
                                this.restart_language_server(id, envrc, window, cx);
                            }
                        });
                    },
                ))
            })
    }
    pub(in super::super) fn language_tools_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let panel = self.language_tools.panel.as_ref()?;
        let title = match panel {
            Panel::Actions { .. } => "Code actions".to_owned(),
            Panel::Message(_) => "Language tools".to_owned(),
            Panel::References {
                total, locations, ..
            } if *total > locations.len() => {
                format!("References · showing {} of {total}", locations.len())
            }
            Panel::References { total, .. } => format!("References · {total}"),
            Panel::Signature { .. } => "Signature help".to_owned(),
            Panel::Rename { .. } => "Rename symbol".to_owned(),
        };
        let body = match panel {
            Panel::Actions { items, .. } => self.code_actions_list(items, cx),
            Panel::Message(message) => div().child(message.clone()),
            Panel::Signature {
                label,
                documentation,
            } => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(label.clone()),
                )
                .children(documentation.clone()),
            Panel::Rename { input, .. } => div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(crate::controls::input(input)),
                )
                .child(
                    crate::controls::compact_button("code-rename-apply")
                        .label("Rename")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.submit_language_rename(window, cx)
                        })),
                ),
            Panel::References {
                source,
                server,
                locations,
                ..
            } => {
                let server = *server;
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.relative_path(source)),
                    )
                    .when(locations.is_empty(), |v| v.child("No references found."))
                    .children(locations.iter().enumerate().map(|(i, location)| {
                        let path = uri_path(&location.uri);
                        let allowed = path.as_ref().is_some_and(|p| within_root(p, &self.root));
                        let range = location.range;
                        let label = format!(
                            "{}:{}:{}",
                            path.as_ref()
                                .map(|p| self.relative_path(p))
                                .unwrap_or_else(|| location.uri.as_str().into()),
                            range.start.line + 1,
                            range.start.character + 1
                        );
                        crate::controls::compact_button(("code-reference", i))
                            .ghost()
                            .label(label)
                            .disabled(!allowed)
                            .tooltip(if allowed {
                                "Open reference"
                            } else {
                                "This location is outside the worktree"
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(path) = &path {
                                    this.go_to_location(path.clone(), range, server, window, cx);
                                }
                            }))
                    }))
            }
        };
        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .border_b_1()
                .border_color(cx.theme().border)
                .text_size(px(crate::ui::FONT_SIZE))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .px_3()
                        .py_1()
                        .gap_2()
                        .child(div().flex_1().child(title))
                        .child(
                            crate::controls::compact_button("code-language-dismiss")
                                .ghost()
                                .label(if self.language_tools.busy {
                                    "Cancel"
                                } else {
                                    "Close"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.language_tools.generation += 1;
                                    this.language_tools.task = None;
                                    this.language_tools.command = None;
                                    this.language_tools.busy = false;
                                    this.language_tools.panel = None;
                                    this.language_tools.rename_subscription = None;
                                    this.focus_document(window, cx);
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .id("code-language-results")
                        .max_h(px(200.))
                        .overflow_y_scroll()
                        .px_3()
                        .pb_2()
                        .child(body),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::workspace_edits;
    use serde_json::json;
    #[test]
    fn workspace_changes_reject_external_files_and_resource_operations() {
        let parse = |value| serde_json::from_value::<lsp_types::WorkspaceEdit>(value).unwrap();
        assert!(
            workspace_edits(parse(json!({"changes":{"file:///other/a.rs":[]}})), "/repo").is_err()
        );
        assert!(
            workspace_edits(
                parse(json!({"changes":{"file:///repo/../a.rs":[]}})),
                "/repo"
            )
            .is_err()
        );
        assert!(
            workspace_edits(
                parse(json!({"documentChanges":[{"kind":"delete","uri":"file:///repo/a.rs"}]})),
                "/repo"
            )
            .is_err()
        );
        let files=workspace_edits(parse(json!({"documentChanges":[{"textDocument":{"uri":"file:///repo/a.rs","version":3},"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":"b"}]}]})), "/repo").unwrap();
        assert_eq!(files[0].version, Some(3));
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::super::{Server, Status, test_support};
    use super::{FileEdits, Panel, PendingEdits, Snapshot, Tool};
    use crate::project_editor::{
        CodeWorkspace,
        interaction_tests::{draw, fixture, open},
    };
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use serde_json::json;
    use std::{collections::HashMap, rc::Rc};

    pub(super) fn connect(
        view: &Entity<CodeWorkspace>,
        cx: &mut VisualTestContext,
    ) -> (
        std::io::PipeWriter,
        std::sync::mpsc::Receiver<serde_json::Value>,
    ) {
        let (writer, messages, _) = connect_events(view, cx);
        (writer, messages)
    }
    pub(super) fn connect_events(
        view: &Entity<CodeWorkspace>,
        cx: &mut VisualTestContext,
    ) -> (
        std::io::PipeWriter,
        std::sync::mpsc::Receiver<serde_json::Value>,
        async_channel::Receiver<newport_client::lsp::Event>,
    ) {
        let (connection, writer, messages, events) = test_support::connection_events();
        view.update_in(cx, |view, window, cx| {
            let server = Rc::new(Server::new("rust-analyzer", true));
            server.ready.set(true);
            *server.status.borrow_mut() = Status::Ready;
            *server.connection.borrow_mut() = Some(connection);
            *server.capabilities.borrow_mut() = serde_json::from_value(json!({
                "documentFormattingProvider":true,"referencesProvider":true,"renameProvider":true,
                "signatureHelpProvider":{"triggerCharacters":["(",","]}, "completionProvider":{}, "codeActionProvider":{"resolveProvider":true}, "executeCommandProvider":{"commands":["test.fix"]}
            }))
            .unwrap();
            view.language_servers.insert("rust-analyzer", server);
            view.attach_language_server("/repo/main.rs", window, cx);
        });
        (writer, messages, events)
    }
    fn rename(path: &str) -> FileEdits {
        FileEdits { path: path.into(), version: None, edits: vec![serde_json::from_value(json!({"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}},"newText":"renamed"})).unwrap()] }
    }
    fn snapshot(view: &Entity<CodeWorkspace>, cx: &VisualTestContext) -> Snapshot {
        view.read_with(cx, |view, cx| {
            view.language_snapshot(Tool::Rename, cx).unwrap()
        })
    }

    #[gpui_kit::test]
    fn rename_loads_closed_files_without_saving_and_undo_restores_the_buffer(
        cx: &mut TestAppContext,
    ) {
        let (view, cx, calls) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (_writer, _messages) = connect(&view, cx);
        let snapshot = snapshot(&view, cx);
        view.update_in(cx, |view, window, cx| {
            view.stage_language_edits(
                snapshot,
                vec![rename("/repo/main.rs"), rename("/repo/other.rs")],
                window,
                cx,
            )
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.buffers.len(), 2);
            for b in &view.buffers {
                assert_eq!(b.editor.read(cx).value().as_ref(), "fn renamed() {}\n");
                assert!(b.dirty(cx));
            }
        });
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|(method, _)| method == "files_write_text")
        );
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.buffers[0].editor.read(cx).value().as_ref(),
                "fn main() {}\n"
            )
        });
    }

    #[gpui_kit::test]
    fn an_in_flight_open_cannot_overwrite_a_renamed_buffer(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (_writer, _messages) = connect(&view, cx);
        let snapshot = snapshot(&view, cx);
        view.update_in(cx,|view,window,cx| {
            view.opening.insert("/repo/other.rs".into());
            let result=view.apply_language_edits(PendingEdits {
                snapshot, files:vec![rename("/repo/main.rs"),rename("/repo/other.rs")], done: None,
                loaded:HashMap::from([("/repo/other.rs".into(),json!({"path":"/repo/other.rs","content":"fn main() {}\n","revision":"original"}))]),
                generation:view.language_tools.generation,
            },window,cx);
            assert!(result.unwrap_err().contains("still opening"));
            assert_eq!(view.buffers.len(),1);
            assert!(!view.buffers[0].dirty(cx));
            // The normal open response can now land without replacing staged edits.
            view.opening.remove("/repo/other.rs");
            let buffer=CodeWorkspace::document_buffer("/repo/other.rs".into(),json!({"content":"fn main() {}\n","revision":"original"}),window,cx).unwrap();
            view.place_buffer(buffer,window,cx);
            assert!(view.buffers.iter().all(|buffer| !buffer.dirty(cx)));
        });
    }

    #[gpui_kit::test]
    fn a_bad_second_file_cannot_partially_apply_a_workspace_edit(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (_writer, _messages) = connect(&view, cx);
        let snapshot = snapshot(&view, cx);
        let mut bad = rename("/repo/other.rs");
        bad.edits[0].range.end.character = 999;
        view.update_in(cx, |view, window, cx| {
            view.stage_language_edits(snapshot, vec![rename("/repo/main.rs"), bad], window, cx)
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.buffers.len(), 1);
            assert_eq!(
                view.buffers[0].editor.read(cx).value().as_ref(),
                "fn main() {}\n"
            );
            assert!(matches!(view.language_tools.panel, Some(Panel::Message(_))));
        });
    }

    #[gpui_kit::test]
    fn changes_after_a_request_reject_its_edits(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (_writer, _messages) = connect(&view, cx);
        let snapshot = snapshot(&view, cx);
        view.update_in(cx, |view, window, cx| {
            view.buffers[0]
                .editor
                .update(cx, |e, cx| e.replace_all("my new content", window, cx));
            let result = view.apply_language_edits(
                PendingEdits {
                    snapshot,
                    files: vec![rename("/repo/main.rs")],
                    loaded: HashMap::new(),
                    generation: view.language_tools.generation,
                    done: None,
                },
                window,
                cx,
            );
            assert!(result.is_err());
        });
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.buffers[0].editor.read(cx).value().as_ref(),
                "my new content"
            )
        });
    }

    #[gpui_kit::test]
    fn language_tools_have_visible_actions_and_a_dismissible_result_panel(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (_writer, _messages) = connect(&view, cx);
        view.update(cx, |view, cx| {
            view.language_message("No references found.", cx)
        });
        draw(cx);
        cx.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            assert!(nodes.iter().any(|n| n.label() == Some("Language tools")));
            assert!(nodes.iter().any(|n| n.label() == Some("Close")));
        });
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::super::{DefinitionProvider, Hooks, HoverProvider};
    use super::interaction_tests::connect;
    use super::{Panel, Tool};
    use crate::project_editor::{
        CodeWorkspace,
        interaction_tests::{draw, fixture, open},
    };
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use serde_json::{Value, json};
    use std::{io::Write, time::Duration};

    pub(super) fn send(writer: &mut std::io::PipeWriter, message: Value) {
        let body = serde_json::to_vec(&message).unwrap();
        let mut message = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        message.extend_from_slice(&body);
        let mut frame = vec![newport_platform::ipc::protocol::REMOTE_STREAM_DATA];
        frame.extend_from_slice(&(message.len() as u32).to_be_bytes());
        frame.extend_from_slice(&message);
        writer.write_all(&frame).unwrap();
    }
    pub(super) fn respond(
        writer: &mut std::io::PipeWriter,
        messages: &std::sync::mpsc::Receiver<Value>,
        method: &str,
        result: Value,
    ) -> Value {
        let sent = request(messages, method);
        send(
            writer,
            json!({"jsonrpc":"2.0","id":sent["id"],"result":result}),
        );
        // The reader processes this barrier after the reply. Wait for its answer
        // inside update_in so no GPUI future registers a cross-thread waker.
        send(
            writer,
            json!({"jsonrpc":"2.0","id":"barrier","method":"workspace/workspaceFolders","params":null}),
        );
        loop {
            if messages.recv_timeout(Duration::from_secs(2)).unwrap()["id"] == "barrier" {
                return sent;
            }
        }
    }
    pub(super) fn settle(view: &Entity<CodeWorkspace>, cx: &mut VisualTestContext) {
        for _ in 0..1000 {
            draw(cx);
            if view.read_with(cx, |view, _| !view.language_tools.busy) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("language tool did not finish");
    }
    pub(super) fn request(messages: &std::sync::mpsc::Receiver<Value>, method: &str) -> Value {
        loop {
            let message = messages.recv_timeout(Duration::from_secs(2)).unwrap();
            if message["method"] == method {
                return message;
            }
        }
    }

    #[gpui_kit::test]
    fn hover_and_definitions_honor_dynamic_selectors_and_unregistration(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (mut writer, messages) = connect(&view, cx);
        view.update_in(cx, |v, window, cx| {
            let attached = v.buffers[0].lsp.as_ref().unwrap();
            let hooks = Hooks { server: attached.server.clone(), document: attached.document.clone() };
            let text = v.buffers[0].editor.read(cx).text().clone();
            hooks.server.capabilities.borrow_mut().hover_provider = Some(lsp_types::HoverProviderCapability::Simple(false));
            hooks.server.capabilities.borrow_mut().definition_provider = Some(lsp_types::OneOf::Left(false));
            let connection = hooks.server.connection().unwrap();
            for language in [None, Some("python"), Some("rust"), None] {
                if let Some(language) = language {
                    hooks.server.protocol.borrow_mut().register(json!({"registrations":[
                        {"id":"hover","method":"textDocument/hover","registerOptions":{"documentSelector":[{"language":language}]}},
                        {"id":"definition","method":"textDocument/definition","registerOptions":{"documentSelector":[{"language":language}]}}
                    ]}), "/repo").unwrap();
                }
                let hover = hooks.hover(&text, 3, window, cx);
                if language == Some("rust") {
                    respond(&mut writer, &messages, "textDocument/hover", Value::Null);
                }
                let definitions = hooks.definitions(&text, 3, window, cx);
                if language == Some("rust") {
                    respond(&mut writer, &messages, "textDocument/definition", Value::Null);
                }
                connection.notify_value("$/testProbe", Value::Null);
                loop {
                    let message = messages.recv_timeout(Duration::from_secs(2)).unwrap();
                    assert_ne!(message["method"], "textDocument/hover");
                    assert_ne!(message["method"], "textDocument/definition");
                    if message["method"] == "$/testProbe" { break; }
                }
                hover.detach();
                definitions.detach();
                hooks.server.protocol.borrow_mut().unregister(json!({"unregisterations":[
                    {"id":"hover","method":"textDocument/hover"},
                    {"id":"definition","method":"textDocument/definition"}
                ]})).unwrap();
            }
        });
        draw(cx);
    }

    #[gpui_kit::test]
    fn formatting_sends_options_and_stages_the_response(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (mut writer, messages) = connect(&view, cx);
        view.update_in(cx, |v, window, cx| {
            v.run_language_tool(Tool::Format, window, cx);
            let sent = respond(
                &mut writer, &messages, "textDocument/formatting",
                json!([{"range":{"start":{"line":0,"character":10},"end":{"line":0,"character":12}},"newText":"{\n}\n"}]),
            );
            assert_eq!(sent["params"]["options"]["tabSize"], 4);
        });
        settle(&view, cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(
                v.buffers[0].editor.read(cx).value().as_ref(),
                "fn main() {\n}\n\n"
            )
        });
    }

    #[gpui_kit::test]
    fn references_and_signature_help_round_trip_to_the_result_panel(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (mut writer, messages) = connect(&view, cx);
        view.update_in(cx, |v, window, cx| {
            v.run_language_tool(Tool::References, window, cx);
            let sent = respond(
                &mut writer, &messages, "textDocument/references",
                json!([{"uri":"file:///repo/main.rs","range":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}}}]),
            );
            assert_eq!(sent["params"]["context"]["includeDeclaration"], true);
        });
        settle(&view, cx);
        view.read_with(cx, |v, _| {
            assert!(matches!(
                &v.language_tools.panel,
                Some(Panel::References { total: 1, .. })
            ))
        });
        view.update_in(cx, |v, window, cx| {
            v.run_language_tool(Tool::Signature, window, cx);
            respond(
                &mut writer,
                &messages,
                "textDocument/signatureHelp",
                json!({"signatures":[{"label":"main()","documentation":"Entry point"}]}),
            );
        });
        settle(&view, cx);
        view.read_with(cx, |v, _| match &v.language_tools.panel {
            Some(Panel::Signature {
                label,
                documentation,
            }) => {
                assert_eq!(label, "main()");
                assert_eq!(documentation.as_deref(), Some("Entry point"));
            }
            _ => panic!("signature was not shown"),
        });
    }
}
