//! Server socket discovery and bounded project metadata.
#[cfg(target_os = "linux")]
pub mod discovery;
mod project;
use newport_protocol::monitor::ports::{MAX_FIELD, Project};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
struct Host {
    root: PathBuf,
}
impl Host {
    fn new(root: &Path) -> Self {
        Self {
            root: std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned()),
        }
    }
    fn path(&self, logical: &str) -> PathBuf {
        self.root.join(logical.trim_start_matches('/'))
    }
}

fn truncate(text: &mut String, limit: usize) {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

fn status_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

pub(crate) fn users(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let uid = fields.nth(1)?.parse().ok()?;
            Some((uid, name.to_owned()))
        })
        .collect()
}
