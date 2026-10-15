//! Headless startup acceptance. Success is reported only after the real backend
//! responds with a readable profile and drains all services and profile ownership.
use newport_backend::application::Application;
use serde_json::{Value, json};
use std::time::Duration;

pub struct Report {
    pub servers: usize,
    pub tunnels: usize,
}

impl Report {
    pub fn update_receipt(&self) -> Value {
        // Keep profile contents, server names and credentials out of the receipt.
        json!({"protocol":"newport-update-health","schema":1,"version":env!("CARGO_PKG_VERSION"),"status":"ready"})
    }
}

pub async fn check(application: Application) -> Result<Report, String> {
    let observed = read(application.endpoint().to_owned()).await;
    // Always drain, including when RPC, profile access or response validation
    // fails. A successful snapshot alone cannot certify a restart is safe.
    let stopped = application.stop().await;
    match (observed, stopped) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(check), Err(stop)) => Err(format!("{check}; backend shutdown also failed: {stop}")),
    }
}

/// Observe the running backend without releasing its profile ownership.
pub async fn read(endpoint: std::path::PathBuf) -> Result<Report, String> {
    tokio::task::spawn_blocking(move || {
        let snapshot = newport_client::backend::Backend::at_path(endpoint).call_with_timeout(
            "snapshot",
            json!({}),
            Duration::from_secs(10),
        )?;
        validate(snapshot)
    })
    .await
    .map_err(|error| format!("Backend health check failed: {error}"))
    .and_then(|result| result)
}

fn validate(snapshot: Value) -> Result<Report, String> {
    if snapshot.get("loadError") != Some(&Value::Null) {
        return Err("The profile could not be opened; startup health was not accepted.".into());
    }
    if snapshot["instanceId"].as_str().is_none_or(str::is_empty) || !snapshot["runtime"].is_object()
    {
        return Err("The backend returned an invalid startup snapshot.".into());
    }
    let config: newport_domain::model::Config = serde_json::from_value(snapshot["config"].clone())
        .map_err(|_| "The backend returned an invalid profile snapshot.".to_owned())?;
    Ok(Report {
        servers: config.servers.len(),
        tunnels: config.tunnels.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop;
    use newport_backend::profile::{Instance, Store};
    use std::sync::Arc;

    #[tokio::test]
    async fn real_startup_check_drains_and_releases_ownership_on_success_and_locked_profile() {
        for corrupt in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let store = Store::for_test(directory.path().into());
            store
                .save(&newport_domain::model::Config::default())
                .unwrap();
            let path = directory.path().join("profiles.stronghold");
            if corrupt {
                std::fs::write(&path, "unreadable vault").unwrap();
            }
            let before = std::fs::read(&path).unwrap();
            let (desktop, _requests) = desktop::Integration::new();
            let application = Application::start(
                store,
                true,
                Arc::new(desktop.clone()),
                desktop::Commands::new(desktop, newport_platform::startup::Startup::disabled()),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                read(application.endpoint().to_owned()).await.is_ok(),
                !corrupt
            );
            assert!(Instance::acquire(directory.path(), true).unwrap().is_none());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            let result = check(application).await;
            if corrupt {
                assert!(
                    result
                        .err()
                        .unwrap()
                        .contains("profile could not be opened")
                );
            } else {
                let report = result.unwrap();
                assert_eq!((report.servers, report.tunnels), (0, 0));
                assert_eq!(report.update_receipt()["status"], "ready");
            }
            assert!(Instance::acquire(directory.path(), true).unwrap().is_some());
            assert_eq!(std::fs::read(path).unwrap(), before);
        }
    }

    #[test]
    fn malformed_or_locked_snapshots_never_report_healthy() {
        let valid = json!({"instanceId":"fixture","runtime":{},"config":{"servers":[],"tunnels":[]},"loadError":null});
        assert!(validate(valid.clone()).is_ok());
        for field in ["loadError", "instanceId", "runtime", "config"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(validate(missing).is_err(), "Missing {field}");
        }
        for (field, value) in [
            ("loadError", json!("Keychain denied")),
            ("instanceId", json!("")),
            ("runtime", json!([])),
            ("config", json!({"servers":[],"tunnels":"invalid"})),
        ] {
            let mut malformed = valid.clone();
            malformed[field] = value;
            assert!(validate(malformed).is_err(), "Malformed {field}");
        }
    }
}
