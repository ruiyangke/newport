//! Bounded startup probe for a previously validated native candidate. A health
//! receipt never itself accepts a transaction or removes its recovery backup.
use serde::Deserialize;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    protocol: String,
    schema: u32,
    version: String,
    status: String,
}

/// Run the candidate headlessly, using this process's profile/environment.
/// The caller must verify the bundle/signature first and drain the old backend
/// before calling. No shell is used and no arbitrary arguments are forwarded.
/// The candidate must exit successfully AFTER releasing its profile and return
/// the exact version's health receipt. This is backend/grammar readiness, not
/// acceptance of GUI startup, accessibility, or remote connectivity.
pub async fn verify_startup(
    executable: &Path,
    expected: &semver::Version,
    timeout: Duration,
) -> Result<(), String> {
    if !executable.is_absolute() {
        return Err("The startup probe requires an absolute executable path.".into());
    }
    let mut command = Command::new(executable);
    command.arg("--check-update-health");
    probe(command, expected, timeout).await
}

async fn probe(
    mut command: Command,
    expected: &semver::Version,
    timeout: Duration,
) -> Result<(), String> {
    if timeout.is_zero() {
        return Err("The startup probe requires a nonzero deadline.".into());
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Could not start the updated app's health check: {error}"))?;
    let output = child
        .stdout
        .take()
        .ok_or("The startup probe has no output pipe.")?;
    let read = async move {
        let mut bytes = Vec::new();
        output
            .take(4097)
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| error.to_string())?;
        if bytes.len() > 4096 {
            return Err("The updated app exceeded the health receipt size limit.".to_owned());
        }
        Ok(bytes)
    };
    let checked = tokio::time::timeout(timeout, async {
        let (bytes, status) = tokio::try_join!(read, async {
            child.wait().await.map_err(|error| error.to_string())
        })?;
        if !status.success() {
            return Err(format!(
                "The updated app's startup check failed ({status})."
            ));
        }
        validate(&bytes, expected)
    })
    .await
    .unwrap_or_else(|_| Err("The updated app's startup check timed out.".into()));
    if checked.is_err() {
        // Includes full pipes, timeouts, failed receipts and inherited output
        // pipes that never close. Always reap a failed probe before rollback.
        let _ = child.kill().await;
        child
            .wait()
            .await
            .map_err(|error| format!("Could not reap the startup probe: {error}"))?;
    }
    checked
}

fn validate(bytes: &[u8], expected: &semver::Version) -> Result<(), String> {
    let receipt: Receipt = serde_json::from_slice(bytes)
        .map_err(|_| "The updated app did not return a valid health receipt.".to_owned())?;
    if receipt.protocol != "newport-update-health"
        || receipt.schema != 1
        || receipt.status != "ready"
        || super::version(&receipt.version)? != *expected
    {
        return Err("The updated app's health receipt does not match the expected version.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECEIPT: &str =
        r#"{"protocol":"newport-update-health","schema":1,"version":"2.0.0","status":"ready"}"#;

    #[test]
    fn only_an_exact_unambiguous_versioned_health_receipt_is_accepted() {
        let expected = "2.0.0".parse().unwrap();
        assert!(validate(RECEIPT.as_bytes(), &expected).is_ok());
        for receipt in [
            "",
            "OK",
            "{}",
            &RECEIPT.replace("2.0.0", "1.0.0"),
            &RECEIPT.replace("ready", "starting"),
            &RECEIPT.replace(":1", ":2"),
            &RECEIPT.replace("newport-update-health", "other"),
            &format!("{RECEIPT}{RECEIPT}"),
            &RECEIPT.replace("\"schema\":1", "\"schema\":1,\"schema\":1"),
            &RECEIPT.replace("\"schema\":1", "\"schema\":1,\"extra\":true"),
        ] {
            assert!(
                validate(receipt.as_bytes(), &expected).is_err(),
                "{receipt}"
            );
        }
    }

    #[cfg(unix)]
    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(script);
        command
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_child_must_exit_successfully_as_well_as_return_a_receipt() {
        let expected = "2.0.0".parse().unwrap();
        let output = format!("printf '%s' '{RECEIPT}'");
        assert!(
            probe(shell(&output), &expected, Duration::from_secs(3))
                .await
                .is_ok()
        );
        for script in [
            format!("{output}; exit 1"),
            "exit 0".into(),
            "printf bad-receipt".into(),
        ] {
            assert!(
                probe(shell(&script), &expected, Duration::from_secs(3))
                    .await
                    .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hung_or_flooding_children_are_bounded_and_reaped() {
        let expected = "2.0.0".parse().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let pid_path = directory.path().join("child.pid");
        let mut sleeper = shell("printf '%s' \"$$\" > \"$NEWPORT_HEALTH_PID\"; exec sleep 10");
        sleeper.env("NEWPORT_HEALTH_PID", &pid_path);
        let started = std::time::Instant::now();
        assert!(
            probe(sleeper, &expected, Duration::from_millis(500))
                .await
                .unwrap_err()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        let pid = std::fs::read_to_string(pid_path)
            .unwrap()
            .parse::<libc::pid_t>()
            .unwrap();
        // SAFETY: signal 0 only queries this recorded child PID; it sends no signal.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert!(
            probe(
                shell("while :; do printf '0123456789abcdef'; done"),
                &expected,
                Duration::from_secs(3)
            )
            .await
            .unwrap_err()
            .contains("size limit")
        );
        assert!(
            probe(shell("exit 0"), &expected, Duration::ZERO)
                .await
                .is_err()
        );
    }
}
