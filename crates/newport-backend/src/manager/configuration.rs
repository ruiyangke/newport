//! Optimistic configuration edits. Encryption and verification run independently
//! for unrelated operations; only the final revision check/rename/publication is
//! coordinated. Plans are pure patches against the latest committed state.
use super::{Manager, SharedState};
use crate::model::Config;
use std::sync::{Arc, atomic::Ordering};
use uuid::Uuid;

type Password = Option<(Uuid, zeroize::Zeroizing<String>)>;

fn available(manager: &Manager) -> Result<(), String> {
    if manager.configuration_closed {
        return Err("Newport is shutting down.".into());
    }
    if let Some(error) = &manager.load_error {
        return Err(error.clone());
    }
    Ok(())
}

impl SharedState {
    /// `plan` may run again after a competing commit. It must not perform I/O or
    /// mutate live resources. Validate captured owner/preference identities here
    /// as well as applying the patch; a stale operation must fail, not rebase its
    /// authority onto a replacement connection.
    pub(crate) async fn edit_configuration<T, F>(
        self: &Arc<Self>,
        password: Password,
        plan: F,
    ) -> Result<T, String>
    where
        T: Send + 'static,
        F: Fn(&Manager) -> Result<(Config, T), String> + Send + Sync + 'static,
    {
        self.edit_configuration_with_commit(password, plan, |_, result| result)
            .await
    }

    /// Register generation-scoped cleanup as part of publication. The callback
    /// runs exactly once after a successful commit (including an unchanged
    /// config), even if the response waiter has gone away. It must only capture
    /// retired resources or spawn owned cleanup; never await or perform I/O.
    pub(crate) async fn edit_configuration_with_commit<T, U, F, C>(
        self: &Arc<Self>,
        password: Password,
        plan: F,
        committed: C,
    ) -> Result<U, String>
    where
        T: Send + 'static,
        U: Send + 'static,
        F: Fn(&Manager) -> Result<(Config, T), String> + Send + Sync + 'static,
        C: FnOnce(&mut Manager, T) -> U + Send + 'static,
    {
        let plan = Arc::new(plan);
        let committed = Arc::new(std::sync::Mutex::new(Some(committed)));
        let password_id = password.as_ref().map(|(id, _)| *id);
        let password = Arc::new(password);
        loop {
            let (config, revision, store) = {
                let mut manager = self.configuration_control().await;
                available(&manager)?;
                let (config, result) = plan(&manager)?;
                config.validate()?;
                if config == manager.config && password_id.is_none() {
                    let callback = committed.lock().unwrap().take().unwrap();
                    return Ok(callback(&mut manager, result));
                }
                (
                    config,
                    self.configuration_revision.load(Ordering::Acquire),
                    manager.store.clone(),
                )
            };
            let proposed = config.clone();
            let credentials = password.clone();
            let prepared = tokio::task::spawn_blocking(move || {
                store.prepare(
                    &proposed,
                    credentials
                        .as_ref()
                        .as_ref()
                        .map(|(id, secret)| (*id, secret.as_str())),
                )
            })
            .await
            .map_err(|error| format!("Profile preparation task failed: {error}"))??;

            let state = self.clone();
            let plan = plan.clone();
            let callback = committed.clone();
            // Once rename starts, the worker owns publication even if the caller
            // closes its window or cancels its response wait. No async suspension
            // can leave a committed file with the previous in-memory config.
            let committed = tokio::task::spawn_blocking(move || {
                let mut manager = state.blocking_control();
                available(&manager)?;
                if state.configuration_revision.load(Ordering::Acquire) != revision {
                    return Ok(None);
                }
                let (checked, result) = plan(&manager)?;
                if checked != config {
                    return Ok(None);
                }
                let durability_error = prepared.publish()?;
                manager.accept_saved(config, password_id);
                let result = callback.lock().unwrap().take().unwrap()(&mut manager, result);
                if let Some(error) = durability_error {
                    return Err(error);
                }
                Ok::<_, String>(Some(result))
            })
            .await
            .map_err(|error| format!("Profile commit task failed: {error}"))??;
            if let Some(result) = committed {
                return Ok(result);
            }
        }
    }
}

#[cfg(test)]
mod tests;
