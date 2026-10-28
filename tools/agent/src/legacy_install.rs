//! Remove the remote installation written by Porthop, before installing new
//! command links. Recognize commands by link targets or embedded markers;
//! another program with the same name is not ours to remove.
use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(crate) fn remove(home: &Path) -> io::Result<()> {
    let bin = home.join(".local/bin");
    remove_links(&bin)?;
    for name in ["porthop-clip", "porthop-open", "porthop-agent"] {
        let path = bin.join(name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.uid() == unsafe { libc::geteuid() } => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.is_file() && metadata.len() <= 128 * 1024 * 1024 {
            let mut bytes = Vec::new();
            fs::File::open(&path)?
                .take(128 * 1024 * 1024)
                .read_to_end(&mut bytes)?;
            let owned = match name {
                "porthop-agent" => bytes.windows(b"porthop-agent/1".len()).any(|window| {
                    window.starts_with(b"porthop-agent/")
                        && window.last().is_some_and(u8::is_ascii_digit)
                }),
                "porthop-clip" => script(&bytes, "# Porthop clipboard helper"),
                "porthop-open" => script(&bytes, "# Porthop browser helper"),
                _ => false,
            };
            if owned {
                if name == "porthop-agent" {
                    stop_agent(&path)?;
                }
                fs::remove_file(path)?;
            }
        }
    }
    // These are the state locations the old remote installer actually wrote.
    // Keep unrelated files under the branded parent, and never follow a link
    // out of it while removing a child.
    for (directory, child) in [
        (".cache/porthop", "clipboard"),
        (".local/share/porthop", "shell-setup.lock"),
    ] {
        let root = home.join(directory);
        let metadata = match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.uid() == unsafe { libc::geteuid() } => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.is_symlink() {
            fs::remove_file(root)?;
        } else if metadata.is_dir() {
            let path = root.join(child);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
                Ok(_) => fs::remove_file(path)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            match fs::remove_dir(root) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

/// Inspect every command link, including custom names. Snapshot the links
/// before deleting any, so a chain such as `copy -> xclip -> porthop-agent`
/// is removed regardless of directory order, even if the agent is missing.
fn remove_links(bin: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(bin) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let canonical_bin = bin.canonicalize()?;
    struct Link {
        path: PathBuf,
        name: OsString,
        target: PathBuf,
        destination: Option<OsString>,
    }
    let mut links = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !metadata.is_symlink() || metadata.uid() != unsafe { libc::geteuid() } {
            continue;
        }
        let target = fs::read_link(&path)?;
        let absolute = bin.join(&target);
        // Resolve only the parent: the final helper may already be gone.
        // This also handles ../bin/helper and symlinked home directories.
        let destination = absolute
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .filter(|parent| parent == &canonical_bin)
            .and_then(|_| absolute.file_name().map(OsString::from));
        links.push(Link {
            path,
            name: entry.file_name(),
            target,
            destination,
        });
    }
    let legacy: HashSet<OsString> = [
        "porthop-agent",
        "porthop-clip",
        "porthop-open",
        "porthop-browser",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    let mut destinations = legacy.clone();
    let mut remove = HashSet::new();
    loop {
        let mut changed = false;
        for link in &links {
            let Some(destination) = &link.destination else {
                continue;
            };
            if destinations.contains(destination)
                || (legacy.contains(&link.name) && destination == "newport-agent")
            {
                remove.insert(link.name.clone());
                changed |= destinations.insert(link.name.clone());
            }
        }
        if !changed {
            break;
        }
    }
    for link in links {
        if remove.contains(&link.name)
            && fs::symlink_metadata(&link.path).is_ok_and(|metadata| {
                metadata.is_symlink() && metadata.uid() == unsafe { libc::geteuid() }
            })
            && fs::read_link(&link.path).is_ok_and(|target| target == link.target)
        {
            fs::remove_file(link.path)?;
        }
    }
    Ok(())
}

fn script(bytes: &[u8], marker: &str) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| text.lines().any(|line| line.starts_with(marker)))
}

#[cfg(target_os = "linux")]
fn stop_agent(path: &Path) -> io::Result<()> {
    let path = path.canonicalize()?;
    for entry in fs::read_dir("/proc")?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        if pid <= 1
            || !entry
                .metadata()
                .is_ok_and(|metadata| metadata.uid() == unsafe { libc::geteuid() })
        {
            continue;
        }
        if fs::read_link(entry.path().join("exe")).is_ok_and(|exe| exe == path) {
            // The obsolete agent has no state to save; don't let it recreate
            // the clipboard cache after removal.
            if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn stop_agent(_: &Path) -> io::Result<()> {
    Ok(())
}
