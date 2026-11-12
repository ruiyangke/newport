//! Offline release signing.
use anyhow::{Context, Result, bail, ensure};
use newport_release::desktop_agent;
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

fn full_agent_public_key(path: &Path) -> Result<String> {
    const LIMIT: u64 = 16 * 1024;
    let check = |metadata: fs::Metadata| -> Result<u64> {
        ensure!(
            metadata.file_type().is_file(),
            "Updater public key must be a regular file"
        );
        ensure!(
            metadata.len() > 0 && metadata.len() <= LIMIT,
            "Invalid updater public key size"
        );
        Ok(metadata.len())
    };
    check(fs::symlink_metadata(path).context("Cannot inspect updater public key")?)?;
    let file = fs::File::open(path).context("Cannot read updater public key")?;
    let size = check(file.metadata()?)?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == size,
        "Updater public key size changed while reading"
    );
    String::from_utf8(bytes).context("Updater public key must be UTF-8")
}

fn private_key() -> Result<newport_release::SecretKey> {
    if env::var_os("NEWPORT_SIGNING_PRIVATE_KEY_PATH").is_some()
        && env::var_os("NEWPORT_SIGNING_PRIVATE_KEY").is_some()
    {
        bail!("Supply a signing key or a key path, not both");
    }
    let encoded = if let Some(path) = env::var_os("NEWPORT_SIGNING_PRIVATE_KEY_PATH") {
        Zeroizing::new(fs::read_to_string(path).context("Cannot read signing key file")?)
    } else {
        Zeroizing::new(
            env::var("NEWPORT_SIGNING_PRIVATE_KEY")
                .context("Set NEWPORT_SIGNING_PRIVATE_KEY or NEWPORT_SIGNING_PRIVATE_KEY_PATH")?,
        )
    };
    let password = env::var("NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD").unwrap_or_default();
    newport_release::secret_key(&encoded, password)
}

fn run() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let action = args
        .first()
        .and_then(|action| action.to_str())
        .unwrap_or_default();
    if action == "verify-desktop-agent" {
        ensure!(
            args.len() == 5,
            "Usage: newport-release verify-desktop-agent <Newport.app> <signed-agent-directory> <version> <public-key-file>"
        );
        let public = full_agent_public_key(Path::new(&args[4]))?;
        let version = args[3].to_str().context("Version must be UTF-8")?;
        let report = desktop_agent::verify_desktop_agent(
            Path::new(&args[1]),
            Path::new(&args[2]),
            version,
            &public,
            &desktop_agent::run_probe,
        )?;
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    if args.len() != 4
        || !["sign", "needle", "agent", "verify-agent"]
            .iter()
            .any(|action| args[0] == *action)
    {
        bail!(
            "Usage: newport-release sign|needle|agent|verify-agent <artifact-or-agent-directory> <version> <public-key-file>\n       newport-release verify-desktop-agent <Newport.app> <signed-agent-directory> <version> <public-key-file>"
        );
    }
    let version = args[2].to_str().context("Version must be UTF-8")?;
    let public = if args[0] == "agent" || args[0] == "verify-agent" {
        full_agent_public_key(Path::new(&args[3]))?
    } else {
        fs::read_to_string(&args[3]).context("Cannot read updater public key")?
    };
    if args[0] == "verify-agent" {
        newport_release::verify_full_agent_export(&PathBuf::from(&args[1]), version, &public)?;
        println!("Full agent release verified");
        return Ok(());
    }
    let key = private_key()?;
    if args[0] == "agent" {
        newport_release::sign_full_agent_export(&PathBuf::from(&args[1]), version, &public, &key)?;
    } else if args[0] == "needle" {
        newport_release::sign_needle(&PathBuf::from(&args[1]), version, &public, &key)?;
    } else {
        newport_release::sign(&PathBuf::from(&args[1]), version, &public, &key)?;
    }
    println!("Release signature written and verified");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
