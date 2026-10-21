//! Release identity shared by the desktop, its build script, and release tools.
//! Verification is always available; private-key handling requires `signing`.
pub mod needle;

#[cfg(feature = "signing")]
mod signing;
#[cfg(feature = "signing")]
pub use minisign::SecretKey;
#[cfg(feature = "signing")]
pub use signing::{secret_key, sign, sign_needle};
