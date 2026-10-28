//! Language servers for the worktree: one per server, started through a
//! Needle worker when a tab first needs it, kept in step with the open tabs,
//! and plugged into each editor's LSP hooks. gpui-kit counts columns in
//! characters; positions convert to the server's encoding at this boundary.
use super::*;
#[path = "project_editor_lsp_completion.rs"]
mod completion;
#[path = "project_editor_lsp_edits.rs"]
mod edits;
#[path = "project_editor_lsp_features.rs"]
mod features;
#[path = "project_editor_lsp_protocol.rs"]
mod protocol;
pub(super) use features::{State as ToolState, Tool};
use gpui_kit::component::input::{
    CompletionProvider, DefinitionProvider, HoverProvider, Rope, RopeExt, ShowDocumentHandler,
};
use newport_client::lsp::{Connection, Event, Open};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

/// Typing pauses this long before the server sees the new text.
const SYNC_DELAY: Duration = Duration::from_millis(150);

/// The server for a file, and the language id it opens with.
pub(super) fn server_for(path: &str) -> Option<(&'static str, &'static str)> {
    let jsx = path.ends_with(".jsx");
    Some(match git_diff_syntax::language(path)? {
        "rust" => ("rust-analyzer", "rust"),
        "typescript" => ("typescript-language-server", "typescript"),
        "tsx" => ("typescript-language-server", "typescriptreact"),
        "javascript" if jsx => ("typescript-language-server", "javascriptreact"),
        "javascript" => ("typescript-language-server", "javascript"),
        "python" => ("pyright", "python"),
        "go" => ("gopls", "go"),
        _ => return None,
    })
}

/// How the server counts columns. gpui-kit counts characters (UTF-32).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Encoding {
    Utf32,
    Utf16,
}

fn line(text: &Rope, row: u32) -> String {
    let row = row as usize;
    text.slice_lines(row..row + 1).to_string()
}

/// The server's position for a byte offset in `text`.
pub(super) fn to_server(encoding: Encoding, text: &Rope, offset: usize) -> lsp_types::Position {
    let position = text.offset_to_position(offset);
    if encoding == Encoding::Utf32 {
        return position;
    }
    let character = line(text, position.line)
        .chars()
        .take(position.character as usize)
        .map(char::len_utf16)
        .sum::<usize>();
    lsp_types::Position::new(position.line, character as u32)
}

/// gpui-kit's position (character columns) for a server position.
pub(super) fn from_server(
    encoding: Encoding,
    text: &Rope,
    position: lsp_types::Position,
) -> lsp_types::Position {
    if encoding == Encoding::Utf32 {
        return position;
    }
    let mut units = 0;
    let mut characters = 0;
    for c in line(text, position.line).chars() {
        if units >= position.character as usize {
            break;
        }
        units += c.len_utf16();
        characters += 1;
    }
    lsp_types::Position::new(position.line, characters)
}

pub(super) fn range_from_server(
    encoding: Encoding,
    text: &Rope,
    range: lsp_types::Range,
) -> lsp_types::Range {
    lsp_types::Range::new(
        from_server(encoding, text, range.start),
        from_server(encoding, text, range.end),
    )
}

/// The edit from `old` to `new` as one change in the server's encoding, or
/// `None` when the texts are equal. The common start and end are left out, so
/// typing sends a few characters rather than the whole file.
pub(super) fn content_change(
    encoding: Encoding,
    old: &Rope,
    new: &Rope,
) -> Option<lsp_types::TextDocumentContentChangeEvent> {
    let prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    let limit = old.len().min(new.len()) - prefix;
    let (mut old_end, mut new_end) = (old.bytes_at(old.len()), new.bytes_at(new.len()));
    let mut suffix = 0;
    while suffix < limit && old_end.prev() == new_end.prev() {
        suffix += 1;
    }
    // Both ends on character boundaries: the bytes there are shared, so the
    // two texts agree on where characters start.
    let start = old
        .floor_char_boundary(prefix)
        .min(new.floor_char_boundary(prefix));
    while suffix > 0 && !old.is_char_boundary(old.len() - suffix) {
        suffix -= 1;
    }
    Some(lsp_types::TextDocumentContentChangeEvent {
        range: Some(lsp_types::Range::new(
            to_server(encoding, old, start),
            to_server(encoding, old, old.len() - suffix),
        )),
        range_length: None,
        text: new.slice(start..new.len() - suffix).to_string(),
    })
}

/// `file://` URI for an absolute server path.
pub(super) fn file_uri(path: &str) -> Option<lsp_types::Uri> {
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri.parse().ok()
}

/// The server path a `file://` URI names.
pub(super) fn uri_path(uri: &lsp_types::Uri) -> Option<String> {
    let rest = uri.as_str().strip_prefix("file://")?;
    // `file://host/path` is not a path on this server.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut path = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = rest
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            path.push(byte);
            i += 3;
        } else {
            path.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(path).ok()
}

/// What the editor knows about a server.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Status {
    Starting,
    LoadingEnvironment,
    Indexing(String),
    Ready,
    /// The server isn't installed on the remote host.
    NotInstalled,
    /// direnv hasn't allowed (or has denied) the worktree's `.envrc`.
    EnvrcBlocked {
        path: String,
        denied: bool,
    },
    /// The worktree has an `.envrc`, but the remote host has no direnv.
    DirenvMissing {
        path: String,
    },
    EnvironmentFailed(String),
    /// The `.envrc` or a file it watches changed; a restart applies it.
    EnvironmentChanged,
    Stopped(Option<String>),
}
impl Status {
    /// A reason the server didn't start, which a later stream end keeps.
    fn explains_stop(&self) -> bool {
        matches!(
            self,
            Self::NotInstalled
                | Self::EnvrcBlocked { .. }
                | Self::DirenvMissing { .. }
                | Self::EnvironmentFailed(_)
        )
    }

    /// The status a worker notification reports, if it changes one.
    fn from_worker(method: &str, params: &Value, current: &Status) -> Option<Status> {
        let path = || params["path"].as_str().unwrap_or(".envrc").to_owned();
        match (method, params["state"].as_str()?) {
            ("$/newport/environment", "loading") => Some(Self::LoadingEnvironment),
            ("$/newport/environment", "blocked") => Some(Self::EnvrcBlocked {
                path: path(),
                denied: false,
            }),
            ("$/newport/environment", "denied") => Some(Self::EnvrcBlocked {
                path: path(),
                denied: true,
            }),
            ("$/newport/environment", "unavailable") => Some(Self::DirenvMissing { path: path() }),
            ("$/newport/environment", "failed") => Some(Self::EnvironmentFailed(
                params["message"]
                    .as_str()
                    .unwrap_or("direnv couldn't load the .envrc.")
                    .to_owned(),
            )),
            ("$/newport/environment", "changed") => Some(Self::EnvironmentChanged),
            ("$/newport/environment", _) => Some(Self::Starting),
            ("$/newport/server", "missing") => Some(Self::NotInstalled),
            ("$/newport/server", "exited") if !current.explains_stop() => {
                let stderr = params["stderr"].as_str().unwrap_or_default().trim();
                let error = params["error"].as_str().map(str::to_owned).or_else(|| {
                    let last = stderr.lines().last()?;
                    Some(last.to_owned())
                });
                Some(Self::Stopped(error))
            }
            _ => None,
        }
    }

    /// The words after the server's name in the header row.
    fn label(&self) -> Option<&str> {
        Some(match self {
            Self::Ready => return None,
            Self::Starting => "Starting",
            Self::LoadingEnvironment => "Loading .envrc",
            // The server's own words for its work, such as "Indexing".
            Self::Indexing(title) => title,
            Self::NotInstalled => "Not installed",
            Self::EnvrcBlocked { .. } | Self::DirenvMissing { .. } | Self::EnvironmentFailed(_) => {
                "Not started"
            }
            Self::EnvironmentChanged => ".envrc changed",
            Self::Stopped(_) => "Stopped",
        })
    }
}

/// What a notice lets the user do about a server.
#[derive(Clone, Copy, Debug, PartialEq)]
enum NoticeAction {
    AllowEnvrc,
    Install,
    TryAgain,
    StartWithoutEnvrc,
    Restart,
}
impl NoticeAction {
    fn label(self) -> &'static str {
        match self {
            Self::AllowEnvrc => "Allow in terminal",
            Self::Install => "Install in terminal",
            Self::TryAgain => "Try again",
            Self::StartWithoutEnvrc => "Start without .envrc",
            Self::Restart => "Restart",
        }
    }
}

/// The command that installs a server, typed into the Terminal for review.
fn install_command(server: &str) -> Option<&'static str> {
    Some(match server {
        "rust-analyzer" => "rustup component add rust-analyzer",
        "typescript-language-server" => "npm install -g typescript-language-server typescript",
        "pyright" => "npm install -g pyright",
        "gopls" => "go install golang.org/x/tools/gopls@latest",
        _ => return None,
    })
}

/// A path as a notice shows it: relative when inside the worktree.
fn shown_path<'a>(path: &'a str, root: &str) -> &'a str {
    path.strip_prefix(root.trim_end_matches('/'))
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(path)
}

/// The notice for a server state that needs the user, and what it offers.
fn notice(server: &str, status: &Status, root: &str) -> Option<(String, Vec<NoticeAction>)> {
    use self::NoticeAction::*;
    Some(match status {
        Status::EnvrcBlocked { path, denied } => (
            format!(
                "direnv {} {}, so {server} didn’t start.",
                if *denied {
                    "has denied"
                } else {
                    "hasn’t allowed"
                },
                shown_path(path, root)
            ),
            // After allowing it in the Terminal, Try again starts the server.
            vec![AllowEnvrc, TryAgain, StartWithoutEnvrc],
        ),
        Status::DirenvMissing { path } => (
            format!(
                "This worktree has {}, but direnv isn’t installed on this server.",
                shown_path(path, root)
            ),
            vec![StartWithoutEnvrc],
        ),
        Status::EnvironmentFailed(message) => {
            let last = message.lines().rev().find(|l| !l.trim().is_empty());
            (
                format!("Couldn’t load .envrc: {}", last.unwrap_or(message).trim()),
                vec![TryAgain, StartWithoutEnvrc],
            )
        }
        Status::NotInstalled => (
            format!("{server} isn’t installed on this server."),
            if install_command(server).is_some() {
                vec![Install, TryAgain]
            } else {
                vec![TryAgain]
            },
        ),
        Status::EnvironmentChanged => (
            format!(".envrc changed. Restart {server} to use the new environment."),
            vec![Restart],
        ),
        Status::Stopped(Some(reason)) => (format!("{server} stopped: {reason}"), vec![Restart]),
        Status::Stopped(None) => (format!("{server} stopped."), vec![Restart]),
        _ => return None,
    })
}

/// A tab's error count from its server.
pub(super) fn problem_count(buffer: &Buffer) -> usize {
    buffer.lsp.as_ref().map_or(0, |attached| {
        attached
            .server
            .diagnostics
            .borrow()
            .get(&buffer.path)
            .map_or(0, |diagnostics| {
                diagnostics
                    .iter()
                    // A diagnostic without a severity is the client's call: an error.
                    .filter(|d| {
                        d.severity
                            .is_none_or(|s| s == lsp_types::DiagnosticSeverity::ERROR)
                    })
                    .count()
            })
    })
}

/// A tab's file as one server sees it.
pub(super) struct Document {
    uri: lsp_types::Uri,
    language: &'static str,
    version: Cell<i32>,
    /// The tab changed since the server last saw its text.
    dirty: Cell<bool>,
    opened: Cell<bool>,
    /// The text the server last saw, for sending only what changed.
    sent: RefCell<Option<Rope>>,
    completions: std::sync::Arc<std::sync::Mutex<Vec<(Rope, lsp_types::Position)>>>,
}

pub(super) struct Server {
    pub(super) id: &'static str,
    /// False when the user chose to start without a blocked `.envrc`.
    pub(super) envrc: bool,
    connection: RefCell<Option<Connection>>,
    ready: Cell<bool>,
    encoding: Cell<Encoding>,
    /// The server takes edits; otherwise every change sends the whole text.
    incremental: Cell<bool>,
    triggers: RefCell<Vec<String>>,
    capabilities: RefCell<lsp_types::ServerCapabilities>,
    protocol: RefCell<protocol::State>,
    pub(super) status: RefCell<Status>,
    progress: RefCell<HashMap<String, String>>,
    /// The latest diagnostics per path, in the server's encoding.
    diagnostics: RefCell<HashMap<String, Vec<lsp_types::Diagnostic>>>,
}
impl Server {
    fn new(id: &'static str, envrc: bool) -> Self {
        Self {
            id,
            envrc,
            connection: RefCell::new(None),
            ready: Cell::new(false),
            encoding: Cell::new(Encoding::Utf16),
            incremental: Cell::new(false),
            triggers: RefCell::new(Vec::new()),
            capabilities: RefCell::new(Default::default()),
            protocol: RefCell::new(Default::default()),
            status: RefCell::new(Status::Starting),
            progress: RefCell::new(HashMap::new()),
            diagnostics: RefCell::new(HashMap::new()),
        }
    }

    fn connection(&self) -> Option<Connection> {
        self.ready
            .get()
            .then(|| self.connection.borrow().clone())
            .flatten()
    }

    fn open(&self, document: &Document, text: &Rope) {
        let Some(connection) = self.connection() else {
            return;
        };
        if document.opened.replace(true) {
            return;
        }
        document.dirty.set(false);
        *document.sent.borrow_mut() = Some(text.clone());
        connection.notify::<lsp_types::notification::DidOpenTextDocument>(
            lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem::new(
                    document.uri.clone(),
                    document.language.into(),
                    document.version.get(),
                    text.to_string(),
                ),
            },
        );
    }

    /// Send the tab's text if it changed since the server last saw it.
    fn flush(&self, document: &Document, text: &Rope) {
        let Some(connection) = self.connection() else {
            return;
        };
        if !document.opened.get() {
            return;
        }
        document.dirty.set(false);
        // Completion runs before InputEvent::Change. Compare the actual snapshot,
        // rather than relying on the event subscription to have marked it dirty.
        if document.sent.borrow().as_ref() == Some(text) {
            return;
        }
        let sent = document.sent.replace(Some(text.clone()));
        let change = match sent.filter(|_| self.incremental.get()) {
            Some(sent) => match content_change(self.encoding.get(), &sent, text) {
                Some(change) => change,
                // Typed and undone: the server already has this text.
                None => return,
            },
            None => lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_string(),
            },
        };
        document.version.set(document.version.get() + 1);
        connection.notify::<lsp_types::notification::DidChangeTextDocument>(
            lsp_types::DidChangeTextDocumentParams {
                text_document: lsp_types::VersionedTextDocumentIdentifier::new(
                    document.uri.clone(),
                    document.version.get(),
                ),
                content_changes: vec![change],
            },
        );
    }

    /// The connection for a request about `document`, after its text is current.
    fn prepare(&self, document: &Document, text: &Rope) -> Option<Connection> {
        if !document.opened.get() {
            return None;
        }
        self.flush(document, text);
        self.connection()
    }

    fn close(&self, document: &Document) {
        if let Some(connection) = self.connection()
            && document.opened.replace(false)
        {
            document.sent.borrow_mut().take();
            connection.notify::<lsp_types::notification::DidCloseTextDocument>(
                lsp_types::DidCloseTextDocumentParams {
                    text_document: lsp_types::TextDocumentIdentifier::new(document.uri.clone()),
                },
            );
        }
    }

    fn saved(&self, document: &Document) {
        if let Some(connection) = self.connection()
            && document.opened.get()
        {
            connection.notify::<lsp_types::notification::DidSaveTextDocument>(
                lsp_types::DidSaveTextDocumentParams {
                    text_document: lsp_types::TextDocumentIdentifier::new(document.uri.clone()),
                    text: None,
                },
            );
        }
    }

    fn update_progress(&self, params: &Value) {
        let token = match &params["token"] {
            Value::String(token) => token.clone(),
            token => token.to_string(),
        };
        let value = &params["value"];
        let mut progress = self.progress.borrow_mut();
        match value["kind"].as_str() {
            Some("begin") => {
                let title = value["title"].as_str().unwrap_or("Working").to_owned();
                progress.insert(token, title);
            }
            Some("end") => {
                progress.remove(&token);
            }
            _ => {}
        }
        drop(progress);
        self.settle();
    }

    /// Ready, or Indexing while the server reports work in progress.
    fn settle(&self) {
        if !self.ready.get() {
            return;
        }
        let mut status = self.status.borrow_mut();
        if !matches!(
            *status,
            Status::Starting | Status::Ready | Status::Indexing(_)
        ) {
            return;
        }
        *status = match self.progress.borrow().values().next() {
            Some(title) => Status::Indexing(title.clone()),
            None => Status::Ready,
        };
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.get_mut().take() {
            connection.shutdown();
        }
    }
}

/// A tab's link to its server.
pub(super) struct Attached {
    pub(super) server: Rc<Server>,
    document: Rc<Document>,
}

/// A definition to show once its file is open, in the server's encoding.
pub(super) struct Jump {
    path: String,
    range: lsp_types::Range,
    server: &'static str,
}

fn client_capabilities() -> lsp_types::ClientCapabilities {
    serde_json::from_value(json!({
        "general": {"positionEncodings": ["utf-32", "utf-16"]},
        "window": {"workDoneProgress": true},
        "workspace": {"configuration": true, "applyEdit": true, "executeCommand": {"dynamicRegistration": true}, "workspaceFolders": true, "workspaceEdit": {"documentChanges": true, "failureHandling": "transactional"}},
        "textDocument": {
            "synchronization": {"didSave": true},
            // gpui-kit inserts completion text as written: no snippets.
            "completion": {
                "dynamicRegistration": true,
                "completionItem": {
                    "resolveSupport": {"properties": ["documentation", "detail", "additionalTextEdits"]},
                    "snippetSupport": false,
                    "documentationFormat": ["markdown", "plaintext"],
                    "labelDetailsSupport": true
                },
                "contextSupport": true
            },
            "hover": {"dynamicRegistration": true, "contentFormat": ["markdown", "plaintext"]},
            "definition": {"dynamicRegistration": true, "linkSupport": true},
            "references": {"dynamicRegistration": true},
            "rename": {"dynamicRegistration": true},
            "formatting": {"dynamicRegistration": true},
            "codeAction": {"dynamicRegistration": true, "dataSupport": true, "disabledSupport": true, "isPreferredSupport": true, "resolveSupport": {"properties": ["edit", "command"]}, "codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["", "quickfix", "refactor", "source", "source.organizeImports"]}}},
            "signatureHelp": {"dynamicRegistration": true, "signatureInformation": {"documentationFormat": ["markdown", "plaintext"]}},
            "publishDiagnostics": {"relatedInformation": false, "versionSupport": true}
        }
    }))
    .unwrap_or_default()
}

fn initialize_params(root: &str, server: &str) -> lsp_types::InitializeParams {
    let uri = file_uri(root);
    let name = root
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(root);
    #[allow(deprecated)]
    lsp_types::InitializeParams {
        process_id: None,
        root_uri: uri.clone(),
        workspace_folders: uri.map(|uri| {
            vec![lsp_types::WorkspaceFolder {
                uri,
                name: name.to_owned(),
            }]
        }),
        // Keep rust-analyzer's own watcher as the default, including with older agents.
        initialization_options: (server == "rust-analyzer")
            .then(|| json!({"files": {"watcher": "server"}})),
        capabilities: client_capabilities(),
        client_info: Some(lsp_types::ClientInfo {
            name: "Newport".into(),
            version: None,
        }),
        ..Default::default()
    }
}

/// Plain text for a snippet, for servers that send one anyway: placeholders
/// keep their default text, choices their first option, tab stops nothing.
fn snippet_text(snippet: &str) -> String {
    let mut text = String::with_capacity(snippet.len());
    let mut chars = snippet.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => text.extend(chars.next()),
            '$' if chars.peek() == Some(&'{') => {
                chars.next();
                // The body up to the matching brace, escapes kept for the recursion.
                let mut body = String::new();
                let mut depth = 1;
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            body.push(c);
                            body.extend(chars.next());
                            continue;
                        }
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    body.push(c);
                }
                let rest = body.trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '_');
                if let Some(default) = rest.strip_prefix(':') {
                    text.push_str(&snippet_text(default));
                } else if let Some(choices) = rest.strip_prefix('|') {
                    let first = choices
                        .trim_end_matches('|')
                        .split(',')
                        .next()
                        .unwrap_or_default();
                    text.push_str(&snippet_text(first));
                }
            }
            '$' if chars.peek().is_some_and(char::is_ascii_digit) => {
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
            }
            c => text.push(c),
        }
    }
    text
}

/// A completion in gpui-kit's terms: an edit in character columns that
/// replaces the word being typed, with plain text.
fn completion_edit(
    mut item: lsp_types::CompletionItem,
    encoding: Encoding,
    text: &Rope,
    offset: usize,
) -> lsp_types::CompletionItem {
    let snippet = item.insert_text_format == Some(lsp_types::InsertTextFormat::SNIPPET);
    let plain = |new_text: &str| {
        if snippet {
            snippet_text(new_text)
        } else {
            new_text.to_owned()
        }
    };
    item.text_edit = Some(match item.text_edit.take() {
        Some(lsp_types::CompletionTextEdit::Edit(edit)) => {
            lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit::new(
                range_from_server(encoding, text, edit.range),
                plain(&edit.new_text),
            ))
        }
        Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) => {
            lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit::new(
                range_from_server(encoding, text, edit.replace),
                plain(&edit.new_text),
            ))
        }
        None => {
            let line_start = text.offset_to_position(offset);
            let typed = line(text, line_start.line)
                .chars()
                .take(line_start.character as usize)
                .collect::<Vec<_>>();
            let word = typed
                .iter()
                .rev()
                .take_while(|c| c.is_alphanumeric() || **c == '_' || **c == '$')
                .count();
            let start =
                lsp_types::Position::new(line_start.line, line_start.character - word as u32);
            let new_text = item
                .insert_text
                .clone()
                .unwrap_or_else(|| item.label.clone());
            lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit::new(
                lsp_types::Range::new(start, line_start),
                plain(&new_text),
            ))
        }
    });
    item.insert_text_format = Some(lsp_types::InsertTextFormat::PLAIN_TEXT);
    item
}

/// The editor hooks for one tab.
struct Hooks {
    server: Rc<Server>,
    document: Rc<Document>,
}

fn text_position(
    document: &Document,
    position: lsp_types::Position,
) -> lsp_types::TextDocumentPositionParams {
    lsp_types::TextDocumentPositionParams::new(
        lsp_types::TextDocumentIdentifier::new(document.uri.clone()),
        position,
    )
}

/// The toolkit supplies its menu query as a trigger; the wire protocol needs
/// one advertised character, or Invoked for ordinary identifier typing.
fn completion_context(
    mut context: lsp_types::CompletionContext,
    text: &Rope,
    offset: usize,
    triggers: &[String],
) -> lsp_types::CompletionContext {
    let last = text.slice(..offset).chars().last().map(|c| c.to_string());
    if context.trigger_kind == lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER
        && last.as_ref().is_some_and(|last| triggers.contains(last))
    {
        context.trigger_character = last;
    } else {
        context.trigger_kind = lsp_types::CompletionTriggerKind::INVOKED;
        context.trigger_character = None;
    }
    context
}

impl CompletionProvider for Hooks {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: lsp_types::CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<lsp_types::CompletionResponse>> {
        let Some(connection) = self.server.prepare(&self.document, text) else {
            return Task::ready(Ok(lsp_types::CompletionResponse::Array(Vec::new())));
        };
        if self
            .server
            .capabilities_for(&self.document)
            .completion_provider
            .is_none()
        {
            return Task::ready(Ok(lsp_types::CompletionResponse::Array(Vec::new())));
        }
        let encoding = self.server.encoding.get();
        let capabilities = self.server.capabilities_for(&self.document);
        let triggers = capabilities
            .completion_provider
            .as_ref()
            .and_then(|p| p.trigger_characters.clone())
            .unwrap_or_default();
        let trigger = completion_context(trigger, text, offset, &triggers);
        let accepted = self.document.completions.clone();
        let request =
            connection.request::<lsp_types::request::Completion>(lsp_types::CompletionParams {
                text_document_position: text_position(
                    &self.document,
                    to_server(encoding, text, offset),
                ),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: Some(trigger),
            });
        let resolve = capabilities
            .completion_provider
            .as_ref()
            .is_some_and(|p| p.resolve_provider == Some(true));
        let executor = cx.background_executor().clone();
        let text = text.clone();
        cx.background_executor().spawn(async move {
            let items = match request.await.map_err(anyhow::Error::msg)? {
                None => Vec::new(),
                Some(lsp_types::CompletionResponse::Array(items)) => items,
                Some(lsp_types::CompletionResponse::List(list)) => list.items,
            };
            let items = if resolve {
                completion::resolve(connection, items, async move {
                    executor.timer(Duration::from_millis(1500)).await;
                })
                .await
            } else {
                items
            };
            let mut carets = Vec::new();
            let mut budget = 8 * 1024 * 1024usize;
            let items = items
                .into_iter()
                .filter_map(|item| {
                    if let Some(edit) = &item.text_edit {
                        let edit = match edit {
                            lsp_types::CompletionTextEdit::Edit(edit) => edit.clone(),
                            lsp_types::CompletionTextEdit::InsertAndReplace(edit) => {
                                lsp_types::TextEdit::new(edit.replace, edit.new_text.clone())
                            }
                        };
                        edits::validate(encoding, &text, vec![edit]).ok()?;
                    }
                    let additional = item.additional_text_edits.clone().unwrap_or_default();
                    let mut item = completion_edit(item, encoding, &text, offset);
                    if !additional.is_empty() {
                        // Validate in server coordinates before translating to the editor.
                        edits::validate(encoding, &text, additional.clone()).ok()?;
                        let additional = additional
                            .into_iter()
                            .map(|mut edit| {
                                edit.range = range_from_server(encoding, &text, edit.range);
                                edit
                            })
                            .collect();
                        let Some(lsp_types::CompletionTextEdit::Edit(primary)) =
                            item.text_edit.take()
                        else {
                            return None;
                        };
                        let (merged, result, caret) =
                            edits::completion(&text, primary, additional).ok()?;
                        budget = budget
                            .checked_sub(merged.new_text.len().saturating_add(result.len()))?;
                        carets.push((result, caret));
                        item.text_edit = Some(lsp_types::CompletionTextEdit::Edit(merged));
                        item.additional_text_edits = None;
                    }
                    Some(item)
                })
                .collect();
            *accepted.lock().unwrap_or_else(|e| e.into_inner()) = carets;
            Ok(lsp_types::CompletionResponse::Array(items))
        })
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        let Some(last) = new_text.chars().last() else {
            return false;
        };
        last.is_alphanumeric()
            || last == '_'
            || self
                .server
                .capabilities_for(&self.document)
                .completion_provider
                .and_then(|p| p.trigger_characters)
                .unwrap_or_default()
                .iter()
                .any(|trigger| trigger.ends_with(last))
    }
}

impl HoverProvider for Hooks {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Option<lsp_types::Hover>>> {
        if !self
            .server
            .capabilities_for(&self.document)
            .hover_provider
            .as_ref()
            .is_some_and(|p| !matches!(p, lsp_types::HoverProviderCapability::Simple(false)))
        {
            return Task::ready(Ok(None));
        }
        let Some(connection) = self.server.prepare(&self.document, text) else {
            return Task::ready(Ok(None));
        };
        let encoding = self.server.encoding.get();
        let request =
            connection.request::<lsp_types::request::HoverRequest>(lsp_types::HoverParams {
                text_document_position_params: text_position(
                    &self.document,
                    to_server(encoding, text, offset),
                ),
                work_done_progress_params: Default::default(),
            });
        let text = text.clone();
        cx.background_executor().spawn(async move {
            let hover = request.await.map_err(anyhow::Error::msg)?;
            Ok(hover.map(|mut hover| {
                hover.range = hover
                    .range
                    .map(|range| range_from_server(encoding, &text, range));
                hover
            }))
        })
    }
}

impl DefinitionProvider for Hooks {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<lsp_types::LocationLink>>> {
        if !self
            .server
            .capabilities_for(&self.document)
            .definition_provider
            .as_ref()
            .is_some_and(|p| !matches!(p, lsp_types::OneOf::Left(false)))
        {
            return Task::ready(Ok(Vec::new()));
        }
        let Some(connection) = self.server.prepare(&self.document, text) else {
            return Task::ready(Ok(Vec::new()));
        };
        let encoding = self.server.encoding.get();
        let request = connection.request::<lsp_types::request::GotoDefinition>(
            lsp_types::GotoDefinitionParams {
                text_document_position_params: text_position(
                    &self.document,
                    to_server(encoding, text, offset),
                ),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            },
        );
        let (text, uri) = (text.clone(), self.document.uri.clone());
        cx.background_executor().spawn(async move {
            let link = |location: lsp_types::Location| lsp_types::LocationLink {
                origin_selection_range: None,
                target_uri: location.uri,
                target_range: location.range,
                target_selection_range: location.range,
            };
            let links = match request.await.map_err(anyhow::Error::msg)? {
                None => Vec::new(),
                Some(lsp_types::GotoDefinitionResponse::Scalar(location)) => vec![link(location)],
                Some(lsp_types::GotoDefinitionResponse::Array(locations)) => {
                    locations.into_iter().map(link).collect()
                }
                Some(lsp_types::GotoDefinitionResponse::Link(links)) => links,
            };
            // gpui-kit jumps within this tab itself; other files keep the
            // server's encoding until they are open.
            Ok(links
                .into_iter()
                .map(|mut link| {
                    link.origin_selection_range = link
                        .origin_selection_range
                        .map(|range| range_from_server(encoding, &text, range));
                    if link.target_uri == uri {
                        link.target_range = range_from_server(encoding, &text, link.target_range);
                        link.target_selection_range =
                            range_from_server(encoding, &text, link.target_selection_range);
                    }
                    link
                })
                .collect())
        })
    }
}

/// Follow a definition into another file of the worktree.
fn show_document(
    workspace: WeakEntity<CodeWorkspace>,
    server: &'static str,
    current: lsp_types::Uri,
) -> ShowDocumentHandler {
    Rc::new(move |params, window, cx| {
        if params.uri == current || params.external == Some(true) {
            return false;
        }
        let Some(path) = uri_path(&params.uri) else {
            return true;
        };
        let range = params.selection.unwrap_or_default();
        let _ = workspace.update(cx, |this, cx| {
            this.go_to_location(path, range, server, window, cx);
        });
        true
    })
}

/// A socket whose drop ends only the server's input, so its last messages,
/// and the stream's outcome, can still arrive.
struct HalfClose(newport_platform::ipc::Stream);
impl std::io::Write for HalfClose {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
impl Drop for HalfClose {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Write);
    }
}

impl CodeWorkspace {
    pub(super) fn finish_language_completion(
        &self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(buffer) = self.buffers.iter().find(|b| b.path == path) else {
            return;
        };
        let Some(attached) = &buffer.lsp else {
            return;
        };
        let candidates = std::mem::take(
            &mut *attached
                .document
                .completions
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        let text = buffer.editor.read(cx).text();
        if let Some((_, caret)) = candidates.into_iter().find(|(result, _)| result == text) {
            buffer.editor.update(cx, |editor, cx| {
                editor.set_cursor_position(caret, window, cx)
            });
        }
    }

    /// Connect an opened tab to its language server, starting the server
    /// the first time a tab needs it.
    pub(super) fn attach_language_server(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.detach_language_server(path, cx);
        let Some(index) = self.buffers.iter().position(|b| b.path == path) else {
            return;
        };
        let Some((id, language)) = server_for(path) else {
            return;
        };
        // Same limits as highlighting: sections and very large files go without.
        let buffer = &self.buffers[index];
        if buffer.failure.is_some()
            || buffer.page.is_some()
            || buffer.saved.len() > newport_domain::text_file::HIGHLIGHT_LIMIT
        {
            return;
        }
        let Some(uri) = file_uri(path) else {
            return;
        };
        let server = match self.language_servers.get(id) {
            Some(server) => server.clone(),
            None => self.start_language_server(id, true, window, cx),
        };
        let document = Rc::new(Document {
            uri: uri.clone(),
            language,
            version: Cell::new(1),
            dirty: Cell::new(false),
            opened: Cell::new(false),
            sent: RefCell::new(None),
            completions: Default::default(),
        });
        let hooks = Rc::new(Hooks {
            server: server.clone(),
            document: document.clone(),
        });
        let handler = show_document(cx.entity().downgrade(), id, uri);
        let buffer = &mut self.buffers[index];
        buffer.editor.update(cx, |editor, cx| {
            server.open(&document, editor.text());
            let lsp = editor.lsp_mut();
            lsp.completion_provider = Some(hooks.clone());
            lsp.hover_provider = Some(hooks.clone());
            lsp.definition_provider = Some(hooks);
            lsp.show_document = Some(handler);
            cx.notify();
        });
        buffer.lsp = Some(Attached { server, document });
        self.apply_diagnostics(path, cx);
    }

    /// Tell the server a tab closed, and take the server out of the tab's
    /// editor: its hooks hold the server, which shuts down only when unused.
    pub(super) fn detach_language_server(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else {
            return;
        };
        let Some(attached) = buffer.lsp.take() else {
            return;
        };
        buffer.lsp_sync = None;
        attached.server.close(&attached.document);
        buffer.editor.update(cx, |editor, cx| {
            let lsp = editor.lsp_mut();
            lsp.completion_provider = None;
            lsp.hover_provider = None;
            lsp.definition_provider = None;
            lsp.show_document = None;
            if let Some(set) = editor.diagnostics_mut() {
                set.clear();
            }
            cx.notify();
        });
    }

    /// Send a tab's edits once typing pauses.
    pub(super) fn schedule_language_sync(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else {
            return;
        };
        let Some(attached) = &buffer.lsp else {
            return;
        };
        let dirty = attached.document.sent.borrow().as_ref() != Some(buffer.editor.read(cx).text());
        attached.document.dirty.set(dirty);
        if !dirty {
            buffer.lsp_sync = None;
            return;
        }
        let path = path.to_owned();
        buffer.lsp_sync = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SYNC_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(buffer) = this.buffers.iter().find(|b| b.path == path)
                    && let Some(attached) = &buffer.lsp
                {
                    attached
                        .server
                        .flush(&attached.document, buffer.editor.read(cx).text());
                }
            });
        }));
    }

    pub(super) fn language_saved(&self, path: &str, cx: &App) {
        if let Some(buffer) = self.buffers.iter().find(|b| b.path == path)
            && let Some(attached) = &buffer.lsp
        {
            attached
                .server
                .flush(&attached.document, buffer.editor.read(cx).text());
            attached.server.saved(&attached.document);
        }
    }

    /// Start a server again, for example after its `.envrc` changed, or
    /// without a blocked `.envrc` when `envrc` is false.
    pub(super) fn restart_language_server(
        &mut self,
        id: &'static str,
        envrc: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<String> = self
            .buffers
            .iter()
            .filter(|b| b.lsp.as_ref().is_some_and(|a| a.server.id == id))
            .map(|b| b.path.clone())
            .collect();
        for path in &paths {
            self.detach_language_server(path, cx);
        }
        // Dropping the last reference shuts the running server down.
        self.language_servers.remove(id);
        self.start_language_server(id, envrc, window, cx);
        for path in &paths {
            self.attach_language_server(path, window, cx);
        }
        cx.notify();
    }

    fn start_language_server(
        &mut self,
        id: &'static str,
        envrc: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Rc<Server> {
        let server = Rc::new(Server::new(id, envrc));
        self.language_servers.insert(id, server.clone());
        let (remote, scope, root) = (
            self.server.clone(),
            self.connection.clone(),
            self.root.clone(),
        );
        let folders = json!([{"uri": file_uri(&root).map(|uri| uri.as_str().to_owned()), "name": root.rsplit('/').find(|p| !p.is_empty()).unwrap_or("/")}]);
        let opening = cx.background_executor().spawn(async move {
            let stream = backend::Backend::discover().open_lsp(Open {
                server_id: &remote,
                expected_connection: &scope,
                root: &root,
                server: id,
                envrc,
            })?;
            let reader = stream.try_clone().map_err(|error| error.to_string())?;
            Ok::<_, String>(Connection::start(reader, HalfClose(stream), folders))
        });
        let weak = Rc::downgrade(&server);
        cx.spawn_in(window, async move |this, cx| {
            let (connection, events) = match opening.await {
                Ok(started) => started,
                Err(error) => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.language_server_event(
                            &weak,
                            Event::Closed { error: Some(error) },
                            window,
                            cx,
                        )
                    });
                    return;
                }
            };
            let connected = this.update_in(cx, |this, _window, cx| {
                this.language_server_connected(&weak, connection.clone(), cx)
            });
            if !matches!(connected, Ok(true)) {
                connection.shutdown();
                return;
            }
            while let Ok(event) = events.recv().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        this.language_server_event(&weak, event, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        server
    }

    /// The server `weak` points to, while it is still this worktree's.
    fn current_language_server(&self, weak: &Weak<Server>) -> Option<Rc<Server>> {
        let server = weak.upgrade()?;
        self.language_servers
            .get(server.id)
            .is_some_and(|current| Rc::ptr_eq(current, &server))
            .then_some(server)
    }

    fn language_server_connected(
        &mut self,
        weak: &Weak<Server>,
        connection: Connection,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(server) = self.current_language_server(weak) else {
            return false;
        };
        *server.connection.borrow_mut() = Some(connection.clone());
        // The worker sends configuration before its "started" notification.
        // Older workers initialize with defaults when that notification arrives.
        cx.notify();
        true
    }

    fn language_server_initialized(
        &mut self,
        weak: &Weak<Server>,
        result: Result<lsp_types::InitializeResult, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.current_language_server(weak) else {
            return;
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let mut status = server.status.borrow_mut();
                if !status.explains_stop() {
                    *status = Status::Stopped(Some(error));
                }
                cx.notify();
                return;
            }
        };
        let Some(connection) = server.connection.borrow().clone() else {
            return;
        };
        let capabilities = result.capabilities;
        *server.capabilities.borrow_mut() = capabilities.clone();
        server.encoding.set(
            if capabilities.position_encoding == Some(lsp_types::PositionEncodingKind::UTF32) {
                Encoding::Utf32
            } else {
                Encoding::Utf16
            },
        );
        server.incremental.set(matches!(
            capabilities.text_document_sync,
            Some(lsp_types::TextDocumentSyncCapability::Kind(
                lsp_types::TextDocumentSyncKind::INCREMENTAL
            )) | Some(lsp_types::TextDocumentSyncCapability::Options(
                lsp_types::TextDocumentSyncOptions {
                    change: Some(lsp_types::TextDocumentSyncKind::INCREMENTAL),
                    ..
                }
            ))
        ));
        *server.triggers.borrow_mut() = capabilities
            .completion_provider
            .and_then(|provider| provider.trigger_characters)
            .unwrap_or_default();
        connection.notify::<lsp_types::notification::Initialized>(lsp_types::InitializedParams {});
        server.ready.set(true);
        server.settle();
        for buffer in &self.buffers {
            if let Some(attached) = &buffer.lsp
                && Rc::ptr_eq(&attached.server, &server)
            {
                server.open(&attached.document, buffer.editor.read(cx).text());
            }
        }
        cx.notify();
    }

    fn language_server_event(
        &mut self,
        weak: &Weak<Server>,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.current_language_server(weak) else {
            return;
        };
        match event {
            Event::Request(request) => self.language_server_request(server, request, window, cx),
            Event::Notification { method, params } => match method.as_str() {
                "$/newport/configuration" => {
                    self.configure_language_server(server.clone(), params, cx)
                }
                "$/newport/filesChanged" => {
                    let changes = server
                        .protocol
                        .borrow()
                        .watched_changes(&params, &self.root);
                    if !changes.is_empty()
                        && let Some(connection) = server.connection()
                    {
                        connection.notify_value(
                            "workspace/didChangeWatchedFiles",
                            json!({"changes":changes}),
                        );
                    }
                }
                "$/newport/watchError" => {
                    self.language_message(
                        params["error"]
                            .as_str()
                            .unwrap_or("File watching failed. Restart the language server."),
                        cx,
                    );
                }
                "textDocument/publishDiagnostics" => {
                    let Ok(params) =
                        serde_json::from_value::<lsp_types::PublishDiagnosticsParams>(params)
                    else {
                        return;
                    };
                    let Some(path) = uri_path(&params.uri) else {
                        return;
                    };
                    if let Some(buffer) = self.buffers.iter().find(|b| b.path == path)
                        && let Some(attached) = &buffer.lsp
                        && (params
                            .version
                            .is_some_and(|version| version != attached.document.version.get())
                            || attached.document.sent.borrow().as_ref()
                                != Some(buffer.editor.read(cx).text()))
                    {
                        return;
                    }
                    server
                        .diagnostics
                        .borrow_mut()
                        .insert(path.clone(), params.diagnostics);
                    self.apply_diagnostics(&path, cx);
                }
                "$/progress" => server.update_progress(&params),
                method if method.starts_with("$/newport/") => {
                    if method == "$/newport/server"
                        && params["state"] == "started"
                        && !server.protocol.borrow().initializing
                    {
                        self.configure_language_server(server.clone(), json!({}), cx);
                    }
                    let next = Status::from_worker(method, &params, &server.status.borrow());
                    if let Some(next) = next {
                        *server.status.borrow_mut() = next;
                        server.settle();
                    }
                }
                _ => return,
            },
            Event::Closed { error } => {
                server.ready.set(false);
                server.connection.borrow_mut().take();
                server.progress.borrow_mut().clear();
                server.diagnostics.borrow_mut().clear();
                {
                    let mut status = server.status.borrow_mut();
                    if !status.explains_stop() && !matches!(*status, Status::Stopped(_)) {
                        *status = Status::Stopped(error);
                    }
                }
                for buffer in &self.buffers {
                    if buffer
                        .lsp
                        .as_ref()
                        .is_some_and(|a| Rc::ptr_eq(&a.server, &server))
                    {
                        buffer.editor.update(cx, |editor, cx| {
                            if let Some(set) = editor.diagnostics_mut() {
                                set.clear();
                            }
                            cx.notify();
                        });
                    }
                }
            }
        }
        cx.notify();
    }

    /// Show a tab's latest diagnostics from its server.
    fn apply_diagnostics(&self, path: &str, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter().find(|b| b.path == path) else {
            return;
        };
        let Some(attached) = &buffer.lsp else {
            return;
        };
        let diagnostics = attached
            .server
            .diagnostics
            .borrow()
            .get(path)
            .cloned()
            .unwrap_or_default();
        let encoding = attached.server.encoding.get();
        buffer.editor.update(cx, |editor, cx| {
            let text = editor.text().clone();
            if let Some(set) = editor.diagnostics_mut() {
                set.reset(&text);
                set.extend(diagnostics.into_iter().map(|mut diagnostic| {
                    diagnostic.range = range_from_server(encoding, &text, diagnostic.range);
                    diagnostic
                }));
            }
            cx.notify();
        });
    }

    /// Open a definition's file and move to it.
    fn go_to_location(
        &mut self,
        path: String,
        range: lsp_types::Range,
        server: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !path.starts_with(&format!("{}/", self.root.trim_end_matches('/'))) {
            self.reveal_notice = Some(
                "This definition is outside the worktree, so it can’t be opened here yet.".into(),
            );
            cx.notify();
            return;
        }
        self.pending_jump = Some(Jump {
            path: path.clone(),
            range,
            server,
        });
        self.open(path, window, cx);
        self.apply_pending_jump(window, cx);
    }

    pub(super) fn apply_pending_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(jump) = &self.pending_jump else {
            return;
        };
        let Some(buffer) = self.buffers.iter().find(|b| b.path == jump.path) else {
            return;
        };
        let jump = self.pending_jump.take().expect("checked");
        if buffer.failure.is_some() {
            return;
        }
        let encoding = self
            .language_servers
            .get(jump.server)
            .map_or(Encoding::Utf16, |server| server.encoding.get());
        buffer.editor.update(cx, |editor, cx| {
            let start = from_server(encoding, editor.text(), jump.range.start);
            editor.set_cursor_position(start, window, cx);
        });
    }

    /// The active tab's server for the header row: its name, a dot, and
    /// what it is doing unless it is ready.
    pub(super) fn language_status(&self, cx: &App) -> Option<Div> {
        let attached = self
            .active
            .and_then(|i| self.buffers.get(i))?
            .lsp
            .as_ref()?;
        let status = attached.server.status.borrow();
        let dark = cx.theme().is_dark();
        let color = match &*status {
            Status::Ready => Some(crate::theme::success(dark)),
            // Working on it: the hollow ring.
            Status::Starting | Status::LoadingEnvironment | Status::Indexing(_) => None,
            Status::Stopped(_) => Some(crate::theme::error(dark)),
            _ => Some(crate::theme::warning(dark)),
        };
        let text = match status.label() {
            Some(label) => format!("{} · {label}", attached.server.id),
            None => attached.server.id.to_owned(),
        };
        Some(
            div()
                .debug_selector(|| "code-language-status".into())
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(6.))
                .child(crate::controls::status_dot(color, cx))
                .child(text),
        )
    }

    /// A row under the header while the active tab's server needs the user.
    pub(super) fn language_notice(&self, cx: &mut Context<Self>) -> Option<Div> {
        let attached = self
            .active
            .and_then(|i| self.buffers.get(i))?
            .lsp
            .as_ref()?;
        let id = attached.server.id;
        let (message, actions) = notice(id, &attached.server.status.borrow(), &self.root)?;
        Some(
            div()
                .debug_selector(|| "code-language-notice".into())
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .border_b_1()
                .border_color(cx.theme().border)
                .text_size(px(crate::ui::FONT_SIZE))
                .child(div().flex_1().min_w_0().child(message))
                .children(actions.into_iter().enumerate().map(|(i, action)| {
                    crate::controls::compact_button(("code-language-action", i))
                        .ghost()
                        .label(action.label())
                        .disabled(!self.available)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.language_action(id, action, window, cx)
                        }))
                })),
        )
    }

    fn language_action(
        &mut self,
        id: &'static str,
        action: NoticeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let envrc = self
            .language_servers
            .get(id)
            .is_none_or(|server| server.envrc);
        match action {
            NoticeAction::AllowEnvrc => match shlex::try_quote(&self.root) {
                Ok(root) => {
                    let command = format!("cd -- {root} && direnv allow");
                    self.type_in_terminal(command, "Allow .envrc", window, cx);
                }
                Err(_) => self.error = Some("Couldn’t open this worktree in the Terminal.".into()),
            },
            NoticeAction::Install => {
                if let Some(command) = install_command(id) {
                    self.type_in_terminal(command.into(), &format!("Install {id}"), window, cx);
                }
            }
            NoticeAction::StartWithoutEnvrc => self.restart_language_server(id, false, window, cx),
            NoticeAction::TryAgain | NoticeAction::Restart => {
                self.restart_language_server(id, envrc, window, cx)
            }
        }
        cx.notify();
    }

    /// Open a Terminal tab with `command` typed in but not run: the user
    /// reviews it and presses Enter.
    fn type_in_terminal(
        &mut self,
        command: String,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = match newport_client::terminal_command::CommandDraft::new(command) {
            Ok(draft) => draft,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        if !self.terminal_visible {
            self.toggle_terminal(window, cx);
        }
        if let Some(terminal) = self.terminal.clone()
            && let Err(error) = terminal.update(cx, |terminal, cx| {
                terminal.open_named_command(draft, title, cx)
            })
        {
            self.error = Some(error);
        }
    }
}

#[cfg(test)]
impl CodeWorkspace {
    /// Link an open tab to a server in `status`, with `errors` diagnostics,
    /// without starting anything.
    pub(super) fn attach_test_server(&mut self, path: &str, status: Status, errors: usize) {
        let server = Rc::new(Server::new("rust-analyzer", true));
        *server.status.borrow_mut() = status;
        let error = lsp_types::Diagnostic::new_simple(lsp_types::Range::default(), "x".into());
        server
            .diagnostics
            .borrow_mut()
            .insert(path.into(), vec![error; errors]);
        let buffer = self.buffers.iter_mut().find(|b| b.path == path).unwrap();
        buffer.lsp = Some(Attached {
            server,
            document: Rc::new(Document {
                uri: file_uri(path).unwrap(),
                language: "rust",
                version: Cell::new(1),
                dirty: Cell::new(false),
                opened: Cell::new(false),
                sent: RefCell::new(None),
                completions: Default::default(),
            }),
        });
    }

    /// Give a tab's editor the hooks of its attached test server, as a real
    /// attach does, and return the server weakly.
    pub(super) fn install_test_hooks(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Weak<Server> {
        let buffer = self.buffers.iter().find(|b| b.path == path).unwrap();
        let attached = buffer.lsp.as_ref().unwrap();
        let hooks = Rc::new(Hooks {
            server: attached.server.clone(),
            document: attached.document.clone(),
        });
        let server = Rc::downgrade(&attached.server);
        buffer.editor.update(cx, |editor, _| {
            let lsp = editor.lsp_mut();
            lsp.completion_provider = Some(hooks.clone());
            lsp.hover_provider = Some(hooks.clone());
            lsp.definition_provider = Some(hooks);
        });
        server
    }

    /// Wait to move to a position in `path`, as following a definition does.
    pub(super) fn set_test_jump(&mut self, path: &str, line: u32, character: u32) {
        self.pending_jump = Some(Jump {
            path: path.into(),
            range: lsp_types::Range::new(
                lsp_types::Position::new(line, character),
                lsp_types::Position::new(line, character),
            ),
            server: "rust-analyzer",
        });
    }
}

#[cfg(test)]
mod tests {
    // Named imports: the crate's glob of gpui-kit brings a `test` macro.
    use super::{
        Encoding, NoticeAction, Rope, Server, Status, completion_edit, content_change, file_uri,
        from_server, initialize_params, notice, server_for, snippet_text, to_server, uri_path,
    };
    use serde_json::{Value, json};

    #[test]
    fn requests_flush_current_text_even_before_the_change_event() {
        use super::{Document, file_uri, test_support};
        use std::{
            cell::{Cell, RefCell},
            time::Duration,
        };
        let (connection, _server, messages) = test_support::connection();
        let server = Server::new("rust-analyzer", true);
        server.ready.set(true);
        server.incremental.set(true);
        *server.connection.borrow_mut() = Some(connection);
        let document = Document {
            uri: file_uri("/repo/main.rs").unwrap(),
            language: "rust",
            version: Cell::new(1),
            dirty: Cell::new(false),
            opened: Cell::new(false),
            sent: RefCell::new(None),
            completions: Default::default(),
        };
        server.open(&document, &Rope::from("x"));
        assert_eq!(
            messages.recv_timeout(Duration::from_secs(2)).unwrap()["method"],
            "textDocument/didOpen"
        );
        // The toolkit invokes the completion provider before emitting Change.
        assert!(!document.dirty.get());
        let connection = server.prepare(&document, &Rope::from("xy")).unwrap();
        let _request = connection.request_value("textDocument/completion", Value::Null);
        let changed = messages.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(changed["method"], "textDocument/didChange");
        assert_eq!(changed["params"]["contentChanges"][0]["text"], "y");
        assert_eq!(
            messages.recv_timeout(Duration::from_secs(2)).unwrap()["method"],
            "textDocument/completion"
        );
    }

    #[test]
    fn completion_triggers_are_single_advertised_characters() {
        use lsp_types::{CompletionContext, CompletionTriggerKind as Kind};
        let trigger = |query: &str| CompletionContext {
            trigger_kind: Kind::TRIGGER_CHARACTER,
            trigger_character: Some(query.into()),
        };
        let ordinary =
            super::completion_context(trigger("pri"), &Rope::from("pri"), 3, &[".".into()]);
        assert_eq!(ordinary.trigger_kind, Kind::INVOKED);
        assert_eq!(ordinary.trigger_character, None);
        let member =
            super::completion_context(trigger("foo."), &Rope::from("foo."), 4, &[".".into()]);
        assert_eq!(member.trigger_kind, Kind::TRIGGER_CHARACTER);
        assert_eq!(member.trigger_character.as_deref(), Some("."));
    }

    fn position(line: u32, character: u32) -> lsp_types::Position {
        lsp_types::Position::new(line, character)
    }

    #[test]
    fn files_map_to_their_servers() {
        assert_eq!(
            server_for("/w/src/main.rs"),
            Some(("rust-analyzer", "rust"))
        );
        assert_eq!(
            server_for("/w/app.tsx"),
            Some(("typescript-language-server", "typescriptreact"))
        );
        assert_eq!(
            server_for("/w/app.jsx"),
            Some(("typescript-language-server", "javascriptreact"))
        );
        assert_eq!(
            server_for("/w/x.mjs"),
            Some(("typescript-language-server", "javascript"))
        );
        assert_eq!(server_for("/w/a.py"), Some(("pyright", "python")));
        assert_eq!(server_for("/w/a.go"), Some(("gopls", "go")));
        assert_eq!(server_for("/w/README.md"), None);
    }

    #[test]
    fn utf16_columns_convert_around_characters_outside_the_bmp() {
        // "😀" is one character but two UTF-16 units.
        let text = Rope::from("a😀b\nx");
        let b = "a😀".len();
        assert_eq!(to_server(Encoding::Utf32, &text, b), position(0, 2));
        assert_eq!(to_server(Encoding::Utf16, &text, b), position(0, 3));
        assert_eq!(
            from_server(Encoding::Utf16, &text, position(0, 3)),
            position(0, 2)
        );
        assert_eq!(
            from_server(Encoding::Utf32, &text, position(0, 2)),
            position(0, 2)
        );
        assert_eq!(
            from_server(Encoding::Utf16, &text, position(1, 1)),
            position(1, 1)
        );
        // Past the end of a line stays at its end.
        assert_eq!(
            from_server(Encoding::Utf16, &text, position(0, 99)),
            position(0, 3)
        );
    }

    #[test]
    fn edits_send_only_what_changed() {
        let change = |encoding, old: &str, new: &str| {
            content_change(encoding, &Rope::from(old), &Rope::from(new))
                .map(|change| (change.range.unwrap(), change.text))
        };
        let range = |a: (u32, u32), b: (u32, u32)| {
            lsp_types::Range::new(position(a.0, a.1), position(b.0, b.1))
        };
        assert_eq!(
            change(Encoding::Utf16, "fn main() {}\n", "fn main() {}\n"),
            None
        );
        // Typing, deleting and replacing on the second line.
        assert_eq!(
            change(Encoding::Utf16, "a\nfoo\n", "a\nfoxo\n"),
            Some((range((1, 2), (1, 2)), "x".into()))
        );
        assert_eq!(
            change(Encoding::Utf16, "a\nfoo\n", "a\nfo\n"),
            Some((range((1, 2), (1, 3)), String::new()))
        );
        assert_eq!(
            change(Encoding::Utf16, "let x = 1;", "let y = 2;"),
            Some((range((0, 4), (0, 9)), "y = 2".into()))
        );
        // "é" and "è" share their first byte: the edit replaces whole characters.
        assert_eq!(
            change(Encoding::Utf32, "café!", "cafè!"),
            Some((range((0, 3), (0, 4)), "è".into()))
        );
        // Columns count in the server's encoding.
        assert_eq!(
            change(Encoding::Utf16, "😀a", "😀b"),
            Some((range((0, 2), (0, 3)), "b".into()))
        );
        assert_eq!(
            change(Encoding::Utf32, "😀a", "😀b"),
            Some((range((0, 1), (0, 2)), "b".into()))
        );
        // Repeated text: the change lands where the texts first differ.
        assert_eq!(
            change(Encoding::Utf16, "aaa", "aaaa"),
            Some((range((0, 3), (0, 3)), "a".into()))
        );
        assert_eq!(
            change(Encoding::Utf16, "", "x\n"),
            Some((range((0, 0), (0, 0)), "x\n".into()))
        );
    }

    #[test]
    fn file_uris_round_trip_with_encoding() {
        let path = "/srv/my app/ü#1.rs";
        let uri = file_uri(path).unwrap();
        assert_eq!(uri.as_str(), "file:///srv/my%20app/%C3%BC%231.rs");
        assert_eq!(uri_path(&uri).as_deref(), Some(path));
        assert_eq!(uri_path(&"file://other/x".parse().unwrap()), None);
        assert_eq!(uri_path(&"https://example.com/x".parse().unwrap()), None);
    }

    #[test]
    fn worker_notifications_set_the_status() {
        let starting = Status::Starting;
        let state = |method: &str, params: Value| Status::from_worker(method, &params, &starting);
        assert_eq!(
            state("$/newport/environment", json!({"state": "loading"})),
            Some(Status::LoadingEnvironment)
        );
        assert_eq!(
            state(
                "$/newport/environment",
                json!({"state": "blocked", "path": "/w/.envrc"})
            ),
            Some(Status::EnvrcBlocked {
                path: "/w/.envrc".into(),
                denied: false
            })
        );
        assert_eq!(
            state(
                "$/newport/environment",
                json!({"state": "unavailable", "path": "/w/.envrc"})
            ),
            Some(Status::DirenvMissing {
                path: "/w/.envrc".into()
            })
        );
        assert_eq!(
            state("$/newport/environment", json!({"state": "changed"})),
            Some(Status::EnvironmentChanged)
        );
        assert_eq!(
            state("$/newport/server", json!({"state": "missing"})),
            Some(Status::NotInstalled)
        );
        assert_eq!(
            state(
                "$/newport/server",
                json!({"state": "exited", "stderr": "warn\nerror: no toolchain\n"})
            ),
            Some(Status::Stopped(Some("error: no toolchain".into())))
        );
        // A reason the server didn't start outlives its exit.
        let blocked = Status::EnvrcBlocked {
            path: "/w/.envrc".into(),
            denied: false,
        };
        assert_eq!(
            Status::from_worker("$/newport/server", &json!({"state": "exited"}), &blocked),
            None
        );
    }

    #[test]
    fn progress_moves_between_indexing_and_ready() {
        let server = Server::new("rust-analyzer", true);
        server.ready.set(true);
        server.settle();
        assert_eq!(*server.status.borrow(), Status::Ready);
        server.update_progress(
            &json!({"token": "a", "value": {"kind": "begin", "title": "Indexing"}}),
        );
        assert_eq!(*server.status.borrow(), Status::Indexing("Indexing".into()));
        server.update_progress(&json!({"token": "a", "value": {"kind": "end"}}));
        assert_eq!(*server.status.borrow(), Status::Ready);
        *server.status.borrow_mut() = Status::EnvironmentChanged;
        server.update_progress(&json!({"token": 1, "value": {"kind": "begin", "title": "x"}}));
        assert_eq!(*server.status.borrow(), Status::EnvironmentChanged);
    }

    #[test]
    fn completions_become_plain_edits_in_character_columns() {
        let text = Rope::from("let 😀 = pri");
        let offset = text.len();
        let item = |json: Value| serde_json::from_value::<lsp_types::CompletionItem>(json).unwrap();
        let edit = |item: lsp_types::CompletionItem| match item.text_edit {
            Some(lsp_types::CompletionTextEdit::Edit(edit)) => (edit.range, edit.new_text),
            _ => panic!("no edit"),
        };
        // No edit: replace the word being typed, "pri" at characters 8..11.
        let (range, new_text) = edit(completion_edit(
            item(json!({"label": "println"})),
            Encoding::Utf16,
            &text,
            offset,
        ));
        assert_eq!(
            (range.start, range.end, new_text.as_str()),
            (position(0, 8), position(0, 11), "println")
        );
        // UTF-16 units 9..12 are characters 8..11: the emoji takes two units.
        let server = item(
            json!({"label": "print", "textEdit": {"range": {"start": {"line": 0, "character": 9}, "end": {"line": 0, "character": 12}}, "newText": "print"}}),
        );
        let (range, _) = edit(completion_edit(server, Encoding::Utf16, &text, offset));
        assert_eq!((range.start, range.end), (position(0, 8), position(0, 11)));
        // Snippets lose their placeholders.
        let snippet =
            item(json!({"label": "f", "insertTextFormat": 2, "insertText": "foo(${1:x}, $2)$0"}));
        assert_eq!(
            edit(completion_edit(snippet, Encoding::Utf32, &text, offset)).1,
            "foo(x, )"
        );
    }

    #[test]
    fn snippets_become_plain_text() {
        assert_eq!(snippet_text("vec![${1:a}, ${2:b}]$0"), "vec![a, b]");
        assert_eq!(snippet_text("${1:outer ${2:inner}}"), "outer inner");
        assert_eq!(snippet_text(r"cost \$5"), "cost $5");
        assert_eq!(
            snippet_text("${1|one,two|} ${TM_FILENAME} ${2:a\\}b}"),
            "one  a}b"
        );
    }

    #[test]
    fn initialize_asks_for_character_columns_first() {
        let params = initialize_params("/srv/app", "rust-analyzer");
        let general = params.capabilities.general.unwrap();
        assert_eq!(
            general.position_encodings.unwrap(),
            [
                lsp_types::PositionEncodingKind::UTF32,
                lsp_types::PositionEncodingKind::UTF16
            ]
        );
        assert_eq!(params.workspace_folders.unwrap()[0].name, "app");
        assert!(params.initialization_options.is_some());
        let snippets = params
            .capabilities
            .text_document
            .unwrap()
            .completion
            .unwrap()
            .completion_item
            .unwrap()
            .snippet_support;
        assert_eq!(snippets, Some(false));
    }

    #[test]
    fn the_header_names_the_server_and_what_it_is_doing() {
        assert_eq!(Status::Ready.label(), None);
        assert_eq!(
            Status::Indexing("Indexing".into()).label(),
            Some("Indexing")
        );
        assert_eq!(Status::LoadingEnvironment.label(), Some("Loading .envrc"));
        assert_eq!(Status::NotInstalled.label(), Some("Not installed"));
        assert_eq!(
            Status::DirenvMissing {
                path: "/w/.envrc".into()
            }
            .label(),
            Some("Not started")
        );
        assert_eq!(Status::Stopped(None).label(), Some("Stopped"));
    }

    #[test]
    fn notices_say_what_happened_and_offer_a_way_forward() {
        let say = |status: Status| notice("rust-analyzer", &status, "/srv/app/");
        let blocked = Status::EnvrcBlocked {
            path: "/srv/app/.envrc".into(),
            denied: false,
        };
        assert_eq!(
            say(blocked),
            Some((
                "direnv hasn’t allowed .envrc, so rust-analyzer didn’t start.".into(),
                vec![
                    NoticeAction::AllowEnvrc,
                    NoticeAction::TryAgain,
                    NoticeAction::StartWithoutEnvrc
                ]
            ))
        );
        // An .envrc above the worktree keeps its full path.
        let denied = Status::EnvrcBlocked {
            path: "/srv/.envrc".into(),
            denied: true,
        };
        assert_eq!(
            say(denied).unwrap().0,
            "direnv has denied /srv/.envrc, so rust-analyzer didn’t start."
        );
        let failed = Status::EnvironmentFailed("direnv: loading\nerror: no devShell\n\n".into());
        assert_eq!(
            say(failed).unwrap().0,
            "Couldn’t load .envrc: error: no devShell"
        );
        assert_eq!(
            say(Status::NotInstalled).unwrap().1,
            [NoticeAction::Install, NoticeAction::TryAgain]
        );
        assert_eq!(
            say(Status::Stopped(Some("error: toolchain".into()))).unwrap(),
            (
                "rust-analyzer stopped: error: toolchain".into(),
                vec![NoticeAction::Restart]
            )
        );
        for quiet in [
            Status::Ready,
            Status::Starting,
            Status::Indexing("x".into()),
        ] {
            assert_eq!(say(quiet), None);
        }
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use newport_client::lsp::Connection;
    use serde_json::Value;
    use std::{io, sync::mpsc};
    struct Writer(mpsc::Sender<Value>);
    impl io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let at = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            self.0
                .send(serde_json::from_slice(&bytes[at..]).unwrap())
                .map_err(io::Error::other)?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    pub fn connection() -> (Connection, io::PipeWriter, mpsc::Receiver<Value>) {
        let (connection, writer, messages, _) = connection_events();
        (connection, writer, messages)
    }
    pub fn connection_events() -> (
        Connection,
        io::PipeWriter,
        mpsc::Receiver<Value>,
        async_channel::Receiver<newport_client::lsp::Event>,
    ) {
        let (input, server) = io::pipe().unwrap();
        let (sent, received) = mpsc::channel();
        let (connection, events) = Connection::start(input, Writer(sent), Value::Null);
        (connection, server, received, events)
    }
}
