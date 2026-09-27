//! Compatibility identifiers: do not change persisted values during a rebrand.
pub const APP_NAME: &str = "Porthop";
pub const APP_ID: &str = "ke.ry.porthop";
pub const DATA_DIR_ENV: &str = "PORTHOP_DATA_DIR";
pub const TEST_KEY_ENV: &str = "PORTHOP_TEST_VAULT_KEY_FILE";
pub const INSTANCE_LOCK: &str = "porthop.lock";
pub const VAULT_CLIENT: &[u8] = b"porthop-profiles-v1";
pub const METRICS_SALT: &[u8] = b"porthop-metrics-endpoint-v2\0";
#[cfg(target_os = "macos")]
pub const MAC_VAULT_SERVICE: &str = "com.porthop.profile-vault";
#[cfg(target_os = "macos")]
pub const PROTECTED_KEY_MARKER: &str = "vault-key.storage";
#[cfg(target_os = "windows")]
pub const ACTIVATION_PREFIX: &str = "Local\\Porthop-activate-";
