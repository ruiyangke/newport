//! Frame tiles: one entry per changed tile of a surface.
//! Tile `id` numbers [`crate::remote_apps::TILE`]-pixel tiles row by row. A keyframe sends
//! every tile once, complete, with no `rect` or `copy`.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub id: u32,
    pub codec: Codec,
    /// The patch `[x, y, width, height]` within the tile the data covers;
    /// the whole tile when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<[u16; 4]>,
    /// Rows copied from the previous frame before the data is applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<TileCopy>,
    /// The AV1 group header, on the first tile of the group only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoGroup>,
    /// Encoded as `codec` says; empty for a copy, and for the tiles of an
    /// AV1 group but its first.
    pub data: bytes::Bytes,
}
impl Tile {
    pub fn new(id: usize, codec: Codec, data: impl Into<bytes::Bytes>) -> Self {
        Self {
            id: id as u32,
            codec,
            data: data.into(),
            rect: None,
            copy: None,
            video: None,
        }
    }
    /// The bytes it takes in a frame.
    #[cfg(feature = "protobuf")]
    pub fn wire_bytes(&self) -> usize {
        prost::Message::encoded_len(
            &crate::encoding::protobuf::generated::newport::remote::apps::Tile::from(self),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    /// One BGRA pixel repeated over the patch.
    Solid,
    RawBgra,
    /// An LZ4 block of BGRA pixels.
    Lz4,
    Zlib,
    Png,
    Jpeg,
    /// No data: the tile is only its `copy`.
    Copy,
    /// A tile of an AV1 group; the group's packet is on its first tile.
    Av1,
}
impl Codec {
    pub const ALL: [Self; 8] = [
        Self::Solid,
        Self::RawBgra,
        Self::Lz4,
        Self::Zlib,
        Self::Png,
        Self::Jpeg,
        Self::Copy,
        Self::Av1,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Solid => "solid",
            Self::RawBgra => "raw-bgra",
            Self::Lz4 => "lz4",
            Self::Zlib => "zlib",
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Copy => "copy",
            Self::Av1 => "av1",
        }
    }
}

/// `h` rows from `source` land at row `dy` of the tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CopyFields", into = "CopyFields")]
pub struct TileCopy {
    pub source: CopySource,
    pub dy: u16,
    pub h: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopySource {
    /// Surface row `source_y` of the previous frame, in this tile's column.
    Surface { source_y: u32 },
    /// Row `sy` of this tile in the previous frame.
    Tile { sy: u16 },
}

#[derive(Serialize, Deserialize)]
struct CopyFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_y: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sy: Option<u16>,
    dy: u16,
    h: u16,
}
impl TryFrom<CopyFields> for TileCopy {
    type Error = &'static str;
    fn try_from(fields: CopyFields) -> Result<Self, Self::Error> {
        let source = match (fields.source_y, fields.sy) {
            (Some(source_y), None) => CopySource::Surface { source_y },
            (None, Some(sy)) => CopySource::Tile { sy },
            (Some(_), Some(_)) => return Err("Ambiguous copy source"),
            (None, None) => return Err("Missing copy source"),
        };
        Ok(Self {
            source,
            dy: fields.dy,
            h: fields.h,
        })
    }
}
impl From<TileCopy> for CopyFields {
    fn from(copy: TileCopy) -> Self {
        let (source_y, sy) = match copy.source {
            CopySource::Surface { source_y } => (Some(source_y), None),
            CopySource::Tile { sy } => (None, Some(sy)),
        };
        Self {
            source_y,
            sy,
            dy: copy.dy,
            h: copy.h,
        }
    }
}

/// One AV1 temporal stream over the tile-aligned surface rectangle
/// `[x, y, width, height]`. `sequence` counts from 1 at the stream's keyframe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoGroup {
    pub rect: [u32; 4],
    pub stream: u64,
    pub sequence: u64,
    pub keyframe: bool,
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tiles_name_their_fields_and_carry_data_as_bytes() {
        let mut tile = Tile::new(3, Codec::RawBgra, vec![1, 2, 3, 4]);
        tile.rect = Some([1, 2, 3, 4]);
        tile.copy = Some(TileCopy {
            source: CopySource::Surface { source_y: 300 },
            dy: 0,
            h: 256,
        });
        assert_eq!(
            serde_json::to_value(&tile).unwrap(),
            json!({"id":3,"codec":"raw-bgra","rect":[1,2,3,4],
                "copy":{"source_y":300,"dy":0,"h":256},"data":[1,2,3,4]})
        );
        // The data is a `bytes` field: its tag, its length, the bytes.
        let wire = crate::encoding::protobuf::generated::newport::remote::apps::Tile::from(&tile);
        let encoded = prost::Message::encode_to_vec(&wire);
        assert!(encoded.windows(6).any(|w| w == [0x1a, 4, 1, 2, 3, 4]));
        assert_eq!(Tile::try_from(wire).unwrap(), tile);
        let video = Tile {
            video: Some(VideoGroup {
                rect: [0, 256, 512, 256],
                stream: 2,
                sequence: 1,
                keyframe: true,
            }),
            ..Tile::new(0, Codec::Av1, vec![9])
        };
        assert_eq!(
            serde_json::to_value(&video).unwrap(),
            json!({"id":0,"codec":"av1","data":[9],
                "video":{"rect":[0,256,512,256],"stream":2,"sequence":1,"keyframe":true}})
        );
        for codec in Codec::ALL {
            assert_eq!(serde_json::to_value(codec).unwrap(), codec.as_str());
        }
    }

    #[test]
    fn copies_name_exactly_one_source() {
        let copy: TileCopy = serde_json::from_value(json!({"sy":4,"dy":1,"h":2})).unwrap();
        assert_eq!(copy.source, CopySource::Tile { sy: 4 });
        for bad in [
            json!({"source_y":1,"sy":4,"dy":1,"h":2}),
            json!({"dy":1,"h":2}),
            json!({"sy":-1,"dy":1,"h":2}),
            json!({"sy":70000,"dy":1,"h":2}),
        ] {
            assert!(serde_json::from_value::<TileCopy>(bad).is_err());
        }
        assert!(serde_json::from_value::<Tile>(json!({"id":0,"codec":"gif","data":[]})).is_err());
        assert!(serde_json::from_value::<Tile>(json!({"id":0,"codec":"solid"})).is_err());
    }
}
