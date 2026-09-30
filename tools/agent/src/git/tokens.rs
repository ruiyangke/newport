//! Stateless request tokens.
//!
//! `repoId`, `snapshot`, `entryId` and `cursor` used to be keys into tables an
//! RPC process kept for the one client connected to it. They are now
//! self-describing: each carries what the agent needs to act on it, so any
//! agent process of the same user can serve a request, whichever channel issued
//! its tokens -- a reconnect, or a second channel, no longer invalidates them.
//!
//! Nothing here is a secret or an authorisation boundary; SSH is that. A token
//! is a claim the agent re-verifies against the repository on every use: the
//! repository's device and inode, the listing's fingerprint, an entry's
//! presence in the current status. A forged token can therefore only name what
//! the same user could already name by path, and is refused the moment it
//! disagrees with the repository.
//!
//! Layout: `<kind>.<base64url(json)>`. The kind prefix keeps a snapshot from
//! ever being accepted as a repository, and decoding is strict -- bounded
//! length, no unknown fields, validated paths -- and happens before any
//! filesystem work.
use super::protocol::Error;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use git2::Repository;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path},
};

/// Long enough for a cursor wrapping a snapshot wrapping a deep repository
/// path; short enough that a hostile value is refused before it is parsed.
const MAX_TOKEN: usize = 16 * 1024;

fn encode(kind: &str, value: &impl Serialize) -> String {
    let json = serde_json::to_vec(value).expect("token fields serialise");
    format!("{kind}.{}", URL_SAFE_NO_PAD.encode(json))
}
fn decode<T: DeserializeOwned>(kind: &str, token: &str) -> Result<T, Error> {
    let invalid = || Error::invalid(format!("Invalid {} token.", name(kind)));
    if token.len() > MAX_TOKEN {
        return Err(invalid());
    }
    let body = token
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix('.'))
        .ok_or_else(invalid)?;
    let json = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid())?;
    serde_json::from_slice(&json).map_err(|_| invalid())
}
fn name(kind: &str) -> &'static str {
    match kind {
        REPO => "repository",
        SNAPSHOT => "snapshot",
        ENTRY => "file entry",
        _ => "cursor",
    }
}
const REPO: &str = "r";
const SNAPSHOT: &str = "s";
const ENTRY: &str = "e";
const CURSOR: &str = "c";

fn bytes(value: &str) -> Result<Vec<u8>, Error> {
    STANDARD
        .decode(value)
        .map_err(|_| Error::invalid("Invalid path in token."))
}

/// A repository, named by its canonical git directory and the directory's
/// device and inode, so a directory replaced at the same path is noticed.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(deny_unknown_fields)]
pub struct RepoRef {
    g: String,
    d: u64,
    i: u64,
}
impl RepoRef {
    pub fn new(git_dir: &Path, device: u64, inode: u64) -> Self {
        Self {
            g: STANDARD.encode(git_dir.as_os_str().as_bytes()),
            d: device,
            i: inode,
        }
    }
    pub fn encode(&self) -> String {
        encode(REPO, self)
    }
    pub fn decode(token: &str) -> Result<Self, Error> {
        let value: Self = decode(REPO, token)?;
        let path = value.git_dir()?;
        let components = Path::new(OsStr::from_bytes(&path)).components();
        if !path.starts_with(b"/")
            || path.contains(&0)
            || components
                .into_iter()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(Error::invalid("Invalid repository token."));
        }
        Ok(value)
    }
    fn git_dir(&self) -> Result<Vec<u8>, Error> {
        bytes(&self.g)
    }
    /// Re-opens the repository the token names, refusing a directory that is
    /// gone or has been replaced since the token was issued.
    pub fn open(&self) -> Result<Repository, Error> {
        let path = self.checked_path()?;
        Repository::open(path)
            .map_err(|_| Error::new("REPO_NOT_FOUND", "The repository is no longer available."))
    }
    /// Validate identity without opening a Git engine. Shared by both adapters.
    pub fn checked_path(&self) -> Result<std::path::PathBuf, Error> {
        let git_dir = self.git_dir()?;
        let path = Path::new(OsStr::from_bytes(&git_dir));
        let metadata = fs::metadata(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::new("REPO_NOT_FOUND", "The repository is no longer available.")
            } else {
                Error::new("IO_ERROR", "A repository file could not be read.")
            }
        })?;
        if metadata.dev() != self.d || metadata.ino() != self.i {
            return Err(Error::new(
                "REPO_REPLACED",
                "The repository directory was replaced.",
            ));
        }
        Ok(path.to_owned())
    }
}

/// One captured listing: which repository and query it answers, the
/// fingerprint it had, and -- for history -- the commit the walk started from,
/// so later pages walk from the same commit even if the branch moves.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRef {
    pub r: RepoRef,
    pub q: String,
    pub f: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<String>,
}
impl SnapshotRef {
    pub fn encode(&self) -> String {
        encode(SNAPSHOT, self)
    }
    pub fn decode(token: &str) -> Result<Self, Error> {
        decode(SNAPSHOT, token)
    }
}

/// A status entry, named by its raw path bytes: the new path, and for a
/// rename the old one too, exactly as the status scan reports them.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(deny_unknown_fields)]
pub struct EntryRef {
    p: Vec<String>,
}
impl EntryRef {
    pub fn new(paths: &[Vec<u8>]) -> Self {
        Self {
            p: paths.iter().map(|p| STANDARD.encode(p)).collect(),
        }
    }
    pub fn encode(&self) -> String {
        encode(ENTRY, self)
    }
    pub fn decode(token: &str) -> Result<Self, Error> {
        let value: Self = decode(ENTRY, token)?;
        let paths = value.paths()?;
        if paths.is_empty()
            || paths.len() > 2
            || paths.iter().any(|path| {
                path.is_empty()
                    || path.starts_with(b"/")
                    || path.contains(&0)
                    || Path::new(OsStr::from_bytes(path))
                        .components()
                        .any(|c| matches!(c, Component::ParentDir))
            })
        {
            return Err(Error::invalid("Invalid file entry token."));
        }
        Ok(value)
    }
    pub fn paths(&self) -> Result<Vec<Vec<u8>>, Error> {
        self.p.iter().map(|p| bytes(p)).collect()
    }
}

/// A position in a listing: the snapshot it belongs to and an offset.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(deny_unknown_fields)]
pub struct CursorRef {
    pub s: SnapshotRef,
    pub o: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<String>,
}
impl CursorRef {
    pub fn encode(&self) -> String {
        encode(CURSOR, self)
    }
    pub fn decode(token: &str) -> Result<Self, Error> {
        decode(CURSOR, token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo() -> RepoRef {
        RepoRef::new(Path::new("/srv/app/.git"), 7, 42)
    }
    #[test]
    fn tokens_round_trip_and_keep_their_kinds_apart() {
        let r = repo();
        assert_eq!(RepoRef::decode(&r.encode()).unwrap(), r);
        let s = SnapshotRef {
            r: r.clone(),
            q: "status".into(),
            f: "abc".into(),
            p: None,
        };
        assert_eq!(SnapshotRef::decode(&s.encode()).unwrap(), s);
        let c = CursorRef {
            s: s.clone(),
            o: 3,
            k: None,
        };
        assert_eq!(CursorRef::decode(&c.encode()).unwrap(), c);
        let e = EntryRef::new(&[b"src/a.rs".to_vec()]);
        assert_eq!(EntryRef::decode(&e.encode()).unwrap(), e);
        // A snapshot is never accepted where a repository is expected.
        assert_eq!(
            RepoRef::decode(&s.encode()).unwrap_err().code,
            "INVALID_REQUEST"
        );
        assert_eq!(
            SnapshotRef::decode(&r.encode()).unwrap_err().code,
            "INVALID_REQUEST"
        );
    }
    #[test]
    fn malformed_tokens_are_refused_before_any_filesystem_work() {
        for token in [
            "",
            "r.",
            "r.!!!",
            "r.e30",
            "x.e30",
            &format!("r.{}", "A".repeat(MAX_TOKEN)),
        ] {
            assert_eq!(
                RepoRef::decode(token).unwrap_err().code,
                "INVALID_REQUEST",
                "{token:.20}"
            );
        }
        // Unknown fields are refused rather than ignored.
        let extra = format!(
            "r.{}",
            URL_SAFE_NO_PAD.encode(br#"{"g":"L3N2","d":1,"i":2,"x":0}"#)
        );
        assert_eq!(RepoRef::decode(&extra).unwrap_err().code, "INVALID_REQUEST");
        // A relative or traversing repository path.
        for path in ["srv/.git", "/srv/../etc/.git"] {
            let token = RepoRef::new(Path::new(path), 1, 2).encode();
            assert_eq!(
                RepoRef::decode(&token).unwrap_err().code,
                "INVALID_REQUEST",
                "{path}"
            );
        }
        // Entry paths are relative, non-empty and never traverse.
        for paths in [
            vec![b"/etc/passwd".to_vec()],
            vec![b"../x".to_vec()],
            vec![b"a/../../x".to_vec()],
            vec![Vec::new()],
            vec![b"a\0b".to_vec()],
            vec![],
        ] {
            let token = EntryRef::new(&paths).encode();
            assert_eq!(
                EntryRef::decode(&token).unwrap_err().code,
                "INVALID_REQUEST"
            );
        }
    }
    #[test]
    fn a_missing_repository_is_not_found() {
        let token = RepoRef::new(Path::new("/definitely/not/here/.git"), 1, 2);
        assert!(matches!(token.open(), Err(e) if e.code == "REPO_NOT_FOUND"));
    }
}
