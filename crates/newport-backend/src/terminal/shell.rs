//! A terminal tab can own several successive SSH shells. GUI approval belongs
//! to exactly one shell; reconnecting never transfers a waiting command.
use super::display::{Prepared, Source};
use newport_domain::terminal::SharedGuiEnvironment;
use newport_domain::terminal::gui_protocol::attachment;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::watch;
use uuid::Uuid;

pub(crate) const STALE: &str =
    "The originating shell ended. Run the command again in the current terminal.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub terminal: Uuid,
    pub epoch: Uuid,
}
impl Identity {
    pub fn new(terminal: Uuid) -> Self {
        Self {
            terminal,
            epoch: Uuid::new_v4(),
        }
    }
}
#[derive(Default)]
pub(crate) struct Registry(Mutex<HashMap<Uuid, Weak<State>>>);
impl Registry {
    /// Observe only this shell's already-retained display. Never invoke its
    /// configured source, acquire demand, or adopt a replacement generation.
    pub(crate) fn check_attachment(
        &self,
        request: &attachment::Check,
        session: Uuid,
    ) -> Result<AttachmentCheck, String> {
        request.validate().map_err(|error| error.to_string())?;
        let rejected = |reason| {
            Ok(AttachmentCheck {
                candidate: Err(reason),
            })
        };
        if request.session != session {
            return rejected(attachment::Reason::SessionMismatch);
        }
        let shell = match self.capture(request.terminal, Some(request.epoch)) {
            Ok(shell) => shell,
            Err(_) => return rejected(attachment::Reason::StaleShell),
        };
        let state = match shell.state() {
            Ok(state) => state,
            Err(_) => return rejected(attachment::Reason::StaleShell),
        };
        let Some(display) = state.display()? else {
            return rejected(attachment::Reason::NoSharedDisplay);
        };
        let Prepared::Shared(lease) = display.as_ref() else {
            return rejected(attachment::Reason::NoSharedDisplay);
        };
        let identity = lease.identity()?;
        let environment = lease.environment()?;
        if identity.session != session || environment.session != session {
            return rejected(attachment::Reason::SessionMismatch);
        }
        if !request.endpoints.matches(&environment) {
            return rejected(attachment::Reason::EndpointMismatch);
        }
        Ok(AttachmentCheck {
            candidate: Ok((shell, display, identity, request.clone())),
        })
    }
    pub fn register(&self, id: Uuid) -> Result<Arc<State>, String> {
        let mut entries = self.0.lock().unwrap();
        entries.retain(|_, entry| {
            entry
                .upgrade()
                .is_some_and(|entry| !entry.control.lock().unwrap().closed)
        });
        if entries.contains_key(&id) {
            return Err("Terminal session already exists.".into());
        }
        if entries.len() >= newport_domain::terminal::MAX_TERMINALS {
            return Err("Close an existing terminal before opening another.".into());
        }
        let state = State::new(id);
        entries.insert(id, Arc::downgrade(&state));
        Ok(state)
    }
    pub fn capture(&self, id: Uuid, epoch: Option<Uuid>) -> Result<Fence, String> {
        let state = self
            .0
            .lock()
            .unwrap()
            .get(&id)
            .and_then(Weak::upgrade)
            .ok_or(STALE)?;
        let current = state.current.subscribe();
        let identity = (*current.borrow()).ok_or(STALE)?;
        if epoch.is_some_and(|epoch| identity.epoch != epoch) {
            return Err(STALE.into());
        }
        Ok(Fence {
            current,
            identity,
            state: Arc::downgrade(&state),
        })
    }
}
/// Captures the initial comparison so the caller can recheck immediately before
/// replying. Holding the Arc adds no display demand and cannot transfer a lease.
pub(crate) struct AttachmentCheck {
    candidate: Result<
        (
            Fence,
            Arc<Prepared>,
            crate::graphical_session::Identity,
            attachment::Check,
        ),
        attachment::Reason,
    >,
}
impl AttachmentCheck {
    pub(crate) fn finish(&self, session: Uuid) -> Result<attachment::Outcome, String> {
        let not_current = |reason| Ok(attachment::Outcome::NotCurrent { reason });
        let (shell, display, identity, request) = match &self.candidate {
            Ok(candidate) => candidate,
            Err(reason) => return not_current(*reason),
        };
        if request.session != session {
            return not_current(attachment::Reason::SessionMismatch);
        }
        if shell.validate().is_err() {
            return not_current(attachment::Reason::StaleShell);
        }
        let state = shell.state()?;
        let current = state.display()?;
        if !current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, display))
        {
            return not_current(attachment::Reason::NoSharedDisplay);
        }
        let Prepared::Shared(lease) = display.as_ref() else {
            unreachable!("captured shared display");
        };
        // Validation errors are preserved, never classified as benign stale
        // state by inspecting their text. This includes permission/transport.
        if &lease.identity()? != identity {
            return Err("Graphical attachment identity changed".into());
        }
        let environment = lease.environment()?;
        if environment.session != session || !request.endpoints.matches(&environment) {
            return not_current(attachment::Reason::EndpointMismatch);
        }
        if shell.validate().is_err() {
            return not_current(attachment::Reason::StaleShell);
        }
        Ok(attachment::Outcome::Current {})
    }
}
pub(crate) struct State {
    id: Uuid,
    control: Mutex<Control>,
    current: watch::Sender<Option<Identity>>,
    displays: watch::Sender<()>,
}
#[derive(Default)]
struct Control {
    closed: bool,
    source: Option<Source>,
    display: Option<Arc<Prepared>>,
}
impl State {
    pub(super) fn new(id: Uuid) -> Arc<Self> {
        Arc::new(Self {
            id,
            control: Mutex::new(Control::default()),
            current: watch::channel(None).0,
            displays: watch::channel(()).0,
        })
    }
    pub fn lifetime(self: &Arc<Self>) -> Lifetime {
        Lifetime(self.clone())
    }
    pub fn begin(self: &Arc<Self>) -> Result<Attempt, String> {
        let control = self.control.lock().unwrap();
        if control.closed || self.current.borrow().is_some() {
            return Err(STALE.into());
        }
        let identity = Identity::new(self.id);
        self.current.send_replace(Some(identity));
        Ok(Attempt {
            state: self.clone(),
            identity,
        })
    }
    pub fn configure(&self, source: Option<Source>) {
        self.control.lock().unwrap().source = source;
    }
    pub fn initialize(&self, display: Arc<Prepared>) -> Result<(), String> {
        let mut control = self.control.lock().unwrap();
        if control.closed || control.display.is_some() {
            return Err(STALE.into());
        }
        control.display = Some(display);
        self.displays.send_replace(());
        Ok(())
    }
    /// Drop a display that failed this shell's startup, so a GUI app launched
    /// later can start another. Any other retained display is kept.
    pub fn forget_display(&self, failed: &Arc<Prepared>) {
        let forgotten = {
            let mut control = self.control.lock().unwrap();
            if !control
                .display
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, failed))
            {
                return;
            }
            self.displays.send_replace(());
            control.display.replace(Arc::new(Prepared::None))
        };
        // Releasing a graphical lease can call its owner; keep that outside the lock.
        drop(forgotten);
    }
    /// The environment of the display this shell holds. One that ended, as
    /// when Needle was removed, is let go: each launch gets the display's
    /// environment, so the next one can start another.
    pub fn held_environment(&self, session: Uuid) -> Result<Option<SharedGuiEnvironment>, String> {
        let held = self.display()?;
        if let Some(display) = &held
            && matches!(display.as_ref(), Prepared::Shared(lease) if lease.ended())
        {
            self.forget_display(display);
            return Ok(None);
        }
        Fence::environment(held.as_ref(), session)
    }
    pub fn display(&self) -> Result<Option<Arc<Prepared>>, String> {
        let control = self.control.lock().unwrap();
        if control.closed {
            return Err(STALE.into());
        }
        Ok(control.display.clone())
    }
    pub fn close(&self) {
        let (display, source) = {
            let mut control = self.control.lock().unwrap();
            control.closed = true;
            self.current.send_replace(None);
            self.displays.send_replace(());
            (control.display.take(), control.source.take())
        };
        // Releasing a graphical lease can call its owner; keep that outside
        // the shell lock, including when a terminal is cancelled synchronously.
        drop((display, source));
    }
    /// Includes a display adopted after this shell became ready. The first
    /// failure is terminal for its GUI environment, not for the SSH shell.
    pub async fn gui_failure(&self) -> String {
        let mut changes = self.displays.subscribe();
        loop {
            let display = match self.display() {
                Ok(display) => display,
                Err(error) => return error,
            };
            if let Some(Prepared::Shared(lease)) = display.as_deref() {
                tokio::select! {
                    error = lease.unavailable() => return error,
                    result = changes.changed() => if result.is_err() { return STALE.into(); },
                }
            } else if changes.changed().await.is_err() {
                return STALE.into();
            }
        }
    }
}
pub(crate) struct Lifetime(Arc<State>);
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.0.close();
    }
}
pub(crate) struct Attempt {
    state: Arc<State>,
    pub identity: Identity,
}
impl Drop for Attempt {
    fn drop(&mut self) {
        let _control = self.state.control.lock().unwrap();
        if *self.state.current.borrow() == Some(self.identity) {
            self.state.current.send_replace(None);
        }
    }
}
#[derive(Clone)]
pub(crate) struct Fence {
    state: Weak<State>,
    current: watch::Receiver<Option<Identity>>,
    identity: Identity,
}
impl Fence {
    pub fn validate(&self) -> Result<(), String> {
        if self.current.has_changed().is_err() || *self.current.borrow() != Some(self.identity) {
            Err(STALE.into())
        } else {
            Ok(())
        }
    }
    fn state(&self) -> Result<Arc<State>, String> {
        self.validate()?;
        self.state.upgrade().ok_or_else(|| STALE.into())
    }
    fn environment(
        display: Option<&Arc<Prepared>>,
        session: Uuid,
    ) -> Result<Option<SharedGuiEnvironment>, String> {
        match display.map(Arc::as_ref) {
            None | Some(Prepared::None) => Ok(None),
            Some(Prepared::Shared(lease)) => {
                let environment = lease.environment()?;
                if environment.session != session {
                    return Err("The display belongs to another server session.".into());
                }
                Ok(Some(environment))
            }
        }
    }
    pub(crate) fn retain(
        &self,
        display: Prepared,
        session: Uuid,
    ) -> Result<SharedGuiEnvironment, String> {
        let display = Arc::new(display);
        let environment =
            Self::environment(Some(&display), session)?.ok_or("A shared display is required")?;
        let state = self.state()?;
        let mut control = state.control.lock().unwrap();
        self.validate()?;
        if control.closed {
            return Err(STALE.into());
        }
        if let Some(current) = Self::environment(control.display.as_ref(), session)? {
            let Some(Prepared::Shared(existing)) = control.display.as_deref() else {
                unreachable!("only a shared display has an environment");
            };
            let Prepared::Shared(candidate) = display.as_ref() else {
                unreachable!("candidate was validated above");
            };
            if existing.identity()? != candidate.identity()? || current != environment {
                return Err("The terminal is already attached to another display.".into());
            }
            return Ok(current);
        }
        control.display = Some(display);
        state.displays.send_replace(());
        Ok(environment)
    }
    pub async fn attach(
        &self,
        owner: &Arc<crate::server_session::ServerSession>,
    ) -> Result<SharedGuiEnvironment, String> {
        let state = self.state()?;
        let source = state
            .control
            .lock()
            .unwrap()
            .source
            .clone()
            .ok_or("Shared GUI handoff is unavailable for this terminal")?;
        // Recheck account policy even when this terminal already holds a display.
        let factory = source().await?;
        factory.validate()?;
        self.validate()?;
        if owner.is_revoked() {
            return Err(STALE.into());
        }
        let session = owner.integration_session_id();
        if let Some(environment) = state.held_environment(session)? {
            return Ok(environment);
        }
        let lease = owner
            .acquire_graphical(crate::graphical_session::Demand::Terminal, factory)
            .await?;
        self.retain(Prepared::Shared(lease), session)
    }
    pub async fn ended(&mut self) {
        while self.validate().is_ok() {
            if self.current.changed().await.is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn reconnect_and_reused_tab_ids_cannot_adopt_old_approval() {
        let registry = Registry::default();
        let id = Uuid::new_v4();
        let shell = registry.register(id).unwrap();
        let lifetime = shell.lifetime();
        assert!(registry.capture(id, None).is_err());
        let first = shell.begin().unwrap();
        let epoch = first.identity.epoch;
        let mut approval = registry.capture(id, Some(epoch)).unwrap();
        assert!(registry.capture(id, Some(Uuid::new_v4())).is_err());
        assert!(registry.capture(Uuid::new_v4(), Some(epoch)).is_err());
        approval.validate().unwrap();
        drop(first);
        approval.ended().await;
        assert!(approval.validate().is_err());
        let second = shell.begin().unwrap();
        assert_ne!(second.identity.epoch, epoch);
        assert!(registry.capture(id, Some(epoch)).is_err());
        assert!(approval.validate().is_err());
        let mut current = registry.capture(id, Some(second.identity.epoch)).unwrap();
        drop(lifetime);
        current.ended().await;
        assert!(current.validate().is_err());
        assert!(shell.begin().is_err());
        let replacement = registry.register(id).unwrap();
        let replacement_attempt = replacement.begin().unwrap();
        drop(second); // A late destructor belongs only to its original state.
        registry
            .capture(id, Some(replacement_attempt.identity.epoch))
            .unwrap()
            .validate()
            .unwrap();
        assert!(current.validate().is_err());
    }
    #[test]
    fn dropping_an_unpolled_terminal_future_revokes_its_registration() {
        let registry = Registry::default();
        let id = Uuid::new_v4();
        let shell = registry.register(id).unwrap();
        let lifetime = shell.lifetime();
        let task = async move {
            let _lifetime = lifetime;
            std::future::pending::<()>().await;
        };
        drop(task);
        assert!(shell.begin().is_err());
        assert!(registry.capture(id, None).is_err());
        assert!(registry.register(id).is_ok());
    }
    #[test]
    fn registry_is_owner_scoped_bounded_and_releases_closed_slots() {
        let a = Registry::default();
        let b = Registry::default();
        let mut states = Vec::new();
        for _ in 0..newport_domain::terminal::MAX_TERMINALS {
            let id = Uuid::new_v4();
            let state = a.register(id).unwrap();
            let attempt = state.begin().unwrap();
            assert!(b.capture(id, Some(attempt.identity.epoch)).is_err());
            assert!(a.register(id).is_err());
            states.push((state, attempt));
        }
        assert!(a.register(Uuid::new_v4()).is_err());
        states[0].0.close();
        assert!(a.register(Uuid::new_v4()).is_ok());
    }
}
