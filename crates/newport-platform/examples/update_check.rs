//! Read-only production updater check. Verifies available packages, never installs.
use newport_platform::updates::{Target, Updater};

fn main() -> Result<(), String> {
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Target::MacArm64,
        ("windows", "x86_64") => Target::WindowsX64,
        ("windows", "aarch64") => Target::WindowsArm64,
        _ => return Err("This platform is not a Newport release target.".into()),
    };
    let product: serde_json::Value =
        serde_json::from_str(include_str!("../../../resources/app.json"))
            .map_err(|error| error.to_string())?;
    let current = product["version"]
        .as_str()
        .ok_or("Missing product version")?;
    let updater = Updater::production(current, target)?;
    match updater.check(|| false, |_| {})? {
        Some(update) => println!(
            "Verified Newport {} for {} ({} bytes); nothing installed.",
            update.version(),
            update.target().key(),
            update.bytes().len()
        ),
        None => println!(
            "No update newer than Newport {current} for {}.",
            target.key()
        ),
    }
    Ok(())
}
