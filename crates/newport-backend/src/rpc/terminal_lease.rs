//! One typed terminal stream owns one local attachment. Disconnect detaches its
//! guard; ending a persistent shell requires the explicit End operation.
use super::{Host, TerminalChannel, failure, transport};
use crate::terminal::{Event, Input, Io};
use newport_protocol::{
    common::Present,
    desktop::{self, Failure, Operation, Outcome, Request, TerminalFlow, stream as dto, wire},
};
use std::sync::{Mutex, MutexGuard};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{Notify, mpsc},
    time::Instant,
};

pub(super) async fn serve<R, W, H>(
    reader: transport::Reader<R>,
    writer: W,
    host: &H,
    request: &Request,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    H: Host,
{
    let Operation::Stream(operation) = &request.operation else {
        anyhow::bail!("Expected terminal operation");
    };
    let dto::Operation::TerminalOpen(open) = operation.as_ref() else {
        anyhow::bail!("Expected terminal open");
    };
    hold(
        reader,
        writer,
        request,
        host.link_status(open.server),
        async {
            host.open_terminal(open.as_ref().clone())
                .await
                .map(|TerminalChannel { io, guard }| (io, guard))
        },
    )
    .await
}

async fn hold<R, W, G>(
    mut reader: transport::Reader<R>,
    mut writer: W,
    request: &Request,
    status: Option<tokio::sync::watch::Receiver<newport_ssh::Status>>,
    opening: impl std::future::Future<Output = Result<(Io, G), Failure>>,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let Operation::Stream(operation) = &request.operation else {
        anyhow::bail!("Expected terminal operation");
    };
    let dto::Operation::TerminalOpen(open) = operation.as_ref() else {
        anyhow::bail!("Expected terminal open");
    };
    let (io, guard) =
        match super::progress::with_progress(&mut reader, &mut writer, request, status, opening)
            .await?
        {
            Ok(channel) => channel,
            Err(error) => {
                return super::write_response(
                    &mut writer,
                    request,
                    Outcome::Failed(Box::new(error)),
                    desktop::OPENED_WRITE_TIMEOUT,
                )
                .await;
            }
        };
    super::write_response(
        &mut writer,
        request,
        Outcome::Opened(Box::new(dto::Opened::Terminal(Box::new(
            dto::TerminalOpened {
                version: desktop::VERSION,
                session: open.session,
            },
        )))),
        desktop::OPENED_WRITE_TIMEOUT,
    )
    .await?;
    let result = relay(reader, writer, io).await;
    drop(guard);
    result
}

#[derive(Default)]
struct Flow {
    state: Mutex<TerminalFlow>,
    reported: Mutex<u64>,
    took: Notify,
    acknowledged: Notify,
}
impl Flow {
    fn state(&self) -> anyhow::Result<MutexGuard<'_, TerminalFlow>> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("Terminal flow state poisoned"))
    }
    fn take(&self, bytes: usize) -> anyhow::Result<()> {
        let bytes = u64::try_from(bytes)?;
        self.state()?.taken(bytes)?;
        let mut pending = self
            .reported
            .lock()
            .map_err(|_| anyhow::anyhow!("Terminal taken state poisoned"))?;
        *pending = pending
            .checked_add(bytes)
            .ok_or_else(|| anyhow::anyhow!("Terminal taken counter overflow"))?;
        self.took.notify_one();
        Ok(())
    }
    fn reported(&self) -> anyhow::Result<u64> {
        let mut pending = self
            .reported
            .lock()
            .map_err(|_| anyhow::anyhow!("Terminal taken state poisoned"))?;
        Ok(std::mem::take(&mut *pending))
    }
}
enum Queued {
    Data { epoch: u64, data: Vec<u8> },
    Resize(u32, u32),
    End,
    Activate,
}

async fn relay<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut reader: transport::Reader<R>,
    mut writer: W,
    io: Io,
) -> anyhow::Result<()> {
    let Io { input, mut output } = io;
    let flow = Flow::default();
    // The byte window bounds input, and this independent frame bound prevents
    // a peer from exhausting memory with zero-byte controls.
    let (queue, mut queued) = mpsc::channel(256);
    let receive = async {
        while let Some(frame) = reader
            .frame(desktop::TERMINAL_CLIENT_LIMITS.message, None)
            .await?
        {
            let item = match wire::decode_terminal_client(frame.frame.body())? {
                dto::TerminalClient::Ack(value) => {
                    flow.state()?.acknowledge(value.bytes)?;
                    flow.acknowledged.notify_one();
                    continue;
                }
                dto::TerminalClient::Input(value) => {
                    flow.state()?.input(value.epoch, value.data.len())?;
                    Queued::Data {
                        epoch: value.epoch,
                        data: value.data.to_vec(),
                    }
                }
                dto::TerminalClient::Resize(value) => {
                    let columns = value.columns.into();
                    let rows = value.rows.into();
                    crate::terminal::size(columns, rows).map_err(anyhow::Error::msg)?;
                    Queued::Resize(columns, rows)
                }
                dto::TerminalClient::End(_) => Queued::End,
                dto::TerminalClient::Activate(_) => Queued::Activate,
            };
            queue
                .try_send(item)
                .map_err(|_| anyhow::anyhow!("Terminal control queue is full"))?;
        }
        anyhow::Ok(())
    };
    let apply = async {
        while let Some(item) = queued.recv().await {
            match item {
                Queued::Data { epoch, data } => {
                    let length = data.len();
                    if epoch == flow.state()?.epoch()
                        && let Ok(permit) = input.reserve().await
                    {
                        // The shell can be replaced while its input queue is
                        // full. Recheck only after ownership of the slot.
                        if epoch == flow.state()?.epoch() {
                            permit.send(Input::Data(data));
                        }
                    }
                    flow.take(length)?;
                }
                Queued::Resize(columns, rows) => {
                    let _ = input.send(Input::Resize(columns, rows)).await;
                }
                Queued::End => {
                    let _ = input.send(Input::End).await;
                }
                Queued::Activate => {
                    let _ = input.send(Input::Activate).await;
                }
            }
        }
        std::future::pending::<anyhow::Result<()>>().await
    };
    let send = async {
        let mut ended = false;
        let mut held = None;
        loop {
            let available = flow.state()?.output_available();
            tokio::select! {
                biased;
                _ = flow.took.notified() => {
                    let bytes = flow.reported()?;
                    if bytes > 0 { write(&mut writer, dto::TerminalServer::Taken(Box::new(dto::Taken { bytes }))).await?; }
                }
                _ = flow.acknowledged.notified(), if available < desktop::MAX_DATA as u64 => {}
                event = async { match held.take() { Some(event) => Some(event), None => output.recv().await } }, if available >= desktop::MAX_DATA as u64 => {
                    match event {
                        Some(Event::Data(mut data)) => {
                            while data.len() < desktop::MAX_DATA {
                                match output.try_recv() {
                                    Ok(Event::Data(more)) => data.extend(more),
                                    Ok(event) => { held = Some(event); break; }
                                    Err(_) => break,
                                }
                            }
                            for chunk in data.chunks(desktop::MAX_DATA) {
                                loop {
                                    let notified = flow.acknowledged.notified();
                                    if flow.state()?.output_available() >= u64::try_from(chunk.len())? { break; }
                                    notified.await;
                                }
                                flow.state()?.output(chunk.len())?;
                                write(&mut writer, dto::TerminalServer::Output(Box::new(dto::Output { data: chunk.to_vec().into() }))).await?;
                            }
                        }
                        Some(Event::Restore { state, prompted }) => write(&mut writer, dto::TerminalServer::Restore(Box::new(dto::Restore { state: state.into(), prompted }))).await?,
                        Some(event) => {
                            let event = event_message(event)?;
                            if matches!(&event, dto::TerminalEvent::Reconnecting(_) | dto::TerminalEvent::Exit(_) | dto::TerminalEvent::Error(_) | dto::TerminalEvent::Ended(_)) { flow.state()?.depart()?; }
                            ended |= matches!(&event, dto::TerminalEvent::Exit(_) | dto::TerminalEvent::Error(_) | dto::TerminalEvent::Ended(_));
                            write(&mut writer, dto::TerminalServer::Event(Box::new(event))).await?;
                            if ended { return anyhow::Ok(()); }
                        }
                        None => {
                            if !ended {
                                write(&mut writer, dto::TerminalServer::Event(Box::new(dto::TerminalEvent::Error(Box::new(failure::operation("Terminal is closed.")))))).await?;
                            }
                            return anyhow::Ok(());
                        }
                    }
                }
            }
        }
    };
    tokio::select! { result = receive => result, result = apply => result, result = send => result }
}

fn event_message(event: Event) -> anyhow::Result<dto::TerminalEvent> {
    Ok(match event {
        Event::Ready => dto::TerminalEvent::Ready(Box::new(Present {})),
        Event::Persistent => dto::TerminalEvent::Persistent(Box::new(Present {})),
        Event::Ended => dto::TerminalEvent::Ended(Box::new(Present {})),
        Event::DisplayLost => dto::TerminalEvent::DisplayLost(Box::new(Present {})),
        Event::Reconnecting { attempt, message } => {
            dto::TerminalEvent::Reconnecting(Box::new(dto::Reconnecting { attempt, message }))
        }
        Event::Exit(code) => dto::TerminalEvent::Exit(Box::new(dto::Exit { code })),
        Event::Error(error) => dto::TerminalEvent::Error(Box::new(Failure {
            error,
            detail: None,
        })),
        Event::Active(active) => dto::TerminalEvent::Active(Box::new(dto::Active { active })),
        Event::NotHeld(reason) => dto::TerminalEvent::NotHeld(Box::new(dto::NotHeld { reason })),
        Event::Size { columns, rows } => {
            dto::TerminalEvent::Size(Box::new(dto::Resize { columns, rows }))
        }
        Event::Data(_) | Event::Restore { .. } => anyhow::bail!("Terminal data is not an event"),
    })
}
async fn write(
    writer: &mut (impl AsyncWrite + Unpin),
    message: dto::TerminalServer,
) -> anyhow::Result<()> {
    let bytes = wire::encode_terminal_server(&message)?;
    transport::write_until(
        writer,
        &bytes,
        Instant::now() + desktop::RESPONSE_WRITE_TIMEOUT,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use uuid::Uuid;
    struct Guard(Arc<Mutex<Vec<Uuid>>>, Uuid);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.lock().unwrap().retain(|id| *id != self.1);
        }
    }
    /// The shell's side of a terminal's I/O.
    struct Shell {
        input: mpsc::Receiver<Input>,
        output: mpsc::Sender<Event>,
    }
    fn terminal(input: usize) -> (Io, Shell) {
        let (input, inputs) = mpsc::channel(input);
        let (outputs, output) = mpsc::channel(32);
        (
            Io { input, output },
            Shell {
                input: inputs,
                output: outputs,
            },
        )
    }
    // Semantic fixture views only. All bytes use the shipping protobuf codec.
    #[derive(Debug, PartialEq)]
    enum TestEvent {
        Ready,
        Persistent,
        Reconnecting { attempt: u32, message: String },
        Exit(Option<u32>),
        Error(String),
        Ended,
        Active(bool),
        DisplayLost,
        NotHeld(String),
        Size(u16, u16),
    }
    #[derive(Debug, PartialEq)]
    enum ToApp {
        Output(Vec<u8>),
        Taken(u64),
        Restore { state: Vec<u8>, prompted: bool },
        Event(TestEvent),
    }
    enum ToBackend {
        Input { epoch: u64, data: Vec<u8> },
        Resize { cols: u16, rows: u16 },
        Ack(u64),
        End,
    }
    impl ToBackend {
        fn encode(self) -> Vec<u8> {
            let message = match self {
                Self::Input { epoch, data } => dto::TerminalClient::Input(Box::new(dto::Input {
                    epoch,
                    data: data.into(),
                })),
                Self::Resize { cols, rows } => dto::TerminalClient::Resize(Box::new(dto::Resize {
                    columns: cols,
                    rows,
                })),
                Self::Ack(bytes) => dto::TerminalClient::Ack(Box::new(dto::Ack { bytes })),
                Self::End => dto::TerminalClient::End(Box::new(Present {})),
            };
            wire::encode_terminal_client(&message).unwrap()
        }
    }
    struct Response {
        error: Option<String>,
    }
    async fn body(stream: &mut DuplexStream, limit: usize) -> Option<bytes::Bytes> {
        let mut length = newport_protocol::encoding::protobuf::LengthDecoder::default();
        let length = loop {
            let byte = match stream.read_u8().await {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return None,
                Err(error) => panic!("{error}"),
            };
            if let Some(length) = length.push(byte, limit).unwrap() {
                break length;
            }
        };
        let mut bytes = vec![0; length];
        stream.read_exact(&mut bytes).await.unwrap();
        Some(bytes.into())
    }
    async fn response(stream: &mut DuplexStream) -> Response {
        let message =
            wire::decode_server(body(stream, desktop::MAX_RESPONSE).await.unwrap()).unwrap();
        match message {
            desktop::ServerMessage::Response(value) => match value.outcome {
                Outcome::Opened(_) => Response { error: None },
                Outcome::Failed(error) => Response {
                    error: Some(error.error.message),
                },
                _ => panic!("expected opening"),
            },
            _ => panic!("expected response"),
        }
    }
    #[tokio::test]
    async fn a_not_held_notice_reaches_the_app_with_its_reason() {
        let (mut app, mut backend) = tokio::io::duplex(64 * 1024);
        let event = event_message(Event::NotHeld("no private directory".into())).unwrap();
        write(&mut backend, dto::TerminalServer::Event(Box::new(event)))
            .await
            .unwrap();
        assert_eq!(
            next(&mut app).await,
            Some(ToApp::Event(TestEvent::NotHeld(
                "no private directory".into()
            )))
        );
    }
    async fn next(stream: &mut DuplexStream) -> Option<ToApp> {
        let body = body(stream, desktop::TERMINAL_SERVER_LIMITS.message).await?;
        Some(match wire::decode_terminal_server(body).unwrap() {
            dto::TerminalServer::Output(value) => ToApp::Output(value.data.to_vec()),
            dto::TerminalServer::Taken(value) => ToApp::Taken(value.bytes),
            dto::TerminalServer::Restore(value) => ToApp::Restore {
                state: value.state.to_vec(),
                prompted: value.prompted,
            },
            dto::TerminalServer::Event(value) => ToApp::Event(match *value {
                dto::TerminalEvent::Ready(_) => TestEvent::Ready,
                dto::TerminalEvent::Persistent(_) => TestEvent::Persistent,
                dto::TerminalEvent::Reconnecting(value) => TestEvent::Reconnecting {
                    attempt: value.attempt,
                    message: value.message,
                },
                dto::TerminalEvent::Exit(value) => TestEvent::Exit(value.code),
                dto::TerminalEvent::Error(value) => TestEvent::Error(value.error.message),
                dto::TerminalEvent::Ended(_) => TestEvent::Ended,
                dto::TerminalEvent::Active(value) => TestEvent::Active(value.active),
                dto::TerminalEvent::DisplayLost(_) => TestEvent::DisplayLost,
                dto::TerminalEvent::NotHeld(value) => TestEvent::NotHeld(value.reason),
                dto::TerminalEvent::Size(value) => TestEvent::Size(value.columns, value.rows),
            }),
        })
    }
    async fn send(stream: &mut DuplexStream, frame: ToBackend) {
        stream.write_all(&frame.encode()).await.unwrap();
    }
    async fn hold<G>(
        stream: DuplexStream,
        opening: impl std::future::Future<Output = Result<(Io, G), String>>,
    ) -> anyhow::Result<()> {
        let (reader, writer) = tokio::io::split(stream);
        let request = Request {
            id: desktop::RequestId::new(1).unwrap(),
            operation: Operation::Stream(Box::new(dto::Operation::TerminalOpen(Box::new(
                dto::TerminalOpenArgs {
                    server: Uuid::from_u128(1),
                    session: Uuid::from_u128(2),
                    columns: 80,
                    rows: 24,
                    persist: false,
                    attach: false,
                    colors: None,
                },
            )))),
        };
        super::hold(
            transport::Reader::new(reader),
            writer,
            &request,
            None,
            async { opening.await.map_err(failure::operation) },
        )
        .await
    }
    /// An open terminal relayed over a socket, and the app's end of it.
    async fn open(
        input: usize,
    ) -> (
        DuplexStream,
        Shell,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    ) {
        let (mut client, server) = tokio::io::duplex(4 * desktop::MAX_DATA);
        let (io, shell) = terminal(input);
        let task = tokio::spawn(hold(server, async move { Ok((io, ())) }));
        assert!(response(&mut client).await.error.is_none());
        (client, shell, task)
    }

    #[tokio::test]
    async fn resize_rejects_invalid_dimensions_before_reaching_the_shell() {
        for (cols, rows) in [(1, 24), (501, 24), (80, 301), (u16::MAX, u16::MAX)] {
            let (mut client, mut shell, task) = open(1).await;
            send(&mut client, ToBackend::Resize { cols, rows }).await;
            assert!(
                tokio::time::timeout(Duration::from_secs(1), shell.input.recv())
                    .await
                    .unwrap()
                    .is_none(),
                "invalid resize {cols}x{rows} reached the shell"
            );
            assert_eq!(
                task.await.unwrap().unwrap_err().to_string(),
                "Invalid terminal dimensions."
            );
        }
    }

    #[tokio::test]
    async fn resize_forwards_dimensions_at_the_allowed_bounds() {
        let (mut client, mut shell, task) = open(1).await;
        for (cols, rows) in [(2, 1), (500, 300)] {
            send(&mut client, ToBackend::Resize { cols, rows }).await;
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(1), shell.input.recv())
                    .await
                    .unwrap(),
                Some(Input::Resize(columns, lines))
                    if columns == u32::from(cols) && lines == u32::from(rows)
            ));
        }
        drop(client);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn disconnect_releases_only_the_owned_session_and_allows_repeated_opens() {
        let unrelated = Uuid::new_v4();
        let sessions = Arc::new(Mutex::new(vec![unrelated]));
        for _ in 0..16 {
            let (mut client, server) = tokio::io::duplex(1024);
            let state = sessions.clone();
            let id = Uuid::new_v4();
            let (io, _shell) = terminal(1);
            let task = tokio::spawn(hold(server, async move {
                state.lock().unwrap().push(id);
                Ok((io, Guard(state, id)))
            }));
            assert!(response(&mut client).await.error.is_none());
            assert_eq!(*sessions.lock().unwrap(), [unrelated, id]);
            drop(client); // Same EOF as client process termination, without a close RPC.
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(*sessions.lock().unwrap(), [unrelated]);
        }
    }
    #[tokio::test]
    async fn end_reaches_the_shell_and_a_restore_reaches_the_app_whole() {
        let (mut client, server) = tokio::io::duplex(1 << 20);
        let id = Uuid::new_v4();
        let sessions = Arc::new(Mutex::new(vec![]));
        let state = sessions.clone();
        let (io, mut shell) = terminal(4);
        let task = tokio::spawn(hold(server, async move {
            state.lock().unwrap().push(id);
            Ok((io, Guard(state, id)))
        }));
        assert!(response(&mut client).await.error.is_none());
        client.write_all(&ToBackend::End.encode()).await.unwrap();
        assert!(matches!(shell.input.recv().await, Some(Input::End)));
        // Larger than an output frame and outside output credit.
        let state = vec![b'x'; 3 * desktop::MAX_DATA];
        shell
            .output
            .send(Event::Restore {
                state: state.clone(),
                prompted: true,
            })
            .await
            .unwrap();
        assert_eq!(
            next(&mut client).await,
            Some(ToApp::Restore {
                state,
                prompted: true
            })
        );
        drop(client);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn lost_open_acknowledgement_releases_the_created_session() {
        let (client, server) = tokio::io::duplex(1);
        let id = Uuid::new_v4();
        let sessions = Arc::new(Mutex::new(vec![]));
        let state = sessions.clone();
        let (opened, ready) = tokio::sync::oneshot::channel();
        let (io, _shell) = terminal(1);
        let task = tokio::spawn(hold(server, async move {
            state.lock().unwrap().push(id);
            let _ = opened.send(());
            Ok((io, Guard(state, id)))
        }));
        ready.await.unwrap();
        // The one-byte buffer cannot contain the full response header.
        drop(client);
        assert!(task.await.unwrap().is_err());
        assert!(sessions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_open_never_closes_an_existing_session() {
        let (mut client, server) = tokio::io::duplex(1024);
        let task = tokio::spawn(hold(server, async {
            Err::<(Io, Guard), _>("Terminal session already exists.".into())
        }));
        assert_eq!(
            response(&mut client).await.error,
            Some("Terminal session already exists.".into())
        );
        task.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn disconnect_cancels_pending_open_and_task_cancellation_drops_guard() {
        let (client, server) = tokio::io::duplex(1024);
        let task = tokio::spawn(hold(
            server,
            std::future::pending::<Result<(Io, Guard), String>>(),
        ));
        drop(client);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );

        let (mut client, server) = tokio::io::duplex(1024);
        let id = Uuid::new_v4();
        let sessions = Arc::new(Mutex::new(vec![id]));
        let state = sessions.clone();
        let (io, _shell) = terminal(1);
        let task = tokio::spawn(hold(server, async move { Ok((io, Guard(state, id))) }));
        assert!(response(&mut client).await.error.is_none());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(sessions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn waiting_output_goes_out_as_one_frame_ahead_of_later_events() {
        let (mut client, server) = tokio::io::duplex(4 * desktop::MAX_DATA);
        let (io, shell) = terminal(1);
        for read in [&b"one "[..], b"two ", b"three"] {
            shell.output.send(Event::Data(read.to_vec())).await.unwrap();
        }
        shell.output.send(Event::Exit(Some(0))).await.unwrap();
        shell
            .output
            .send(Event::Data(b"after".to_vec()))
            .await
            .unwrap();
        let task = tokio::spawn(hold(server, async move { Ok((io, ())) }));
        assert!(response(&mut client).await.error.is_none());
        assert_eq!(
            next(&mut client).await,
            Some(ToApp::Output(b"one two three".to_vec()))
        );
        assert_eq!(
            next(&mut client).await,
            Some(ToApp::Event(TestEvent::Exit(Some(0))))
        );
        // Exit is final even if an erroneous producer queues later bytes.
        assert!(next(&mut client).await.is_none());
        drop(shell);
        drop(client);
        let _ = task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn output_stops_when_credit_runs_out_and_an_ack_resumes_it() {
        let (mut client, shell, task) = open(1).await;
        let chunk = vec![b'x'; desktop::MAX_DATA];
        let chunks = 2 * (desktop::OUTPUT_CREDIT as usize / desktop::MAX_DATA) + 40;
        let producer = tokio::spawn({
            let output = shell.output.clone();
            async move {
                for _ in 0..chunks {
                    output.send(Event::Data(chunk.clone())).await.unwrap();
                }
            }
        });
        let mut received = 0;
        while received < desktop::OUTPUT_CREDIT {
            match next(&mut client).await {
                Some(ToApp::Output(data)) => received += data.len() as u64,
                other => panic!("unexpected {other:?}"),
            }
        }
        // An app that draws nothing for 30 s pauses the shell; the tab stays.
        assert!(
            tokio::time::timeout(Duration::from_secs(30), next(&mut client))
                .await
                .is_err()
        );
        assert_eq!(shell.output.capacity(), 0);
        assert!(!producer.is_finished());
        assert!(!task.is_finished());
        drop(shell);
        send(&mut client, ToBackend::Ack(received)).await;
        let mut total = received;
        loop {
            match next(&mut client).await {
                Some(ToApp::Output(data)) => {
                    total += data.len() as u64;
                    // The relay may already have ended after the last output.
                    let ack = ToBackend::Ack(data.len() as u64).encode();
                    let _ = client.write_all(&ack).await;
                }
                Some(ToApp::Event(event)) => {
                    assert_eq!(event, TestEvent::Error("Terminal is closed.".into()));
                    break;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(total, (chunks * desktop::MAX_DATA) as u64);
        assert!(next(&mut client).await.is_none());
        producer.await.unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_input_queue_holds_input_in_order_without_ending_the_tab() {
        let (mut client, mut shell, task) = open(1).await;
        for byte in 0..8u8 {
            send(
                &mut client,
                ToBackend::Input {
                    epoch: 0,
                    data: vec![byte],
                },
            )
            .await;
        }
        send(
            &mut client,
            ToBackend::Resize {
                cols: 100,
                rows: 40,
            },
        )
        .await;
        // A shell that reads nothing for 30 s is normal, not an error.
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert!(!task.is_finished());
        let mut applied = Vec::new();
        for _ in 0..8 {
            match shell.input.recv().await.unwrap() {
                Input::Data(data) => applied.extend(data),
                Input::Resize(..) => panic!("resize overtook input"),
                Input::End | Input::Activate => panic!("unexpected control input"),
            }
        }
        assert_eq!(applied, (0..8).collect::<Vec<u8>>());
        assert!(matches!(
            shell.input.recv().await,
            Some(Input::Resize(100, 40))
        ));
        let mut taken = 0;
        while taken < 8 {
            match next(&mut client).await {
                Some(ToApp::Taken(bytes)) => taken += bytes,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(taken, 8);
        drop(client);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn acknowledgements_pass_input_the_shell_is_not_reading() {
        let (mut client, shell, task) = open(1).await;
        // The shell blocks on its output and reads no input, as a program
        // echoing a paste does when the view falls behind.
        for _ in 0..4 {
            send(
                &mut client,
                ToBackend::Input {
                    epoch: 0,
                    data: vec![b'p'; 1024],
                },
            )
            .await;
        }
        let chunk = vec![b'x'; desktop::MAX_DATA];
        let chunks = desktop::OUTPUT_CREDIT as usize / desktop::MAX_DATA + 1;
        let output = shell.output.clone();
        let producer = tokio::spawn(async move {
            for _ in 0..chunks {
                output.send(Event::Data(chunk.clone())).await.unwrap();
            }
        });
        let mut received = 0;
        tokio::time::timeout(Duration::from_secs(5), async {
            while received < chunks as u64 * desktop::MAX_DATA as u64 {
                match next(&mut client).await {
                    Some(ToApp::Output(data)) => {
                        received += data.len() as u64;
                        send(&mut client, ToBackend::Ack(data.len() as u64)).await;
                    }
                    Some(ToApp::Taken(_)) => {}
                    other => panic!("unexpected {other:?}"),
                }
            }
        })
        .await
        .expect("output stalled behind input");
        producer.await.unwrap();
        drop(client);
        task.await.unwrap().unwrap();
        drop(shell);
    }

    #[tokio::test]
    async fn input_typed_for_a_replaced_shell_never_reaches_its_replacement() {
        let (mut client, mut shell, task) = open(8).await;
        shell
            .output
            .send(Event::Reconnecting {
                attempt: 1,
                message: "retry".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            next(&mut client).await,
            Some(ToApp::Event(TestEvent::Reconnecting { attempt: 1, .. }))
        ));
        send(
            &mut client,
            ToBackend::Input {
                epoch: 0,
                data: b"stale".to_vec(),
            },
        )
        .await;
        send(
            &mut client,
            ToBackend::Input {
                epoch: 1,
                data: b"fresh".to_vec(),
            },
        )
        .await;
        assert!(matches!(shell.input.recv().await, Some(Input::Data(data)) if data == b"fresh"));
        let mut taken = 0;
        while taken < 10 {
            match next(&mut client).await {
                Some(ToApp::Taken(bytes)) => taken += bytes,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(shell.input.try_recv().is_err());
        drop(client);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_shell_that_ends_closes_the_stream_after_its_last_event() {
        let (mut client, shell, task) = open(1).await;
        shell.output.send(Event::Ready).await.unwrap();
        shell
            .output
            .send(Event::Data(b"bye".to_vec()))
            .await
            .unwrap();
        shell.output.send(Event::Exit(Some(7))).await.unwrap();
        drop(shell);
        assert_eq!(
            next(&mut client).await,
            Some(ToApp::Event(TestEvent::Ready))
        );
        assert_eq!(
            next(&mut client).await,
            Some(ToApp::Output(b"bye".to_vec()))
        );
        assert_eq!(
            next(&mut client).await,
            Some(ToApp::Event(TestEvent::Exit(Some(7))))
        );
        assert!(next(&mut client).await.is_none());
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn input_beyond_the_window_closes_the_stream() {
        let (mut client, _shell, task) = open(1).await;
        let chunk = vec![0; desktop::MAX_DATA];
        let writer = tokio::spawn(async move {
            for _ in 0..=desktop::INPUT_WINDOW as usize / desktop::MAX_DATA + 1 {
                if client
                    .write_all(
                        &ToBackend::Input {
                            epoch: 0,
                            data: chunk.clone(),
                        }
                        .encode(),
                    )
                    .await
                    .is_err()
                {
                    break;
                }
            }
            client
        });
        assert!(task.await.unwrap().is_err());
        drop(writer.await.unwrap());
    }

    #[tokio::test]
    async fn replacement_while_input_waits_for_capacity_discards_the_old_epoch() {
        let (mut client, mut shell, task) = open(1).await;
        send(
            &mut client,
            ToBackend::Input {
                epoch: 0,
                data: b"first".to_vec(),
            },
        )
        .await;
        send(
            &mut client,
            ToBackend::Input {
                epoch: 0,
                data: b"blocked-stale".to_vec(),
            },
        )
        .await;
        while shell.input.len() != 1 {
            tokio::task::yield_now().await;
        }
        shell
            .output
            .send(Event::Reconnecting {
                attempt: 1,
                message: "replacement".into(),
            })
            .await
            .unwrap();
        loop {
            match next(&mut client).await {
                Some(ToApp::Event(TestEvent::Reconnecting { .. })) => break,
                Some(ToApp::Taken(_)) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(matches!(shell.input.recv().await, Some(Input::Data(data)) if data == b"first"));
        send(
            &mut client,
            ToBackend::Input {
                epoch: 1,
                data: b"fresh".to_vec(),
            },
        )
        .await;
        let received = tokio::time::timeout(Duration::from_secs(2), shell.input.recv())
            .await
            .unwrap();
        assert!(matches!(received, Some(Input::Data(data)) if data == b"fresh"));
        assert!(shell.input.try_recv().is_err());
        drop(client);
        task.await.unwrap().unwrap();
    }
}
