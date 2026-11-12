//! Exact admission budgets without allocating or compiling the byte codec.
use super::{file_tree, session_launch};

fn varint(value: u64) -> usize {
    ((64 - value.leading_zeros()).max(1) as usize).div_ceil(7)
}
fn field(length: usize) -> usize {
    1 + varint(length as u64) + length
}
pub(super) fn launch(target: &session_launch::Target) -> usize {
    use session_launch::Target;
    field(match target {
        Target::Command { argv } => argv.iter().map(|arg| field(arg.len())).sum(),
        Target::Desktop { id, uris, action } => {
            (if id.is_empty() { 0 } else { field(id.len()) })
                + uris.iter().map(|uri| field(uri.len())).sum::<usize>()
                + action.as_ref().map_or(0, |action| field(action.len()))
        }
    })
}
pub(super) fn manifest(entries: &[file_tree::Entry]) -> usize {
    entries
        .iter()
        .map(|entry| {
            let parent = entry
                .parent
                .map_or(0, |parent| 1 + varint(u64::from(parent)));
            let name = if entry.name.is_empty() {
                0
            } else {
                field(entry.name.len())
            };
            let contents = match entry.contents {
                file_tree::Contents::Directory {} => field(0),
                file_tree::Contents::File { bytes } => 1 + varint(bytes),
            };
            field(parent + name + contents)
        })
        .sum()
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    use crate::encoding::protobuf::generated::newport::remote::apps as pb;
    use prost::Message;

    #[test]
    fn admission_sizes_match_the_codec_at_integer_and_length_boundaries() {
        for length in [0, 1, 127, 128, 255, 16383, 16384, 32768] {
            for target in [
                session_launch::Target::Command {
                    argv: vec![String::new(), "x".repeat(length)],
                },
                session_launch::Target::Desktop {
                    id: "x".repeat(length),
                    uris: vec![String::new()],
                    action: None,
                },
                session_launch::Target::Desktop {
                    id: String::new(),
                    uris: vec!["x".repeat(length)],
                    action: Some(String::new()),
                },
            ] {
                assert_eq!(
                    launch(&target),
                    pb::LaunchTarget::from(&target).encoded_len()
                );
            }
            for parent in [None, Some(0), Some(127), Some(128), Some(u16::MAX)] {
                for bytes in [0, 127, 128, 16383, 16384, u64::MAX] {
                    let entries = [
                        file_tree::Entry {
                            parent,
                            name: "x".repeat(length),
                            contents: file_tree::Contents::Directory {},
                        },
                        file_tree::Entry {
                            parent,
                            name: String::new(),
                            contents: file_tree::Contents::File { bytes },
                        },
                    ];
                    let encoded = pb::upload::BeginTree {
                        transfer: 0,
                        entries: entries.iter().map(Into::into).collect(),
                    };
                    assert_eq!(manifest(&entries), encoded.encoded_len());
                }
            }
        }
    }
}
