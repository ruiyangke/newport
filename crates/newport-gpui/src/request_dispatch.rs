//! Bounded worker capacity reserved for revocation. Slow ordinary RPCs and
//! preview rendering cannot occupy these workers or their request queue.
use super::{Completion, Work, backend, file_image, file_pdf};
use serde_json::json;
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

const ORDINARY_WORKERS: usize = 8;
const REVOCATION_WORKERS: usize = 2;
const ORDINARY_QUEUE: usize = 32;
const REVOCATION_QUEUE: usize = 8;

pub(super) struct Dispatcher {
    ordinary: mpsc::SyncSender<Work>,
    revocation: mpsc::SyncSender<Work>,
}
impl Dispatcher {
    pub fn try_send(&self, request: Work) -> Result<(), mpsc::TrySendError<()>> {
        let result = if newport_platform::ipc::protocol::is_integration_revocation(
            &request.method,
            &request.args,
        ) {
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
                    let request = incoming.lock().unwrap().recv();
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

fn execute(mut request: Work) -> Completion {
    let backend = backend::Backend::discover();
    let server = request.args["serverId"]
        .as_str()
        .or_else(|| request.args["id"].as_str())
        .or_else(|| request.args["server"]["id"].as_str())
        .or_else(|| request.args["expectedConnection"]["id"].as_str())
        .unwrap_or_default()
        .to_owned();
    let mut pdf = request.pdf;
    let mut pdf_text = None;
    let mut result = if request.method == "native.prepare_preview" {
        Ok(request.args["data"].take())
    } else if request.method == "native.pdf_page" {
        Ok(json!({"kind":"pdf", "page":request.args["page"]}))
    } else if request.method == "native.add_project" {
        backend.add_project(request.args).map_err(Into::into)
    } else if request.method == "native.repository_settings" {
        backend.repository_settings(&server, request.args["repoId"].as_str().unwrap_or_default())
    } else if request.method == "native.git_logs" {
        backend
            .call_with_timeout(
                "$events",
                json!({"after":request.args["after"]}),
                Duration::from_secs(5),
            )
            .and_then(|value| {
                newport_gpui::git_log_text::batch_at(
                    value,
                    &server,
                    request.args["repoId"].as_str().unwrap_or_default(),
                    request.args["columns"]
                        .as_u64()
                        .unwrap_or(160)
                        .clamp(2, 500) as u16,
                )
            })
            .map_err(Into::into)
    } else {
        backend.call_detailed(&request.method, request.args)
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
    use std::sync::Condvar;

    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    fn request(method: &str, args: serde_json::Value) -> Work {
        Work {
            pdf: None,
            key: method.into(),
            generation: 1,
            method: method.into(),
            args,
        }
    }

    #[test]
    fn completion_retains_server_from_reviewed_connection() {
        // Exercise the worker completion path without contacting a real backend.
        // Lifecycle commands carry their server exclusively in expectedConnection.
        let mut work = request(
            "native.prepare_preview",
            json!({
                "expectedConnection":{"id":"reviewed-server"},
                "data":"Restored"
            }),
        );
        work.key = "integration:helper-restore:reviewed-server".into();
        let result = execute(work);
        assert_eq!(result.server, "reviewed-server");
        assert_eq!(result.key, "integration:helper-restore:reviewed-server");
        assert_eq!(result.result.unwrap(), json!("Restored"));
    }

    #[test]
    fn integration_revocation_workers_and_queue_survive_ordinary_saturation() {
        for block_revocations in [false, true] {
            let release = Release(Arc::new((Mutex::new(false), Condvar::new())));
            let blocked = release.0.clone();
            let (started, entered) = mpsc::channel();
            let (work, replies) = start_with(move |request| {
                if request.args["hold"] == true {
                    let _ = started.send(());
                    let guard = blocked.0.lock().unwrap();
                    drop(blocked.1.wait_while(guard, |released| !*released).unwrap());
                }
                Completion {
                    pdf: None,
                    pdf_text: None,
                    image: None,
                    server: String::new(),
                    key: request.key,
                    generation: request.generation,
                    result: Ok(request.args),
                }
            });
            let (method, mut args, workers, queue) = if block_revocations {
                (
                    "set_integration_enabled",
                    json!({"feature":"browser","enabled":false}),
                    REVOCATION_WORKERS,
                    REVOCATION_QUEUE,
                )
            } else {
                ("fixture.slow", json!({}), ORDINARY_WORKERS, ORDINARY_QUEUE)
            };
            args["hold"] = json!(true);
            for _ in 0..workers {
                assert!(work.try_send(request(method, args.clone())).is_ok());
            }
            for _ in 0..workers {
                entered.recv_timeout(Duration::from_secs(3)).unwrap();
            }
            for _ in 0..queue {
                assert!(work.try_send(request(method, args.clone())).is_ok());
            }
            assert!(matches!(
                work.try_send(request(method, args)),
                Err(mpsc::TrySendError::Full(_))
            ));
            if block_revocations {
                // Off requests cannot consume ordinary capacity or deadlock all work.
                assert!(
                    work.try_send(request(
                        "set_integration_enabled",
                        json!({"feature":"browser","enabled":true})
                    ))
                    .is_ok()
                );
                assert_eq!(
                    replies
                        .recv_timeout(Duration::from_secs(3))
                        .unwrap()
                        .result
                        .unwrap()["enabled"],
                    true
                );
            } else {
                for (method, feature) in [
                    ("set_integration_enabled", "browser"),
                    ("set_clipboard_enabled", "clipboard"),
                ] {
                    assert!(
                        work.try_send(request(method, json!({"feature":feature,"enabled":false})))
                            .is_ok()
                    );
                    assert_eq!(
                        replies
                            .recv_timeout(Duration::from_secs(3))
                            .unwrap()
                            .result
                            .unwrap()["enabled"],
                        false
                    );
                }
                for args in [
                    json!({"feature":"browser","enabled":true}),
                    json!({"feature":"unknown","enabled":false}),
                ] {
                    assert!(matches!(
                        work.try_send(request("set_integration_enabled", args)),
                        Err(mpsc::TrySendError::Full(_))
                    ));
                }
            }
            drop(work);
            drop(release);
        }
    }
}
