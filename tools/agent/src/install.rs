//! Install managed command aliases while preserving unrelated files.
use std::{env, fs, io, os::unix::fs::symlink, path::PathBuf};
pub fn install() -> io::Result<()> {
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    let dir = home.join(".local/bin");
    fs::create_dir_all(&dir)?;
    for name in ["xclip", "wl-paste", "xdg-open", "newport-browser"] {
        let path = dir.join(name);
        let managed = fs::read_link(&path)
            .ok()
            .is_some_and(|v| crate::migration::managed_alias(&v));
        if managed {
            fs::remove_file(&path)?;
        }
        if name == "newport-browser" && fs::symlink_metadata(&path).is_ok() {
            return Err(io::Error::other(
                "newport-browser is occupied by an unrelated command",
            ));
        }
        if fs::symlink_metadata(&path).is_err() {
            symlink("newport-agent", &path)?;
        }
    }
    crate::migration::commands(&dir)?;
    let ours = env::current_exe()?.canonicalize()?;
    let resolved = env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join("xclip"))
        .find(|p| p.is_file());
    let ready = resolved.and_then(|p| p.canonicalize().ok()).as_ref() == Some(&ours);
    println!(
        "NEWPORT_SHIM_PATH={}",
        if ready { "ready" } else { "missing" }
    );
    if let Err(error) = crate::shell_setup::install() {
        eprintln!("newport-agent: automatic shell setup skipped: {error}");
    }
    Ok(())
}
