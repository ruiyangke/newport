//! The HTTPS fixture uses Node's maintained HTTP/TLS parser; Rust owns test orchestration.
//! Keeping this small fixture avoids adding a second HTTP/TLS dependency stack.
use crate::{command, process};
pub fn run() -> anyhow::Result<()> {
    process::run(command("node").arg("scripts/test-git-https.mjs"))
}
