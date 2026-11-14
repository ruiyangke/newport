//! Bounded worker capacity reserved for revocation. Slow ordinary RPCs and
//! preview rendering cannot occupy these workers or their request queue.
use super::{Completion, Outcome, Work, backend, file_image};
use newport_protocol::desktop;
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

const ORDINARY_WORKERS: usize = 8;
const REVOCATION_WORKERS: usize = 2;
const ORDINARY_QUEUE: usize = 32;
const REVOCATION_QUEUE: usize = 8;

/// Local presentation work is explicit and never encoded as a desktop request.
#[derive(Debug)]
pub(super) enum Request {
    /// A desktop operation whose decoded reply goes back as is.
    Typed(Box<desktop::Operation>),
    /// A Git read whose checked reply goes back as is.
    Git(Box<desktop::git::RequestArgs>),
    Discovery {
        server: uuid::Uuid,
        scope: desktop::configuration::ConnectionScope,
        refresh: bool,
        previous: Option<newport_client::discovery::Reading>,
    },
    /// Decodes a file preview's image off the UI thread.
    PreparePreview {
        server: Option<uuid::Uuid>,
        preview: Box<desktop::files::Preview>,
    },
    AddProject {
        server: uuid::Uuid,
        name: String,
        path: newport_protocol::git::Path,
    },
    RepositorySettings {
        server: uuid::Uuid,
        repo_id: String,
    },
    GitLogs {
        server: uuid::Uuid,
        repo_id: String,
        after: u64,
        columns: u16,
    },
}
impl Request {
    /// The desktop operation this request sends.
    #[cfg(test)]
    pub(super) fn operation(&self) -> Option<&desktop::Operation> {
        match self {
            Self::Typed(operation) => Some(operation),
            _ => None,
        }
    }
    fn server(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Typed(operation) => operation.server_id(),
            Self::Git(args) => Some(args.server),
            Self::PreparePreview { server, .. } => *server,
            Self::AddProject { server, .. }
            | Self::Discovery { server, .. }
            | Self::RepositorySettings { server, .. }
            | Self::GitLogs { server, .. } => Some(*server),
        }
    }
    fn revokes_integration(&self) -> bool {
        match self {
            Self::Typed(operation) => match operation.as_ref() {
                desktop::Operation::Configuration(operation) => match operation.as_ref() {
                    desktop::configuration::Operation::SetClipboardEnabled(args) => !args.enabled,
                    desktop::configuration::Operation::SetIntegrationEnabled(args) => !args.enabled,
                    _ => false,
                },
                _ => false,
            },
            _ => false,
        }
    }
}

pub(super) struct Dispatcher {
    ordinary: mpsc::SyncSender<Work>,
    revocation: mpsc::SyncSender<Work>,
}
impl Dispatcher {
    pub fn try_send(&self, request: Work) -> Result<(), mpsc::TrySendError<()>> {
        let result = if request.request.revokes_integration() {
            self.revocation.try_send(request)
        } else {
            self.ordinary.try_send(request)
        };
        result.map_err(|error| match error {
            mpsc::TrySendError::Full(_) => mpsc::TrySendError::Full(()),
            mpsc::TrySendError::Disconnected(_) => mpsc::TrySendError::Disconnected(()),
        })
    }
}

// Offline view fixtures intercept all requests in one observation channel.
#[cfg(test)]
impl From<mpsc::SyncSender<Work>> for Dispatcher {
    fn from(sender: mpsc::SyncSender<Work>) -> Self {
        Self {
            ordinary: sender.clone(),
            revocation: sender,
        }
    }
}

pub(super) fn start() -> (Dispatcher, mpsc::Receiver<Completion>) {
    start_with(execute)
}
fn start_with(
    execute: impl Fn(Work) -> Completion + Send + Sync + 'static,
) -> (Dispatcher, mpsc::Receiver<Completion>) {
    let (ordinary, incoming) = mpsc::sync_channel(ORDINARY_QUEUE);
    let (revocation, revocations) = mpsc::sync_channel(REVOCATION_QUEUE);
    let (outgoing, replies) = mpsc::channel();
    let execute = Arc::new(execute);
    for (count, incoming) in [
        (ORDINARY_WORKERS, incoming),
        (REVOCATION_WORKERS, revocations),
    ] {
        let incoming = Arc::new(Mutex::new(incoming));
        for _ in 0..count {
            let incoming = incoming.clone();
            let outgoing = outgoing.clone();
            let execute = execute.clone();
            std::thread::spawn(move || {
                loop {
                    let request = incoming
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .recv();
                    let Ok(request) = request else {
                        break;
                    };
                    if outgoing.send(execute(request)).is_err() {
                        break;
                    }
                }
            });
        }
    }
    (
        Dispatcher {
            ordinary,
            revocation,
        },
        replies,
    )
}

fn execute(request: Work) -> Completion {
    let backend = backend::Backend::discover();
    let server = request
        .request
        .server()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let result = match request.request {
        Request::Typed(operation) => {
            Outcome::Typed(backend.execute(*operation).map_err(Into::into))
        }
        Request::Git(args) => {
            let args = *args;
            Outcome::Git(backend.git(args.server, args.request, args.retry_connection))
        }
        Request::Discovery {
            server,
            scope,
            refresh,
            previous,
        } => Outcome::Discovery(crate::connections::DiscoveryCompletion {
            scope,
            reading: newport_client::discovery::Reading::prepare(
                backend.discover_ports(server, refresh),
                previous.as_ref(),
            ),
        }),
        Request::PreparePreview { preview, .. } => {
            Outcome::Preview(file_image::prepare(*preview).map_err(Into::into))
        }
        Request::AddProject { server, name, path } => {
            Outcome::Project(backend.add_project(server, &name, path).map_err(Into::into))
        }
        Request::RepositorySettings { server, repo_id } => {
            Outcome::Git(backend.repository_settings(&server.to_string(), &repo_id))
        }
        Request::GitLogs {
            server: scope,
            repo_id,
            after,
            columns,
        } => Outcome::GitLog(
            backend
                .execute_with_timeout(
                    desktop::Operation::Events(Box::new(desktop::events::Operation::Poll(
                        Box::new(desktop::events::PollArgs { after }),
                    ))),
                    Duration::from_secs(5),
                )
                .map_err(Into::into)
                .and_then(newport_client::backend::events_reply)
                .and_then(|batch| {
                    crate::git_log_text::batch_at(batch, scope, &repo_id, columns)
                        .map_err(Into::into)
                }),
        ),
    };
    Completion {
        server,
        key: request.key,
        generation: request.generation,
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Condvar;
    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    fn work(request: Request, hold: bool) -> Work {
        Work {
            key: if hold { "hold" } else { "complete" }.into(),
            generation: 1,
            request,
        }
    }
    fn text_preview() -> desktop::files::Preview {
        desktop::files::Preview {
            kind: desktop::files::PreviewKind::Text,
            mime: "text/plain".into(),
            content: desktop::files::Content::Text(Box::new("Restored".into())),
            truncated: false,
        }
    }
    fn ordinary() -> Request {
        Request::PreparePreview {
            server: None,
            preview: Box::new(text_preview()),
        }
    }
    fn integration(enabled: bool, clipboard: bool) -> Request {
        use desktop::configuration::*;
        let server = uuid::Uuid::new_v4();
        let expected = ConnectionScope {
            settings: ConnectionSettings {
                server_id: server,
                ssh_host: "host".into(),
                ssh_user: "user".into(),
                ssh_port: 22,
                auth_method: AuthMethod::PublicKey,
                identity_file: None,
                agent_source: None,
                agent_key_fingerprint: None,
                proxy: None,
            },
            connection_revision: 1,
            backend_instance: uuid::Uuid::new_v4(),
        };
        let operation = if clipboard {
            Operation::SetClipboardEnabled(Box::new(SetClipboardEnabledArgs {
                server,
                enabled,
                expected,
            }))
        } else {
            Operation::SetIntegrationEnabled(Box::new(SetIntegrationEnabledArgs {
                server,
                enabled,
                expected,
                feature: Feature::Browser,
            }))
        };
        Request::Typed(Box::new(desktop::Operation::Configuration(Box::new(
            operation,
        ))))
    }
    #[test]
    fn completion_retains_captured_server_identity() {
        let server = uuid::Uuid::new_v4();
        let mut work = work(
            Request::PreparePreview {
                server: Some(server),
                preview: Box::new(text_preview()),
            },
            false,
        );
        work.key = format!("integration:helper-restore:{server}");
        let result = execute(work);
        assert_eq!(result.server, server.to_string());
        assert_eq!(result.key, format!("integration:helper-restore:{server}"));
        assert!(
            matches!(result.result, Outcome::Preview(Ok(prepared)) if prepared.preview == text_preview())
        );
    }
    /// Whether the next completion reports a revocation.
    fn revoked(replies: &mpsc::Receiver<Completion>) -> bool {
        replies.recv_timeout(Duration::from_secs(3)).unwrap().key == "revoked"
    }
    #[test]
    fn integration_revocation_workers_and_queue_survive_ordinary_saturation() {
        for block_revocations in [false, true] {
            let release = Release(Arc::new((Mutex::new(false), Condvar::new())));
            let blocked = release.0.clone();
            let (started, entered) = mpsc::channel();
            let (dispatch, replies) = start_with(move |request| {
                if request.key == "hold" {
                    let _ = started.send(());
                    let guard = blocked.0.lock().unwrap();
                    drop(blocked.1.wait_while(guard, |released| !*released).unwrap());
                }
                let revoked = request.request.revokes_integration();
                Completion {
                    server: String::new(),
                    key: if revoked { "revoked" } else { "kept" }.into(),
                    generation: request.generation,
                    result: Outcome::Typed(Err("Not sent".into())),
                }
            });
            let (workers, queue) = if block_revocations {
                (REVOCATION_WORKERS, REVOCATION_QUEUE)
            } else {
                (ORDINARY_WORKERS, ORDINARY_QUEUE)
            };
            let blocked_request = || {
                if block_revocations {
                    integration(false, false)
                } else {
                    ordinary()
                }
            };
            for _ in 0..workers {
                assert!(dispatch.try_send(work(blocked_request(), true)).is_ok());
            }
            for _ in 0..workers {
                entered.recv_timeout(Duration::from_secs(3)).unwrap();
            }
            for _ in 0..queue {
                assert!(dispatch.try_send(work(blocked_request(), true)).is_ok());
            }
            assert!(matches!(
                dispatch.try_send(work(blocked_request(), true)),
                Err(mpsc::TrySendError::Full(_))
            ));
            if block_revocations {
                assert!(
                    dispatch
                        .try_send(work(integration(true, false), false))
                        .is_ok()
                );
                assert!(!revoked(&replies));
            } else {
                for clipboard in [false, true] {
                    assert!(
                        dispatch
                            .try_send(work(integration(false, clipboard), false))
                            .is_ok()
                    );
                    assert!(revoked(&replies));
                }
                for request in [integration(true, false), ordinary()] {
                    assert!(matches!(
                        dispatch.try_send(work(request, false)),
                        Err(mpsc::TrySendError::Full(_))
                    ));
                }
            }
            drop(dispatch);
            drop(release);
        }
    }
}
