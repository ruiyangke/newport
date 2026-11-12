//! Profiles: plain configuration files, with SSH passwords and the account
//! sign-in sealed with a master key kept in OS credential storage.
pub mod account;
mod config_file;
pub(crate) mod import;
mod instance;
pub(crate) mod leftovers;
#[cfg(test)]
mod manager_tests;
mod seal;
pub mod secrets;
mod store;
pub(crate) mod upgrade;
pub use instance::Instance;
pub use secrets::MasterKey;
use std::path::{Path, PathBuf};
pub use store::Store;

/// An explicit profile directory for scripts, tests and development builds,
/// as [`canonical`] gives it.
pub fn data_directory_override() -> Option<PathBuf> {
    std::env::var_os("NEWPORT_DATA_DIR").map(|directory| canonical(Path::new(&directory)))
}

/// The profile directory this process opens.
pub fn directory() -> Result<PathBuf, String> {
    match data_directory_override() {
        Some(path) => Ok(path),
        None => default_directory().map(|directory| canonical(&directory)),
    }
}

/// A profile directory as one path, whichever folder Newport starts in, so
/// the IDs that hash it and the shortcuts that name it agree: absolute,
/// without the `\\?\` prefix Windows may add, in the case it was given.
/// Links are kept: a profile is the path it was named by. Empty stays empty.
pub fn canonical(directory: &Path) -> PathBuf {
    if directory.as_os_str().is_empty() {
        return PathBuf::new();
    }
    let absolute = std::path::absolute(directory).unwrap_or_else(|_| directory.to_owned());
    match absolute.to_str().and_then(without_verbatim) {
        Some(plain) => PathBuf::from(plain),
        None => absolute,
    }
}

/// `\\?\C:\x` as `C:\x`, and `\\?\UNC\host\share` as `\\host\share`;
/// `None` for any other path.
fn without_verbatim(path: &str) -> Option<String> {
    if let Some(share) = path.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{share}"));
    }
    let rest = path.strip_prefix(r"\\?\")?;
    let drive = rest.as_bytes();
    (drive.len() >= 3 && drive[0].is_ascii_alphabetic() && drive[1] == b':' && drive[2] == b'\\')
        .then(|| rest.to_owned())
}

/// Open the product profile with its OS credential adapter.
pub fn open() -> Result<Store, String> {
    Store::open(directory()?, MasterKey::system)
}

fn default_directory() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let base = dirs::data_local_dir();
    #[cfg(not(windows))]
    let base = dirs::data_dir();
    Ok(base
        .ok_or("Cannot locate application data directory")?
        .join("Newport"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_directory_is_one_absolute_path() {
        assert_eq!(canonical(Path::new("")), PathBuf::new());
        let relative = canonical(Path::new("target/profile"));
        assert!(relative.is_absolute(), "{}", relative.display());
        assert_eq!(
            relative,
            std::env::current_dir().unwrap().join("target/profile")
        );
        let absolute = std::env::temp_dir().join("Newport Profile");
        assert_eq!(canonical(&absolute), absolute, "case and spaces stay");
        assert_eq!(canonical(&relative), relative);
    }

    #[test]
    fn windows_verbatim_prefixes_are_dropped() {
        assert_eq!(
            without_verbatim(r"\\?\C:\Users\Ada\Newport").as_deref(),
            Some(r"C:\Users\Ada\Newport")
        );
        assert_eq!(
            without_verbatim(r"\\?\UNC\host\share\Newport").as_deref(),
            Some(r"\\host\share\Newport")
        );
        for kept in [
            r"C:\Users\Ada",
            r"\\host\share",
            r"\\?\Volume{0}\x",
            r"\\?\C:",
            "/Users/ada",
        ] {
            assert_eq!(without_verbatim(kept), None, "{kept}");
        }
    }
}
