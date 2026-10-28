//! Prepare a verified macOS update without modifying the installed application.
use super::{Target, VerifiedUpdate, version};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
};

const MAX_UNPACKED: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const IDENTIFIER: &str = "app.newport";

mod transaction;
pub use transaction::{RecoveryState, RecoveryStatus, StartupGuard, Transaction};

/// The private staging directory is a sibling of the installed bundle so a
/// later installer can exchange them on the same filesystem. Dropping a staged
/// package only removes its own candidate; it never removes the installed app.
pub struct PreparedBundle {
    staging: tempfile::TempDir,
    destination: PathBuf,
    expected_version: semver::Version,
    installed_version: semver::Version,
    team: String,
}

impl PreparedBundle {
    pub fn prepare(update: &VerifiedUpdate, installed: &Path) -> Result<Self, String> {
        if update.target() != Target::MacArm64 {
            return Err("The update is not a macOS ARM64 package.".into());
        }
        if !installed.is_absolute()
            || fs::symlink_metadata(installed)
                .map_err(|error| error.to_string())?
                .file_type()
                .is_symlink()
        {
            return Err("The installed app must be an absolute, non-symlink bundle path.".into());
        }
        let destination = installed
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let installed_version = bundle_version(&destination)?;
        if update.version().cmp_precedence(&installed_version) != std::cmp::Ordering::Greater {
            return Err("The candidate is not newer than the installed app.".into());
        }
        let team = signing_team(&destination)?;
        let staging = tempfile::Builder::new()
            .prefix(".newport-update-")
            .tempdir_in(
                destination
                    .parent()
                    .ok_or("Missing application parent directory.")?,
            )
            .map_err(|error| error.to_string())?;
        crate::filesystem::protect_directory(staging.path()).map_err(|error| error.to_string())?;
        extract(update.bytes(), staging.path())?;
        validate_candidate(&staging.path().join("Newport.app"), update.version(), &team)?;
        Ok(Self {
            staging,
            destination,
            expected_version: update.version().clone(),
            installed_version,
            team,
        })
    }

    pub fn candidate(&self) -> PathBuf {
        self.staging.path().join("Newport.app")
    }
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    pub fn installed_version(&self) -> &semver::Version {
        &self.installed_version
    }

    /// Recheck immediately before installation: an on-disk staged bundle or the
    /// installed app may have changed while the user was deciding to restart.
    pub fn revalidate(&self) -> Result<(), String> {
        if bundle_version(&self.destination)? != self.installed_version
            || signing_team(&self.destination)? != self.team
        {
            return Err("The installed app changed after this update was prepared.".into());
        }
        validate_candidate(&self.candidate(), &self.expected_version, &self.team)
    }

    /// Call after the user chooses to install, not merely after downloading.
    /// Persist recovery information before any replacement. The host must drain
    /// its backend before reconciling this transaction and confirm health after
    /// relaunch before accepting it. Dropping a transaction preserves recovery.
    pub fn begin(self) -> Result<Transaction, String> {
        Transaction::begin(self)
    }
}

fn validate_candidate(path: &Path, expected: &semver::Version, team: &str) -> Result<(), String> {
    if &bundle_version(path)? != expected {
        return Err("The bundle version does not match the signed update version.".into());
    }
    validate_executable(path)?;
    if signing_team(path)? != team {
        return Err("The update was signed by a different developer team.".into());
    }
    Ok(())
}

fn info(path: &Path) -> Result<plist::Dictionary, String> {
    for directory in [path.to_owned(), path.join("Contents")] {
        if !fs::symlink_metadata(directory)
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            return Err("Application bundle directories must not be symlinks.".into());
        }
    }
    if path.extension().and_then(|part| part.to_str()) != Some("app") {
        return Err("Expected an application bundle.".into());
    }
    let info = path.join("Contents/Info.plist");
    let metadata = fs::symlink_metadata(&info).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err("Invalid application Info.plist.".into());
    }
    let value = plist::Value::from_file(info).map_err(|error| error.to_string())?;
    let dict = value
        .into_dictionary()
        .ok_or("Invalid application metadata.")?;
    if dict
        .get("CFBundleIdentifier")
        .and_then(plist::Value::as_string)
        != Some(IDENTIFIER)
        || dict
            .get("CFBundlePackageType")
            .and_then(plist::Value::as_string)
            != Some("APPL")
    {
        return Err("The update is not a Newport application bundle.".into());
    }
    Ok(dict)
}

fn bundle_version(path: &Path) -> Result<semver::Version, String> {
    version(
        info(path)?
            .get("CFBundleShortVersionString")
            .and_then(plist::Value::as_string)
            .ok_or("The bundle has no release version.")?,
    )
}

fn validate_executable(bundle: &Path) -> Result<(), String> {
    executable(bundle).map(|_| ())
}

/// Resolve and validate the bundle's executable after code-signature validation.
/// This validates layout/architecture; it does not replace signature checking.
pub fn executable(bundle: &Path) -> Result<PathBuf, String> {
    let dict = info(bundle)?;
    let name = dict
        .get("CFBundleExecutable")
        .and_then(plist::Value::as_string)
        .ok_or("Missing bundle executable.")?;
    if name.is_empty()
        || Path::new(name).components().count() != 1
        || name.contains(['/', '\\'])
        || name == "."
        || name == ".."
    {
        return Err("Invalid bundle executable name.".into());
    }
    let path = bundle.join("Contents/MacOS").join(name);
    if !fs::symlink_metadata(bundle.join("Contents/MacOS"))
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        return Err("The application executable directory must not be a symlink.".into());
    }
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err("The bundle executable is missing or not executable.".into());
    }
    let mut header = [0u8; 32];
    fs::File::open(&path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| error.to_string())?;
    if u32::from_le_bytes(header[0..4].try_into().unwrap()) != 0xfeedfacf
        || u32::from_le_bytes(header[4..8].try_into().unwrap()) != 0x0100000c
        || u32::from_le_bytes(header[12..16].try_into().unwrap()) != 2
    {
        return Err("The update executable is not an ARM64 Mach-O application.".into());
    }
    Ok(path)
}

fn signing_team(bundle: &Path) -> Result<String, String> {
    let result = Command::new("/usr/bin/codesign")
        .args([
            "--verify",
            "--deep",
            "--strict",
            "-R=identifier \"app.newport\" and anchor apple generic",
        ])
        .arg(bundle)
        .output()
        .map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err("Application code-signature verification failed.".into());
    }
    let result = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=4"])
        .arg(bundle)
        .output()
        .map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err("Cannot read the application's signing identity.".into());
    }
    let output = String::from_utf8(result.stderr).map_err(|error| error.to_string())?;
    let team = output
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))
        .filter(|team| !team.is_empty() && *team != "not set")
        .ok_or("The application has no developer signing team.")?;
    Ok(team.into())
}

fn archive_path(path: &Path) -> Result<PathBuf, String> {
    let mut components = path.components();
    if components.next() != Some(Component::Normal("Newport.app".as_ref())) {
        return Err("The archive must contain exactly one Newport.app root.".into());
    }
    if components.any(|component| !matches!(component, Component::Normal(_))) {
        return Err("The update archive contains an unsafe path.".into());
    }
    Ok(path.to_owned())
}

fn safe_link(path: &Path, target: &Path) -> Result<(), String> {
    if target.is_absolute() {
        return Err("Absolute archive links are not supported.".into());
    }
    let mut depth = path
        .parent()
        .ok_or("Invalid archive link.")?
        .components()
        .count();
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 1 => depth -= 1,
            _ => return Err("An archive link escapes the application bundle.".into()),
        }
    }
    Ok(())
}

fn extract(bytes: &[u8], staging: &Path) -> Result<(), String> {
    // The normal tar iterator decodes PAX/long-name records into memory before
    // yielding files. Inspect raw headers first so metadata cannot bypass the
    // file-size quota or allocate an unbounded buffer during path decoding.
    let mut raw = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut raw_size = 0u64;
    for (index, entry) in raw
        .entries()
        .map_err(|error| error.to_string())?
        .raw(true)
        .enumerate()
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() && !kind.is_symlink() && entry.size() > 1024 * 1024 {
            return Err("The update archive metadata exceeds its size limit.".into());
        }
        raw_size = raw_size
            .checked_add(entry.size())
            .filter(|size| *size <= MAX_UNPACKED)
            .ok_or("The expanded update exceeds its size limit.")?;
        if index >= MAX_ENTRIES {
            return Err("The update archive has too many entries.".into());
        }
    }
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut paths = HashSet::new();
    let mut size = 0u64;
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let path = archive_path(&entry.path().map_err(|error| error.to_string())?)?;
        if !paths.insert(path.clone()) || paths.len() > MAX_ENTRIES {
            return Err("The archive has duplicate paths or too many entries.".into());
        }
        size = size
            .checked_add(entry.size())
            .filter(|size| *size <= MAX_UNPACKED)
            .ok_or("The expanded update exceeds its size limit.")?;
        // Do not write through any previously extracted symlink, even an
        // internal framework alias. Legitimate bundle archives store real files
        // under their actual paths and aliases as separate entries.
        for ancestor in path
            .ancestors()
            .skip(1)
            .filter(|ancestor| !ancestor.as_os_str().is_empty())
        {
            if fs::symlink_metadata(staging.join(ancestor))
                .is_ok_and(|meta| meta.file_type().is_symlink())
            {
                return Err("The archive attempts to write through a symlink.".into());
            }
        }
        let kind = entry.header().entry_type();
        if kind.is_symlink() {
            let target = entry
                .link_name()
                .map_err(|error| error.to_string())?
                .ok_or("Missing archive link target.")?;
            safe_link(&path, &target)?;
        } else if !kind.is_file() && !kind.is_dir() {
            return Err("Unsupported archive entry type.".into());
        }
        if !entry
            .unpack_in(staging)
            .map_err(|error| error.to_string())?
        {
            return Err("An archive entry could not be extracted safely.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
