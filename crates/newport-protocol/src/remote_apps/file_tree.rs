//! Directory-aware manifest, for uploads and the files of a drag out.
//! Parents precede children; names are single components, never remote paths.
use crate::remote_apps::file_upload::Limits;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_ENTRIES: usize = 256;
pub const MAX_DEPTH: usize = 32;
/// Leaves room for session/context fields within the wire header.
pub const MAX_MANIFEST_BYTES: usize = 24 * 1024;
/// Encoded size budget for one entry of a typical manifest: a file with a
/// three-digit parent, a 16-byte name and a ten-digit size, rounded up.
pub const TYPICAL_ENTRY_BYTES: usize = 32;
/// Entries of typical size that one `BeginTree` manifest can carry.
pub const MANIFEST_ENTRIES: usize = MAX_MANIFEST_BYTES / TYPICAL_ENTRY_BYTES;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub parent: Option<u16>,
    pub name: String,
    pub contents: Contents,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Contents {
    Directory {},
    File { bytes: u64 },
}

/// Validates the entire tree before any filesystem work. Repeated basenames
/// under different parents are legal; duplicate siblings and file parents are not.
pub fn validate(entries: &[Entry]) -> Result<(), String> {
    validate_with(entries, &Limits::DROP)
}

pub fn validate_with(entries: &[Entry], limits: &Limits) -> Result<(), String> {
    if entries.is_empty() || entries.len() > limits.max_entries {
        return Err("Invalid tree entry count".into());
    }
    let mut names = BTreeSet::new();
    let mut depths = Vec::with_capacity(entries.len());
    let mut roots = 0;
    let mut total = 0u64;
    for (index, entry) in entries.iter().enumerate() {
        if entry.name.is_empty()
            || entry.name.len() > 255
            || matches!(entry.name.as_str(), "." | "..")
            || entry
                .name
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
            || !names.insert((entry.parent, &entry.name))
        {
            return Err("Invalid or duplicate tree name".into());
        }
        let depth = if let Some(parent) = entry.parent {
            let parent = usize::from(parent);
            if parent >= index || !matches!(entries[parent].contents, Contents::Directory {}) {
                return Err("Invalid tree parent".into());
            }
            depths[parent] + 1
        } else {
            roots += 1;
            1
        };
        if depth > MAX_DEPTH || roots > limits.max_files {
            return Err("Tree depth or root count exceeded".into());
        }
        depths.push(depth);
        if let Contents::File { bytes } = entry.contents {
            total = total.checked_add(bytes).ok_or("Tree size overflow")?;
        }
    }
    if total > limits.max_batch_bytes {
        return Err("Tree exceeds batch limit".into());
    }
    if manifest_bytes(entries) > MAX_MANIFEST_BYTES {
        return Err("Tree manifest too large".into());
    }
    Ok(())
}

/// The bytes `entries` take in a `BeginTree`.
pub fn manifest_bytes(entries: &[Entry]) -> usize {
    super::size::manifest(entries)
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    use crate::remote_apps::file_upload::{MAX_BATCH_BYTES, MAX_FILES};
    fn directory(parent: Option<u16>, name: &str) -> Entry {
        Entry {
            parent,
            name: name.into(),
            contents: Contents::Directory {},
        }
    }
    #[test]
    fn rejects_cycles_missing_or_file_parents_and_duplicate_siblings() {
        for parent in [0, 1, u16::MAX] {
            assert!(validate(&[directory(Some(parent), "x")]).is_err());
        }
        let root = directory(None, "root");
        assert!(
            validate(&[
                root.clone(),
                directory(Some(0), "x"),
                directory(Some(0), "x")
            ])
            .is_err()
        );
        let file = Entry {
            contents: Contents::File { bytes: 0 },
            ..root.clone()
        };
        assert!(validate(&[file, directory(Some(0), "x")]).is_err());
        validate(&[root, directory(Some(0), "x"), directory(None, "x")]).unwrap();
    }
    #[test]
    fn bounds_names_depth_counts_bytes_and_encoded_manifest() {
        for name in ["", ".", "..", "../x", "/x", "x/y", "x\\y", "x\0", "x\n"] {
            assert!(validate(&[directory(None, name)]).is_err());
        }
        let mut deep = vec![directory(None, "root")];
        for index in 1..MAX_DEPTH {
            deep.push(directory(Some((index - 1) as u16), "x"));
        }
        validate(&deep).unwrap();
        deep.push(directory(Some((MAX_DEPTH - 1) as u16), "x"));
        assert!(validate(&deep).is_err());
        assert!(validate(&[]).is_err());
        let roots: Vec<_> = (0..=MAX_FILES)
            .map(|i| directory(None, &i.to_string()))
            .collect();
        assert!(validate(&roots).is_err());
        for bytes in [MAX_BATCH_BYTES + 1, u64::MAX] {
            assert!(
                validate(&[Entry {
                    parent: None,
                    name: "x".into(),
                    contents: Contents::File { bytes }
                }])
                .is_err()
            );
        }
        for sizes in [[MAX_BATCH_BYTES, 1], [u64::MAX, 1]] {
            let entries: Vec<_> = sizes
                .into_iter()
                .enumerate()
                .map(|(index, bytes)| Entry {
                    parent: None,
                    name: index.to_string(),
                    contents: Contents::File { bytes },
                })
                .collect();
            assert!(validate(&entries).is_err());
        }
        let mut wide = vec![directory(None, "root")];
        for index in 1..MAX_ENTRIES {
            wide.push(directory(Some(0), &index.to_string()));
        }
        validate(&wide).unwrap();
        wide.push(directory(Some(0), "extra"));
        assert!(validate(&wide).is_err());
        wide.pop();
        for entry in &mut wide[1..] {
            entry.name.push_str(&"a".repeat(200));
        }
        assert!(validate(&wide).unwrap_err().contains("manifest"));
        assert!(
            serde_json::from_str::<Entry>(
                r#"{"parent":null,"name":"x","contents":{"kind":"directory","bytes":1}}"#
            )
            .is_err()
        );
    }
    fn file(parent: Option<u16>, name: &str, bytes: u64) -> Entry {
        Entry {
            parent,
            name: name.into(),
            contents: Contents::File { bytes },
        }
    }
    #[test]
    fn drop_limits_reject_what_they_always_rejected() {
        assert_eq!(Limits::DROP.max_entries, 256);
        assert_eq!(Limits::DROP.max_files, 32);
        assert_eq!(Limits::DROP.max_batch_bytes, 128 * 1024 * 1024);
        let many: Vec<_> = (0..=MAX_ENTRIES)
            .map(|i| directory(None, &i.to_string()))
            .collect();
        assert!(validate(&many).is_err());
        assert!(validate_with(&many, &Limits::DROP).is_err());
        assert!(validate(&[file(None, "big", MAX_BATCH_BYTES + 1)]).is_err());
        let roots: Vec<_> = (0..=MAX_FILES)
            .map(|i| file(None, &i.to_string(), 0))
            .collect();
        assert!(validate(&roots).is_err());
    }
    #[test]
    fn clipboard_limits_accept_batches_beyond_the_drop_limits() {
        let limits = Limits::CLIPBOARD;
        let roots: Vec<_> = (0..MAX_FILES * 2)
            .map(|i| file(None, &i.to_string(), 0))
            .collect();
        assert!(validate(&roots).is_err());
        validate_with(&roots, &limits).unwrap();
        let big = [file(None, "big", MAX_BATCH_BYTES + 1)];
        assert!(validate(&big).is_err());
        validate_with(&big, &limits).unwrap();
        validate_with(&[file(None, "max", limits.max_batch_bytes)], &limits).unwrap();
        assert!(validate_with(&[file(None, "max", limits.max_batch_bytes + 1)], &limits).is_err());
        let mut wide = vec![directory(None, "root")];
        for index in 1..=MAX_ENTRIES {
            wide.push(directory(Some(0), &index.to_string()));
        }
        assert!(validate(&wide).is_err());
        validate_with(&wide, &limits).unwrap();
    }
    #[test]
    fn clipboard_entry_limit_is_what_one_manifest_carries() {
        let limits = Limits::CLIPBOARD;
        assert_eq!(limits.max_entries, MANIFEST_ENTRIES);
        assert_eq!(MANIFEST_ENTRIES, 768);
        assert!(
            manifest_bytes(&[file(Some(999), &"n".repeat(16), 9_999_999_999)])
                <= TYPICAL_ENTRY_BYTES
        );
        const { assert!(MANIFEST_ENTRIES > MAX_ENTRIES) };
        let mut entries = vec![directory(None, "root")];
        while entries.len() < limits.max_entries {
            let name = format!("{:0>16}", entries.len());
            entries.push(file(Some(0), &name, 9_999_999_999));
        }
        // Sizes of ten digits are encoded only; their sum exceeds any batch.
        let mut sized = entries.clone();
        for entry in &mut sized[1..] {
            entry.contents = Contents::File { bytes: 0 };
        }
        validate_with(&sized, &limits).unwrap();
        assert!(manifest_bytes(&entries) <= MAX_MANIFEST_BYTES);
        let message = crate::remote_apps::v1::ClientMessage::FileUpload {
            session: u64::MAX,
            message: crate::remote_apps::file_upload::Client::BeginTree {
                transfer: u64::MAX,
                entries,
            },
        };
        assert!(
            crate::remote_apps::wire::encode_client(&message)
                .unwrap()
                .len()
                <= crate::remote_apps::CLIENT_LIMITS.message
        );
    }
}
