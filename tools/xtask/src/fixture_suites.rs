//! Required fixtures and prerequisite checks for ignored backend acceptance tests.
use anyhow::{Context, Result, bail, ensure};
use std::path::PathBuf;

/// Build once before starting fixtures, then execute Cargo's reported test
/// binary directly so compilation cannot consume the fixture runtime budget.
pub fn backend_executable() -> Result<PathBuf> {
    let output = tempfile::NamedTempFile::new()?;
    let build = crate::process::run(
        crate::command("cargo")
            .args([
                "test",
                "--locked",
                "-p",
                "newport-backend",
                "--lib",
                "--no-run",
                "--message-format=json",
            ])
            .arg("--target-dir")
            .arg(crate::root().join("target"))
            .env_remove("CARGO_TARGET_DIR")
            .stdout(output.reopen()?),
    );
    let mut executable = None;
    for line in std::fs::read_to_string(output.path())?.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(rendered) = message["message"]["rendered"].as_str() {
            eprint!("{rendered}");
        }
        if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == "newport_backend"
            && message["profile"]["test"] == true
        {
            executable = message["executable"].as_str().map(PathBuf::from);
        }
    }
    build?;
    executable.context("Cargo did not report the backend test executable")
}

pub const LINUX: &[&str] = &[
    "remote_tests::",
    "agent_lifecycle::",
    "server_setup::lifecycle_tests::",
    "git::connection_tests::",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fixture {
    Ssh,
    Linux,
    NativeClipboard,
    ComposedClipboard,
    FullProduct,
}

impl Fixture {
    pub fn command(self) -> &'static str {
        match self {
            Self::Ssh => "cargo xtask test ssh",
            Self::Linux => "cargo xtask test remote",
            Self::NativeClipboard => "cargo xtask test clipboard",
            Self::ComposedClipboard => {
                "cargo xtask test clipboard --linux-peer PATH (local composed-agent launcher)"
            }
            Self::FullProduct => {
                "NEWPORT_AGENT_RESOURCE_DIR=PATH cargo xtask test remote --filter agent_full_product_tests::"
            }
        }
    }
}

pub const NATIVE_CLIPBOARD: &[&str] = &[
    "clipboard::async_tests::real_native_adapter_keeps_fences_through_backend_chunks_and_suppresses_import_echo",
    "clipboard_export::source::tests::native::focus_revocation_cancels_a_waiting_native_paste_and_remote_demand",
    "clipboard_export::source::tests::native::native_export_is_lazy_and_event_pump_completes_text_and_png",
];
pub const COMPOSED_CLIPBOARD: &[&str] = &[
    "clipboard::async_tests::native_origin_roundtrip_preserves_the_real_linux_selection_and_user_clear",
    "clipboard_export::copy::tests::live::real_linux_copies_survive_focus_and_release_without_replacing_native_selection",
    "clipboard_export::copy::tests::live::native::native_copy_publication_survives_focus_and_preserves_a_newer_local_copy",
    "clipboard_export::copy::tests::live::native::router::router_preserves_lazy_text_and_image_pastes_after_focus_moves",
];
pub const FULL_PRODUCT: &str = "agent_full_product_tests::";
const FULL_PRODUCT_TEST: &str =
    "agent_full_product_tests::full_product_install_opens_registered_gui_and_core_workers";

pub fn requires_full_product(filter: &str) -> bool {
    filter.starts_with(FULL_PRODUCT) || FULL_PRODUCT_TEST.contains(filter)
}

pub fn fixture(name: &str) -> Result<Fixture> {
    if name.starts_with(FULL_PRODUCT) {
        return Ok(Fixture::FullProduct);
    }
    if NATIVE_CLIPBOARD.contains(&name) {
        return Ok(Fixture::NativeClipboard);
    }
    if COMPOSED_CLIPBOARD.contains(&name) {
        return Ok(Fixture::ComposedClipboard);
    }
    if LINUX.iter().any(|prefix| name.contains(prefix)) {
        return Ok(Fixture::Linux);
    }
    if [
        "clipboard::tests::",
        "clipboard_downloads::tests::fixture::",
        "clipboard_files::tests::fixture::",
        "commands::control::connection_tests::",
        "commands::terminal_links::tests::",
        "files::",
        "graphical_session::tests::terminal::",
        "known_hosts::tests::",
        "manager::integration_tests::",
        "manager::tunnel_cleanup_tests::",
        "ports::tests::",
        "rpc::features::tests::",
        "server_session::tests::",
        "ssh::",
        "terminal::tests::",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
    {
        return Ok(Fixture::Ssh);
    }
    bail!("Ignored backend test has no fixture assignment: {name}")
}

pub fn select(names: &[String], wanted: Fixture) -> Result<Vec<String>> {
    let mut selected = Vec::new();
    let mut other = Vec::new();
    for name in names {
        let required = fixture(name)?;
        if required == wanted {
            selected.push(name.clone());
        } else if !other.contains(&required.command()) {
            other.push(required.command());
        }
    }
    ensure!(
        !selected.is_empty(),
        "No tests matched {}. Matching tests require: {}",
        wanted.command(),
        other.join("; ")
    );
    Ok(selected)
}

pub fn list(executable: &std::path::Path, args: &[String]) -> Result<Vec<String>> {
    let list = crate::process::text(
        crate::command(executable)
            .args(args)
            .args(["--ignored", "--list"]),
    )?;
    let names: Vec<_> = list
        .lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .map(str::to_owned)
        .collect();
    for name in &names {
        fixture(name)?;
    }
    Ok(names)
}

/// The launcher is caller-supplied; its composed agent and display stay in a
/// disposable local fixture. Never discover a running desktop or server.
pub fn clipboard(linux_peer: Option<&std::path::Path>) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "Native clipboard fixtures require macOS"
    );
    let peer = linux_peer
        .map(|path| -> Result<PathBuf> {
            let path = path
                .canonicalize()
                .context("Local Linux clipboard peer launcher is missing")?;
            ensure!(
                path.is_file(),
                "Linux clipboard peer launcher must be an executable file"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                ensure!(
                    std::fs::metadata(&path)?.permissions().mode() & 0o111 != 0,
                    "Linux clipboard peer launcher is not executable"
                );
            }
            full_product_resources()?;
            Ok(path)
        })
        .transpose()?;
    crate::process::run(
        crate::command("cargo")
            .args([
                "build",
                "--locked",
                "-p",
                "newport-platform",
                "--example",
                "clipboard_owner_check",
                "--target-dir",
                "target",
            ])
            .env_remove("CARGO_TARGET_DIR"),
    )?;
    let helper = crate::root()
        .join("target/debug/examples/clipboard_owner_check")
        .canonicalize()?;
    let executable = backend_executable()?;
    let listed = list(&executable, &[])?;
    let mut names = select(&listed, Fixture::NativeClipboard)?;
    ensure!(
        names.len() == NATIVE_CLIPBOARD.len(),
        "Native clipboard fixture inventory is incomplete"
    );
    if peer.is_some() {
        let composed = select(&listed, Fixture::ComposedClipboard)?;
        ensure!(
            composed.len() == COMPOSED_CLIPBOARD.len(),
            "Composed clipboard fixture inventory is incomplete"
        );
        names.extend(composed);
    }
    let mut command = crate::command(&executable);
    command
        .args(["--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .args(names)
        .env("NEWPORT_CLIPBOARD_HELPER", helper)
        .env_remove("NEWPORT_CLIPBOARD_LINUX_PEER");
    if let Some(peer) = peer {
        command.env("NEWPORT_CLIPBOARD_LINUX_PEER", peer);
    }
    crate::process::Process::spawn(&mut command)?.success(std::time::Duration::from_secs(300))
}

pub fn full_product_resources() -> Result<PathBuf> {
    let directory = std::env::var_os("NEWPORT_AGENT_RESOURCE_DIR").context(
        "This fixture requires an explicit full agent export in NEWPORT_AGENT_RESOURCE_DIR",
    )?;
    crate::remote_apps::checked_full_resources(&crate::root(), &PathBuf::from(directory))
}

pub fn runs(names: Vec<String>) -> Vec<Vec<String>> {
    // The shared remote_tests suite restores its fixture changes. Lifecycle tests
    // install, uninstall or leave signed metadata and removed builds; each of
    // those must receive a fresh server, including after a panic.
    let (shared, isolated): (Vec<_>, Vec<_>) = names
        .into_iter()
        .partition(|name| name.contains("remote_tests::"));
    let mut runs = Vec::new();
    if !shared.is_empty() {
        runs.push(shared);
    }
    runs.extend(isolated.into_iter().map(|name| vec![name]));
    runs
}

/// Lifecycle tests only need OpenSSH; privileged service fixtures are reserved
/// for tests that actually exercise them.
pub fn auxiliary_services(names: &[String]) -> (bool, bool) {
    (
        names
            .iter()
            .any(|name| name.starts_with("remote_tests::services::")),
        names.iter().any(|name| {
            name.starts_with("remote_tests::containers::")
                || name.starts_with("remote_tests::compose::")
        }),
    )
}

#[cfg(test)]
mod tests {

    #[test]
    fn fixture_selection_keeps_specialized_tests_in_their_required_lanes() {
        use super::{Fixture, fixture};
        for name in super::NATIVE_CLIPBOARD {
            assert_eq!(fixture(name).unwrap(), Fixture::NativeClipboard);
        }
        for name in super::COMPOSED_CLIPBOARD {
            assert_eq!(fixture(name).unwrap(), Fixture::ComposedClipboard);
        }
        for (name, expected) in [
            ("clipboard::tests::managed_bridge", Fixture::Ssh),
            ("clipboard::tests::remote_tests::upgrade", Fixture::Linux),
            ("agent_lifecycle::tests::install", Fixture::Linux),
            ("agent_full_product_tests::install", Fixture::FullProduct),
        ] {
            assert_eq!(fixture(name).unwrap(), expected);
        }
        assert!(fixture("new_fixture::needs_a_server").is_err());
        assert!(fixture("clipboard::async_tests::new_native_fixture").is_err());
    }

    #[test]
    fn full_product_filters_require_the_export_before_starting_fixtures() {
        for filter in [
            super::FULL_PRODUCT,
            super::FULL_PRODUCT_TEST,
            "full_product_install",
        ] {
            assert!(super::requires_full_product(filter));
        }
        for filter in super::LINUX {
            assert!(!super::requires_full_product(filter));
        }
    }

    #[test]
    fn wrong_lane_or_unknown_tests_cannot_pass_as_an_empty_run() {
        use super::{Fixture, select};
        let full = "agent_full_product_tests::install".to_owned();
        let error = select(std::slice::from_ref(&full), Fixture::Ssh)
            .unwrap_err()
            .to_string();
        assert!(error.contains("NEWPORT_AGENT_RESOURCE_DIR"));
        assert!(select(&[], Fixture::Ssh).is_err());
        assert!(select(&["unknown::test".into()], Fixture::Ssh).is_err());
        let ssh = "ssh::integration_tests::embedded_auth".to_owned();
        let mut names = vec![full, ssh.clone()];
        names.extend(
            super::NATIVE_CLIPBOARD
                .iter()
                .map(|name| (*name).to_owned()),
        );
        names.extend(
            super::COMPOSED_CLIPBOARD
                .iter()
                .map(|name| (*name).to_owned()),
        );
        assert_eq!(select(&names, Fixture::Ssh).unwrap(), vec![ssh]);
        assert_eq!(
            select(&names, Fixture::NativeClipboard).unwrap(),
            super::NATIVE_CLIPBOARD
        );
        assert_eq!(
            select(&names, Fixture::ComposedClipboard).unwrap(),
            super::COMPOSED_CLIPBOARD
        );
    }

    #[test]
    fn auxiliary_servers_match_the_selected_feature_suites() {
        for (name, expected) in [
            ("agent_lifecycle::tests::install", (false, false)),
            ("remote_tests::git::history", (false, false)),
            ("remote_tests::services::units", (true, false)),
            ("remote_tests::containers::list", (false, true)),
            ("remote_tests::compose::up", (false, true)),
        ] {
            assert_eq!(super::auxiliary_services(&[name.into()]), expected);
        }
        assert_eq!(
            super::auxiliary_services(&[
                "remote_tests::services::units".into(),
                "remote_tests::compose::up".into(),
            ]),
            (true, true)
        );
    }

    #[test]
    fn destructive_cases_never_share_a_server_with_other_tests() {
        let runs = super::runs(
            [
                "agent_lifecycle::release_tests::tamper",
                "agent_lifecycle::tests::install",
                "remote_tests::files",
                "clipboard::tests::remote_tests::upgrade",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        );
        assert_eq!(
            runs,
            vec![
                vec![
                    "remote_tests::files",
                    "clipboard::tests::remote_tests::upgrade"
                ],
                vec!["agent_lifecycle::release_tests::tamper"],
                vec!["agent_lifecycle::tests::install"],
            ]
        );
        assert!(super::runs(Vec::new()).is_empty());
    }
}
