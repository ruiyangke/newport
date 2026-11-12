//! What a session can do beyond the base protocol.
//!
//! `Hello.features` is what the client will do in this session;
//! `Welcome.features` is what the session has: the part of it this display
//! serves. A feature is an `Option`; a sub-feature is a field of its parent's
//! type, so it can't be granted without the parent.
use serde::{Deserialize, Serialize};

/// The root of the tree.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Features {
    /// Input method composition, for each viewer of the display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_input: Option<Present>,
    /// Uploads with folders, and dropping an upload into a window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload: Option<Present>,
    /// Dragging text and images out of remote apps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drag_out: Option<DragOut>,
    /// Opus streams with their state, and capture times on frames and audio
    /// so a client keeps them in sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<Present>,
}

/// `drag_out`'s sub-features.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DragOut {
    /// Files and folders too, staged through the file tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Present>,
}

/// A feature with no parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Present {}

pub trait Grant: Sized {
    /// The part of `wanted` this server serves, or None.
    fn grant(&self, wanted: &Self) -> Option<Self>;
}

impl Grant for Present {
    fn grant(&self, _: &Self) -> Option<Self> {
        Some(Self {})
    }
}

/// A feature is granted when it was wanted and is served, and then only the
/// part of it that is.
fn grant<T: Grant>(served: &Option<T>, wanted: &Option<T>) -> Option<T> {
    served.as_ref()?.grant(wanted.as_ref()?)
}

/// Grants a struct of features field by field. The struct expression names
/// every field, so a feature added to the type can't be left out here.
macro_rules! grant_fields {
    ($type:ty { $($field:ident),* $(,)? }) => {
        impl Grant for $type {
            fn grant(&self, wanted: &Self) -> Option<Self> {
                Some(Self {
                    $($field: grant(&self.$field, &wanted.$field),)*
                })
            }
        }
    };
}
grant_fields!(Features {
    text_input,
    upload,
    drag_out,
    audio
});
grant_fields!(DragOut { files });

impl Features {
    /// Everything this build can serve.
    pub fn all() -> Self {
        Self {
            text_input: Some(Present {}),
            upload: Some(Present {}),
            drag_out: Some(DragOut {
                files: Some(Present {}),
            }),
            audio: Some(Present {}),
        }
    }
    /// What `wanted` gets of what this serves.
    pub fn granted(&self, wanted: &Self) -> Self {
        Grant::grant(self, wanted).unwrap_or_default()
    }
    /// Whether these hold nothing `wanted` doesn't: a grant must not.
    pub fn within(&self, wanted: &Self) -> bool {
        wanted.granted(self) == *self
    }
    /// Whether files can be dragged out.
    pub fn drag_out_files(&self) -> bool {
        self.drag_out
            .as_ref()
            .is_some_and(|drag_out| drag_out.files.is_some())
    }
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    use prost::Message;

    fn tree(text_input: bool, upload: bool, drag_out: Option<bool>, audio: bool) -> Features {
        let present = |on: bool| on.then_some(Present {});
        Features {
            text_input: present(text_input),
            upload: present(upload),
            drag_out: drag_out.map(|files| DragOut {
                files: present(files),
            }),
            audio: present(audio),
        }
    }

    /// Every tree of four features with `drag_out.files`.
    fn every_tree() -> Vec<Features> {
        let mut trees = Vec::new();
        for bits in 0..16u8 {
            for drag_out in [None, Some(false), Some(true)] {
                trees.push(tree(
                    bits & 1 != 0,
                    bits & 2 != 0,
                    drag_out.filter(|_| bits & 4 != 0),
                    bits & 8 != 0,
                ));
            }
        }
        trees
    }

    #[test]
    fn a_grant_holds_only_what_was_wanted_and_is_served() {
        for served in every_tree() {
            for wanted in every_tree() {
                let granted = served.granted(&wanted);
                assert!(granted.within(&wanted), "{served:?} {wanted:?}");
                assert!(granted.within(&served), "{served:?} {wanted:?}");
                assert_eq!(granted, wanted.granted(&served));
                for (granted, wanted, served) in [
                    (granted.text_input, wanted.text_input, served.text_input),
                    (granted.upload, wanted.upload, served.upload),
                    (granted.audio, wanted.audio, served.audio),
                ] {
                    assert_eq!(granted.is_some(), wanted.is_some() && served.is_some());
                }
            }
        }
    }

    #[test]
    fn a_sub_feature_is_never_granted_without_its_parent() {
        let files_only = Features {
            drag_out: Some(DragOut {
                files: Some(Present {}),
            }),
            ..Features::default()
        };
        // A display that serves no dragging out grants none of its files.
        assert_eq!(Features::default().granted(&files_only).drag_out, None);
        assert_eq!(files_only.granted(&Features::default()).drag_out, None);
        // Dragging out without files grants no files.
        let no_files = tree(false, false, Some(false), false);
        assert_eq!(
            Features::all().granted(&no_files).drag_out,
            Some(DragOut::default())
        );
        assert!(!Features::all().granted(&no_files).drag_out_files());
        for served in every_tree() {
            for wanted in every_tree() {
                let granted = served.granted(&wanted);
                if granted.drag_out_files() {
                    assert!(wanted.drag_out_files() && served.drag_out_files());
                }
            }
        }
    }

    #[test]
    fn a_grant_of_more_than_was_wanted_is_not_within_it() {
        let wanted = tree(true, false, None, false);
        assert!(!tree(true, true, None, false).within(&wanted));
        assert!(!tree(true, false, Some(false), false).within(&wanted));
        assert!(!tree(false, false, Some(true), false).within(&tree(
            false,
            false,
            Some(false),
            false
        )));
        assert!(Features::default().within(&wanted));
        assert!(wanted.within(&wanted));
    }

    #[test]
    fn trees_round_trip_and_an_empty_tree_is_empty() {
        use crate::encoding::protobuf::generated::newport::remote::apps as wire;
        for features in every_tree() {
            let encoded = wire::Features::from(&features).encode_to_vec();
            let decoded = Features::from(wire::Features::decode(encoded.as_slice()).unwrap());
            assert_eq!(decoded, features);
        }
        assert!(
            wire::Features::from(&Features::default())
                .encode_to_vec()
                .is_empty()
        );
        // A present feature with no parameters is on the wire, not left out.
        let present = wire::Features::from(&tree(false, false, None, true)).encode_to_vec();
        assert_eq!(present, [0x22, 0x00]);
        assert_eq!(
            serde_json::to_value(tree(true, false, Some(false), false)).unwrap(),
            serde_json::json!({"text_input": {}, "drag_out": {}})
        );
    }

    #[test]
    fn features_a_newer_peer_adds_are_skipped_at_every_level() {
        use crate::encoding::protobuf::generated::newport::remote::apps as wire;
        let mut encoded = wire::Features::from(&Features::all()).encode_to_vec();
        // A feature numbered after every known one, with a parameter.
        prost::encoding::encode_key(99, prost::encoding::WireType::LengthDelimited, &mut encoded);
        prost::encoding::encode_varint(2, &mut encoded);
        encoded.extend([0x08, 0x01]);
        // And a sub-feature of `drag_out` this build doesn't know.
        let mut drag_out = wire::DragOut::from(&DragOut {
            files: Some(Present {}),
        })
        .encode_to_vec();
        prost::encoding::encode_key(7, prost::encoding::WireType::Varint, &mut drag_out);
        prost::encoding::encode_varint(1, &mut drag_out);
        prost::encoding::encode_key(3, prost::encoding::WireType::LengthDelimited, &mut encoded);
        prost::encoding::encode_varint(drag_out.len() as u64, &mut encoded);
        encoded.extend(drag_out);
        let decoded = Features::from(wire::Features::decode(encoded.as_slice()).unwrap());
        assert_eq!(decoded, Features::all());
    }
}
