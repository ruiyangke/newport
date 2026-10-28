//! UI focus is scoped to a live display and one presenter generation. UI blur
//! revokes local write authority synchronously, before asynchronous runtime IO.
//! No native API, protocol operation or window rendering runs under this lock.
use super::*;
use crate::graphical_session::Identity;
use std::sync::atomic::AtomicU64;

pub(super) struct Binding {
    identity: Identity,
    active: AtomicBool,
    presenter: AtomicU64,
}
pub(super) struct Selection {
    binding: Arc<Binding>,
    presenter: u64,
    epoch: u64,
}

/// Owned by the graphical driver, independently of window presentation. Drop
/// retires this display's pending and accepted copies, including after blur.
pub struct Display {
    inner: Weak<Inner>,
    binding: Arc<Binding>,
}

/// One UI attachment. This weak handle cannot keep a display/transport alive.
/// A replacement presenter invalidates every callback from the previous one.
pub struct Presenter {
    inner: Weak<Inner>,
    binding: Weak<Binding>,
    id: u64,
}

/// A snapshot of the selected display. Use it to bind runtime authorization;
/// a delayed consumer cannot use it after blur, refocus or replacement.
pub struct Focus {
    inner: Weak<Inner>,
    binding: Arc<Binding>,
    epoch: u64,
}
impl Focus {
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn identity(&self) -> &Identity {
        &self.binding.identity
    }
    pub fn is_current(&self) -> bool {
        self.inner.upgrade().is_some_and(|inner| {
            !inner.closed.load(Ordering::Acquire) && self.current(&inner.state.lock().unwrap())
        })
    }
    fn current(&self, state: &State) -> bool {
        self.binding.active.load(Ordering::Acquire)
            && state.selected.as_ref().is_some_and(|selected| {
                selected.epoch == self.epoch && Arc::ptr_eq(&selected.binding, &self.binding)
            })
    }
}

impl Router {
    /// Call after validating the shared display handshake. Registration grants
    /// no export permission; a matching live runtime route must acknowledge it.
    pub fn register_display(&self, identity: Identity) -> Option<Display> {
        let mut state = self.0.state.lock().unwrap();
        if self.0.closed.load(Ordering::Acquire)
            || identity.runtime.instance.is_nil()
            || identity.session.is_nil()
            || identity.worker.is_nil()
            || identity.display_generation == 0
            || !crate::agent_runtime::protocol::valid_build(&identity.runtime.build_sha256)
        {
            return None;
        }
        state.displays.retain(|display| {
            display
                .upgrade()
                .is_some_and(|d| d.active.load(Ordering::Acquire))
        });
        if state.displays.iter().filter_map(Weak::upgrade).any(|d| {
            d.identity.runtime.instance == identity.runtime.instance
                && (d.identity.worker == identity.worker || d.identity.session == identity.session)
        }) {
            return None;
        }
        let binding = Arc::new(Binding {
            identity,
            active: AtomicBool::new(true),
            presenter: AtomicU64::new(0),
        });
        state.displays.push(Arc::downgrade(&binding));
        Some(Display {
            inner: Arc::downgrade(&self.0),
            binding,
        })
    }
    /// Subscribe before reading the endpoint's selected display. Notifications
    /// carry no authority and may coalesce; always inspect the current snapshot.
    pub fn observe_focus(&self) -> tokio::sync::watch::Receiver<()> {
        self.0.focus_changed.subscribe()
    }
}

impl Display {
    pub fn presenter(&self) -> Option<Presenter> {
        let inner = self.inner.upgrade()?;
        let mut state = inner.state.lock().unwrap();
        if inner.closed.load(Ordering::Acquire) || !self.binding.active.load(Ordering::Acquire) {
            return None;
        }
        let id = state.next.checked_add(1)?;
        state.next = id;
        self.binding.presenter.store(id, Ordering::Release);
        if state
            .selected
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(&s.binding, &self.binding))
        {
            deselect(&inner, &mut state);
        }
        Some(Presenter {
            inner: self.inner.clone(),
            binding: Arc::downgrade(&self.binding),
            id,
        })
    }
    pub fn revoke(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut state = inner.state.lock().unwrap();
        if !self.binding.active.swap(false, Ordering::AcqRel) {
            return;
        }
        if state
            .selected
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(&s.binding, &self.binding))
        {
            deselect(&inner, &mut state);
        }
        let identity = &self.binding.identity;
        let owns = |runtime: &str, worker: &str| {
            runtime == identity.runtime.instance.to_string()
                && worker == identity.worker.to_string()
        };
        if state
            .focus
            .as_ref()
            .is_some_and(|f| owns(f.grant.runtime(), f.grant.worker()))
        {
            lose_focus(&mut state);
        }
        if state
            .pending
            .as_ref()
            .is_some_and(|p| owns(p.grant.runtime(), p.grant.worker()))
        {
            state.pending = None;
        }
        if let Some(accepted) = state.accepted.as_ref().filter(|a| {
            let offer = &a.source.offer().offer;
            matches!(&offer.origin, crate::agent_runtime::protocol::clipboard::Origin::Native { worker, .. }
                if owns(&offer.runtime, worker))
        }) {
            accepted.source.revoke();
        }
    }
}
impl Drop for Display {
    fn drop(&mut self) {
        self.revoke();
    }
}
impl Presenter {
    /// Report application-active AND any-window-active for this shared display.
    /// Repeated true reports (including a switch between its apps) retain the
    /// same epoch. Old presenters cannot steal or clear a replacement's focus.
    pub fn set_focused(&self, focused: bool) -> bool {
        let (Some(inner), Some(binding)) = (self.inner.upgrade(), self.binding.upgrade()) else {
            return false;
        };
        let mut state = inner.state.lock().unwrap();
        if inner.closed.load(Ordering::Acquire)
            || !binding.active.load(Ordering::Acquire)
            || binding.presenter.load(Ordering::Acquire) != self.id
        {
            return false;
        }
        let selected = state
            .selected
            .as_ref()
            .is_some_and(|s| s.presenter == self.id && Arc::ptr_eq(&s.binding, &binding));
        if focused && !selected {
            let Some(epoch) = state.next.checked_add(1) else {
                return false;
            };
            state.next = epoch;
            state.focusing = None;
            lose_focus(&mut state);
            state.selected = Some(Selection {
                binding,
                presenter: self.id,
                epoch,
            });
            inner.focus_changed.send_replace(());
        } else if !focused && selected {
            deselect(&inner, &mut state);
        }
        true
    }
}
impl Drop for Presenter {
    fn drop(&mut self) {
        self.set_focused(false);
    }
}
fn deselect(inner: &Inner, state: &mut State) {
    state.selected = None;
    state.focusing = None;
    lose_focus(state);
    inner.focus_changed.send_replace(());
}

impl Endpoint {
    /// Positive runtime retirement evidence fences only this exact display.
    /// Reuse normal display revocation so a different display's pending or
    /// accepted copy and focus are left intact, even on the same integration.
    pub(crate) fn retire_display(&self, identity: &Identity) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        if !self.binding.current() || self.binding.runtime != identity.runtime.instance.to_string()
        {
            return;
        }
        let binding = inner
            .state
            .lock()
            .unwrap()
            .displays
            .iter()
            .filter_map(Weak::upgrade)
            .find(|binding| binding.identity == *identity);
        if let Some(binding) = binding {
            // This temporary handle retires the binding on drop. No lock is
            // held while Drop performs the existing per-origin copy fencing.
            drop(Display {
                inner: self.inner.clone(),
                binding,
            });
        }
    }
    pub fn focused_display(&self) -> Option<Focus> {
        let inner = self.inner.upgrade()?;
        let state = inner.state.lock().unwrap();
        let selected = state.selected.as_ref()?;
        if inner.closed.load(Ordering::Acquire)
            || !self.binding.current()
            || !selected.binding.active.load(Ordering::Acquire)
            || self.binding.runtime != selected.binding.identity.runtime.instance.to_string()
        {
            return None;
        }
        Some(Focus {
            inner: self.inner.clone(),
            binding: selected.binding.clone(),
            epoch: selected.epoch,
        })
    }
    /// Reserve before sending runtime control. Rechecks the snapshot under the
    /// same lock as UI blur, preventing delayed async focus work from winning.
    pub fn begin_display_focus(&self, focus: &Focus) -> Option<FocusRequest> {
        let inner = self.inner.upgrade()?;
        let mut state = inner.state.lock().unwrap();
        if !self.inner.ptr_eq(&focus.inner)
            || !focus.current(&state)
            || self.binding.runtime != focus.identity().runtime.instance.to_string()
        {
            return None;
        }
        self.reserve_focus(&inner, &mut state, focus.identity().worker)
    }
}
