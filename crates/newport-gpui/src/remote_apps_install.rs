//! Remote Apps uses the backend's shared agent installer and progress. The
//! backend owns verification, activation, rollback fencing and update policy.
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub(crate) struct Progress(Arc<Mutex<Upload>>);
#[derive(Debug, Default)]
struct Upload {
    sent: usize,
    total: usize,
    uploaded: bool,
}
impl Progress {
    #[cfg(test)]
    pub(crate) fn at(total: usize, sent: usize) -> Self {
        Self(Arc::new(Mutex::new(Upload {
            total,
            sent,
            uploaded: total > 0 && sent >= total,
        })))
    }
    pub fn fraction(&self) -> f32 {
        let upload = self.0.lock().unwrap();
        if upload.total == 0 {
            0.
        } else {
            (upload.sent as f32 / upload.total as f32).min(1.)
        }
    }
    pub fn uploaded(&self) -> bool {
        self.0.lock().unwrap().uploaded
    }
    #[cfg_attr(not(any(newport_remote_apps, test)), allow(dead_code))]
    fn observe(&self, value: &Value) {
        let mut upload = self.0.lock().unwrap();
        if value["stage"] == "uploading" {
            upload.sent = value["sent"].as_u64().unwrap_or_default() as usize;
            upload.total = value["total"].as_u64().unwrap_or_default() as usize;
            upload.uploaded = upload.total > 0 && upload.sent >= upload.total;
        } else {
            upload.uploaded = matches!(
                value["stage"].as_str(),
                Some("installing" | "verifying" | "activating" | "ready")
            );
        }
    }
}

pub(crate) fn available() -> bool {
    cfg!(newport_remote_apps)
}

#[cfg(newport_remote_apps)]
pub(crate) fn start(
    server: &str,
    status: &Value,
) -> Result<(Progress, async_channel::Receiver<Result<String, String>>), String> {
    if status["bundledVariant"] != "full" {
        return Err("This Newport build does not include the full agent. Update Newport to install Remote Apps.".into());
    }
    let expected = status["expectedConnection"].clone();
    if expected["id"].as_str() != Some(server) {
        return Err("Check the server connection again before installing.".into());
    }
    let progress = Progress::default();
    let observed = progress.clone();
    let server = server.to_owned();
    let (sender, receiver) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let (finished, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let installed = crate::backend::Backend::discover()
                .call(
                    "setup_install",
                    serde_json::json!({"expectedConnection": expected}),
                )
                .map(|_| "Newport agent installed and verified.".to_owned());
            let _ = finished.send(installed);
        });
        let backend = crate::backend::Backend::discover();
        loop {
            match result.recv_timeout(std::time::Duration::from_millis(250)) {
                Ok(result) => {
                    let _ = sender.send_blocking(result);
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = sender.send_blocking(Err("The install stopped unexpectedly.".into()));
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Progress failure must not turn a completed backend install
                    // into an error. Its own reply is the authoritative outcome.
                    if let Ok(snapshot) = backend.call_with_timeout(
                        "snapshot",
                        serde_json::json!({}),
                        std::time::Duration::from_secs(2),
                    ) {
                        observed.observe(&snapshot["runtime"]["agentInstallation"][&server]);
                    }
                }
            }
        }
    });
    Ok((progress, receiver))
}

#[cfg(not(newport_remote_apps))]
pub(crate) fn start(
    _: &str,
    _: &Value,
) -> Result<(Progress, async_channel::Receiver<Result<String, String>>), String> {
    Err("Installing Remote Apps requires a Newport build with the full agent.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn progress_tracks_backend_upload_and_verification() {
        let progress = Progress::default();
        assert!(!progress.uploaded());
        progress.observe(&json!({"stage":"uploading", "sent":100, "total":200}));
        assert_eq!(progress.fraction(), 0.5);
        assert!(!progress.uploaded());
        progress.observe(&json!({"stage":"verifying", "sent":0, "total":0}));
        assert!(progress.uploaded());
        assert_eq!(Progress::at(200, 250).fraction(), 1.);
        assert_eq!(Progress::at(0, 0).fraction(), 0.);
    }
    #[test]
    fn invalid_install_requests_never_connect() {
        assert!(start("server", &json!({"bundledVariant":"core"})).is_err());
        assert!(
            start(
                "server",
                &json!({"bundledVariant":"full", "expectedConnection":{"id":"other"}})
            )
            .is_err()
        );
    }
}
