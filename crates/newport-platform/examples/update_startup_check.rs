//! Opt-in native health-probe check. Requires an explicit isolated profile and
//! candidate executable; never installs, accepts an update, or deletes backups.
#[tokio::main]
async fn main() -> Result<(), String> {
    if std::env::var_os("NEWPORT_DATA_DIR").is_none_or(|value| value.is_empty()) {
        return Err("Set NEWPORT_DATA_DIR to an isolated test profile.".into());
    }
    let executable = std::env::var_os("NEWPORT_UPDATE_TEST_EXECUTABLE")
        .ok_or("Set NEWPORT_UPDATE_TEST_EXECUTABLE to a trusted native Newport executable.")?;
    let metadata: serde_json::Value =
        serde_json::from_str(include_str!("../../../resources/app.json"))
            .map_err(|error| error.to_string())?;
    let expected = semver::Version::parse(
        metadata["version"]
            .as_str()
            .ok_or("Missing product version")?,
    )
    .map_err(|error| error.to_string())?;
    newport_platform::updates::verify_startup(
        std::path::Path::new(&executable),
        &expected,
        std::time::Duration::from_secs(60),
    )
    .await?;
    println!("Startup health verified for {expected}; no update was accepted.");
    Ok(())
}
