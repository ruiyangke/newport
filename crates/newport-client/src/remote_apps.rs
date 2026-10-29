//! Application intent and observation over local IPC. No presentation, license
//! renewal, clipboard synchronization, SSH stream, or automatic launch retry.
use crate::{account, backend::Backend, request_error::RequestError};
pub use newport_platform::ipc::applications::{Application as Handle, Elsewhere, Notice, Target};
use newport_platform::ipc::applications::{Ending, Launched, Observation, Outcome, State as Phase};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

type Exchange = dyn Fn(&str, Value, Duration) -> Result<Value, RequestError> + Send + Sync;
const MAX_PENDING: usize = 32;
const READ_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    server: uuid::Uuid,
    expected: Value,
}
impl Scope {
    pub fn new(server: &str, expected: Value) -> Result<Self, String> {
        let id: uuid::Uuid = server.parse().map_err(|_| "Invalid server identity")?;
        if !expected.as_array().is_some_and(|parts| parts.len() == 10)
            || expected[0].as_str() != Some(server)
            || expected[9].as_str().is_none_or(str::is_empty)
        {
            return Err("Refresh the server connection before opening applications.".into());
        }
        Ok(Self {
            server: id,
            expected,
        })
    }
    fn args(&self) -> Value {
        json!({"serverId":self.server,"expectedConnection":self.expected})
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
    /// What the server last said about it while it runs.
    notice: Option<Notice>,
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
        let state = self.0.state.lock().unwrap();
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
    /// Information about the running app, such as where its file dialogs
    /// open. Never set once it ended.
    pub fn notice(&self) -> Option<Notice> {
        let state = self.0.state.lock().unwrap();
        if state.phase.ended() {
            return None;
        }
        state.notice
    }
    /// Explicit repeated Quit may retry an uncertain stop on the same handle.
    /// It never substitutes a display-wide quit or a replacement connection.
    pub fn stop(&self) {
        let mut state = self.0.state.lock().unwrap();
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
        Self::with_exchange(Arc::new(move |method, args, timeout| {
            backend.call_detailed_with_timeout(method, args, timeout)
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
        target.validate()?;
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
                notice: None,
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
                let mut state = work.0.state.lock().unwrap();
                if state.stop_requested {
                    state.phase = Status::Ended;
                    return;
                }
            }
            let mut args = work.0.scope.args();
            args["target"] = serde_json::to_value(target).unwrap();
            // Includes account/installation/admission; intermediate agent
            // consent frames have the backend client's normal liveness bounds.
            let result = exchange("remote_apps_launch", args, Duration::from_secs(600));
            let mut state = work.0.state.lock().unwrap();
            match result {
                Ok(value) => match serde_json::from_value::<Launched>(value) {
                    Ok(result) => {
                        state.handle = Some(result.application);
                        state.phase = match result.outcome {
                            Outcome::Accepted => Status::Starting,
                            Outcome::Rejected(message) => Status::Refused { message, action: None },
                            Outcome::Unknown(message) => Status::Unknown(message),
                        };
                    }
                    Err(_) => state.phase = Status::Unknown("The launch reply was invalid. Check the remote windows before trying again.".into()),
                },
                Err(error) => state.phase = refusal(error),
            }
        }).map_err(|error| error.to_string())?;
        Ok(application)
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
                        let state = application.0.state.lock().unwrap();
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
fn refusal(error: RequestError) -> Status {
    if error.code.as_deref() == Some("APPLICATION_NOT_SUBMITTED") {
        return Status::Refused {
            message: error.message,
            action: None,
        };
    }
    if let Some(reason) = error
        .code
        .as_deref()
        .and_then(|code| code.strip_prefix("APPLICATION_REFUSED:"))
    {
        let refusal = account::LicenseRefusal {
            reason: Some(reason.into()),
            message: error.message,
        };
        return Status::Refused {
            action: refusal.action(),
            message: refusal.message,
        };
    }
    Status::Unknown(error.message)
}
fn observe(exchange: &Exchange, scope: &Scope, group: &[(Application, Handle)]) {
    let mut args = scope.args();
    args["applications"] = json!(group.iter().map(|(_, handle)| handle).collect::<Vec<_>>());
    let value = match exchange("remote_apps_states", args, READ_TIMEOUT) {
        Ok(value) => value,
        Err(error) => {
            let phase = if error.code.as_deref() == Some("APPLICATION_CONNECTION_CHANGED") {
                Status::Unavailable(error.message)
            } else {
                Status::Unknown(error.message)
            };
            for (application, _) in group {
                let mut state = application.0.state.lock().unwrap();
                state.phase = phase.clone();
                if matches!(phase, Status::Unavailable(_)) {
                    state.stop_error = None;
                }
            }
            return;
        }
    };
    let result = serde_json::from_value::<Vec<Observation>>(value)
        .map_err(|_| "Invalid application observations".to_owned())
        .and_then(|rows| {
            if rows.len() != group.len()
                || rows.iter().zip(group).any(|(row, (_, handle))| {
                    &row.application != handle || row.state.is_some() == row.error.is_some()
                })
            {
                return Err("Application observation identities changed".into());
            }
            Ok(rows)
        });
    let rows = match result {
        Ok(rows) => rows,
        Err(message) => {
            for (application, _) in group {
                application.0.state.lock().unwrap().phase = Status::Unknown(message.clone());
            }
            return;
        }
    };
    for ((application, handle), row) in group.iter().zip(rows) {
        let stop = {
            let mut state = application.0.state.lock().unwrap();
            // An ending the person asked for is not a failure to report.
            let requested = state.stop_requested;
            state.notice = row.notice;
            state.phase = match row.state {
                Some(Phase::Pending | Phase::Opening) => Status::Starting,
                Some(Phase::Background) => Status::Background,
                Some(Phase::Running) => Status::Running,
                Some(Phase::Stopping) => Status::Stopping,
                Some(Phase::Ended) => match row.ending {
                    Some(Ending::Failed(_) | Ending::NoWindow(_) | Ending::DisplayStopped)
                        if requested =>
                    {
                        Status::Ended
                    }
                    Some(Ending::Failed(message)) => Status::Failed(message),
                    Some(Ending::NoWindow(output)) => Status::EndedWithoutWindow(output),
                    Some(Ending::HandedOff) => Status::HandedOff,
                    Some(Ending::DisplayStopped) => Status::DisplayStopped,
                    None => Status::Ended,
                },
                None => Status::Unavailable(row.error.unwrap()),
            };
            if matches!(
                state.phase,
                Status::Ended
                    | Status::Failed(_)
                    | Status::EndedWithoutWindow(_)
                    | Status::HandedOff
                    | Status::DisplayStopped
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
            let mut args = scope.args();
            args["application"] = serde_json::to_value(handle).unwrap();
            let result = exchange("remote_apps_stop", args, READ_TIMEOUT);
            let message = match result {
                Ok(Value::Bool(_)) => None,
                Ok(_) => Some(
                    "Quit could not be confirmed. Check the app before choosing Quit again.".into(),
                ),
                Err(error) => Some(error.message),
            };
            application.0.state.lock().unwrap().stop_error = message;
        }
    }
}

#[cfg(test)]
mod tests;
