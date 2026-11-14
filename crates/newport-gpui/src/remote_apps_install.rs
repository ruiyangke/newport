//! Remote Apps uses the backend's shared agent installer and progress. The
//! backend owns verification, activation, rollback fencing and update policy.
use newport_protocol::desktop::applications::Availability;
use newport_protocol::desktop::configuration::{InstallProgress, InstallStage};
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
    fn observe(&self, value: Option<&InstallProgress>) {
        let mut upload = self.0.lock().unwrap();
        match value {
            Some(value) if value.stage == InstallStage::Uploading => {
                upload.sent = value.sent as usize;
                upload.total = value.total as usize;
                upload.uploaded = upload.total > 0 && upload.sent >= upload.total;
            }
            _ => {
                upload.uploaded = value.is_some_and(|value| {
                    matches!(
                        value.stage,
                        InstallStage::Installing
                            | InstallStage::Verifying
                            | InstallStage::Activating
                            | InstallStage::Ready
                    )
                });
            }
        }
    }
}

pub(crate) fn available() -> bool {
    cfg!(newport_remote_apps)
}

#[cfg(newport_remote_apps)]
pub(crate) fn start(
    server: &str,
    status: Option<&Availability>,
) -> Result<(Progress, async_channel::Receiver<Result<String, String>>), String> {
    let Some(status) =
        status.filter(|status| status.bundled_variant == newport_protocol::product::Variant::Full)
    else {
        return Err("This Newport build does not include the full agent. Update Newport to install Remote Apps.".into());
    };
    let expected = status.expected_server.clone();
    if expected.id.to_string() != server {
        return Err("Check the server connection again before installing.".into());
    }
    let expected_id = expected.id;
    let progress = Progress::default();
    let observed = progress.clone();
    let (sender, receiver) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let (finished, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let installed = crate::backend::Backend::discover()
                .execute(newport_protocol::desktop::Operation::Connection(Box::new(
                    newport_protocol::desktop::connection::Operation::SetupInstall(Box::new(
                        newport_protocol::desktop::connection::SetupInstallArgs {
                            expected_server: expected,
                        },
                    )),
                )))
                .map(|_| "Newport agent installed and verified.".to_owned())
                .map_err(|error| error.to_string());
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
                    use newport_protocol::desktop::{self, configuration};
                    if let Ok(snapshot) = backend
                        .execute_with_timeout(
                            desktop::Operation::Configuration(Box::new(
                                configuration::Operation::Snapshot(Box::new(
                                    newport_protocol::common::Present {},
                                )),
                            )),
                            std::time::Duration::from_secs(2),
                        )
                        .map_err(Into::into)
                        .and_then(newport_client::backend::snapshot_reply)
                    {
                        observed.observe(snapshot.runtime.agent_installation(expected_id));
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
    _: Option<&Availability>,
) -> Result<(Progress, async_channel::Receiver<Result<String, String>>), String> {
    Err("Installing Remote Apps requires a Newport build with the full agent.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_tracks_backend_upload_and_verification() {
        let progress = Progress::default();
        assert!(!progress.uploaded());
        let stage = |stage, sent, total| crate::snapshot_fixture::install(stage, sent, total);
        progress.observe(Some(&stage(InstallStage::Uploading, 100, 200)));
        assert_eq!(progress.fraction(), 0.5);
        assert!(!progress.uploaded());
        progress.observe(Some(&stage(InstallStage::Verifying, 0, 0)));
        assert!(progress.uploaded());
        progress.observe(None);
        assert!(!progress.uploaded());
        assert_eq!(Progress::at(200, 250).fraction(), 1.);
        assert_eq!(Progress::at(0, 0).fraction(), 0.);
    }
    #[test]
    fn invalid_install_requests_never_connect() {
        use crate::remote_apps_fixture::status;
        use newport_protocol::product::Variant;
        assert!(start("server", None).is_err());
        assert!(start("server", Some(&status())).is_err());
        let full = newport_protocol::desktop::applications::Availability {
            bundled_variant: Variant::Full,
            ..status()
        };
        assert!(start("server", Some(&full)).is_err());
    }
}
