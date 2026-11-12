//! Shared bounds and project facts for server discovery.
use super::Invalid;
use serde::{Deserialize, Serialize};
pub mod discovery;
pub const MAX_LISTENERS: usize = 4096;
pub const MAX_FIELD: usize = 4096;
pub const MAX_ARGUMENTS: usize = MAX_FIELD;
pub const MAX_REPLY_TEXT: usize = 3 * 1024 * 1024;

/// A directory's project, found the way Projectile finds one.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub root: String,
    /// What identified the root: `.git`, `package.json`, `Makefile`…
    pub marker: String,
    /// The checkout the directory is in when the root is a Git repository;
    /// `root` is then the repository every worktree shares.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main: Option<bool>,
}

fn text<'a>(values: impl IntoIterator<Item = &'a str>, bytes: &mut usize) -> Result<(), Invalid> {
    for value in values {
        *bytes = bytes.saturating_add(value.len());
        if value.len() > MAX_FIELD || value.contains('\0') {
            return Err(Invalid("port discovery text"));
        }
    }
    Ok(())
}
fn project(value: &Project, bytes: &mut usize) -> Result<(), Invalid> {
    text(
        [value.root.as_str(), &value.marker]
            .into_iter()
            .chain(value.worktree.as_deref())
            .chain(value.branch.as_deref()),
        bytes,
    )
}
