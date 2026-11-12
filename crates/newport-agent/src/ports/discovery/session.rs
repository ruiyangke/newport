use super::{Discovery, Mode};
use newport_protocol::{
    common::{Error, Retry},
    monitor::ports::discovery::{Request, Snapshot},
};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct Session {
    discovery: Discovery,
    generation: u64,
}

impl Session {
    pub fn handle(
        &mut self,
        request: &Request,
        containers: &crate::containers::Containers,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Snapshot, Error> {
        self.generation = self.generation.checked_add(1).ok_or_else(|| Error {
            code: "discovery_sequence_exhausted".into(),
            message: "Restart the discovery worker to continue scanning.".into(),
            retry: Retry::Yes,
        })?;
        if request.refresh {
            self.discovery.invalidate();
        }
        self.discovery
            .collect(
                self.generation,
                Instant::now() + Duration::from_secs(20),
                Mode::Full(containers),
                cancelled,
            )
            .map(|scan| scan.snapshot)
            .map_err(|error| Error {
                code: "discovery_failed".into(),
                message: error.to_string(),
                retry: Retry::Yes,
            })
    }
}
