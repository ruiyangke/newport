//! An LSP worker remains opaque; desktop ownership and completion are typed.
use super::{Host, transport};
use newport_protocol::desktop::{
    self, DisconnectPolicy, OpaqueFlow, OpaqueKind, Operation, Outcome, Request, stream as dto,
    wire,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) async fn serve<R, W, H>(
    mut input: transport::Reader<R>,
    mut output: W,
    host: &H,
    request: &Request,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    H: Host,
{
    let Operation::Stream(operation) = &request.operation else {
        anyhow::bail!("Expected stream operation");
    };
    let dto::Operation::LspOpen(open) = operation.as_ref() else {
        anyhow::bail!("Expected LSP open");
    };
    let mut started = None;
    let opening = async {
        started = Some(host.open_lsp(open.as_ref().clone()).await);
    };
    if super::progress::with_policy(
        &mut input,
        &mut output,
        request,
        host.link_status(open.server),
        DisconnectPolicy::Complete,
        opening,
    )
    .await
    .is_err()
    {
        return Ok(());
    }
    let channel = match started {
        Some(Ok(channel)) => channel,
        Some(Err(error)) => {
            return super::write_response(
                &mut output,
                request,
                Outcome::Failed(Box::new(error)),
                desktop::OPENED_WRITE_TIMEOUT,
            )
            .await;
        }
        None => anyhow::bail!("Language server open did not complete"),
    };
    super::write_response(
        &mut output,
        request,
        Outcome::Opened(Box::new(dto::Opened::Lsp(Box::new(dto::LspOpened {
            version: desktop::VERSION,
            server: open.server,
        })))),
        desktop::OPENED_WRITE_TIMEOUT,
    )
    .await?;
    let result = pump(channel.stream, input, output).await;
    drop(channel.guard);
    result
}

async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    remote: Box<dyn super::LspIo>,
    mut input: transport::Reader<R>,
    mut output: W,
) -> anyhow::Result<()> {
    let (mut remote_output, mut remote_input) = tokio::io::split(remote);
    let forward = async {
        let mut flow = OpaqueFlow::new(OpaqueKind::Lsp);
        while let Some(frame) = input.frame(desktop::OPAQUE_LIMITS.message, None).await? {
            let message = wire::decode_opaque_client(frame.frame.body())?;
            flow.client(&message)?;
            match message {
                dto::OpaqueClient::Data(data) => {
                    super::remote_stream::remote_write(async {
                        remote_input.write_all(data.as_ref()).await?;
                        anyhow::Ok(())
                    })
                    .await?;
                }
                dto::OpaqueClient::InputComplete(_) => {
                    super::remote_stream::remote_write(async {
                        remote_input.shutdown().await?;
                        anyhow::Ok(())
                    })
                    .await?;
                }
            }
        }
        anyhow::bail!("Desktop LSP stream disconnected")
    };
    let send = async {
        let mut flow = OpaqueFlow::new(OpaqueKind::Lsp);
        let mut buffer = vec![0; desktop::OPAQUE_CHUNK];
        let error = loop {
            match remote_output.read(&mut buffer).await {
                Ok(0) => break None,
                Ok(read) => {
                    super::remote_stream::opaque_write(
                        &mut output,
                        &mut flow,
                        dto::OpaqueServer::Data(Box::new(buffer[..read].to_vec().into())),
                    )
                    .await?
                }
                Err(error) => break Some(super::failure::operation(error).error),
            }
        };
        super::remote_stream::opaque_write(
            &mut output,
            &mut flow,
            dto::OpaqueServer::Completion(Box::new(dto::Completion::Lsp(Box::new(
                dto::LspCompletion { error },
            )))),
        )
        .await?;
        tokio::time::timeout(desktop::CLOSURE_TIMEOUT, output.shutdown()).await??;
        anyhow::Ok(())
    };
    tokio::select! { biased; result = forward => result, result = send => result }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn operation(root: &str, name: &str) -> Operation {
        let server = uuid::Uuid::from_u128(1);
        Operation::Stream(Box::new(dto::Operation::LspOpen(Box::new(
            dto::LspOpenArgs {
                server,
                expected: crate::rpc::tests::scope(server),
                root: root.into(),
                server_name: name.into(),
                envrc: true,
            },
        ))))
    }
    #[test]
    fn opens_need_an_absolute_root_and_a_plain_server_id() {
        let longest_root = format!("/{}", "x".repeat(4095));
        let longest_name = "a".repeat(40);
        for (root, name) in [
            ("/srv/app", "rust-analyzer"),
            ("/", "gopls"),
            ("/srv/space 项目", "typescript-language-server"),
            (longest_root.as_str(), longest_name.as_str()),
        ] {
            assert!(
                operation(root, name).validate().is_ok(),
                "{root:?}, {name:?}"
            );
        }
        let oversized_root = format!("{longest_root}x");
        let oversized_name = format!("{longest_name}a");
        for (root, name) in [
            ("", "rust-analyzer"),
            ("srv/app", "rust-analyzer"),
            ("C:\\srv\\app", "rust-analyzer"),
            ("/srv/app\0", "rust-analyzer"),
            (oversized_root.as_str(), "rust-analyzer"),
            ("/srv/app", "rust analyzer; rm -rf /"),
            ("/srv/app", ""),
            ("/srv/app", "Rust-analyzer"),
            ("/srv/app", "rust_analyzer"),
            ("/srv/app", "rust-analyzer1"),
            ("/srv/app", "rust-analyzér"),
            ("/srv/app", oversized_name.as_str()),
        ] {
            assert!(
                operation(root, name).validate().is_err(),
                "{root:?}, {name:?}"
            );
        }
    }
    #[tokio::test]
    async fn input_reaches_the_worker_and_output_returns_as_typed_frames() {
        let (worker, mut peer) = tokio::io::duplex(1024);
        let (client_input, mut client_writer) = tokio::io::duplex(1024);
        let (client_reader, client_output) = tokio::io::duplex(1024);
        let pumping = tokio::spawn(pump(
            Box::new(worker),
            transport::Reader::new(client_input),
            client_output,
        ));
        client_writer
            .write_all(
                &wire::encode_opaque_client(&dto::OpaqueClient::Data(Box::new(
                    bytes::Bytes::from_static(b"request"),
                )))
                .unwrap(),
            )
            .await
            .unwrap();
        let mut received = [0; 7];
        peer.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"request");
        peer.write_all(b"reply").await.unwrap();
        drop(peer);
        let mut frames = transport::Reader::new(client_reader);
        let data = frames
            .frame(desktop::OPAQUE_LIMITS.message, None)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(wire::decode_opaque_server(data.frame.body()).unwrap(),dto::OpaqueServer::Data(bytes) if bytes.as_ref().as_ref()==b"reply")
        );
        let end = frames
            .frame(desktop::OPAQUE_LIMITS.message, None)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(wire::decode_opaque_server(end.frame.body()).unwrap(),dto::OpaqueServer::Completion(value) if matches!(value.as_ref(),dto::Completion::Lsp(value) if value.error.is_none()))
        );
        assert!(
            frames
                .frame(desktop::OPAQUE_LIMITS.message, None)
                .await
                .unwrap()
                .is_none()
        );
        pumping.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_worker_write_releases_the_stream_with_a_typed_timeout() {
        let (worker, mut peer) = tokio::io::duplex(1);
        let (client_input, mut client_writer) = tokio::io::duplex(1024);
        let (_client_reader, client_output) = tokio::io::duplex(1024);
        let pumping = tokio::spawn(pump(
            Box::new(worker),
            transport::Reader::new(client_input),
            client_output,
        ));
        client_writer
            .write_all(
                &wire::encode_opaque_client(&dto::OpaqueClient::Data(Box::new(
                    bytes::Bytes::from_static(b"blocked worker input"),
                )))
                .unwrap(),
            )
            .await
            .unwrap();
        let mut first = [0];
        // Receiving one byte proves the write is in flight. The remaining input
        // cannot fit in the worker's buffer, even after this byte is consumed.
        peer.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"b");
        drop(client_writer);
        tokio::time::advance(desktop::RESPONSE_WRITE_TIMEOUT).await;
        let error = pumping.await.unwrap().unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::TimedOut,
        );
        let mut remaining = Vec::new();
        peer.read_to_end(&mut remaining).await.unwrap();
        assert!(remaining.len() <= 1);
    }
}
