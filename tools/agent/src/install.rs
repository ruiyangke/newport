//! Command links and shell setup, written directly. Installing again changes
//! nothing; uninstalling is Newport's job and removes what this writes.
use std::{
    env, fs, io,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

/// The commands Needle answers, as links to `newport-agent` in `~/.local/bin`.
const LINKS: [&str; 5] = ["xclip", "xsel", "wl-paste", "xdg-open", "newport-browser"];

/// `newport-agent install`, run by hand on a server.
pub fn install() -> io::Result<()> {
    if let Some(notice) = apply()? {
        eprintln!("{notice}");
    }
    let ours = env::current_exe()?.canonicalize()?;
    let resolved = env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join("xclip"))
        .find(|p| p.is_file());
    let ready = resolved.and_then(|p| p.canonicalize().ok()).as_ref() == Some(&ours);
    println!(
        "NEWPORT_SHIM_PATH={}",
        if ready { "ready" } else { "missing" }
    );
    Ok(())
}

/// Writes the command links and the shell setup. Returns what the person
/// still has to set up by hand, if anything.
pub(crate) fn apply() -> io::Result<Option<String>> {
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    // Shell setup first: a malformed startup file fails before any link.
    let notices = crate::shell_setup::apply()?;
    crate::legacy_install::remove(&home)?;
    let directory = home.join(".local/bin");
    fs::create_dir_all(&directory)?;
    for name in LINKS {
        link(&directory.join(name), name == "newport-browser")?;
    }
    Ok(setup_notice(notices))
}

pub(crate) fn setup_notice(notices: Vec<String>) -> Option<String> {
    (!notices.is_empty()).then(|| format!("Manual shell setup needed. {}", notices.join(" ")))
}

/// A link to `newport-agent`, unless something else is there: another
/// `xclip` or `xdg-open` stays, and `newport-browser`, which only Needle
/// provides, must be free.
fn link(path: &Path, required: bool) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => symlink("newport-agent", path),
        Err(error) => Err(error),
        Ok(_) if fs::read_link(path).is_ok_and(|target| target == Path::new("newport-agent")) => {
            Ok(())
        }
        Ok(_) if required => Err(io::Error::other(format!(
            "{} is occupied by an unrelated command",
            path.display()
        ))),
        Ok(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_added_once_and_other_commands_stay() {
        let dir = tempfile::tempdir().unwrap();
        let xclip = dir.path().join("xclip");
        fs::write(&xclip, "someone else's xclip").unwrap();
        link(&xclip, false).unwrap();
        assert_eq!(fs::read_to_string(&xclip).unwrap(), "someone else's xclip");
        let browser = dir.path().join("newport-browser");
        link(&browser, true).unwrap();
        link(&browser, true).unwrap();
        assert_eq!(fs::read_link(&browser).unwrap(), Path::new("newport-agent"));
        fs::remove_file(&browser).unwrap();
        fs::write(&browser, "unrelated").unwrap();
        assert!(link(&browser, true).is_err());
    }
}
