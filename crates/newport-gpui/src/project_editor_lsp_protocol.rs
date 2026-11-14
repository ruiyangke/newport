//! Negotiated capabilities and subscriptions are scoped to a server/worktree.
use super::*;
use lsp_types::{Registration, RegistrationParams, ServerCapabilities, UnregistrationParams};
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct State {
    registrations: Vec<Registration>,
    pub initializing: bool,
    pub watching: bool,
}
fn property(method: &str) -> Option<&'static str> {
    Some(match method {
        "textDocument/completion" => "completionProvider",
        "textDocument/hover" => "hoverProvider",
        "textDocument/definition" => "definitionProvider",
        "textDocument/references" => "referencesProvider",
        "textDocument/rename" => "renameProvider",
        "textDocument/formatting" => "documentFormattingProvider",
        "textDocument/signatureHelp" => "signatureHelpProvider",
        "textDocument/codeAction" => "codeActionProvider",
        "workspace/executeCommand" => "executeCommandProvider",
        _ => return None,
    })
}
fn glob(pattern: &str) -> Result<globset::GlobMatcher, String> {
    globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| format!("Invalid file pattern: {e}"))
}
fn options(registration: &Registration) -> Value {
    registration
        .register_options
        .clone()
        .unwrap_or_else(|| json!({}))
}
fn selector_matches(options: &Value, document: &Document) -> bool {
    let Some(filters) = options["documentSelector"].as_array() else {
        return true;
    };
    let path = uri_path(&document.uri).unwrap_or_default();
    filters.iter().any(|f| {
        f["language"]
            .as_str()
            .is_none_or(|l| l == document.language)
            && f["scheme"].as_str().is_none_or(|s| s == "file")
            && f["pattern"]
                .as_str()
                .is_none_or(|p| glob(p).is_ok_and(|g| g.is_match(&path)))
    })
}
fn watcher_pattern(watcher: &Value, root: &str) -> Result<(String, String), String> {
    let pattern = &watcher["globPattern"];
    if let Some(pattern) = pattern.as_str() {
        return Ok((root.into(), pattern.into()));
    }
    let base = pattern["baseUri"]
        .as_str()
        .or_else(|| pattern["baseUri"]["uri"].as_str())
        .and_then(|uri| uri.parse::<lsp_types::Uri>().ok())
        .and_then(|uri| uri_path(&uri))
        .ok_or("The watch pattern has no valid base URI.")?;
    if base != root && !features::within_root(&base, root) {
        return Err("The watch base is outside the worktree.".into());
    }
    let pattern = pattern["pattern"]
        .as_str()
        .ok_or("The watch pattern is missing.")?;
    Ok((base, pattern.into()))
}
impl State {
    pub fn register(&mut self, params: Value, root: &str) -> Result<(), String> {
        let params: RegistrationParams =
            serde_json::from_value(params).map_err(|e| e.to_string())?;
        if self.registrations.len() + params.registrations.len() > 128 {
            return Err("Too many dynamic registrations.".into());
        }
        let mut ids: HashSet<_> = self.registrations.iter().map(|r| r.id.clone()).collect();
        for registration in &params.registrations {
            if !ids.insert(registration.id.clone()) {
                return Err("Duplicate registration ID.".into());
            }
            let options = options(registration);
            if !options.is_object() {
                return Err("Registration options must be an object.".into());
            }
            if registration.method == "workspace/didChangeWatchedFiles" {
                if !self.watching {
                    return Err("Remote file watching is unavailable.".into());
                }
                let watchers = options["watchers"]
                    .as_array()
                    .ok_or("Missing file watchers.")?;
                if watchers.len() > 128 {
                    return Err("Too many file watch patterns.".into());
                }
                for watcher in watchers {
                    let (_, pattern) = watcher_pattern(watcher, root)?;
                    glob(&pattern)?;
                    if watcher
                        .get("kind")
                        .is_some_and(|k| k.as_u64().is_none_or(|k| k > 7))
                    {
                        return Err("Invalid file watch kind.".into());
                    }
                }
            } else {
                let property = property(&registration.method).ok_or_else(|| {
                    format!("Unsupported dynamic capability: {}", registration.method)
                })?;
                let mut caps = json!({});
                caps[property] = options.clone();
                serde_json::from_value::<ServerCapabilities>(caps).map_err(|e| e.to_string())?;
                if let Some(selector) = options.get("documentSelector").filter(|v| !v.is_null()) {
                    let filters = selector.as_array().ok_or("Invalid document selector.")?;
                    for filter in filters {
                        serde_json::from_value::<lsp_types::DocumentFilter>(filter.clone())
                            .map_err(|e| e.to_string())?;
                        if let Some(pattern) = filter["pattern"].as_str() {
                            glob(pattern)?;
                        }
                    }
                }
            }
        }
        self.registrations.extend(params.registrations);
        Ok(())
    }
    pub fn unregister(&mut self, params: Value) -> Result<(), String> {
        let params: UnregistrationParams =
            serde_json::from_value(params).map_err(|e| e.to_string())?;
        self.registrations.retain(|r| {
            !params
                .unregisterations
                .iter()
                .any(|u| u.id == r.id && u.method == r.method)
        });
        Ok(())
    }
    pub fn capabilities(
        &self,
        base: &ServerCapabilities,
        document: &Document,
    ) -> ServerCapabilities {
        let mut caps = serde_json::to_value(base).unwrap_or_else(|_| json!({}));
        for registration in &self.registrations {
            let options = options(registration);
            if let Some(property) = property(&registration.method)
                && selector_matches(&options, document)
            {
                caps[property] = options;
            }
        }
        serde_json::from_value(caps).unwrap_or_else(|_| base.clone())
    }
    /// The changes a server registered to watch, for `workspace/didChangeWatchedFiles`.
    pub fn watched_changes(
        &self,
        files: &worker::FilesChanged,
        root: &str,
    ) -> Vec<lsp_types::FileEvent> {
        let watchers: Vec<_> = self
            .registrations
            .iter()
            .filter(|r| r.method == "workspace/didChangeWatchedFiles")
            .flat_map(|r| {
                options(r)["watchers"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        files
            .changes
            .iter()
            .filter_map(|change| {
                let uri = change.uri.parse::<lsp_types::Uri>().ok()?;
                let path = uri_path(&uri)?;
                if !features::within_root(&path, root) {
                    return None;
                }
                // The change type and its `WatchKind` bit.
                let (typ, bit) = match change.kind {
                    worker::FileChangeKind::Created => (lsp_types::FileChangeType::CREATED, 1),
                    worker::FileChangeKind::Changed => (lsp_types::FileChangeType::CHANGED, 2),
                    worker::FileChangeKind::Deleted => (lsp_types::FileChangeType::DELETED, 4),
                };
                let watched = watchers.iter().any(|watcher| {
                    let mask = watcher["kind"].as_u64().unwrap_or(7);
                    if mask & bit == 0 {
                        return false;
                    }
                    let Ok((base, pattern)) = watcher_pattern(watcher, root) else {
                        return false;
                    };
                    let Some(relative) =
                        path.strip_prefix(&format!("{}/", base.trim_end_matches('/')))
                    else {
                        return false;
                    };
                    glob(&pattern).is_ok_and(|g| g.is_match(relative) || g.is_match(&path))
                });
                watched.then_some(lsp_types::FileEvent { uri, typ })
            })
            .collect()
    }
}

impl Server {
    pub(super) fn capabilities_for(&self, document: &Document) -> ServerCapabilities {
        self.protocol
            .borrow()
            .capabilities(&self.capabilities.borrow(), document)
    }
}
impl CodeWorkspace {
    pub(super) fn language_server_request(
        &mut self,
        server: Rc<Server>,
        request: newport_client::lsp::IncomingRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = match request.method.as_str() {
            "client/registerCapability" => server
                .protocol
                .borrow_mut()
                .register(request.params.clone(), &self.root),
            "client/unregisterCapability" => server
                .protocol
                .borrow_mut()
                .unregister(request.params.clone()),
            "workspace/applyEdit" => {
                self.apply_server_edits(server, request, window, cx);
                return;
            }
            _ => {
                request.respond(Err((-32601, "Unsupported request.".into())));
                return;
            }
        };
        request.respond(result.map(|()| Value::Null).map_err(|e| (-32602, e)));
        cx.notify();
    }
    /// Initializes the server with what the worker sent, or with the defaults
    /// when that notification was dropped.
    pub(super) fn configure_language_server(
        &mut self,
        server: Rc<Server>,
        config: Option<worker::Configuration>,
        cx: &mut Context<Self>,
    ) {
        if server.protocol.borrow().initializing {
            return;
        }
        server.protocol.borrow_mut().initializing = true;
        server.protocol.borrow_mut().watching = config.as_ref().is_some_and(|c| c.file_watch);
        if let Some(problem) = config.as_ref().and_then(|c| c.settings_error.clone()) {
            self.language_message(problem, cx);
        }
        let Some(connection) = server.connection.borrow().clone() else {
            return;
        };
        let mut params = initialize_params(&self.root, server.id);
        let settings = match config {
            Some(config) => {
                params.initialization_options = Some(config.initialization_options);
                config.settings
            }
            None => Value::Object(Default::default()),
        };
        connection.set_configuration(settings.clone());
        if server.protocol.borrow().watching {
            params
                .capabilities
                .workspace
                .as_mut()
                .unwrap()
                .did_change_watched_files =
                Some(lsp_types::DidChangeWatchedFilesClientCapabilities {
                    dynamic_registration: Some(true),
                    relative_pattern_support: Some(true),
                });
        }
        let initialize = connection.request::<lsp_types::request::Initialize>(params);
        let weak = Rc::downgrade(&server);
        cx.spawn(async move |this, cx| {
            let result = initialize.await;
            let _ = this.update(cx, |this, cx| {
                this.language_server_initialized(&weak, result, cx);
                if let Some(server) = this.current_language_server(&weak)
                    && server.ready.get()
                {
                    connection.notify::<lsp_types::notification::DidChangeConfiguration>(
                        lsp_types::DidChangeConfigurationParams { settings },
                    );
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Document, file_uri};
    use super::State;
    use newport_protocol::lsp::{
        FileChange,
        FileChangeKind::{self, Changed, Created, Deleted},
        FilesChanged,
    };
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    fn document(path: &str, language: &'static str) -> Document {
        Document {
            uri: file_uri(path).unwrap(),
            language,
            version: Cell::new(1),
            dirty: Cell::new(false),
            opened: Cell::new(true),
            sent: RefCell::new(None),
            completions: Default::default(),
        }
    }
    #[test]
    fn registrations_are_atomic_scoped_and_reversible() {
        let mut state = State::default();
        let register = json!({"registrations":[{"id":"a","method":"textDocument/codeAction","registerOptions":{"documentSelector":[{"language":"rust","pattern":"**/*.rs"}],"resolveProvider":true}}]});
        state.register(register, "/repo").unwrap();
        let caps = Default::default();
        assert!(
            state
                .capabilities(&caps, &document("/repo/a.rs", "rust"))
                .code_action_provider
                .is_some()
        );
        assert!(
            state
                .capabilities(&caps, &document("/repo/a.py", "python"))
                .code_action_provider
                .is_none()
        );
        assert!(state.register(json!({"registrations":[{"id":"b","method":"textDocument/hover"},{"id":"c","method":"unsupported"}]}), "/repo").is_err());
        assert!(
            state
                .capabilities(&caps, &document("/repo/a.rs", "rust"))
                .hover_provider
                .is_none()
        );
        state
            .unregister(json!({"unregisterations":[{"id":"a","method":"textDocument/codeAction"}]}))
            .unwrap();
        assert!(
            state
                .capabilities(&caps, &document("/repo/a.rs", "rust"))
                .code_action_provider
                .is_none()
        );
    }
    fn files(changes: &[(&str, FileChangeKind)]) -> FilesChanged {
        FilesChanged {
            changes: changes
                .iter()
                .map(|&(uri, kind)| FileChange {
                    uri: uri.into(),
                    kind,
                })
                .collect(),
        }
    }
    #[test]
    fn watches_filter_relative_patterns_kinds_and_worktree_boundaries() {
        let mut state = State {
            watching: true,
            ..Default::default()
        };
        state.register(json!({"registrations":[{"id":"watch","method":"workspace/didChangeWatchedFiles","registerOptions":{"watchers":[{"globPattern":{"baseUri":"file:///repo/src","pattern":"**/*.rs"},"kind":5}]}}]}), "/repo").unwrap();
        let changes = state.watched_changes(
            &files(&[
                ("file:///repo/src/a.rs", Created),
                ("file:///repo/src/nested/b.rs", Deleted),
                ("file:///repo/src/a.rs", Changed),
                ("file:///repo/src/a.py", Created),
                ("file:///other/a.rs", Created),
            ]),
            "/repo",
        );
        assert_eq!(
            changes
                .iter()
                .map(|change| (change.uri.as_str(), change.typ))
                .collect::<Vec<_>>(),
            [
                ("file:///repo/src/a.rs", lsp_types::FileChangeType::CREATED),
                (
                    "file:///repo/src/nested/b.rs",
                    lsp_types::FileChangeType::DELETED
                ),
            ]
        );
        assert!(state.register(json!({"registrations":[{"id":"bad","method":"workspace/didChangeWatchedFiles","registerOptions":{"watchers":[{"globPattern":{"baseUri":"file:///other","pattern":"**"}}]}}]}), "/repo").is_err());
        state.unregister(json!({"unregisterations":[{"id":"watch","method":"workspace/didChangeWatchedFiles"}]})).unwrap();
        assert!(
            state
                .watched_changes(&files(&[("file:///repo/src/a.rs", Created)]), "/repo")
                .is_empty()
        );
    }
}
