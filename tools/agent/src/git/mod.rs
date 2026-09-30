//! Dedicated Git RPC channel; independent of clipboard/browser v5.
mod backend;
mod bootstrap;
mod branches;
mod checkout;
mod cli;
mod cloning;
mod command_log;
mod config_keys;
mod conflicts;
mod credentials;
mod discard;
mod hunks;
mod integration;
mod journal;
mod metrics;
mod operations;
pub mod protocol;
mod rebase;
mod remote_rename;
mod remotes;
mod replay;
mod repository;
mod reset;
mod stash;
mod tags;
mod tokens;
mod worktrees;
use backend::Output;
use base64::{engine::general_purpose::STANDARD, Engine};
use protocol::{Error, Message};
use serde_json::json;
use std::{
    io::{self, Write},
    sync::mpsc,
    time::Duration,
};
use uuid::Uuid;

pub fn serve() -> io::Result<()> {
    use std::os::fd::{AsFd, AsRawFd};
    let stdout = std::fs::File::from(io::stdout().as_fd().try_clone_to_owned()?);
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut output = crate::transport::DeadlineWriter {
        inner: stdout,
        timeout: Duration::from_secs(5),
    };
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        loop {
            let message = protocol::read(&mut stdin);
            let failed = message.is_err();
            if tx.send(message).is_err() || failed {
                break;
            }
        }
    });
    run(
        |timeout| {
            rx.recv_timeout(timeout)
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Git RPC input timed out"))?
        },
        &mut output,
        std::env::var_os("HOME")
            .map(|home| std::path::PathBuf::from(home).join(".local/state/newport/git")),
    )
}
fn run(
    mut receive: impl FnMut(Duration) -> io::Result<Message>,
    output: &mut impl Write,
    journal_root: Option<std::path::PathBuf>,
) -> io::Result<()> {
    let mut metrics = metrics::Metrics::from_env();
    protocol::write(
        output,
        &Message::Hello {
            protocol: "newport.git".into(),
            versions: vec![protocol::VERSION],
            instance_id: Uuid::new_v4().to_string(),
            limits: json!({"maxFrameBytes":protocol::MAX_FRAME,"commandLogs":true,"maxInflight":1,"maxPageItems":200,"maxChunkBytes":protocol::CHUNK_SIZE,"streamWindow":protocol::STREAM_WINDOW}),
        },
    )?;
    let init = receive(Duration::from_secs(5))?;
    let Message::Initialize {
        id,
        version,
        client_id,
        client_version,
        command_logs,
    } = init
    else {
        return Err(io::Error::other("Expected Git protocol initialization"));
    };
    if version != protocol::VERSION
        || Uuid::parse_str(&id).is_err()
        || Uuid::parse_str(&client_id).is_err()
        || client_version.len() > 64
    {
        protocol::write(
            output,
            &Message::failure(
                id,
                Error::new(
                    "UNSUPPORTED_PROTOCOL",
                    "Unsupported or invalid initialization.",
                ),
            ),
        )?;
        return Ok(());
    }
    let mut service = backend::Backend::from_env(|| {
        journal_root.and_then(|root| journal::Journal::open(root, client_id).ok())
    })?;
    let methods: Vec<_> = service
        .methods()
        .iter()
        .copied()
        .filter(|m| {
            service.writable()
                || (!m.starts_with("operation.") && *m != "repo.init" && *m != "repo.clone")
        })
        .collect();
    protocol::write(
        output,
        &Message::Ready {
            id,
            version,
            capabilities: service.capabilities(json!({"methods":methods,"actions":if service.writable(){vec!["stage","unstage","commit","conflict.resolve","commit.amend","branch.create","branch.rename","branch.delete","branch.set_upstream","checkout","remote.add","remote.rename","remote.set_url","remote.remove","fetch","push","push.with_lease","branch.delete_remote","worktree.add","worktree.remove","worktree.repair","worktree.prune","worktree.lock","worktree.unlock","tag.delete_remote","merge.fast_forward","pull.fast_forward","merge","merge.abort","stash.save","stash.apply","stash.pop","stash.drop","tag.create","tag.delete","tag.push","cherry_pick","revert","integration.continue","integration.abort","rebase","integration.skip","reset","discard"]}else{vec![]},"features":["status_summary.path","remote_refs.filter","status.filter","index.hunks","index.lines","discard.hunks","stash.entry_index","worktree.new_branch","diff.streaming","diff.tuple_v1","paths.bytes","history.snapshot_pagination","branches.filter","worktrees.filter","worktrees.snapshot_filter","history.summary","tags.summary"],"objectFormats":["sha1","sha256"]})),
        },
    )?;
    loop {
        let message = match receive(Duration::from_secs(45)) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        match message {
            Message::Ping { nonce } if nonce.len() <= 128 => {
                protocol::write(output, &Message::Pong { nonce })?
            }
            Message::Request { id, method, params } => {
                if Uuid::parse_str(&id).is_err() {
                    protocol::write(
                        output,
                        &Message::failure(id, Error::invalid("Request ID must be a UUID.")),
                    )?;
                    continue;
                }
                command_log::begin(command_logs);
                let started = metrics.start();
                let result = if !methods.contains(&method.as_str()) {
                    Err(Error::new(
                        "UNSUPPORTED_METHOD",
                        "This Git method is not available.",
                    ))
                } else {
                    serde_json::from_value(json!({"method":method,"params":params}))
                        .map_err(|_| Error::invalid("Invalid method parameters."))
                        .and_then(|r| service.request(r))
                };
                // Excludes response encoding, transport, and stream ACK waits.
                // Write the opt-in timing before the reply so a benchmark can
                // correlate it immediately after receiving that reply.
                metrics.finish(&id, started);
                for entry in command_log::take() {
                    protocol::write(
                        output,
                        &Message::CommandLog {
                            id: id.clone(),
                            entry,
                        },
                    )?;
                }
                match result {
                    Err(error) => protocol::write(output, &Message::failure(id, error))?,
                    Ok(Output::Json(value)) => {
                        let reply = Message::success(id.clone(), value);
                        match protocol::encode(&reply) {
                            Ok(frame) => {
                                // Reuse the frame that passed the size check;
                                // serializing a second time doubles this work.
                                output.write_all(&frame)?;
                                output.flush()?;
                            }
                            Err(_) => protocol::write(
                                output,
                                &Message::failure(
                                    id,
                                    Error::new(
                                        "LIMIT_EXCEEDED",
                                        "Response exceeds the frame limit.",
                                    ),
                                ),
                            )?,
                        }
                    }
                    Ok(Output::Diff { snapshot, bytes }) => {
                        stream_diff(&mut receive, output, id, snapshot, &bytes)?;
                    }
                }
            }
            _ => return Err(io::Error::other("Unexpected Git protocol message")),
        }
    }
}

/// Bound outstanding data while avoiding a network round trip for each chunk.
/// Existing clients still acknowledge every chunk in order; no new messages
/// or acknowledgement semantics are required.
fn stream_diff(
    receive: &mut impl FnMut(Duration) -> io::Result<Message>,
    output: &mut impl Write,
    id: String,
    snapshot: String,
    bytes: &[u8],
) -> io::Result<()> {
    let stream_id = Uuid::new_v4().to_string();
    protocol::write(
        output,
        &Message::Begin {
            id: id.clone(),
            stream_id: stream_id.clone(),
            snapshot,
        },
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
            protocol::write(
                output,
                &Message::Chunk {
                    id: id.clone(),
                    stream_id: stream_id.clone(),
                    seq: sent,
                    bytes_b64: STANDARD.encode(chunk),
                },
            )?;
        }
        if acknowledged == sent {
            break;
        }
        match receive(Duration::from_secs(30))? {
            Message::Ack {
                stream_id: ack,
                seq,
            } if ack == stream_id && seq == acknowledged + 1 => acknowledged = seq,
            Message::Ping { nonce } if nonce.len() <= 128 => {
                protocol::write(output, &Message::Pong { nonce })?
            }
            _ => return Err(io::Error::other("Invalid Git stream acknowledgement")),
        }
    }
    protocol::write(
        output,
        &Message::success(id, json!({"lastSeq":sent,"bytes":bytes.len()})),
    )
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
        fn messages(&self) -> Vec<Message> {
            let bytes = self.0.borrow();
            let mut input = bytes.as_slice();
            let mut messages = Vec::new();
            while !input.is_empty() {
                messages.push(protocol::read(&mut input).unwrap());
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
                assert!(!messages
                    .iter()
                    .any(|m| matches!(m, Message::Response { .. })));
                let chunks: Vec<_> = messages
                    .iter()
                    .filter_map(|message| {
                        if let Message::Chunk { stream_id, seq, .. } = message {
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
                    return Ok(Message::Ping {
                        nonce: "alive".into(),
                    });
                }
                acknowledged += 1;
                Ok(Message::Ack {
                    stream_id: chunks[0].0.clone(),
                    seq: acknowledged,
                })
            },
            &mut output,
            "request".into(),
            "snapshot".into(),
            &bytes,
        )
        .unwrap();
        assert_eq!(acknowledged, total);
        let messages = capture.messages();
        let reconstructed: Vec<_> = messages
            .iter()
            .filter_map(|message| {
                if let Message::Chunk { bytes_b64, .. } = message {
                    Some(STANDARD.decode(bytes_b64).unwrap())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        assert_eq!(reconstructed, bytes);
        assert!(messages
            .iter()
            .any(|m| matches!(m, Message::Pong { nonce } if nonce == "alive")));
        assert!(
            matches!(messages.last().unwrap(), Message::Response { result: Some(value), .. } if value["lastSeq"] == total && value["bytes"] == bytes.len())
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
                    let Message::Begin { stream_id, .. } = &messages[0] else {
                        panic!()
                    };
                    if mode == "timeout" {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    Ok(Message::Ack {
                        stream_id: if mode == "wrong-stream" {
                            "other".into()
                        } else {
                            stream_id.clone()
                        },
                        seq: if mode == "skipped" { 2 } else { 1 },
                    })
                },
                &mut output,
                "request".into(),
                "snapshot".into(),
                &vec![b'x'; protocol::CHUNK_SIZE * 5],
            );
            assert!(result.is_err());
            assert_eq!(calls, if mode == "duplicate" { 2 } else { 1 });
            assert!(!capture
                .messages()
                .iter()
                .any(|m| matches!(m, Message::Response { .. })));
        }
    }

    #[test]
    fn handshake_rejects_mutations_and_unknown_params() {
        let mut input = vec![
            Message::Initialize {
                command_logs: false,
                id: Uuid::new_v4().to_string(),
                version: 1,
                client_id: Uuid::new_v4().to_string(),
                client_version: "test".into(),
            },
            Message::Request {
                id: Uuid::new_v4().to_string(),
                method: "operation.start".into(),
                params: json!({}),
            },
            Message::Request {
                id: Uuid::new_v4().to_string(),
                method: "repo.open".into(),
                params: json!({"path":protocol::Path::new(b"/tmp"),"execute":"bad"}),
            },
        ]
        .into_iter();
        let mut bytes = Vec::new();
        run(
            |_| {
                input
                    .next()
                    .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))
            },
            &mut bytes,
            None,
        )
        .unwrap();
        let mut input = bytes.as_slice();
        assert!(matches!(
            protocol::read(&mut input).unwrap(),
            Message::Hello { .. }
        ));
        assert!(matches!(
            protocol::read(&mut input).unwrap(),
            Message::Ready { .. }
        ));
        assert!(
            matches!(protocol::read(&mut input).unwrap(),Message::Response { error:Some(Error { code,.. }),.. } if code=="UNSUPPORTED_METHOD")
        );
        assert!(
            matches!(protocol::read(&mut input).unwrap(),Message::Response { error:Some(Error { code,.. }),.. } if code=="INVALID_REQUEST")
        );
    }
}
