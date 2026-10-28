//! What a page shows while its server isn't connected: its last readings
//! with a note, or, with none yet, why it's empty. The header shows the
//! connection itself.
use crate::*;
use newport_domain::connectivity::Phase;

impl Newport {
    /// The selected server's connection while it isn't up.
    pub(super) fn connection_down(&self, cx: &App) -> Option<Phase> {
        crate::connectivity::scoped(&self.server, &self.snapshot, cx)
            .map(|state| state.phase)
            .filter(|phase| *phase != Phase::Connected)
    }

    /// In place of a page with nothing to show until the server is connected.
    pub(super) fn connection_pending(
        &self,
        phase: Phase,
        feature: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        empty::pane(
            IconName::ServerOff,
            phase.label(),
            format!(
                "{feature} appear once {} is connected.",
                self.server_label()
            ),
            cx,
        )
        .into_any_element()
    }

    fn server_label(&self) -> String {
        self.selected_server()
            .map(|server| text(server, "name"))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "the server".into())
    }
}

/// Above readings taken before the connection went down.
pub(super) fn last_reading(phase: Phase) -> String {
    format!("{}. Showing the last reading.", phase.label())
}

#[cfg(test)]
#[path = "connection_wait_tests.rs"]
mod tests;
