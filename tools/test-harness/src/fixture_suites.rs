//! Tests needing real Linux installation cannot run against the adverse SSH
//! fixture, which deliberately recognizes only a small set of commands.
pub const LINUX: &[&str] = &[
    "remote_tests::",
    "agent_lifecycle::",
    "server_setup::lifecycle_tests::",
    "server_session::removal_tests::",
    "git::connection_tests::",
];

pub fn runs(names: Vec<String>) -> Vec<Vec<String>> {
    // The original remote suite restores its fixture changes. Lifecycle tests
    // intentionally leave corrupt journals, signed metadata or removed builds;
    // each of those must receive a fresh server, including after a panic.
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

#[cfg(test)]
mod tests {
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
