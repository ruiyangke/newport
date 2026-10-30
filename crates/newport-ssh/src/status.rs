use crate::Error;
use std::time::Instant;

/// One server's state, as every surface shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    /// What the connections can do.
    pub reach: Reach,
    /// The login in progress, or why the last one didn't succeed.
    pub login: Option<Login>,
    /// Requests waiting for a connection or a free session.
    pub waiting: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reach {
    /// Nothing uses the server and no connection is open.
    Idle,
    /// No connection has been ready yet.
    Connecting,
    /// At least one connection is ready.
    Ready { connections: usize },
    /// Connections were ready before; the next attempt is at `next`.
    Reconnecting { attempt: u32, next: Option<Instant> },
    /// Nothing works, and nothing retries until [`crate::Link::retry`].
    Failed(Error),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Login {
    InProgress,
    WaitingForApproval,
    Failed(Error),
}

impl Default for Status {
    fn default() -> Self {
        Self {
            reach: Reach::Idle,
            login: None,
            waiting: 0,
        }
    }
}
