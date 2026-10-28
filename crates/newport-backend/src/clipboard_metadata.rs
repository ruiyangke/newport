//! Native metadata observation must not suspend the integration event pump:
//! the native provider may itself be waiting for data arriving on that stream.
use super::{ClipboardOffer, clipboard_offer_at};
use crate::{desktop_integration::Desktop, integration_permissions::Gate};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct Metadata {
    jobs: tokio::task::JoinSet<(u64, Result<Option<ClipboardOffer>, String>)>,
    previous: Option<(u64, isize)>,
}
impl Metadata {
    pub(super) fn pending(&self) -> bool {
        !self.jobs.is_empty()
    }
    pub(super) fn start(&mut self, app: &Desktop, gate: &Arc<Gate>) {
        let Some(ticket) = gate.clipboard_ticket() else {
            return;
        };
        if self.pending() {
            return;
        }
        let app = app.clone();
        let gate = gate.clone();
        // A disable/re-enable can be coalesced by the permission watch. The
        // remote cache was revoked even if the consumer never observed "off".
        let previous = self
            .previous
            .filter(|(epoch, _)| *epoch == ticket)
            .map(|(_, revision)| revision);
        self.jobs.spawn(async move {
            let offer = clipboard_offer_at(&app, previous, Some(gate), Some(ticket)).await;
            (ticket, offer)
        });
    }
    pub(super) fn cancel(&mut self) {
        self.jobs.abort_all();
        self.previous = None;
    }
    pub(super) fn observed(&mut self, ticket: u64, revision: isize) {
        self.previous = Some((ticket, revision));
    }
    pub(super) async fn next(
        &mut self,
        gate: &Gate,
    ) -> Result<Option<(u64, ClipboardOffer)>, String> {
        match self.jobs.join_next().await {
            Some(Ok((ticket, result))) if gate.clipboard_ticket() == Some(ticket) => {
                result.map(|offer| offer.map(|offer| (ticket, offer)))
            }
            Some(Err(error)) if !error.is_cancelled() => Err(error.to_string()),
            _ => Ok(None),
        }
    }
}
