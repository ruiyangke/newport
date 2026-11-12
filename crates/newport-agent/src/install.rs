//! Command links and shell setup, written directly. Installing again changes
//! nothing; uninstalling is Newport's job and removes what this writes.
use std::{collections::HashSet, fs, io, os::unix::fs::symlink, path::Path};

/// Commands whose stable paths select the verified browser helper build.
const LINKS: [&str; 2] = ["xdg-open", "newport-browser"];

/// Names whose links to `newport-agent` would shadow the server's own
/// clipboard commands, which Newport terminals' displays serve.
const SHADOWING: [&str; 3] = ["xclip", "xsel", "wl-paste"];

/// `newport-agent install`, run by hand on a server.
pub fn install() -> io::Result<()> {
    if let Some(notice) = crate::deployment::configure_current()? {
        eprintln!("{notice}");
    }
    Ok(())
}

/// Writes the command links and the shell setup. Returns what the person
/// still has to set up by hand, if anything.
pub(crate) fn apply(home: &Path, target: &Path) -> io::Result<Option<String>> {
    // Shell setup first: a malformed startup file fails before any link.
    let notices = crate::shell_setup::apply()?;
    crate::legacy_install::remove(home)?;
    let directory = home.join(".local/bin");
    remove_shadowing(&directory)?;
    refresh_helpers(home, target, true)?;
    Ok(setup_notice(notices))
}

pub(crate) fn setup_notice(notices: Vec<String>) -> Option<String> {
    (!notices.is_empty()).then(|| format!("Manual shell setup needed. {}", notices.join(" ")))
}

/// Removes Newport's links named after the server's clipboard commands from
/// `directory`. Anything else of that name is someone else's and stays.
pub(crate) fn remove_shadowing(directory: &Path) -> io::Result<()> {
    for name in SHADOWING {
        let path = directory.join(name);
        if fs::read_link(&path).is_ok_and(|target| target == Path::new("newport-agent")) {
            match fs::remove_file(&path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => (),
            }
        }
    }
    Ok(())
}

fn managed_build(target: &Path, builds: &Path) -> Option<String> {
    let hash = target.parent()?.file_name()?.to_str()?;
    (newport_protocol::product::valid_build(hash)
        && target == builds.join(hash).join("newport-agent"))
    .then(|| hash.to_owned())
}

fn managed_link(path: &Path, builds: &Path, create: bool, required: bool) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(create),
        Err(error) => Err(error),
        Ok(metadata) if metadata.is_symlink() => {
            let target = fs::read_link(path)?;
            if target == Path::new("newport-agent") || managed_build(&target, builds).is_some() {
                Ok(true)
            } else if required {
                Err(occupied(path))
            } else {
                Ok(false)
            }
        }
        Ok(_) if required => Err(occupied(path)),
        Ok(_) => Ok(false),
    }
}
fn occupied(path: &Path) -> io::Error {
    io::Error::other(format!(
        "{} is occupied by an unrelated command",
        path.display()
    ))
}

/// Repoints owned aliases even when the default entry point stays. Creation
/// requires integration setup; existing unrelated commands always stay.
pub(crate) fn refresh_helpers(home: &Path, target: &Path, create: bool) -> io::Result<()> {
    let builds = home.join(".local/share/newport/needle/builds");
    if managed_build(target, &builds).is_none() {
        return Err(io::Error::other(
            "Browser helpers require an immutable Needle build",
        ));
    }
    let directory = home.join(".local/bin");
    fs::create_dir_all(&directory)?;
    // Check both before changing either, so an unrelated required command
    // cannot leave a partially configured helper pair.
    let mut paths = Vec::new();
    for name in LINKS {
        let path = directory.join(name);
        let required = create && name == "newport-browser";
        if managed_link(&path, &builds, create, required)? {
            paths.push((path, required));
        }
    }
    let staging = tempfile::Builder::new()
        .prefix(".needle-helpers-")
        .tempdir_in(&directory)?;
    for (path, required) in paths {
        // A link already at this build stays as it is.
        if fs::read_link(&path).is_ok_and(|current| current == target)
            || !managed_link(&path, &builds, create, required)?
        {
            continue;
        }
        let link = staging.path().join("helper");
        symlink(target, &link)?;
        fs::rename(link, path)?;
    }
    fs::File::open(directory)?.sync_all()
}

/// Helpers can keep a build in use even when it owns no live process.
pub(crate) fn referenced_builds(directory: &Path, builds: &Path) -> io::Result<HashSet<String>> {
    let mut hashes = HashSet::new();
    for name in LINKS {
        let path = directory.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_symlink() => {
                if let Some(hash) = managed_build(&fs::read_link(path)?, builds) {
                    hashes.insert(hash);
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(hashes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_added_once_and_other_commands_stay() {
        let home = tempfile::Builder::new()
            .prefix("browser home ")
            .tempdir()
            .unwrap();
        let dir = home.path().join(".local/bin");
        fs::create_dir_all(&dir).unwrap();
        let target = home
            .path()
            .join(".local/share/newport/needle/builds")
            .join("a".repeat(64))
            .join("newport-agent");
        let opener = dir.join("xdg-open");
        fs::write(&opener, "someone else's xdg-open").unwrap();
        refresh_helpers(home.path(), &target, true).unwrap();
        assert_eq!(
            fs::read_to_string(&opener).unwrap(),
            "someone else's xdg-open"
        );
        let browser = dir.join("newport-browser");
        use std::os::unix::fs::MetadataExt;
        let created = fs::symlink_metadata(&browser).unwrap().ino();
        refresh_helpers(home.path(), &target, true).unwrap();
        assert_eq!(fs::read_link(&browser).unwrap(), target);
        assert_eq!(
            fs::symlink_metadata(&browser).unwrap().ino(),
            created,
            "a link already at the build is left as it is"
        );
        fs::remove_file(&browser).unwrap();
        fs::write(&browser, "unrelated").unwrap();
        assert!(refresh_helpers(home.path(), &target, true).is_err());
        assert_eq!(fs::read_to_string(&browser).unwrap(), "unrelated");
    }

    #[test]
    fn owned_helpers_move_independently_and_an_unrelated_required_alias_is_preserved() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/bin");
        let builds = home.path().join(".local/share/newport/needle/builds");
        fs::create_dir_all(&bin).unwrap();
        let first = builds.join("a".repeat(64)).join("newport-agent");
        let second = builds.join("b".repeat(64)).join("newport-agent");
        symlink("newport-agent", bin.join("newport-browser")).unwrap();
        refresh_helpers(home.path(), &first, false).unwrap();
        assert_eq!(fs::read_link(bin.join("newport-browser")).unwrap(), first);
        assert!(fs::symlink_metadata(bin.join("xdg-open")).is_err());
        refresh_helpers(home.path(), &second, true).unwrap();
        for name in LINKS {
            assert_eq!(fs::read_link(bin.join(name)).unwrap(), second);
        }
        assert_eq!(
            referenced_builds(&bin, &builds).unwrap(),
            HashSet::from(["b".repeat(64)])
        );
        fs::remove_file(bin.join("newport-browser")).unwrap();
        symlink(
            "/usr/local/bin/unrelated-browser",
            bin.join("newport-browser"),
        )
        .unwrap();
        assert!(refresh_helpers(home.path(), &first, true).is_err());
        assert_eq!(
            fs::read_link(bin.join("xdg-open")).unwrap(),
            second,
            "both aliases are checked before updating"
        );
        assert_eq!(
            fs::read_link(bin.join("newport-browser")).unwrap(),
            Path::new("/usr/local/bin/unrelated-browser")
        );
    }

    #[test]
    fn newports_clipboard_command_links_go_and_other_commands_of_their_names_stay() {
        let dir = tempfile::tempdir().unwrap();
        symlink("newport-agent", dir.path().join("xclip")).unwrap();
        symlink("newport-agent", dir.path().join("wl-paste")).unwrap();
        fs::write(dir.path().join("xsel"), "someone else's xsel").unwrap();
        symlink("/usr/bin/xclip", dir.path().join("other")).unwrap();
        remove_shadowing(dir.path()).unwrap();
        assert!(fs::symlink_metadata(dir.path().join("xclip")).is_err());
        assert!(fs::symlink_metadata(dir.path().join("wl-paste")).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("xsel")).unwrap(),
            "someone else's xsel"
        );
        assert!(fs::symlink_metadata(dir.path().join("other")).is_ok());
        // A directory without them, or none at all, is already clean.
        remove_shadowing(dir.path()).unwrap();
        remove_shadowing(&dir.path().join("missing")).unwrap();
    }
}
