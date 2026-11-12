//! Bounded worker capacity reserved for revocation. Slow ordinary RPCs and
//! preview rendering cannot occupy these workers or their request queue.
use super::{Completion, Work, backend, file_image, file_pdf};
use newport_protocol::desktop;
use serde_json::Value;
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
    Desktop(Box<desktop::Operation>),
    Discovery {
        server: uuid::Uuid,
        scope: Value,
        refresh: bool,
        previous: Option<newport_client::discovery::Reading>,
    },
    PreparePreview {
        server: Option<uuid::Uuid>,
        data: Value,
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
    fn server(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Desktop(operation) => operation.server_id(),
            Self::PreparePreview { server, .. } => *server,
            Self::AddProject { server, .. }
            | Self::Discovery { server, .. }
            | Self::RepositorySettings { server, .. }
            | Self::GitLogs { server, .. } => Some(*server),
        }
    }
    fn revokes_integration(&self) -> bool {
        match self {
            Self::Desktop(operation) => match operation.as_ref() {
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
    let mut pdf = request.pdf;
    let mut pdf_text = None;
    let mut discovery = None;
    let mut result = match request.request {
        Request::Discovery {
            server,
            scope,
            refresh,
            previous,
        } => {
            discovery = Some(crate::connections::DiscoveryCompletion {
                scope,
                reading: newport_client::discovery::Reading::prepare(
                    backend.discover_ports(server, refresh),
                    previous.as_ref(),
                ),
            });
            Ok(Value::Null)
        }
        Request::PreparePreview { data, .. } => Ok(data),
        Request::AddProject { server, name, path } => {
            backend.add_project(server, &name, path).map_err(Into::into)
        }
        Request::RepositorySettings { server, repo_id } => {
            backend.repository_settings(&server.to_string(), &repo_id)
        }
        Request::GitLogs {
            server,
            repo_id,
            after,
            columns,
        } => backend
            .execute_with_timeout(
                desktop::Operation::Events(Box::new(desktop::events::Operation::Poll(Box::new(
                    desktop::events::PollArgs { after },
                )))),
                Duration::from_secs(5),
            )
            .map_err(Into::into)
            .and_then(newport_client::desktop_result::project)
            .and_then(|value| {
                crate::git_log_text::batch_at(value, &server.to_string(), &repo_id, columns)
                    .map_err(Into::into)
            }),
        Request::Desktop(operation) => backend.execute_rendered(*operation),
    };
    let image =
        if request.key == "preview" && result.as_ref().is_ok_and(|data| data["kind"] == "pdf") {
            let data = result.as_ref().unwrap();
            let bytes = pdf
                .clone()
                .map(Ok)
                .unwrap_or_else(|| file_pdf::decode(data["content"].as_str().unwrap_or_default()));
            match bytes.and_then(|bytes| {
                file_pdf::render(bytes, data["page"].as_u64().unwrap_or(1) as usize)
            }) {
                Ok(page) => {
                    result = Ok(page.data);
                    pdf = Some(page.bytes);
                    pdf_text = Some(page.text);
                    Some(page.image)
                }
                Err(error) => {
                    result = Err(error.into());
                    None
                }
            }
        } else if request.key == "preview" {
            match result.as_mut().ok().map(file_image::prepare).transpose() {
                Ok(image) => image.flatten(),
                Err(error) => {
                    result = Err(error.into());
                    None
                }
            }
        } else {
            None
        };
    Completion {
        discovery,
        pdf,
        pdf_text,
        image,
        server,
        key: request.key,
        generation: request.generation,
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
            pdf: None,
            key: if hold { "hold" } else { "complete" }.into(),
            generation: 1,
            request,
        }
    }
    fn ordinary() -> Request {
        Request::PreparePreview {
            server: None,
            data: Value::Null,
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
        Request::Desktop(Box::new(desktop::Operation::Configuration(Box::new(
            operation,
        ))))
    }
    #[test]
    fn completion_retains_captured_server_identity() {
        let server = uuid::Uuid::new_v4();
        let mut work = work(
            Request::PreparePreview {
                server: Some(server),
                data: json!("Restored"),
            },
            false,
        );
        work.key = format!("integration:helper-restore:{server}");
        let result = execute(work);
        assert_eq!(result.server, server.to_string());
        assert_eq!(result.key, format!("integration:helper-restore:{server}"));
        assert_eq!(result.result.unwrap(), json!("Restored"));
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
                    discovery: None,
                    pdf: None,
                    pdf_text: None,
                    image: None,
                    server: String::new(),
                    key: request.key,
                    generation: request.generation,
                    result: Ok(json!({"revoked":revoked})),
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
                assert_eq!(
                    replies
                        .recv_timeout(Duration::from_secs(3))
                        .unwrap()
                        .result
                        .unwrap()["revoked"],
                    false
                );
            } else {
                for clipboard in [false, true] {
                    assert!(
                        dispatch
                            .try_send(work(integration(false, clipboard), false))
                            .is_ok()
                    );
                    assert_eq!(
                        replies
                            .recv_timeout(Duration::from_secs(3))
                            .unwrap()
                            .result
                            .unwrap()["revoked"],
                        true
                    );
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
