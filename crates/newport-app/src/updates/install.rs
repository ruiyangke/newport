//! macOS install handoff. Preparation does not change the live app. A durable
//! transaction is acquired before draining, and replacement runs only afterward.
use newport_platform::updates::{
    VerifiedUpdate,
    macos::{self, PreparedBundle, Transaction},
};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

pub struct Request {
    prepared: PreparedBundle,
    version: semver::Version,
}
impl Request {
    pub fn prepare(package: &VerifiedUpdate) -> Result<Self, String> {
        let running = std::env::current_exe().map_err(|error| error.to_string())?;
        let bundle = bundle_for_executable(&running)?;
        if macos::executable(&bundle)?
            .canonicalize()
            .map_err(|error| error.to_string())?
            != running.canonicalize().map_err(|error| error.to_string())?
        {
            return Err("The running executable does not match its app bundle.".into());
        }
        let prepared = PreparedBundle::prepare(package, &bundle)?;
        if prepared.installed_version().to_string() != env!("CARGO_PKG_VERSION") {
            return Err(
                "The installed bundle and running app versions differ. Restart before updating."
                    .into(),
            );
        }
        Ok(Self {
            prepared,
            version: package.version().clone(),
        })
    }

    /// After the UI accepts restart, acquire the installation lock and persist
    /// intent BEFORE releasing backend/profile ownership. No exchange happens.
    pub fn begin(self) -> Result<Installation, String> {
        let transaction = self.prepared.begin()?;
        Ok(Installation::resume(transaction, self.version, false))
    }
}

pub(super) fn bundle_for_executable(executable: &Path) -> Result<PathBuf, String> {
    let macos = executable.parent().ok_or("Missing executable directory.")?;
    let contents = macos.parent().ok_or("Missing app Contents directory.")?;
    let bundle = contents.parent().ok_or("Missing app bundle.")?;
    if !executable.is_absolute()
        || macos.file_name() != Some("MacOS".as_ref())
        || contents.file_name() != Some("Contents".as_ref())
        || bundle.extension() != Some("app".as_ref())
    {
        return Err("Updates require an installed macOS app bundle.".into());
    }
    Ok(bundle.to_owned())
}

pub struct Installation {
    transaction: Option<Transaction>,
    destination: PathBuf,
    recovery: PathBuf,
    version: semver::Version,
    accepted: bool,
}
impl Installation {
    pub(super) fn resume(
        transaction: Transaction,
        version: semver::Version,
        accepted: bool,
    ) -> Self {
        Self {
            destination: transaction.destination().to_owned(),
            recovery: transaction.recovery_directory().to_owned(),
            transaction: Some(transaction),
            version,
            accepted,
        }
    }
    /// The caller must have completed backend shutdown successfully. Backups
    /// survive success: only the relaunched app may accept normal startup.
    pub async fn run(mut self, shutdown: Result<(), String>) -> Result<(), String> {
        let accepted = self.accepted;
        execute(&mut self, shutdown, accepted).await
    }

    fn transaction(&mut self) -> Result<&mut Transaction, String> {
        if self.transaction.is_none() {
            self.transaction = Some(Transaction::open(&self.destination, &self.recovery)?);
        }
        self.transaction
            .as_mut()
            .ok_or("Missing update transaction.".into())
    }
}

trait Driver {
    fn replace(&mut self) -> Result<(), String>;
    async fn check(&mut self) -> Result<(), String>;
    fn restore(&mut self) -> Result<(), String>;
    fn launch(&mut self) -> Result<(), String>;
}
impl Driver for Installation {
    fn replace(&mut self) -> Result<(), String> {
        self.transaction()?.reconcile().map(|_| ())
    }
    async fn check(&mut self) -> Result<(), String> {
        let executable = macos::executable(&self.destination)?;
        newport_platform::updates::verify_startup(
            &executable,
            &self.version,
            Duration::from_secs(60),
        )
        .await
    }
    fn restore(&mut self) -> Result<(), String> {
        self.transaction()?.restore().map(|_| ())
    }
    fn launch(&mut self) -> Result<(), String> {
        // Revalidate immediately before executing. This also realizes persisted
        // restoration intent after a failed health check.
        self.transaction()?.reconcile()?;
        let executable = macos::executable(&self.destination)?;
        // The new process must be able to open recovery. On spawn failure the
        // journal remains, and restore() reacquires the same transaction.
        drop(self.transaction.take());
        Command::new(executable)
            .current_dir(self.destination.parent().ok_or("Missing app parent.")?)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("Could not restart Newport: {error}"))?;
        Ok(())
    }
}

async fn execute(
    driver: &mut impl Driver,
    shutdown: Result<(), String>,
    accepted: bool,
) -> Result<(), String> {
    shutdown?;
    let attempt = async {
        driver.replace()?;
        driver.check().await?;
        driver.launch()
    }
    .await;
    if let Err(error) = attempt {
        // Acceptance may already have removed some backup files. There is no
        // valid rollback target; preserve the journal and report the failure.
        if accepted {
            return Err(error);
        }
        // Replacement errors can be ambiguous (exchange succeeded before sync
        // failed). Always use the journal's idempotent restoration path.
        driver.restore().map_err(|restore| {
            format!("{error} Recovery was preserved, but restoration failed: {restore}")
        })?;
        driver.launch().map_err(|launch| {
            format!("{error} The previous app was restored, but could not restart: {launch}")
        })?;
        return Err(format!(
            "{error} The previous app was restored and restarted."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fixture {
        calls: Vec<&'static str>,
        fail: Option<&'static str>,
        restore_fails: bool,
    }
    impl Fixture {
        fn step(&mut self, step: &'static str) -> Result<(), String> {
            self.calls.push(step);
            if self.fail == Some(step) {
                self.fail = None;
                Err(format!("{step} failed"))
            } else {
                Ok(())
            }
        }
    }
    impl Driver for Fixture {
        fn replace(&mut self) -> Result<(), String> {
            self.step("replace")
        }
        async fn check(&mut self) -> Result<(), String> {
            self.step("check")
        }
        fn restore(&mut self) -> Result<(), String> {
            self.calls.push("restore");
            if self.restore_fails {
                Err("restore failed".into())
            } else {
                Ok(())
            }
        }
        fn launch(&mut self) -> Result<(), String> {
            self.step("launch")
        }
    }
    #[tokio::test]
    async fn only_a_successful_health_check_can_launch_the_updated_app() {
        let mut f = Fixture::default();
        execute(&mut f, Ok(()), false).await.unwrap();
        assert_eq!(f.calls, ["replace", "check", "launch"]);
    }
    #[tokio::test]
    async fn failed_replacement_health_or_launch_restore_before_restarting() {
        for (failure, expected) in [
            ("replace", vec!["replace", "restore", "launch"]),
            ("check", vec!["replace", "check", "restore", "launch"]),
            (
                "launch",
                vec!["replace", "check", "launch", "restore", "launch"],
            ),
        ] {
            let mut f = Fixture {
                fail: Some(failure),
                ..Default::default()
            };
            assert!(
                execute(&mut f, Ok(()), false)
                    .await
                    .unwrap_err()
                    .contains("previous app was restored and restarted")
            );
            assert_eq!(f.calls, expected);
        }
    }
    #[tokio::test]
    async fn failed_restore_does_not_launch_an_uncertain_bundle() {
        let mut f = Fixture {
            fail: Some("check"),
            restore_fails: true,
            ..Default::default()
        };
        assert!(
            execute(&mut f, Ok(()), false)
                .await
                .unwrap_err()
                .contains("restoration failed")
        );
        assert_eq!(f.calls, ["replace", "check", "restore"]);
    }
    #[test]
    fn install_requires_a_real_bundle_location() {
        assert_eq!(
            bundle_for_executable(Path::new(
                "/Applications/Newport.app/Contents/MacOS/newport"
            ))
            .unwrap(),
            Path::new("/Applications/Newport.app")
        );
        for invalid in [
            "newport",
            "/tmp/newport",
            "/tmp/Newport/Contents/MacOS/newport",
            "/tmp/Newport.app/Resources/MacOS/newport",
        ] {
            assert!(bundle_for_executable(Path::new(invalid)).is_err());
        }
    }

    #[tokio::test]
    async fn failed_service_drain_never_changes_or_launches_a_bundle() {
        let mut f = Fixture::default();
        assert_eq!(
            execute(&mut f, Err("sessions did not drain".into()), false)
                .await
                .unwrap_err(),
            "sessions did not drain"
        );
        assert!(f.calls.is_empty());
    }

    #[tokio::test]
    async fn accepted_recovery_never_restores_a_possibly_incomplete_backup() {
        for failure in ["replace", "check", "launch"] {
            let mut fixture = Fixture {
                fail: Some(failure),
                ..Default::default()
            };
            assert!(execute(&mut fixture, Ok(()), true).await.is_err());
            assert!(!fixture.calls.contains(&"restore"));
            assert_eq!(fixture.calls.last(), Some(&failure));
        }
        let mut fixture = Fixture::default();
        execute(&mut fixture, Ok(()), true).await.unwrap();
        assert_eq!(fixture.calls, ["replace", "check", "launch"]);
    }
}
