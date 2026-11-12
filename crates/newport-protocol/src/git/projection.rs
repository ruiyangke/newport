//! Desktop serialization of typed results, including requested diff line rows.
use super::*;
use serde::{
    Serialize, Serializer,
    ser::{SerializeSeq, SerializeStruct, SerializeTuple},
};

pub struct Projection<'a> {
    reply: &'a Reply,
    tuples: bool,
}
impl Reply {
    pub fn projection(&self, request: &Request) -> Projection<'_> {
        let tuples = matches!(
            request,
            Request::DiffPage {
                line_encoding: Some(_),
                ..
            } | Request::CommitDiffPage {
                line_encoding: Some(_),
                ..
            }
        );
        Projection {
            reply: self,
            tuples,
        }
    }
}
impl Serialize for Projection<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.reply {
            Reply::DiffPage(page) | Reply::CommitDiffPage(page) if self.tuples => {
                let mut state = serializer.serialize_struct("Page", 4)?;
                state.serialize_field("snapshot", &page.snapshot)?;
                state.serialize_field(
                    "entries",
                    &Files {
                        values: &page.entries,
                        working: !page.metadata.read_only,
                    },
                )?;
                state.serialize_field("nextCursor", &page.next_cursor)?;
                state.serialize_field("metadata", &page.metadata)?;
                state.end()
            }
            _ => self.reply.serialize(serializer),
        }
    }
}
struct Files<'a> {
    values: &'a [FileFragment],
    working: bool,
}
impl Serialize for Files<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.values.len()))?;
        for value in self.values {
            seq.serialize_element(&FileFragmentProjection {
                value,
                working: self.working,
            })?;
        }
        seq.end()
    }
}
struct FileFragmentProjection<'a> {
    value: &'a FileFragment,
    working: bool,
}
impl Serialize for FileFragmentProjection<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("FileFragment", 13)?;
        state.serialize_field("oldPath", &self.value.old_path)?;
        state.serialize_field("newPath", &self.value.new_path)?;
        state.serialize_field("oldOid", &self.value.old_oid)?;
        state.serialize_field("newOid", &self.value.new_oid)?;
        state.serialize_field("oldMode", &self.value.old_mode)?;
        state.serialize_field("newMode", &self.value.new_mode)?;
        state.serialize_field("status", &self.value.status)?;
        state.serialize_field("binary", &self.value.binary)?;
        state.serialize_field("additions", &self.value.additions)?;
        state.serialize_field("deletions", &self.value.deletions)?;
        state.serialize_field("fileIndex", &self.value.file_index)?;
        state.serialize_field("omissionReason", &self.value.omission_reason)?;
        state.serialize_field(
            "hunks",
            &Hunks {
                values: &self.value.hunks,
                working: self.working,
            },
        )?;
        state.end()
    }
}
struct Hunks<'a> {
    values: &'a [HunkFragment],
    working: bool,
}
impl Serialize for Hunks<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.values.len()))?;
        for value in self.values {
            seq.serialize_element(&HunkFragmentProjection {
                value,
                working: self.working,
            })?;
        }
        seq.end()
    }
}
struct HunkFragmentProjection<'a> {
    value: &'a HunkFragment,
    working: bool,
}
impl Serialize for HunkFragmentProjection<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("HunkFragment", 8)?;
        state.serialize_field("id", &self.value.id)?;
        state.serialize_field("index", &self.value.index)?;
        state.serialize_field("oldStart", &self.value.old_start)?;
        state.serialize_field("oldLines", &self.value.old_lines)?;
        state.serialize_field("newStart", &self.value.new_start)?;
        state.serialize_field("newLines", &self.value.new_lines)?;
        state.serialize_field("totalLines", &self.value.total_lines)?;
        state.serialize_field(
            "lines",
            &Lines {
                values: &self.value.lines,
                working: self.working,
            },
        )?;
        state.end()
    }
}
struct Lines<'a> {
    values: &'a [LineFragment],
    working: bool,
}
impl Serialize for Lines<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.values.len()))?;
        for value in self.values {
            seq.serialize_element(&Line {
                value,
                working: self.working,
            })?;
        }
        seq.end()
    }
}
struct Line<'a> {
    value: &'a LineFragment,
    working: bool,
}
impl Serialize for Line<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_tuple(if self.working { 8 } else { 7 })?;
        seq.serialize_element(&self.value.line_index)?;
        seq.serialize_element(&self.value.byte_offset)?;
        seq.serialize_element(&self.value.line_complete)?;
        seq.serialize_element(&self.value.origin)?;
        seq.serialize_element(&self.value.old_line)?;
        seq.serialize_element(&self.value.new_line)?;
        seq.serialize_element(&self.value.content_bytes_b64)?;
        if self.working {
            seq.serialize_element(&self.value.id)?;
        }
        seq.end()
    }
}
