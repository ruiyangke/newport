//! Code-action resolution and command-scoped server edits.
use super::*;
use lsp_types::{CodeAction, CodeActionOrCommand, Command};

pub(super) struct CommandState {
    server: Rc<Server>,
    snapshot: Snapshot,
    applying: bool,
    finished: Option<Result<(), String>>,
    edit_failure: Option<String>,
}
impl Snapshot {
    fn refresh(&self, workspace: &CodeWorkspace, cx: &App) -> Option<Self> {
        let buffer = workspace.buffers.iter().find(|b| b.path == self.path)?;
        let attached = buffer.lsp.as_ref()?;
        if !Rc::ptr_eq(&attached.server, &self.server) {
            return None;
        }
        for buffer in &workspace.buffers {
            if let Some(a) = &buffer.lsp {
                a.server.flush(&a.document, buffer.editor.read(cx).text());
            }
        }
        let editor = buffer.editor.read(cx);
        Some(Self {
            path: self.path.clone(),
            server: self.server.clone(),
            document: attached.document.clone(),
            text: editor.text().clone(),
            cursor: editor.cursor(),
            selection: editor.selected_range(),
            buffers: workspace
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
}
fn action_title(action: &CodeActionOrCommand) -> &str {
    match action {
        CodeActionOrCommand::CodeAction(a) => &a.title,
        CodeActionOrCommand::Command(c) => &c.title,
    }
}
fn disabled(action: &CodeActionOrCommand) -> Option<&str> {
    match action {
        CodeActionOrCommand::CodeAction(a) => a.disabled.as_ref().map(|d| d.reason.as_str()),
        _ => None,
    }
}
impl CodeWorkspace {
    pub(super) fn request_code_actions(
        &mut self,
        snapshot: Snapshot,
        imports: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let encoding = snapshot.server.encoding.get();
        let range = lsp_types::Range::new(
            to_server(encoding, &snapshot.text, snapshot.selection.start),
            to_server(encoding, &snapshot.text, snapshot.selection.end),
        );
        let diagnostics = snapshot
            .server
            .diagnostics
            .borrow()
            .get(&snapshot.path)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|d| {
                (d.range.start.line, d.range.start.character)
                    <= (range.end.line, range.end.character)
                    && (d.range.end.line, d.range.end.character)
                        >= (range.start.line, range.start.character)
            })
            .collect();
        let params = lsp_types::CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier::new(snapshot.document.uri.clone()),
            range,
            context: lsp_types::CodeActionContext {
                diagnostics,
                only: imports.then(|| vec![lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS]),
                trigger_kind: Some(lsp_types::CodeActionTriggerKind::INVOKED),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        self.language_request::<request::CodeActionRequest>(snapshot, params, window, cx, |this, snapshot, items, _, cx| {
            let mut items = items.unwrap_or_default();
            items.sort_by_key(|item| (disabled(item).is_some(), !matches!(item, CodeActionOrCommand::CodeAction(a) if a.is_preferred == Some(true))));
            if items.is_empty() { this.language_message("No code actions at this position.", cx); }
            else { this.language_tools.panel = Some(Panel::Actions { snapshot, items }); cx.notify(); }
        });
    }
    pub(super) fn code_actions_list(
        &self,
        items: &[CodeActionOrCommand],
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(items.iter().enumerate().map(|(index, item)| {
                crate::controls::compact_button(("code-action", index))
                    .ghost()
                    .label(action_title(item).to_owned())
                    .disabled(disabled(item).is_some())
                    .tooltip(disabled(item).unwrap_or("Apply code action").to_owned())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_code_action(index, window, cx)
                    }))
            }))
    }
    pub(super) fn choose_code_action(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Panel::Actions { snapshot, items }) = &self.language_tools.panel else {
            return;
        };
        let Some(item) = items.get(index).filter(|i| disabled(i).is_none()).cloned() else {
            return;
        };
        let snapshot = snapshot.clone();
        if !snapshot.valid(self, cx) {
            self.language_message("The files changed. Request code actions again.", cx);
            return;
        }
        match item {
            CodeActionOrCommand::Command(command) => {
                self.execute_language_command(snapshot, command, window, cx)
            }
            CodeActionOrCommand::CodeAction(action) => {
                let resolve = matches!(snapshot.server.capabilities_for(&snapshot.document).code_action_provider,
                    Some(lsp_types::CodeActionProviderCapability::Options(options)) if options.resolve_provider == Some(true));
                if resolve {
                    self.language_request::<request::CodeActionResolveRequest>(
                        snapshot,
                        action,
                        window,
                        cx,
                        |this, snapshot, action, window, cx| {
                            this.apply_code_action(snapshot, action, window, cx)
                        },
                    );
                } else {
                    self.apply_code_action(snapshot, action, window, cx);
                }
            }
        }
    }
    fn apply_code_action(
        &mut self,
        snapshot: Snapshot,
        action: CodeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(disabled) = action.disabled {
            self.language_message(disabled.reason, cx);
            return;
        }
        if !snapshot.valid(self, cx) {
            self.language_message("The files changed. Request code actions again.", cx);
            return;
        }
        let command = action.command;
        if let Some(edit) = action.edit {
            let files = match workspace_edits(edit, &self.root) {
                Ok(files) => files,
                Err(error) => {
                    self.language_message(error, cx);
                    return;
                }
            };
            let source = snapshot.clone();
            self.stage_language_edits_then(
                snapshot,
                files,
                Some(Box::new(move |this, result, window, cx| {
                    if result.is_ok()
                        && let Some(command) = command
                        && let Some(snapshot) = source.refresh(this, cx)
                    {
                        this.execute_language_command(snapshot, command, window, cx);
                    }
                })),
                window,
                cx,
            );
        } else if let Some(command) = command {
            self.execute_language_command(snapshot, command, window, cx);
        } else {
            self.language_message("This code action returned no edits or command.", cx);
        }
    }
    fn execute_language_command(
        &mut self,
        snapshot: Snapshot,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = snapshot.server.connection() else {
            return;
        };
        self.language_tools.generation += 1;
        let generation = self.language_tools.generation;
        self.language_tools.busy = true;
        self.language_tools.panel = Some(Panel::Message(format!("Running {}…", command.title)));
        self.language_tools.command = Some(CommandState {
            server: snapshot.server.clone(),
            snapshot,
            applying: false,
            finished: None,
            edit_failure: None,
        });
        let request =
            connection.request::<request::ExecuteCommand>(lsp_types::ExecuteCommandParams {
                command: command.command,
                arguments: command.arguments.unwrap_or_default(),
                work_done_progress_params: Default::default(),
            });
        let timer = cx.background_executor().timer(Duration::from_secs(30));
        let request = cx.background_executor().spawn(async move {
            futures_lite::future::race(request, async move {
                timer.await;
                Err("The language server took too long. Try again.".into())
            })
            .await
        });
        self.language_tools.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = request.await.map(|_| ());
            let _ = this.update_in(cx, |this, _, cx| {
                if this.language_tools.generation != generation {
                    return;
                }
                if let Err(error) = result {
                    // Invalidate any file reads still waiting on this command.
                    this.language_tools.generation += 1;
                    this.language_tools.command = None;
                    this.language_message(error, cx);
                } else if let Some(command) = &mut this.language_tools.command {
                    command.finished = Some(Ok(()));
                    this.finish_language_command(cx);
                }
            });
        }));
        cx.notify();
    }
    fn finish_language_command(&mut self, cx: &mut Context<Self>) {
        if self
            .language_tools
            .command
            .as_ref()
            .is_some_and(|c| !c.applying && c.finished.is_some())
        {
            let command = self.language_tools.command.take().unwrap();
            self.language_message(command.edit_failure.map(|error| format!("The server could not apply an edit: {error}. Review any earlier unsaved changes."))
                .unwrap_or_else(|| "Code action completed. Review any unsaved changes, then save.".into()), cx);
        }
    }
    fn reject_command_edit(
        &mut self,
        request: newport_client::lsp::IncomingRequest,
        reason: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(command) = &mut self.language_tools.command {
            command.edit_failure = Some(reason.into());
        }
        request.respond(Ok(json!({"applied":false,"failureReason":reason})));
        cx.notify();
    }
    pub(in super::super) fn apply_server_edits(
        &mut self,
        server: Rc<Server>,
        request: newport_client::lsp::IncomingRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let failure = |request: newport_client::lsp::IncomingRequest, reason: &str| {
            request.respond(Ok(json!({"applied":false,"failureReason":reason})))
        };
        let Some(command) = self
            .language_tools
            .command
            .as_ref()
            .filter(|c| Rc::ptr_eq(&c.server, &server))
        else {
            failure(
                request,
                "Choose a code action before applying server edits.",
            );
            return;
        };
        if command.applying {
            self.reject_command_edit(request, "Another edit batch is still being applied.", cx);
            return;
        }
        if !command.snapshot.valid(self, cx) {
            self.reject_command_edit(
                request,
                "The files changed while the command was running.",
                cx,
            );
            return;
        }
        let params: lsp_types::ApplyWorkspaceEditParams =
            match serde_json::from_value(request.params.clone()) {
                Ok(params) => params,
                Err(error) => {
                    self.reject_command_edit(request, &error.to_string(), cx);
                    return;
                }
            };
        let files = match workspace_edits(params.edit, &self.root) {
            Ok(files) => files,
            Err(error) => {
                self.reject_command_edit(request, &error, cx);
                return;
            }
        };
        let snapshot = command.snapshot.clone();
        let source = snapshot.clone();
        self.language_tools.command.as_mut().unwrap().applying = true;
        self.stage_language_edits_then(
            snapshot,
            files,
            Some(Box::new(move |this, result, _, cx| {
                request.respond(Ok(match &result {
                    Ok(()) => json!({"applied":true}),
                    Err(error) => json!({"applied":false,"failureReason":error}),
                }));
                let refreshed = source.refresh(this, cx);
                if let Some(command) = &mut this.language_tools.command {
                    command.applying = false;
                    if let Err(error) = &result {
                        command.edit_failure = Some(error.clone());
                    }
                    if result.is_ok()
                        && let Some(snapshot) = refreshed
                    {
                        command.snapshot = snapshot;
                    }
                    this.language_tools.busy = true;
                }
                this.finish_language_command(cx);
            })),
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        interaction_tests::connect_events,
        protocol_tests::{request, respond, send, settle},
    };
    use super::{CodeActionOrCommand, Command, Panel, Tool};
    use crate::project_editor::interaction_tests::{draw, fixture, open};
    use gpui_kit::TestAppContext;
    use newport_client::lsp::Event;
    use serde_json::{Value, json};
    use std::time::Duration;
    fn edit(path: &str) -> Value {
        json!({"changes":{path:[{"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}},"newText":"fixed"}]}})
    }
    #[gpui_kit::test]
    fn deferred_code_actions_send_selection_diagnostics_and_stage_multiple_files(
        cx: &mut TestAppContext,
    ) {
        let (view, cx, calls) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (mut writer, messages, _events) = connect_events(&view, cx);
        view.update_in(cx, |v, window, cx| {
            v.buffers[0].editor.update(cx, |e, cx| e.set_selected_range(3..7, cx));
            let server = v.language_servers["rust-analyzer"].clone();
            server.diagnostics.borrow_mut().insert("/repo/main.rs".into(), vec![serde_json::from_value(json!({"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}},"message":"fix this","data":{"diagnostic":1}})).unwrap()]);
            v.run_language_tool(Tool::Actions, window, cx);
            let sent = respond(&mut writer, &messages, "textDocument/codeAction", json!([
                {"title":"Unavailable","disabled":{"reason":"Cannot fix this"}},
                {"title":"Fix symbol","kind":"quickfix","isPreferred":true,"data":{"token":9}}
            ]));
            assert_eq!(sent["params"]["range"]["start"]["character"], 3);
            assert_eq!(sent["params"]["context"]["diagnostics"][0]["data"]["diagnostic"], 1);
        });
        settle(&view, cx);
        view.update_in(cx, |v, window, cx| {
            let Some(Panel::Actions { items, .. }) = &v.language_tools.panel else {
                panic!("no actions");
            };
            assert!(
                matches!(&items[0], CodeActionOrCommand::CodeAction(a) if a.title == "Fix symbol")
            );
            v.choose_code_action(0, window, cx);
            let mut edits = edit("file:///repo/main.rs");
            edits["changes"]["file:///repo/other.rs"] =
                edits["changes"]["file:///repo/main.rs"].clone();
            let sent = respond(
                &mut writer,
                &messages,
                "codeAction/resolve",
                json!({"title":"Fix symbol","edit":edits}),
            );
            assert_eq!(sent["params"]["data"]["token"], 9);
        });
        settle(&view, cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(v.buffers.len(), 2);
            for buffer in &v.buffers {
                assert_eq!(buffer.editor.read(cx).value().as_ref(), "fn fixed() {}\n");
                assert!(buffer.dirty(cx));
            }
        });
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|(method, _)| method == "files_write_text")
        );
    }
    #[gpui_kit::test]
    fn title_only_code_actions_are_resolved_before_applying(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (mut writer, messages, _) = connect_events(&view, cx);
        view.update_in(cx, |v, window, cx| {
            v.run_language_tool(Tool::Actions, window, cx);
            respond(
                &mut writer,
                &messages,
                "textDocument/codeAction",
                json!([{"title":"Fix symbol"}]),
            );
        });
        settle(&view, cx);
        view.update_in(cx, |v, window, cx| {
            v.choose_code_action(0, window, cx);
            let sent = respond(
                &mut writer,
                &messages,
                "codeAction/resolve",
                json!({"title":"Fix symbol","edit":edit("file:///repo/main.rs")}),
            );
            assert_eq!(sent["params"]["title"], "Fix symbol");
            assert!(sent["params"].get("data").is_none());
        });
        settle(&view, cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(
                v.buffers[0].editor.read(cx).value().as_ref(),
                "fn fixed() {}\n"
            );
            assert!(v.buffers[0].dirty(cx));
        });
    }
    #[gpui_kit::test]
    fn organize_imports_requests_only_import_actions(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (mut writer, messages, _) = connect_events(&view, cx);
        view.update_in(cx, |v, window, cx| {
            v.run_language_tool(Tool::OrganizeImports, window, cx);
            let sent = respond(&mut writer, &messages, "textDocument/codeAction", json!([]));
            assert_eq!(
                sent["params"]["context"]["only"],
                json!(["source.organizeImports"])
            );
        });
        settle(&view, cx);
    }
    #[gpui_kit::test]
    fn a_command_can_apply_workspace_edits_before_its_reply_without_deadlocking(
        cx: &mut TestAppContext,
    ) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (mut writer, messages, events) = connect_events(&view, cx);
        view.update_in(cx, |v, window, cx| {
            let snapshot = v.language_snapshot(Tool::Actions, cx).unwrap();
            let server = snapshot.server.clone();
            v.execute_language_command(snapshot, Command { title:"Fix symbol".into(), command:"test.fix".into(), arguments:None }, window, cx);
            let command = request(&messages, "workspace/executeCommand");
            send(&mut writer, json!({"jsonrpc":"2.0","id":"edit","method":"workspace/applyEdit","params":{"edit":edit("file:///repo/main.rs")}}));
            let Event::Request(incoming) = events.recv_blocking().unwrap() else { panic!("missing edit"); };
            v.apply_server_edits(server, incoming, window, cx);
            loop {
                let answer = messages.recv_timeout(Duration::from_secs(2)).unwrap();
                if answer["id"] == "edit" { assert_eq!(answer["result"]["applied"], true); break; }
            }
            send(&mut writer, json!({"jsonrpc":"2.0","id":command["id"],"result":null}));
            send(&mut writer, json!({"jsonrpc":"2.0","id":"barrier","method":"workspace/workspaceFolders"}));
            loop { if messages.recv_timeout(Duration::from_secs(2)).unwrap()["id"] == "barrier" { break; } }
        });
        settle(&view, cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(
                v.buffers[0].editor.read(cx).value().as_ref(),
                "fn fixed() {}\n"
            );
            assert!(v.language_tools.command.is_none());
        });
    }
    #[gpui_kit::test]
    fn server_edits_reject_unsolicited_requests_and_intervening_typing(cx: &mut TestAppContext) {
        let (view, cx, _) = fixture(cx);
        open(&view, cx, "/repo/main.rs");
        let (mut writer, messages, events) = connect_events(&view, cx);
        view.update_in(cx, |v, window, cx| {
            let snapshot = v.language_snapshot(Tool::Actions, cx).unwrap();
            let server = snapshot.server.clone();
            send(&mut writer, json!({"jsonrpc":"2.0","id":"unsolicited","method":"workspace/applyEdit","params":{"edit":edit("file:///repo/main.rs")}}));
            let Event::Request(incoming) = events.recv_blocking().unwrap() else { panic!("no edit"); };
            v.apply_server_edits(server.clone(), incoming, window, cx);
            loop { let answer = messages.recv_timeout(Duration::from_secs(2)).unwrap(); if answer["id"] == "unsolicited" { assert_eq!(answer["result"]["applied"], false); break; } }
            v.execute_language_command(snapshot, Command { title:"Fix".into(), command:"test.fix".into(), arguments:None }, window, cx);
            let command = request(&messages, "workspace/executeCommand");
            v.buffers[0].editor.update(cx, |e, cx| e.replace_all("my new text", window, cx));
            send(&mut writer, json!({"jsonrpc":"2.0","id":"stale","method":"workspace/applyEdit","params":{"edit":edit("file:///repo/main.rs")}}));
            let Event::Request(incoming) = events.recv_blocking().unwrap() else { panic!("no edit"); };
            v.apply_server_edits(server, incoming, window, cx);
            loop { let answer = messages.recv_timeout(Duration::from_secs(2)).unwrap(); if answer["id"] == "stale" { assert_eq!(answer["result"]["applied"], false); break; } }
            send(&mut writer, json!({"jsonrpc":"2.0","id":command["id"],"result":null}));
            send(&mut writer, json!({"jsonrpc":"2.0","id":"barrier","method":"workspace/workspaceFolders"}));
            loop { if messages.recv_timeout(Duration::from_secs(2)).unwrap()["id"] == "barrier" { break; } }
        });
        draw(cx);
        view.read_with(cx, |v, cx| {
            assert_eq!(v.buffers[0].editor.read(cx).value().as_ref(), "my new text");
            assert!(matches!(&v.language_tools.panel, Some(Panel::Message(message)) if message.contains("files changed")));
        });
    }
}
