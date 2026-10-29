/// Product resource bound, independent of sshd's per-connection session limit.
pub const MAX_TERMINALS: usize = 32;

#[path = "../../../tools/agent/src/gui_protocol.rs"]
pub mod gui_protocol;
pub use gui_protocol::SharedGuiEnvironment;
