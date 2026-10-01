use russh_sftp::protocol::*;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub struct Sftp {
    root: PathBuf,
    files: HashMap<String, fs::File>,
    dirs: HashMap<String, Vec<File>>,
    next: u64,
}
fn error(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}
fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: String::new(),
    }
}
fn attrs(m: fs::Metadata) -> FileAttributes {
    FileAttributes {
        size: Some(m.len()),
        uid: Some(m.uid()),
        gid: Some(m.gid()),
        permissions: Some(m.mode()),
        atime: Some(m.atime() as u32),
        mtime: Some(m.mtime() as u32),
        ..Default::default()
    }
}
impl Sftp {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: HashMap::new(),
            dirs: HashMap::new(),
            next: 0,
        }
    }
    fn path(&self, path: &str) -> Result<PathBuf, StatusCode> {
        let relative = Path::new(path.trim_start_matches('/'));
        if relative.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        }) {
            return Err(StatusCode::PermissionDenied);
        }
        let target = self.root.join(relative);
        let checked = if fs::symlink_metadata(&target).is_ok() {
            target.canonicalize()
        } else {
            target
                .parent()
                .ok_or(StatusCode::PermissionDenied)?
                .canonicalize()
        }
        .map_err(error)?;
        if !checked.starts_with(&self.root) {
            return Err(StatusCode::PermissionDenied);
        }
        Ok(target)
    }
    fn handle(&mut self) -> String {
        self.next += 1;
        self.next.to_string()
    }
}
impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;
    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }
    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusCode> {
        let full = self.path(&path)?.canonicalize().map_err(error)?;
        Ok(Name {
            id,
            files: vec![File::dummy(format!(
                "/{}",
                full.strip_prefix(&self.root)
                    .map_err(|_| StatusCode::PermissionDenied)?
                    .display()
            ))],
        })
    }
    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        Ok(Attrs {
            id,
            attrs: attrs(fs::metadata(self.path(&path)?).map_err(error)?),
        })
    }
    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        Ok(Attrs {
            id,
            attrs: attrs(fs::symlink_metadata(self.path(&path)?).map_err(error)?),
        })
    }
    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, StatusCode> {
        Ok(Attrs {
            id,
            attrs: attrs(
                self.files
                    .get(&handle)
                    .ok_or(StatusCode::Failure)?
                    .metadata()
                    .map_err(error)?,
            ),
        })
    }
    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, StatusCode> {
        let mut files = Vec::new();
        for item in fs::read_dir(self.path(&path)?).map_err(error)? {
            let item = item.map_err(error)?;
            files.push(File::new(
                item.file_name().to_string_lossy(),
                attrs(fs::symlink_metadata(item.path()).map_err(error)?),
            ));
        }
        let handle = self.handle();
        self.dirs.insert(handle.clone(), files);
        Ok(Handle { id, handle })
    }
    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, StatusCode> {
        let files = self.dirs.get_mut(&handle).ok_or(StatusCode::Failure)?;
        if files.is_empty() {
            return Err(StatusCode::Eof);
        }
        Ok(Name {
            id,
            files: std::mem::take(files),
        })
    }
    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        attributes: FileAttributes,
    ) -> Result<Handle, StatusCode> {
        let file = OpenOptions::new()
            .read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE))
            .append(pflags.contains(OpenFlags::APPEND))
            .create(pflags.contains(OpenFlags::CREATE))
            .create_new(pflags.contains(OpenFlags::EXCLUDE))
            .truncate(pflags.contains(OpenFlags::TRUNCATE))
            .mode(attributes.permissions.unwrap_or(0o600))
            .open(self.path(&filename)?)
            .map_err(error)?;
        let handle = self.handle();
        self.files.insert(handle.clone(), file);
        Ok(Handle { id, handle })
    }
    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, StatusCode> {
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(SeekFrom::Start(offset)).map_err(error)?;
        let mut data = vec![0; (len as usize).min(65536)];
        let n = file.read(&mut data).map_err(error)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(n);
        Ok(Data { id, data })
    }
    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, StatusCode> {
        let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
        file.seek(SeekFrom::Start(offset)).map_err(error)?;
        file.write_all(&data).map_err(error)?;
        Ok(ok(id))
    }
    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusCode> {
        if self.files.remove(&handle).is_none() && self.dirs.remove(&handle).is_none() {
            return Err(StatusCode::Failure);
        }
        Ok(ok(id))
    }
    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, StatusCode> {
        fs::remove_file(self.path(&filename)?).map_err(error)?;
        Ok(ok(id))
    }
    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, StatusCode> {
        let new = self.path(&newpath)?;
        if new.exists() {
            return Err(StatusCode::Failure);
        }
        fs::rename(self.path(&oldpath)?, new).map_err(error)?;
        Ok(ok(id))
    }
}
