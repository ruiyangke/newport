//! One admitted operation per connection. Request bytes are never replayed.
use super::Backend;
use newport_platform::ipc::{
    self,
    framed::{Reader, Writer},
};
use newport_protocol::{
    common,
    desktop::{self, wire},
};
use std::{
    error, fmt, io,
    net::Shutdown,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Connect,
    Admission,
    Request,
    Response,
    Closure,
}

#[derive(Debug)]
pub enum Cause {
    Io(io::Error),
    Wire(wire::Error),
    Contract(desktop::Invalid),
    Refused(common::Error),
    Remote(desktop::Failure),
}
impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(value) => value.fmt(f),
            Self::Wire(value) => value.fmt(f),
            Self::Contract(value) => value.fmt(f),
            Self::Refused(value) => f.write_str(&value.message),
            Self::Remote(value) => f.write_str(&value.error.message),
        }
    }
}
impl error::Error for Cause {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(match self {
            Self::Io(value) => value,
            Self::Wire(value) => value,
            Self::Contract(value) => value,
            Self::Refused(value) => value,
            Self::Remote(value) => &value.error,
        })
    }
}
impl From<io::Error> for Cause {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<wire::Error> for Cause {
    fn from(value: wire::Error) -> Self {
        Self::Wire(value)
    }
}
impl From<desktop::Invalid> for Cause {
    fn from(value: desktop::Invalid) -> Self {
        Self::Contract(value)
    }
}

#[derive(Debug)]
pub struct Error {
    pub phase: Phase,
    pub cause: Cause,
    /// Some request bytes may have reached the owner. This never authorizes replay.
    pub dispatched: bool,
    endpoint: PathBuf,
    changes_state: bool,
}
impl Error {
    pub(crate) fn new(
        backend: &Backend,
        phase: Phase,
        dispatched: bool,
        cause: impl Into<Cause>,
    ) -> Self {
        Self {
            phase,
            cause: cause.into(),
            dispatched,
            endpoint: backend.socket_path().to_owned(),
            changes_state: false,
        }
    }
    pub fn retry(&self) -> common::Retry {
        match &self.cause {
            Cause::Refused(value) => value.retry,
            Cause::Remote(value) => value.error.retry,
            _ if self.dispatched => common::Retry::Unknown,
            _ => common::Retry::No,
        }
    }
    pub fn code(&self) -> &str {
        match &self.cause {
            Cause::Refused(value) => &value.code,
            Cause::Remote(value) => &value.error.code,
            Cause::Contract(_) | Cause::Wire(_) => "PROTOCOL_ERROR",
            Cause::Io(_) => "CONNECTION_INTERRUPTED",
        }
    }
    pub fn license_refusal(&self) -> Option<desktop::account::RefusalReason> {
        match &self.cause {
            Cause::Remote(desktop::Failure {
                detail: Some(desktop::FailureDetail::LicenseRefusal(value)),
                ..
            }) => Some(**value),
            _ => None,
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.phase == Phase::Connect {
            match &self.cause {
                Cause::Io(error) => f.write_str(&super::unreachable_backend(&self.endpoint, error)),
                cause => write!(
                    f,
                    "Newport’s backend could not be reached at {}: {cause}",
                    self.endpoint.display()
                ),
            }
        } else if self.dispatched && self.changes_state && !matches!(self.cause, Cause::Remote(_)) {
            write!(
                f,
                "{}. The request was not retried; check the outcome before repeating a write.",
                self.cause
            )
        } else {
            self.cause.fmt(f)
        }
    }
}
impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(&self.cause)
    }
}

/// The admitted stream retains all bytes read alongside its opening response.
pub struct Opened {
    pub identity: desktop::stream::Opened,
    pub reader: Reader,
    pub writer: Writer,
}
impl Opened {
    pub fn cancel(&self) -> io::Result<()> {
        self.reader.try_clone_stream()?.shutdown(Shutdown::Both)
    }
}

enum ResultValue {
    Complete(desktop::Request, desktop::Reply),
    Opened(Opened),
}

impl Backend {
    pub fn execute(&self, operation: desktop::Operation) -> Result<desktop::Reply, Error> {
        let timeout = operation_timeout(&operation);
        self.execute_with_timeout(operation, timeout)
    }
    pub fn execute_with_timeout(
        &self,
        operation: desktop::Operation,
        timeout: Duration,
    ) -> Result<desktop::Reply, Error> {
        self.execute_retained_with_timeout(operation, timeout)
            .map(|(_, reply)| reply)
    }
    pub(crate) fn execute_retained_with_timeout(
        &self,
        operation: desktop::Operation,
        timeout: Duration,
    ) -> Result<(desktop::Operation, desktop::Reply), Error> {
        if matches!(operation, desktop::Operation::Stream(_)) {
            return Err(Error::new(
                self,
                Phase::Request,
                false,
                desktop::Invalid("stream operation requires open"),
            ));
        }
        match self.exchange(operation, timeout)? {
            ResultValue::Complete(request, value) => Ok((request.operation, value)),
            ResultValue::Opened(_) => Err(Error::new(
                self,
                Phase::Response,
                true,
                desktop::Invalid("unexpected opened stream"),
            )),
        }
    }
    pub fn open(
        &self,
        operation: desktop::stream::Operation,
        timeout: Duration,
    ) -> Result<Opened, Error> {
        match self.exchange(desktop::Operation::Stream(Box::new(operation)), timeout)? {
            ResultValue::Opened(value) => Ok(value),
            ResultValue::Complete(_, _) => Err(Error::new(
                self,
                Phase::Response,
                true,
                desktop::Invalid("missing opened stream"),
            )),
        }
    }

    fn exchange(
        &self,
        operation: desktop::Operation,
        timeout: Duration,
    ) -> Result<ResultValue, Error> {
        let changes_state = changes_state(&operation);
        self.exchange_inner(operation, timeout)
            .map_err(|mut error| {
                error.changes_state = changes_state;
                error
            })
    }
    fn exchange_inner(
        &self,
        operation: desktop::Operation,
        timeout: Duration,
    ) -> Result<ResultValue, Error> {
        let started = Instant::now();
        let end = started.checked_add(timeout).ok_or_else(|| {
            Error::new(
                self,
                Phase::Connect,
                false,
                desktop::Invalid("request deadline"),
            )
        })?;
        let admission = end.min(started + desktop::ADMISSION_TIMEOUT);
        let request = desktop::ClientMessage::Request(Box::new(desktop::Request {
            id: desktop::RequestId::new(1)
                .map_err(|error| Error::new(self, Phase::Request, false, error))?,
            operation,
        }));
        // Encode and validate before connecting, so invalid input reaches no owner.
        let frame = wire::encode_client(&request)
            .map_err(|error| Error::new(self, Phase::Request, false, error))?;
        let stream = ipc::connect_private_until(self.socket_path(), admission)
            .map_err(|error| Error::new(self, Phase::Connect, false, error))?;
        let mut reader = Reader::new(stream);
        let mut writer = Writer::new(
            reader
                .try_clone_stream()
                .map_err(|error| Error::new(self, Phase::Connect, false, error))?,
        );
        let mut state = desktop::Exchange::default();
        let hello = desktop::ClientMessage::Hello(Box::new(desktop::Hello {
            version: desktop::VERSION,
            contract_id: Some(desktop::CONTRACT_ID.into()),
        }));
        state
            .client(&hello)
            .map_err(|error| Error::new(self, Phase::Admission, false, error))?;
        let greeting = wire::encode_client(&hello)
            .map_err(|error| Error::new(self, Phase::Admission, false, error))?;
        writer
            .send_until(&greeting, admission)
            .map_err(|error| Error::new(self, Phase::Admission, false, error))?;
        let greeting = reader
            .next_until(desktop::CONTROL_LIMITS.message, admission)
            .map_err(|error| Error::new(self, Phase::Admission, false, error))?
            .ok_or_else(|| Error::new(self, Phase::Admission, false, wire::Error::Ended))?;
        let greeting = wire::decode_admission(greeting.body())
            .map_err(|error| Error::new(self, Phase::Admission, false, error))?;
        if let desktop::ServerMessage::Ready(ready) = &greeting {
            desktop::require_contract_id(ready.contract_id.as_deref()).map_err(|error| {
                Error::new(self, Phase::Admission, false, Cause::Refused(error))
            })?;
        }
        state
            .server(&greeting)
            .map_err(|error| Error::new(self, Phase::Admission, false, error))?;
        if let desktop::ServerMessage::Refused(reason) = greeting {
            verify_eof(self, &mut reader, &mut state, admission, false)?;
            return Err(Error::new(
                self,
                Phase::Admission,
                false,
                Cause::Refused(*reason),
            ));
        }
        state
            .client(&request)
            .map_err(|error| Error::new(self, Phase::Request, false, error))?;
        writer
            .send_until(&frame, admission)
            .map_err(|error| Error::new(self, Phase::Request, true, error))?;
        drop(frame);
        let desktop::ClientMessage::Request(request) = request else {
            return Err(Error::new(
                self,
                Phase::Request,
                false,
                desktop::Invalid("request state"),
            ));
        };
        let mut clock = ResponseClock::new(end);
        loop {
            let deadline = clock
                .deadline(Instant::now())
                .map_err(|error| Error::new(self, Phase::Response, true, error))?;
            let frame = reader
                .next_until(desktop::MAX_RESPONSE, deadline)
                .map_err(|error| Error::new(self, Phase::Response, true, error))?
                .ok_or_else(|| Error::new(self, Phase::Response, true, wire::Error::Ended))?;
            let response = wire::decode_server_for(frame.body(), &request)
                .map_err(|error| Error::new(self, Phase::Response, true, error))?;
            state
                .server(&response)
                .map_err(|error| Error::new(self, Phase::Response, true, error))?;
            match response {
                desktop::ServerMessage::Progress(value) => {
                    let desktop::ProgressState::AgentApproval(value) = value.state;
                    clock
                        .approval(value.awaiting, Instant::now())
                        .map_err(|error| Error::new(self, Phase::Response, true, error))?;
                }
                desktop::ServerMessage::Response(value) => match value.outcome {
                    desktop::Outcome::Opened(identity) => {
                        return Ok(ResultValue::Opened(Opened {
                            identity: *identity,
                            reader,
                            writer,
                        }));
                    }
                    desktop::Outcome::Completed(reply) => {
                        verify_eof(self, &mut reader, &mut state, deadline, true)?;
                        return Ok(ResultValue::Complete(*request, *reply));
                    }
                    desktop::Outcome::Failed(failure) => {
                        verify_eof(self, &mut reader, &mut state, deadline, true)?;
                        return Err(Error::new(
                            self,
                            Phase::Response,
                            true,
                            Cause::Remote(*failure),
                        ));
                    }
                },
                desktop::ServerMessage::Ready(_) | desktop::ServerMessage::Refused(_) => {
                    return Err(Error::new(
                        self,
                        Phase::Response,
                        true,
                        desktop::Invalid("response phase"),
                    ));
                }
            }
        }
    }
}

fn verify_eof(
    backend: &Backend,
    reader: &mut Reader,
    state: &mut desktop::Exchange,
    deadline: Instant,
    dispatched: bool,
) -> Result<(), Error> {
    let deadline = deadline.min(Instant::now() + desktop::CLOSURE_TIMEOUT);
    if reader
        .next_until(desktop::MAX_RESPONSE, deadline)
        .map_err(|error| Error::new(backend, Phase::Closure, dispatched, error))?
        .is_some()
    {
        return Err(Error::new(
            backend,
            Phase::Closure,
            dispatched,
            desktop::Invalid("data after final response"),
        ));
    }
    state
        .eof()
        .map_err(|error| Error::new(backend, Phase::Closure, dispatched, error))
}

/// A consent wait pauses the operation budget, while each heartbeat has a bounded deadline.
struct ResponseClock {
    end: Instant,
    paused: Option<Duration>,
}
impl ResponseClock {
    fn new(end: Instant) -> Self {
        Self { end, paused: None }
    }
    fn deadline(&self, now: Instant) -> io::Result<Instant> {
        let end = if self.paused.is_some() {
            now + Duration::from_secs(30)
        } else {
            self.end
        };
        if end <= now {
            Err(io::Error::from(io::ErrorKind::TimedOut))
        } else {
            Ok(end)
        }
    }
    fn approval(&mut self, awaiting: bool, now: Instant) -> io::Result<()> {
        match (awaiting, self.paused) {
            (true, None) => {
                self.paused = Some(
                    self.end
                        .checked_duration_since(now)
                        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?,
                )
            }
            (false, Some(remaining)) => {
                self.end = now
                    .checked_add(remaining)
                    .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
                self.paused = None;
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn operation_timeout(operation: &desktop::Operation) -> Duration {
    use desktop::Kind;
    Duration::from_secs(match operation.kind() {
        Kind::FilesDownload | Kind::FilesUpload => 24 * 60 * 60,
        Kind::MetricsCollect
        | Kind::ConnectionReinstallAgent
        | Kind::ConnectionSetupInstall
        | Kind::ConnectionSetupFinish => 960,
        _ => 600,
    })
}

#[cfg(test)]
mod tests;

fn changes_state(operation: &desktop::Operation) -> bool {
    use desktop::*;
    match operation {
        Operation::Configuration(value) => {
            !matches!(value.as_ref(), configuration::Operation::Snapshot(_))
        }
        Operation::Connection(value) => matches!(
            value.as_ref(),
            connection::Operation::SetupInstall(_)
                | connection::Operation::SetupFinish(_)
                | connection::Operation::SetupSkip(_)
                | connection::Operation::ReinstallAgent(_)
                | connection::Operation::UninstallAgent(_)
        ),
        Operation::Preferences(value) => !matches!(
            value.as_ref(),
            preferences::Operation::ProjectsList(_)
                | preferences::Operation::Settings(_)
                | preferences::Operation::Installed(_)
        ),
        Operation::Account(value) => !matches!(
            value.as_ref(),
            account::Operation::Status(_)
                | account::Operation::RemoteLicense(_)
                | account::Operation::Refresh(_)
        ),
        Operation::Host(value) => !matches!(
            value.as_ref(),
            host::Operation::StartupSettings(_) | host::Operation::UpdateStatus(_)
        ),
        Operation::Files(value) => matches!(
            value.as_ref(),
            files::Operation::WriteText(_)
                | files::Operation::Upload(_)
                | files::Operation::Download(_)
        ),
        Operation::Metrics(value) => matches!(value.as_ref(), metrics::Operation::ClearCache(_)),
        Operation::Services(value) => matches!(value.as_ref(), services::Operation::Action(_)),
        Operation::Containers(value) => matches!(
            value.as_ref(),
            containers::Operation::Action(_)
                | containers::Operation::ProjectAction(_)
                | containers::Operation::Remove(_)
                | containers::Operation::Prune(_)
                | containers::Operation::ComposeRun(_)
                | containers::Operation::ComposeCancel(_)
        ),
        Operation::Applications(value) => matches!(
            value.as_ref(),
            applications::Operation::Launch(_)
                | applications::Operation::Show(_)
                | applications::Operation::Stop(_)
        ),
        Operation::Git(value) => match value.as_ref() {
            git::Operation::Request(value) => matches!(
                value.request,
                newport_protocol::git::Request::Start { .. }
                    | newport_protocol::git::Request::Init { .. }
                    | newport_protocol::git::Request::Clone { .. }
            ),
            git::Operation::AcknowledgeOperation(_) | git::Operation::ReviewOperation(_) => true,
            git::Operation::RefreshRepository(_) | git::Operation::PendingOperations(_) => false,
        },
        Operation::Terminal(value) => !matches!(value.as_ref(), terminal::Operation::List(_)),
        Operation::Stream(_) => true,
        Operation::Events(_) => false,
    }
}
