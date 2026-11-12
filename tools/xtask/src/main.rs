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
mod discovery;
mod fixture_suites;
mod grammars;
mod https;
mod keychain;
mod notices;
mod package;
mod patched;
mod pdfium;
mod pe;
mod process;
mod protocol_reference;
mod release;
mod remote;
mod remote_apps;
mod signing;
mod ssh;
#[cfg(all(test, unix))]
mod test_support;
mod trace;
mod util;
mod windows_clipboard_tests;
mod windows_window_tests;

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
    /// Generate the shared protocol reference from compiled schema comments.
    ProtocolReference {
        /// Fail if the checked-in reference is stale; do not write documentation.
        #[arg(long)]
        check: bool,
    },
    /// Format Rust sources, forwarding arguments to cargo fmt.
    Fmt {
        #[arg(last = true)]
        args: Vec<OsString>,
    },
    /// Run the desktop app with staged resources and a development profile.
    Dev {
        /// Compile Remote apps in, from the private newport-remote checkout
        /// at remote/.
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
    /// Generate the patched dependencies in patches/ from their crates.io
    /// releases and patches; every other task does this first.
    Patched {
        /// Write this crate's patch from the edits made to its generated source.
        #[arg(long, value_name = "CRATE")]
        save: Option<String>,
    },
    /// Build the Linux agent binaries in Docker, or verify prebuilt ones with NEWPORT_PREBUILT=1.
    Agents {
        /// Build the full agent resources from the private checkout.
        #[arg(long, conflicts_with_all = ["if_changed", "architectures"])]
        remote_apps: bool,
        /// Only when they are missing or older than the agent's sources.
        #[arg(long)]
        if_changed: bool,
        /// Only these architectures (x86_64, aarch64); default both. CI
        /// builds each on a runner of that architecture.
        #[arg(long = "arch", conflicts_with = "if_changed")]
        architectures: Vec<String>,
    },
    /// Build or export the Full agent (core and Remote apps) from remote/.
    FullAgent {
        #[command(subcommand)]
        action: remote_apps::full_agent::Action,
    },
    /// Record or verify an artifact manifest (newport-build.json).
    Artifact {
        #[command(subcommand)]
        action: artifact::Action,
    },
    /// macOS cargo runner: bundle, sign and launch `newport`; run anything else directly.
    #[command(hide = true)]
    DevRun {
        /// macOS qualification only: use Newport Dev/app.newport.dev with dev-only
        /// entitlements and an explicitly set, fresh empty NEWPORT_DATA_DIR.
        /// Reusing a populated profile is deliberately refused; nothing is erased.
        #[arg(long)]
        isolated_bundle: bool,
        binary: PathBuf,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Formatting, Clippy and tests.
    Check {
        #[arg(value_enum)]
        scope: Option<check::Scope>,
        /// Limit desktop Clippy and tests to these packages (requires `rust`).
        #[arg(short = 'p', long = "package", requires = "scope")]
        packages: Vec<String>,
    },
    /// Integration and acceptance tests.
    Test {
        #[command(subcommand)]
        suite: TestSuite,
    },
    /// Write the third-party notices a desktop package carries, installing
    /// the pinned cargo-about first.
    Notices(notices::Options),
    /// Stage, verify and archive the desktop app.
    Package(package::Options),
    /// Release gates, signing and update manifests.
    Release {
        #[command(subcommand)]
        action: release::Action,
    },
    /// Print a captured channel's frames as Protocol Buffers JSON, one a line.
    Trace {
        /// The protocol whose schemas the channel uses, such as newport-protocol
        /// or remote-apps, or the path of an encoded FileDescriptorSet.
        protocol: String,
        /// The fully qualified name of the channel's top-level message.
        message: String,
        /// The captured frames; standard input when omitted.
        capture: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum TestSuite {
    /// Run prebuilt discovery tests in an isolated Linux container as its newport user.
    Discovery(discovery::Options),
    /// Provision disposable distros and run discovery with elevation allowed and denied.
    DiscoveryMatrix(discovery::matrix::Options),
    /// Build static Core discovery fixtures in Docker, with canonical executable names.
    DiscoveryBuild(discovery::matrix::build::Options),
    /// Build and run the real GPUI window lifecycle example on Windows.
    WindowsWindow(windows_window_tests::Options),
    /// Build and run the real GPUI input lifecycle example on Windows.
    WindowsInput(windows_window_tests::Options),
    /// Exercise licensed provider window state with a running isolated Newport profile.
    WindowsProviderWindow(windows_window_tests::LicensedOptions),
    /// Exercise licensed provider input with a running isolated Newport profile.
    WindowsProviderInput(windows_window_tests::LicensedOptions),
    /// Build and run the private provider OLE contract example on Windows.
    WindowsDrag(windows_window_tests::DragOptions),
    /// Observe native OLE cancellation during a real stationary press-and-hold.
    WindowsDragGesture(windows_window_tests::GestureOptions),
    /// Observe one manual Explorer drop into an owned WM_DROPFILES receiver.
    WindowsFileDrop(windows_window_tests::FileDropOptions),
    /// Run the owned COM file extraction experiment on Windows.
    WindowsDropExtraction(windows_window_tests::DragOptions),
    /// Run an explicitly selected Windows clipboard fixture on an isolated station.
    ClipboardWindows(windows_clipboard_tests::Options),
    /// Run Rust tests, forwarding Cargo arguments (including --manifest-path).
    Rust {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
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
    /// Native clipboard checks on disposable macOS pasteboards.
    Clipboard {
        /// Also run composed Linux clipboard checks using this local fixture launcher.
        /// Requires a full export in NEWPORT_AGENT_RESOURCE_DIR.
        #[arg(long)]
        linux_peer: Option<PathBuf>,
    },
    /// Testcontainers with real OpenSSH and remote services.
    Remote {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Agent Git over HTTPS.
    Https,
    /// Agent runtime checks inside the Alpine image.
    Runtime {
        /// Only these architectures (x86_64, aarch64); default both.
        #[arg(long = "arch")]
        architectures: Vec<String>,
    },
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
    let task = Cli::parse().command;
    if let Task::Patched { save: Some(name) } = &task {
        return patched::save(name);
    }
    patched::ensure_all()?;
    match task {
        Task::Patched { .. } => Ok(()),
        Task::ProtocolReference { check } => protocol_reference::run(check),
        Task::Fmt { args } => check::fmt(&args),
        Task::Dev { remote_apps, args } => check::dev(&args, remote_apps),
        Task::Build { build, remote_apps } => check::build(&build, remote_apps),
        Task::Grammars(args) => grammars::build(args.release, args.target().as_deref()).map(drop),
        Task::Pdfium(args) => pdfium::stage(args.release, args.target().as_deref()).map(drop),
        Task::Agents {
            remote_apps: true, ..
        } => remote_apps::build_full_resources().map(drop),
        Task::Agents {
            remote_apps: false,
            if_changed: false,
            architectures,
        } => remote::build_agents(&architectures),
        Task::Agents {
            if_changed: true, ..
        } => check::agents_if_changed(),
        Task::FullAgent { action } => remote_apps::full_agent::run(action),
        Task::Artifact { action } => artifact::run(action),
        Task::DevRun {
            isolated_bundle,
            binary,
            args,
        } => {
            if isolated_bundle {
                dev_run::run_isolated(&binary, &args)
            } else {
                dev_run::run(&binary, &args)
            }
        }
        Task::Check { scope, packages } => check::run(scope, &packages),
        Task::Test { suite } => match suite {
            TestSuite::Discovery(options) => discovery::run(options),
            TestSuite::DiscoveryMatrix(options) => discovery::matrix::run(options),
            TestSuite::DiscoveryBuild(options) => discovery::matrix::build::run(options),
            TestSuite::Desktop { headless } => desktop::run(headless),
            TestSuite::Ssh { args } => ssh::run(&args),
            TestSuite::Remote { args } => remote::run(&args),
            TestSuite::Clipboard { linux_peer } => fixture_suites::clipboard(linux_peer.as_deref()),
            TestSuite::Https => https::run(),
            TestSuite::Runtime { architectures } => remote::runtime(&architectures),
            TestSuite::GitContract { args } => contract::run(false, &args),
            TestSuite::GitCliContract { args } => contract::run(true, &args),
            TestSuite::Benchmark { args } => benchmark::run(&args),
            TestSuite::Keychain => keychain::run(),
            TestSuite::RemoteApps { args } => check::test_remote_apps(&args),
            TestSuite::Rust { args } => check::test_rust(&args),
            TestSuite::ClipboardWindows(options) => windows_clipboard_tests::run(options),
            TestSuite::WindowsWindow(options) => windows_window_tests::run(options),
            TestSuite::WindowsInput(options) => windows_window_tests::run_input(options),
            TestSuite::WindowsProviderWindow(options) => {
                windows_window_tests::run_provider_window(options)
            }
            TestSuite::WindowsProviderInput(options) => {
                windows_window_tests::run_provider_input(options)
            }
            TestSuite::WindowsDrag(options) => windows_window_tests::run_drag(options),
            TestSuite::WindowsDragGesture(options) => {
                windows_window_tests::run_drag_gesture(options)
            }
            TestSuite::WindowsFileDrop(options) => windows_window_tests::run_file_drop(options),
            TestSuite::WindowsDropExtraction(options) => {
                windows_window_tests::run_drop_extraction(options)
            }
        },
        Task::Notices(options) => notices::run(&options),
        Task::Package(options) => package::run(&options).map(drop),
        Task::Release { action } => release::run(action),
        Task::Trace {
            protocol,
            message,
            capture,
        } => trace::run(&protocol, &message, capture.as_deref()),
    }
}

#[cfg(test)]
mod dev_identity_cli_tests {
    use super::*;

    #[test]
    fn full_agents_cannot_silently_use_core_build_options() {
        let parsed = Cli::try_parse_from(["xtask", "agents", "--remote-apps"]).unwrap();
        assert!(matches!(
            parsed.command,
            Task::Agents {
                remote_apps: true,
                ..
            }
        ));
        for option in [vec!["--if-changed"], vec!["--arch", "aarch64"]] {
            let mut args = vec!["xtask", "agents", "--remote-apps"];
            args.extend(option);
            assert!(Cli::try_parse_from(args).is_err());
        }
        for args in [
            vec!["xtask", "agents"],
            vec!["xtask", "agents", "--if-changed"],
            vec!["xtask", "agents", "--arch", "aarch64"],
        ] {
            assert!(matches!(
                Cli::try_parse_from(args).unwrap().command,
                Task::Agents {
                    remote_apps: false,
                    ..
                }
            ));
        }
    }

    #[test]
    fn isolated_bundle_is_explicit_and_default_runner_arguments_stay_unchanged() {
        for (arguments, expected) in [
            (
                vec!["xtask", "dev-run", "target/newport", "--setup-preview"],
                false,
            ),
            (
                vec![
                    "xtask",
                    "dev-run",
                    "--isolated-bundle",
                    "target/newport",
                    "--setup-preview",
                ],
                true,
            ),
        ] {
            let Task::DevRun {
                isolated_bundle,
                binary,
                args,
            } = Cli::try_parse_from(arguments).unwrap().command
            else {
                panic!("expected dev-run");
            };
            assert_eq!(isolated_bundle, expected);
            assert_eq!(binary, PathBuf::from("target/newport"));
            assert_eq!(args, vec![OsString::from("--setup-preview")]);
        }
    }
}
