use super::super::{Failure, process};
use newport_protocol::monitor::ports::{MAX_FIELD, discovery::valid_cgroup_path};
use std::path::{Path, PathBuf};

const MOUNT_BYTES: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Mount {
    pub id: i32,
    pub root: String,
    pub path: PathBuf,
}

pub(super) fn discover(root: &Path) -> Result<Option<Mount>, Failure> {
    let bytes = process::read(&root.join("proc/self/mountinfo"), MOUNT_BYTES)?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| Failure::Invalid("Invalid mount inventory"))?;
    let mounts = parse(text)?;
    // Prefer the widest visible subtree, then a deterministic mount identity.
    Ok(mounts
        .into_iter()
        .min_by_key(|mount| (mount.root.len(), mount.id)))
}

fn parse(text: &str) -> Result<Vec<Mount>, Failure> {
    let mut result = Vec::new();
    for line in text.lines() {
        let Some((properties, filesystem)) = line.split_once(" - ") else {
            return Err(Failure::Invalid("Invalid mount inventory record"));
        };
        if filesystem.split_whitespace().next() != Some("cgroup2") {
            continue;
        }
        let mut fields = properties.split_whitespace();
        let id = fields
            .next()
            .and_then(|text| text.parse::<i32>().ok())
            .filter(|id| *id > 0)
            .ok_or(Failure::Invalid("Invalid cgroup mount identity"))?;
        let root = fields
            .nth(2)
            .ok_or(Failure::Invalid("Missing cgroup mount root"))?;
        let path = fields
            .next()
            .ok_or(Failure::Invalid("Missing cgroup mount path"))?;
        let root = decode(root)?;
        let path = decode(path)?;
        if !valid_cgroup_path(&root) || !valid_cgroup_path(&path) {
            return Err(Failure::Invalid("Invalid cgroup mount path"));
        }
        result.push(Mount {
            id,
            root,
            path: path.into(),
        });
    }
    Ok(result)
}

fn decode(text: &str) -> Result<String, Failure> {
    if text.len() > MAX_FIELD {
        return Err(Failure::Exhausted("Mount path size limit reached"));
    }
    let mut bytes = Vec::with_capacity(text.len());
    let mut input = text.bytes();
    while let Some(byte) = input.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        let escape = [input.next(), input.next(), input.next()];
        bytes.push(match escape {
            [Some(b'0'), Some(b'4'), Some(b'0')] => b' ',
            [Some(b'0'), Some(b'1'), Some(b'1')] => b'\t',
            [Some(b'0'), Some(b'1'), Some(b'2')] => b'\n',
            [Some(b'1'), Some(b'3'), Some(b'4')] => b'\\',
            _ => return Err(Failure::Invalid("Invalid mount path escape")),
        });
    }
    String::from_utf8(bytes).map_err(|_| Failure::Invalid("Mount path is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mount_inventory_preserves_subtree_roots_and_escaped_mount_points() {
        let mounts = parse("9 1 0:25 / /sys rw - sysfs sysfs rw\n42 9 0:26 /tenant.slice /sys/cgroup\\040v2 rw shared:4 - cgroup2 cgroup rw\n").unwrap();
        assert_eq!(
            mounts,
            [Mount {
                id: 42,
                root: "/tenant.slice".into(),
                path: "/sys/cgroup v2".into()
            }]
        );
    }
    #[test]
    fn invalid_mount_paths_cannot_escape_the_visible_root() {
        for root in ["relative", "/../escape", "/unit//child", "/unit\\000child"] {
            assert!(
                parse(&format!(
                    "42 9 0:26 {root} /sys/fs/cgroup rw - cgroup2 cgroup rw\n"
                ))
                .is_err()
            );
        }
        assert!(parse("broken\n").is_err());
    }
}
