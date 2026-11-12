//! Optional stop-and-wait incoming file upload. Completion identifies a staged
//! batch, never a caller-supplied remote path. Drag/drop references that batch.
pub use crate::remote_apps::CHUNK_BYTES;
use serde::{Deserialize, Serialize};
pub const MAX_FILES: usize = 32;
pub const MAX_BATCH_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_SESSION_BYTES: u64 = 512 * 1024 * 1024;

/// Bounds for one kind of file transfer. Drops move what the user dragged; a
/// clipboard paste moves a whole copy and has its own, larger bounds and quota.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_entries: usize,
    /// Top-level entries of one batch.
    pub max_files: usize,
    pub max_batch_bytes: u64,
    /// Bytes reserved across all batches of one staging area.
    pub max_session_bytes: u64,
}
impl Limits {
    pub const DROP: Self = Self {
        max_entries: crate::remote_apps::file_tree::MAX_ENTRIES,
        max_files: MAX_FILES,
        max_batch_bytes: MAX_BATCH_BYTES,
        max_session_bytes: MAX_SESSION_BYTES,
    };
    /// Entries are limited to what one `BeginTree` manifest can describe.
    pub const CLIPBOARD: Self = Self {
        max_entries: crate::remote_apps::file_tree::MANIFEST_ENTRIES,
        max_files: crate::remote_apps::file_tree::MANIFEST_ENTRIES,
        max_batch_bytes: 2 * 1024 * 1024 * 1024,
        max_session_bytes: 4 * 1024 * 1024 * 1024,
    };
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSpec {
    pub name: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Client {
    Begin {
        transfer: u64,
        files: Vec<FileSpec>,
    },
    /// Files and folders, as a tree.
    BeginTree {
        transfer: u64,
        entries: Vec<crate::remote_apps::file_tree::Entry>,
    },
    Chunk {
        transfer: u64,
        file: u16,
        offset: u64,
        data: bytes::Bytes,
    },
    Finish {
        transfer: u64,
    },
    /// Cancel pending or completed-but-unexposed staging. Once URI bytes have
    /// been offered to an application, files remain readable until session end.
    Cancel {
        transfer: u64,
    },
}
impl Client {
    pub fn transfer(&self) -> u64 {
        match self {
            Self::Begin { transfer, .. }
            | Self::BeginTree { transfer, .. }
            | Self::Chunk { transfer, .. }
            | Self::Finish { transfer }
            | Self::Cancel { transfer } => *transfer,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with(&Limits::DROP)
    }
    pub fn validate_with(&self, limits: &Limits) -> Result<(), String> {
        if self.transfer() == 0 {
            return Err("Invalid upload identity".into());
        }
        match self {
            Self::Begin { files, .. } => validate_files_with(files, limits),
            Self::BeginTree { entries, .. } => {
                crate::remote_apps::file_tree::validate_with(entries, limits)
            }
            Self::Chunk {
                file, offset, data, ..
            } if usize::from(*file) >= limits.max_entries
                || data.is_empty()
                || data.len() > CHUNK_BYTES
                || offset
                    .checked_add(data.len() as u64)
                    .is_none_or(|n| n > limits.max_batch_bytes) =>
            {
                Err("Invalid upload chunk".into())
            }
            _ => Ok(()),
        }
    }
}
pub fn validate_files(files: &[FileSpec]) -> Result<(), String> {
    validate_files_with(files, &Limits::DROP)
}
pub fn validate_files_with(files: &[FileSpec], limits: &Limits) -> Result<(), String> {
    if files.is_empty() || files.len() > limits.max_files {
        return Err("Invalid file count".into());
    }
    let mut names = std::collections::BTreeSet::new();
    let mut total = 0u64;
    for file in files {
        if file.name.is_empty()
            || file.name.len() > 255
            || matches!(file.name.as_str(), "." | "..")
            || file
                .name
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
            || !names.insert(&file.name)
        {
            return Err("Invalid or duplicate file name".into());
        }
        total = total.checked_add(file.bytes).ok_or("File size overflow")?;
    }
    if total > limits.max_batch_bytes {
        return Err("File upload exceeds batch limit".into());
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Server {
    /// Only this file and offset can be sent next. `file == count` means Finish.
    Ack {
        transfer: u64,
        file: u16,
        offset: u64,
    },
    Ready {
        transfer: u64,
    },
    Cancelled {
        transfer: u64,
    },
    Rejected {
        transfer: u64,
        reason: String,
    },
}
impl Server {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with(&Limits::DROP)
    }
    pub fn validate_with(&self, limits: &Limits) -> Result<(), String> {
        let transfer = match self {
            Self::Ack { transfer, .. }
            | Self::Ready { transfer }
            | Self::Cancelled { transfer }
            | Self::Rejected { transfer, .. } => *transfer,
        };
        if transfer == 0 {
            return Err("Invalid upload response identity".into());
        }
        if let Self::Ack { file, offset, .. } = self
            && (usize::from(*file) > limits.max_entries || *offset > limits.max_batch_bytes)
        {
            return Err("Invalid upload credit".into());
        }
        if let Self::Rejected { reason, .. } = self
            && reason.len() > 256
        {
            return Err("Invalid upload rejection".into());
        }
        Ok(())
    }
}
#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    #[test]
    fn largest_chunk_fits_bounded_client_line_and_fields_are_strict() {
        let chunk = Client::Chunk {
            transfer: u64::MAX,
            file: 31,
            offset: 0,
            data: vec![255; CHUNK_BYTES].into(),
        };
        chunk.validate().unwrap();
        assert!(serde_json::to_vec(&chunk).unwrap().len() < 32 * 1024);
        assert!(
            serde_json::from_str::<Client>(r#"{"kind":"finish","transfer":1,"path":"/tmp/x"}"#)
                .is_err()
        );
        assert!(
            Client::Chunk {
                transfer: 1,
                file: 0,
                offset: u64::MAX,
                data: vec![1].into()
            }
            .validate()
            .is_err()
        );
    }
    fn spec(name: &str, bytes: u64) -> FileSpec {
        FileSpec {
            name: name.into(),
            bytes,
        }
    }
    #[test]
    fn drop_limits_reject_what_they_always_rejected() {
        let many: Vec<_> = (0..=MAX_FILES).map(|i| spec(&i.to_string(), 0)).collect();
        assert!(validate_files(&many).is_err());
        assert!(validate_files(&[spec("big", MAX_BATCH_BYTES + 1)]).is_err());
        assert!(validate_files(&[spec("a", MAX_BATCH_BYTES), spec("b", 1)]).is_err());
        let chunk = |file, offset| Client::Chunk {
            transfer: 1,
            file,
            offset,
            data: vec![1].into(),
        };
        assert!(chunk(256, 0).validate().is_err());
        assert!(chunk(0, MAX_BATCH_BYTES).validate().is_err());
        chunk(255, MAX_BATCH_BYTES - 1).validate().unwrap();
        let ack = |file, offset| Server::Ack {
            transfer: 1,
            file,
            offset,
        };
        assert!(ack(257, 0).validate().is_err());
        assert!(ack(0, MAX_BATCH_BYTES + 1).validate().is_err());
        ack(256, MAX_BATCH_BYTES).validate().unwrap();
    }
    #[test]
    fn clipboard_limits_accept_batches_beyond_the_drop_limits() {
        let limits = Limits::CLIPBOARD;
        let many: Vec<_> = (0..MAX_FILES * 2)
            .map(|i| spec(&i.to_string(), MAX_BATCH_BYTES / 8))
            .collect();
        assert!(validate_files(&many).is_err());
        validate_files_with(&many, &limits).unwrap();
        assert!(validate_files_with(&[spec("big", limits.max_batch_bytes + 1)], &limits).is_err());
        let begin = Client::Begin {
            transfer: 1,
            files: many,
        };
        assert!(begin.validate().is_err());
        begin.validate_with(&limits).unwrap();
        let chunk = Client::Chunk {
            transfer: 1,
            file: 256,
            offset: MAX_BATCH_BYTES,
            data: vec![1].into(),
        };
        assert!(chunk.validate().is_err());
        chunk.validate_with(&limits).unwrap();
        let ack = Server::Ack {
            transfer: 1,
            file: 257,
            offset: MAX_BATCH_BYTES + 1,
        };
        assert!(ack.validate().is_err());
        ack.validate_with(&limits).unwrap();
        assert!(limits.max_batch_bytes <= limits.max_session_bytes);
    }
}
