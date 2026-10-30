//! A terminal is held until the connection that opened it ends, and that
//! connection carries the terminal's I/O (`protocol::terminal`).
use super::{Host, TerminalChannel, TerminalOpen};
use crate::terminal::{Event, Input, Io};
use newport_platform::ipc::asynchronous::Stream;
use newport_platform::ipc::protocol::terminal::{self as wire, ToApp, ToBackend};
use newport_platform::ipc::protocol::{Response, VERSION};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, mpsc};

pub(super) async fn serve<H: Host>(stream: Stream, host: &H, args: Value) -> anyhow::Result<()> {
    hold(stream, async move {
        let request: TerminalOpen =
            serde_json::from_value(args).map_err(|error| error.to_string())?;
        let TerminalChannel { io, guard } = host.open_terminal(request).await?;
        Ok((io, guard))
    })
    .await
}

async fn hold<S, G>(
    mut stream: S,
    open: impl std::future::Future<Output = Result<(Io, G), String>>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let result = tokio::select! {
        result = open => result,
        _ = stream.read_u8() => return Ok(()),
    };
    let response = Response {
        version: VERSION,
        result: Value::Null,
        error: result.as_ref().err().map(|error| json!(error)),
    };
    // Keep the guard alive while acknowledging and relaying. Failed responses,
    // EOF, a malformed frame or cancellation all release only this PTY.
    let opened = result.ok();
    let bytes = rmp_serde::to_vec_named(&response)?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await
    })
    .await??;
    if let Some((io, guard)) = opened {
        let result = relay(stream, io).await;
        drop(guard);
        return result;
    }
    Ok(())
}

/// What the relay's directions share.
#[derive(Default)]
struct Flow {
    /// Output bytes the app has acknowledged.
    acked: AtomicU64,
    acknowledged: Notify,
    /// Input bytes the shell's queue has taken or that were dropped as stale.
    taken: AtomicU64,
    took: Notify,
    /// Shells the stream has left behind.
    epoch: AtomicU64,
}

enum Queued {
    Data { epoch: u64, data: Vec<u8> },
    Resize(u32, u32),
}

/// Carries the terminal's I/O until the app closes the socket or the shell's
/// output ends. Nothing here times out: credit and the input window pace it.
async fn relay<S: AsyncRead + AsyncWrite>(stream: S, io: Io) -> anyhow::Result<()> {
    let Io { input, mut output } = io;
    let (mut reader, mut writer) = tokio::io::split(stream);
    let flow = Flow::default();
    let (queue, mut queued) = mpsc::unbounded_channel();
    // Reading never waits for the shell, so acknowledgements arrive while
    // input waits; the app's input window bounds the queue.
    let receive = async {
        let mut received = 0u64;
        while let Some((kind, payload)) = read_frame(&mut reader).await? {
            match ToBackend::decode(kind, payload).map_err(anyhow::Error::msg)? {
                ToBackend::Ack(bytes) => {
                    flow.acked.fetch_add(bytes, Ordering::AcqRel);
                    flow.acknowledged.notify_one();
                }
                ToBackend::Input { epoch, data } => {
                    received += data.len() as u64;
                    anyhow::ensure!(
                        received - flow.taken.load(Ordering::Acquire) <= wire::INPUT_WINDOW,
                        "Terminal input exceeded its window"
                    );
                    let _ = queue.send(Queued::Data { epoch, data });
                }
                ToBackend::Resize { cols, rows } => {
                    let _ = queue.send(Queued::Resize(cols.into(), rows.into()));
                }
            }
        }
        anyhow::Ok(())
    };
    // Input reaches the shell in order; a full shell queue waits here.
    let apply = async {
        while let Some(item) = queued.recv().await {
            match item {
                Queued::Data { epoch, data } => {
                    let length = data.len() as u64;
                    if epoch == flow.epoch.load(Ordering::Acquire) {
                        let _ = input.send(Input::Data(data)).await;
                    }
                    flow.taken.fetch_add(length, Ordering::AcqRel);
                    flow.took.notify_one();
                }
                Queued::Resize(cols, rows) => {
                    if crate::terminal::size(cols, rows).is_ok() {
                        let _ = input.send(Input::Resize(cols, rows)).await;
                    }
                }
            }
        }
        std::future::pending().await
    };
    // Output is taken from the shell only while the app has credit for it;
    // otherwise it backs up into the shell's channel and pauses only it.
    let send = async {
        let (mut sent, mut reported, mut ended) = (0u64, 0u64, false);
        // An event met while gathering output, sent before anything newer.
        let mut held = None;
        loop {
            let credit =
                sent.saturating_sub(flow.acked.load(Ordering::Acquire)) < wire::OUTPUT_CREDIT;
            let frames = tokio::select! {
                biased;
                _ = flow.took.notified() => {
                    let taken = flow.taken.load(Ordering::Acquire);
                    if taken == reported {
                        continue;
                    }
                    let frame = ToApp::Taken(taken - reported).encode();
                    reported = taken;
                    frame
                }
                _ = flow.acknowledged.notified(), if !credit => continue,
                event = async {
                    match held.take() {
                        Some(event) => Some(event),
                        None => output.recv().await,
                    }
                }, if credit => match event {
                    Some(Event::Data(mut data)) => {
                        // Output already waiting goes out with it, so a burst of
                        // small reads reaches the app as few full frames.
                        while data.len() < wire::MAX_DATA {
                            match output.try_recv() {
                                Ok(Event::Data(more)) => data.extend(more),
                                Ok(event) => {
                                    held = Some(event);
                                    break;
                                }
                                Err(_) => break,
                            }
                        }
                        sent += data.len() as u64;
                        data.chunks(wire::MAX_DATA)
                            .flat_map(|chunk| ToApp::Output(chunk.to_vec()).encode())
                            .collect()
                    }
                    Some(event) => {
                        let event = wire_event(event);
                        if event.ends_shell() {
                            flow.epoch.fetch_add(1, Ordering::AcqRel);
                        }
                        ended |= event.ends_stream();
                        ToApp::Event(event).encode()
                    }
                    // Closed without a last event: the backend closed the tab.
                    None if !ended => {
                        ended = true;
                        ToApp::Event(wire::Event::Error("Terminal is closed.".into())).encode()
                    }
                    None => {
                        writer.shutdown().await?;
                        return anyhow::Ok(());
                    }
                },
            };
            writer.write_all(&frames).await?;
        }
    };
    tokio::select! {
        result = receive => result,
        result = send => result,
        result = apply => result,
    }
}

fn wire_event(event: Event) -> wire::Event {
    match event {
        Event::Ready => wire::Event::Ready,
        Event::Reconnecting { attempt, message } => wire::Event::Reconnecting { attempt, message },
        Event::Exit(code) => wire::Event::Exit(code),
        Event::Error(message) => wire::Event::Error(message),
        Event::Data(_) => unreachable!("output is sent as data frames"),
    }
}

/// The next frame, or `None` when the app closes the socket between frames.
async fn read_frame(
    reader: &mut (impl AsyncRead + Unpin),
) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0; wire::HEADER];
    if reader.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut header[1..]).await?;
    let (kind, length) = wire::header(header)?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    Ok(Some((kind, payload)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::DuplexStream;
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
    async fn response(stream: &mut DuplexStream) -> Response {
        let len = stream.read_u32().await.unwrap();
        let mut data = vec![0; len as usize];
        stream.read_exact(&mut data).await.unwrap();
        rmp_serde::from_slice(&data).unwrap()
    }
    async fn next(stream: &mut DuplexStream) -> Option<ToApp> {
        let (kind, payload) = read_frame(stream).await.unwrap()?;
        Some(ToApp::decode(kind, payload).unwrap())
    }
    async fn send(stream: &mut DuplexStream, frame: ToBackend) {
        stream.write_all(&frame.encode()).await.unwrap();
    }
    /// An open terminal relayed over a socket, and the app's end of it.
    async fn open(
        input: usize,
    ) -> (
        DuplexStream,
        Shell,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    ) {
        let (mut client, server) = tokio::io::duplex(4 * wire::MAX_DATA);
        let (io, shell) = terminal(input);
        let task = tokio::spawn(hold(server, async move { Ok((io, ())) }));
        assert!(response(&mut client).await.error.is_none());
        (client, shell, task)
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
            Some(json!("Terminal session already exists."))
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
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

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
        let (mut client, server) = tokio::io::duplex(4 * wire::MAX_DATA);
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
            Some(ToApp::Event(wire::Event::Exit(Some(0))))
        );
        assert_eq!(
            next(&mut client).await,
            Some(ToApp::Output(b"after".to_vec()))
        );
        drop(shell);
        drop(client);
        let _ = task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn output_stops_when_credit_runs_out_and_an_ack_resumes_it() {
        let (mut client, shell, task) = open(1).await;
        let chunk = vec![b'x'; wire::MAX_DATA];
        let chunks = 2 * (wire::OUTPUT_CREDIT as usize / wire::MAX_DATA) + 40;
        let producer = tokio::spawn({
            let output = shell.output.clone();
            async move {
                for _ in 0..chunks {
                    output.send(Event::Data(chunk.clone())).await.unwrap();
                }
            }
        });
        let mut received = 0;
        while received < wire::OUTPUT_CREDIT {
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
                    assert_eq!(event, wire::Event::Error("Terminal is closed.".into()));
                    break;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(total, (chunks * wire::MAX_DATA) as u64);
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
        let chunk = vec![b'x'; wire::MAX_DATA];
        let chunks = wire::OUTPUT_CREDIT as usize / wire::MAX_DATA + 1;
        let output = shell.output.clone();
        let producer = tokio::spawn(async move {
            for _ in 0..chunks {
                output.send(Event::Data(chunk.clone())).await.unwrap();
            }
        });
        let mut received = 0;
        tokio::time::timeout(Duration::from_secs(5), async {
            while received < chunks as u64 * wire::MAX_DATA as u64 {
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
            Some(ToApp::Event(wire::Event::Reconnecting { attempt: 1, .. }))
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
            Some(ToApp::Event(wire::Event::Ready))
        );
        assert_eq!(
            next(&mut client).await,
            Some(ToApp::Output(b"bye".to_vec()))
        );
        assert_eq!(
            next(&mut client).await,
            Some(ToApp::Event(wire::Event::Exit(Some(7))))
        );
        assert!(next(&mut client).await.is_none());
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn input_beyond_the_window_closes_the_stream() {
        let (mut client, _shell, task) = open(1).await;
        let chunk = vec![0; wire::MAX_DATA];
        let writer = tokio::spawn(async move {
            for _ in 0..=wire::INPUT_WINDOW as usize / wire::MAX_DATA + 1 {
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
}
