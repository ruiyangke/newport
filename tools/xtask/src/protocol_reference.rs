//! Deterministic protocol documentation from the crate's compiled descriptors.
use anyhow::Result;

pub fn run(check: bool) -> Result<()> {
    let mut cargo = crate::command("cargo");
    cargo.args([
        "run",
        "--offline",
        "--locked",
        "-p",
        "newport-protocol",
        "--features",
        "protobuf",
        "--example",
        "protocol_reference",
        "--",
    ]);
    if check {
        cargo.arg("--check");
    }
    crate::util::run(crate::util::without_release_secrets(&mut cargo))
}
