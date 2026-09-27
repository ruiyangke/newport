//! Porthop → Newport migration entry points and compatibility policy.
//!
//! Startup order: resolve profile, acquire the shared lock, prepare UI data,
//! load the vault through credentials::load, then finish startup registration.
//! Never replace existing Newport data or discard the original credential.
pub mod credentials;
pub mod legacy;
mod profile;
mod startup;
mod ui;
pub use profile::{data_directory_override, import_profiles, profile_directory, remove_plaintext};
pub use startup::migrate as finish_startup;
pub use ui::prepare as prepare_ui;
