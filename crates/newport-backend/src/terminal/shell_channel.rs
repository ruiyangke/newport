//! A tab's shell on its server's link: a session channel with a PTY, running
//! the terminal command. While the link can't reach the server, the tab's
//! request waits in the link's queue; after a lost connection the tab opens a
//! new shell, and the app keeps the scrollback. The tab is its shell's one
//! Mac: using it shows the account's display here.
use super::{
    Event, Input, Shell,
    display::{Viewing, view},
};
use crate::{model::Server, server_session::ServerSession};
use anyhow::{Context, Result, bail};
use newport_ssh::{Fate, Link, Login, Open, Reach, Retry};
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf, client};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

/// How long the server may take to grant the PTY and start the command.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one keystroke write may wait for the server to take it.
const INPUT_TIMEOUT: Duration = Duration::from_secs(10);
/// The most output a shell may write before its command has started.
const SETUP_OUTPUT: usize = 4 * 1024 * 1024;
/// The largest output event, so one burst never holds the app's stream.
const OUTPUT_CHUNK: usize = 32 * 1024;
/// How long closing a finished shell's channel may take.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
const STALE_SESSION: &str = "Server session changed. Reconnect before retrying.";

/// How one shell ended.
enum End {
    Exited(Option<u32>),
    /// The app closed the tab's input.
    Closed,
    /// The connection under the shell closed; the tab opens another.
    Lost(newport_ssh::Error),
}

/// Runs the tab's shells until one exits, the tab closes, or the shell can't
/// be started for a reason a new connection wouldn't fix.
pub(crate) async fn run(
    owner: Arc<ServerSession>,
    server: &Server,
    terminal: uuid::Uuid,
    mut size: (u32, u32),
    mut input: mpsc::Receiver<Input>,
    output: &mpsc::Sender<Event>,
    shell: Shell,
) -> Result<()> {
    if !owner.server().same_connection(server) || owner.is_revoked() {
        bail!(STALE_SESSION);
    }
    let mut cancelled = owner.cancellation();
    let link = owner.link().clone();
    // Only someone using the tab here shows the display: never its opening by
    // itself, as when Newport restores it at launch.
    let mut viewing = Viewing::new(shell.display.clone().map(|access| view(&owner, access)));
    let display = shell.display.is_some();
    let run = async {
        // Opening a tab is the person asking: a link that stopped trying logs
        // in afresh rather than ending the tab with its earlier failure. The
        // link takes the retry before the tab's first open.
        if matches!(link.status().borrow().reach, Reach::Failed(_)) {
            owner.retry_fresh();
        }
        let routing = owner.integration_session_id();
        loop {
            let opened = tokio::select! {
                result = open(&link) => result,
                _ = discard_input(&mut input, &mut size) => return Ok(()),
            };
            let channel = match opened {
                Ok(channel) => channel,
                // The link already waited for a connection; an open that timed
                // out on one waits again.
                Err(error) if error.retry() == Retry::Backoff => continue,
                // The link stopped until something retries it. The tab waits
                // with the server, and comes back when anything brings the
                // link back; Return in the tab asks again.
                Err(error) if error.retry() == Retry::OnRetry => {
                    output.send(reconnecting(&link, &error)).await?;
                    tokio::select! {
                        result = link.open(Open::Session) => result?,
                        _ = retry_on_return(&owner, &mut input, &mut size) => return Ok(()),
                    }
                }
                Err(error) => return Err(error.into()),
            };
            let command = crate::ssh::terminal_command(routing, Some(terminal), display);
            let result = start(
                channel,
                Io {
                    size: &mut size,
                    input: &mut input,
                    output,
                },
                command,
                &mut viewing,
            )
            .await;
            match result? {
                End::Exited(exit) => {
                    output.send(Event::Exit(exit)).await?;
                    return Ok(());
                }
                End::Closed => return Ok(()),
                End::Lost(error) => {
                    output.send(reconnecting(&link, &error)).await?;
                }
            }
        }
    };
    tokio::select! {
        biased;
        _ = async { if !*cancelled.borrow() { let _ = cancelled.changed().await; } } => {
            bail!(STALE_SESSION)
        },
        result = run => result,
    }
}

/// A session channel, waiting in the link's queue while it can't reach the
/// server, but not past a failure only Retry can clear: the tab hears of it.
async fn open(link: &Link) -> Result<newport_ssh::Channel, newport_ssh::Error> {
    link.open_unless_stopped(Open::Session).await
}

/// What the tab shows while its next shell waits for the link.
pub(super) fn reconnecting(link: &Link, lost: &newport_ssh::Error) -> Event {
    let status = link.status().borrow().clone();
    let attempt = match status.reach {
        Reach::Reconnecting { attempt, .. } => attempt.max(1),
        _ => 1,
    };
    let message = match status.login {
        Some(Login::Failed(error)) => error.to_string(),
        _ => lost.to_string(),
    };
    Event::Reconnecting { attempt, message }
}

/// While the link is stopped, Return is the person asking to connect again;
/// other keystrokes are discarded. Ends when the tab closes or ends its input.
pub(super) async fn retry_on_return(
    owner: &ServerSession,
    input: &mut mpsc::Receiver<Input>,
    size: &mut (u32, u32),
) {
    while let Some(command) = input.recv().await {
        match command {
            Input::Resize(cols, rows) => *size = (cols, rows),
            Input::Data(data) if data.contains(&b'\r') => owner.retry_fresh(),
            Input::Data(_) | Input::Activate => {}
            Input::End => return,
        }
    }
}

/// Discard keystrokes for an unavailable shell, but keep the window size.
/// Ends when the tab closes or ends its input.
pub(super) async fn discard_input(input: &mut mpsc::Receiver<Input>, size: &mut (u32, u32)) {
    while let Some(command) = input.recv().await {
        match command {
            Input::Resize(cols, rows) => *size = (cols, rows),
            Input::End => return,
            Input::Data(_) | Input::Activate => {}
        }
    }
}

/// A shell's local ends: its size, the user's input and the events it emits.
struct Io<'a> {
    size: &'a mut (u32, u32),
    input: &'a mut mpsc::Receiver<Input>,
    output: &'a mpsc::Sender<Event>,
}

/// The channel's write half, closed when the shell's run ends however it
/// ends, so its session slot is given back.
struct Writer {
    half: Option<ChannelWriteHalf<client::Msg>>,
    runtime: tokio::runtime::Handle,
}
impl std::ops::Deref for Writer {
    type Target = ChannelWriteHalf<client::Msg>;
    fn deref(&self) -> &Self::Target {
        self.half
            .as_ref()
            .expect("a writer holds its half until dropped")
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        if let Some(half) = self.half.take() {
            self.runtime.spawn(async move {
                let _ = tokio::time::timeout(CLOSE_TIMEOUT, half.close()).await;
            });
        }
    }
}

async fn request_reply(
    reader: &mut ChannelReadHalf,
    request: &str,
    early_output: &mut Vec<u8>,
) -> Result<()> {
    loop {
        match reader.wait().await {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure) => bail!("Server refused {request}."),
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                if early_output.len() + data.len() > SETUP_OUTPUT {
                    bail!("Terminal setup output exceeded 4 MiB");
                }
                early_output.extend_from_slice(&data);
            }
            Some(ChannelMsg::Eof | ChannelMsg::Close) | None => {
                bail!("SSH channel closed while requesting {request}.");
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                bail!("Remote shell exited during setup (status {exit_status}).")
            }
            Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                bail!("Remote shell ended during setup: {signal_name:?}")
            }
            // Flow-control messages can arrive before the request's reply.
            _ => {}
        }
    }
}

/// Why a channel ended, when it was its connection's loss.
async fn connection_lost(fate: &Fate) -> Option<newport_ssh::Error> {
    match fate.error() {
        Some(error) => Some(error),
        None => crate::remote::lost(fate).await,
    }
}

/// One shell on `channel`: the PTY and the terminal command, then output and
/// input until the shell ends.
async fn start(
    channel: newport_ssh::Channel,
    Io {
        size,
        input,
        output,
    }: Io<'_>,
    command: String,
    viewing: &mut Viewing,
) -> Result<End> {
    let fate = channel.fate();
    let (mut reader, half) = channel.split();
    let writer = Writer {
        half: Some(half),
        runtime: tokio::runtime::Handle::current(),
    };
    let mut early_output = Vec::new();
    let setup = tokio::time::timeout(SETUP_TIMEOUT, async {
        writer
            .request_pty(true, "xterm-256color", size.0, size.1, 0, 0, &[])
            .await?;
        request_reply(&mut reader, "the terminal PTY", &mut early_output).await?;
        // An exec request avoids relying on sshd's AcceptEnv configuration.
        writer.exec(true, command).await?;
        request_reply(&mut reader, "an interactive shell", &mut early_output).await?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Terminal setup timed out");
    match setup {
        Ok(Ok(())) => {}
        Ok(Err(error)) | Err(error) => {
            return match connection_lost(&fate).await {
                Some(lost) => Ok(End::Lost(lost)),
                None => Err(error),
            };
        }
    }
    // Only input for this shell is forwarded; earlier keystrokes are dropped,
    // but not that someone used the tab.
    while let Ok(command) = input.try_recv() {
        match command {
            Input::Resize(cols, rows) => *size = (cols, rows),
            Input::Activate => viewing.activated(),
            Input::Data(_) | Input::End => {}
        }
    }
    if writer.window_change(size.0, size.1, 0, 0).await.is_err() {
        if let Some(lost) = connection_lost(&fate).await {
            return Ok(End::Lost(lost));
        }
        bail!("The terminal's channel closed before its shell started.");
    }
    output.send(Event::Ready).await?;
    // The tab is this shell's only Mac.
    output.send(Event::Active(true)).await?;
    for bytes in early_output.chunks(OUTPUT_CHUNK) {
        output.send(Event::Data(bytes.to_vec())).await?;
    }
    let receive = async {
        let mut exit = None;
        while let Some(message) = reader.wait().await {
            match message {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    for bytes in data.chunks(OUTPUT_CHUNK) {
                        output.send(Event::Data(bytes.to_vec())).await?;
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => exit = Some(exit_status),
                ChannelMsg::ExitSignal { signal_name, .. } => {
                    bail!("Remote shell ended: {signal_name:?}")
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok::<_, anyhow::Error>(exit)
    };
    let send = async {
        loop {
            tokio::select! {
                command = input.recv() => match command {
                    Some(Input::Data(bytes)) => {
                        tokio::time::timeout(INPUT_TIMEOUT, writer.data(bytes.as_slice()))
                            .await
                            .context("Terminal input timed out")??;
                    }
                    Some(Input::Resize(cols, rows)) => {
                        *size = (cols, rows);
                        writer.window_change(cols, rows, 0, 0).await?;
                    }
                    // This shell belongs to the connection: ending it is closing it.
                    Some(Input::End) | None => break,
                    Some(Input::Activate) => viewing.activated(),
                },
                () = viewing.lost() => output.send(Event::DisplayLost).await?,
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    enum Completion {
        Receive(Result<Option<u32>>),
        Send(Result<()>),
    }
    // Read and write independently, so echoed input can't deadlock a paste.
    let completion = tokio::select! {
        result = receive => Completion::Receive(result),
        result = send => Completion::Send(result),
    };
    // An exit status is authoritative, even when the connection closed right
    // after it; a channel that ends without one may have lost its connection.
    let lost = match &completion {
        Completion::Receive(Ok(Some(_))) | Completion::Send(Ok(())) => None,
        _ => connection_lost(&fate).await,
    };
    match (completion, lost) {
        (Completion::Receive(Ok(Some(exit))), _) => Ok(End::Exited(Some(exit))),
        (Completion::Send(Ok(())), _) => Ok(End::Closed),
        (_, Some(lost)) => Ok(End::Lost(lost)),
        (Completion::Receive(Ok(None)), None) => Ok(End::Exited(None)),
        (Completion::Receive(Err(error)) | Completion::Send(Err(error)), None) => Err(error),
    }
}
