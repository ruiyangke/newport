//! The Git CLI is the sole executor; framing and journaling stay engine-neutral.
pub enum Output {
    Reply(super::protocol::Reply),
    Diff {
        snapshot: String,
        diff: Box<super::protocol::Diff>,
    },
}
use super::{
    cli,
    protocol::{Error, Request},
};
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
        super::protocol::METHODS
    }
    pub(super) fn capabilities(&self, methods: Vec<String>) -> super::protocol::Capabilities {
        super::protocol::Capabilities {
            methods,
            object_formats: vec!["sha1".into(), "sha256".into()],
            features: cli::FEATURES.iter().map(|value| (*value).into()).collect(),
            actions: if self.writable() {
                cli::ACTIONS.iter().map(|value| (*value).into()).collect()
            } else {
                vec![]
            },
        }
    }
    pub(super) fn request(&mut self, request: Request) -> Result<Output, Error> {
        self.0.request(request)
    }
}
