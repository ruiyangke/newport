//! Application intent and observation over local IPC. No presentation, license
//! renewal, clipboard synchronization, SSH stream, or automatic launch retry.
use crate::{
    account,
    backend::{
        Backend,
        exchange::{Error, Phase as ExchangePhase},
    },
    desktop_inputs,
};
pub use newport_protocol::desktop::applications::{Application as Handle, Target};
use newport_protocol::desktop::applications::{Ending, Outcome, State as Phase};
use newport_protocol::desktop::{self, applications as protocol};
use serde_json::Value;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

type Exchange =
    dyn Fn(protocol::Operation, Duration) -> Result<protocol::Reply, Error> + Send + Sync;
const MAX_PENDING: usize = 32;
const READ_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    server: uuid::Uuid,
    expected: desktop::configuration::ConnectionScope,
}
impl Scope {
    pub fn new(server: &str, expected: Value) -> Result<Self, String> {
        let id: uuid::Uuid = server.parse().map_err(|_| "Invalid server identity")?;
        let expected =
            desktop_inputs::connection_scope(&expected).map_err(|error| error.to_string())?;
        if expected.settings.server_id != id {
            return Err("Refresh the server connection before opening applications.".into());
        }
        Ok(Self {
            server: id,
            expected,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Starting,
    /// It still runs, but opened no window in time. A window may still open.
    Background,
    Running,
    Stopping,
    Ended,
    /// It ended after the launch failed, with what it reported.
    Failed(String),
    /// It ended without opening a window, with the end of what it reported.
    EndedWithoutWindow(Option<String>),
    /// An instance of the app that was already running took the launch.
    HandedOff,
    /// Its display stopped on the server, and its windows closed with it.
    DisplayStopped,
    /// Another computer shows its display now, and it keeps running there:
    /// that computer's name, None when it gave none. It is still observed:
    /// once this computer shows the display again, it runs here again.
    ShownElsewhere(Option<String>),
    Refused {
        message: String,
        action: Option<account::Action>,
    },
    /// Neither failure nor success is established. Never replay the launch.
    Unknown(String),
    /// The backend confirmed that the original display can no longer be used.
    Unavailable(String),
}
impl Status {
    /// Nothing further will be observed for it.
    fn ended(&self) -> bool {
        matches!(
            self,
            Self::Ended
                | Self::Failed(_)
                | Self::EndedWithoutWindow(_)
                | Self::HandedOff
                | Self::DisplayStopped
                | Self::Refused { .. }
                | Self::Unavailable(_)
        )
    }
}
struct State {
    phase: Status,
    handle: Option<Handle>,
    stop_requested: bool,
    stop_sent: bool,
    stop_error: Option<String>,
}
struct Shared {
    scope: Scope,
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct Application(Arc<Shared>);
impl Application {
    pub fn status(&self) -> Status {
        let state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(error) = &state.stop_error {
            return Status::Unknown(error.clone());
        }
        if state.stop_requested
            && matches!(
                state.phase,
                Status::Starting | Status::Background | Status::Running
            )
        {
            Status::Stopping
        } else {
            state.phase.clone()
        }
    }
    /// Explicit repeated Quit may retry an uncertain stop on the same handle.
    /// It never substitutes a display-wide quit or a replacement connection.
    pub fn stop(&self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.stop_requested = true;
        if state.stop_error.take().is_some() {
            state.stop_sent = false;
        }
    }
}

pub struct Controller {
    exchange: Arc<Exchange>,
    launches: Arc<AtomicUsize>,
    polling: Arc<AtomicBool>,
    next_poll: Option<Instant>,
}
impl Controller {
    pub fn new(backend: Backend) -> Self {
        Self::with_exchange(Arc::new(move |operation, timeout| {
            match backend.execute_with_timeout(
                desktop::Operation::Applications(Box::new(operation)),
                timeout,
            )? {
                desktop::Reply::Applications(value) => Ok(*value),
                _ => Err(Error::new(
                    &backend,
                    ExchangePhase::Response,
                    true,
                    desktop::Invalid("applications response"),
                )),
            }
        }))
    }
    fn with_exchange(exchange: Arc<Exchange>) -> Self {
        Self {
            exchange,
            launches: Default::default(),
            polling: Default::default(),
            next_poll: None,
        }
    }
    pub fn launch(&self, scope: Scope, target: Target) -> Result<Application, String> {
        target.validate().map_err(|error| error.to_string())?;
        self.launches
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_PENDING).then_some(n + 1)
            })
            .map_err(|_| "Too many applications are still opening.")?;
        let release = Launching(self.launches.clone());
        let application = Application(Arc::new(Shared {
            scope,
            state: Mutex::new(State {
                phase: Status::Starting,
                handle: None,
                stop_requested: false,
                stop_sent: false,
                stop_error: None,
            }),
        }));
        let work = application.clone();
        let exchange = self.exchange.clone();
        std::thread::Builder::new().name("remote-app-launch".into()).spawn(move || {
            let _release = release;
            {
                let mut state = work.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.stop_requested {
                    state.phase = Status::Ended;
                    return;
                }
            }
            let operation = protocol::Operation::Launch(Box::new(protocol::LaunchArgs {
                server: work.0.scope.server, expected: work.0.scope.expected.clone(), target,
            }));
            let result = exchange(operation, Duration::from_secs(600));
            let mut state = work.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match result {
                Ok(protocol::Reply::Launch(result)) => {
                    state.handle = Some(result.application);
                    state.phase = match result.outcome {
                        Outcome::Accepted(_) => Status::Starting,
                        Outcome::Rejected(message) => Status::Refused { message: *message, action: None },
                        Outcome::Unknown(message) => Status::Unknown(*message),
                    };
                }
                Ok(_) => state.phase = Status::Unknown("The launch reply was invalid. Check the remote windows before trying again.".into()),
                Err(error) => state.phase = refusal(error),
            }
        }).map_err(|error| error.to_string())?;
        Ok(application)
    }
    /// Shows the server's apps on this computer without launching one: every
    /// window on its display moves here. `done` gets the result on another
    /// thread.
    pub fn show(
        &self,
        scope: Scope,
        done: impl FnOnce(Result<(), String>) + Send + 'static,
    ) -> Result<(), String> {
        let exchange = self.exchange.clone();
        std::thread::Builder::new()
            .name("remote-apps-show".into())
            .spawn(move || {
                // Includes account, installation and admission, as a launch does.
                let result = exchange(
                    protocol::Operation::Show(Box::new(protocol::ShowArgs {
                        server: scope.server,
                        expected: scope.expected,
                    })),
                    Duration::from_secs(600),
                );
                done(match result {
                    Ok(protocol::Reply::Show(_)) => Ok(()),
                    Ok(_) => Err("Invalid show applications response".into()),
                    Err(error) => Err(error.to_string()),
                });
            })
            .map(drop)
            .map_err(|error| error.to_string())
    }
    /// At most one observation worker, with one bounded batch per captured
    /// server connection. UI polls never block on IPC or create GUI consumers.
    pub fn poll(&mut self, applications: Vec<Application>) {
        if applications.is_empty() {
            return;
        }
        if self.next_poll.is_some_and(|at| Instant::now() < at)
            || self.polling.swap(true, Ordering::AcqRel)
        {
            return;
        }
        self.next_poll = Some(Instant::now() + Duration::from_millis(500));
        let exchange = self.exchange.clone();
        let guard = Polling(self.polling.clone());
        let _ = std::thread::Builder::new()
            .name("remote-app-status".into())
            .spawn(move || {
                let _guard = guard;
                let mut groups: Vec<(Scope, Vec<(Application, Handle)>)> = Vec::new();
                for application in applications {
                    let handle = {
                        let state = application
                            .0
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if state.phase.ended() {
                            continue;
                        }
                        state.handle.clone()
                    };
                    let Some(handle) = handle else { continue };
                    if let Some((_, group)) = groups
                        .iter_mut()
                        .find(|(scope, _)| scope == &application.0.scope)
                    {
                        group.push((application, handle));
                    } else {
                        groups.push((application.0.scope.clone(), vec![(application, handle)]));
                    }
                }
                for (scope, group) in groups {
                    for group in group.chunks(128) {
                        observe(&*exchange, &scope, group);
                    }
                }
            });
    }
}
struct Launching(Arc<AtomicUsize>);
impl Drop for Launching {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Polling(Arc<AtomicBool>);
impl Drop for Polling {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
fn refusal(error: Error) -> Status {
    if error.code() == "APPLICATION_NOT_SUBMITTED" {
        return Status::Refused {
            message: error.to_string(),
            action: None,
        };
    }
    if let Some(reason) = error.license_refusal() {
        let action = match reason {
            desktop::account::RefusalReason::SignedOut => Some(account::Action::SignIn),
            desktop::account::RefusalReason::Free
            | desktop::account::RefusalReason::TrialElsewhere => Some(account::Action::OpenBilling),
            desktop::account::RefusalReason::FreeSeat => Some(account::Action::OpenPortal),
            desktop::account::RefusalReason::Unavailable
            | desktop::account::RefusalReason::Clock
            | desktop::account::RefusalReason::Offline => None,
        };
        return Status::Refused {
            action,
            message: error.to_string(),
        };
    }
    Status::Unknown(error.to_string())
}

fn observe(exchange: &Exchange, scope: &Scope, group: &[(Application, Handle)]) {
    let operation = protocol::Operation::States(Box::new(protocol::StatesArgs {
        server: scope.server,
        expected: scope.expected.clone(),
        applications: group.iter().map(|(_, handle)| handle.clone()).collect(),
    }));
    let value = match exchange(operation, READ_TIMEOUT) {
        Ok(value) => value,
        Err(error) => {
            let phase = if error.code() == "APPLICATION_CONNECTION_CHANGED" {
                Status::Unavailable(error.to_string())
            } else {
                Status::Unknown(error.to_string())
            };
            for (application, _) in group {
                let mut state = application
                    .0
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if matches!(state.phase, Status::ShownElsewhere(_)) {
                    // Its windows aren't here: nothing to check or to control.
                    if matches!(phase, Status::Unavailable(_)) {
                        state.phase = Status::Ended;
                    }
                    continue;
                }
                state.phase = phase.clone();
                if matches!(phase, Status::Unavailable(_)) {
                    state.stop_error = None;
                }
            }
            return;
        }
    };
    let result = match value {
        protocol::Reply::States(rows) => {
            let rows = rows.observations;
            if rows.len() != group.len()
                || rows.iter().zip(group).any(|(row, (_, handle))| {
                    &row.application != handle || row.state.is_some() == row.error.is_some()
                })
            {
                Err("Application observation identities changed".to_owned())
            } else {
                Ok(rows)
            }
        }
        _ => Err("Invalid application observations".to_owned()),
    };
    let rows = match result {
        Ok(rows) => rows,
        Err(message) => {
            for (application, _) in group {
                let mut state = application
                    .0
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !matches!(state.phase, Status::ShownElsewhere(_)) {
                    state.phase = Status::Unknown(message.clone());
                }
            }
            return;
        }
    };
    for ((application, handle), row) in group.iter().zip(rows) {
        let stop = {
            let mut state = application
                .0
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // An ending the person asked for is not a failure to report.
            let requested = state.stop_requested;
            let elsewhere = matches!(state.phase, Status::ShownElsewhere(_));
            state.phase =
                match row.state {
                    Some(Phase::Pending | Phase::Opening) => Status::Starting,
                    Some(Phase::Background) => Status::Background,
                    Some(Phase::Running) => Status::Running,
                    Some(Phase::Stopping) => Status::Stopping,
                    Some(Phase::Ended) => match row.ending {
                        Some(Ending::ShownElsewhere(computer)) => {
                            Status::ShownElsewhere(computer.detail)
                        }
                        // Its display ended there, or is no longer reachable:
                        // none of its windows were here.
                        _ if elsewhere => Status::Ended,
                        Some(
                            Ending::Failed(_) | Ending::NoWindow(_) | Ending::DisplayStopped(_),
                        ) if requested => Status::Ended,
                        Some(Ending::Failed(message)) => Status::Failed(*message),
                        Some(Ending::NoWindow(output)) => Status::EndedWithoutWindow(output.detail),
                        Some(Ending::HandedOff(_)) => Status::HandedOff,
                        Some(Ending::DisplayStopped(_)) => Status::DisplayStopped,
                        None => Status::Ended,
                    },
                    None if elsewhere => Status::Ended,
                    None => Status::Unavailable(row.error.unwrap()),
                };
            if matches!(
                state.phase,
                Status::Ended
                    | Status::Failed(_)
                    | Status::EndedWithoutWindow(_)
                    | Status::HandedOff
                    | Status::DisplayStopped
                    | Status::ShownElsewhere(_)
                    | Status::Unavailable(_)
            ) {
                state.stop_error = None;
            }
            let stop = state.stop_requested
                && !state.stop_sent
                && matches!(
                    row.state,
                    Some(Phase::Opening | Phase::Background | Phase::Running)
                );
            if stop {
                state.stop_sent = true;
            }
            stop
        };
        if stop {
            let operation = protocol::Operation::Stop(Box::new(protocol::StopArgs {
                server: scope.server,
                expected: scope.expected.clone(),
                application: handle.clone(),
            }));
            let result = exchange(operation, READ_TIMEOUT);
            let message = match result {
                Ok(protocol::Reply::Stop(_)) => None,
                Ok(_) => Some(
                    "Quit could not be confirmed. Check the app before choosing Quit again.".into(),
                ),
                Err(error) => Some(error.to_string()),
            };
            application
                .0
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop_error = message;
        }
    }
}

#[cfg(test)]
mod tests;
