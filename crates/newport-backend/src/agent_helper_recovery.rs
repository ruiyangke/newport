//! Explicit standalone helper restoration. Observation never calls these functions.
use crate::{
    agent_lifecycle::{check, helper_protocol},
    lifecycle_receipts::{
        HelperRecovery, HelperRecoveryPhase, Store,
        protocol::{HelperOrigin, HelperReview, HelperState, StandaloneHelper},
    },
    ssh::ExecSession,
};
use uuid::Uuid;

async fn executor(session: &ExecSession, build: &str) -> Result<String, String> {
    if !crate::agent_runtime::protocol::valid_build(build) {
        return Err("Invalid helper recovery build".into());
    }
    let pinned = format!("\"$HOME/.local/share/newport/needle/builds/{build}/newport-agent\"");
    let exists = session
        .execute(&format!("if test -x {pinned}; then printf yes; fi"), None)
        .await?
        == "yes";
    let command = if exists {
        pinned
    } else {
        "\"$HOME/.local/bin/newport-agent\"".into()
    };
    let protocol = helper_protocol(session, &command, build).await?;
    if protocol["standaloneHelperRecovery"] != 1 || protocol["usageLease"] != 2 {
        return Err("This recorded Needle build does not support standalone helper recovery. Its evidence has been preserved.".into());
    }
    Ok(command)
}

pub(crate) async fn restore(
    session: &ExecSession,
    store: &Store,
    id: Uuid,
    build: &str,
    fingerprint: &str,
    validate: impl Fn() -> Result<(), String>,
) -> Result<String, String> {
    let valid = crate::agent_runtime::protocol::valid_build;
    if id.is_nil() || !valid(build) || !valid(fingerprint) {
        return Err("Invalid standalone helper restoration request".into());
    }
    validate()?;
    let record = match store.helper_recovery(session.server(), id)? {
        Some(record) => {
            if record.build != build || record.fingerprint != fingerprint {
                return Err(
                    "Helper restoration identity changed. Resume its recorded request.".into(),
                );
            }
            record
        }
        None => {
            store.require_agent_available(session.server())?;
            if check(session).await?.installed_hash.as_deref() != Some(build) {
                return Err("Needle changed since the helper operation was selected. Inspect its history again.".into());
            }
            let history = super::inspect(session, build)
                .await?
                .ok_or("Needle does not support helper history")?;
            let row = history
                .operations
                .iter()
                .find(|r| r.operation_id == id)
                .ok_or("Standalone helper operation is no longer available")?;
            if row.fingerprint != fingerprint || row.origin != HelperOrigin::Standalone {
                return Err("Helper ownership changed or cannot be attributed. Inspect the operation again.".into());
            }
            if row.state != HelperState::RolledBack
                && let Some(reason) = &row.restore_blocked
            {
                return Err(reason.clone());
            }
            executor(session, build).await?;
            validate()?;
            store
                .begin_helper_recovery(session.server(), id, build, fingerprint)
                .await?
        }
    };
    if record.phase == HelperRecoveryPhase::Restored {
        return Ok("Shell changes already restored.".into());
    }
    let result = execute(session, &record, &validate).await;
    validate()?;
    store
        .update_helper_recovery(&record, result.is_ok(), result.as_ref().err().cloned())
        .await?;
    result
}

async fn execute(
    session: &ExecSession,
    record: &HelperRecovery,
    validate: &impl Fn() -> Result<(), String>,
) -> Result<String, String> {
    let command = executor(session, &record.build).await?;
    validate()?;
    let output = session
        .execute(
            &format!(
                "{command} deployment restore-helper {} {}",
                record.operation_id, record.fingerprint
            ),
            None,
        )
        .await?;
    if output.len() > 512 * 1024 {
        return Err("Helper restoration response exceeds limit".into());
    }
    let result: StandaloneHelper =
        serde_json::from_str(&output).map_err(|_| "Invalid helper restoration response")?;
    if result.operation_id != record.operation_id
        || result.fingerprint != record.fingerprint
        || result.origin != HelperOrigin::Standalone
        || result.state != HelperState::RolledBack
        || result.file_count > 32
    {
        return Err("Needle did not confirm the recorded helper restoration".into());
    }
    Ok("Recorded shell changes restored. Needle services remain available.".into())
}

pub(crate) async fn review(
    session: &ExecSession,
    store: &Store,
    id: Uuid,
    keep: Option<(usize, &str)>,
) -> Result<HelperReview, String> {
    let record = store
        .helper_recovery(session.server(), id)?
        .ok_or("Helper restoration not found")?;
    if record.phase != HelperRecoveryPhase::Restoring {
        return Err("Only pending helper restoration can be reviewed".into());
    }
    if keep.is_some_and(|(index, fingerprint)| {
        index >= 32 || !crate::agent_runtime::protocol::valid_build(fingerprint)
    }) {
        return Err("Invalid file review decision".into());
    }
    let command = executor(session, &record.build).await?;
    let args = match keep {
        None => format!("review-helper {id} {}", record.fingerprint),
        Some((index, fingerprint)) => format!(
            "keep-helper {id} {} {index} {fingerprint}",
            record.fingerprint
        ),
    };
    let output = session
        .execute(&format!("{command} deployment {args}"), None)
        .await?;
    if output.len() > 512 * 1024 {
        return Err("Helper file review exceeds limit".into());
    }
    let report: HelperReview =
        serde_json::from_str(&output).map_err(|_| "Invalid helper file review")?;
    if report.operation_id != id || report.files.len() > 32 {
        return Err("Helper file review identity changed".into());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[tokio::test]
    #[ignore = "Requires the disposable Linux OpenSSH fixture"]
    async fn standalone_restore_persists_intent_preserves_services_and_resumes_after_restart() {
        let server: crate::model::Server = serde_json::from_value(json!({
            "id":Uuid::new_v4(),"name":"Standalone recovery fixture","sshUser":"fixture","sshHost":"127.0.0.1",
            "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(),"agentInstallApproved":true
        })).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let registry = crate::server_session::Registry::new(directory.path());
        let owner = registry.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        lease.install(false, false).await.unwrap();
        lease
            .execute("\"$HOME/.local/bin/newport-agent\" install", None)
            .await
            .unwrap();
        lease.check_installation().await.unwrap();
        let history = owner.lifecycle.helper_history.snapshot().history.unwrap();
        let row = history.operations[0].clone();
        let build = history.build_sha256;
        let id = row.operation_id;
        let (mut metrics, _) = lease.launch(crate::agent::Service::Monitor).await.unwrap();
        let runtime = owner.agent_runtime.snapshot().hello.unwrap().instance_id;
        let generation = lease.generation();
        assert!(
            lease
                .restore_standalone_helpers(id, &build, &"0".repeat(64))
                .await
                .is_err()
        );
        assert!(
            owner
                .lifecycle
                .receipts
                .helper_recovery(&server, id)
                .unwrap()
                .is_none()
        );
        lease
            .execute("sed -i 's/ env / personal-edit /g' \"$HOME/.bashrc\"", None)
            .await
            .unwrap();
        assert!(
            lease
                .restore_standalone_helpers(id, &build, &row.fingerprint)
                .await
                .is_err()
        );
        let pending = owner
            .lifecycle
            .receipts
            .helper_recovery(&server, id)
            .unwrap()
            .unwrap();
        assert_eq!(pending.phase, HelperRecoveryPhase::Restoring);
        assert!(pending.error.is_some());
        assert_eq!(
            registry.lifecycle_operations()[&server.id]["helperRecoveries"][0]["operationId"],
            id.to_string()
        );
        let files = lease.review_helpers(id).await.unwrap();
        let file = files
            .files
            .iter()
            .find(|f| f.path.ends_with("/.bashrc"))
            .unwrap();
        let index = file.index;
        let old = file.fingerprint.clone().unwrap();
        lease
            .execute(
                "printf '\\n# another personal edit\\n' >> \"$HOME/.bashrc\"",
                None,
            )
            .await
            .unwrap();
        assert!(lease.keep_helper(id, index, old).await.is_err());
        let files = lease.review_helpers(id).await.unwrap();
        let fresh = files
            .files
            .iter()
            .find(|f| f.index == index)
            .unwrap()
            .fingerprint
            .clone()
            .unwrap();
        lease.keep_helper(id, index, fresh).await.unwrap();
        let personal = lease.execute("cat \"$HOME/.bashrc\"", None).await.unwrap();
        assert_eq!(
            owner.agent_runtime.snapshot().hello.unwrap().instance_id,
            runtime
        );
        assert_eq!(lease.generation(), generation);
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        metrics.write_all(b"{\"op\":\"snapshot\"}\n").await.unwrap();
        metrics.shutdown().await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), metrics.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let reading: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            reading.is_object() && reading.get("error").is_none_or(serde_json::Value::is_null),
            "{reading}"
        );
        drop(metrics);
        registry.shutdown().await;

        let registry = crate::server_session::Registry::new(directory.path());
        let owner = registry.session(&server, 0);
        let lease = owner.acquire(&server).await.unwrap();
        assert_eq!(
            owner
                .lifecycle
                .receipts
                .helper_recovery(&server, id)
                .unwrap()
                .unwrap()
                .fingerprint,
            row.fingerprint
        );
        assert!(
            owner.lifecycle.helper_history.snapshot().history.is_none(),
            "durable intent survives independently of discovery cache"
        );
        // Losing the UI waiter cannot cancel an admitted restoration.
        let gate = owner.lifecycle.operation.lock().await;
        let waiter = tokio::spawn({
            let lease = owner.acquire(&server).await.unwrap();
            let build = build.clone();
            let fingerprint = row.fingerprint.clone();
            async move {
                lease
                    .restore_standalone_helpers(id, &build, &fingerprint)
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while owner.lifecycle.tasks.active().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        drop(gate);
        tokio::time::timeout(Duration::from_secs(10), async {
            while owner.lifecycle.tasks.active().is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let result = owner
            .lifecycle
            .receipts
            .helper_recovery(&server, id)
            .unwrap()
            .unwrap();
        assert_eq!(
            result.phase,
            HelperRecoveryPhase::Restored,
            "{:?}",
            result.error
        );
        assert_eq!(
            lease.execute("cat \"$HOME/.bashrc\"", None).await.unwrap(),
            personal
        );
        assert_eq!(
            lease
                .check_installation()
                .await
                .unwrap()
                .installed_hash
                .as_deref(),
            Some(build.as_str())
        );
        lease
            .execute(
                "printf '# newest personal file\\n' > \"$HOME/.bashrc\"",
                None,
            )
            .await
            .unwrap();
        lease
            .restore_standalone_helpers(id, &build, &row.fingerprint)
            .await
            .unwrap();
        assert_eq!(
            lease.execute("cat \"$HOME/.bashrc\"", None).await.unwrap(),
            "# newest personal file\n"
        );
        assert!(lease.review_helpers(id).await.is_err());
        registry.shutdown().await;
    }
}
