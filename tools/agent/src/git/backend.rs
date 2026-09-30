//! Select once per RPC process, before accepting any repository request.
//!
//! `NEWPORT_GIT_BACKEND=git2` (the default) keeps the existing production engine.
//! `NEWPORT_GIT_BACKEND=cli` enables the experimental CLI adapter.
//! Both use the same protocol version, typed requests, output framing and errors.
//! Ready advertises the engine and its actual methods/features: there is never
//! per-method fallback. Both engines use the same durable operation journal.
//!
//! Compare disposable fixtures using `scripts/benchmark-git-backends.py` with
//! `--agent tools/agent/target/release/newport-agent --output <report.json>`.
//! The desktop defaults to git2; its NEWPORT_GIT_BACKEND environment setting
//! selects the engine for all remote channels in a connection.
pub use super::protocol::Output;
use super::{
    cli,
    protocol::{self, Error, Request},
    repository::Service,
};
use serde_json::{json, Value};

pub(super) enum Backend {
    Git2(Box<Service>),
    Cli(cli::Service),
}
impl Backend {
    pub(super) fn from_env(
        journal: impl FnOnce() -> Option<super::journal::Journal>,
    ) -> std::io::Result<Self> {
        match std::env::var("NEWPORT_GIT_BACKEND") {
            Err(std::env::VarError::NotPresent) => Ok(Self::Git2(Box::new(
                journal().map(Service::with_journal).unwrap_or_default(),
            ))),
            Ok(value) if value == "git2" => Ok(Self::Git2(Box::new(
                journal().map(Service::with_journal).unwrap_or_default(),
            ))),
            Ok(value) if value == "cli" => {
                cli::check_runtime().map_err(std::io::Error::other)?;
                Ok(Self::Cli(cli::Service::with_journal(journal())))
            }
            _ => Err(std::io::Error::other(
                "NEWPORT_GIT_BACKEND must be git2 or cli",
            )),
        }
    }
    pub(super) fn writable(&self) -> bool {
        match self {
            Self::Git2(s) => s.writable(),
            Self::Cli(s) => s.writable(),
        }
    }
    pub(super) fn methods(&self) -> &'static [&'static str] {
        match self {
            Self::Git2(_) => protocol::METHODS,
            Self::Cli(_) => cli::METHODS,
        }
    }
    pub(super) fn capabilities(&self, mut value: Value) -> Value {
        value["backend"] = json!(match self {
            Self::Git2(_) => "git2",
            Self::Cli(_) => "cli",
        });
        if matches!(self, Self::Cli(_)) {
            value["features"] = json!(cli::FEATURES);
            value["actions"] = if self.writable() {
                json!(cli::ACTIONS)
            } else {
                json!([])
            };
        }
        value
    }
    pub(super) fn request(&mut self, request: Request) -> Result<Output, Error> {
        match self {
            Self::Git2(s) => s.request(request),
            Self::Cli(s) => s.request(request),
        }
    }
}
