//! Tests needing real Linux installation cannot run against the adverse SSH
//! fixture, which deliberately recognizes only a small set of commands.
use anyhow::{Context, Result};
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
