//! Live Remote apps sessions. Newport's server queries find the agent and
//! list the applications; this module only starts, lists and stops sessions.
//!
//! Every session runs over Newport's own connection to the server, so its
//! sign-in, host trust and reconnection apply, and each remote window is a
//! native window of this application. Streaming needs the private Newport
//! Remote crates, which only full builds add (see `build.rs`).
use crate::remote_apps_api as api;
use gpui_kit::App;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// What a launch needs from the page: preferences and the server's agent.
pub(crate) struct Launch<'a> {
    pub server: &'a str,
    /// The server's display name, for session windows and reports.
    #[cfg_attr(not(newport_remote_apps), allow(dead_code))]
    pub server_name: &'a str,
    pub agent: &'a str,
    /// Whether the server can run X11 applications; otherwise "Automatic"
    /// means Wayland.
    pub x11: bool,
    #[cfg_attr(not(newport_remote_apps), allow(dead_code))]
    pub settings: api::StreamingSettings,
    #[cfg_attr(not(newport_remote_apps), allow(dead_code))]
    pub clipboard: bool,
}

struct Session {
    id: api::SessionId,
    server: String,
    app_id: String,
    app_name: String,
    /// Windows are showing.
    running: bool,
    /// Set once the session has finished, with its failure if any.
    ended: Option<Option<String>>,
    /// Quit was chosen before the windows appeared.
    cancelled: bool,
    #[cfg(newport_remote_apps)]
    native: Option<newport_remote_provider_gpui::native_session::NativeSession>,
}

pub(crate) struct Sessions {
    sessions: Rc<RefCell<Vec<Session>>>,
    next: Cell<u64>,
    /// Set when session state changes off the page's update path.
    changed: Arc<AtomicBool>,
    /// Record sessions without streaming, for tests.
    scripted: bool,
    #[cfg(newport_remote_apps)]
    registry: RefCell<Option<newport_remote_provider_gpui::session_registry::SessionRegistry>>,
}

impl Sessions {
    pub fn new() -> Self {
        Self {
            sessions: Default::default(),
            next: Cell::new(0),
            changed: Default::default(),
            scripted: false,
            #[cfg(newport_remote_apps)]
            registry: Default::default(),
        }
    }

    /// Sessions that start immediately and stream nothing.
    #[cfg(test)]
    pub fn scripted() -> Self {
        Self {
            scripted: true,
            ..Self::new()
        }
    }

    /// A session whose windows haven't appeared yet.
    #[cfg(test)]
    pub fn record_starting(&self, server: &str, app_id: &str) {
        let id = self.record(server, app_id.into(), app_id.into());
        if let Some(session) = self.sessions.borrow_mut().iter_mut().find(|s| s.id == id) {
            session.running = false;
        }
    }

    /// Whether this build can stream at all.
    pub fn available(&self) -> bool {
        self.scripted || cfg!(newport_remote_apps)
    }

    /// True once after session state changed.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    pub fn launch(
        &self,
        launch: Launch,
        request: api::LaunchRequest,
        cx: &mut App,
    ) -> Result<api::SessionId, String> {
        let arguments = launch_arguments(&request)?;
        self.start(launch, request.app_id, request.name, arguments, cx)
    }

    pub fn run(
        &self,
        launch: Launch,
        request: api::CommandRequest,
        cx: &mut App,
    ) -> Result<api::SessionId, String> {
        let arguments = run_arguments(&request, launch.x11)?;
        self.start(launch, request.id, request.name, arguments, cx)
    }

    /// Sessions whose windows are open or opening, on any server.
    pub fn active(&self) -> usize {
        self.sessions
            .borrow()
            .iter()
            .filter(|session| session.ended.is_none() && !session.cancelled)
            .count()
    }

    /// Stop showing a session that ended, once its alert is dismissed.
    pub fn forget(&self, id: api::SessionId) {
        self.sessions
            .borrow_mut()
            .retain(|session| session.id != id || session.ended.is_none());
        self.changed.store(true, Ordering::Release);
    }

    pub fn list(&self) -> Vec<api::SessionSummary> {
        let mut sessions = self.sessions.borrow_mut();
        // Sessions that ended cleanly leave nothing to show.
        sessions.retain(|session| !matches!(session.ended, Some(None)));
        sessions
            .iter()
            .map(|session| api::SessionSummary {
                id: session.id,
                server: session.server.clone(),
                app_id: session.app_id.clone(),
                app_name: session.app_name.clone(),
                state: match &session.ended {
                    Some(error) => api::SessionState::Ended {
                        error: error.clone(),
                    },
                    None if session.running => api::SessionState::Running,
                    None => api::SessionState::Starting,
                },
            })
            .collect()
    }

    pub fn stop(&self, id: api::SessionId) {
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.iter_mut().find(|s| s.id == id) else {
            return;
        };
        if self.scripted {
            session.ended = Some(None);
            self.changed.store(true, Ordering::Release);
            return;
        }
        #[cfg(newport_remote_apps)]
        if let Some(native) = session.native.clone() {
            native.shutdown();
            return;
        }
        // Still connecting: the start finishes by closing instead of showing.
        session.cancelled = true;
        session.ended = Some(None);
        self.changed.store(true, Ordering::Release);
    }

    /// Stop every session before the application quits.
    pub fn shutdown(&self, _cx: &mut App) {
        #[cfg(newport_remote_apps)]
        if let Some(registry) = self.registry.borrow().clone() {
            registry.begin_shutdown(_cx);
            // The process is exiting: don't leave remote commands running.
            registry.terminate_all(_cx);
        }
    }

    fn record(&self, server: &str, app_id: String, app_name: String) -> api::SessionId {
        let id = api::SessionId(self.next.get() + 1);
        self.next.set(id.0);
        self.sessions.borrow_mut().push(Session {
            id,
            server: server.to_owned(),
            app_id,
            app_name,
            running: self.scripted,
            ended: None,
            cancelled: false,
            #[cfg(newport_remote_apps)]
            native: None,
        });
        self.changed.store(true, Ordering::Release);
        id
    }

    fn start(
        &self,
        launch: Launch,
        app_id: String,
        app_name: String,
        arguments: String,
        cx: &mut App,
    ) -> Result<api::SessionId, String> {
        if launch.agent.is_empty() {
            return Err("Install Newport Remote on this server first.".into());
        }
        if self.scripted {
            return Ok(self.record(launch.server, app_id, app_name));
        }
        let command = format!("{} {arguments}", prefix(launch.agent)?);
        self.stream(launch, app_id, app_name, command, cx)
    }

    #[cfg(not(newport_remote_apps))]
    fn stream(
        &self,
        _: Launch,
        _: String,
        _: String,
        _: String,
        _: &mut App,
    ) -> Result<api::SessionId, String> {
        Err("Remote apps aren’t part of this build.".into())
    }

    /// Open the command's stream off the UI thread, then present its windows.
    #[cfg(newport_remote_apps)]
    fn stream(
        &self,
        launch: Launch,
        app_id: String,
        app_name: String,
        command: String,
        cx: &mut App,
    ) -> Result<api::SessionId, String> {
        use newport_remote_client as client;
        use newport_remote_protocol::performance::CodecMode;
        use newport_remote_provider_gpui as provider;
        let streaming = launch.settings;
        let settings = newport_remote_config::SessionSettings {
            audio: streaming.audio,
            av_sync: streaming.audio,
            max_fps: streaming.fps_goal,
            codec_mode: match streaming.codec {
                api::CodecPreference::Auto => CodecMode::Auto,
                api::CodecPreference::Lossless => CodecMode::Lossless,
                api::CodecPreference::Fast => CodecMode::Fast,
            },
            clipboard: launch.clipboard,
            ime: true,
            ..Default::default()
        };
        let settings = provider::settings::local_support()
            .effective(&settings)?
            .settings;
        // Read on the UI thread, as AppKit requires.
        let display = provider::platform::launch_display_metrics()
            .map(|(scale, area)| {
                newport_remote_protocol::display::DisplayProfile::from_metrics(scale, area)
            })
            .transpose()?;
        let registry = self
            .registry
            .borrow_mut()
            .get_or_insert_with(|| provider::session_registry::SessionRegistry::new(cx))
            .clone();
        registry.ensure_accepting(cx)?;
        let id = self.record(launch.server, app_id, app_name.clone());
        let sessions = self.sessions.clone();
        let changed = self.changed.clone();
        let identity = provider::session_registry::Identity {
            application: app_name,
            server: launch.server_name.to_owned(),
        };
        let server = launch.server.to_owned();
        let hud = streaming.hud;
        cx.spawn(async move |cx| {
            // A blocking exchange of up to 45 s: keep it off GPUI's executors.
            let (sender, receiver) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = sender.send_blocking(open_stream(&server, &command));
            });
            let opened = receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("The connection attempt stopped.".into()));
            let watched = sessions.clone();
            let cancelled = move || {
                watched
                    .borrow()
                    .iter()
                    .find(|s| s.id == id)
                    .is_none_or(|s| s.cancelled)
            };
            let update = move |change: &dyn Fn(&mut Session)| {
                if let Some(session) = sessions.borrow_mut().iter_mut().find(|s| s.id == id) {
                    change(session);
                }
                changed.store(true, Ordering::Release);
            };
            let fail = |error: String| update(&|session| session.ended = Some(Some(error.clone())));
            let (reader, writer, close, outcome) = match opened {
                Ok(stream) => stream,
                Err(error) => return fail(error),
            };
            if cancelled() {
                close();
                return;
            }
            let channel = newport_remote_transport::Channel::new(
                reader,
                writer,
                StreamDriver { close, outcome },
            );
            let attached = client::RemoteSession::attach(
                channel,
                client::SessionOptions {
                    settings,
                    display,
                    native_sizing: true,
                    ..Default::default()
                },
            );
            let connection = match attached {
                Ok(connection) => connection,
                Err(error) => return fail(error),
            };
            if cancelled() {
                connection.control().terminate();
                return;
            }
            let finished = update.clone();
            let presented = cx.update(|cx| {
                provider::native_session::present(
                    connection,
                    provider::native_session::Options {
                        presentation: provider::settings::PresentationSettings {
                            hud,
                            trace: false,
                        },
                        identity,
                        quit_on_end: false,
                        on_finished: Some(Rc::new(move |control, _| {
                            let error = match control.state() {
                                client::SessionState::Finished(client::SessionOutcome::Failed(
                                    error,
                                )) => Some(error),
                                _ => None,
                            };
                            finished(&|session| session.ended = Some(error.clone()));
                        })),
                        registry: Some(registry),
                    },
                    cx,
                )
            });
            match presented {
                Ok(native) => update(&|session| {
                    session.running = native.is_running();
                    session.native = Some(native.clone());
                }),
                Err(error) => fail(error),
            }
        })
        .detach();
        Ok(id)
    }

    /// Mark sessions whose windows appeared since the last look.
    pub fn poll(&self) {
        #[cfg(newport_remote_apps)]
        for session in self.sessions.borrow_mut().iter_mut() {
            let running = session.native.as_ref().is_some_and(|n| n.is_running());
            if session.ended.is_none() && running != session.running {
                session.running = running;
                self.changed.store(true, Ordering::Release);
            }
        }
    }
}

/// The agent arguments that open a catalogue application.
fn launch_arguments(request: &api::LaunchRequest) -> Result<String, String> {
    let mut arguments = format!("--display private --app {}", quote(&request.app_id)?);
    if let Some(action) = &request.action {
        arguments.push_str(&format!(" --action {}", quote(action)?));
    }
    Ok(arguments)
}

/// The agent arguments that run a program directly. "Automatic" uses X11
/// when the server has it, and Wayland otherwise.
fn run_arguments(request: &api::CommandRequest, x11: bool) -> Result<String, String> {
    if request.argv.is_empty() {
        return Err("Enter a program to run.".into());
    }
    let display = match request.display {
        api::DisplayBackend::Wayland => "wayland",
        api::DisplayBackend::X11 => "private",
        api::DisplayBackend::Auto if x11 => "private",
        api::DisplayBackend::Auto => "wayland",
    };
    // The agent takes no environment options; `env` sets them for the program.
    let mut argv = Vec::new();
    if !request.env.is_empty() {
        argv.push("env".to_owned());
        argv.extend(
            request
                .env
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
    }
    argv.extend(request.argv.iter().cloned());
    let words = argv
        .iter()
        .map(|word| quote(word))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("--display {display} -- {}", words.join(" ")))
}

/// `exec /path/run-agent`, the start of every agent command.
fn prefix(agent: &str) -> Result<String, String> {
    Ok(format!("exec {}", quote(agent)?))
}

/// Quote a word for the remote shell.
fn quote(word: &str) -> Result<String, String> {
    shlex::try_quote(word)
        .map(|quoted| quoted.into_owned())
        .map_err(|_| "Commands can’t contain NUL characters.".to_owned())
}

/// A command's stdout and stdin on the server, a way to close both, and how
/// the command ended once it has.
#[cfg(newport_remote_apps)]
pub(crate) type Stream = (
    Box<dyn std::io::Read + Send>,
    Box<dyn std::io::Write + Send>,
    Arc<dyn Fn() + Send + Sync>,
    Arc<std::sync::Mutex<Option<Outcome>>>,
);

/// Start a command over Newport's connection to the server.
#[cfg(all(newport_remote_apps, unix))]
pub(crate) fn open_stream(server: &str, command: &str) -> Result<Stream, String> {
    let stream = crate::backend::Backend::discover().open_remote_stream(server, command)?;
    let reader = stream.try_clone().map_err(|error| error.to_string())?;
    let closer = stream.try_clone().map_err(|error| error.to_string())?;
    let outcome = Arc::new(std::sync::Mutex::new(None));
    Ok((
        Box::new(Frames::new(reader, outcome.clone())),
        Box::new(HalfClose(stream)),
        Arc::new(move || {
            let _ = closer.shutdown(std::net::Shutdown::Both);
        }),
        outcome,
    ))
}

#[cfg(all(newport_remote_apps, not(unix)))]
pub(crate) fn open_stream(_: &str, _: &str) -> Result<Stream, String> {
    Err("Remote apps aren’t available on this platform yet.".into())
}

/// Dropping the writer ends the command's input without closing its output.
#[cfg(all(newport_remote_apps, unix))]
struct HalfClose(std::os::unix::net::UnixStream);
#[cfg(all(newport_remote_apps, unix))]
impl std::io::Write for HalfClose {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
#[cfg(all(newport_remote_apps, unix))]
impl Drop for HalfClose {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Write);
    }
}

/// How a remote command ended, from the stream's final frame.
#[derive(Debug, Default, PartialEq, serde::Deserialize)]
pub(crate) struct Outcome {
    status: Option<u32>,
    signal: Option<String>,
    #[serde(default)]
    pub stderr: String,
    /// Why the command ended without exiting: refused, or connection lost.
    #[serde(default)]
    error: Option<String>,
}

impl Outcome {
    /// A failure message, with the command's last error lines when it wrote any.
    #[cfg_attr(not(newport_remote_apps), allow(dead_code))]
    pub fn failure(&self) -> Option<String> {
        if let Some(error) = &self.error {
            return Some(error.clone());
        }
        let reason = match (self.status, &self.signal) {
            (Some(0), _) => return None,
            (_, Some(signal)) => format!("The agent was stopped by {signal}."),
            (Some(status), None) => format!("The agent exited with status {status}."),
            (None, None) => return None,
        };
        let detail: Vec<_> = self
            .stderr
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        Some(match detail.len() {
            0 => reason,
            len => detail[len.saturating_sub(3)..].join("\n"),
        })
    }
}

/// Reads stdout out of the stream's frames and keeps the final outcome.
struct Frames<R> {
    inner: R,
    remaining: usize,
    outcome: Arc<std::sync::Mutex<Option<Outcome>>>,
    done: bool,
}

impl<R: std::io::Read> Frames<R> {
    #[cfg_attr(not(newport_remote_apps), allow(dead_code))]
    fn new(inner: R, outcome: Arc<std::sync::Mutex<Option<Outcome>>>) -> Self {
        Self {
            inner,
            remaining: 0,
            outcome,
            done: false,
        }
    }
}

impl<R: std::io::Read> std::io::Read for Frames<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        use newport_platform::ipc::protocol::{REMOTE_STREAM_DATA, REMOTE_STREAM_EXIT};
        while self.remaining == 0 {
            if self.done {
                return Ok(0);
            }
            let mut header = [0; 5];
            match self.inner.read_exact(&mut header) {
                Ok(()) => {}
                // A stream cut between frames ends without an outcome.
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.done = true;
                    return Ok(0);
                }
                Err(error) => return Err(error),
            }
            let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
            match header[0] {
                REMOTE_STREAM_DATA => self.remaining = len,
                REMOTE_STREAM_EXIT if len <= 64 * 1024 => {
                    let mut payload = vec![0; len];
                    self.inner.read_exact(&mut payload)?;
                    *self.outcome.lock().unwrap_or_else(|e| e.into_inner()) =
                        serde_json::from_slice(&payload).ok();
                    self.done = true;
                }
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Invalid remote stream frame",
                    ));
                }
            }
        }
        let limit = buffer.len().min(self.remaining);
        let read = self.inner.read(&mut buffer[..limit])?;
        if read == 0 {
            self.done = true;
        }
        self.remaining -= read;
        Ok(read)
    }
}

/// Closing the stream ends the remote command and unblocks both halves.
#[cfg(newport_remote_apps)]
struct StreamDriver {
    close: Arc<dyn Fn() + Send + Sync>,
    outcome: Arc<std::sync::Mutex<Option<Outcome>>>,
}
#[cfg(newport_remote_apps)]
impl newport_remote_transport::Driver for StreamDriver {
    fn cancel(&self) {
        (self.close)();
    }
    fn finish(&self) -> Result<(), String> {
        match self
            .outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(Outcome::failure)
        {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_commands_quote_every_word() {
        assert_eq!(quote("it's").unwrap(), r#""it's""#);
        assert_eq!(prefix("/a b/run-agent").unwrap(), "exec '/a b/run-agent'");
        assert!(quote("a\0b").is_err());
        let request = api::LaunchRequest {
            app_id: "google-chrome.desktop".into(),
            name: "Google Chrome".into(),
            action: Some("new-private-window".into()),
        };
        assert_eq!(
            launch_arguments(&request).unwrap(),
            "--display private --app google-chrome.desktop --action new-private-window"
        );
        let request = api::CommandRequest {
            id: "command:1".into(),
            name: "viewer".into(),
            argv: vec!["viewer".into(), "a file".into()],
            env: vec![("GDK_SCALE".into(), "2".into())],
            display: api::DisplayBackend::Wayland,
        };
        assert_eq!(
            run_arguments(&request, true).unwrap(),
            "--display wayland -- env 'GDK_SCALE=2' viewer 'a file'"
        );
        let automatic = api::CommandRequest {
            display: api::DisplayBackend::Auto,
            ..request
        };
        assert!(
            run_arguments(&automatic, true)
                .unwrap()
                .starts_with("--display private ")
        );
        assert!(
            run_arguments(&automatic, false)
                .unwrap()
                .starts_with("--display wayland ")
        );
    }

    #[test]
    fn frames_yield_stdout_and_keep_the_outcome() {
        use std::io::Read;
        let mut wire = Vec::new();
        for (kind, payload) in [
            (0u8, b"hel".to_vec()),
            (0, b"lo".to_vec()),
            (
                1,
                br#"{"status":127,"signal":null,"stderr":"exec: run-agent: not found\n"}"#.to_vec(),
            ),
        ] {
            wire.push(kind);
            wire.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            wire.extend_from_slice(&payload);
        }
        let outcome = Arc::new(std::sync::Mutex::new(None));
        let mut frames = Frames::new(&wire[..], outcome.clone());
        let mut stdout = String::new();
        frames.read_to_string(&mut stdout).unwrap();
        assert_eq!(stdout, "hello");
        let outcome = outcome.lock().unwrap().take().unwrap();
        assert_eq!(outcome.failure().unwrap(), "exec: run-agent: not found");
        let clean = Outcome {
            status: Some(0),
            ..Outcome::default()
        };
        assert_eq!(clean.failure(), None);
        // A lost connection is a failure, not a clean exit.
        let lost: Outcome = serde_json::from_str(
            r#"{"status":null,"signal":null,"stderr":"","error":"The connection to the server was lost."}"#,
        )
        .unwrap();
        assert_eq!(
            lost.failure().as_deref(),
            Some("The connection to the server was lost.")
        );
    }

    #[test]
    fn quitting_a_starting_session_ends_it() {
        let sessions = Sessions::new();
        let id = sessions.record("server", "app".into(), "App".into());
        assert_eq!(sessions.active(), 1);
        sessions.stop(id);
        assert_eq!(sessions.active(), 0);
        assert!(sessions.list().is_empty());
    }
}
