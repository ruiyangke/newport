//! The Git CLI is the sole executor; framing and journaling stay engine-neutral.
pub use super::protocol::Output;
use super::{
    cli,
    protocol::{Error, Request},
};
use serde_json::{json, Value};
pub(super) struct Backend(cli::Service);
impl Backend {
    pub(super) fn new(
        journal: impl FnOnce() -> Option<super::journal::Journal>,
    ) -> std::io::Result<Self> {
        cli::check_runtime().map_err(std::io::Error::other)?;
        Ok(Self(cli::Service::with_journal(journal())))
    }
    pub(super) fn writable(&self) -> bool {
        self.0.writable()
    }
    pub(super) fn methods(&self) -> &'static [&'static str] {
        cli::METHODS
    }
    pub(super) fn capabilities(&self, mut value: Value) -> Value {
        value["backend"] = json!("cli");
        value["features"] = json!(cli::FEATURES);
        value["actions"] = if self.writable() {
            json!(cli::ACTIONS)
        } else {
            json!([])
        };
        value
    }
    pub(super) fn request(&mut self, request: Request) -> Result<Output, Error> {
        self.0.request(request)
    }
}
