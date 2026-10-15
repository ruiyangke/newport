//! Validated operation outcomes shared by native Git controls and recovery.
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub operation_id: uuid::Uuid,
    pub server_id: uuid::Uuid,
    pub action: String,
    pub state: ReceiptState,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    Pending,
    Succeeded,
    Failed,
    Rejected,
    NeedsResolution,
    OutcomeUnknown,
    ReviewedUnknown,
}
impl ReceiptState {
    pub fn dismissible(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Rejected | Self::ReviewedUnknown
        )
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Outcome not confirmed",
            Self::Succeeded => "Completed",
            Self::Failed => "Failed",
            Self::Rejected => "Not started",
            Self::NeedsResolution => "Needs resolution",
            Self::OutcomeUnknown => "Outcome unknown",
            Self::ReviewedUnknown => "Reviewed; outcome unknown",
        }
    }
}
pub fn receipts(value: Value, server: &str) -> Result<Vec<Receipt>, String> {
    let rows: Vec<Receipt> =
        serde_json::from_value(value).map_err(|_| "Invalid saved Git outcomes")?;
    let mut ids = std::collections::HashSet::new();
    if rows.len() > 1000
        || rows.iter().any(|row| {
            row.server_id.to_string() != server
                || !ids.insert(row.operation_id)
                || row.action.is_empty()
                || row.action.len() > 80
        })
    {
        return Err("Saved Git outcomes contain invalid identities".into());
    }
    Ok(rows)
}

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

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Running,
    Succeeded,
    Failed,
    NeedsResolution,
    OutcomeUnknown,
    ReviewedUnknown,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub operation_id: String,
    pub repository: String,
    pub payload_hash: String,
    pub state: State,
    pub seq: u64,
    pub result: Option<serde_json::Map<String, Value>>,
    pub error: Option<OperationError>,
}
#[derive(Debug, Deserialize)]
pub struct OperationError {
    pub code: String,
    pub message: String,
    pub retry: String,
}
impl Operation {
    pub fn decode(value: Value, expected_id: &str) -> Result<Self, String> {
        let operation: Self = serde_json::from_value(value)
            .map_err(|_| "Invalid Git operation response; check its saved outcome")?;
        if operation.operation_id != expected_id
            || operation.repository.is_empty()
            || operation.payload_hash.is_empty()
            || (matches!(operation.state, State::Succeeded | State::NeedsResolution)
                && operation.result.is_none())
            || (operation.state == State::Failed && operation.error.is_none())
        {
            return Err("Git operation response did not confirm the requested outcome".into());
        }
        Ok(operation)
    }
}

#[cfg(test)]
mod tests {
    use super::{Operation, State};
    use serde_json::json;
    fn outcome() -> serde_json::Value {
        json!({"operationId":"op","repository":"repo","payloadHash":"hash","seq":1,"state":"succeeded","result":{},"error":null})
    }
    #[test]
    fn outcome_requires_matching_identity_and_complete_result() {
        assert_eq!(
            Operation::decode(outcome(), "op").unwrap().state,
            State::Succeeded
        );
        assert!(Operation::decode(outcome(), "other").is_err());
        let mut value = outcome();
        value["result"] = serde_json::Value::Null;
        assert!(Operation::decode(value.clone(), "op").is_err());
        value["state"] = json!("failed");
        assert!(Operation::decode(value.clone(), "op").is_err());
        value["error"] =
            json!({"code":"AUTH_FAILED","message":"Authentication failed","retry":"never"});
        assert_eq!(Operation::decode(value, "op").unwrap().state, State::Failed);
    }
    #[test]
    fn uncertainty_and_running_are_not_success() {
        for (name, state) in [
            ("running", State::Running),
            ("outcome_unknown", State::OutcomeUnknown),
            ("reviewed_unknown", State::ReviewedUnknown),
        ] {
            let mut value = outcome();
            value["state"] = json!(name);
            value["result"] = serde_json::Value::Null;
            assert_eq!(Operation::decode(value, "op").unwrap().state, state);
        }
        let mut value = outcome();
        value["state"] = json!("completed");
        assert!(Operation::decode(value, "op").is_err());
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;
    use serde_json::json;
    const SERVER: &str = "810833d4-5258-45fa-ae8d-eea15ffa6df8";
    fn receipt() -> Value {
        json!({"operationId":"15b4856d-a324-479c-a462-83a4bf52c9d1","serverId":SERVER,"action":"fetch","state":"pending"})
    }
    #[test]
    fn recovery_rejects_wrong_server_duplicate_ids_and_unknown_states() {
        assert_eq!(receipts(json!([receipt()]), SERVER).unwrap().len(), 1);
        assert!(receipts(json!([receipt()]), "wrong-server").is_err());
        assert!(receipts(json!([receipt(), receipt()]), SERVER).is_err());
        let mut value = receipt();
        value["state"] = json!("done");
        assert!(receipts(json!([value]), SERVER).is_err());
        assert!(receipts(json!({}), SERVER).is_err());
    }
    #[test]
    fn uncertain_or_unresolved_receipts_cannot_be_dismissed() {
        for state in [
            ReceiptState::Pending,
            ReceiptState::OutcomeUnknown,
            ReceiptState::NeedsResolution,
        ] {
            assert!(!state.dismissible());
        }
        for state in [
            ReceiptState::Succeeded,
            ReceiptState::Failed,
            ReceiptState::Rejected,
            ReceiptState::ReviewedUnknown,
        ] {
            assert!(state.dismissible());
        }
    }
}
