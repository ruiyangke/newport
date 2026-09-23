//! Install managed command aliases while preserving unrelated files.
use std::{env, fs, io, os::unix::fs::symlink, path::PathBuf};
pub fn install() -> io::Result<()> {
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    let dir = home.join(".local/bin");
    fs::create_dir_all(&dir)?;
    for name in ["xclip", "wl-paste", "xdg-open", "porthop-browser"] {
        let path = dir.join(name);
        let managed = fs::read_link(&path).ok().is_some_and(|v| {
            matches!(
                v.to_str(),
                Some("porthop-agent" | "porthop-clip" | "porthop-open")
            )
        });
        if managed {
            fs::remove_file(&path)?;
        }
        if name == "porthop-browser" && fs::symlink_metadata(&path).is_ok() {
            return Err(io::Error::other(
                "porthop-browser is occupied by an unrelated command",
            ));
        }
        if fs::symlink_metadata(&path).is_err() {
            symlink("porthop-agent", &path)?;
        }
    }
    // Remove only our obsolete Bash helpers, never unrelated commands.
    for (name, marker) in [
        ("porthop-clip", "# Porthop clipboard helper"),
        ("porthop-open", "# Porthop browser helper"),
    ] {
        let path = dir.join(name);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file())
            && fs::read_to_string(&path).is_ok_and(|s| s.lines().any(|l| l.starts_with(marker)))
        {
            fs::remove_file(path)?;
        }
    }
    let ours = env::current_exe()?.canonicalize()?;
    let resolved = env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join("xclip"))
        .find(|p| p.is_file());
    let ready = resolved.and_then(|p| p.canonicalize().ok()).as_ref() == Some(&ours);
    println!(
        "PORTHOP_SHIM_PATH={}",
        if ready { "ready" } else { "missing" }
    );
    Ok(())
}
