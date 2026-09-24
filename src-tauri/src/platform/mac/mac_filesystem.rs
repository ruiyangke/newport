//! Permissions and durability for local private application files.
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, OpenOptions},
    io,
    path::Path,
};
pub fn application_data_directory() -> Option<std::path::PathBuf> {
    dirs::data_dir()
}

pub fn private_file_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.mode(0o600);
    options
}
pub fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}
pub fn protect_directory(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}
pub fn protect_file(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}
pub fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

pub fn database_url(path: &Path) -> Result<String, String> {
    let url = url::Url::from_file_path(path).map_err(|_| "Invalid metrics database path")?;
    Ok(format!("sqlite:{}", &url.as_str()["file://".len()..]))
}
