//! Desired and acknowledged integration access. Disabling revokes access before
//! the remote acknowledgment; stale acknowledgments never restore permissions.
use crate::agent::control::{Applied, Permissions};
use serde::Serialize;
use std::sync::Mutex;
use tokio::sync::watch;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub desired: Permissions,
    pub effective: Permissions,
    pub error: Option<String>,
    pub clipboard_epoch: u64,
    pub browser_epoch: u64,
    pub session_epoch: u64,
    #[serde(skip)]
    preference_revisions: [u64; 2],
}
/// A host-owned handle to live session authorization. It never acquires the
/// manager's async lock or reads profile storage.
#[derive(Clone)]
pub struct Access {
    sessions: std::sync::Arc<crate::server_session::Registry>,
    instance: uuid::Uuid,
}
impl Access {
    pub(crate) fn new(
        sessions: std::sync::Arc<crate::server_session::Registry>,
        instance: uuid::Uuid,
    ) -> Self {
        Self { sessions, instance }
    }
    pub(crate) fn begin(
        &self,
        id: uuid::Uuid,
        browser: bool,
        enabled: bool,
        expected: &serde_json::Value,
    ) -> Result<
        (
            std::sync::Arc<crate::server_session::ServerSession>,
            Preference,
        ),
        String,
    > {
        self.sessions
            .integration_preference(id, self.instance, expected, browser, enabled)
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Preference {
    pub browser: bool,
    pub enabled: bool,
    revision: u64,
}

pub struct Gate {
    state: Mutex<Snapshot>,
    changes: watch::Sender<Permissions>,
}
pub struct SessionGuard(std::sync::Arc<Gate>, u64);
impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.disconnect_if(self.1);
    }
}
impl Gate {
    pub fn session(self: &std::sync::Arc<Self>) -> SessionGuard {
        let mut state = self.state.lock().unwrap();
        state.session_epoch += 1;
        state.clipboard_epoch += 1;
        state.browser_epoch += 1;
        state.effective = Permissions {
            revision: 0,
            clipboard: false,
            browser: false,
        };
        state.error = None;
        self.changes.send_replace(state.desired);
        SessionGuard(self.clone(), state.session_epoch)
    }
    pub fn new(clipboard: bool, browser: bool) -> Self {
        let desired = Permissions {
            revision: 1,
            clipboard,
            browser,
        };
        Self {
            state: Mutex::new(Snapshot {
                desired,
                effective: Permissions {
                    revision: 0,
                    clipboard: false,
                    browser: false,
                },
                error: None,
                clipboard_epoch: 0,
                browser_epoch: 0,
                session_epoch: 0,
                preference_revisions: [0; 2],
            }),
            changes: watch::channel(desired).0,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<Permissions> {
        self.changes.subscribe()
    }
    pub fn retry(&self) {
        let mut state = self.state.lock().unwrap();
        state.desired.revision += 1;
        state.error = None;
        self.changes.send_replace(state.desired);
    }
    pub fn update(&self, clipboard: bool, browser: bool) {
        let mut state = self.state.lock().unwrap();
        state.preference_revisions[0] += 1;
        state.preference_revisions[1] += 1;
        self.update_locked(&mut state, clipboard, browser);
    }
    pub fn update_feature(&self, browser: bool, enabled: bool) {
        let mut state = self.state.lock().unwrap();
        state.preference_revisions[usize::from(browser)] += 1;
        let clipboard = if browser {
            state.desired.clipboard
        } else {
            enabled
        };
        let browser = if browser {
            enabled
        } else {
            state.desired.browser
        };
        self.update_locked(&mut state, clipboard, browser);
    }
    pub(crate) fn begin_preference(&self, browser: bool, enabled: bool) -> Preference {
        let mut state = self.state.lock().unwrap();
        let index = usize::from(browser);
        state.preference_revisions[index] += 1;
        if !enabled {
            let before = state.desired.revision;
            let clipboard = if browser {
                state.desired.clipboard
            } else {
                false
            };
            let browser_enabled = if browser {
                false
            } else {
                state.desired.browser
            };
            self.update_locked(&mut state, clipboard, browser_enabled);
            // Even an already-off request fences a pending enable/setup save.
            if state.desired.revision == before {
                state.desired.revision += 1;
                self.changes.send_replace(state.desired);
            }
        }
        Preference {
            browser,
            enabled,
            revision: state.preference_revisions[index],
        }
    }
    pub(crate) fn preference_is_current(&self, preference: Preference) -> bool {
        self.state.lock().unwrap().preference_revisions[usize::from(preference.browser)]
            == preference.revision
    }
    pub(crate) fn commit_preference(&self, preference: Preference) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.preference_revisions[usize::from(preference.browser)] != preference.revision {
            return false;
        }
        let clipboard = if preference.browser {
            state.desired.clipboard
        } else {
            preference.enabled
        };
        let browser = if preference.browser {
            preference.enabled
        } else {
            state.desired.browser
        };
        self.update_locked(&mut state, clipboard, browser);
        true
    }
    pub(crate) fn preference_revisions(&self) -> [u64; 2] {
        self.state.lock().unwrap().preference_revisions
    }
    pub(crate) fn commit_setup(
        &self,
        revision: u64,
        preferences: [u64; 2],
        clipboard: bool,
        browser: bool,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.desired.revision != revision || state.preference_revisions != preferences {
            return false;
        }
        state.preference_revisions[0] += 1;
        state.preference_revisions[1] += 1;
        self.update_locked(&mut state, clipboard, browser);
        true
    }

    fn update_locked(&self, state: &mut Snapshot, clipboard: bool, browser: bool) {
        if state.desired.clipboard == clipboard && state.desired.browser == browser {
            return;
        }
        state.desired = Permissions {
            revision: state.desired.revision + 1,
            clipboard,
            browser,
        };
        if !clipboard {
            state.clipboard_epoch += 1;
        }
        if !browser {
            state.browser_epoch += 1;
        }
        state.effective.clipboard &= clipboard;
        state.effective.browser &= browser;
        state.error = None;
        self.changes.send_replace(state.desired);
    }
    #[cfg(test)]
    pub fn acknowledge(&self, applied: Applied) -> bool {
        self.acknowledge_for(self.snapshot().session_epoch, applied)
    }
    pub fn acknowledge_for(&self, epoch: u64, applied: Applied) -> bool {
        let mut state = self.state.lock().unwrap();
        let p = applied.permissions;
        if epoch != state.session_epoch
            || p.revision != state.desired.revision
            || p.clipboard && !state.desired.clipboard
            || p.browser && !state.desired.browser
        {
            return false;
        }
        // An acknowledged denial is a revocation even when the desired
        // preference remains enabled. A later successful retry must not make
        // tickets issued before that denial valid again.
        if state.effective.clipboard && !p.clipboard {
            state.clipboard_epoch += 1;
        }
        if state.effective.browser && !p.browser {
            state.browser_epoch += 1;
        }
        state.effective = p;
        state.error = applied.error;
        true
    }
    pub fn disconnected(&self, error: Option<String>) {
        let mut state = self.state.lock().unwrap();
        state.session_epoch += 1;
        state.clipboard_epoch += 1;
        state.browser_epoch += 1;
        state.effective.clipboard = false;
        state.effective.browser = false;
        state.effective.revision = 0;
        if error.is_some() {
            state.error = error;
        }
        self.changes.send_replace(state.desired);
    }
    pub(crate) fn disconnect_if(&self, epoch: u64) {
        let mut state = self.state.lock().unwrap();
        if state.session_epoch != epoch {
            return;
        }
        state.session_epoch += 1;
        state.clipboard_epoch += 1;
        state.browser_epoch += 1;
        state.effective = Permissions {
            revision: 0,
            clipboard: false,
            browser: false,
        };
        self.changes.send_replace(state.desired);
    }
    /// Permission is checked at execution, including queued UI callbacks. Hold
    /// the guard for the local access so revocation and access have a clear order.
    pub fn clipboard<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        let state = self.state.lock().unwrap();
        state.effective.clipboard.then(action)
    }
    pub fn browser<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        let state = self.state.lock().unwrap();
        state.effective.browser.then(action)
    }
    pub fn clipboard_ticket(&self) -> Option<u64> {
        let state = self.state.lock().unwrap();
        state.effective.clipboard.then_some(state.clipboard_epoch)
    }
    pub fn clipboard_at<T>(&self, ticket: u64, action: impl FnOnce() -> T) -> Option<T> {
        let state = self.state.lock().unwrap();
        (state.effective.clipboard && state.clipboard_epoch == ticket).then(action)
    }
    pub fn browser_ticket(&self) -> Option<u64> {
        let state = self.state.lock().unwrap();
        state.effective.browser.then_some(state.browser_epoch)
    }
    pub fn browser_at<T>(&self, ticket: u64, action: impl FnOnce() -> T) -> Option<T> {
        let state = self.state.lock().unwrap();
        (state.effective.browser && state.browser_epoch == ticket).then(action)
    }
    pub fn clipboard_enabled(&self) -> bool {
        self.state.lock().unwrap().effective.clipboard
    }
    pub fn browser_enabled(&self) -> bool {
        self.state.lock().unwrap().effective.browser
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ack(gate: &Gate) {
        assert!(gate.acknowledge(Applied {
            permissions: gate.snapshot().desired,
            error: None
        }));
    }
    #[test]
    fn acknowledged_denial_permanently_retires_tickets_before_retry() {
        let gate = Gate::new(true, true);
        ack(&gate);
        let clipboard = gate.clipboard_ticket().unwrap();
        let browser = gate.browser_ticket().unwrap();
        let mut permissions = gate.snapshot().desired;
        permissions.clipboard = false;
        permissions.browser = false;
        assert!(gate.acknowledge(Applied {
            permissions,
            error: Some("temporarily unavailable".into())
        }));
        assert!(gate.clipboard_at(clipboard, || ()).is_none());
        assert!(gate.browser_at(browser, || ()).is_none());
        gate.retry();
        ack(&gate);
        assert!(gate.clipboard_enabled());
        assert!(gate.browser_enabled());
        assert!(gate.clipboard_at(clipboard, || ()).is_none());
        assert!(gate.browser_at(browser, || ()).is_none());
    }
    #[test]
    fn preference_intents_fence_delayed_enables_without_blocking_other_features() {
        let gate = Gate::new(false, true);
        ack(&gate);
        let enable = gate.begin_preference(false, true);
        let other = gate.begin_preference(true, true);
        let before = gate.snapshot().desired.revision;
        let disable = gate.begin_preference(false, false);
        assert!(
            gate.snapshot().desired.revision > before,
            "already-off still fences setup"
        );
        assert!(!gate.commit_preference(enable));
        assert!(
            gate.commit_preference(other),
            "independent feature stays valid"
        );
        assert!(gate.commit_preference(disable));
        ack(&gate);
        assert!(!gate.clipboard_enabled());
        assert!(gate.browser_enabled());
        let fresh = gate.begin_preference(false, true);
        assert!(gate.commit_preference(fresh));
        assert!(
            !gate.clipboard_enabled(),
            "enable still requires acknowledgement"
        );
        ack(&gate);
        assert!(gate.clipboard_enabled());
    }
    #[test]
    fn setup_save_cannot_commit_after_new_preference_intent() {
        let gate = Gate::new(true, true);
        let revision = gate.snapshot().desired.revision;
        let preferences = gate.preference_revisions();
        let disable = gate.begin_preference(false, false);
        assert!(!gate.commit_setup(revision, preferences, true, true));
        assert!(gate.commit_preference(disable));
        let revision = gate.snapshot().desired.revision;
        let preferences = gate.preference_revisions();
        gate.begin_preference(true, true);
        assert!(!gate.commit_setup(revision, preferences, true, true));
    }

    #[test]
    fn disabled_access_is_immediate_and_late_ack_cannot_restore_it() {
        let gate = Gate::new(true, true);
        assert!(gate.clipboard(|| "data").is_none());
        ack(&gate);
        let stale = gate.snapshot().desired;
        gate.update(false, true);
        assert!(gate.clipboard(|| "data").is_none());
        assert!(gate.browser(|| "open").is_some());
        assert!(!gate.acknowledge(Applied {
            permissions: stale,
            error: None
        }));
        gate.update(true, true);
        assert!(!gate.clipboard_enabled());
        ack(&gate);
        assert!(gate.clipboard_enabled());
        gate.disconnected(None);
        assert!(!gate.browser_enabled());
        assert!(!gate.clipboard_enabled());
        assert!(gate.snapshot().desired.browser);
    }
    #[test]
    fn acknowledgement_cannot_grant_unrequested_access() {
        let gate = Gate::new(false, true);
        let mut invalid = gate.snapshot().desired;
        invalid.clipboard = true;
        assert!(!gate.acknowledge(Applied {
            permissions: invalid,
            error: None
        }));
        assert!(!gate.clipboard_enabled());
    }
    #[test]
    fn previous_connection_ack_and_guard_cannot_affect_replacement() {
        let gate = std::sync::Arc::new(Gate::new(true, true));
        let old = gate.session();
        let epoch = gate.snapshot().session_epoch;
        let permissions = gate.snapshot().desired;
        let replacement = gate.session();
        assert!(!gate.acknowledge_for(
            epoch,
            Applied {
                permissions,
                error: None
            }
        ));
        ack(&gate);
        drop(old);
        assert!(gate.clipboard_enabled());
        drop(replacement);
        assert!(!gate.clipboard_enabled());
    }
}
