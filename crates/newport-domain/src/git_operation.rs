//! Validated operation outcomes shared by native Git controls and recovery.
use newport_protocol::git::{Operation, OperationState, Reply};

/// Only errors before operation.start are safe to dismiss without recovery.
#[derive(Debug)]
pub struct WriteError {
    pub message: String,
    pub dispatched: bool,
}
impl From<String> for WriteError {
    fn from(message: String) -> Self {
        Self {
            message,
            dispatched: false,
        }
    }
}
impl From<&str> for WriteError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

/// The operation a write or lookup reported, once it names `expected_id` and
/// carries the result or error its state promises.
pub fn confirmed(reply: Reply, expected_id: &str) -> Result<Operation, String> {
    let Reply::Operation(operation) = reply else {
        return Err("Invalid Git operation response; check its saved outcome".into());
    };
    let operation = *operation;
    if operation.operation_id != expected_id
        || operation.repository.is_empty()
        || operation.payload_hash.is_empty()
        || (matches!(
            operation.state,
            OperationState::Succeeded | OperationState::NeedsResolution
        ) && operation.result.is_none())
        || (operation.state == OperationState::Failed && operation.error.is_none())
    {
        return Err("Git operation response did not confirm the requested outcome".into());
    }
    Ok(operation)
}

#[cfg(test)]
mod tests {
    use super::confirmed;
    use newport_protocol::git::{
        Error, Operation, OperationResult, OperationState, Refresh, Reply,
    };
    fn outcome() -> Operation {
        Operation {
            operation_id: "op".into(),
            repository: "repo".into(),
            payload_hash: "hash".into(),
            state: OperationState::Succeeded,
            seq: 1,
            result: Some(OperationResult::Refresh(Refresh {
                refresh_required: true,
            })),
            error: None,
        }
    }
    fn reply(operation: Operation) -> Reply {
        Reply::Operation(Box::new(operation))
    }
    #[test]
    fn outcome_requires_matching_identity_and_complete_result() {
        assert_eq!(
            confirmed(reply(outcome()), "op").unwrap().state,
            OperationState::Succeeded
        );
        assert!(confirmed(reply(outcome()), "other").is_err());
        for invalid in [
            Operation {
                repository: String::new(),
                ..outcome()
            },
            Operation {
                payload_hash: String::new(),
                ..outcome()
            },
        ] {
            assert!(confirmed(reply(invalid), "op").is_err());
        }
        let mut value = outcome();
        value.result = None;
        assert!(confirmed(reply(value.clone()), "op").is_err());
        value.state = OperationState::NeedsResolution;
        assert!(confirmed(reply(value.clone()), "op").is_err());
        value.state = OperationState::Failed;
        assert!(confirmed(reply(value.clone()), "op").is_err());
        value.error = Some(Error::new("AUTH_FAILED", "Authentication failed"));
        assert_eq!(
            confirmed(reply(value), "op").unwrap().state,
            OperationState::Failed
        );
    }
    #[test]
    fn uncertainty_and_running_are_not_success() {
        for state in [
            OperationState::Running,
            OperationState::OutcomeUnknown,
            OperationState::ReviewedUnknown,
        ] {
            let value = Operation {
                state,
                result: None,
                ..outcome()
            };
            assert_eq!(confirmed(reply(value), "op").unwrap().state, state);
        }
    }
    #[test]
    fn other_replies_are_not_outcomes() {
        let other = Reply::Close(Box::new(newport_protocol::git::Closed { closed: true }));
        assert!(confirmed(other, "op").is_err());
    }
}
