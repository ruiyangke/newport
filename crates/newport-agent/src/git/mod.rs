//! Git service execution and acknowledged diff delivery.
mod backend;
mod bootstrap;
mod cli;
mod cloning;
mod command_log;
mod journal;
mod metrics;
use newport_protocol::git as protocol;
mod remotes;
mod tokens;
use backend::Output;
use protocol::{
    Begin, Binary, Chunk, ClientMessage, Error, Log, Ready, Reply, Response, ServerMessage,
    StreamComplete,
    wire::{self, Message},
};
use std::{
    io::{self, Write},
    sync::mpsc,
    time::Duration,
};
use uuid::Uuid;

pub(super) use crate::paths::runtime as temporary_directory;

pub fn serve() -> io::Result<()> {
    serve_bound(crate::transport::Cancellation::default())
}
pub(crate) fn serve_bound(cancellation: crate::transport::Cancellation) -> io::Result<()> {
    use std::os::fd::AsFd;
    let stdout = std::fs::File::from(io::stdout().as_fd().try_clone_to_owned()?);
    crate::transport::set_nonblocking(stdout.as_fd())?;
    let mut output = crate::transport::DeadlineWriter {
        inner: stdout,
        timeout: Duration::from_secs(5),
        cancellation: cancellation.clone(),
    };
    let (tx, rx) = mpsc::sync_channel(1);
    let mut stdin = cancellation.stdin()?;
    std::thread::spawn(move || {
        loop {
            let message = wire::read::<ClientMessage>(&mut stdin).map_err(io::Error::from);
            let failed = message.is_err();
            if tx.send(message).is_err() || failed {
                break;
            }
        }
    });
    run(
        |timeout| {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                cancellation.check()?;
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Git RPC input timed out",
                    ));
                }
                match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                    Ok(message) => {
                        cancellation.check()?;
                        return message;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                    Err(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "Git RPC input closed",
                        ));
                    }
                }
            }
        },
        &mut output,
        std::env::var_os("HOME")
            .map(|home| std::path::PathBuf::from(home).join(".local/state/newport/git")),
    )
}
fn run(
    mut receive: impl FnMut(Duration) -> io::Result<ClientMessage>,
    output: &mut impl Write,
    journal_root: Option<std::path::PathBuf>,
) -> io::Result<()> {
    let mut metrics = metrics::Metrics::from_env();
    let ClientMessage::Configure(configure) = receive(Duration::from_secs(5))? else {
        return Err(io::Error::other("Expected Git client configuration"));
    };
    let mut service = match backend::Backend::new(|| {
        journal_root.and_then(|root| journal::Journal::open(root, configure.client_id).ok())
    }) {
        Ok(service) => service,
        Err(error) => {
            wire::write(
                output,
                &ServerMessage::Failed(Error::new("GIT_UNAVAILABLE", error.to_string())),
            )?;
            return Ok(());
        }
    };
    let methods: Vec<String> = service
        .methods()
        .iter()
        .copied()
        .filter(|method| {
            service.writable()
                || (!method.starts_with("operation.")
                    && *method != "repo.init"
                    && *method != "repo.clone")
        })
        .map(str::to_owned)
        .collect();
    wire::write(
        output,
        &ServerMessage::Ready(Ready {
            id: configure.id,
            limits: protocol::Limits::default(),
            capabilities: service.capabilities(methods.clone()),
        }),
    )?;
    loop {
        let message = match receive(Duration::from_secs(45)) {
            Ok(message) => message,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        match message {
            ClientMessage::Ping(nonce) => wire::write(output, &ServerMessage::Pong(nonce))?,
            ClientMessage::Call(call) => {
                command_log::begin(configure.command_logs);
                let started = metrics.start();
                let result = if methods.iter().any(|method| method == call.request.method()) {
                    service.request(*call.request)
                } else {
                    Err(Error::new(
                        "UNSUPPORTED_METHOD",
                        "This Git method is not available.",
                    ))
                };
                // Measure execution before encoding and stream acknowledgement waits.
                metrics.finish(&call.id, started);
                for entry in command_log::take() {
                    wire::write(
                        output,
                        &ServerMessage::Log(Log {
                            id: call.id.clone(),
                            entry,
                        }),
                    )?;
                }
                match result {
                    Err(error) => failure(output, call.id, error)?,
                    Ok(Output::Reply(result)) => {
                        let reply = ServerMessage::Response(Response {
                            id: call.id.clone(),
                            result: Ok(result),
                        });
                        match reply.encode() {
                            Ok(frame) => {
                                output.write_all(&frame)?;
                                output.flush()?;
                            }
                            Err(error) => failure(
                                output,
                                call.id,
                                Error::new(
                                    "LIMIT_EXCEEDED",
                                    format!("Git response could not be encoded: {error}"),
                                ),
                            )?,
                        }
                    }
                    Ok(Output::Diff { snapshot, diff }) => match wire::encode_diff(&diff) {
                        Ok(bytes) => stream_diff(&mut receive, output, call.id, snapshot, &bytes)?,
                        Err(error) => failure(
                            output,
                            call.id,
                            Error::new(
                                "LIMIT_EXCEEDED",
                                format!("Diff could not be encoded: {error}. Use paginated reads."),
                            ),
                        )?,
                    },
                }
            }
            _ => return Err(io::Error::other("Unexpected Git client message")),
        }
    }
}
fn failure(output: &mut impl Write, id: String, error: Error) -> io::Result<()> {
    wire::write(
        output,
        &ServerMessage::Response(Response {
            id,
            result: Err(error),
        }),
    )
    .map_err(Into::into)
}

/// Bound outstanding data without waiting for a round trip after each chunk.
fn stream_diff(
    receive: &mut impl FnMut(Duration) -> io::Result<ClientMessage>,
    output: &mut impl Write,
    id: String,
    snapshot: String,
    bytes: &[u8],
) -> io::Result<()> {
    let stream_id = Uuid::new_v4().to_string();
    wire::write(
        output,
        &ServerMessage::Begin(Begin {
            id: id.clone(),
            stream_id: stream_id.clone(),
            snapshot,
        }),
    )?;
    let mut chunks = bytes.chunks(protocol::CHUNK_SIZE);
    let mut sent = 0u32;
    let mut acknowledged = 0u32;
    loop {
        while sent - acknowledged < protocol::STREAM_WINDOW {
            let Some(chunk) = chunks.next() else {
                break;
            };
            sent += 1;
            wire::write(
                output,
                &ServerMessage::Chunk(Chunk {
                    id: id.clone(),
                    stream_id: stream_id.clone(),
                    seq: sent,
                    data: Binary(chunk.to_vec()),
                }),
            )?;
        }
        if acknowledged == sent {
            break;
        }
        match receive(Duration::from_secs(30))? {
            ClientMessage::Ack(protocol::Ack {
                stream_id: ack,
                seq,
            }) if ack == stream_id && seq == acknowledged + 1 => acknowledged = seq,
            ClientMessage::Ping(nonce) => wire::write(output, &ServerMessage::Pong(nonce))?,
            _ => return Err(io::Error::other("Invalid Git stream acknowledgement")),
        }
    }
    wire::write(
        output,
        &ServerMessage::Response(Response {
            id,
            result: Ok(Reply::StreamComplete(Box::new(StreamComplete {
                last_seq: sent,
                bytes: bytes.len() as u64,
            }))),
        }),
    )?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Default)]
    struct Capture(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Capture {
        fn messages(&self) -> Vec<ServerMessage> {
            let bytes = self.0.borrow();
            let mut input = bytes.as_slice();
            let mut messages = Vec::new();
            while !input.is_empty() {
                messages.push(wire::read(&mut input).unwrap());
            }
            messages
        }
    }

    #[test]
    fn streaming_bounds_outstanding_chunks_and_waits_for_final_ack() {
        let mut output = Capture::default();
        let capture = output.clone();
        let total = protocol::STREAM_WINDOW * 2 + 3;
        let bytes = vec![b'x'; protocol::CHUNK_SIZE * (total as usize - 1) + 7];
        let mut acknowledged = 0;
        let mut pinged = false;
        stream_diff(
            &mut |_| {
                let messages = capture.messages();
                assert!(
                    !messages
                        .iter()
                        .any(|m| matches!(m, ServerMessage::Response(..)))
                );
                let chunks: Vec<_> = messages
                    .iter()
                    .filter_map(|message| {
                        if let ServerMessage::Chunk(Chunk { stream_id, seq, .. }) = message {
                            Some((stream_id, seq))
                        } else {
                            None
                        }
                    })
                    .collect();
                assert_eq!(
                    chunks.len() as u32,
                    (acknowledged + protocol::STREAM_WINDOW).min(total)
                );
                if !pinged {
                    pinged = true;
                    return Ok(ClientMessage::Ping("alive".into()));
                }
                acknowledged += 1;
                Ok(ClientMessage::Ack(protocol::Ack {
                    stream_id: chunks[0].0.clone(),
                    seq: acknowledged,
                }))
            },
            &mut output,
            Uuid::new_v4().to_string(),
            "snapshot".into(),
            &bytes,
        )
        .unwrap();
        assert_eq!(acknowledged, total);
        let messages = capture.messages();
        let reconstructed: Vec<_> = messages
            .iter()
            .filter_map(|message| {
                if let ServerMessage::Chunk(Chunk { data, .. }) = message {
                    Some(data.0.clone())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        assert_eq!(reconstructed, bytes);
        assert!(
            messages
                .iter()
                .any(|m| matches!(m, ServerMessage::Pong(nonce) if nonce == "alive"))
        );
        assert!(
            matches!(messages.last().unwrap(), ServerMessage::Response(Response { result: Ok(Reply::StreamComplete(value)), .. }) if value.last_seq == total && value.bytes == bytes.len() as u64)
        );
    }

    #[test]
    fn streaming_rejects_skipped_duplicate_or_wrong_stream_acks() {
        for mode in ["skipped", "duplicate", "wrong-stream", "timeout"] {
            let mut output = Capture::default();
            let capture = output.clone();
            let mut calls = 0;
            let result = stream_diff(
                &mut |_| {
                    calls += 1;
                    let messages = capture.messages();
                    let ServerMessage::Begin(Begin { stream_id, .. }) = &messages[0] else {
                        panic!()
                    };
                    if mode == "timeout" {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    Ok(ClientMessage::Ack(protocol::Ack {
                        stream_id: if mode == "wrong-stream" {
                            "other".into()
                        } else {
                            stream_id.clone()
                        },
                        seq: if mode == "skipped" { 2 } else { 1 },
                    }))
                },
                &mut output,
                Uuid::new_v4().to_string(),
                "snapshot".into(),
                &vec![b'x'; protocol::CHUNK_SIZE * 5],
            );
            assert!(result.is_err());
            assert_eq!(calls, if mode == "duplicate" { 2 } else { 1 });
            assert!(
                !capture
                    .messages()
                    .iter()
                    .any(|m| matches!(m, ServerMessage::Response(..)))
            );
        }
    }

    fn configure(command_logs: bool) -> ClientMessage {
        ClientMessage::Configure(protocol::Configure {
            id: Uuid::new_v4().to_string(),
            client_id: Uuid::new_v4().to_string(),
            client_version: "test".into(),
            command_logs,
        })
    }
    #[test]
    fn configuration_executes_repository_reads_and_correlated_logs() {
        use std::os::unix::ffi::OsStrExt;
        let repo = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--template=", "--initial-branch=main"])
                .arg(repo.path())
                .output()
                .unwrap()
                .status
                .success()
        );
        let id = Uuid::new_v4().to_string();
        let mut messages = vec![
            configure(true),
            ClientMessage::Call(protocol::Call {
                id: id.clone(),
                request: Box::new(protocol::Request::Open {
                    path: protocol::Path::new(repo.path().as_os_str().as_bytes()),
                }),
            }),
            ClientMessage::Ping("alive".into()),
        ]
        .into_iter();
        let mut output = Capture::default();
        run(
            |_| {
                messages
                    .next()
                    .ok_or_else(|| io::ErrorKind::UnexpectedEof.into())
            },
            &mut output,
            None,
        )
        .unwrap();
        let messages = output.messages();
        assert!(matches!(&messages[0], ServerMessage::Ready(..)));
        let mut logs = 0;
        for message in &messages[1..messages.len() - 1] {
            match message {
                ServerMessage::Log(Log { id: reply, .. }) => {
                    assert_eq!(reply, &id);
                    logs += 1;
                }
                ServerMessage::Response(Response {
                    id: reply,
                    result: Ok(Reply::Open(value)),
                }) => {
                    assert_eq!(reply, &id);
                    assert!(!value.bare);
                    assert!(!value.repo_id.is_empty());
                }
                other => panic!("Unexpected {other:?}"),
            }
        }
        assert!(logs > 0);
        assert!(matches!(messages.last().unwrap(), ServerMessage::Pong(nonce) if nonce == "alive"));
    }
    #[test]
    fn read_only_configuration_excludes_mutations_and_rejects_operation_calls() {
        assert!(serde_json::from_value::<protocol::Request>(serde_json::json!({"method":"repo.open","params":{"path":protocol::Path::new(b"/tmp"),"execute":"bad"}})).is_err());
        let mut input = vec![
            configure(false),
            ClientMessage::Call(protocol::Call {
                id: Uuid::new_v4().to_string(),
                request: Box::new(protocol::Request::Get {
                    operation_id: Uuid::new_v4().to_string(),
                }),
            }),
        ]
        .into_iter();
        let mut output = Capture::default();
        run(
            |_| {
                input
                    .next()
                    .ok_or_else(|| io::ErrorKind::UnexpectedEof.into())
            },
            &mut output,
            None,
        )
        .unwrap();
        let messages = output.messages();
        let ServerMessage::Ready(Ready { capabilities, .. }) = &messages[0] else {
            panic!()
        };
        assert!(capabilities.actions.is_empty());
        assert!(
            !capabilities
                .methods
                .iter()
                .any(|method| method.starts_with("operation."))
        );
        assert!(
            matches!(&messages[1], ServerMessage::Response(Response { result: Err(Error { code, .. }), .. }) if code == "UNSUPPORTED_METHOD")
        );
    }
}
