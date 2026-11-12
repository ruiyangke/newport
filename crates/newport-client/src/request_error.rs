//! Preserve protocol error identity separately from human-readable feedback.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestError {
    pub message: String,
    pub code: Option<String>,
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

impl From<crate::backend::exchange::Error> for RequestError {
    fn from(error: crate::backend::exchange::Error) -> Self {
        Self {
            message: error.to_string(),
            code: Some(error.code().to_owned()),
        }
    }
}
