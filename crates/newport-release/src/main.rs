//! Offline release signing, independent of either desktop host.
use anyhow::{Context, Result, bail};
use std::{env, fs, path::PathBuf};
use zeroize::Zeroizing;

fn run() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 4 || (args[0] != "sign" && args[0] != "needle") {
        bail!(
            "Usage: newport-release sign|needle <artifact-or-agent-directory> <version> <public-key-file>"
        );
    }
    let version = args[2].to_str().context("Version must be UTF-8")?;
    let public = fs::read_to_string(&args[3]).context("Cannot read updater public key")?;
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
    let key = newport_release::secret_key(&encoded, password)?;
    if args[0] == "needle" {
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
