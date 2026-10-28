//! Account maintenance belongs to the backend task set, even at zero windows.
use super::*;
use newport_backend::account::{
    LicenseRefusal,
    remote_sessions::{Access, Policy},
};
use std::future::Future;

#[derive(Clone)]
pub(super) struct Guard {
    policy: watch::Receiver<Policy>,
    epoch: u64,
    device: String,
}
impl Guard {
    pub fn new(policy: watch::Receiver<Policy>, device: &str) -> Result<Self, String> {
        let epoch = policy.borrow().epoch;
        let guard = Self {
            policy,
            epoch,
            device: device.into(),
        };
        guard.validate()?;
        Ok(guard)
    }
    fn permits(&self, policy: &Policy, existing: bool) -> bool {
        policy.epoch == self.epoch
            && match &policy.access {
                Access::Renew { device } => device == &self.device,
                Access::Keep { device } => existing && device == &self.device,
                _ => false,
            }
    }
    fn validate(&self) -> Result<(), String> {
        if self.policy.has_changed().is_err() {
            return Err("Remote apps account observer stopped".into());
        }
        self.permits(&self.policy.borrow(), false)
            .then_some(())
            .ok_or_else(|| "Remote apps account access changed".into())
    }
}
pub(super) fn validate(guard: Option<&Guard>) -> Result<(), String> {
    guard.map_or(Ok(()), Guard::validate)
}
pub(super) async fn revoked(guard: Option<&Guard>) {
    if let Some(guard) = guard {
        let mut policy = guard.policy.clone();
        let _ = policy
            .wait_for(|policy| !guard.permits(policy, false))
            .await;
    } else {
        std::future::pending::<()>().await;
    }
}

fn clients(registry: &Registry) -> Vec<Weak<Driver>> {
    registry
        .0
        .clients
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect()
}
fn revoke(registry: &Registry, policy: &Policy) {
    for client in clients(registry) {
        if let Some(client) = client.upgrade()
            && client
                .account
                .as_ref()
                .is_some_and(|guard| !guard.permits(policy, true))
        {
            client.stop();
        }
    }
}
struct StopOnDrop(Registry);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Binding is synchronous, before the backend starts serving requests. The
/// injected reader is the real Account in production and a local fixture in tests.
pub(super) fn maintain<F, Fut>(
    registry: Registry,
    mut policy: watch::Receiver<Policy>,
    read: F,
) -> newport_backend::desktop_integration::GraphicalMaintenance
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<RemoteLicense, LicenseRefusal>> + Send,
{
    assert!(
        registry.0.account.set(policy.clone()).is_ok(),
        "Graphical account already bound"
    );
    let mut changes = registry.0.clients_changed.subscribe();
    // Also close when the backend drops the unpolled worker future.
    let stop = StopOnDrop(registry.clone());
    Box::pin(async move {
        let _stop = stop;
        loop {
            let current = policy.borrow_and_update().clone();
            changes.borrow_and_update();
            if registry.is_closed() {
                return;
            }
            revoke(&registry, &current);
            let targets: Vec<_> = clients(&registry)
                .into_iter()
                .filter(|client| {
                    client.upgrade().is_some_and(|client| {
                        !client.publication.stopped.load(Ordering::Acquire)
                            && client
                                .account
                                .as_ref()
                                .is_some_and(|guard| guard.permits(&current, false))
                    })
                })
                .collect();
            let mut retry = false;
            if !targets.is_empty() {
                let license = tokio::select! {
                    result = policy.changed() => { if result.is_err() { return; } continue; },
                    _ = changes.changed() => continue,
                    license = read() => license,
                };
                match license {
                    Ok(license) => {
                        // Machine/signature checks can block. Retain weak
                        // drivers until each attempt, never while reading the
                        // account. Stop fences the client independently of this
                        // task if a revoke arrives during verification.
                        let mut job = tokio::task::spawn_blocking(move || {
                            for target in targets {
                                let Some(client) = target.upgrade() else {
                                    continue;
                                };
                                if client.publication.stopped.load(Ordering::Acquire)
                                    || validate(client.account.as_ref()).is_err()
                                {
                                    continue;
                                }
                                if let Err(error) =
                                    client.connection.control().renew_license(SessionLicense {
                                        token: license.token.to_string(),
                                        machine: license.machine.clone(),
                                        trusted_time: license.trusted_time,
                                    })
                                {
                                    eprintln!("Shared display license renewal refused: {error}");
                                }
                            }
                        });
                        let mut changed = false;
                        loop {
                            tokio::select! {
                                result = &mut job => {
                                    if result.is_err() { retry = true; }
                                    break;
                                }
                                result = policy.changed() => {
                                    if result.is_err() { return; }
                                    revoke(&registry, &policy.borrow_and_update());
                                    changed = true;
                                }
                                _ = changes.changed() => { changed = true; }
                            }
                            if registry.is_closed() {
                                return;
                            }
                        }
                        if changed {
                            continue;
                        }
                    }
                    Err(refusal) => {
                        // A temporary read failure preserves the currently
                        // signed deadline; plan/seat refusals await policy change.
                        retry = matches!(refusal.reason, "unavailable" | "offline" | "clock");
                    }
                }
            }
            tokio::select! {
                result = policy.changed() => { if result.is_err() { return; } },
                _ = changes.changed() => {},
                _ = tokio::time::sleep(Duration::from_secs(30)), if retry => {},
            }
        }
    })
}
