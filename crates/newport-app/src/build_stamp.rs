//! What this build is, so builds of one version can be told apart.
use newport_protocol::desktop::host::BuildStamp;

/// The commit the app was built from, with `+` when the checkout had changes.
pub const COMMIT: Option<&str> = option_env!("NEWPORT_BUILD_COMMIT");
/// Built on a person's computer rather than in CI.
const LOCAL: bool = option_env!("NEWPORT_BUILD_LOCAL").is_some();

pub fn status() -> BuildStamp {
    BuildStamp {
        commit: COMMIT.map(str::to_owned),
        local: LOCAL,
        built_at: built_at(),
    }
}

/// When the app was built: its executable's modification time, in
/// milliseconds. A compile-time clock would rebuild the app every time.
fn built_at() -> Option<i64> {
    let modified = std::env::current_exe()
        .ok()?
        .metadata()
        .ok()?
        .modified()
        .ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_build_time_is_the_executables() {
        let built = super::status().built_at.expect("a build time");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(built > 0 && built <= now, "{built}");
        assert_eq!(super::status().commit.as_deref(), super::COMMIT);
    }
}
