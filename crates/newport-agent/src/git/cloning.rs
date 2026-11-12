//! Flush a prepared repository before atomic publication.
use super::protocol::Error;
use std::{
    fs::{self, File},
    path::Path,
};
fn io_error(_: std::io::Error) -> Error {
    Error::new("IO_ERROR", "Cannot prepare the clone destination.")
}
pub(super) fn sync_tree(path: &Path, count: &mut usize) -> Result<(), Error> {
    *count += 1;
    if *count > 250_000 {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "The clone exceeds the publication entry limit.",
        ));
    }
    let meta = fs::symlink_metadata(path).map_err(io_error)?;
    if meta.is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path).map_err(io_error)? {
            sync_tree(&entry.map_err(io_error)?.path(), count)?;
        }
    } else if !meta.is_file() {
        return Err(Error::new(
            "UNSUPPORTED_CAPABILITY",
            "The clone contains an unsupported special file.",
        ));
    }
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(io_error)
}
