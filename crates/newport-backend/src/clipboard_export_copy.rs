//! Acknowledged runtime copies. This is read authority, not native publication
//! authority: the router must still validate focus and native revision before
//! publishing, and revoke a copy displaced by a newer native clipboard value.
use super::*;
use crate::agent_runtime::protocol::clipboard::CopyLease;

/// One acknowledged copy owned by its caller. It may be retained across focus
/// changes. Drop/revoke immediately fences returned bytes; the event pump then
/// releases runtime authority. Only an accepted Commit reply can construct it.
pub struct CommittedCopy {
    lease: CopyLease,
    current: Arc<AtomicBool>,
    disconnected: Arc<AtomicBool>,
}
impl CommittedCopy {
    pub fn offer(&self) -> &ExportOffer {
        &self.lease.offer
    }
    pub fn id(&self) -> &str {
        &self.lease.id
    }
    pub fn is_current(&self) -> bool {
        self.current.load(Ordering::Acquire) && !self.disconnected.load(Ordering::Acquire)
    }
    pub fn revoke(&self) {
        self.current.store(false, Ordering::Release);
    }
    pub(crate) fn connection_fence(&self) -> &Arc<AtomicBool> {
        &self.disconnected
    }
}
impl Drop for CommittedCopy {
    fn drop(&mut self) {
        self.revoke();
    }
}

/// Await beside the integration event pump. Abandoning this future does not
/// replay Commit: its eventual reply is drained and any accepted copy released.
pub struct Commit {
    id: u64,
    deadline: Instant,
    reply: oneshot::Receiver<Result<CommittedCopy, String>>,
}
impl Commit {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub async fn receive(self) -> Result<CommittedCopy, String> {
        let copy = tokio::time::timeout_at(self.deadline, self.reply)
            .await
            .map_err(|_| "Clipboard copy acknowledgment timed out".to_owned())?
            .map_err(|_| "Clipboard copy connection closed".to_owned())??;
        if !copy.is_current() {
            return Err(CHANGED.into());
        }
        Ok(copy)
    }
}
pub(super) struct Pending {
    id: u64,
    pub(super) deadline: Instant,
    proposal: Lease,
    reply: Option<oneshot::Sender<Result<CommittedCopy, String>>>,
}
impl Pending {
    pub(super) fn fail(mut self, reason: &str) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(Err(reason.into()));
        }
    }
}
pub(super) struct State {
    lease: CopyLease,
    pub(super) current: Arc<AtomicBool>,
    pub(super) release_deadline: Option<Instant>,
}
impl State {
    pub(super) fn offer(&self) -> &crate::agent_runtime::protocol::clipboard::Offer {
        &self.lease.offer.offer
    }
}
impl Receiver {
    pub(crate) fn commit(&mut self, proposal: &Lease) -> Result<(Commit, Control), String> {
        self.expire();
        if self.version != wire::COPY_VERSION || !self.enabled || self.disabling {
            return Err("Negotiate clipboard copy support before committing".into());
        }
        if !proposal.is_current()
            || !Arc::ptr_eq(&proposal.disconnected, &self.disconnected)
            || !self.source.as_ref().is_some_and(|source| {
                source.offer == proposal.offer && Arc::ptr_eq(&source.current, &proposal.current)
            })
        {
            return Err(CHANGED.into());
        }
        if self.committing.is_some()
            || self
                .committed
                .as_ref()
                .is_some_and(|copy| !copy.current.load(Ordering::Acquire))
        {
            return Err("Previous clipboard copy acknowledgment or release is pending".into());
        }
        let id = self
            .last_id
            .checked_add(1)
            .ok_or("Clipboard export request IDs exhausted")?;
        let command = Control::Commit {
            id,
            offer: proposal.offer.clone(),
        };
        command.encode().map_err(|e| e.to_string())?;
        self.last_id = id;
        let deadline = Instant::now() + TIMEOUT;
        let (reply, receive) = oneshot::channel();
        self.committing = Some(Pending {
            id,
            deadline,
            proposal: proposal.clone(),
            reply: Some(reply),
        });
        Ok((
            Commit {
                id,
                deadline,
                reply: receive,
            },
            command,
        ))
    }
    pub(crate) fn read_copy(
        &mut self,
        copy: &CommittedCopy,
        format: &str,
    ) -> Result<(Read, Control), String> {
        self.expire();
        if self.version != wire::COPY_VERSION
            || !self.enabled
            || self.disabling
            || !copy.is_current()
            || !Arc::ptr_eq(&copy.disconnected, &self.disconnected)
            || !self.committed.as_ref().is_some_and(|state| {
                state.lease == copy.lease && Arc::ptr_eq(&state.current, &copy.current)
            })
        {
            return Err(CHANGED.into());
        }
        self.start_read(
            copy.offer(),
            format,
            copy.current.clone(),
            Some(copy.lease.clone()),
        )
    }
    pub(crate) fn release_copy(&mut self, copy: &CommittedCopy) -> Result<(), String> {
        if !Arc::ptr_eq(&copy.disconnected, &self.disconnected) {
            return Err("Clipboard copy belongs to another integration connection".into());
        }
        // A stale owner's release never changes a newer copy's independent flag.
        copy.revoke();
        self.expire();
        Ok(())
    }
    pub(crate) fn copy_release(&self) -> Option<Control> {
        let copy = self.committed.as_ref()?;
        (!copy.current.load(Ordering::Acquire) && copy.release_deadline.is_none()).then(|| {
            Control::Release {
                copy: copy.lease.clone(),
            }
        })
    }
    pub(crate) fn copy_release_sent(&mut self, id: &str) {
        if let Some(copy) = self.committed.as_mut().filter(|copy| copy.lease.id == id) {
            copy.release_deadline = Some(Instant::now() + TIMEOUT);
        }
    }
    pub(super) fn abandon_stale_commit(&mut self) {
        if let Some(pending) = self.committing.as_mut()
            && (!pending.proposal.is_current()
                || pending
                    .reply
                    .as_ref()
                    .is_some_and(|reply| reply.is_closed()))
            && let Some(reply) = pending.reply.take()
        {
            let _ = reply.send(Err(CHANGED.into()));
        }
    }
    pub(super) fn invalidate_copy(&mut self, reason: &str, remote: bool) {
        if let Some(copy) = self.committed.take() {
            copy.current.store(false, Ordering::Release);
        }
        self.invalidate_reads(true, reason, remote);
    }
    pub(super) fn accept_commit(&mut self, id: u64, copy: Option<CopyLease>) -> Result<(), String> {
        if self.version != wire::COPY_VERSION {
            return Err("Unnegotiated clipboard copy event".into());
        }
        let pending = self
            .committing
            .take()
            .ok_or("Unrequested clipboard copy acknowledgment")?;
        if pending.id != id {
            return Err("Clipboard copy acknowledgment ID does not match".into());
        }
        let Some(copy) = copy else {
            pending.fail("Remote clipboard copy proposal was rejected");
            return Ok(());
        };
        if copy.offer != pending.proposal.offer {
            return Err("Clipboard copy acknowledgment changed its source".into());
        }
        // The runtime has replaced its old copy, even if this caller abandoned
        // the request. Never retain the old read authority on a rejected write.
        self.invalidate_copy(CHANGED, true);
        let accepted = pending.proposal.is_current()
            && pending
                .reply
                .as_ref()
                .is_some_and(|reply| !reply.is_closed());
        let current = Arc::new(AtomicBool::new(accepted));
        self.committed = Some(State {
            lease: copy.clone(),
            current: current.clone(),
            release_deadline: None,
        });
        if accepted {
            let _ = pending.reply.unwrap().send(Ok(CommittedCopy {
                lease: copy,
                current,
                disconnected: self.disconnected.clone(),
            }));
        } else {
            pending.fail(CHANGED);
        }
        Ok(())
    }
    pub(super) fn accept_copy_revoked(&mut self, id: &str) -> Result<(), String> {
        if self.version != wire::COPY_VERSION {
            return Err("Unnegotiated clipboard copy event".into());
        }
        if self
            .committed
            .as_ref()
            .is_none_or(|copy| copy.lease.id != id)
        {
            return Err("Unknown clipboard copy revocation".into());
        }
        self.invalidate_copy(CHANGED, true);
        Ok(())
    }
}

#[cfg(test)]
#[path = "clipboard_export_copy_tests.rs"]
mod tests;
