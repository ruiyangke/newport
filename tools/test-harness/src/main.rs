//! Backend test orchestration. Frontend checks are explicit opt-in subprocesses.
use anyhow::{bail, Result};
use std::{path::PathBuf, process::Command};
mod benchmark;
#[cfg(unix)]
mod cockpit;
mod contract;
mod https;
#[cfg(unix)]
mod native;
mod process;
mod remote;
mod ssh;
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}
fn command(name: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(name);
    c.current_dir(root());
    c
}
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let action = args.next().unwrap_or_else(|| "help".into());
    let args: Vec<String> = args.collect();
    match action.as_str() {
        "build-agents" => remote::build_agents(),
        "ssh" => ssh::run(&args),
        "remote" => remote::run(&args),
        "https" => https::run(),
        "runtime" => {
            anyhow::ensure!(
                args.is_empty() || args == ["--docker"],
                "Usage: runtime [--docker]"
            );
            remote::runtime()
        }
        "git-contract" => contract::run(false, &args),
        "git-cli-contract" => contract::run(true, &args),
        "benchmark" => benchmark::run(&args),
        #[cfg(unix)]
        "cockpit" => cockpit::run(),
        #[cfg(unix)]
        "native" => native::run(),
        "help" => {
            println!("newport-tests <build-agents|ssh|remote|https|runtime|git-contract|git-cli-contract|benchmark|cockpit|native>\nContract checks accept --frontend to additionally run the TypeScript decoder.\nRemote checks accept --filter TEST. Benchmark options match git_benchmark.");
            Ok(())
        }
        _ => bail!("Unknown harness command: {action}"),
    }
}
