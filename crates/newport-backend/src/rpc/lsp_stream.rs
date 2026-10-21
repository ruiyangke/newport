//! A language server's messages carried over a native connection through a
//! Needle `lsp` worker. Framing matches remote streams: the client writes the
//! server's input as raw bytes and reads `[kind][length][payload]` frames.
use super::{Host, LspOpen};
use newport_platform::ipc::asynchronous::Stream;
use newport_platform::ipc::protocol::{REMOTE_STREAM_DATA, REMOTE_STREAM_EXIT, Response, VERSION};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

fn parse(args: Value) -> Result<LspOpen, String> {
    let open: LspOpen = serde_json::from_value(args).map_err(|error| error.to_string())?;
    if !open.root.starts_with('/') || open.root.len() > 4096 || open.root.contains('\0') {
        return Err("Choose an absolute worktree path.".into());
    }
    if open.server.is_empty()
        || open.server.len() > 40
        || !open
            .server
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-')
    {
        return Err("Unknown language server.".into());
    }
    Ok(open)
}

pub(super) async fn serve<H: Host>(
    mut stream: Stream,
    host: &H,
    args: Value,
) -> anyhow::Result<()> {
    let result = match parse(args) {
        Ok(open) => {
            // Starting may wait on an SSH agent approval; progress frames keep
            // the client waiting. A client that left drops the started worker.
            let mut started = None;
            let server = open.server_id;
            let opening = async {
                started = Some(host.open_lsp(open).await);
                Ok(Value::Null)
            };
            let progress =
                super::progress::with_progress(&mut stream, Some(server), false, opening).await;
            if progress.is_err() {
                return Ok(());
            }
            started.unwrap_or_else(|| Err("The language server did not start.".into()))
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
    let Ok(channel) = result else {
        return Ok(());
    };
    if !matches!(replied, Ok(Ok(()))) {
        return Ok(());
    }
    let (reader, writer) = tokio::io::split(stream);
    let outcome = pump(channel.stream, reader, writer).await;
    drop(channel.guard);
    outcome
}

/// Copy the client's bytes to the worker, and the worker's output to the
/// client as frames, then say how the stream ended.
async fn pump(
    remote: Box<dyn super::LspIo>,
    mut input: impl AsyncRead + Unpin + Send + 'static,
    mut output: impl AsyncWrite + Unpin,
) -> anyhow::Result<()> {
    let (mut remote_output, mut remote_input) = tokio::io::split(remote);
    // Input runs apart from output: a full input window must never stop the
    // server's messages, or its exit, from being read.
    let forward = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut input, &mut remote_input).await;
        let _ = remote_input.shutdown().await;
    });
    let mut buffer = vec![0; 32 * 1024];
    let result = async {
        let error = loop {
            match remote_output.read(&mut buffer).await {
                Ok(0) => break None,
                Ok(read) => frame(&mut output, REMOTE_STREAM_DATA, &buffer[..read]).await?,
                Err(error) => break Some(error.to_string()),
            }
        };
        let exit = json!({ "error": error });
        frame(&mut output, REMOTE_STREAM_EXIT, exit.to_string().as_bytes()).await?;
        output.shutdown().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    forward.abort();
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

    fn args(root: &str, server: &str) -> Value {
        json!({
            "serverId": uuid::Uuid::nil(),
            "expectedConnection": [],
            "root": root,
            "server": server,
            "envrc": true,
        })
    }

    #[test]
    fn opens_need_an_absolute_root_and_a_plain_server_id() {
        assert!(parse(args("/srv/app", "rust-analyzer")).is_ok());
        assert!(parse(args("srv/app", "rust-analyzer")).is_err());
        assert!(parse(args("/srv/app", "rust analyzer; rm -rf /")).is_err());
        assert!(parse(args("/srv/app", "")).is_err());
        let mut extra = args("/srv/app", "gopls");
        extra["command"] = json!("sh");
        assert!(parse(extra).is_err());
    }

    #[tokio::test]
    async fn input_reaches_the_worker_and_output_returns_as_frames() {
        let (worker, mut worker_side) = tokio::io::duplex(1024);
        let (client_input, mut client_writer) = tokio::io::duplex(1024);
        let (mut client_reader, client_output) = tokio::io::duplex(1024);
        let pumping = tokio::spawn(pump(Box::new(worker), client_input, client_output));
        client_writer.write_all(b"request").await.unwrap();
        let mut received = [0; 7];
        worker_side.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"request");
        worker_side.write_all(b"reply").await.unwrap();
        drop(worker_side);
        let mut frames = Vec::new();
        client_reader.read_to_end(&mut frames).await.unwrap();
        pumping.await.unwrap().unwrap();
        assert_eq!(&frames[..10], b"\x00\x00\x00\x00\x05reply");
        assert_eq!(frames[10], REMOTE_STREAM_EXIT);
        let exit: Value = serde_json::from_slice(&frames[15..]).unwrap();
        assert!(exit["error"].is_null());
    }
}
