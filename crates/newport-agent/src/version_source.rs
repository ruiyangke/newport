//! Chooses the product version a build bakes in. Shared with `build.rs`, which
//! cannot run its own tests.

/// The version from the build environment and from `resources/app.json`. A
/// Docker build passes the first because a Cargo cache that outlives the
/// image layers can hold a build script result that the file's modification
/// time no longer invalidates; the second is all a plain Cargo build has.
pub fn resolve(from_env: Option<&str>, from_metadata: Option<&str>) -> Result<String, String> {
    let from_env = from_env.filter(|version| !version.is_empty());
    match (from_env, from_metadata) {
        (Some(env), Some(file)) if env != file => Err(format!(
            "NEWPORT_AGENT_VERSION is {env} but resources/app.json says {file}"
        )),
        (Some(version), _) | (None, Some(version)) => Ok(version.to_owned()),
        (None, None) => Err("Missing Newport product version".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn the_environment_version_wins_when_the_file_is_absent() {
        assert_eq!(resolve(Some("0.7.1"), None).unwrap(), "0.7.1");
    }

    #[test]
    fn the_file_version_is_used_without_an_environment_version() {
        assert_eq!(resolve(None, Some("0.7.0")).unwrap(), "0.7.0");
        assert_eq!(resolve(Some(""), Some("0.7.0")).unwrap(), "0.7.0");
    }

    #[test]
    fn disagreeing_versions_fail_instead_of_picking_one() {
        let error = resolve(Some("0.7.1"), Some("0.7.0")).unwrap_err();
        assert!(error.contains("0.7.1") && error.contains("0.7.0"));
    }

    #[test]
    fn matching_versions_pass_and_no_version_fails() {
        assert_eq!(resolve(Some("0.7.0"), Some("0.7.0")).unwrap(), "0.7.0");
        assert!(resolve(None, None).is_err());
    }
}
