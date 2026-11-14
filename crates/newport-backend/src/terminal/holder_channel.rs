//! A tab's shell held on its server: Needle runs
//! it in a holder process there, so a lost connection, Quit or a crash only
//! detaches the tab. Attaching again replays the holder's copy of the
//! terminal into the tab and carries on, with no new shell.
use super::{
    Colors, Event, Input, Shell,
    display::{Access, Viewing, view},
    shell_channel::{discard_input, reconnecting, retry_on_return},
};
use crate::{agent::Service, agent_runtime::Error, model::Server, server_session::ServerSession};
use anyhow::{Context, Result, bail};
use newport_protocol::{
    encoding::protobuf::{FrameError, tokio::FrameReader},
    runtime::Hello,
    terminal::{
        self as service, Computer, Reply, Request,
        holder::{self as protocol, Attach, ClientMessage, ServerMessage},
        wire,
    },
};
use newport_ssh::{Link, Login, Open, Reach, Retry};
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};
use uuid::Uuid;

/// One budget for the holder greeting, attachment and initial screen.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a worker request may take, starting a holder included.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one keystroke write may wait for the server to take it.
const INPUT_TIMEOUT: Duration = Duration::from_secs(10);
/// The largest output event, so one burst never holds the app's stream.
const OUTPUT_CHUNK: usize = 32 * 1024;
/// How long a restored tab waits before asking a failing Needle again.
const RETRY_DELAY: Duration = Duration::from_secs(2);
const STALE_SESSION: &str = "Server session changed. Reconnect before retrying.";

/// This computer's name, which other Macs show for its tabs. A Mac's id
/// for a server is its integration routing id there
/// (`ServerSession::integration_session_id`), which Needle's routes are keyed
/// by, so a held shell's requests can be routed to it.
pub(crate) static COMPUTER: LazyLock<String> =
    LazyLock::new(newport_platform::account::computer_name);

/// What a held shell runs: the login shell, with only what never changes for
/// a shell several Macs share.
/// It names no Mac and no display: each of its programs' requests picks an
/// attached Mac when it is made.
pub(super) fn held_command(terminal: Uuid) -> String {
    let script = format!(
        "NEWPORT_TERMINAL={terminal}; \
         PATH=\"$HOME/.local/bin${{PATH:+:$PATH}}\"; \
         BROWSER=\"$HOME/.local/bin/newport-browser\"; \
         export NEWPORT_TERMINAL PATH BROWSER; exec \"$SHELL\" -l"
    );
    format!("exec sh -c {}", crate::ssh::shell_quote(&script))
}

/// A shell Needle holds for a terminal.
#[derive(Clone, Debug)]
pub(crate) struct Holder {
    id: String,
    socket: String,
}

pub(crate) enum Found {
    Running(Holder),
    /// No holder for this terminal, or Needle can't hold shells here.
    Gone,
    /// The link stopped until something retries it, as after a refused login.
    Stopped,
    /// A permanent refusal must keep the listed shell visible.
    Failed(Error),
}

/// A terminal's holder id: the terminal's UUID without dashes, short enough
/// for the server's socket paths.
pub(super) fn holder_id(terminal: Uuid) -> String {
    terminal.simple().to_string()
}

/// Asks the terminal worker one thing.
pub(super) async fn request(
    owner: &Arc<ServerSession>,
    server: &Server,
    body: Request,
) -> Result<Reply, Error> {
    request_with(owner, server, |_| Ok(body)).await
}

/// Asks the terminal worker one thing, phrased for the Needle it runs in.
async fn request_with(
    owner: &Arc<ServerSession>,
    server: &Server,
    body: impl FnOnce(&Hello) -> Result<Request, Error>,
) -> Result<Reply, Error> {
    let needle = owner.needle(server).map_err(|_| Error::Stale)?;
    let mut stream = needle.launch(Service::Terminal).await?;
    let hello = needle.runtime_hello().map_err(Error::Unavailable)?;
    let body = body(&hello)?;
    require_holder(&hello, &body)?;
    exchange_with_timeout(&mut stream, &body, REQUEST_TIMEOUT).await
}

async fn exchange_with_timeout(
    stream: &mut (impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin),
    request: &Request,
    timeout: Duration,
) -> Result<Reply, Error> {
    tokio::time::timeout(timeout, exchange(stream, request))
        .await
        .map_err(|_| {
            uncertain(
                request,
                Error::Interrupted("The terminal worker timed out.".into()),
            )
        })?
}

fn uncertain(request: &Request, error: Error) -> Error {
    if matches!(
        request,
        Request::Start { .. } | Request::Rename { .. } | Request::End { .. }
    ) {
        Error::Remote(newport_protocol::common::Error {
            code: "holder_outcome_unknown".into(),
            message: format!(
                "The terminal operation's outcome is unknown; it was not retried. {error}"
            ),
            retry: newport_protocol::common::Retry::Unknown,
        })
    } else {
        error
    }
}

fn require_holder(hello: &Hello, request: &Request) -> Result<(), Error> {
    if matches!(
        request,
        Request::Start { .. } | Request::Rename { .. } | Request::End { .. }
    ) {
        hello
            .require(protocol::CAPABILITY, protocol::VERSION)
            .map_err(Error::Remote)?;
    }
    Ok(())
}

async fn exchange(
    stream: &mut (impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin),
    request: &Request,
) -> Result<Reply, Error> {
    let bytes =
        wire::encode_request(request).map_err(|error| Error::Unavailable(error.to_string()))?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|error| uncertain(request, Error::Interrupted(error.to_string())))?;
    stream
        .flush()
        .await
        .map_err(|error| uncertain(request, Error::Interrupted(error.to_string())))?;
    let mut reader = FrameReader::new(stream, service::SERVER_LIMITS);
    let body = reader
        .next()
        .await
        .map_err(|error| {
            uncertain(
                request,
                match error {
                    FrameError::Io(_) | FrameError::Truncated => {
                        Error::Interrupted(error.to_string())
                    }
                    _ => Error::Unavailable(error.to_string()),
                },
            )
        })?
        .ok_or_else(|| {
            uncertain(
                request,
                Error::Interrupted("The terminal worker closed early.".into()),
            )
        })?;
    let reply = wire::decode_reply(body)
        .map_err(|error| uncertain(request, Error::Unavailable(error.to_string())))?;
    reply
        .validate_for(request)
        .map_err(|error| uncertain(request, Error::Unavailable(error.to_string())))?;
    let (stream, buffered) = reader.into_parts();
    if !buffered.is_empty() {
        return Err(uncertain(
            request,
            Error::Unavailable("The terminal worker sent trailing bytes.".into()),
        ));
    }
    // This service requires actual EOF; a transport's UnexpectedEof is not closure evidence.
    let mut extra = [0];
    if stream
        .read(&mut extra)
        .await
        .map_err(|error| uncertain(request, Error::Interrupted(error.to_string())))?
        != 0
    {
        return Err(uncertain(
            request,
            Error::Unavailable("The terminal worker sent trailing bytes.".into()),
        ));
    }
    match reply {
        Reply::Failed(error) => Err(Error::Remote(error)),
        reply => Ok(reply),
    }
}

async fn list(owner: &Arc<ServerSession>, server: &Server) -> Result<Vec<Holder>, Error> {
    match request(owner, server, Request::List).await? {
        Reply::Listed(holders) => Ok(holders
            .into_iter()
            .map(|holder| Holder {
                id: holder.id,
                socket: holder.socket,
            })
            .collect()),
        _ => Err(Error::Unavailable(
            "The terminal worker sent no list.".into(),
        )),
    }
}

/// Resolves once the link stops until something retries it, as after a
/// refused login: a request to Needle would wait for that retry.
async fn stopped(link: &Link) {
    let mut status = link.status();
    loop {
        {
            let status = status.borrow_and_update();
            if matches!(status.reach, Reach::Failed(_))
                || matches!(status.login, Some(Login::Failed(_)))
            {
                return;
            }
        }
        if status.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
}

/// A restored tab's holder. It keeps asking while the server is out of
/// reach, and reports a link that waits for a retry.
async fn find(owner: &Arc<ServerSession>, server: &Server, terminal: Uuid) -> Found {
    let id = holder_id(terminal);
    loop {
        let found = tokio::select! {
            result = list(owner, server) => result,
            _ = stopped(owner.link()) => return Found::Stopped,
        };
        return match found {
            Ok(holders) => holders
                .into_iter()
                .find(|holder| holder.id == id)
                .map_or(Found::Gone, Found::Running),
            Err(error) if error.transient() => {
                tokio::time::sleep(RETRY_DELAY).await;
                continue;
            }
            Err(error) => Found::Failed(error),
        };
    }
}

/// What a restored tab shows while its link waits for a retry.
fn waiting(link: &Link) -> Event {
    let message = match &link.status().borrow().login {
        Some(Login::Failed(error)) => error.to_string(),
        _ => "Waiting to reconnect to the server.".into(),
    };
    Event::Reconnecting {
        attempt: 1,
        message,
    }
}

/// Whether a Needle shows the account's stable display on the computer in
/// use, so its holders can be told to expect it.
fn shows_stable_display(hello: &Hello) -> bool {
    hello
        .require(newport_protocol::runtime::service::REMOTE_APPS, 1)
        .is_ok()
}

/// The worker's `start` for a terminal.
fn start_request(
    terminal: Uuid,
    (columns, rows): (u32, u32),
    creator: Uuid,
    stable_display: bool,
) -> Result<Request, Error> {
    let invalid = |_| Error::Unavailable("Invalid terminal size.".into());
    Ok(Request::Start {
        id: holder_id(terminal),
        columns: columns.try_into().map_err(invalid)?,
        rows: rows.try_into().map_err(invalid)?,
        command: held_command(terminal),
        creator: Computer {
            id: creator.to_string(),
            name: COMPUTER.clone(),
        },
        stable_display,
    })
}

/// Starts or finds the requested holder exactly once. An uncertain start never
/// substitutes another shell or sends an automatic End: a completed holder stays
/// listed so its owner can decide what to do.
async fn start(
    owner: &Arc<ServerSession>,
    server: &Server,
    terminal: Uuid,
    size: (u32, u32),
    access: Option<&Access>,
) -> Result<Holder, Error> {
    let named = access.is_some();
    let creator = owner.integration_session_id();
    match request_with(owner, server, |hello| {
        start_request(
            terminal,
            size,
            creator,
            named && shows_stable_display(hello),
        )
    })
    .await?
    {
        Reply::Started { socket } => Ok(Holder {
            id: holder_id(terminal),
            socket,
        }),
        _ => Err(Error::Unavailable(
            "The terminal worker sent no socket.".into(),
        )),
    }
}

/// How one attach ended.
enum Attached {
    Exited(Option<u32>),
    /// The tab closed its input: it detaches and the shell keeps running.
    Closed,
    /// The channel ended: the connection was lost, or the holder went away.
    Disconnected,
}

/// A tab whose shell Needle couldn't hold, handed back for an ordinary
/// shell: its display as prepared, its input and its size.
pub(crate) struct Fallback {
    pub shell: Shell,
    pub input: mpsc::Receiver<Input>,
    pub size: (u32, u32),
}

/// Runs the requested holder until exit or detach. A restored tab never
/// starts a replacement. An unconfigured server uses an ordinary shell, and so
/// does a new tab whose holder Needle certainly didn't start, after saying so;
/// any other holder failure remains visible instead of changing shells.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    owner: Arc<ServerSession>,
    server: &Server,
    terminal: Uuid,
    mut size: (u32, u32),
    mut input: mpsc::Receiver<Input>,
    output: &mpsc::Sender<Event>,
    shell: Shell,
    attach: bool,
    colors: Colors,
) -> Result<Option<Fallback>> {
    if !owner.server().same_connection(server) || owner.is_revoked() {
        bail!(STALE_SESSION);
    }
    let mut cancelled = owner.cancellation();
    let link = owner.link().clone();
    let run = async {
        let access = shell.display.clone();
        // As configured now: setup may have finished after this server's
        // session, and the caller's copy, began.
        let profile = owner.profile();
        if !(profile.setup_complete && profile.agent_install_approved) {
            if attach {
                output.send(Event::Ended).await?;
                return Ok(None);
            }
            return Ok(Some(Fallback { shell, input, size }));
        }
        let found = if attach {
            loop {
                match find(&owner, server, terminal).await {
                    Found::Running(holder) => break Some(holder),
                    Found::Gone => {
                        output.send(Event::Ended).await?;
                        return Ok(None);
                    }
                    Found::Failed(error) => return Err(error.into()),
                    Found::Stopped => {
                        output.send(waiting(&link)).await?;
                        tokio::select! {
                            result = link.open(Open::Session) => drop(result?),
                            _ = retry_on_return(&owner, &mut input, &mut size) => return Ok(None),
                        }
                    }
                }
            }
        } else {
            None
        };
        let holder = match found {
            Some(holder) => holder,
            None => match start(&owner, server, terminal, size, access.as_ref()).await {
                Ok(holder) => holder,
                // Needle launched nothing: an ordinary shell can't duplicate a
                // held one, and the tab says it won't keep running.
                Err(Error::Remote(error)) if error.code == service::NOT_STARTED => {
                    output.send(Event::NotHeld(error.message)).await?;
                    return Ok(Some(Fallback { shell, input, size }));
                }
                Err(error) => return Err(error.into()),
            },
        };
        output.send(Event::Persistent).await?;
        let attachment = Attachment {
            holder: &holder,
            colors,
            mac: owner.integration_session_id(),
        };
        // A new tab; a restored or listed one only attaches. Only someone
        // using the tab here shows the display, never its opening by itself.
        let opened = !attach;
        let mut viewing = Viewing::new(access.map(|access| view(&owner, access)));
        follow(
            &owner,
            server,
            &link,
            attachment,
            opened,
            &mut viewing,
            &mut size,
            &mut input,
            output,
        )
        .await?;
        Ok(None)
    };
    tokio::select! {
        biased;
        _ = async { if !*cancelled.borrow() { let _ = cancelled.changed().await; } } => {
            bail!(STALE_SESSION)
        },
        result = run => result,
    }
}

/// What a tab attaches to, the colours it answers queries with, and this
/// Mac's id for the server.
#[derive(Clone, Copy)]
struct Attachment<'a> {
    holder: &'a Holder,
    colors: Colors,
    mac: Uuid,
}

/// Attaches to the holder, and again after each lost connection, until the
/// shell exits, or the tab closes or detaches. Only the first attach of a tab
/// someone `opened` says so: reattaching makes no Mac the active one.
#[allow(clippy::too_many_arguments)]
async fn follow(
    owner: &Arc<ServerSession>,
    server: &Server,
    link: &Link,
    attachment: Attachment<'_>,
    mut opened: bool,
    viewing: &mut Viewing,
    size: &mut (u32, u32),
    input: &mut mpsc::Receiver<Input>,
    output: &mpsc::Sender<Event>,
) -> Result<()> {
    let holder = attachment.holder;
    loop {
        let channel = tokio::select! {
            result = connect(owner, server, link, holder) => result,
            _ = discard_input(input, size) => return Ok(()),
        };
        let channel = match channel {
            Ok(channel) => channel,
            Err(Connect::Link(error)) if error.retry() == Retry::Backoff => continue,
            Err(Connect::Link(error)) if error.retry() == Retry::OnRetry => {
                output.send(reconnecting(link, &error)).await?;
                tokio::select! {
                    result = link.open(Open::Session) => drop(result?),
                    _ = retry_on_return(owner, input, size) => return Ok(()),
                }
                continue;
            }
            Err(Connect::Link(error)) => return Err(error.into()),
            Err(Connect::Runtime(error)) => return Err(error.into()),
        };
        let fate = channel.fate();
        let stream = channel.into_stream();
        let attached =
            match attach_once(stream, attachment, opened, viewing, size, input, output).await {
                Ok(attached) => attached,
                Err(error)
                    if error.downcast_ref::<Error>().is_none()
                        && error.downcast_ref::<GreetingFailure>().is_none()
                        && matches!(
                            error.downcast_ref::<FrameError>(),
                            Some(FrameError::Truncated)
                        )
                        && crate::remote::lost(&fate).await.is_some() =>
                {
                    Attached::Disconnected
                }
                Err(error) => return Err(error),
            };
        opened = false;
        match attached {
            Attached::Exited(exit) => {
                output.send(Event::Exit(exit)).await?;
                return Ok(());
            }
            Attached::Closed => return Ok(()),
            Attached::Disconnected => match crate::remote::lost(&fate).await {
                Some(lost) => output.send(reconnecting(link, &lost)).await?,
                // The connection is fine; the holder closed the channel.
                None => {
                    let alive = list(owner, server).await?.iter().any(|h| h.id == holder.id);
                    if !alive {
                        output.send(Event::Ended).await?;
                        return Ok(());
                    }
                }
            },
        }
    }
}

enum Connect {
    Link(newport_ssh::Error),
    Runtime(Error),
}

/// A forwarded socket, or the selected runtime's verified immutable executable.
/// A refusal never invokes the installed-default alias or any other helper.
async fn connect(
    owner: &Arc<ServerSession>,
    server: &Server,
    link: &Link,
    holder: &Holder,
) -> Result<newport_ssh::Channel, Connect> {
    let path = holder.socket.clone();
    match link
        .open_unless_stopped(Open::DirectStreamLocal { path })
        .await
    {
        Ok(channel) => Ok(channel),
        Err(newport_ssh::Error::ChannelRefused { .. }) => {
            let validate = || {
                if owner.is_revoked() || !owner.server().same_connection(server) {
                    Err(Error::Stale)
                } else {
                    Ok(())
                }
            };
            let runtime = owner
                .agent_runtime
                .selected(link, owner.integration_session_id(), &validate)
                .await
                .map_err(Connect::Runtime)?;
            runtime
                .hello()
                .require(protocol::CAPABILITY, protocol::VERSION)
                .map_err(|error| Connect::Runtime(Error::Remote(error)))?;
            let executable = runtime
                .verified_executable(link)
                .await
                .map_err(Connect::Runtime)?;
            validate().map_err(Connect::Runtime)?;
            let command = attachment_command(&executable, &holder.id).map_err(Connect::Runtime)?;
            let channel = crate::remote::command::stream(link, &command)
                .await
                .map_err(|error| match error {
                    crate::remote::command::Error::Link(error) => Connect::Link(error),
                    error => Connect::Runtime(Error::Unavailable(error.to_string())),
                })?;
            validate().map_err(Connect::Runtime)?;
            if runtime.is_closed() {
                return Err(Connect::Runtime(Error::Interrupted(
                    "Selected runtime disconnected".into(),
                )));
            }
            Ok(channel)
        }
        Err(error) => Err(Connect::Link(error)),
    }
}

fn attachment_command(executable: &str, id: &str) -> Result<String, Error> {
    if !service::valid_id(id) {
        return Err(Error::Unavailable("Invalid terminal identity.".into()));
    }
    Ok(format!("exec {executable} terminal attach {id}"))
}

fn inflate(compressed: &[u8]) -> Result<Vec<u8>> {
    use flate2::{Decompress, FlushDecompress, Status};
    let mut inflater = Decompress::new(false);
    let mut state = Vec::new();
    let mut buffer = [0; OUTPUT_CHUNK];
    loop {
        let before_in = inflater.total_in() as usize;
        let before_out = inflater.total_out();
        let room = buffer.len().min(protocol::MAX_SNAPSHOT + 1 - state.len());
        let status = inflater
            .decompress(
                &compressed[before_in..],
                &mut buffer[..room],
                // Output is deliberately chunked. `Finish` promises enough
                // output space for the whole stream to some inflater backends.
                FlushDecompress::None,
            )
            .context("The terminal's state could not be read.")?;
        let produced = (inflater.total_out() - before_out) as usize;
        state.extend_from_slice(&buffer[..produced]);
        anyhow::ensure!(
            state.len() <= protocol::MAX_SNAPSHOT,
            "The terminal's state is too large."
        );
        if status == Status::StreamEnd {
            anyhow::ensure!(
                inflater.total_in() as usize == compressed.len(),
                "Data follows the terminal's compressed state."
            );
            return Ok(state);
        }
        anyhow::ensure!(
            inflater.total_in() as usize != before_in || produced != 0,
            "The terminal's compressed state is truncated."
        );
    }
}

#[derive(Debug)]
struct GreetingFailure(anyhow::Error);
impl std::fmt::Display for GreetingFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("The terminal did not complete its greeting. Attachment was not retried. If it predates the update, close the tab to end its shell, then open a new one.")
    }
}
impl std::error::Error for GreetingFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}
fn greeting_failure(error: anyhow::Error) -> anyhow::Error {
    let malformed =
        |frame: &FrameError| !matches!(frame, FrameError::Io(_) | FrameError::Truncated);
    let incompatible = error.downcast_ref::<FrameError>().is_some_and(malformed)
        || error
            .downcast_ref::<protocol::wire::Error>()
            .is_some_and(|error| match error {
                protocol::wire::Error::Invalid(_) => true,
                protocol::wire::Error::Frame(frame) => malformed(frame),
                protocol::wire::Error::Ended => false,
            });
    if incompatible {
        error.context(Error::Remote(protocol::reopen_required()))
    } else {
        GreetingFailure(error).into()
    }
}

async fn message<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
) -> Result<ServerMessage> {
    let body = reader
        .next()
        .await?
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::UnexpectedEof))?;
    Ok(protocol::wire::decode_server(body)?)
}

fn dimensions(size: (u32, u32)) -> Result<(u16, u16)> {
    let columns = u16::try_from(size.0)?;
    let rows = u16::try_from(size.1)?;
    anyhow::ensure!(columns != 0 && rows != 0, "Invalid terminal size.");
    Ok((columns, rows))
}

async fn send<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    message: &ClientMessage,
) -> Result<()> {
    let bytes = protocol::wire::encode_client(message)?;
    writer.write_all(&bytes).await?;
    Ok(())
}

/// Identity first, then attachment and an initial screen under one deadline.
/// No queued keystroke or control reaches a peer before all three are valid.
async fn attach_once(
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite,
    attachment: Attachment<'_>,
    opened: bool,
    viewing: &mut Viewing,
    size: &mut (u32, u32),
    input: &mut mpsc::Receiver<Input>,
    output: &mpsc::Sender<Event>,
) -> Result<Attached> {
    tokio::pin!(stream);
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = FrameReader::new(reader, protocol::SERVER_LIMITS);
    let deadline = tokio::time::Instant::now() + ATTACH_TIMEOUT;
    let greeting = tokio::time::timeout_at(deadline, message(&mut reader))
        .await
        .map_err(anyhow::Error::new)
        .and_then(|result| result)
        .map_err(greeting_failure)?;
    match greeting {
        ServerMessage::Hello(hello) => hello
            .require(&attachment.holder.id)
            .context(Error::Remote(protocol::reopen_required()))?,
        ServerMessage::Failed(error) => return Err(Error::Remote(error).into()),
        _ => return Err(Error::Remote(protocol::reopen_required()).into()),
    }
    let (columns, rows) = dimensions(*size)?;
    let attach = ClientMessage::Attach(Attach {
        version: protocol::VERSION,
        columns,
        rows,
        control: false,
        opened,
        computer: Some(Computer {
            id: attachment.mac.to_string(),
            name: COMPUTER.clone(),
        }),
        colors: attachment.colors,
    });
    let (prompted, shell_size, state) = tokio::time::timeout_at(deadline, async {
        send(&mut writer, &attach).await?;
        let prompted = match message(&mut reader).await? {
            ServerMessage::Attached { prompted } => prompted,
            ServerMessage::Failed(error) => return Err(Error::Remote(error).into()),
            _ => bail!("The terminal did not acknowledge its attachment."),
        };
        // The shell's size, which the screen is drawn at, comes first.
        let mut next = message(&mut reader).await?;
        let shell_size = match next {
            ServerMessage::Size { columns, rows } => {
                next = message(&mut reader).await?;
                Some((columns, rows))
            }
            _ => None,
        };
        let state = match next {
            ServerMessage::Screen(state) => state,
            ServerMessage::Failed(error) => return Err(Error::Remote(error).into()),
            _ => bail!("The terminal sent no initial screen."),
        };
        Ok::<_, anyhow::Error>((prompted, shell_size, state))
    })
    .await
    .context("The terminal attachment timed out.")??;
    if let Some((columns, rows)) = shell_size {
        output.send(Event::Size { columns, rows }).await?;
    }
    output
        .send(Event::Restore {
            state: state.to_vec(),
            prompted,
        })
        .await?;
    // Input queued for the previous screen is stale; activation and latest size survive.
    let before = *size;
    let mut activate = false;
    while let Ok(command) = input.try_recv() {
        match command {
            Input::Resize(columns, rows) => *size = (columns, rows),
            Input::Activate => activate = true,
            Input::Data(_) | Input::End => {}
        }
    }
    if *size != before {
        let (columns, rows) = dimensions(*size)?;
        tokio::time::timeout(
            INPUT_TIMEOUT,
            send(&mut writer, &ClientMessage::Resize { columns, rows }),
        )
        .await??;
    }
    if activate {
        tokio::time::timeout(INPUT_TIMEOUT, send(&mut writer, &ClientMessage::Activate)).await??;
        viewing.activated();
    }
    output.send(Event::Ready).await?;
    let mut input_open = true;
    let mut full_pending = true;
    loop {
        tokio::select! {
            read = reader.next() => {
                let body = match read {
                    Ok(Some(body)) => body,
                    Ok(None) | Err(FrameError::Io(_)) => return Ok(Attached::Disconnected),
                    Err(error) => return Err(error.into()),
                };
                match protocol::wire::decode_server(body)? {
                    ServerMessage::Screen(state) => {
                        full_pending = true;
                        output.send(Event::Restore { state: state.to_vec(), prompted }).await?;
                    }
                    ServerMessage::Full(compressed) if full_pending => {
                        full_pending = false;
                        output.send(Event::Restore { state: inflate(&compressed)?, prompted }).await?;
                    }
                    ServerMessage::Output(bytes) => {
                        full_pending = false;
                        for bytes in bytes.chunks(OUTPUT_CHUNK) {
                            output.send(Event::Data(bytes.to_vec())).await?;
                        }
                    }
                    ServerMessage::Exit(exit) => {
                        let (_, trailing) = reader.into_parts();
                        anyhow::ensure!(trailing.is_empty(), "Data follows the terminal exit.");
                        return Ok(Attached::Exited(match exit { service::Exit::Code(code) => Some(code), service::Exit::Signal(_) => None }));
                    }
                    ServerMessage::Active(active) => {
                        full_pending = false;
                        if !active { viewing.inactive(); }
                        output.send(Event::Active(active)).await?;
                    }
                    // Before the output drawn at it, and any screen sent with it.
                    ServerMessage::Size { columns, rows } => {
                        output.send(Event::Size { columns, rows }).await?;
                    }
                    ServerMessage::Failed(error) => return Err(Error::Remote(error).into()),
                    _ => bail!("The terminal sent an unexpected frame."),
                }
            }
            () = viewing.lost() => output.send(Event::DisplayLost).await?,
            command = input.recv(), if input_open => {
                let command = match command {
                    Some(Input::Data(bytes)) => {
                        for bytes in bytes.chunks(protocol::MAX_DATA) {
                            tokio::time::timeout(INPUT_TIMEOUT, send(&mut writer, &ClientMessage::Input(bytes.to_vec().into()))).await??;
                        }
                        continue;
                    }
                    Some(Input::Resize(columns, rows)) => {
                        *size = (columns, rows);
                        let (columns, rows) = dimensions(*size)?;
                        ClientMessage::Resize { columns, rows }
                    }
                    Some(Input::End) => { input_open = false; ClientMessage::End }
                    Some(Input::Activate) => { viewing.activated(); ClientMessage::Activate }
                    None => return Ok(Attached::Closed),
                };
                // Any failed/partial write ends this attach; bytes are never replayed.
                tokio::time::timeout(INPUT_TIMEOUT, send(&mut writer, &command)).await??;
            }
        }
    }
}

/// Renames a terminal's shell on its server, for every Mac's tab.
pub(crate) async fn rename(
    owner: &Arc<ServerSession>,
    terminal: Uuid,
    name: &str,
) -> Result<bool, Error> {
    let server = owner.server().clone();
    let body = Request::Rename {
        id: holder_id(terminal),
        name: name.into(),
    };
    renamed(request(owner, &server, body).await?)
}

fn renamed(reply: Reply) -> Result<bool, Error> {
    match reply {
        Reply::Renamed { renamed } => Ok(renamed),
        _ => Err(Error::Unavailable(
            "The terminal worker sent no rename outcome.".into(),
        )),
    }
}

/// Ends a terminal's shell on its server, as Close does from a tab that
/// isn't attached to it. Only when its holder can't greet Needle and runs
/// another build, a protocol mismatch, is the holder asked to end its shell
/// with SIGTERM instead.
pub(crate) async fn end(owner: &Arc<ServerSession>, terminal: Uuid) -> Result<bool, Error> {
    let server = owner.server().clone();
    let id = holder_id(terminal);
    let error = match request(owner, &server, Request::End { id: id.clone() })
        .await
        .and_then(ended)
    {
        Ok(ended) => return Ok(ended),
        Err(error) => error,
    };
    if !greeting_failed(&error) {
        return Err(error);
    }
    // The runtime this connection uses names this desktop's build; no
    // worker needs to run for it.
    let Some(own) = owner
        .agent_runtime
        .snapshot()
        .hello
        .map(|hello| hello.build_sha256)
    else {
        return Err(error);
    };
    match crate::remote::command::stdout(owner.link(), &end_other_build(&id, &own), None).await {
        Ok(output) if output.trim() == "ended" => Ok(true),
        Ok(_) => Err(error),
        Err(failure) => {
            log::warn!("Cannot end a holder from another build: {failure}");
            Err(error)
        }
    }
}

/// Whether Needle couldn't end the shell because its holder didn't greet it.
fn greeting_failed(error: &Error) -> bool {
    matches!(error, Error::Remote(remote)
        if remote.code == newport_protocol::terminal::holder::REOPEN_REQUIRED
            || remote.code == newport_protocol::terminal::holder::GREETING_INCOMPLETE)
}

/// Sends SIGTERM to the holder of terminal `id` when it runs a build other
/// than `own`, and waits for it to end its shell and exit: prints `ended`.
/// Prints `same` for a holder of this build, which only Needle may end, and
/// `gone` when no holder runs.
fn end_other_build(id: &str, own: &str) -> String {
    debug_assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    debug_assert!(own.bytes().all(|b| b.is_ascii_hexdigit()));
    // The pattern is assembled at run time, so this script's own command line,
    // which holds its pieces, never matches it.
    format!(
        r#"role=hol; role=${{role}}der; holder=
for p in /proc/[0-9]*; do
  test "${{p#/proc/}}" = "$$" && continue
  cmd=$(tr '\0' ' ' < "$p/cmdline" 2>/dev/null) || continue
  case "$cmd" in *"/newport-agent terminal $role {id} "*) holder=${{p#/proc/}}; break;; esac
done
if test -z "$holder"; then echo gone; exit 0; fi
case "$cmd" in *"/needle/builds/{own}/newport-agent terminal $role "*) echo same; exit 0;; esac
kill -TERM "$holder" 2>/dev/null || {{ echo gone; exit 0; }}
alive() {{ test -d "/proc/$holder" && ! grep -q '^State:.*Z' "/proc/$holder/status" 2>/dev/null; }}
i=0; while alive && test $i -lt 50; do sleep 0.1; i=$((i+1)); done
if alive; then echo running; else echo ended; fi"#
    )
}

fn ended(reply: Reply) -> Result<bool, Error> {
    match reply {
        Reply::Ended { ended } => Ok(ended),
        _ => Err(Error::Unavailable(
            "The terminal worker sent no end outcome.".into(),
        )),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn only_a_greeting_failure_lets_close_end_a_holder_itself() {
        let remote = |code: &str| {
            Error::Remote(newport_protocol::common::Error {
                code: code.into(),
                message: "failed".into(),
                retry: newport_protocol::common::Retry::No,
            })
        };
        assert!(greeting_failed(&remote(
            newport_protocol::terminal::holder::REOPEN_REQUIRED
        )));
        assert!(greeting_failed(&remote(
            newport_protocol::terminal::holder::GREETING_INCOMPLETE
        )));
        for other in ["holder_outcome_unknown", "refused", "busy"] {
            assert!(!greeting_failed(&remote(other)), "{other}");
        }
        assert!(!greeting_failed(&Error::Interrupted("timed out".into())));
        assert!(!greeting_failed(&Error::Stale));
    }

    /// Runs `end_other_build` against holders started from fake builds in a
    /// scratch tree: only the holder of that terminal on another build ends.
    #[cfg(target_os = "linux")]
    #[test]
    fn closing_ends_only_a_holder_of_that_terminal_on_another_build() {
        let root = tempfile::tempdir().unwrap();
        let holder = |build: &str, id: &str| {
            let directory = root.path().join(format!("needle/builds/{build}"));
            std::fs::create_dir_all(&directory).unwrap();
            let agent = directory.join("newport-agent");
            std::fs::write(&agent, "#!/bin/sh\nsleep 30\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::process::Command::new(&agent)
                .args(["terminal", "holder", id, "80", "24"])
                .spawn()
                .unwrap()
        };
        let (old, own) = ("a".repeat(64), "b".repeat(64));
        let (id, other) = ("1".repeat(32), "2".repeat(32));
        let run = |id: &str| {
            String::from_utf8(
                std::process::Command::new("sh")
                    .args(["-c", &end_other_build(id, &own)])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
        };
        let mut stale = holder(&old, &id);
        let mut current = holder(&own, &other);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(run(&other).trim(), "same");
        assert_eq!(run(&id).trim(), "ended");
        assert!(stale.try_wait().unwrap().is_some());
        assert!(current.try_wait().unwrap().is_none());
        assert_eq!(run(&"3".repeat(32)).trim(), "gone");
        current.kill().unwrap();
        let _ = current.wait();
    }
    use crate::terminal::display::FakeView;
    use std::sync::atomic::Ordering;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn worker_results_are_typed_and_an_uncertain_mutation_is_never_replayed() {
        for result in [
            Some(Reply::Ended { ended: false }),
            Some(Reply::Renamed { renamed: true }),
            Some(Reply::Failed(newport_protocol::common::Error {
                code: "refused".into(),
                message: "Holder refused".into(),
                retry: newport_protocol::common::Retry::No,
            })),
            Some(Reply::Failed(newport_protocol::common::Error {
                code: "holder_outcome_unknown".into(),
                message: "The holder closed before confirming the terminal operation. Its outcome is unknown; it was not retried.".into(),
                retry: newport_protocol::common::Retry::Unknown,
            })),
            None,
        ] {
            let request = Request::End {
                id: Uuid::new_v4().simple().to_string(),
            };
            let expected = request.clone();
            let (mut client, mut peer) = tokio::io::duplex(4096);
            let success = matches!(result, Some(Reply::Ended { .. }));
            let expected_error = match &result { Some(Reply::Failed(error)) => Some(error.clone()), _ => None };
            let server = tokio::spawn(async move {
                let mut reader = FrameReader::new(&mut peer, service::CLIENT_LIMITS);
                let body = reader.next().await.unwrap().unwrap();
                assert_eq!(wire::decode_request(body).unwrap(), expected);
                let (_, buffered) = reader.into_parts();
                assert!(buffered.is_empty());
                if let Some(reply) = result {
                    for byte in wire::encode_reply(&reply).unwrap() {
                        peer.write_all(&[byte]).await.unwrap();
                    }
                }
                peer.shutdown().await.unwrap();
                let mut replayed = Vec::new();
                peer.read_to_end(&mut replayed).await.unwrap();
                assert!(replayed.is_empty());
            });
            let result = exchange(&mut client, &request).await;
            assert_eq!(result.is_ok(), success);
            if let Some(expected) = expected_error {
                assert!(matches!(&result, Err(Error::Remote(remote)) if remote == &expected));
            } else if let Err(error) = &result {
                assert_unknown(error);
            }
            drop(client);
            server.await.unwrap();
        }
    }

    fn assert_unknown(error: &Error) {
        let Error::Remote(remote) = error else {
            panic!("expected uncertain mutation: {error}")
        };
        assert_eq!(remote.code, "holder_outcome_unknown");
        assert_eq!(remote.retry, newport_protocol::common::Retry::Unknown);
        assert!(!error.transient());
    }

    enum ReadStep {
        Bytes(Vec<u8>),
        Eof,
        Error(std::io::ErrorKind),
        Pending,
    }
    struct ExchangeStream {
        reads: std::collections::VecDeque<ReadStep>,
        written: Vec<u8>,
        write_limit: Option<usize>,
        fail_flush: bool,
    }
    impl ExchangeStream {
        fn new(reads: impl IntoIterator<Item = ReadStep>) -> Self {
            Self {
                reads: reads.into_iter().collect(),
                written: Vec::new(),
                write_limit: None,
                fail_flush: false,
            }
        }
    }
    impl tokio::io::AsyncRead for ExchangeStream {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            output: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            use std::task::Poll;
            match self
                .reads
                .pop_front()
                .expect("unexpected extra transport read")
            {
                ReadStep::Bytes(bytes) => {
                    let count = bytes.len().min(output.remaining());
                    output.put_slice(&bytes[..count]);
                    if count < bytes.len() {
                        self.reads
                            .push_front(ReadStep::Bytes(bytes[count..].to_vec()));
                    }
                    Poll::Ready(Ok(()))
                }
                ReadStep::Eof => Poll::Ready(Ok(())),
                ReadStep::Error(kind) => {
                    Poll::Ready(Err(std::io::Error::new(kind, "fixture transport failure")))
                }
                ReadStep::Pending => {
                    self.reads.push_front(ReadStep::Pending);
                    Poll::Pending
                }
            }
        }
    }
    impl tokio::io::AsyncWrite for ExchangeStream {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            let count = match self.write_limit {
                Some(limit) if self.written.len() >= limit => {
                    return std::task::Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "fixture partial write",
                    )));
                }
                Some(limit) => bytes.len().min(limit - self.written.len()),
                None => bytes.len(),
            };
            self.written.extend_from_slice(&bytes[..count]);
            std::task::Poll::Ready(Ok(count))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(if self.fail_flush {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "fixture flush failure",
                ))
            } else {
                Ok(())
            })
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn holder_management_keeps_false_and_true_outcomes_after_clean_eof() {
        let id = holder_id(Uuid::new_v4());
        for value in [false, true] {
            for (request, reply) in [
                (
                    Request::Rename {
                        id: id.clone(),
                        name: "build".into(),
                    },
                    Reply::Renamed { renamed: value },
                ),
                (
                    Request::End { id: id.clone() },
                    Reply::Ended { ended: value },
                ),
            ] {
                let mut stream = ExchangeStream::new([
                    ReadStep::Bytes(wire::encode_reply(&reply).unwrap()),
                    ReadStep::Eof,
                ]);
                let result = exchange_with_timeout(&mut stream, &request, Duration::from_secs(1))
                    .await
                    .unwrap();
                assert_eq!(result, reply);
                assert_eq!(
                    match request {
                        Request::Rename { .. } => renamed(result),
                        _ => ended(result),
                    }
                    .unwrap(),
                    value
                );
                assert_eq!(stream.written, wire::encode_request(&request).unwrap());
            }
        }
        assert!(renamed(Reply::Ended { ended: true }).is_err());
        assert!(ended(Reply::Renamed { renamed: true }).is_err());
    }

    #[tokio::test]
    async fn holder_response_requires_actual_eof_without_buffered_or_late_trailing_bytes() {
        let request = Request::End {
            id: holder_id(Uuid::new_v4()),
        };
        let reply = wire::encode_reply(&Reply::Ended { ended: true }).unwrap();
        let mut coalesced = reply.clone();
        coalesced.extend_from_slice(&reply);
        let mut partial = reply.clone();
        partial.push(0x80);
        for reads in [
            vec![ReadStep::Bytes(coalesced)],
            vec![
                ReadStep::Bytes(reply.clone()),
                ReadStep::Bytes(reply.clone()),
            ],
            vec![ReadStep::Bytes(partial)],
            vec![ReadStep::Bytes(reply.clone()), ReadStep::Bytes(vec![0x80])],
            vec![
                ReadStep::Bytes(reply[..reply.len() - 1].to_vec()),
                ReadStep::Eof,
            ],
            vec![
                ReadStep::Bytes(reply),
                ReadStep::Error(std::io::ErrorKind::UnexpectedEof),
            ],
        ] {
            let mut stream = ExchangeStream::new(reads);
            let error = exchange_with_timeout(&mut stream, &request, Duration::from_secs(1))
                .await
                .unwrap_err();
            assert_unknown(&error);
            assert_eq!(stream.written, wire::encode_request(&request).unwrap());
        }
    }

    #[tokio::test]
    async fn missing_holder_closure_is_bounded_and_does_not_authorize_mutation_replay() {
        for (request, reply) in [
            (
                Request::End {
                    id: holder_id(Uuid::new_v4()),
                },
                Reply::Ended { ended: false },
            ),
            (Request::List, Reply::Listed(Vec::new())),
        ] {
            let mut stream = ExchangeStream::new([
                ReadStep::Bytes(wire::encode_reply(&reply).unwrap()),
                ReadStep::Pending,
            ]);
            let error = exchange_with_timeout(&mut stream, &request, Duration::from_millis(20))
                .await
                .unwrap_err();
            match request {
                Request::End { .. } => assert_unknown(&error),
                Request::List => assert!(matches!(error, Error::Interrupted(_))),
                _ => unreachable!(),
            }
            assert_eq!(stream.written, wire::encode_request(&request).unwrap());
        }
    }

    #[tokio::test]
    async fn dispatched_mutation_write_flush_and_decode_failures_are_uncertain() {
        for request in [
            start_request(Uuid::new_v4(), (80, 24), Uuid::new_v4(), false).unwrap(),
            Request::Rename {
                id: holder_id(Uuid::new_v4()),
                name: "build".into(),
            },
            Request::End {
                id: holder_id(Uuid::new_v4()),
            },
        ] {
            for failure in 0..4 {
                let mut stream = ExchangeStream::new(match failure {
                    2 => vec![ReadStep::Error(std::io::ErrorKind::ConnectionReset)],
                    3 => vec![ReadStep::Bytes(vec![1, 0xff])],
                    _ => Vec::new(),
                });
                stream.write_limit = (failure == 0).then_some(1);
                stream.fail_flush = failure == 1;
                let error = exchange_with_timeout(&mut stream, &request, Duration::from_secs(1))
                    .await
                    .unwrap_err();
                assert_unknown(&error);
                let encoded = wire::encode_request(&request).unwrap();
                assert_eq!(
                    stream.written,
                    if failure == 0 {
                        encoded[..1].to_vec()
                    } else {
                        encoded
                    }
                );
            }
        }
        let mut stream = ExchangeStream::new([]);
        let result = exchange_with_timeout(
            &mut stream,
            &Request::End {
                id: "invalid".into(),
            },
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(result, Err(Error::Unavailable(_))));
        assert!(stream.written.is_empty());
    }

    fn hello(remote_apps: &[u32], terminal: &[u32]) -> Hello {
        use newport_protocol::runtime::{self as runtime, Capability, Limits};
        let capability = |versions: &[u32]| Capability {
            contract_id: None,
            versions: versions.to_vec(),

            unavailable: None,
        };
        Hello {
            product: newport_protocol::product::Variant::Full,
            versions: vec![runtime::VERSION],
            instance_id: Uuid::new_v4().to_string(),
            build_sha256: "0".repeat(64),
            agent_version: None,
            worker_socket: "/tmp/unused.sock".into(),
            services: [
                ("remote-apps".to_owned(), capability(remote_apps)),
                ("terminal".to_owned(), capability(terminal)),
            ]
            .into(),
            dependencies: Default::default(),
            limits: Limits {
                max_control_bytes: runtime::MAX_FRAME,
                max_inflight: 1,
                max_workers: runtime::MAX_WORKERS,
                max_registrations: runtime::MAX_REGISTRATIONS,
            },
        }
    }

    #[test]
    fn holder_mutations_require_the_selected_runtime_capability() {
        use newport_protocol::{common::Retry, runtime::Capability};
        let mut runtime = hello(&[1], &[1]);
        let id = holder_id(Uuid::new_v4());
        let start = start_request(Uuid::new_v4(), (80, 24), Uuid::new_v4(), false).unwrap();
        for request in [
            start,
            Request::Rename {
                id: id.clone(),
                name: "build".into(),
            },
            Request::End { id },
        ] {
            assert!(
                matches!(require_holder(&runtime, &request), Err(Error::Remote(error)) if error.retry == Retry::No)
            );
            for capability in [
                Capability {
                    contract_id: None,
                    versions: vec![2],
                    unavailable: None,
                },
                Capability {
                    contract_id: None,
                    versions: vec![1],
                    unavailable: Some("unavailable".into()),
                },
            ] {
                runtime
                    .services
                    .insert(protocol::CAPABILITY.into(), capability);
                assert!(
                    matches!(require_holder(&runtime, &request), Err(Error::Remote(error)) if error.retry == Retry::No)
                );
            }
            runtime.services.insert(
                protocol::CAPABILITY.into(),
                Capability {
                    contract_id: None,
                    versions: vec![1],
                    unavailable: None,
                },
            );
            assert!(require_holder(&runtime, &request).is_ok());
            runtime.services.remove(protocol::CAPABILITY);
        }
        assert!(
            require_holder(&runtime, &Request::List).is_ok(),
            "old rows remain inspectable"
        );
        assert!(require_holder(&runtime, &Request::Watch).is_ok());
    }

    #[test]
    fn only_a_needle_that_shows_the_stable_display_is_told_to_expect_it() {
        assert!(shows_stable_display(&hello(&[1], &[1])));
        assert!(!shows_stable_display(&hello(&[], &[1])));
        let terminal = Uuid::new_v4();
        let creator = Uuid::new_v4();
        let expected = start_request(terminal, (80, 24), creator, true).unwrap();
        assert!(
            matches!(expected, Request::Start { stable_display: true, creator: computer, .. } if computer.id == creator.to_string())
        );
        let plain = start_request(terminal, (80, 24), creator, false).unwrap();
        assert!(matches!(
            plain,
            Request::Start {
                stable_display: false,
                ..
            }
        ));
        assert!(start_request(terminal, (u32::MAX, 24), creator, false).is_err());
    }

    async fn next(output: &mut mpsc::Receiver<Event>) -> Event {
        tokio::time::timeout(Duration::from_secs(5), output.recv())
            .await
            .expect("the tab gets an event")
            .expect("the tab's output is open")
    }

    async fn sent(server: &mut FrameReader<tokio::io::DuplexStream>, expected: ClientMessage) {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), server.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let message = protocol::wire::decode_client(frame).unwrap();
            if matches!(message, ClientMessage::Attach(_)) {
                continue;
            }
            assert_eq!(message, expected);
            return;
        }
    }

    async fn write_server(
        server: &mut FrameReader<tokio::io::DuplexStream>,
        message: ServerMessage,
    ) {
        for fragment in protocol::wire::encode_server(&message).unwrap().chunks(3) {
            server.get_mut().write_all(fragment).await.unwrap();
            tokio::task::yield_now().await;
        }
    }

    async fn greet(server: &mut FrameReader<tokio::io::DuplexStream>, id: &str) {
        write_server(
            server,
            ServerMessage::Hello(protocol::Hello {
                version: protocol::VERSION,
                terminal_id: id.into(),
            }),
        )
        .await;
        let frame = tokio::time::timeout(Duration::from_secs(5), server.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(
            protocol::wire::decode_client(frame).unwrap(),
            ClientMessage::Attach(_)
        ));
        write_server(server, ServerMessage::Attached { prompted: true }).await;
        write_server(server, ServerMessage::Screen(Vec::new().into())).await;
    }

    #[tokio::test]
    async fn using_the_shell_here_shows_the_display_until_another_computer_is_active() {
        let fake = FakeView::new();
        let mut viewing = Viewing::new(Some(fake.view()));
        let (client, server) = tokio::io::duplex(1 << 16);
        let (input_tx, mut input) = mpsc::channel(8);
        let (output, mut events) = mpsc::channel(8);
        let holder = Holder {
            id: holder_id(Uuid::new_v4()),
            socket: "/unused".into(),
        };
        let attachment = Attachment {
            holder: &holder,
            colors: None,
            mac: Uuid::new_v4(),
        };
        let mut size = (80, 24);
        let attach = attach_once(
            client,
            attachment,
            false,
            &mut viewing,
            &mut size,
            &mut input,
            &output,
        );
        let holder_side = async {
            let mut server = FrameReader::new(server, protocol::CLIENT_LIMITS);
            greet(&mut server, &holder.id).await;
            assert!(matches!(next(&mut events).await, Event::Restore { .. }));
            assert!(matches!(next(&mut events).await, Event::Ready));

            // A reattach is told this computer is active: that alone never
            // takes the display, as after a wake.
            write_server(&mut server, ServerMessage::Active(true)).await;
            assert!(matches!(next(&mut events).await, Event::Active(true)));
            fake.stays((0, 0)).await;

            // Using the shell here shows the display, and holds it; using it
            // again while it does asks nothing more.
            for _ in 0..3 {
                input_tx.send(Input::Activate).await.unwrap();
                sent(&mut server, ClientMessage::Activate).await;
            }
            fake.until((1, 1)).await;
            fake.stays((1, 1)).await;
            // Another computer became active: it lets the display go.
            write_server(&mut server, ServerMessage::Active(false)).await;
            assert!(matches!(next(&mut events).await, Event::Active(false)));
            fake.until((1, 0)).await;

            // A computer that may not show the display isn't told anything:
            // no keystroke would change that.
            fake.refused.store(true, Ordering::Release);
            input_tx.send(Input::Activate).await.unwrap();
            sent(&mut server, ClientMessage::Activate).await;
            fake.until((2, 0)).await;
            write_server(
                &mut server,
                ServerMessage::Output(b"still here".to_vec().into()),
            )
            .await;
            assert!(matches!(next(&mut events).await, Event::Data(data) if data == b"still here"));
            fake.refused.store(false, Ordering::Release);

            // A display that fails to show tells the tab, so its next use
            // asks again; the shell carries on meanwhile.
            fake.failing.store(true, Ordering::Release);
            input_tx.send(Input::Activate).await.unwrap();
            sent(&mut server, ClientMessage::Activate).await;
            fake.until((3, 0)).await;
            assert!(matches!(next(&mut events).await, Event::DisplayLost));
            input_tx.send(Input::Data(b"ls\n".to_vec())).await.unwrap();
            sent(&mut server, ClientMessage::Input(b"ls\n".to_vec().into())).await;
            fake.failing.store(false, Ordering::Release);
            fake.stays((3, 0)).await;
            input_tx.send(Input::Activate).await.unwrap();
            sent(&mut server, ClientMessage::Activate).await;
            fake.until((4, 1)).await;

            // Taken over by another computer while still active here: the tab
            // hears so, and a reattach doesn't take it back; using the shell
            // here does, and using it again while it shows asks nothing more.
            fake.taken.notify_one();
            assert!(matches!(next(&mut events).await, Event::DisplayLost));
            fake.until((4, 0)).await;
            write_server(&mut server, ServerMessage::Active(true)).await;
            assert!(matches!(next(&mut events).await, Event::Active(true)));
            fake.stays((4, 0)).await;
            input_tx.send(Input::Activate).await.unwrap();
            sent(&mut server, ClientMessage::Activate).await;
            fake.until((5, 1)).await;
            input_tx.send(Input::Activate).await.unwrap();
            sent(&mut server, ClientMessage::Activate).await;
            fake.stays((5, 1)).await;
            write_server(&mut server, ServerMessage::Output(b"shown".to_vec().into())).await;
            assert!(matches!(next(&mut events).await, Event::Data(data) if data == b"shown"));

            write_server(&mut server, ServerMessage::Exit(service::Exit::Code(0))).await;
        };
        let (attached, ()) = tokio::join!(attach, holder_side);
        assert!(matches!(attached.unwrap(), Attached::Exited(Some(0))));
        // The shell ended: the display goes with the tab's attachment.
        drop(viewing);
        fake.until((5, 0)).await;
    }

    #[tokio::test]
    async fn only_a_keystroke_shows_the_display_never_opening_or_reattaching() {
        for (opened, typed) in [(true, false), (false, false), (true, true), (false, true)] {
            let fake = FakeView::new();
            let mut viewing = Viewing::new(Some(fake.view()));
            let (client, server) = tokio::io::duplex(1 << 16);
            let (input_tx, mut input) = mpsc::channel(8);
            let (output, mut events) = mpsc::channel(8);
            let holder = Holder {
                id: holder_id(Uuid::new_v4()),
                socket: "/unused".into(),
            };
            let attachment = Attachment {
                holder: &holder,
                colors: None,
                mac: Uuid::new_v4(),
            };
            // Typed while the tab still attached, as into a new tab.
            if typed {
                input_tx.send(Input::Activate).await.unwrap();
            }
            let mut size = (80, 24);
            let attach = attach_once(
                client,
                attachment,
                opened,
                &mut viewing,
                &mut size,
                &mut input,
                &output,
            );
            let holder_side = async {
                let mut server = FrameReader::new(server, protocol::CLIENT_LIMITS);
                greet(&mut server, &holder.id).await;
                assert!(matches!(next(&mut events).await, Event::Restore { .. }));
                assert!(matches!(next(&mut events).await, Event::Ready));
                if typed {
                    fake.until((1, 1)).await;
                } else {
                    fake.stays((0, 0)).await;
                }
                write_server(&mut server, ServerMessage::Exit(service::Exit::Code(0))).await;
            };
            let (attached, ()) = tokio::join!(attach, holder_side);
            assert!(matches!(attached.unwrap(), Attached::Exited(Some(0))));
            // A closed tab holds no display.
            drop(viewing);
            fake.until((usize::from(typed), 0)).await;
        }
    }

    #[tokio::test]
    async fn using_a_terminal_whose_display_shows_makes_that_display_take_copies() {
        let fake = FakeView::new();
        let mut viewing = Viewing::new(Some(fake.view()));
        viewing.activated();
        fake.until((1, 1)).await;
        assert_eq!(fake.stood.load(Ordering::Acquire), 0);
        // Used again, as after another server's tab: the display it shows
        // takes copies, and nothing asks to show it again.
        viewing.activated();
        fake.stays((1, 1)).await;
        assert_eq!(fake.stood.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn a_shell_without_display_access_never_shows_the_display() {
        let mut viewing = Viewing::new(None);
        viewing.activated();
        assert!(!viewing.asking());
    }

    #[tokio::test]
    async fn old_wrong_and_failed_peers_receive_no_attach_or_queued_input() {
        let holder = Holder {
            id: holder_id(Uuid::new_v4()),
            socket: "/unused".into(),
        };
        let wrong = protocol::wire::encode_server(&ServerMessage::Hello(protocol::Hello {
            version: protocol::VERSION,
            terminal_id: holder_id(Uuid::new_v4()),
        }))
        .unwrap();
        let failed =
            protocol::wire::encode_server(&ServerMessage::Failed(protocol::reopen_required()))
                .unwrap();
        for bytes in [vec![1, 0, 0, 0, 5, 0, 0, 0, 1, 0], wrong, failed, vec![]] {
            let (client, mut peer) = tokio::io::duplex(4096);
            let (input_tx, mut input) = mpsc::channel(8);
            input_tx
                .send(Input::Data(b"must stay local".to_vec()))
                .await
                .unwrap();
            input_tx.send(Input::End).await.unwrap();
            let (output, mut events) = mpsc::channel(8);
            let mut view = Viewing::new(None);
            let mut size = (80, 24);
            let attachment = Attachment {
                holder: &holder,
                colors: None,
                mac: Uuid::new_v4(),
            };
            let remote = async {
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), peer.read_u8())
                        .await
                        .is_err(),
                    "no bytes before greeting"
                );
                peer.write_all(&bytes).await.unwrap();
                peer.shutdown().await.unwrap();
                let mut sent = Vec::new();
                peer.read_to_end(&mut sent).await.unwrap();
                assert!(sent.is_empty());
            };
            let (result, ()) = tokio::join!(
                attach_once(
                    client, attachment, false, &mut view, &mut size, &mut input, &output
                ),
                remote
            );
            let error = result.err().expect("old peer is refused, never Gone");
            if bytes.is_empty() {
                assert!(error.downcast_ref::<GreetingFailure>().is_some());
                assert!(error.chain().any(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|cause| cause.kind() == std::io::ErrorKind::UnexpectedEof)
                }));
                assert!(!error.to_string().contains("incompatible protocol"));
            } else {
                assert!(
                    matches!(error.downcast_ref::<Error>(), Some(Error::Remote(remote)) if remote.code == protocol::REOPEN_REQUIRED && remote.retry == newport_protocol::common::Retry::No)
                );
            }
            assert!(events.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn attachment_and_screen_are_required_before_input_or_ready() {
        for reply in [
            ServerMessage::Output(b"too early".to_vec().into()),
            ServerMessage::Screen(Vec::new().into()),
            ServerMessage::Renamed("wrong role".into()),
        ] {
            let holder = Holder {
                id: holder_id(Uuid::new_v4()),
                socket: "/unused".into(),
            };
            let (client, peer) = tokio::io::duplex(4096);
            let mut peer = FrameReader::new(peer, protocol::CLIENT_LIMITS);
            let (input_tx, mut input) = mpsc::channel(8);
            input_tx
                .send(Input::Data(b"withheld".to_vec()))
                .await
                .unwrap();
            let (output, mut events) = mpsc::channel(8);
            let mut view = Viewing::new(None);
            let mut size = (80, 24);
            let remote = async {
                write_server(
                    &mut peer,
                    ServerMessage::Hello(protocol::Hello {
                        version: 1,
                        terminal_id: holder.id.clone(),
                    }),
                )
                .await;
                let frame = peer.next().await.unwrap().unwrap();
                assert!(matches!(
                    protocol::wire::decode_client(frame).unwrap(),
                    ClientMessage::Attach(_)
                ));
                write_server(&mut peer, reply).await;
                assert!(
                    peer.next().await.unwrap().is_none(),
                    "no input follows invalid acknowledgement"
                );
            };
            let attachment = Attachment {
                holder: &holder,
                colors: None,
                mac: Uuid::new_v4(),
            };
            let (result, ()) = tokio::join!(
                attach_once(
                    client, attachment, false, &mut view, &mut size, &mut input, &output
                ),
                remote
            );
            assert!(result.is_err());
            assert!(events.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn a_control_acknowledgement_on_a_viewer_stream_stops_before_later_output() {
        let holder = Holder {
            id: holder_id(Uuid::new_v4()),
            socket: "/unused".into(),
        };
        let (client, peer) = tokio::io::duplex(4096);
        let mut peer = FrameReader::new(peer, protocol::CLIENT_LIMITS);
        let (_input_tx, mut input) = mpsc::channel(8);
        let (output, mut events) = mpsc::channel(8);
        let mut view = Viewing::new(None);
        let mut size = (80, 24);
        let remote = async {
            greet(&mut peer, &holder.id).await;
            assert!(matches!(next(&mut events).await, Event::Restore { .. }));
            assert!(matches!(next(&mut events).await, Event::Ready));
            let wrong_role =
                protocol::wire::encode_server(&ServerMessage::Renamed("persisted name".into()))
                    .unwrap();
            let later = protocol::wire::encode_server(&ServerMessage::Output(
                b"must not appear".to_vec().into(),
            ))
            .unwrap();
            peer.get_mut()
                .write_all(&[wrong_role, later].concat())
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_secs(3), peer.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .is_none()
            );
            assert!(events.try_recv().is_err());
        };
        let attachment = Attachment {
            holder: &holder,
            colors: None,
            mac: Uuid::new_v4(),
        };
        let (result, ()) = tokio::join!(
            attach_once(
                client, attachment, false, &mut view, &mut size, &mut input, &output
            ),
            remote
        );
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("unexpected frame")
        );
    }

    #[tokio::test]
    async fn later_screen_and_full_pairs_restore_resized_holders_without_a_new_ready() {
        use std::io::Write;
        let holder = Holder {
            id: holder_id(Uuid::new_v4()),
            socket: "/unused".into(),
        };
        let (client, peer) = tokio::io::duplex(4096);
        let mut peer = FrameReader::new(peer, protocol::CLIENT_LIMITS);
        let (_input_tx, mut input) = mpsc::channel(8);
        let (output, mut events) = mpsc::channel(8);
        let mut view = Viewing::new(None);
        let mut size = (80, 24);
        let remote = async {
            greet(&mut peer, &holder.id).await;
            assert!(matches!(next(&mut events).await, Event::Restore { .. }));
            assert!(matches!(next(&mut events).await, Event::Ready));
            for state in [b"resized one".as_slice(), b"resized twice".as_slice()] {
                let mut compressed =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
                compressed.write_all(state).unwrap();
                write_server(&mut peer, ServerMessage::Screen(state.to_vec().into())).await;
                write_server(
                    &mut peer,
                    ServerMessage::Full(compressed.finish().unwrap().into()),
                )
                .await;
                for _ in 0..2 {
                    assert!(
                        matches!(next(&mut events).await, Event::Restore { state: actual, .. } if actual == state)
                    );
                }
            }
            write_server(&mut peer, ServerMessage::Exit(service::Exit::Code(0))).await;
        };
        let attachment = Attachment {
            holder: &holder,
            colors: None,
            mac: Uuid::new_v4(),
        };
        let (result, ()) = tokio::join!(
            attach_once(
                client, attachment, false, &mut view, &mut size, &mut input, &output
            ),
            remote
        );
        assert!(matches!(result.unwrap(), Attached::Exited(Some(0))));
    }

    #[test]
    fn compressed_state_must_end_exactly_within_its_expanded_limit() {
        use std::io::Write;
        let compress = |bytes: &[u8]| {
            let mut writer =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
            writer.write_all(bytes).unwrap();
            writer.finish().unwrap()
        };
        let compressed = compress(b"screen");
        assert_eq!(inflate(&compressed).unwrap(), b"screen");
        for length in 0..compressed.len() {
            assert!(inflate(&compressed[..length]).is_err());
        }
        assert!(inflate(&[compressed.as_slice(), &[0]].concat()).is_err());
        assert!(inflate(&compress(&vec![0; protocol::MAX_SNAPSHOT + 1])).is_err());
        assert_eq!(
            inflate(&compress(&vec![0; protocol::MAX_SNAPSHOT]))
                .unwrap()
                .len(),
            protocol::MAX_SNAPSHOT
        );
    }

    #[test]
    fn maximum_restored_state_accepts_bounded_deflate_overhead() {
        let state = vec![42; protocol::MAX_SNAPSHOT];
        let mut compressed = Vec::with_capacity(state.len() + 2048);
        // Stored DEFLATE blocks make the valid encoded size deterministically
        // larger than the restored bytes, independently of compressor tuning.
        let chunks = state.chunks(u16::MAX as usize);
        let last = chunks.len() - 1;
        for (index, chunk) in chunks.enumerate() {
            compressed.push(u8::from(index == last));
            let length = u16::try_from(chunk.len()).unwrap();
            compressed.extend(length.to_le_bytes());
            compressed.extend((!length).to_le_bytes());
            compressed.extend_from_slice(chunk);
        }
        assert!(compressed.len() > protocol::MAX_SNAPSHOT);
        assert!(compressed.len() <= protocol::MAX_COMPRESSED_SNAPSHOT);
        let encoded =
            protocol::wire::encode_server(&ServerMessage::Full(compressed.into())).unwrap();
        let body = newport_protocol::encoding::protobuf::read_frame(
            &mut encoded.as_slice(),
            protocol::SERVER_LIMITS.message,
        )
        .unwrap()
        .unwrap();
        let ServerMessage::Full(compressed) = protocol::wire::decode_server(body).unwrap() else {
            panic!("expected full snapshot");
        };
        assert_eq!(inflate(&compressed).unwrap(), state);
    }

    #[test]
    fn stalled_or_truncated_greetings_keep_their_cause_without_claiming_old_protocol() {
        for cause in [
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::ConnectionReset,
        ] {
            let failure = greeting_failure(std::io::Error::from(cause).into());
            assert!(failure.downcast_ref::<GreetingFailure>().is_some());
            assert!(failure.chain().any(|source| {
                source
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == cause)
            }));
            assert!(!failure.to_string().contains("incompatible protocol"));
        }
        assert!(
            greeting_failure(FrameError::Truncated.into())
                .downcast_ref::<GreetingFailure>()
                .is_some()
        );
        assert!(
            greeting_failure(FrameError::Malformed.into())
                .downcast_ref::<Error>()
                .is_some()
        );
    }

    #[test]
    fn attachment_commands_require_a_literal_terminal_id() {
        let id = holder_id(Uuid::new_v4());
        let executable = "\"$HOME/.local/share/newport/needle/builds/verified/newport-agent\"";
        assert_eq!(
            attachment_command(executable, &id).unwrap(),
            format!("exec {executable} terminal attach {id}")
        );
        assert!(attachment_command(executable, "x; false").is_err());
    }

    #[test]
    fn a_held_shell_names_its_terminal_and_needles_commands() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let login = home.path().join("login shell");
        std::fs::write(
            &login,
            "#!/bin/sh\nprintf '%s\\n' \"$1\" \"$NEWPORT_TERMINAL\" \"$BROWSER\" \"$PATH\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&login, std::fs::Permissions::from_mode(0o700)).unwrap();
        let terminal = Uuid::new_v4();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &held_command(terminal)])
            .env("HOME", home.path())
            .env("SHELL", &login)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let home = home.path().display();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "-l\n{terminal}\n{home}/.local/bin/newport-browser\n\
                 {home}/.local/bin:/usr/bin:/bin\n"
            )
        );
    }
}
