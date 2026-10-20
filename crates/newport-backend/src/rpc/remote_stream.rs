//! A remote command's stdin, stdout and outcome carried over a native connection.
use super::Host;
use newport_platform::ipc::asynchronous::Stream;
use newport_platform::ipc::protocol::{REMOTE_STREAM_DATA, REMOTE_STREAM_EXIT, Response, VERSION};
use russh::ChannelMsg;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// After the client stops writing, how long the command may run on.
const INPUT_CLOSED_GRACE: Duration = Duration::from_secs(5);

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Open {
    server_id: uuid::Uuid,
    command: String,
}

pub(super) async fn serve<H: Host>(
    mut stream: Stream,
    host: &H,
    args: Value,
) -> anyhow::Result<()> {
    let open = serde_json::from_value::<Open>(args)
        .map_err(|error| error.to_string())
        .and_then(|open| {
            if open.command.trim().is_empty() || open.command.len() > 64 * 1024 {
                Err("Invalid remote command.".to_owned())
            } else {
                Ok(open)
            }
        });
    let result = match open {
        Ok(open) => {
            // Connecting may wait on an SSH agent approval; progress frames
            // keep the client waiting while it is pending. Once exec is sent
            // the start always completes, so a client that left can have the
            // command stopped rather than left running.
            let mut started = None;
            let server = open.server_id;
            let opening = async {
                started = Some(host.open_remote_stream(open.server_id, open.command).await);
                Ok(Value::Null)
            };
            let progress =
                super::progress::with_progress(&mut stream, Some(server), false, opening).await;
            let started = started.unwrap_or_else(|| Err("The command did not start.".into()));
            if progress.is_err() {
                close(started).await;
                return Ok(());
            }
            started
        }
        Err(error) => Err(error),
    };
    let response = Response {
        version: VERSION,
        result: Value::Null,
        error: result.as_ref().err().map(|error| json!(error)),
    };
    let bytes = rmp_serde::to_vec_named(&response)?;
    let replied = tokio::time::timeout(Duration::from_secs(10), async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await
    })
    .await;
    if !matches!(replied, Ok(Ok(()))) {
        close(result).await;
        return Ok(());
    }
    let Ok(remote) = result else {
        return Ok(());
    };
    let (reader, writer) = tokio::io::split(stream);
    let outcome = pump(remote.channel, reader, writer).await;
    drop(remote.guard);
    outcome
}

/// Stop a started command whose client is gone.
async fn close(started: Result<super::RemoteChannel, String>) {
    if let Ok(remote) = started {
        let _ = tokio::time::timeout(Duration::from_secs(3), remote.channel.close()).await;
    }
}

/// Why the command's stream ended.
#[derive(Debug, PartialEq)]
enum Ending {
    /// The command exited, with a status or a signal.
    Exited,
    /// The client finished, and the command stopped with its input.
    Stopped,
    /// The server refused to run the command.
    Refused,
    /// The connection ended without the command's exit.
    Lost,
}

/// The outcome frame's payload.
fn exit_report(
    ending: Ending,
    status: Option<u32>,
    signal: Option<String>,
    stderr: &[u8],
) -> Value {
    let error = match ending {
        Ending::Exited | Ending::Stopped => None,
        Ending::Refused => Some("The server refused to run the command."),
        Ending::Lost => Some("The connection to the server was lost."),
    };
    json!({
        "status": status,
        "signal": signal,
        "stderr": String::from_utf8_lossy(stderr),
        "error": error,
    })
}

/// Copy the client's bytes to the command's stdin, and the command's stdout
/// to the client as frames, then report how the command ended.
async fn pump(
    channel: russh::Channel<russh::client::Msg>,
    input: impl AsyncRead + Unpin + Send + 'static,
    mut output: impl AsyncWrite + Unpin,
) -> anyhow::Result<()> {
    let (mut remote, writer) = channel.split();
    let writer = std::sync::Arc::new(writer);
    // Input is copied apart from output: a full input window must never stop
    // output, or the command's exit, from being read.
    let (input_done, mut input_ended) = tokio::sync::oneshot::channel::<()>();
    let forward = tokio::spawn({
        let writer = writer.clone();
        let mut input = input;
        async move {
            let mut buffer = vec![0; 32 * 1024];
            loop {
                match input.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if writer.data(&buffer[..read]).await.is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = writer.eof().await;
            let _ = input_done.send(());
        }
    });
    let (mut stderr, mut status, mut signal) = (Vec::new(), None, None);
    let mut input_closed: Option<tokio::time::Instant> = None;
    let result = async {
        let ending = loop {
            let grace = async {
                match input_closed {
                    Some(at) => tokio::time::sleep_until(at + INPUT_CLOSED_GRACE).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                message = remote.wait() => match message {
                    Some(ChannelMsg::Data { data }) => {
                        frame(&mut output, REMOTE_STREAM_DATA, &data).await?;
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        crate::remote_apps::keep_tail(&mut stderr, &data);
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => status = Some(exit_status),
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        signal = Some(format!("{signal_name:?}"));
                    }
                    Some(ChannelMsg::Failure) => break Ending::Refused,
                    Some(_) => {}
                    None if status.is_some() || signal.is_some() => break Ending::Exited,
                    None if input_closed.is_some() => break Ending::Stopped,
                    None => break Ending::Lost,
                },
                _ = &mut input_ended, if input_closed.is_none() => {
                    input_closed = Some(tokio::time::Instant::now());
                }
                // A client that is gone cannot read the rest; don't leave the
                // command running on the server.
                _ = grace => break Ending::Stopped,
            }
        };
        let exit = exit_report(ending, status, signal.clone(), &stderr);
        frame(&mut output, REMOTE_STREAM_EXIT, exit.to_string().as_bytes()).await?;
        output.shutdown().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    forward.abort();
    let _ = tokio::time::timeout(Duration::from_secs(3), writer.close()).await;
    result
}

async fn frame(
    output: &mut (impl AsyncWrite + Unpin),
    kind: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    output.write_u8(kind).await?;
    output.write_u32(payload.len() as u32).await?;
    output.write_all(payload).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_carry_stdout_and_the_outcome_last() {
        let (mut client, server) = tokio::io::duplex(1024);
        let task = tokio::spawn(async move {
            let mut server = server;
            frame(&mut server, REMOTE_STREAM_DATA, b"hello")
                .await
                .unwrap();
            frame(&mut server, REMOTE_STREAM_EXIT, br#"{"status":3}"#)
                .await
                .unwrap();
        });
        let mut kinds = Vec::new();
        for _ in 0..2 {
            let kind = client.read_u8().await.unwrap();
            let len = client.read_u32().await.unwrap();
            let mut payload = vec![0; len as usize];
            client.read_exact(&mut payload).await.unwrap();
            kinds.push((kind, payload));
        }
        task.await.unwrap();
        assert_eq!(kinds[0], (REMOTE_STREAM_DATA, b"hello".to_vec()));
        assert_eq!(kinds[1].0, REMOTE_STREAM_EXIT);
    }

    #[test]
    fn the_outcome_says_why_a_command_ended_without_exiting() {
        let exited = exit_report(Ending::Exited, Some(2), None, b"bad");
        assert_eq!(
            (exited["status"].clone(), exited["error"].clone()),
            (json!(2), Value::Null)
        );
        assert_eq!(exited["stderr"], "bad");
        assert!(exit_report(Ending::Stopped, None, None, b"")["error"].is_null());
        assert_eq!(
            exit_report(Ending::Lost, None, None, b"")["error"],
            "The connection to the server was lost."
        );
        assert_eq!(
            exit_report(Ending::Refused, None, None, b"")["error"],
            "The server refused to run the command."
        );
    }
}
