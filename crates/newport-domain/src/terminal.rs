/// Product resource bound, independent of sshd's per-connection session limit.
pub const MAX_TERMINALS: usize = 32;

/// Protocols owned by one shared graphical session. This selects the session,
/// independently of an application's preference for X11 or Wayland.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum SharedDisplayMode {
    #[default]
    Mixed,
    Wayland,
}
impl SharedDisplayMode {
    pub fn validate_environment(&self, environment: &SharedGuiEnvironment) -> Result<(), String> {
        environment.validate()?;
        if environment.native_wayland() != (*self == Self::Wayland) {
            return Err("Shared display does not match the requested session mode".into());
        }
        Ok(())
    }
}

#[path = "../../../tools/agent/src/gui_protocol.rs"]
pub mod gui_protocol;
pub use gui_protocol::SharedGuiEnvironment;
