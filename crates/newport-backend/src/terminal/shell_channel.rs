//! A tab's shell on its server's link: a session channel with a PTY, running
//! the terminal command. While the link can't reach the server, the tab's
//! request waits in the link's queue; after a lost connection the tab opens a
//! new shell, and the app keeps the scrollback.
use super::{
    Event, Input, Shell,
    display::{Prepared, Unavailable},
};
use crate::{model::Server, server_session::ServerSession, ssh::terminal_startup};
use anyhow::{Context, Result, bail};
use newport_ssh::{Fate, Link, Login, Open, Reach, Retry};
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf, client};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

/// How long the server may take to grant the PTY and start the command.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the shell may take to say whether it joined the shared display.
const READINESS_TIMEOUT: Duration = Duration::from_secs(10);
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
    let run = async {
        // Opening a tab is the person asking: a link that stopped trying logs
        // in afresh rather than ending the tab with its earlier failure. The
        // link takes the retry before the tab's first open.
        if matches!(link.status().borrow().reach, Reach::Failed(_)) {
            owner.retry_fresh();
        }
        let selector = owner.integration_session_id();
        let gui = tokio::select! {
            result = shell.display.prepare(&owner) => result?,
            _ = discard_input(&mut input, &mut size) => return Ok(()),
        };
        // A shell's state owns its display, so a display it drops is released.
        let mut gui = Arc::new(gui);
        if let Some(shell) = &shell.terminal {
            shell.initialize(gui).map_err(anyhow::Error::msg)?;
            gui = Arc::new(Prepared::None);
        }
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
                Err(error) => return Err(error.into()),
            };
            let attempt = shell
                .terminal
                .as_ref()
                .map(|shell| shell.begin())
                .transpose()
                .map_err(anyhow::Error::msg)?;
            let current_gui = shell
                .terminal
                .as_ref()
                .map(|shell| shell.display())
                .transpose()
                .map_err(anyhow::Error::msg)?
                .flatten()
                .unwrap_or_else(|| gui.clone());
            let failed = Arc::downgrade(&current_gui);
            let result = start(
                channel,
                Io {
                    size: &mut size,
                    input: &mut input,
                    output,
                },
                selector,
                current_gui,
                attempt.as_ref().map(|attempt| attempt.identity),
                shell.terminal.as_deref(),
            )
            .await;
            drop(attempt);
            // A shared display that fails a shell's startup is dropped, and
            // the shell starts again at once without it. The failure is
            // logged, never reported to the terminal.
            if let Err(error) = &result
                && error.is::<Unavailable>()
            {
                log::info!("Terminal starts again without the shared display: {error:#}");
                match &shell.terminal {
                    Some(shell) => {
                        if let Some(failed) = failed.upgrade() {
                            shell.forget_display(&failed);
                        }
                    }
                    None => gui = Arc::new(Prepared::None),
                }
                continue;
            }
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
/// server, but not past a failure only Retry can clear: the tab ends with it.
async fn open(link: &Link) -> Result<newport_ssh::Channel, newport_ssh::Error> {
    link.open_unless_stopped(Open::Session).await
}

/// What the tab shows while its next shell waits for the link.
fn reconnecting(link: &Link, lost: &newport_ssh::Error) -> Event {
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

/// Discard keystrokes for an unavailable shell, but keep the window size.
async fn discard_input(input: &mut mpsc::Receiver<Input>, size: &mut (u32, u32)) {
    while let Some(command) = input.recv().await {
        if let Input::Resize(cols, rows) = command {
            *size = (cols, rows);
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

/// The shell exited before it started; with a shared display, that is the
/// display's failure.
#[derive(Debug)]
struct SetupExited(String);
impl std::fmt::Display for SetupExited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for SetupExited {}

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
                return Err(SetupExited(format!(
                    "Remote shell exited during setup (status {exit_status})."
                ))
                .into());
            }
            Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                return Err(SetupExited(format!(
                    "Remote shell ended during setup: {signal_name:?}"
                ))
                .into());
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

/// One shell on `channel`: the PTY, the terminal command with its display
/// handshake, then output and input until the shell ends.
async fn start(
    channel: newport_ssh::Channel,
    Io {
        size,
        input,
        output,
    }: Io<'_>,
    integration: uuid::Uuid,
    gui: Arc<Prepared>,
    terminal: Option<super::shell::Identity>,
    shell: Option<&super::shell::State>,
) -> Result<End> {
    let fate = channel.fate();
    let (mut reader, half) = channel.split();
    let writer = Writer {
        half: Some(half),
        runtime: tokio::runtime::Handle::current(),
    };
    let mut early_output = Vec::new();
    let mut readiness = matches!(*gui, Prepared::Shared(_)).then(terminal_startup::Readiness::new);
    let setup = tokio::time::timeout(SETUP_TIMEOUT, async {
        writer
            .request_pty(true, "xterm-256color", size.0, size.1, 0, 0, &[])
            .await?;
        request_reply(&mut reader, "the terminal PTY", &mut early_output).await?;
        // An exec request avoids relying on sshd's AcceptEnv configuration.
        writer
            .exec(
                true,
                crate::ssh::terminal_command_preflight(
                    integration,
                    &gui,
                    terminal,
                    readiness.as_ref(),
                )
                .map_err(|error| gui.failure(error))?,
            )
            .await?;
        if let Err(error) =
            request_reply(&mut reader, "an interactive shell", &mut early_output).await
        {
            // An early exit may come before the exec's reply. With a shared
            // display, an exit before readiness is the display's failure.
            if readiness.is_some() && error.is::<SetupExited>() {
                return Err(gui.failure(error.to_string()));
            }
            return Err(error);
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Terminal setup timed out");
    match setup {
        Ok(Ok(())) => {}
        Ok(Err(error)) if error.is::<Unavailable>() => return Err(error),
        Ok(Err(error)) | Err(error) => {
            return match connection_lost(&fate).await {
                Some(lost) => Ok(End::Lost(lost)),
                None => Err(error),
            };
        }
    }
    if let Some(readiness) = &mut readiness {
        let preflight = async {
            loop {
                match readiness
                    .strip(&mut early_output)
                    .map_err(|error| gui.failure(error))?
                {
                    Some(terminal_startup::Ack::Ready) => {
                        // The shell checked the remote paths. Also fence the
                        // locally held generation before any queued input.
                        gui.assignments(integration)
                            .map_err(|error| gui.failure(error))?;
                        return Ok::<_, anyhow::Error>(Ok(terminal_startup::Ack::Ready));
                    }
                    Some(ack) => return Ok(Ok(ack)),
                    None => {}
                }
                match reader.wait().await {
                    Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                        early_output.extend_from_slice(&data);
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        return Err(gui.failure(format!("Graphical terminal preflight failed (status {exit_status}). Reconnect this terminal.")));
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        return Err(gui.failure(format!("Graphical terminal preflight ended: {signal_name:?}. Reconnect this terminal.")));
                    }
                    Some(ChannelMsg::Close) | None => {
                        // A lost connection closes its channels just before
                        // the link reports the loss.
                        if let Some(lost) = connection_lost(&fate).await {
                            return Ok(Err(lost));
                        }
                        return Err(gui.failure("Graphical terminal closed before display readiness was confirmed. Reconnect this terminal.".into()));
                    }
                    _ => {}
                }
            }
        };
        let ack = tokio::time::timeout(READINESS_TIMEOUT, preflight)
            .await
            .map_err(|_| {
                gui.failure(
                    "Graphical terminal readiness timed out. Reconnect this terminal.".into(),
                )
            })??;
        let ack = match ack {
            Ok(ack) => ack,
            Err(lost) => return Ok(End::Lost(lost)),
        };
        // The shell started without the display. Drop it, so a GUI app
        // launched later can start another.
        if ack == terminal_startup::Ack::Unavailable {
            log::info!(
                "Terminal started without the shared display: its endpoints were unavailable"
            );
            if let Some(shell) = shell {
                shell.forget_display(&gui);
            }
        }
    }
    // The shell's state keeps any display it uses from here on.
    drop(gui);
    // Only input for this shell is forwarded; earlier keystrokes are dropped.
    while let Ok(command) = input.try_recv() {
        if let Input::Resize(cols, rows) = command {
            *size = (cols, rows);
        }
    }
    if writer.window_change(size.0, size.1, 0, 0).await.is_err() {
        if let Some(lost) = connection_lost(&fate).await {
            return Ok(End::Lost(lost));
        }
        bail!("The terminal's channel closed before its shell started.");
    }
    output.send(Event::Ready).await?;
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
        while let Some(command) = input.recv().await {
            match command {
                Input::Data(bytes) => {
                    tokio::time::timeout(INPUT_TIMEOUT, writer.data(bytes.as_slice()))
                        .await
                        .context("Terminal input timed out")??;
                }
                Input::Resize(cols, rows) => {
                    *size = (cols, rows);
                    writer.window_change(cols, rows, 0, 0).await?;
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    enum Completion {
        Receive(Result<Option<u32>>),
        Send(Result<()>),
    }
    let gui_notice = async {
        if let Some(shell) = shell {
            let error = shell.gui_failure().await;
            log::info!("A terminal's shared display ended; its shell stays connected: {error}");
        }
        // Its environment stays fenced; only a new shell may get a
        // replacement. The shell itself keeps running.
        std::future::pending::<Result<()>>().await
    };
    // Read and write independently, so echoed input can't deadlock a paste.
    let completion = tokio::select! {
        result = receive => Completion::Receive(result),
        result = send => Completion::Send(result),
        result = gui_notice => Completion::Send(result),
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
