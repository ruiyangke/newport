use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

pub(super) fn error(e: impl Serialize) -> Value {
    serde_json::to_value(e).unwrap_or_else(|_| json!("Backend error"))
}
pub(super) fn encode(v: impl Serialize) -> Result<Value, Value> {
    serde_json::to_value(v).map_err(|e| json!(e.to_string()))
}
pub(super) fn argument<T: DeserializeOwned>(args: &Value, name: &str) -> Result<T, Value> {
    serde_json::from_value(args.get(name).cloned().unwrap_or(Value::Null))
        .map_err(|e| json!(format!("Invalid {name}: {e}")))
}
