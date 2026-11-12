//! Opt-in signed-artifact exercise. Copies the supplied app into a disposable
//! directory, installs/restores there, and NEVER launches either app version.
#[cfg(target_os = "macos")]
fn main() -> Result<(), String> {
    use newport_platform::updates::{
        Target, Updater,
        macos::{PreparedBundle, RecoveryState, StartupGuard, Transaction},
    };
    let source = std::env::var_os("NEWPORT_UPDATE_TEST_APP").ok_or("Set NEWPORT_UPDATE_TEST_APP to a signed old Newport.app; only a disposable copy is modified.")?;
    let source = std::path::PathBuf::from(source)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let info = plist::Value::from_file(source.join("Contents/Info.plist"))
        .map_err(|error| error.to_string())?;
    let previous = info
        .as_dictionary()
        .and_then(|info| info.get("CFBundleShortVersionString"))
        .and_then(plist::Value::as_string)
        .ok_or("Missing old app version.")?;
    println!("Checking for a signed update newer than {previous}.");
    let update = Updater::production(previous, Target::MacArm64)?
        .check(|| false, |_| {})?
        .ok_or("No newer signed release is available for this test app.")?;
    let root = tempfile::tempdir().map_err(|error| error.to_string())?;
    newport_platform::filesystem::protect_directory(root.path())
        .map_err(|error| error.to_string())?;
    let destination = root.path().join("Newport.app");
    let copied = std::process::Command::new("/usr/bin/ditto")
        .arg(&source)
        .arg(&destination)
        .status()
        .map_err(|error| error.to_string())?;
    if !copied.success() {
        return Err("Could not copy the old test app.".into());
    }
    println!(
        "Preparing the verified {} package beside the disposable copy.",
        update.version()
    );
    let prepared = PreparedBundle::prepare(&update, &destination)?;
    let transaction = prepared.begin()?;
    let recovery = transaction.recovery_directory().to_owned();
    let destination = transaction.destination().to_owned();
    drop(transaction); // exercise reopening the durable record before exchange
    let guard = StartupGuard::acquire(&destination)?;
    assert!(StartupGuard::acquire(&destination).is_err());
    let mut transaction = guard
        .into_transaction()
        .ok_or("Recovery journal not found")?;
    let status = transaction.inspect()?;
    assert_eq!(status.installed.to_string(), previous);
    assert_eq!(&status.intended, update.version());
    assert!(!status.accepted);
    assert_eq!(transaction.reconcile()?, RecoveryState::Updated);
    println!("Signed bundle replacement succeeded; reopening recovery.");
    drop(transaction);
    let mut transaction =
        Transaction::discover(&destination)?.ok_or("Recovery journal not found")?;
    let status = transaction.inspect()?;
    assert_eq!(&status.installed, update.version());
    assert_eq!(status.installed, status.intended);
    assert_eq!(transaction.reconcile()?, RecoveryState::Updated);
    assert_eq!(transaction.restore()?, RecoveryState::Previous);
    drop(transaction);
    let mut transaction =
        Transaction::discover(&destination)?.ok_or("Recovery journal not found")?;
    assert_eq!(transaction.reconcile()?, RecoveryState::Previous);
    let status = transaction.inspect()?;
    assert_eq!(status.installed.to_string(), previous);
    assert_eq!(status.installed, status.intended);
    transaction.accept(&semver::Version::parse(previous).map_err(|error| error.to_string())?)?;
    assert!(!recovery.exists());
    assert!(Transaction::discover(&destination)?.is_none());
    println!(
        "Signed bundle restoration and recovery cleanup succeeded. No app was launched; source unchanged."
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() -> Result<(), String> {
    Err("This signed bundle transaction check runs on macOS.".into())
}
