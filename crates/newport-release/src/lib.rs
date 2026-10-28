//! Release identity shared by the desktop, its build script, and release tools.
//! Verification is always available; private-key handling requires `signing`.
#[path = "../../../tools/agent/src/product.rs"]
pub mod agent_product;
pub mod desktop;
pub mod needle;

#[cfg(feature = "signing")]
mod signing;
#[cfg(feature = "signing")]
pub use minisign::SecretKey;
#[cfg(feature = "signing")]
pub use signing::agent_export::{sign_full_agent_export, verify_full_agent_export};
#[cfg(feature = "signing")]
pub use signing::desktop_agent;
#[cfg(feature = "signing")]
pub use signing::{secret_key, sign, sign_agent, sign_needle};
