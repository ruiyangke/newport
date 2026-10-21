//! Language servers through the Needle `lsp` worker on a real Linux server:
//! the login and direnv environment, direnv's allow list, change detection,
//! and stopping everything a server started.
use super::{installed_server, owner};
use crate::{agent::Service, ssh::ExecSession};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    time::{Duration, timeout},
};

/// A minimal server, installed as `gopls`: it answers initialize, hover
/// (with `LSP_GREETING` and its working directory) and shutdown, and leaves
/// a `sleep` running that only a process-group stop can end.
const FAKE_SERVER: &str = r#"#!/bin/bash
export LC_ALL=C
sleep 300 &
while IFS= read -r line; do
  line=${line%$'\r'}
  case "$line" in
    Content-Length:*) length=${line#Content-Length: } ;;
    "")
      IFS= read -r -N "$length" body
      id=$(printf '%s' "$body" | grep -o '"id":[0-9]*' | head -n 1 | cut -d: -f2)
      case "$body" in
        *'"method":"initialize"'*) result='{"capabilities":{"positionEncoding":"utf-32","hoverProvider":true}}' ;;
        *'"method":"textDocument/hover"'*) result="{\"contents\":\"${LSP_GREETING:-none} $(pwd)\"}" ;;
        *'"method":"shutdown"'*) result='null' ;;
        *'"method":"exit"'*) exit 0 ;;
        *) continue ;;
      esac
      [ -n "$id" ] || continue
      response="{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":$result}"
      printf 'Content-Length: %d\r\n\r\n%s' "${#response}" "$response"
      ;;
  esac
done
"#;

struct Lsp<S> {
    input: BufReader<tokio::io::ReadHalf<S>>,
    output: tokio::io::WriteHalf<S>,
    /// For a failure report: what arrived, and where to list processes.
    received: Vec<String>,
    server: crate::model::Server,
}
impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite> Lsp<S> {
    fn new(stream: S, server: crate::model::Server) -> Self {
        let (input, output) = tokio::io::split(stream);
        Self {
            input: BufReader::new(input),
            output,
            received: Vec::new(),
            server,
        }
    }
    async fn send(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).unwrap();
        let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        bytes.extend_from_slice(&body);
        self.output.write_all(&bytes).await.unwrap();
        self.output.flush().await.unwrap();
    }
    /// The next message, or `None` once the worker ends the stream.
    async fn receive(&mut self) -> Option<Value> {
        let next = timeout(Duration::from_secs(60), async {
            let mut length = None;
            loop {
                let mut line = String::new();
                if self.input.read_line(&mut line).await.unwrap() == 0 {
                    return None;
                }
                let line = line.trim_end();
                if line.is_empty() && length.is_some() {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length: ") {
                    length = Some(value.parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; length.unwrap()];
            self.input.read_exact(&mut body).await.unwrap();
            Some(serde_json::from_slice::<Value>(&body).unwrap())
        })
        .await;
        match next {
            Ok(message) => {
                self.received.extend(message.as_ref().map(Value::to_string));
                message
            }
            Err(_) => {
                let session = ExecSession::connect(&self.server).await.unwrap();
                let processes =
                    run(&session, "ps -u \"$(id -u)\" -o pid,pgid,stat,etime,args").await;
                panic!(
                    "No message within a minute.\nReceived: {:#?}\nProcesses:\n{processes}",
                    self.received
                );
            }
        }
    }
    /// The params of the next worker notification for `method`.
    async fn state(&mut self, method: &str) -> Value {
        loop {
            let message = self.receive().await.expect("stream ended early");
            if message["method"] == method {
                return message["params"].clone();
            }
        }
    }
    async fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let message = self.receive().await.expect("stream ended early");
            if message["id"] == id {
                return message["result"].clone();
            }
        }
    }
    /// Initialize, then hover: the server's greeting and working directory.
    async fn greeting(&mut self) -> String {
        self.request(1, "initialize", json!({"capabilities": {}}))
            .await;
        let hover = self
            .request(
                2,
                "textDocument/hover",
                json!({"textDocument": {"uri": "file:///x"}, "position": {"line": 0, "character": 0}}),
            )
            .await;
        hover["contents"].as_str().unwrap().to_owned()
    }
    async fn stop(&mut self) -> Value {
        self.request(3, "shutdown", Value::Null).await;
        self.send(json!({"jsonrpc": "2.0", "method": "exit"})).await;
        let exited = self.state("$/newport/server").await;
        assert!(
            self.receive().await.is_none(),
            "the stream ends after the server"
        );
        exited
    }
}

async fn run(session: &ExecSession, command: &str) -> String {
    session.execute(command, None).await.unwrap()
}

#[tokio::test]
#[ignore = "Requires cargo xtask test remote and real OpenSSH"]
async fn language_servers_start_in_the_worktree_environment() {
    let server = installed_server("NEWPORT_TEST_SSH_PORT", "Language servers", "fixture").await;
    let session = ExecSession::connect(&server).await.unwrap();
    let home = run(&session, "printf %s \"$HOME\"").await;
    let root = format!("{home}/lsp fixture/app");
    run(
        &session,
        "rm -rf ~/'lsp fixture' && mkdir -p ~/'lsp fixture/app' ~/.local/bin",
    )
    .await;
    session
        .execute(
            "cat > ~/.local/bin/gopls && chmod +x ~/.local/bin/gopls",
            Some(FAKE_SERVER.as_bytes()),
        )
        .await
        .unwrap();
    let owner = owner(&server);
    let launch = |envrc: bool, id: &'static str| {
        let (owner, server, root) = (owner.clone(), server.clone(), root.clone());
        async move {
            let lease = owner.acquire(&server).await.unwrap();
            let (stream, _) = lease
                .launch(Service::Lsp {
                    root,
                    server: id.into(),
                    envrc,
                })
                .await
                .unwrap();
            (lease, Lsp::new(stream, server))
        }
    };

    // No .envrc: the login environment, in the worktree.
    let (_lease, mut lsp) = launch(true, "gopls").await;
    assert_eq!(lsp.state("$/newport/environment").await["state"], "none");
    assert_eq!(lsp.state("$/newport/server").await["state"], "started");
    assert_eq!(lsp.greeting().await, format!("none {root}"));
    assert_eq!(lsp.stop().await["state"], "exited");
    // The worker stopped what the server left behind.
    tokio::time::sleep(Duration::from_millis(500)).await;
    // `slee[p]` keeps this shell's own command line from matching.
    assert_eq!(
        run(&session, "pgrep -u \"$(id -u)\" -f 'slee[p] 300' | wc -l")
            .await
            .trim(),
        "0"
    );

    // A server that isn't installed says so.
    let (_lease, mut lsp) = launch(true, "rust-analyzer").await;
    assert_eq!(lsp.state("$/newport/server").await["state"], "missing");
    assert!(lsp.receive().await.is_none());

    // direnv blocks an .envrc nobody allowed; nothing starts.
    run(
        &session,
        "printf 'export LSP_GREETING=from-envrc\\n' > ~/'lsp fixture/.envrc'",
    )
    .await;
    let envrc = format!("{home}/lsp fixture/.envrc");
    let (_lease, mut lsp) = launch(true, "gopls").await;
    let blocked = lsp.state("$/newport/environment").await;
    assert_eq!(
        (blocked["state"].as_str(), blocked["path"].as_str()),
        (Some("blocked"), Some(envrc.as_str()))
    );
    assert!(lsp.receive().await.is_none());

    // The user may start without it.
    let (_lease, mut lsp) = launch(false, "gopls").await;
    assert_eq!(lsp.state("$/newport/environment").await["state"], "skipped");
    assert_eq!(lsp.greeting().await, format!("none {root}"));
    lsp.stop().await;

    // Once allowed, the server gets its variables, and an edit is noticed.
    run(&session, "cd ~/'lsp fixture/app' && direnv allow").await;
    let (_lease, mut lsp) = launch(true, "gopls").await;
    assert_eq!(lsp.state("$/newport/environment").await["state"], "loading");
    assert_eq!(lsp.state("$/newport/environment").await["state"], "loaded");
    assert_eq!(lsp.greeting().await, format!("from-envrc {root}"));
    run(
        &session,
        "sleep 1 && printf 'export OTHER=1\\n' >> ~/'lsp fixture/.envrc'",
    )
    .await;
    assert_eq!(lsp.state("$/newport/environment").await["state"], "changed");
    lsp.stop().await;

    run(
        &session,
        "cd ~/'lsp fixture/app' && direnv deny; rm -rf ~/'lsp fixture' ~/.local/bin/gopls",
    )
    .await;
    session.close().await;
}
