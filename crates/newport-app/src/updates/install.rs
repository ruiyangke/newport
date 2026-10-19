//! macOS install handoff. Preparation does not change the live app. Accepting
//! restart persists a durable transaction, then quits and relaunches. AppKit
//! exits inside terminate:, so the relaunched app replaces the bundle during
//! startup recovery, before it loads credentials or starts the backend.
use newport_platform::updates::{
    VerifiedUpdate,
    macos::{self, PreparedBundle, Transaction},
};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

pub struct Request {
    prepared: PreparedBundle,
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
        Ok(Self { prepared })
    }

    /// Acquire the installation lock and persist intent while the backend
    /// still owns the profile. No exchange happens.
    pub fn begin(self) -> Result<Begun, String> {
        self.prepared.begin().map(Begun)
    }
}

/// Persisted install intent the UI has not confirmed yet. Dropping it keeps
/// the journal (the next launch installs); a refused handoff abandons it.
pub struct Begun(Transaction);
impl Begun {
    pub fn relaunch(&self) -> Result<Relaunch, String> {
        Relaunch::spawn(
            std::process::id(),
            Path::new("/usr/bin/open"),
            &[self.0.destination().as_os_str()],
        )
        .map_err(|error| format!("Couldn’t restart Newport: {error}"))
    }
    pub fn abandon(self) -> Result<(), String> {
        self.0.abandon()
    }
}

/// Runs a command once this process has exited. Dropping it before then
/// stops the waiter, so an abandoned handoff never reopens the app later.
pub struct Relaunch(Option<Child>);
impl Relaunch {
    fn spawn(pid: u32, program: &Path, arguments: &[&OsStr]) -> std::io::Result<Self> {
        use std::os::unix::process::CommandExt;
        Command::new("/bin/sh")
            .arg("-c")
            .arg(r#"while kill -0 "$0" 2>/dev/null; do sleep 0.1; done; exec "$@""#)
            .arg(pid.to_string())
            .arg(program)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Not in this app's process group: signals to it must not reach the waiter.
            .process_group(0)
            .spawn()
            .map(|child| Self(Some(child)))
    }
    /// Keep the waiter running after this process exits.
    pub fn release(mut self) {
        self.0.take();
    }
}
impl Drop for Relaunch {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub enum Handoff<B, R> {
    /// Quit now; the relaunched app installs.
    Quit(R),
    /// Intent was persisted, but the restart was refused or nobody waited.
    Abandon(B),
    /// Nothing was persisted.
    Refused,
}

/// Order the restart handoff. Intent is persisted before the unsaved-buffer
/// check, so nothing can edit a buffer between that check and quit. Every
/// refusal after that abandons the persisted installation.
pub fn handoff<B, R>(
    begun: Result<B, String>,
    check: impl FnOnce() -> Result<(), String>,
    relaunch: impl FnOnce(&B) -> Result<R, String>,
    reply: impl FnOnce(Result<(), String>) -> bool,
) -> Handoff<B, R> {
    let begun = match begun {
        Ok(begun) => begun,
        Err(error) => {
            reply(Err(error));
            return Handoff::Refused;
        }
    };
    match check().and_then(|()| relaunch(&begun)) {
        Err(error) => {
            reply(Err(error));
            Handoff::Abandon(begun)
        }
        // A timed-out/cancelled requester must never arm an install. Dropping
        // the relaunch stops its waiter before the caller abandons.
        Ok(relaunch) if !reply(Ok(())) => {
            drop(relaunch);
            Handoff::Abandon(begun)
        }
        Ok(relaunch) => Handoff::Quit(relaunch),
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
    profile: PathBuf,
}
impl Installation {
    pub(super) fn resume(
        transaction: Transaction,
        version: semver::Version,
        accepted: bool,
        profile: PathBuf,
    ) -> Self {
        Self {
            destination: transaction.destination().to_owned(),
            recovery: transaction.recovery_directory().to_owned(),
            transaction: Some(transaction),
            version,
            accepted,
            profile,
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
    fn report(&mut self, error: &str);
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
    fn report(&mut self, error: &str) {
        if let Err(record) = super::recovery::record_failure(&self.profile, &self.version, error) {
            eprintln!("Couldn’t record the update failure: {record}");
        }
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
        // Record the reason before anything relaunches: stderr is discarded
        // for apps opened from Finder, so the next app shows it in Settings.
        driver.report(&error);
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
        fn report(&mut self, _: &str) {
            self.calls.push("report");
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
            ("replace", vec!["replace", "report", "restore", "launch"]),
            (
                "check",
                vec!["replace", "check", "report", "restore", "launch"],
            ),
            (
                "launch",
                vec!["replace", "check", "launch", "report", "restore", "launch"],
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
        assert_eq!(f.calls, ["replace", "check", "report", "restore"]);
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
            assert_eq!(fixture.calls.last(), Some(&"report"));
        }
        let mut fixture = Fixture::default();
        execute(&mut fixture, Ok(()), true).await.unwrap();
        assert_eq!(fixture.calls, ["replace", "check", "launch"]);
    }

    // Handoff fixtures: the persisted intent and the relaunch waiter record
    // whether they were abandoned or released.
    struct Intent(&'static str);
    struct Waiter<'a>(&'a std::cell::RefCell<Vec<&'static str>>);
    impl Drop for Waiter<'_> {
        fn drop(&mut self) {
            self.0.borrow_mut().push("waiter stopped");
        }
    }

    #[test]
    fn handoff_checks_buffers_after_persisting_and_relaunches_before_replying() {
        let calls = std::cell::RefCell::new(Vec::new());
        let outcome = handoff(
            Ok(Intent("persisted")),
            || {
                calls.borrow_mut().push("check");
                Ok(())
            },
            |_| {
                calls.borrow_mut().push("relaunch");
                Ok(Waiter(&calls))
            },
            |result| {
                calls.borrow_mut().push("reply");
                result.is_ok()
            },
        );
        let Handoff::Quit(waiter) = outcome else {
            panic!("an accepted handoff must quit");
        };
        assert_eq!(*calls.borrow(), ["check", "relaunch", "reply"]);
        std::mem::forget(waiter);
    }

    #[test]
    fn refused_handoffs_report_the_reason_and_abandon_persisted_intent() {
        // Failed persistence: nothing to abandon, and the requester learns why.
        let replied = std::cell::RefCell::new(None);
        let outcome = handoff::<Intent, ()>(
            Err("locked".into()),
            || panic!("never check buffers without persisted intent"),
            |_| panic!("never relaunch without persisted intent"),
            |result| {
                *replied.borrow_mut() = Some(result);
                true
            },
        );
        assert!(matches!(outcome, Handoff::Refused));
        assert_eq!(replied.take(), Some(Err("locked".into())));

        // Unsaved buffers or a failed relaunch: report and abandon.
        for (dirty, relaunch_fails) in [(true, false), (false, true)] {
            let replied = std::cell::RefCell::new(None);
            let outcome = handoff::<Intent, ()>(
                Ok(Intent("persisted")),
                || if dirty { Err("unsaved".into()) } else { Ok(()) },
                |_| {
                    assert!(!dirty, "never relaunch with unsaved buffers");
                    if relaunch_fails {
                        Err("no waiter".into())
                    } else {
                        Ok(())
                    }
                },
                |result| {
                    *replied.borrow_mut() = Some(result);
                    true
                },
            );
            assert!(matches!(outcome, Handoff::Abandon(Intent("persisted"))));
            assert!(replied.take().unwrap().is_err());
        }

        // A requester that stopped waiting: stop the waiter, then abandon.
        let calls = std::cell::RefCell::new(Vec::new());
        let outcome = handoff(
            Ok(Intent("persisted")),
            || Ok(()),
            |_| Ok(Waiter(&calls)),
            |_| false,
        );
        assert_eq!(*calls.borrow(), ["waiter stopped"]);
        assert!(matches!(outcome, Handoff::Abandon(Intent("persisted"))));
    }

    fn eventually(condition: impl Fn() -> bool) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        condition()
    }

    #[test]
    fn relaunch_runs_only_after_the_process_exits() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("relaunched");
        let mut app = Command::new("/bin/sleep").arg("0.5").spawn().unwrap();
        let relaunch =
            Relaunch::spawn(app.id(), Path::new("/usr/bin/touch"), &[marker.as_os_str()]).unwrap();
        relaunch.release();
        std::thread::sleep(Duration::from_millis(200));
        assert!(!marker.exists(), "relaunched while the app was running");
        // Reap it, as launchd does once a real app exits.
        app.wait().unwrap();
        assert!(eventually(|| marker.exists()));
    }

    #[test]
    fn dropping_a_relaunch_stops_the_waiter() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("relaunched");
        let mut app = Command::new("/bin/sleep").arg("0.3").spawn().unwrap();
        drop(
            Relaunch::spawn(app.id(), Path::new("/usr/bin/touch"), &[marker.as_os_str()]).unwrap(),
        );
        app.wait().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists());
    }
}
