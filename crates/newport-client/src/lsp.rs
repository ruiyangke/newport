//! Language servers reached through the backend's `$lsp_open` stream. A reader
//! thread parses the server's messages and a writer thread sends ours, so the
//! UI thread never blocks on the connection. Server requests Newport doesn't
//! act on get neutral answers; dropping a pending request cancels it.
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    io::{self, BufRead, BufReader, Read, Write},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicI64, Ordering},
    },
};

const STOPPED: &str = "The language server stopped.";
/// Matches the Needle worker: nothing the editor asks for needs more.
const MAX_MESSAGE: usize = 32 * 1024 * 1024;
/// Messages the writer may fall behind by before the connection is failed: a
/// server that stops reading must not grow memory without bound.
const OUTBOUND_CAPACITY: usize = 4096;
/// Events the UI may fall behind by. Beyond it, notifications are dropped; the
/// last slot is kept for `Event::Closed`.
const EVENTS_CAPACITY: usize = 1024;
const OUTBOUND_OVERFLOW: &str = "The language server stopped reading and was disconnected.";

/// What the editor asks the backend to start.
pub struct Open<'a> {
    pub server_id: &'a str,
    pub expected_connection: &'a Value,
    pub root: &'a str,
    /// A server id Needle knows, such as `rust-analyzer`.
    pub server: &'a str,
    /// False when the user chose to start without a blocked `.envrc`.
    pub envrc: bool,
}

#[derive(Debug, PartialEq)]
pub enum Event {
    /// A server notification, or one of the worker's `$/newport/*` states.
    Notification { method: String, params: Value },
    /// A request requiring workspace state. Answering never blocks the reader.
    Request(IncomingRequest),
    /// The stream ended; `error` says why when it wasn't a clean stop.
    Closed { error: Option<String> },
}

type Reply = async_channel::Sender<Result<Value, String>>;

struct Inner {
    outbound: async_channel::Sender<Vec<u8>>,
    next_id: AtomicI64,
    configuration: Mutex<Value>,
    /// `None` once the stream ended: no reply can arrive any more.
    pending: Mutex<Option<HashMap<i64, Reply>>>,
    /// Why this side ended the connection, reported by `Event::Closed`.
    failure: Mutex<Option<String>>,
}
impl Inner {
    fn send(&self, message: &Value) {
        if let Ok(body) = serde_json::to_vec(message)
            && let Err(async_channel::TrySendError::Full(_)) = self.outbound.try_send(encode(&body))
        {
            self.failure
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(|| OUTBOUND_OVERFLOW.to_owned());
            // The writer drains what is queued, then ends the server's input.
            self.outbound.close();
        }
    }
    fn pending(&self) -> std::sync::MutexGuard<'_, Option<HashMap<i64, Reply>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl Connection {
    /// Read relay frames from `input` and write raw LSP to `output`.
    /// `folders` answers `workspace/workspaceFolders`.
    pub fn start(
        input: impl Read + Send + 'static,
        output: impl Write + Send + 'static,
        folders: Value,
    ) -> (Self, async_channel::Receiver<Event>) {
        Self::start_bounded(input, output, folders, OUTBOUND_CAPACITY, EVENTS_CAPACITY)
    }

    fn start_bounded(
        input: impl Read + Send + 'static,
        output: impl Write + Send + 'static,
        folders: Value,
        outbound_capacity: usize,
        events_capacity: usize,
    ) -> (Self, async_channel::Receiver<Event>) {
        let (outbound, queued) = async_channel::bounded::<Vec<u8>>(outbound_capacity);
        let (events, received) = async_channel::bounded(events_capacity);
        let inner = Arc::new(Inner {
            outbound,
            next_id: AtomicI64::new(1),
            configuration: Mutex::new(Value::Null),
            pending: Mutex::new(Some(HashMap::new())),
            failure: Mutex::new(None),
        });
        std::thread::spawn(move || {
            let mut output = output;
            while let Ok(bytes) = queued.recv_blocking() {
                if output
                    .write_all(&bytes)
                    .and_then(|()| output.flush())
                    .is_err()
                {
                    break;
                }
            }
            // Dropping the output ends the server's input; it stops on its own.
        });
        let reader = inner.clone();
        std::thread::spawn(move || {
            let exit = Arc::new(Mutex::new(None));
            let mut input = BufReader::new(Frames {
                inner: input,
                remaining: 0,
                exit: exit.clone(),
                done: false,
            });
            let failure = loop {
                match read_message(&mut input) {
                    Ok(Some(body)) => dispatch(&reader, &events, &folders, &body),
                    Ok(None) => break None,
                    Err(error) => break Some(error.to_string()),
                }
            };
            for (_, reply) in reader.pending().take().unwrap_or_default() {
                let _ = reply.try_send(Err(STOPPED.into()));
            }
            reader.outbound.close();
            let error = exit
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .or(failure);
            let error = reader
                .failure
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .or(error);
            let _ = events.try_send(Event::Closed { error });
        });
        (Self { inner }, received)
    }

    /// Settings for this server/worktree, used for section-scoped queries.
    pub fn set_configuration(&self, settings: Value) {
        *self
            .inner
            .configuration
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = settings;
    }

    /// Send a request; the future resolves with its result. Dropping the
    /// future first tells the server to stop working on it.
    pub fn request_value(
        &self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, String>> + Send + 'static {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, response) = async_channel::bounded(1);
        let registered = self
            .inner
            .pending()
            .as_mut()
            .map(|pending| pending.insert(id, reply))
            .is_some();
        if registered {
            self.inner
                .send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        }
        let guard = Pending {
            id,
            inner: Arc::downgrade(&self.inner),
            armed: registered,
        };
        async move {
            // Move the whole guard in; capturing only its `armed` field would
            // drop it, and cancel the request, as soon as this returns.
            let mut guard = guard;
            if !guard.armed {
                return Err(STOPPED.to_owned());
            }
            let result = response
                .recv()
                .await
                .unwrap_or_else(|_| Err(STOPPED.into()));
            guard.armed = false;
            result
        }
    }

    pub fn request<R: lsp_types::request::Request>(
        &self,
        params: R::Params,
    ) -> impl Future<Output = Result<R::Result, String>> + Send + 'static
    where
        R::Result: Send + 'static,
    {
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        let response = self.request_value(R::METHOD, params);
        async move {
            let value = response.await?;
            serde_json::from_value(value).map_err(|error| {
                format!("The language server answered {} oddly: {error}", R::METHOD)
            })
        }
    }

    pub fn notify_value(&self, method: &str, params: Value) {
        self.inner
            .send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        self.notify_value(
            N::METHOD,
            serde_json::to_value(params).unwrap_or(Value::Null),
        );
    }

    /// Ask the server to shut down and exit, then end its input. Nothing waits
    /// for the answer: the worker stops a server that doesn't exit.
    pub fn shutdown(&self) {
        if self.is_closed() {
            return;
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        self.inner
            .send(&json!({"jsonrpc": "2.0", "id": id, "method": "shutdown", "params": null}));
        self.notify_value("exit", Value::Null);
        self.close();
    }

    /// End the server's input. It then stops on its own; the worker stops it
    /// if it doesn't.
    pub fn close(&self) {
        self.inner.outbound.close();
    }

    pub fn is_closed(&self) -> bool {
        self.inner.outbound.is_closed()
    }
}

/// A server request held by the workspace while it validates or loads files.
/// An abandoned request always receives a failure, including on cancellation.
pub struct IncomingRequest {
    id: Option<Value>,
    pub method: String,
    pub params: Value,
    inner: Weak<Inner>,
}
impl std::fmt::Debug for IncomingRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingRequest")
            .field("id", &self.id)
            .field("method", &self.method)
            .field("params", &self.params)
            .finish()
    }
}
impl PartialEq for IncomingRequest {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.method == other.method && self.params == other.params
    }
}
impl IncomingRequest {
    pub fn respond(mut self, result: Result<Value, (i32, String)>) {
        self.send(result);
    }
    fn send(&mut self, result: Result<Value, (i32, String)>) {
        if let Some(id) = self.id.take()
            && let Some(inner) = self.inner.upgrade()
        {
            let response = match result {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err((code, message)) => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
                }
            };
            inner.send(&response);
        }
    }
}
impl Drop for IncomingRequest {
    fn drop(&mut self) {
        if self.method == "workspace/applyEdit" {
            self.send(Ok(json!({"applied":false,"failureReason":"The operation was cancelled or the workspace closed."})));
        } else {
            self.send(Err((
                -32800,
                "The workspace closed or cancelled this request.".into(),
            )));
        }
    }
}

fn configuration_section(settings: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|s| !s.is_empty()) else {
        return settings.clone();
    };
    // Accept both literal dotted keys and nested objects, as editor settings do.
    if let Some(value) = settings.get(section) {
        return value.clone();
    }
    let mut value = settings;
    for key in section.split('.') {
        let Some(next) = value.get(key) else {
            return Value::Null;
        };
        value = next;
    }
    value.clone()
}

/// A request still waiting for its result; dropped early, it cancels.
struct Pending {
    id: i64,
    inner: Weak<Inner>,
    armed: bool,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(inner) = self.inner.upgrade() {
            let removed = inner
                .pending()
                .as_mut()
                .and_then(|pending| pending.remove(&self.id));
            if removed.is_some() {
                inner.send(&json!({"jsonrpc": "2.0", "method": "$/cancelRequest", "params": {"id": self.id}}));
            }
        }
    }
}

fn dispatch(
    inner: &Arc<Inner>,
    events: &async_channel::Sender<Event>,
    folders: &Value,
    body: &[u8],
) {
    let Ok(message) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    let method = message["method"].as_str();
    match (message.get("id"), method) {
        (Some(id), None) => {
            let Some(id) = id.as_i64() else { return };
            let reply = inner
                .pending()
                .as_mut()
                .and_then(|pending| pending.remove(&id));
            if let Some(reply) = reply {
                let result = match message.get("error") {
                    Some(error) if !error.is_null() => Err(error["message"]
                        .as_str()
                        .unwrap_or("The language server reported an error.")
                        .to_owned()),
                    _ => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = reply.try_send(result);
            }
        }
        (Some(id), Some(method)) => {
            if matches!(
                method,
                "workspace/applyEdit" | "client/registerCapability" | "client/unregisterCapability"
            ) {
                // A dropped request still gets its failure reply.
                notify_ui(
                    events,
                    Event::Request(IncomingRequest {
                        id: Some(id.clone()),
                        method: method.into(),
                        params: message.get("params").cloned().unwrap_or(Value::Null),
                        inner: Arc::downgrade(inner),
                    }),
                );
                return;
            }
            let configuration = inner
                .configuration
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let response = match answer(method, &message["params"], folders, &configuration) {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err((code, text)) => {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": text}})
                }
            };
            inner.send(&response);
        }
        (None, Some(method)) => {
            notify_ui(
                events,
                Event::Notification {
                    method: method.to_owned(),
                    params: message.get("params").cloned().unwrap_or(Value::Null),
                },
            );
        }
        (None, None) => {}
    }
}

/// Queue an event unless the UI has fallen behind. Responses never come
/// through here, and the reader is the only sender, so the slot kept free for
/// `Event::Closed` stays free.
fn notify_ui(events: &async_channel::Sender<Event>, event: Event) {
    if events.capacity().is_none_or(|cap| events.len() + 1 < cap) {
        let _ = events.try_send(event);
    }
}

/// Answers to requests a server sends the editor.
fn answer(
    method: &str,
    params: &Value,
    folders: &Value,
    configuration: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "workspace/configuration" => Ok(Value::Array(
            params["items"]
                .as_array()
                .ok_or((-32602, "Expected configuration items.".into()))?
                .iter()
                .map(|item| configuration_section(configuration, item["section"].as_str()))
                .collect(),
        )),
        "workspace/workspaceFolders" => Ok(folders.clone()),
        "window/workDoneProgress/create"
        | "window/showMessageRequest"
        | "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/inlineValue/refresh"
        | "workspace/codeLens/refresh"
        | "workspace/diagnostic/refresh"
        | "workspace/foldingRange/refresh" => Ok(Value::Null),
        "window/showDocument" => Ok(json!({"success": false})),
        _ => Err((-32601, format!("Newport doesn't handle {method}."))),
    }
}

fn encode(body: &[u8]) -> Vec<u8> {
    let mut message = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    message.extend_from_slice(body);
    message
}

/// One message body, or `None` at a clean end of input.
fn read_message(input: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidData, message.to_owned());
    let mut length = None;
    let mut headers = 0;
    loop {
        let mut line = Vec::new();
        if input.by_ref().take(1025).read_until(b'\n', &mut line)? == 0 {
            return if headers == 0 {
                Ok(None)
            } else {
                Err(invalid(
                    "The language server's output ended inside a message.",
                ))
            };
        }
        if !line.ends_with(b"\n") {
            return Err(invalid(
                "The language server sent an invalid message header.",
            ));
        }
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if headers == 0 {
                continue;
            }
            break;
        }
        headers += 1;
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|l| *l <= MAX_MESSAGE);
            if length.is_none() {
                return Err(invalid(
                    "The language server sent an invalid message length.",
                ));
            }
        }
    }
    let length =
        length.ok_or_else(|| invalid("The language server sent a message without a length."))?;
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    Ok(Some(body))
}

/// The server's output carried in relay frames, and the final exit frame.
struct Frames<R> {
    inner: R,
    remaining: usize,
    exit: Arc<Mutex<Option<String>>>,
    done: bool,
}
impl<R: Read> Read for Frames<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        use newport_platform::ipc::protocol::{REMOTE_STREAM_DATA, REMOTE_STREAM_EXIT};
        while self.remaining == 0 {
            if self.done {
                return Ok(0);
            }
            let mut header = [0; 5];
            match self.inner.read_exact(&mut header) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    *self.exit.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some("The connection to the server was lost.".into());
                    self.done = true;
                    return Ok(0);
                }
                Err(error) => return Err(error),
            }
            let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
            match header[0] {
                REMOTE_STREAM_DATA => self.remaining = len,
                REMOTE_STREAM_EXIT if len <= 64 * 1024 => {
                    let mut payload = vec![0; len];
                    self.inner.read_exact(&mut payload)?;
                    let exit: Value = serde_json::from_slice(&payload).unwrap_or_default();
                    *self.exit.lock().unwrap_or_else(|e| e.into_inner()) =
                        exit["error"].as_str().map(str::to_owned);
                    self.done = true;
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Invalid language server stream frame",
                    ));
                }
            }
        }
        let limit = buffer.len().min(self.remaining);
        let read = self.inner.read(&mut buffer[..limit])?;
        if read == 0 {
            self.done = true;
        }
        self.remaining -= read;
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_platform::ipc::protocol::{REMOTE_STREAM_DATA, REMOTE_STREAM_EXIT};

    fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![kind];
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    /// A connection to a fake relay: the test reads what the client wrote and
    /// writes frames the client reads.
    fn connect() -> (
        Connection,
        async_channel::Receiver<Event>,
        BufReader<io::PipeReader>,
        io::PipeWriter,
    ) {
        let (client_input, relay_output) = io::pipe().unwrap();
        let (relay_input, client_output) = io::pipe().unwrap();
        let (connection, events) = Connection::start(
            client_input,
            client_output,
            json!([{"uri": "file:///w", "name": "w"}]),
        );
        (
            connection,
            events,
            BufReader::new(relay_input),
            relay_output,
        )
    }

    fn server_says(relay: &mut io::PipeWriter, message: Value) {
        let body = serde_json::to_vec(&message).unwrap();
        relay
            .write_all(&frame(REMOTE_STREAM_DATA, &encode(&body)))
            .unwrap();
    }

    fn client_said(relay: &mut BufReader<io::PipeReader>) -> Value {
        serde_json::from_slice(&read_message(relay).unwrap().unwrap()).unwrap()
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        futures_lite::future::block_on(future)
    }

    #[test]
    fn requests_resolve_with_their_result_or_error() {
        let (connection, _events, mut from_client, mut to_client) = connect();
        let first = connection.request_value("textDocument/hover", json!({"x": 1}));
        let second = connection.request_value("textDocument/definition", Value::Null);
        let sent = client_said(&mut from_client);
        assert_eq!(
            (sent["method"].as_str(), sent["params"]["x"].as_i64()),
            (Some("textDocument/hover"), Some(1))
        );
        let id = sent["id"].clone();
        let other = client_said(&mut from_client)["id"].clone();
        server_says(
            &mut to_client,
            json!({"jsonrpc": "2.0", "id": other, "error": {"code": -32800, "message": "no"}}),
        );
        server_says(
            &mut to_client,
            json!({"jsonrpc": "2.0", "id": id, "result": {"contents": "doc"}}),
        );
        assert_eq!(block_on(first).unwrap()["contents"], "doc");
        assert_eq!(block_on(second).unwrap_err(), "no");
        // Answered requests are never cancelled.
        connection.notify_value("probe", Value::Null);
        assert_eq!(client_said(&mut from_client)["method"], "probe");
    }

    #[test]
    fn server_requests_get_neutral_answers() {
        let (_connection, _events, mut from_client, mut to_client) = connect();
        server_says(
            &mut to_client,
            json!({"jsonrpc": "2.0", "id": 7, "method": "workspace/configuration", "params": {"items": [{}, {}]}}),
        );
        assert_eq!(
            client_said(&mut from_client),
            json!({"jsonrpc": "2.0", "id": 7, "result": [null, null]})
        );
        server_says(
            &mut to_client,
            json!({"jsonrpc": "2.0", "id": "f", "method": "workspace/workspaceFolders"}),
        );
        assert_eq!(
            client_said(&mut from_client)["result"][0]["uri"],
            "file:///w"
        );
        server_says(
            &mut to_client,
            json!({"jsonrpc": "2.0", "id": 8, "method": "experimental/unknown"}),
        );
        assert_eq!(client_said(&mut from_client)["error"]["code"], -32601);
    }

    #[test]
    fn notifications_arrive_as_events_and_the_end_closes_requests() {
        let (connection, events, mut from_client, mut to_client) = connect();
        server_says(
            &mut to_client,
            json!({"jsonrpc": "2.0", "method": "$/newport/environment", "params": {"state": "loaded"}}),
        );
        assert_eq!(
            block_on(events.recv()).unwrap(),
            Event::Notification {
                method: "$/newport/environment".into(),
                params: json!({"state": "loaded"})
            }
        );
        let waiting = connection.request_value("shutdown", Value::Null);
        client_said(&mut from_client);
        to_client
            .write_all(&frame(REMOTE_STREAM_EXIT, br#"{"error":null}"#))
            .unwrap();
        assert_eq!(block_on(waiting).unwrap_err(), STOPPED);
        assert_eq!(
            block_on(events.recv()).unwrap(),
            Event::Closed { error: None }
        );
        assert!(block_on(connection.request_value("x", Value::Null)).is_err());
    }

    #[test]
    fn a_slow_ui_loses_notifications_but_not_responses_or_the_close() {
        let (client_input, mut relay_output) = io::pipe().unwrap();
        let (relay_input, client_output) = io::pipe().unwrap();
        let (connection, events) = Connection::start_bounded(
            client_input,
            client_output,
            Value::Null,
            OUTBOUND_CAPACITY,
            3,
        );
        let mut from_client = BufReader::new(relay_input);
        let waiting = connection.request_value("textDocument/hover", Value::Null);
        let id = client_said(&mut from_client)["id"].clone();
        for n in 0..10 {
            server_says(
                &mut relay_output,
                json!({"jsonrpc": "2.0", "method": "window/logMessage", "params": {"n": n}}),
            );
        }
        server_says(
            &mut relay_output,
            json!({"jsonrpc": "2.0", "id": id, "result": "doc"}),
        );
        assert_eq!(block_on(waiting).unwrap(), "doc");
        drop(relay_output);
        let mut received = Vec::new();
        while let Ok(event) = block_on(events.recv()) {
            received.push(event);
        }
        assert!(received.len() <= 3);
        assert!(matches!(received.last(), Some(Event::Closed { .. })));
    }

    #[test]
    fn a_server_that_stops_reading_fails_the_connection() {
        let (client_input, relay_output) = io::pipe().unwrap();
        let (relay_input, client_output) = io::pipe().unwrap();
        let (connection, events) =
            Connection::start_bounded(client_input, client_output, Value::Null, 2, EVENTS_CAPACITY);
        // Nobody reads `relay_input`, so the writer stalls once the pipe fills.
        let filler = json!({"text": "x".repeat(256 * 1024)});
        for _ in 0..8 {
            connection.notify_value("textDocument/didChange", filler.clone());
        }
        assert!(connection.is_closed());
        drop(relay_output);
        assert_eq!(
            block_on(events.recv()).unwrap(),
            Event::Closed {
                error: Some(OUTBOUND_OVERFLOW.into())
            }
        );
        drop(relay_input);
    }

    #[test]
    fn a_lost_connection_says_so() {
        let (_connection, events, _from_client, to_client) = connect();
        drop(to_client);
        assert_eq!(
            block_on(events.recv()).unwrap(),
            Event::Closed {
                error: Some("The connection to the server was lost.".into())
            }
        );
    }

    #[test]
    fn dropping_a_pending_request_cancels_it() {
        let (connection, _events, mut from_client, _to_client) = connect();
        let request = connection.request_value("textDocument/completion", Value::Null);
        let id = client_said(&mut from_client)["id"].clone();
        drop(request);
        let cancel = client_said(&mut from_client);
        assert_eq!(cancel["method"], "$/cancelRequest");
        assert_eq!(cancel["params"]["id"], id);
    }

    #[test]
    fn shutting_down_asks_the_server_to_exit_and_ends_its_input() {
        let (connection, _events, mut from_client, _to_client) = connect();
        connection.shutdown();
        assert_eq!(client_said(&mut from_client)["method"], "shutdown");
        assert_eq!(client_said(&mut from_client)["method"], "exit");
        assert!(read_message(&mut from_client).unwrap().is_none());
        assert!(connection.is_closed());
        connection.shutdown();
    }
    #[test]
    fn configuration_queries_read_actual_sections() {
        let (connection, _events, mut output, mut input) = connect();
        connection.set_configuration(
            json!({"python":{"analysis":{"typeCheckingMode":"strict"}}, "literal.key":17}),
        );
        server_says(
            &mut input,
            json!({"jsonrpc":"2.0","id":"settings","method":"workspace/configuration","params":{"items":[{"section":"python.analysis"},{"section":"literal.key"},{"section":"missing"},{}]}}),
        );
        let result = client_said(&mut output)["result"].clone();
        assert_eq!(result[0]["typeCheckingMode"], "strict");
        assert_eq!(result[1], 17);
        assert!(result[2].is_null());
        assert_eq!(
            result[3]["python"]["analysis"]["typeCheckingMode"],
            "strict"
        );
    }
    #[test]
    fn deferred_server_requests_do_not_block_responses_and_always_receive_a_reply() {
        let (connection, events, mut output, mut input) = connect();
        let waiting =
            connection.request_value("workspace/executeCommand", json!({"command":"fix"}));
        let id = client_said(&mut output)["id"].clone();
        server_says(
            &mut input,
            json!({"jsonrpc":"2.0","id":"edit","method":"workspace/applyEdit","params":{"edit":{}}}),
        );
        let Event::Request(request) = events.recv_blocking().unwrap() else {
            panic!("missing request");
        };
        server_says(&mut input, json!({"jsonrpc":"2.0","id":id,"result":null}));
        assert!(block_on(waiting).is_ok());
        drop(request);
        let reply = client_said(&mut output);
        assert_eq!(reply["id"], "edit");
        assert_eq!(reply["result"]["applied"], false);
        server_says(
            &mut input,
            json!({"jsonrpc":"2.0","id":"register","method":"client/registerCapability","params":{"registrations":[]}}),
        );
        let Event::Request(request) = events.recv_blocking().unwrap() else {
            panic!("missing registration");
        };
        request.respond(Ok(Value::Null));
        assert_eq!(
            client_said(&mut output),
            json!({"jsonrpc":"2.0","id":"register","result":null})
        );
    }
}
