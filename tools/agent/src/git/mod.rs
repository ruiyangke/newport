//! Dedicated Git RPC channel; independent of clipboard/browser v5.
mod bootstrap;
mod branches;
mod checkout;
mod cloning;
mod conflicts;
mod discard;
mod hunks;
mod integration;
mod journal;
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
use base64::{engine::general_purpose::STANDARD, Engine};
use protocol::{Error, Message};
use repository::{Output, Service};
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
    protocol::write(
        output,
        &Message::Hello {
            protocol: "newport.git".into(),
            versions: vec![protocol::VERSION],
            instance_id: Uuid::new_v4().to_string(),
            limits: json!({"maxFrameBytes":protocol::MAX_FRAME,"maxInflight":1,"maxPageItems":200,"maxChunkBytes":protocol::CHUNK_SIZE,"streamWindow":1}),
        },
    )?;
    let init = receive(Duration::from_secs(5))?;
    let Message::Initialize {
        id,
        version,
        client_id,
        client_version,
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
    let mut service = journal_root
        .and_then(|root| journal::Journal::open(root, client_id).ok())
        .map(Service::with_journal)
        .unwrap_or_default();
    let methods: Vec<_> = protocol::METHODS
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
            capabilities: json!({"methods":methods,"actions":if service.writable(){vec!["stage","unstage","commit","conflict.resolve","commit.amend","branch.create","branch.rename","branch.delete","branch.set_upstream","checkout","remote.add","remote.rename","remote.set_url","remote.remove","fetch","push","push.with_lease","branch.delete_remote","worktree.add","worktree.remove","worktree.repair","worktree.prune","worktree.lock","worktree.unlock","tag.delete_remote","merge.fast_forward","pull.fast_forward","merge","merge.abort","stash.save","stash.apply","stash.pop","stash.drop","tag.create","tag.delete","tag.push","cherry_pick","revert","integration.continue","integration.abort","rebase","integration.skip","reset","discard"]}else{vec![]},"features":["index.hunks","index.lines","discard.hunks","stash.entry_index","worktree.new_branch","diff.streaming","paths.bytes","history.snapshot_pagination"],"objectFormats":["sha1","sha256"]}),
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
                match result {
                    Err(error) => protocol::write(output, &Message::failure(id, error))?,
                    Ok(Output::Json(value)) => {
                        let reply = Message::success(id.clone(), value);
                        if protocol::encode(&reply).is_err() {
                            protocol::write(
                                output,
                                &Message::failure(
                                    id,
                                    Error::new(
                                        "LIMIT_EXCEEDED",
                                        "Response exceeds the frame limit.",
                                    ),
                                ),
                            )?;
                        } else {
                            protocol::write(output, &reply)?;
                        }
                    }
                    Ok(Output::Diff { snapshot, bytes }) => {
                        let stream_id = Uuid::new_v4().to_string();
                        let mut last_seq = 0;
                        protocol::write(
                            output,
                            &Message::Begin {
                                id: id.clone(),
                                stream_id: stream_id.clone(),
                                snapshot,
                            },
                        )?;
                        for (i, chunk) in bytes.chunks(protocol::CHUNK_SIZE).enumerate() {
                            last_seq = (i + 1) as u32;
                            protocol::write(
                                output,
                                &Message::Chunk {
                                    id: id.clone(),
                                    stream_id: stream_id.clone(),
                                    seq: last_seq,
                                    bytes_b64: STANDARD.encode(chunk),
                                },
                            )?;
                            loop {
                                match receive(Duration::from_secs(30))? {
                                    Message::Ack {
                                        stream_id: ack,
                                        seq,
                                    } if ack == stream_id && seq == last_seq => break,
                                    Message::Ping { nonce } if nonce.len() <= 128 => {
                                        protocol::write(output, &Message::Pong { nonce })?
                                    }
                                    _ => {
                                        return Err(io::Error::other(
                                            "Invalid Git stream acknowledgement",
                                        ))
                                    }
                                }
                            }
                        }
                        protocol::write(
                            output,
                            &Message::success(id, json!({"lastSeq":last_seq,"bytes":bytes.len()})),
                        )?;
                    }
                }
            }
            _ => return Err(io::Error::other("Unexpected Git protocol message")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handshake_rejects_mutations_and_unknown_params() {
        let mut input = vec![
            Message::Initialize {
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
