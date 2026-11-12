//! Native filesystem invalidations scoped to one leased project watch.
use super::Invalid;
use serde::{Deserialize, Serialize};

pub const MAX_DIRECTORIES: usize = 64;
pub const MAX_PATHS: usize = 2048;
pub const MAX_PATH: usize = 16 * 1024;
pub const MAX_CURSOR: usize = 128;
pub const MAX_REQUEST_TEXT: usize = 48 * 1024;
pub const MAX_REPLY_TEXT: usize = 3 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub watch: String,
    pub root: String,
    pub directories: Vec<String>,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub cursor: Option<String>,
    pub paths: Vec<String>,
    pub reset: bool,
}

/// Remote paths always use Linux separators, including on a Windows desktop.
pub fn path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= MAX_PATH
        && !value.contains('\0')
        && (value == "/"
            || value[1..]
                .split('/')
                .all(|part| !matches!(part, "" | "." | "..")))
}
pub fn within(root: &str, value: &str) -> bool {
    value == root
        || value
            .strip_prefix(root)
            .is_some_and(|rest| root == "/" || rest.starts_with('/'))
}
fn cursor(value: &str) -> bool {
    value.len() <= MAX_CURSOR && !value.contains('\0')
}
impl Request {
    pub fn validate(&self) -> Result<(), Invalid> {
        if !uuid::Uuid::parse_str(&self.watch).is_ok_and(|id| !id.is_nil())
            || !path(&self.root)
            || self.directories.len() > MAX_DIRECTORIES
            || self.cursor.as_ref().is_some_and(|value| !cursor(value))
        {
            return Err(Invalid("file watch request"));
        }
        let mut bytes =
            self.root.len() + self.watch.len() + self.cursor.as_ref().map_or(0, String::len);
        let mut unique = std::collections::BTreeSet::new();
        for directory in &self.directories {
            bytes = bytes.saturating_add(directory.len());
            if !path(directory)
                || !within(&self.root, directory)
                || !unique.insert(directory)
                || bytes > MAX_REQUEST_TEXT
            {
                return Err(Invalid("file watch directory"));
            }
        }
        Ok(())
    }
}
impl Reply {
    pub fn validate_for(&self, request: &Request) -> Result<(), Invalid> {
        if self.paths.len() > MAX_PATHS
            || self
                .cursor
                .as_ref()
                .is_some_and(|value| value.is_empty() || !cursor(value))
        {
            return Err(Invalid("file watch reply"));
        }
        if request.directories.is_empty() {
            if self.cursor.is_some() || !self.paths.is_empty() || self.reset {
                return Err(Invalid("released file watch"));
            }
        } else if self.cursor.is_none() {
            return Err(Invalid("file watch cursor"));
        }
        let mut bytes = 0usize;
        let mut unique = std::collections::BTreeSet::new();
        for value in &self.paths {
            bytes = bytes.saturating_add(value.len());
            if !path(value)
                || !within(&request.root, value)
                || !unique.insert(value)
                || bytes > MAX_REPLY_TEXT
            {
                return Err(Invalid("file watch invalidation"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request {
        Request {
            watch: "11111111-1111-4111-8111-111111111111".into(),
            root: "/repo".into(),
            directories: vec!["/repo/src".into()],
            cursor: None,
        }
    }
    #[test]
    fn linux_paths_cannot_escape_the_watch_on_any_desktop() {
        let mut request = request();
        request.validate().unwrap();
        for value in [
            "/repo/../outside",
            "/repo2/src",
            "/repo/./src",
            "/repo//src",
            "C:\\repo",
            "/repo/src\0",
        ] {
            request.directories = vec![value.into()];
            assert!(request.validate().is_err(), "{value}");
        }
        assert!(path("/"));
    }
    #[test]
    fn watch_replies_need_a_cursor_and_keep_invalidation_budgets() {
        let mut request = request();
        let mut reply = Reply {
            cursor: Some("current".into()),
            paths: vec!["/repo/src/file".into()],
            reset: true,
        };
        reply.validate_for(&request).unwrap();
        reply.paths.push("/other/file".into());
        assert!(reply.validate_for(&request).is_err());
        assert!(Reply::default().validate_for(&request).is_err());
        request.directories.clear();
        Reply::default().validate_for(&request).unwrap();
        assert!(reply.validate_for(&request).is_err());
    }
}
