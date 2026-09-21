//! SSH authentication shares the profile Store and its cached vault unlock key.
//! Only Rust can read passwords; they are never part of Config or Snapshot.
use crate::config::Store;
use std::sync::{Arc, OnceLock};
static STORE: OnceLock<Arc<Store>> = OnceLock::new();
pub fn initialize(store: Arc<Store>) {
    let _ = STORE.set(store);
}
pub fn get(id: uuid::Uuid) -> Result<zeroize::Zeroizing<String>, String> {
    STORE
        .get()
        .ok_or("Password vault is unavailable")?
        .password(id)
}
