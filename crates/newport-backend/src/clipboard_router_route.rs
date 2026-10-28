//! Runtime authorization runs beside the integration event pump. UI focus and
//! permission epochs fence local publication immediately; route IO is bounded
//! to one request and never grants authority to an obsolete display snapshot.
use super::{Endpoint, Router};
use crate::{
    agent_runtime::ClipboardRoute,
    clipboard_export::source::Grant,
    clipboard_router::{FocusRequest, display::Focus},
    graphical_session::Identity,
    integration_permissions::Gate,
    server_session::ServerSession,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Weak},
};
use tokio::{
    sync::watch,
    task::{JoinError, JoinSet},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    Applied(u64),
    Retired,
    /// Local identity became obsolete before any control request was sent.
    /// This is cancellation of scheduled work, not a remote retirement claim.
    Superseded,
}
type Reply = Pin<Box<dyn Future<Output = Result<Resolution, String>> + Send>>;
pub(crate) trait Control: Send + Sync {
    fn permissions(&self) -> &Arc<Gate>;
    fn validate(&self, identity: &Identity) -> bool;
    fn apply(&self, identity: Identity, exports: bool, focused: bool) -> Reply;
}
pub(crate) struct Backend {
    owner: Weak<ServerSession>,
    permissions: Arc<Gate>,
}
impl Backend {
    pub(crate) fn bind(owner: &Arc<ServerSession>) -> Arc<dyn Control> {
        Arc::new(Self {
            owner: Arc::downgrade(owner),
            permissions: owner.permissions.clone(),
        })
    }
}
impl Control for Backend {
    fn permissions(&self) -> &Arc<Gate> {
        &self.permissions
    }
    fn validate(&self, identity: &Identity) -> bool {
        self.owner.upgrade().is_some_and(|owner| {
            !owner.is_revoked() && owner.graphical.validate_identity(identity).is_ok()
        })
    }
    fn apply(&self, identity: Identity, exports: bool, focused: bool) -> Reply {
        let owner = self.owner.clone();
        Box::pin(async move {
            let owner = owner
                .upgrade()
                .ok_or("Server session ended before clipboard authorization")?;
            if owner.is_revoked() {
                return Err("Server session ended before clipboard authorization".into());
            }
            if owner.graphical.validate_identity(&identity).is_err() {
                return Ok(Resolution::Superseded);
            }
            let revision = owner
                .agent_runtime
                .clipboard_route_outcome(&identity, exports, focused)
                .await?;
            if owner.is_revoked() {
                return Err("Server session ended during clipboard authorization".into());
            }
            Ok(match revision {
                ClipboardRoute::Applied(revision) => Resolution::Applied(revision),
                ClipboardRoute::Retired => Resolution::Retired,
            })
        })
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Key {
    identity: Identity,
    focus: Option<u64>,
    permission: Option<u64>,
}
pub(crate) struct Completion {
    key: Key,
    focus: Option<FocusRequest>,
    revision: Result<Resolution, String>,
}
pub(crate) enum Work {
    Changed,
    Complete(Result<Completion, JoinError>),
}
pub(crate) struct Routes {
    control: Arc<dyn Control>,
    changes: watch::Receiver<()>,
    permissions: watch::Receiver<crate::agent::control::Permissions>,
    tasks: JoinSet<Completion>,
    last: Option<Identity>,
    applied: Option<Key>,
}
impl Routes {
    pub(crate) fn new(router: &Router, control: Arc<dyn Control>) -> Self {
        Self {
            changes: router.observe_focus(),
            permissions: control.permissions().subscribe(),
            control,
            tasks: JoinSet::new(),
            last: None,
            applied: None,
        }
    }
    pub(crate) fn refresh(&mut self, endpoint: &Endpoint) {
        // A permission change may coalesce with its acknowledgment. Permission
        // tickets, not the notification count, determine accepted-copy validity.
        self.changes.borrow_and_update();
        self.permissions.borrow_and_update();
        let permission = self.control.permissions().clipboard_ticket();
        if permission.is_none() {
            endpoint.withdraw();
        }
        if !self.tasks.is_empty() {
            return;
        }
        let selected = endpoint.focused_display();
        if let Some(focus) = &selected {
            if !self.control.validate(focus.identity()) {
                endpoint.withdraw();
                // Display preparation may publish UI focus before backend
                // Ready. The existing integration timer retries without IO.
                return;
            }
            self.last = Some(focus.identity().clone());
        }
        let Some(identity) = self.last.clone() else {
            return;
        };
        if !self.control.validate(&identity) {
            endpoint.withdraw();
            self.last = None;
            self.applied = None;
            return;
        }
        let key = Key {
            identity,
            focus: permission.and_then(|_| selected.as_ref().map(Focus::epoch)),
            permission,
        };
        if self.applied.as_ref() == Some(&key) {
            return;
        }
        let focus = if key.focus.is_some() {
            let Some(request) = selected
                .as_ref()
                .and_then(|focus| endpoint.begin_display_focus(focus))
            else {
                return;
            };
            Some(request)
        } else {
            endpoint.blur();
            None
        };
        let apply = self.control.apply(
            key.identity.clone(),
            key.permission.is_some(),
            key.focus.is_some(),
        );
        self.tasks.spawn(async move {
            Completion {
                key,
                focus,
                revision: apply.await,
            }
        });
    }
    pub(crate) async fn next(&mut self) -> Work {
        tokio::select! {
            _ = self.changes.changed() => Work::Changed,
            _ = self.permissions.changed() => Work::Changed,
            result = self.tasks.join_next(), if !self.tasks.is_empty() => Work::Complete(result.unwrap()),
        }
    }
    pub(crate) fn complete(&mut self, work: Work, endpoint: &Endpoint) -> Result<(), String> {
        if let Work::Complete(result) = work {
            let completion = result.map_err(|error| error.to_string())?;
            // Permission/protocol/transport failures remain errors even if a
            // local display invalidation overtakes their completion. Only the
            // runtime's negotiated retirement evidence or local cancellation
            // before control IO is benign.
            let outcome = completion.revision?;
            if matches!(outcome, Resolution::Retired | Resolution::Superseded)
                || !self.control.validate(&completion.key.identity)
            {
                endpoint.retire_display(&completion.key.identity);
                self.last = None;
                self.applied = None;
            } else {
                let Resolution::Applied(revision) = outcome else {
                    unreachable!()
                };
                if let Some(focus) = completion.focus
                    && let Some(grant) = completion.key.permission.and_then(|ticket| {
                        Grant::permitted(
                            completion.key.identity.runtime.instance,
                            completion.key.identity.worker,
                            revision,
                            self.control.permissions().clone(),
                            ticket,
                        )
                    })
                {
                    focus.acknowledge(grant);
                }
                self.applied = Some(completion.key);
            }
        }
        self.refresh(endpoint);
        Ok(())
    }
}
