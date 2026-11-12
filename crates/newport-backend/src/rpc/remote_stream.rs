//! Commands use typed opaque chunks; Remote Apps carry their original service
//! frames. A worker outcome is published only after its remote closure is valid.
use super::{Host, RemoteChannel, failure, transport};
use newport_protocol::{
    desktop::{
        self, DisconnectPolicy, OpaqueFlow, OpaqueKind, Operation, Outcome, Request, stream as dto,
        wire,
    },
    worker::{self, ClientControl, Direction, ServerControl, Transport},
};
use russh::ChannelMsg;
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::watch,
    time::Instant,
};

const INPUT_CLOSED_GRACE: Duration = Duration::from_secs(5);

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
    let (server, apps, opened) = match operation.as_ref() {
        dto::Operation::CommandOpen(args) => (
            args.server,
            false,
            dto::Opened::Command(Box::new(dto::CommandOpened {
                version: desktop::VERSION,
                server: args.server,
            })),
        ),
        dto::Operation::RemoteAppsOpen(args) => (
            args.server,
            true,
            dto::Opened::RemoteApps(Box::new(dto::RemoteAppsOpened {
                version: desktop::VERSION,
                server: args.server,
                service_version: newport_protocol::remote_apps::REVISION,
            })),
        ),
        _ => anyhow::bail!("Expected command or Remote Apps open"),
    };
    let mut started = None;
    let opening = async {
        started = Some(match operation.as_ref() {
            dto::Operation::CommandOpen(args) => {
                host.open_remote_stream(server, args.command.clone()).await
            }
            dto::Operation::RemoteAppsOpen(args) => {
                host.open_remote_apps(server, args.arguments.clone()).await
            }
            _ => Err(failure::operation("Invalid stream open")),
        });
    };
    // Once exec may have been sent, finish acquiring the command so an abandoned
    // client can close its actual channel. Never start it a second time.
    if super::progress::with_policy(
        &mut input,
        &mut output,
        request,
        host.link_status(server),
        DisconnectPolicy::Complete,
        opening,
    )
    .await
    .is_err()
    {
        if let Some(Ok(remote)) = started {
            close(remote).await;
        }
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
        None => anyhow::bail!("Command open did not complete"),
    };
    if let Err(error) = super::write_response(
        &mut output,
        request,
        Outcome::Opened(Box::new(opened)),
        desktop::OPENED_WRITE_TIMEOUT,
    )
    .await
    {
        close(channel).await;
        return Err(error);
    }
    let result = pump(channel.channel, input, output, apps).await;
    drop(channel.guard);
    result
}

async fn close(remote: RemoteChannel) {
    let _ = tokio::time::timeout(Duration::from_secs(3), remote.channel.close()).await;
}

async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    channel: crate::service_channel::Channel,
    mut input: transport::Reader<R>,
    mut output: W,
    apps: bool,
) -> anyhow::Result<()> {
    let (mut remote, writer) = channel.split();
    let (completed, mut complete) = watch::channel(false);
    let result = {
        let forward = async {
            let mut opaque = OpaqueFlow::new(OpaqueKind::Command);
            let mut native = worker::wire::ClientDecoder::default();
            let native_limits = Transport::REMOTE_APPS.limits(Direction::ToWorker);
            let limit = if apps {
                native_limits.message
            } else {
                desktop::OPAQUE_LIMITS.message
            };
            while let Some(frame) = input.frame(limit, None).await? {
                if apps {
                    match native.decode(frame.frame.body(), &native_limits)? {
                        worker::wire::Frame::Service(_) => {
                            remote_write(async {
                                writer
                                    .data(&frame.frame.into_bytes())
                                    .await
                                    .map_err(anyhow::Error::msg)
                            })
                            .await?
                        }
                        worker::wire::Frame::Control(ClientControl::InputComplete) => {
                            remote_write(async { writer.eof().await.map_err(anyhow::Error::msg) })
                                .await?;
                            completed.send_replace(true);
                        }
                        worker::wire::Frame::Control(ClientControl::Signal(signal)) => {
                            remote_write(async {
                                writer.signal(signal).await.map_err(anyhow::Error::msg)
                            })
                            .await?
                        }
                    }
                } else {
                    let message = wire::decode_opaque_client(frame.frame.body())?;
                    opaque.client(&message)?;
                    match message {
                        dto::OpaqueClient::Data(data) => {
                            remote_write(async {
                                writer.data(data.as_ref()).await.map_err(anyhow::Error::msg)
                            })
                            .await?
                        }
                        dto::OpaqueClient::InputComplete(_) => {
                            remote_write(async { writer.eof().await.map_err(anyhow::Error::msg) })
                                .await?;
                            completed.send_replace(true);
                        }
                    }
                }
            }
            anyhow::bail!("Desktop stream disconnected")
        };
        tokio::pin!(forward);
        let mut flow = OpaqueFlow::new(OpaqueKind::Command);
        let (mut stderr, mut status, mut signal) = (Vec::new(), None, None);
        let mut input_closed = None;
        async {
        let ending = loop {
            let grace = async {
                match input_closed { Some(deadline) => tokio::time::sleep_until(deadline).await, None => std::future::pending().await }
            };
            tokio::select! {
                biased;
                result = &mut forward => return result,
                changed = complete.changed(), if input_closed.is_none() => {
                    if changed.is_ok() && *complete.borrow_and_update() { input_closed = Some(Instant::now() + INPUT_CLOSED_GRACE); }
                }
                _ = grace => break dto::Ending::Stopped,
                message = remote.wait() => match message {
                    Ok(Some(ChannelMsg::Data { data })) => {
                        if apps {
                            // service_channel has already bounded and separated
                            // each native frame, retaining its exact prefix.
                            transport::write_until(&mut output, &data, Instant::now() + desktop::RESPONSE_WRITE_TIMEOUT).await?;
                        } else {
                            for chunk in data.chunks(desktop::OPAQUE_CHUNK) {
                                opaque_write(&mut output, &mut flow, dto::OpaqueServer::Data(Box::new(chunk.to_vec().into()))).await?;
                            }
                        }
                    }
                    Ok(Some(ChannelMsg::ExtendedData { data, .. })) => {
                        if apps {
                            for chunk in data.chunks(worker::MAX_CHUNK) {
                                let control = ServerControl::Stderr(worker::Chunk::new(chunk.to_vec().into())?);
                                transport::write_until(&mut output, &worker::wire::encode_server(&control)?, Instant::now() + desktop::RESPONSE_WRITE_TIMEOUT).await?;
                            }
                        } else { keep_tail(&mut stderr, &data); }
                    }
                    Ok(Some(ChannelMsg::ExitStatus { exit_status })) => status = Some(exit_status),
                    Ok(Some(ChannelMsg::ExitSignal { signal_name, .. })) => signal = Some(crate::service_channel::signal_text(&signal_name)),
                    Ok(Some(ChannelMsg::Failure)) => break dto::Ending::Refused,
                    Ok(Some(_)) => {}
                    Ok(None) if status.is_some() || signal.is_some() => break dto::Ending::Exited,
                    Ok(None) if input_closed.is_some() => break dto::Ending::Stopped,
                    Ok(None) | Err(_) => break dto::Ending::Lost,
                }
            }
        };
        if apps {
            if ending == dto::Ending::Exited {
                let exit = remote.worker_exit().ok_or_else(|| anyhow::anyhow!("Worker exit lacks validated transport closure"))?;
                transport::write_until(&mut output, &worker::wire::encode_server(&ServerControl::Exit(exit))?, Instant::now() + desktop::RESPONSE_WRITE_TIMEOUT).await?;
            }
            // Abnormal native termination deliberately has no fabricated Exit.
            // The peer reports EOF without final worker outcome as incomplete.
        } else {
            let error = match ending {
                dto::Ending::Refused => Some(failure::operation("The server refused to run the command.").error),
                dto::Ending::Lost => Some(failure::operation("The connection to the server was lost.").error),
                _ => None,
            };
            opaque_write(&mut output, &mut flow, dto::OpaqueServer::Completion(Box::new(dto::Completion::Command(Box::new(dto::CommandCompletion { status, signal, ending, stderr: stderr.into(), error }))))).await?;
        }
        tokio::time::timeout(desktop::CLOSURE_TIMEOUT, output.shutdown()).await??;
        anyhow::Ok(())
    }.await
    };
    drop(remote);
    let _ = tokio::time::timeout(Duration::from_secs(3), writer.close()).await;
    result
}

fn keep_tail(tail: &mut Vec<u8>, bytes: &[u8]) {
    let retained = bytes.len().min(desktop::STDERR_LIMIT);
    let old = desktop::STDERR_LIMIT - retained;
    if tail.len() > old {
        tail.drain(..tail.len() - old);
    }
    tail.extend_from_slice(&bytes[bytes.len() - retained..]);
}

/// Backpressure cannot retain a detached desktop lease indefinitely. A timed
/// out write retires the forwarding task and its actual remote channel.
pub(super) async fn remote_write<T>(
    operation: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let deadline = Instant::now() + desktop::RESPONSE_WRITE_TIMEOUT;
    let result = tokio::time::timeout_at(deadline, operation)
        .await
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::TimedOut, error))??;
    if Instant::now() >= deadline {
        return Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into());
    }
    Ok(result)
}

pub(super) async fn opaque_write(
    output: &mut (impl AsyncWrite + Unpin),
    flow: &mut OpaqueFlow,
    message: dto::OpaqueServer,
) -> anyhow::Result<()> {
    flow.server(&message)?;
    transport::write_until(
        output,
        &wire::encode_opaque_server(&message)?,
        Instant::now() + desktop::RESPONSE_WRITE_TIMEOUT,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn apps(arguments: Vec<String>) -> Operation {
        Operation::Stream(Box::new(dto::Operation::RemoteAppsOpen(Box::new(
            dto::RemoteAppsOpenArgs {
                server: uuid::Uuid::from_u128(1),
                arguments,
            },
        ))))
    }
    #[test]
    fn worker_arguments_are_literal_and_cannot_select_a_shell_command() {
        let arguments = vec![
            "--".to_owned(),
            "a b".into(),
            "$(touch surprise)".into(),
            "'quoted'".into(),
            "".into(),
        ];
        let request = Request {
            id: desktop::RequestId::new(1).unwrap(),
            operation: apps(arguments.clone()),
        };
        let encoded =
            wire::encode_client(&desktop::ClientMessage::Request(Box::new(request))).unwrap();
        let body = newport_protocol::encoding::protobuf::read_frame(
            &mut encoded.as_slice(),
            desktop::MAX_REQUEST,
        )
        .unwrap()
        .unwrap();
        let desktop::ClientMessage::Request(request) = wire::decode_client(body, false).unwrap()
        else {
            panic!("request")
        };
        let Operation::Stream(operation) = request.operation else {
            panic!("stream")
        };
        let dto::Operation::RemoteAppsOpen(args) = *operation else {
            panic!("apps, never command")
        };
        assert_eq!(args.arguments, arguments);
    }
    #[test]
    fn worker_arguments_enforce_count_byte_and_nul_limits() {
        for arguments in [
            vec!["x".into(); 129],
            vec!["x".repeat(16385)],
            vec!["x".repeat(16384); 5],
            vec!["a\0b".into()],
            vec!["é".repeat(8193)],
        ] {
            assert!(apps(arguments).validate().is_err());
        }
        for arguments in [vec![], vec!["".into(); 128], vec!["x".repeat(16384); 4]] {
            assert!(apps(arguments).validate().is_ok());
        }
        for command in ["".into(), " \n".into(), "x".repeat(65537)] {
            assert!(
                Operation::Stream(Box::new(dto::Operation::CommandOpen(Box::new(
                    dto::CommandOpenArgs {
                        server: uuid::Uuid::from_u128(1),
                        command
                    }
                ))))
                .validate()
                .is_err()
            );
        }
    }
    #[tokio::test]
    async fn frames_carry_stdout_and_the_outcome_last() {
        let (client, mut server) = tokio::io::duplex(1024);
        let task = tokio::spawn(async move {
            let mut flow = OpaqueFlow::new(OpaqueKind::Command);
            opaque_write(
                &mut server,
                &mut flow,
                dto::OpaqueServer::Data(Box::new(bytes::Bytes::from_static(b"hello"))),
            )
            .await
            .unwrap();
            opaque_write(
                &mut server,
                &mut flow,
                dto::OpaqueServer::Completion(Box::new(dto::Completion::Command(Box::new(
                    dto::CommandCompletion {
                        status: Some(3),
                        signal: None,
                        ending: dto::Ending::Exited,
                        stderr: bytes::Bytes::new(),
                        error: None,
                    },
                )))),
            )
            .await
            .unwrap();
            assert!(
                opaque_write(
                    &mut server,
                    &mut flow,
                    dto::OpaqueServer::Data(Box::new(bytes::Bytes::from_static(b"late")))
                )
                .await
                .is_err()
            );
        });
        let mut reader = transport::Reader::new(client);
        let first = reader
            .frame(desktop::OPAQUE_LIMITS.message, None)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(wire::decode_opaque_server(first.frame.body()).unwrap(),dto::OpaqueServer::Data(value) if value.as_ref().as_ref()==b"hello")
        );
        let last = reader
            .frame(desktop::OPAQUE_LIMITS.message, None)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(wire::decode_opaque_server(last.frame.body()).unwrap(),dto::OpaqueServer::Completion(value) if matches!(value.as_ref(),dto::Completion::Command(value) if value.status==Some(3)))
        );
        assert!(
            reader
                .frame(desktop::OPAQUE_LIMITS.message, None)
                .await
                .unwrap()
                .is_none()
        );
        task.await.unwrap();
    }
    #[test]
    fn stderr_tail_is_bounded_and_keeps_the_latest_bytes() {
        let mut bytes = vec![b'a'; desktop::STDERR_LIMIT];
        keep_tail(&mut bytes, b"last");
        assert_eq!(bytes.len(), desktop::STDERR_LIMIT);
        assert!(bytes.ends_with(b"last"));
        keep_tail(&mut bytes, &vec![b'z'; desktop::STDERR_LIMIT + 1]);
        assert_eq!(bytes, vec![b'z'; desktop::STDERR_LIMIT]);
    }
}
