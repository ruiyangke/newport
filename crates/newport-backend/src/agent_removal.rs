//! Explicit remote removal using the installation's retained, verified build.
use super::{check, helper_protocol};
use crate::{
    lifecycle_receipts::{
        Removal, RemovalPhase, Store,
        protocol::{RemovalReceipt, RemovalState},
    },
    ssh::ExecSession,
};
use uuid::Uuid;

fn command(build: &str) -> Result<String, String> {
    if !crate::agent_runtime::protocol::valid_build(build) {
        return Err("Invalid Needle removal build".into());
    }
    Ok(format!(
        "\"$HOME/.local/share/newport/needle/builds/{build}/newport-agent\""
    ))
}

async fn probe(session: &ExecSession, build: &str) -> Result<String, String> {
    let command = command(build)?;
    let protocol = helper_protocol(session, &command, build).await?;
    if protocol["removalOperations"] != true || protocol["removalCancellation"] != true {
        return Err("This Needle build cannot safely reconcile removal. Explicitly update Needle before removing it.".into());
    }
    Ok(command)
}

pub(crate) async fn review(
    session: &ExecSession,
    store: &Store,
    id: Uuid,
    keep: Option<(usize, &str)>,
) -> Result<crate::lifecycle_receipts::protocol::HelperReview, String> {
    use crate::lifecycle_receipts::protocol::HelperReview;
    let record = store
        .removal(session.server(), id)?
        .ok_or("Needle removal not found")?;
    if record.phase != RemovalPhase::Restoring {
        return Err("Only this connection's pending removal restoration can be reviewed".into());
    }
    if keep.is_some_and(|(index, fingerprint)| {
        index >= 32 || !crate::agent_runtime::protocol::valid_build(fingerprint)
    }) {
        return Err("Invalid file review decision".into());
    }
    let command = command(&record.build)?;
    let protocol = helper_protocol(session, &command, &record.build).await?;
    if protocol["removalReview"] != true {
        return Err("This recorded Needle build does not support removal file review. Its recovery evidence has been preserved.".into());
    }
    let args = match keep {
        None => format!("review-removal {id} --build {}", record.build),
        Some((index, fingerprint)) => format!(
            "keep-removal {id} --build {} {index} {fingerprint}",
            record.build
        ),
    };
    let report: HelperReview = serde_json::from_str(
        &session
            .execute(&format!("{command} deployment {args}"), None)
            .await?,
    )
    .map_err(|e| format!("Invalid removal file review: {e}"))?;
    if report.operation_id != id || report.files.len() > 32 {
        return Err("Invalid removal file review response".into());
    }
    Ok(report)
}

pub(crate) async fn prepare(
    session: &ExecSession,
    store: &Store,
    id: Uuid,
    build: &str,
) -> Result<Removal, String> {
    if id.is_nil() {
        return Err("Invalid Needle removal ID".into());
    }
    command(build)?;
    if let Some(record) = store.removal(session.server(), id)? {
        if record.build != build {
            return Err("Needle removal build changed".into());
        }
        return Ok(record);
    }
    store.require_agent_available(session.server())?;
    let current = check(session).await?;
    if current.installed_hash.as_deref() != Some(build) {
        return Err(
            "Needle changed since removal was requested. Inspect it before retrying.".into(),
        );
    }
    probe(session, build).await?;
    store.begin_removal(session.server(), id, build).await
}

pub(crate) async fn execute(
    session: &ExecSession,
    store: &Store,
    mut record: Removal,
    restore: bool,
) -> Result<String, String> {
    let id = record.operation_id;
    let command = probe(session, &record.build).await?;
    let next = if restore {
        RemovalPhase::Restoring
    } else {
        RemovalPhase::Removing
    };
    record = store
        .update_removal(session.server(), id, record.phase, next, None)
        .await?;
    let command = if restore {
        format!(
            "{command} deployment restore-removal {id} --build {}",
            record.build
        )
    } else {
        format!(
            "{command} deployment remove {} --operation {id}",
            record.build
        )
    };
    let output = session.execute(&command, None).await?;
    let receipt: RemovalReceipt = serde_json::from_str(&output)
        .map_err(|e| format!("Invalid Needle removal receipt: {e}"))?;
    if receipt.operation_id != id || receipt.build != record.build {
        return Err("Needle removal receipt does not match the recorded request".into());
    }
    match receipt.state {
        RemovalState::Restored => {
            // Another explicit recovery may have won the remote lock. Observe
            // its cancellation tombstone instead of trying to remove again.
            if record.phase != RemovalPhase::Restoring {
                record = store
                    .update_removal(
                        session.server(),
                        id,
                        record.phase,
                        RemovalPhase::Restoring,
                        None,
                    )
                    .await?;
            }
            store
                .update_removal(
                    session.server(),
                    id,
                    record.phase,
                    RemovalPhase::Restored,
                    None,
                )
                .await?;
            Ok("Needle removal restored or cancelled. Agent services are available again.".into())
        }
        RemovalState::Removed if !restore => {
            store
                .update_removal(
                    session.server(),
                    id,
                    record.phase,
                    RemovalPhase::Removed,
                    None,
                )
                .await?;
            Ok("Needle removed. SSH terminals and forwarding remain available.".into())
        }
        _ => Err(
            "Needle removal is not confirmed complete. Resume or restore the recorded operation."
                .into(),
        ),
    }
}
