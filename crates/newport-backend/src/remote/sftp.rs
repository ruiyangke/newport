//! SFTP over a server's link: one shared session for everyday file
//! operations, and a session of its own for each transfer.
use super::{lost, unsent};
use newport_ssh::{Link, Open};
use russh::ChannelMsg;
use russh_sftp::client::RawSftpSession;
use std::{
    fmt, io,
    pin::Pin,
    sync::{
        Arc, Mutex as StdMutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    time::Instant,
};

/// How long opening a session may take once its channel is open.
const SETUP_TIMEOUT: Duration = Duration::from_secs(20);
/// How long one SFTP request may take. The library times each request from
/// its first poll, and bulk transfers keep a window of reads in flight, so a
/// slow link needs longer than the library's default for the last of them.
const REQUEST_TIMEOUT_SECS: u64 = 120;
/// How long the shared session stays open without operations.
pub const SHARED_IDLE: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No channel could be opened, or its connection was lost.
    Link(newport_ssh::Error),
    /// The server has no SFTP subsystem.
    Unavailable,
    /// The SFTP handshake failed.
    Protocol(String),
    TimedOut,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Link(error) => error.fmt(f),
            Error::Unavailable => f.write_str("The server doesn't provide SFTP."),
            Error::Protocol(error) => write!(f, "SFTP setup failed: {error}"),
            Error::TimedOut => f.write_str("SFTP setup timed out."),
        }
    }
}

impl std::error::Error for Error {}

impl From<newport_ssh::Error> for Error {
    fn from(error: newport_ssh::Error) -> Self {
        Error::Link(error)
    }
}

/// One SFTP session on its own channel. Request IDs multiplex independent
/// operations on it.
pub struct Session {
    raw: RawSftpSession,
    closed: Arc<AtomicBool>,
    /// A held session keeps its server's connections open.
    _link: Link,
}

impl std::ops::Deref for Session {
    type Target = RawSftpSession;
    fn deref(&self) -> &RawSftpSession {
        &self.raw
    }
}

impl Session {
    /// Waits while the link reconnects, but fails with the link's error while
    /// it has stopped retrying: every SFTP operation is one someone waits for,
    /// or one that is retried later.
    async fn open(link: &Link) -> Result<Self, Error> {
        let mut channel = link
            .open_unless_stopped(Open::Session)
            .await
            .map_err(Error::Link)?;
        tokio::time::timeout(SETUP_TIMEOUT, async move {
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(|error| Error::Link(unsent(error)))?;
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Success) => break,
                    Some(ChannelMsg::Failure) => return Err(Error::Unavailable),
                    Some(_) => {}
                    None => {
                        return Err(match lost(&channel.fate()).await {
                            Some(error) => Error::Link(error),
                            None => Error::Unavailable,
                        });
                    }
                }
            }
            let closed = Arc::new(AtomicBool::new(false));
            let raw = RawSftpSession::new(Watched {
                inner: channel.into_stream(),
                closed: closed.clone(),
            });
            raw.init()
                .await
                .map_err(|error| Error::Protocol(error.to_string()))?;
            raw.set_timeout(REQUEST_TIMEOUT_SECS);
            Ok(Self {
                raw,
                closed,
                _link: link.clone(),
            })
        })
        .await
        .map_err(|_| Error::TimedOut)?
    }

    /// Whether the session's channel has closed; its requests then fail.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// Marks the session closed once its stream ends or fails.
struct Watched {
    inner: russh::ChannelStream<russh::client::Msg>,
    closed: Arc<AtomicBool>,
}

impl Drop for Watched {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

impl AsyncRead for Watched {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let capacity = buf.remaining();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        let ended = match &result {
            Poll::Ready(Err(_)) => true,
            Poll::Ready(Ok(())) => capacity > 0 && buf.filled().len() == before,
            Poll::Pending => false,
        };
        if ended {
            self.closed.store(true, Ordering::Release);
        }
        result
    }
}

impl AsyncWrite for Watched {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, bytes);
        if matches!(result, Poll::Ready(Err(_))) {
            self.closed.store(true, Ordering::Release);
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A server's SFTP. Cloning shares it.
#[derive(Clone)]
pub struct Sftp {
    link: Link,
    state: Arc<State>,
}

struct State {
    session: tokio::sync::Mutex<Option<Arc<Session>>>,
    /// When the shared session was last handed out or given back.
    used: StdMutex<Instant>,
    idle: Duration,
}

impl Sftp {
    pub fn new(link: Link) -> Self {
        Self::with_idle(link, SHARED_IDLE)
    }

    pub(crate) fn with_idle(link: Link, idle: Duration) -> Self {
        Self {
            link,
            state: Arc::new(State {
                session: tokio::sync::Mutex::default(),
                used: StdMutex::new(Instant::now()),
                idle,
            }),
        }
    }

    /// The shared session, opened on first use and again once its channel
    /// has closed. It closes after [`SHARED_IDLE`] with no [`Shared`] held.
    pub async fn shared(&self) -> Result<Shared, Error> {
        let mut cached = self.state.session.lock().await;
        if let Some(session) = cached.as_ref().filter(|session| !session.is_closed()) {
            return Ok(Shared::new(session.clone(), &self.state));
        }
        let session = Arc::new(Session::open(&self.link).await?);
        *cached = Some(session.clone());
        let shared = Shared::new(session.clone(), &self.state);
        tokio::spawn(close_when_idle(
            Arc::downgrade(&self.state),
            Arc::downgrade(&session),
        ));
        Ok(shared)
    }

    /// A session on its own channel, so a large transfer never queues behind
    /// other operations. It closes when dropped.
    pub async fn dedicated(&self) -> Result<Session, Error> {
        Session::open(&self.link).await
    }
}

/// A hold on the shared session; it stays open while any is held.
pub struct Shared {
    session: Arc<Session>,
    state: Arc<State>,
}

impl Shared {
    fn new(session: Arc<Session>, state: &Arc<State>) -> Self {
        *state.used.lock().unwrap() = Instant::now();
        Self {
            session,
            state: state.clone(),
        }
    }
}

#[cfg(test)]
impl Shared {
    pub(crate) fn downgrade(&self) -> Weak<Session> {
        Arc::downgrade(&self.session)
    }
}

impl std::ops::Deref for Shared {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        *self.state.used.lock().unwrap() = Instant::now();
    }
}

/// Drops `session` from the cache once nothing has held it for the idle
/// time, which closes its channel. Ends early when the session is replaced.
async fn close_when_idle(state: Weak<State>, session: Weak<Session>) {
    let mut deadline = match state.upgrade() {
        Some(state) => *state.used.lock().unwrap() + state.idle,
        None => return,
    };
    loop {
        tokio::time::sleep_until(deadline).await;
        let Some(state) = state.upgrade() else {
            return;
        };
        let mut cached = state.session.lock().await;
        let Some(current) = cached
            .as_ref()
            .filter(|current| std::ptr::eq(Arc::as_ptr(current), session.as_ptr()))
        else {
            return;
        };
        // The cache holds one reference; any other is a `Shared` in use.
        if Arc::strong_count(current) > 1 {
            deadline = Instant::now() + state.idle;
            continue;
        }
        let used = *state.used.lock().unwrap();
        if used + state.idle <= Instant::now() {
            *cached = None;
            return;
        }
        deadline = used + state.idle;
    }
}
