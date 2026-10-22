//! Newport build, packaging, release and test tooling. See README.md.
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::{ffi::OsString, path::PathBuf};

mod artifact;
mod benchmark;
mod build_info;
mod check;
mod contract;
mod desktop;
mod dev_run;
mod fixture_suites;
mod ghostty;
mod grammars;
mod https;
mod keychain;
mod package;
mod pdfium;
mod pe;
mod process;
mod release;
mod remote;
mod remote_apps;
mod signing;
mod ssh;
mod util;

pub use util::{command, root};

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    bin_name = "cargo xtask",
    about = "Newport build, packaging, release and test tooling"
)]
struct Cli {
    #[command(subcommand)]
    command: Task,
}

/// `--release` and `--target`, shared by build-like commands.
#[derive(Args, Clone, Debug, Default)]
pub struct BuildArgs {
    /// Use the release profile.
    #[arg(long)]
    pub release: bool,
    /// Target triple; defaults to CARGO_BUILD_TARGET, then the host.
    #[arg(long)]
    pub target: Option<String>,
}

impl BuildArgs {
    /// The explicit target, else `CARGO_BUILD_TARGET`.
    pub fn target(&self) -> Option<String> {
        util::build_target(self.target.as_deref())
    }
}

#[derive(Subcommand)]
enum Task {
    /// Prepare Zig and the patched Ghostty source, then run a command in that environment.
    Env {
        /// Write the environment to GITHUB_ENV and GITHUB_PATH instead of running a command.
        #[arg(long, conflicts_with = "command")]
        github: bool,
        /// The command to run, after `--`.
        #[arg(
            last = true,
            required_unless_present = "github",
            value_name = "COMMAND"
        )]
        command: Vec<OsString>,
    },
    /// Run the desktop app with staged resources and a development profile.
    Dev {
        /// Compile Remote apps in, from the private newport-remote checkout
        /// (NEWPORT_REMOTE_DIR, default ../newport-remote).
        #[arg(long)]
        remote_apps: bool,
        #[arg(last = true)]
        args: Vec<OsString>,
    },
    /// Build the desktop app with staged resources.
    Build {
        #[command(flatten)]
        build: BuildArgs,
        /// Compile Remote apps in, from the private newport-remote checkout.
        #[arg(long)]
        remote_apps: bool,
    },
    /// Build and stage the tree-sitter grammar libraries.
    Grammars(BuildArgs),
    /// Download, verify and stage PDFium (Windows targets).
    Pdfium(BuildArgs),
    /// Build the Linux agent binaries in Docker, or verify prebuilt ones with NEWPORT_PREBUILT=1.
    Agents {
        /// Only when they are missing or older than the agent's sources.
        #[arg(long)]
        if_changed: bool,
    },
    /// Record or verify an artifact manifest (newport-build.json).
    Artifact {
        #[command(subcommand)]
        action: artifact::Action,
    },
    /// macOS cargo runner: bundle, sign and launch `newport`; run anything else directly.
    #[command(hide = true)]
    DevRun {
        binary: PathBuf,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Formatting, Clippy and tests.
    Check {
        #[arg(value_enum)]
        scope: Option<check::Scope>,
    },
    /// Integration and acceptance tests.
    Test {
        #[command(subcommand)]
        suite: TestSuite,
    },
    /// Stage, verify and archive the desktop app.
    Package(package::Options),
    /// Release gates, signing and update manifests.
    Release {
        #[command(subcommand)]
        action: release::Action,
    },
}

#[derive(Subcommand)]
enum TestSuite {
    /// Persistence, resource staging and the desktop app's lifecycle.
    Desktop {
        #[arg(long)]
        headless: bool,
    },
    /// Disposable SSH fixture.
    Ssh {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Testcontainers with real OpenSSH and remote services.
    Remote {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Agent Git over HTTPS.
    Https,
    /// Agent runtime checks inside the Alpine image.
    Runtime,
    /// Real agent Git responses through the app's validators.
    GitContract {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Agent Git CLI responses through the app's validators.
    GitCliContract {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Git benchmark.
    Benchmark {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// macOS protected Keychain test with a signed test bundle.
    Keychain,
    /// newport-gpui's tests with Remote apps compiled in, the only way that
    /// code is tested.
    RemoteApps {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Task::Env { github, command } => ghostty::env_command(github, &command),
        Task::Dev { remote_apps, args } => check::dev(&args, remote_apps),
        Task::Build { build, remote_apps } => check::build(&build, remote_apps),
        Task::Grammars(args) => grammars::build(args.release, args.target().as_deref()).map(drop),
        Task::Pdfium(args) => pdfium::stage(args.release, args.target().as_deref()).map(drop),
        Task::Agents { if_changed: false } => remote::build_agents(),
        Task::Agents { if_changed: true } => check::agents_if_changed(),
        Task::Artifact { action } => artifact::run(action),
        Task::DevRun { binary, args } => dev_run::run(&binary, &args),
        Task::Check { scope } => check::run(scope),
        Task::Test { suite } => match suite {
            TestSuite::Desktop { headless } => desktop::run(headless),
            TestSuite::Ssh { args } => ssh::run(&args),
            TestSuite::Remote { args } => remote::run(&args),
            TestSuite::Https => https::run(),
            TestSuite::Runtime => remote::runtime(),
            TestSuite::GitContract { args } => contract::run(false, &args),
            TestSuite::GitCliContract { args } => contract::run(true, &args),
            TestSuite::Benchmark { args } => benchmark::run(&args),
            TestSuite::Keychain => keychain::run(),
            TestSuite::RemoteApps { args } => check::test_remote_apps(&args),
        },
        Task::Package(options) => package::run(&options).map(drop),
        Task::Release { action } => release::run(action),
    }
}
