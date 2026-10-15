//! Preserve protocol error identity separately from human-readable feedback.
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestError {
    pub message: String,
    pub code: Option<String>,
}
impl RequestError {
    pub fn from_response(value: &Value) -> Self {
        Self {
            message: crate::backend::error_text(value),
            code: value.get("code").and_then(Value::as_str).map(str::to_owned),
        }
    }
}
impl From<String> for RequestError {
    fn from(message: String) -> Self {
        Self {
            message,
            code: None,
        }
    }
}
impl From<&str> for RequestError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for RequestError {}
