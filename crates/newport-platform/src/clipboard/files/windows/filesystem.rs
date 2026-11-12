//! Handle-relative Windows source traversal. Reparse objects are never read.
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptionsExt as CapOpenOptionsExt;
use cap_std::fs::{Dir, OpenOptions};
use std::{
    fs::File,
    mem::{offset_of, size_of},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::Path,
};
use windows::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, GENERIC_READ, HANDLE},
    Storage::FileSystem::*,
};

pub(crate) fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    volume: u64,
    id: [u8; 16],
    created: i64,
    pub(super) written: i64,
    changed: i64,
    attributes: u32,
    pub(crate) bytes: u64,
    pub(crate) directory: bool,
    pub(crate) links: u32,
}

impl Identity {
    pub(super) fn object_key(&self) -> [u8; 32] {
        let mut key = [0; 32];
        key[..8].copy_from_slice(&self.volume.to_le_bytes());
        key[8..24].copy_from_slice(&self.id);
        key[24..].copy_from_slice(&self.created.to_le_bytes());
        key
    }
}

fn information<T: Default>(file: &File, class: FILE_INFO_BY_HANDLE_CLASS) -> Result<T, String> {
    let mut info = T::default();
    // SAFETY: the handle is open, and `info` is writable for its own size. Callers
    // pair `T` with the class that fills it.
    unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            class,
            (&mut info as *mut T).cast(),
            size_of::<T>() as u32,
        )
    }
    .map_err(|e| format!("Query file identity: {e}"))?;
    Ok(info)
}

pub(crate) fn identity(file: &File) -> Result<Identity, String> {
    // SAFETY: a plain query on an open handle.
    if unsafe { GetFileType(handle(file)) } != FILE_TYPE_DISK {
        return Err("Only regular disk files and directories can be transferred".into());
    }
    let basic: FILE_BASIC_INFO = information(file, FileBasicInfo)?;
    if basic.FileAttributes
        & (FILE_ATTRIBUTE_REPARSE_POINT.0
            | FILE_ATTRIBUTE_OFFLINE.0
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0
            | FILE_ATTRIBUTE_RECALL_ON_OPEN.0)
        != 0
    {
        return Err("Reparse points and cloud-placeholder files cannot be transferred".into());
    }
    let standard: FILE_STANDARD_INFO = information(file, FileStandardInfo)?;
    let id: FILE_ID_INFO = information(file, FileIdInfo)?;
    if standard.DeletePending || standard.EndOfFile < 0 {
        return Err("File is being removed or has an invalid size".into());
    }
    Ok(Identity {
        volume: id.VolumeSerialNumber,
        id: id.FileId.Identifier,
        created: basic.CreationTime,
        written: basic.LastWriteTime,
        changed: basic.ChangeTime,
        attributes: basic.FileAttributes,
        bytes: standard.EndOfFile as u64,
        directory: standard.Directory,
        links: standard.NumberOfLinks,
    })
}

pub(crate) fn open_child(parent: &File, name: &str) -> Result<File, String> {
    super::super::windows_policy::component(name)?;
    let directory = Dir::from_std_file(parent.try_clone().map_err(|e| e.to_string())?);
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    // This API accepts one validated component. Native handle-relative lookup
    // survives renames; cap-std's multi-component directory pinning is not needed.
    // `maybe_dir` would remove share-delete even from ordinary lease files.
    options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_NO_RECALL.0);
    options.share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0);
    let file = directory
        .open_with(name, &options)
        .map_err(|e| format!("Open dropped entry: {e}"))?
        .into_std();
    identity(&file)?;
    Ok(file)
}

pub(crate) fn open_source(path: &Path) -> Result<File, String> {
    let text = path.to_str().ok_or("File path must be valid Unicode")?;
    let (drive, components) = super::super::windows_policy::drive_path(text)?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(
            FILE_FLAG_BACKUP_SEMANTICS.0
                | FILE_FLAG_OPEN_REPARSE_POINT.0
                | FILE_FLAG_OPEN_NO_RECALL.0,
        )
        .open(format!("\\\\?\\{drive}:\\"))
        .map_err(|e| format!("Open source volume: {e}"))?;
    identity(&file)?;
    for component in components {
        if !identity(&file)?.directory {
            return Err("Source ancestor is not a directory".into());
        }
        file = open_child(&file, component)?;
    }
    Ok(file)
}

pub(crate) fn stable_reader(file: &File) -> Result<File, String> {
    // Windows may defer write timestamps until a writer closes. Deny existing
    // and new write handles while copying instead of relying on time alone.
    // SAFETY: the source handle is open; the result is a new handle.
    let reopened = unsafe {
        ReOpenFile(
            handle(file),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_DELETE,
            // Reopen the validated object with default synchronous I/O; no
            // pathname is resolved and the new handle is checked again below.
            FILE_FLAGS_AND_ATTRIBUTES(0),
        )
    }
    .map_err(|e| format!("Source file is busy or cannot be snapshotted: {e}"))?;
    // SAFETY: ReOpenFile succeeded, so the handle is new and owned.
    let reader = unsafe { File::from_raw_handle(reopened.0) };
    if identity(file)? != identity(&reader)? {
        return Err("Dropped file changed while acquiring its snapshot handle".into());
    }
    Ok(reader)
}

pub(super) fn writable(file: &File) -> Result<File, String> {
    // SAFETY: the source handle is open; the result is a new handle.
    let reopened = unsafe {
        ReOpenFile(
            handle(file),
            GENERIC_READ.0 | windows::Win32::Foundation::GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAGS_AND_ATTRIBUTES(0),
        )
    }
    .map_err(|e| e.to_string())?;
    // SAFETY: ReOpenFile succeeded, so the handle is new and owned.
    let writable = unsafe { File::from_raw_handle(reopened.0) };
    if identity(file)? != identity(&writable)? {
        return Err("Clipboard lease changed while acquiring its writable handle".into());
    }
    Ok(writable)
}

/// Hold a path component in place. A retained payload also excludes writers.
pub(super) fn pin(file: &File, payload: bool) -> Result<File, String> {
    let share = if payload {
        FILE_SHARE_READ
    } else {
        FILE_SHARE_READ | FILE_SHARE_WRITE
    };
    retain(file, GENERIC_READ.0, share)
}

/// Pin every ancestor so a path-based native API cannot be redirected after
/// validation. Each following component is opened relative to its held parent.
pub(super) fn pin_path(path: &Path) -> Result<Vec<File>, String> {
    let text = path.to_str().ok_or("File path must be valid Unicode")?;
    let (drive, parts) = super::super::windows_policy::drive_path(text)?;
    let root = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(format!("\\\\?\\{drive}:\\"))
        .map_err(|e| e.to_string())?;
    let mut held = vec![pin(&root, false)?];
    for component in parts {
        let next = open_child(held.last().unwrap(), component)?;
        held.push(pin(&next, false)?);
    }
    Ok(held)
}

/// Enumerate the held directory, without reconstructing its path after rename.
pub(crate) fn children(file: &File, available: usize) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut buffer = vec![0u64; 8192];
    let capacity = buffer.len() * size_of::<u64>();
    let mut class = FileIdBothDirectoryRestartInfo;
    loop {
        // SAFETY: the handle is open, and `buffer` is 8-byte aligned and `capacity` bytes.
        let result = unsafe {
            GetFileInformationByHandleEx(
                handle(file),
                class,
                buffer.as_mut_ptr().cast(),
                capacity as u32,
            )
        };
        if let Err(error) = result {
            if error.code() == windows::core::HRESULT::from_win32(ERROR_NO_MORE_FILES.0) {
                break;
            }
            return Err(format!("Enumerate dropped directory: {error}"));
        }
        class = FileIdBothDirectoryInfo;
        let mut offset = 0usize;
        loop {
            let name_offset = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
            if offset + size_of::<FILE_ID_BOTH_DIR_INFO>() > capacity {
                return Err("Invalid directory entry".into());
            }
            // SAFETY: the offset keeps a whole entry inside the buffer (checked above).
            // The system aligns entries, and `next` is checked to keep them aligned.
            let entry = unsafe {
                &*buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFO>()
            };
            let bytes = entry.FileNameLength as usize;
            let next = entry.NextEntryOffset as usize;
            if !bytes.is_multiple_of(2) || bytes > capacity - offset - name_offset {
                return Err("Invalid directory entry name".into());
            }
            // SAFETY: the name lies inside the buffer (`bytes` checked above) and
            // starts at an even offset from the aligned buffer.
            let wide = unsafe {
                std::slice::from_raw_parts(
                    buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset + name_offset)
                        .cast::<u16>(),
                    bytes / 2,
                )
            };
            let name = String::from_utf16(wide).map_err(|_| "File name must be valid Unicode")?;
            if name != "." && name != ".." {
                if names.len() >= available {
                    return Err("Tree entry count exceeded".into());
                }
                super::super::windows_policy::component(&name)?;
                if entry.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
                    return Err("Reparse points cannot be transferred".into());
                }
                names.push(name);
            }
            if next == 0 {
                break;
            }
            if next < name_offset + bytes || !next.is_multiple_of(8) || next > capacity - offset {
                return Err("Invalid directory entry offset".into());
            }
            offset += next;
        }
    }
    names.sort_unstable();
    Ok(names)
}

/// Reacquire rights from the held object, never from its mutable pathname.
fn retain(file: &File, access: u32, share: FILE_SHARE_MODE) -> Result<File, String> {
    let before = identity(file)?;
    let held = if before.directory {
        let directory = Dir::from_std_file(file.try_clone().map_err(|e| e.to_string())?);
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_NO_RECALL.0);
        options.access_mode(access);
        options.share_mode(share.0);
        directory
            .open_with(".", &options)
            .map_err(|e| format!("Retain held directory: {e}"))?
            .into_std()
    } else {
        // SAFETY: the source handle is open; the result is a new handle.
        let handle =
            unsafe { ReOpenFile(handle(file), access, share, FILE_FLAGS_AND_ATTRIBUTES(0)) }
                .map_err(|e| format!("Retain held file: {e}"))?;
        // SAFETY: ReOpenFile succeeded, so the handle is new and owned.
        unsafe { File::from_raw_handle(handle.0) }
    };
    if identity(&held)? != before {
        return Err("Object changed while acquiring its retained handle".into());
    }
    Ok(held)
}

/// Acquire deletion authority for this object, excluding future renames. No
/// pathname lookup occurs between identity verification and cleanup.
pub(super) fn retirement_handle(file: &File) -> Result<File, String> {
    retain(
        file,
        GENERIC_READ.0 | DELETE.0,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    )
}

/// Rename the held object under the held namespace without replacing a target.
pub(super) fn rename_held(file: &File, parent: &File, name: &str) -> Result<(), String> {
    super::super::windows_policy::component(name)?;
    use windows::{
        Wdk::Storage::FileSystem::{
            FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
        },
        Win32::{Foundation::RtlNtStatusToDosError, System::IO::IO_STATUS_BLOCK},
    };
    let wide: Vec<_> = name.encode_utf16().collect();
    let bytes = size_of::<FILE_RENAME_INFORMATION>() + wide.len() * 2;
    let mut buffer = vec![0u64; bytes.div_ceil(8)];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: `buffer` is 8-byte aligned and holds the structure plus the whole
    // name, which is copied in bounds; both handles are open.
    let result = unsafe {
        info.write(FILE_RENAME_INFORMATION::default());
        (*info).RootDirectory = handle(parent);
        (*info).FileNameLength = (wide.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), (*info).FileName.as_mut_ptr(), wide.len());
        // Our handles use synchronous IO; the native request retains both source
        // and destination identity without translating a mutable DOS path.
        NtSetInformationFile(
            handle(file),
            &mut status,
            info.cast(),
            bytes as u32,
            FileRenameInformation,
        )
    };
    if result.0 != 0 {
        let error =
            // SAFETY: a pure status-code translation.
            std::io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(result) } as i32);
        return Err(format!(
            "Retire cleanup target: {error} (NTSTATUS {:#x})",
            result.0
        ));
    }
    Ok(())
}

fn mark_deleted(file: &File) -> Result<(), String> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: the handle is open, and `info` is the structure the class names.
    unsafe {
        SetFileInformationByHandle(
            handle(file),
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    }
    .map_err(|e| format!("Remove held cache object: {e}"))
}

/// Remove only objects opened beneath the held directory. Reparse points and
/// unowned contents stop cleanup; bounded enumeration never follows them.
pub(super) fn remove_held(file: File, limit: usize) -> Result<(), String> {
    let mut pending = vec![(file, false)];
    let mut remaining = limit;
    while let Some((file, visited)) = pending.pop() {
        let meta = identity(&file)?;
        super::security::verify_private(&file, false).map_err(|e| e.to_string())?;
        if visited || !meta.directory {
            mark_deleted(&file)?;
            continue;
        }
        let names = children(&file, remaining)?;
        remaining -= names.len();
        let mut children = Vec::with_capacity(names.len());
        for name in names {
            let child = open_child(&file, &name)?;
            children.push((retirement_handle(&child)?, false));
        }
        pending.push((file, true));
        pending.extend(children);
    }
    Ok(())
}

pub(super) fn remove_file_held(file: &File) -> Result<(), String> {
    let held = retirement_handle(file)?;
    if identity(&held)?.directory {
        return Err("Expected a regular lease file".into());
    }
    mark_deleted(&held)
}
