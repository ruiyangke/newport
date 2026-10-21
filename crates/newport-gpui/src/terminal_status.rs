//! Terminal lifecycle for presentation, separate from transient input warnings.
//!
//! `started` records whether this connection scope has ever reached a shell. It
//! survives retries and disconnection so existing terminal output can stay visible.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Connected,
    Reconnecting { attempt: u32 },
    Disconnected,
    Exited(Option<u32>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalStatus {
    pub phase: Phase,
    pub started: bool,
}

impl Default for TerminalStatus {
    fn default() -> Self {
        Self {
            phase: Phase::Disconnected,
            started: false,
        }
    }
}

impl TerminalStatus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start another connection attempt in the same scope, clearing any failure.
    pub fn begin(&mut self) {
        self.phase = Phase::Connecting;
    }
    pub fn ready(&mut self) {
        self.phase = Phase::Connected;
        self.started = true;
    }
    pub fn reconnecting(&mut self, attempt: u32) {
        self.phase = Phase::Reconnecting { attempt };
    }
    pub fn disconnected(&mut self) {
        self.phase = Phase::Disconnected;
    }
    pub fn exited(&mut self, code: Option<u32>) {
        self.phase = Phase::Exited(code);
    }
    pub fn failed(&mut self, error: impl Into<String>) {
        self.phase = Phase::Failed(error.into());
    }

    /// A changed server/backend/credential scope must not inherit a started shell.
    pub fn reset_scope(&mut self) {
        *self = Self::default();
    }

    pub fn label(&self) -> String {
        match &self.phase {
            Phase::Connecting => "Connecting…".into(),
            Phase::Connected => "Connected".into(),
            Phase::Reconnecting { attempt } => format!("Reconnecting · {attempt}"),
            Phase::Disconnected => "Disconnected".into(),
            Phase::Exited(Some(code)) => format!("Shell exited · {code}"),
            Phase::Exited(None) => "Shell closed".into(),
            Phase::Failed(_) => "Connection failed".into(),
        }
    }
    pub fn error(&self) -> Option<&str> {
        match &self.phase {
            Phase::Failed(error) => Some(error),
            _ => None,
        }
    }
    pub fn cancel_label(&self) -> Option<&'static str> {
        match self.phase {
            Phase::Connecting => Some("Cancel connection"),
            Phase::Reconnecting { .. } => Some("Stop reconnecting"),
            _ => None,
        }
    }
    pub fn connected(&self) -> bool {
        matches!(self.phase, Phase::Connected)
    }
    /// States that hide the verbose status text.
    pub fn steady(&self) -> bool {
        matches!(self.phase, Phase::Connected | Phase::Disconnected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn started_survives_retries_and_disconnect_until_scope_changes() {
        let mut state = TerminalStatus::new();
        assert!(!state.started);
        assert_eq!(state.phase, Phase::Disconnected);
        assert_eq!(state.cancel_label(), None);
        state.begin();
        assert_eq!(state.cancel_label(), Some("Cancel connection"));
        state.reconnecting(1);
        assert!(!state.started, "failed startup never reached a shell");
        assert_eq!(state.cancel_label(), Some("Stop reconnecting"));
        state.ready();
        assert!(state.started && state.connected() && state.steady());
        state.reconnecting(2);
        assert!(state.started && !state.connected() && !state.steady());
        assert_eq!(state.label(), "Reconnecting · 2");
        state.disconnected();
        assert!(state.started && state.steady());
        assert_eq!(state.cancel_label(), None);
        state.begin();
        assert!(state.started);
        assert_eq!(state.phase, Phase::Connecting);
        state.reset_scope();
        assert_eq!(state, TerminalStatus::default());
    }

    #[test]
    fn failure_is_exposed_separately_and_cleared_by_lifecycle_transitions() {
        let mut state = TerminalStatus::default();
        state.failed("authentication failed");
        assert_eq!(state.error(), Some("authentication failed"));
        assert_eq!(state.label(), "Connection failed");
        assert!(!state.started && !state.connected());
        assert!(!state.steady());
        assert_eq!(state.cancel_label(), None);
        state.begin();
        assert_eq!(state.error(), None);
        state.ready();
        state.failed("connection lost");
        assert!(state.started);
        state.disconnected();
        assert_eq!(state.error(), None);
        assert!(state.started);
        state.failed("retry failed");
        state.reset_scope();
        assert_eq!(state.error(), None);
        assert!(!state.started);
    }

    #[test]
    fn shell_exit_retains_started_and_does_not_offer_cancel() {
        let mut state = TerminalStatus::default();
        state.ready();
        state.exited(Some(127));
        assert_eq!(state.label(), "Shell exited · 127");
        assert_eq!(state.error(), None);
        assert_eq!(state.cancel_label(), None);
        assert!(state.started && !state.connected() && !state.steady());
        state.exited(None);
        assert_eq!(state.label(), "Shell closed");
        state.begin();
        assert!(state.started);
        state.ready();
        assert!(state.connected());
    }
}
