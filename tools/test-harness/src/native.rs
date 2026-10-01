use crate::{
    command,
    process::{self, Process},
    root,
};
use anyhow::{ensure, Result};
use std::{fs, os::unix::fs::PermissionsExt, time::Duration};
pub fn run() -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "Native lifecycle smoke test requires macOS"
    );
    let binary =
        root().join("src-tauri/target/release/bundle/macos/Newport.app/Contents/MacOS/newport");
    ensure!(
        binary.exists(),
        "Build the release app bundle before running native smoke tests"
    );
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("profile");
    fs::create_dir(&profile)?;
    let key = temp.path().join("fixture-key");
    let bytes = process::capture(command("openssl").args(["rand", "32"]), None)?;
    fs::write(&key, bytes)?;
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600))?;
    fs::write(
        profile.join("config.json"),
        serde_json::to_vec(
            &serde_json::json!({"servers":[{"id":"8d43df10-73f7-4358-b818-c41dddc2006e","name":"Offline fixture","sshHost":"127.0.0.1","sshPort":1,"sshUser":"fixture","clipboardEnabled":true}],"tunnels":[]}),
        )?,
    )?;
    let launch = || {
        let mut c = command(&binary);
        c.env("NEWPORT_DATA_DIR", &profile)
            .env("NEWPORT_TEST_VAULT_KEY_FILE", &key);
        c
    };
    let mut app = Process::spawn(&mut launch())?;
    process::ready(&profile.join("profiles.stronghold"), &mut app)?;
    process::ready(&profile.join("preferences.json"), &mut app)?;
    let preferences: serde_json::Value =
        serde_json::from_slice(&fs::read(profile.join("preferences.json"))?)?;
    assert_eq!(preferences["sidebarWidth"].as_f64(), Some(204.0));
    assert_eq!(
        fs::metadata(profile.join("metrics.sqlite3"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let sql = |query: &str| {
        process::text(
            command("/usr/bin/sqlite3")
                .arg("-readonly")
                .arg(profile.join("metrics.sqlite3"))
                .arg(query),
        )
    };
    assert_eq!(sql("PRAGMA user_version;")?, "2");
    assert_eq!(
        sql("SELECT version FROM _sqlx_migrations WHERE success=1 ORDER BY version;")?,
        "1\n2"
    );
    assert_eq!(sql("SELECT COUNT(*) FROM samples;")?, "0");
    Process::spawn(&mut launch())?.success(Duration::from_secs(5))?;
    let stop = |app: &mut Process| -> Result<()> {
        unsafe {
            libc::kill(app.0.id() as i32, libc::SIGTERM);
        }
        app.success(Duration::from_secs(10))
    };
    stop(&mut app)?;
    assert!(!profile.join("config.json").exists());
    let vault = fs::read(profile.join("profiles.stronghold"))?;
    assert!(!vault.is_empty());
    let logs = || fs::read_to_string(profile.join("logs/newport.log"));
    for text in [
        "Desktop and background workers initialized",
        "Background workers and SSH sessions stopped",
        "Restoring clipboard sharing for 1 saved profiles",
    ] {
        assert!(logs()?.contains(text), "{text}");
    }
    assert_eq!(fs::metadata(&profile)?.permissions().mode() & 0o777, 0o700);
    let mut app = Process::spawn(launch().arg("--autostart"))?;
    std::thread::sleep(Duration::from_secs(2));
    ensure!(app.0.try_wait()?.is_none(), "Background launch exited");
    stop(&mut app)?;
    assert_eq!(fs::read(profile.join("profiles.stronghold"))?, vault);
    // Once the app has checkpointed and closed, WAL sidecars no longer exist.
    // Immutable mode lets Apple's SQLite read that closed database without
    // trying to create shared-memory files through a read-only connection.
    let persisted = process::text(
        command("/usr/bin/sqlite3")
            .arg("-readonly")
            .arg(format!(
                "file:{}?immutable=1",
                profile.join("metrics.sqlite3").display()
            ))
            .arg("SELECT COUNT(*) FROM _sqlx_migrations;"),
    )?;
    assert_eq!(persisted, "2");
    assert!(!profile.join("config.json").exists());
    assert_eq!(
        logs()?
            .matches("Restoring clipboard sharing for 1 saved profiles")
            .count(),
        2
    );
    println!("PASS: native lifecycle, encrypted migration, SQL, preferences and instance lock");
    Ok(())
}
