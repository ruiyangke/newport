use std::fmt;

/// Everything that can go wrong reaching a server, classified once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The server couldn't be reached.
    Unreachable(String),
    /// The proxy command ended before the connection did; `output` is what
    /// it wrote to stderr, or how it exited.
    ProxyFailed { output: String },
    /// The proxy command can't be run as configured, such as one with an
    /// unknown token.
    InvalidProxy(String),
    /// The jump host can't be used as configured, such as one that is no
    /// longer saved.
    InvalidJump(String),
    /// A step with a deadline didn't finish in time.
    TimedOut,
    /// The connection under a lease closed.
    Lost(String),
    /// The chosen SSH agent isn't running.
    AgentUnavailable,
    /// The SSH agent didn't approve the login.
    ApprovalRequired,
    /// The server refused every credential.
    AuthRefused,
    /// A credential on this computer couldn't be used, such as an identity
    /// file that can't be read.
    Credentials(String),
    /// The server presented a different host key than the one it is known by.
    HostKeyChanged { fingerprint: String },
    /// The host key verifier refused the server's key for another reason,
    /// such as a host certificate.
    HostKeyRejected(String),
    /// The server allows no sessions (`MaxSessions 0`).
    SessionsDisabled,
    /// The server refused a forwarding channel.
    ChannelRefused { reason: russh::ChannelOpenFailure },
    /// The server refused a remote forward, for example because a connection
    /// it hasn't noticed is gone still holds the port.
    RequestDenied,
    /// Too many requests already wait.
    Busy,
    /// The link was dropped.
    Closed,
}

/// What the link does after an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retry {
    /// Try again after a growing pause.
    Backoff,
    /// Wait for [`crate::Link::retry`].
    OnRetry,
    /// Retrying can't help.
    Never,
}

impl Error {
    pub fn retry(&self) -> Retry {
        match self {
            Error::Unreachable(_)
            | Error::ProxyFailed { .. }
            | Error::TimedOut
            | Error::Lost(_)
            | Error::AgentUnavailable
            | Error::RequestDenied => Retry::Backoff,
            Error::ApprovalRequired
            | Error::AuthRefused
            | Error::HostKeyChanged { .. }
            | Error::HostKeyRejected(_) => Retry::OnRetry,
            Error::InvalidProxy(_)
            | Error::InvalidJump(_)
            | Error::SessionsDisabled
            | Error::Credentials(_)
            | Error::ChannelRefused { .. }
            | Error::Busy
            | Error::Closed => Retry::Never,
        }
    }

    /// Whether a change on this computer, such as a wake, a network change
    /// or the person unlocking it, may let a login that stopped on this error
    /// succeed: the agent can ask for approval again, or offer keys it held
    /// back while locked. A host key needs the person to check it.
    pub fn eased_by_system(&self) -> bool {
        matches!(self, Error::ApprovalRequired | Error::AuthRefused)
    }

    /// A stable name for the error, for the app to word it.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Unreachable(_) => "SSH_UNREACHABLE",
            Error::ProxyFailed { .. } => "SSH_PROXY_FAILED",
            Error::InvalidProxy(_) => "SSH_INVALID_PROXY",
            Error::InvalidJump(_) => "SSH_INVALID_JUMP",
            Error::TimedOut => "SSH_TIMED_OUT",
            Error::Lost(_) => "SSH_LOST",
            Error::AgentUnavailable => "SSH_AGENT_UNAVAILABLE",
            Error::ApprovalRequired => "SSH_APPROVAL_REQUIRED",
            Error::AuthRefused => "SSH_AUTH_REFUSED",
            Error::Credentials(_) => "SSH_CREDENTIALS",
            Error::HostKeyChanged { .. } => "SSH_HOST_KEY_CHANGED",
            Error::HostKeyRejected(_) => "SSH_HOST_KEY_REJECTED",
            Error::SessionsDisabled => "SSH_SESSIONS_DISABLED",
            Error::ChannelRefused { .. } => "SSH_CHANNEL_REFUSED",
            Error::RequestDenied => "SSH_REQUEST_DENIED",
            Error::Busy => "SSH_BUSY",
            Error::Closed => "SSH_CLOSED",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unreachable(cause) => write!(f, "The server couldn't be reached: {cause}"),
            Error::ProxyFailed { output } => {
                write!(f, "The proxy command failed: {}", output.trim())
            }
            Error::InvalidProxy(reason) => {
                write!(f, "The proxy command can't be run: {reason}")
            }
            Error::InvalidJump(reason) => write!(f, "The jump host can't be used: {reason}"),
            Error::TimedOut => f.write_str("The server didn't answer in time."),
            Error::Lost(cause) => write!(f, "The connection was lost: {cause}"),
            Error::AgentUnavailable => f.write_str("The SSH agent isn't running."),
            Error::ApprovalRequired => f.write_str("The SSH agent didn't approve the login."),
            Error::AuthRefused => f.write_str("The server refused the login."),
            Error::Credentials(reason) => {
                write!(
                    f,
                    "This server's SSH credentials couldn't be used: {reason}"
                )
            }
            Error::HostKeyChanged { fingerprint } => {
                write!(f, "The server's host key changed to {fingerprint}.")
            }
            Error::HostKeyRejected(reason) => {
                write!(f, "The server's host key was refused: {reason}")
            }
            Error::SessionsDisabled => f.write_str("The server allows no SSH sessions."),
            Error::ChannelRefused { reason } => {
                write!(f, "The server refused the channel ({reason:?}).")
            }
            Error::RequestDenied => f.write_str("The server refused to forward the port."),
            Error::Busy => f.write_str("Too many requests are waiting for this server."),
            Error::Closed => f.write_str("The link was closed."),
        }
    }
}

impl std::error::Error for Error {}
