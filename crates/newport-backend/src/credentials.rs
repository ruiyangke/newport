//! SSH authentication shares the profile Store and its cached vault unlock key.
//! Only Rust can read passwords; they are never part of Config or Snapshot.
use crate::profile::Store;
use std::sync::{Arc, Mutex};
static STORE: Mutex<Option<Arc<Store>>> = Mutex::new(None);

// Tests creating a live backend share the same process-wide SSH credential slot.
#[cfg(test)]
pub(crate) static TEST_OWNER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Process-wide SSH authentication belongs to exactly one live backend.
/// Hold this registration until its SSH workers and requests have drained.
pub struct Registration(Arc<Store>);

pub fn register(store: Arc<Store>) -> Result<Registration, String> {
    let mut current = STORE.lock().map_err(|_| "Password vault is unavailable")?;
    if current.is_some() {
        return Err("A backend already owns the password vault in this process.".into());
    }
    *current = Some(store.clone());
    Ok(Registration(store))
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut current = STORE.lock().unwrap_or_else(|error| error.into_inner());
        if current
            .as_ref()
            .is_some_and(|store| Arc::ptr_eq(store, &self.0))
        {
            *current = None;
        }
    }
}
pub fn get(id: uuid::Uuid) -> Result<zeroize::Zeroizing<String>, String> {
    let store = STORE
        .lock()
        .map_err(|_| "Password vault is unavailable")?
        .clone()
        .ok_or("Password vault is unavailable")?;
    store.password(id)
}
