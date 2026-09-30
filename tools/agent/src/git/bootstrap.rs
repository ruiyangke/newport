//! Repository creation before a repository handle exists. Publish prepared
//! metadata with a no-replace directory rename; never reinitialize a project.
use super::{
    branches,
    journal::{self, Journal},
    protocol::{Error, Path as WirePath},
};
use git2::Repository;
use serde_json::{json, Value};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
};
fn io_error(_: io::Error) -> Error {
    Error::new("IO_ERROR", "Cannot prepare the repository directory.")
}
fn unknown() -> Error {
    Error::new("OUTCOME_UNKNOWN","Repository metadata may have been published. Query the operation and inspect the directory before retrying.")
}
fn engine(_: git2::Error) -> Error {
    Error::new("GIT_ERROR", "Cannot initialize the requested repository.")
}
fn plain_directory(root: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(root.join(".git")) {
        Ok(_) => {
            return Err(Error::new(
                "ALREADY_REPOSITORY",
                "This directory already contains Git metadata; it will not be replaced.",
            ))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(e)),
    }
    match Repository::discover(root) {
        Ok(_) => Err(Error::new(
            "ALREADY_REPOSITORY",
            "The selected directory is already inside a repository.",
        )),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
        Err(e) => Err(engine(e)),
    }
}
pub(super) fn root(path: &WirePath) -> Result<(PathBuf, File, String), Error> {
    let bytes = path.decode()?;
    let normalized: PathBuf = Path::new(OsStr::from_bytes(&bytes)).components().collect();
    let path = normalized.as_path();
    if !path.is_absolute() {
        return Err(Error::invalid("Choose an absolute directory path."));
    }
    let meta = fs::symlink_metadata(path)
        .map_err(|_| Error::new("DIRECTORY_REQUIRED", "Choose an existing directory."))?;
    if !meta.is_dir() {
        return Err(Error::new(
            "DIRECTORY_REQUIRED",
            "Choose a directory, not a file or symbolic link.",
        ));
    }
    let canonical = path.canonicalize().map_err(io_error)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&canonical)
        .map_err(io_error)?;
    let actual = file.metadata().map_err(io_error)?;
    if actual.dev() != meta.dev() || actual.ino() != meta.ino() {
        return Err(Error::new(
            "PATH_CHANGED",
            "The selected directory changed.",
        ));
    }
    let identity = journal::hash(
        &[
            b"bootstrap:".as_slice(),
            canonical.as_os_str().as_bytes(),
            &actual.dev().to_be_bytes(),
            &actual.ino().to_be_bytes(),
        ]
        .concat(),
    );
    Ok((canonical, file, identity))
}
/// Both paths are relative to a held directory descriptor. The destination
/// must remain absent even if another process creates it after preflight.
fn publish(directory: &File, source: &Path) -> Result<(), Error> {
    publish_to(directory, source, Path::new(".git"))
}
pub(super) fn publish_to(directory: &File, source: &Path, destination: &Path) -> Result<(), Error> {
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| Error::invalid("Invalid metadata path."))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| Error::invalid("Invalid destination path."))?;
    // SAFETY: live directory descriptors and NUL-terminated paths; libc retains
    // neither pointer. NOREPLACE/EXCL guarantees no existing entry is replaced.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = -1;
    if result != 0 {
        let error = io::Error::last_os_error();
        return Err(if error.kind() == io::ErrorKind::AlreadyExists {
            Error::new(
                "ALREADY_REPOSITORY",
                "Git metadata appeared during initialization; it was not replaced.",
            )
        } else {
            io_error(error)
        });
    }
    directory.sync_all().map_err(|_| unknown())
}
fn sync_metadata(path: &Path, count: &mut usize) -> Result<(), Error> {
    *count += 1;
    if *count > 256 {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "Prepared repository metadata exceeds its entry limit.",
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.is_dir() {
        for item in fs::read_dir(path).map_err(io_error)? {
            sync_metadata(&item.map_err(io_error)?.path(), count)?;
        }
    } else if !metadata.is_file() {
        return Err(Error::new(
            "UNSUPPORTED_CAPABILITY",
            "Unexpected prepared repository metadata.",
        ));
    }
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(io_error)
}
fn prepare(root: &Path, directory: &File, branch: &str) -> Result<Value, Error> {
    plain_directory(root)?;
    let temporary = tempfile::Builder::new()
        .prefix(".newport-git-init-")
        .tempdir_in(root)
        .map_err(io_error)?;
    let mut options = git2::RepositoryInitOptions::new();
    options
        .no_reinit(true)
        .external_template(false)
        .initial_head(&format!("refs/heads/{branch}"));
    let prepared = Repository::init_opts(temporary.path(), &options).map_err(engine)?;
    if prepared
        .config()
        .map_err(engine)?
        .get_string("core.worktree")
        .is_ok()
    {
        return Err(Error::new(
            "UNSUPPORTED_CONFIGURATION",
            "Initialization produced an unexpected worktree override.",
        ));
    }
    drop(prepared);
    sync_metadata(&temporary.path().join(".git"), &mut 0)?;
    let before = directory.metadata().map_err(io_error)?;
    let now = fs::symlink_metadata(root).map_err(io_error)?;
    if before.dev() != now.dev() || before.ino() != now.ino() {
        return Err(Error::new(
            "PATH_CHANGED",
            "The selected directory moved during initialization.",
        ));
    }
    plain_directory(root)?;
    let source = Path::new(
        temporary
            .path()
            .file_name()
            .ok_or_else(|| Error::invalid("Invalid temporary directory."))?,
    )
    .join(".git");
    publish(directory, &source)?;
    let repo = Repository::open(root).map_err(|_| unknown())?;
    if repo.is_bare()
        || repo
            .find_reference("HEAD")
            .map_err(|_| unknown())?
            .symbolic_target()
            .map_err(|_| unknown())?
            != Some(&format!("refs/heads/{branch}"))
    {
        return Err(unknown());
    }
    Ok(
        json!({"initialized":true,"path":WirePath::new(root.as_os_str().as_bytes()),"initialBranch":branch,"bare":false,"openRequired":true}),
    )
}
pub fn init(
    journal: &Journal,
    operation_id: &str,
    path: &WirePath,
    initial_branch: &str,
) -> Result<Value, Error> {
    branches::name(initial_branch)?;
    let hash = journal::hash(
        &serde_json::to_vec(
            &json!({"method":"repo.init","path":path.bytes_b64,"initialBranch":initial_branch}),
        )
        .map_err(|_| Error::invalid("Invalid initialization request."))?,
    );
    if journal.existing(operation_id, &hash)?.is_some() {
        return serde_json::to_value(journal.get(operation_id)?).map_err(|_| unknown());
    }
    let (root, directory, identity) = root(path)?;
    let _lock = journal.lock_repository(&identity)?;
    if let Some(record) = journal.existing(operation_id, &hash)? {
        return serde_json::to_value(record).map_err(|_| unknown());
    }
    plain_directory(&root)?;
    let mut record = journal.begin(operation_id, hash, identity)?;
    match prepare(&root, &directory, initial_branch) {
        Ok(value) => {
            record.state = "succeeded".into();
            record.result = Some(value);
        }
        Err(error) => {
            record.state = if error.code == "OUTCOME_UNKNOWN" {
                "outcome_unknown"
            } else {
                "failed"
            }
            .into();
            record.error = Some(error);
        }
    }
    record.seq += 1;
    journal.save(&record).map_err(|_| unknown())?;
    serde_json::to_value(record).map_err(|_| unknown())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    fn setup() -> (tempfile::TempDir, WirePath, Journal) {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        let journal =
            Journal::open(temp.path().join("journal"), Uuid::new_v4().to_string()).unwrap();
        let path = WirePath::new(project.as_os_str().as_bytes());
        (temp, path, journal)
    }
    #[test]
    fn initializes_existing_files_and_replays_without_reinitializing() {
        let (temp, path, journal) = setup();
        let project = temp.path().join("project");
        fs::write(project.join("README.md"), "keep").unwrap();
        let operation = Uuid::new_v4().to_string();
        let result = init(&journal, &operation, &path, "develop").unwrap();
        assert_eq!(result["state"], "succeeded", "{result}");
        let repo = Repository::open(&project).unwrap();
        assert_eq!(
            repo.head().err().unwrap().code(),
            git2::ErrorCode::UnbornBranch
        );
        assert_eq!(repo.references().unwrap().count(), 0);
        assert!(!repo.is_bare());
        assert_eq!(
            repo.workdir().unwrap().canonicalize().unwrap(),
            project.canonicalize().unwrap()
        );
        assert_eq!(
            repo.find_reference("HEAD")
                .unwrap()
                .symbolic_target()
                .unwrap(),
            Some("refs/heads/develop")
        );
        assert_eq!(fs::read(project.join("README.md")).unwrap(), b"keep");
        repo.config()
            .unwrap()
            .set_str("newport.test", "preserve")
            .unwrap();
        assert_eq!(
            init(&journal.clone(), &operation, &path, "develop").unwrap(),
            result
        );
        assert_eq!(
            repo.config().unwrap().get_string("newport.test").unwrap(),
            "preserve"
        );
        assert_eq!(
            init(&journal, &operation, &path, "other").unwrap_err().code,
            "OPERATION_ID_REUSED"
        );
    }
    #[test]
    fn initial_branch_is_always_a_local_branch_name() {
        let (temp, path, journal) = setup();
        let result = init(&journal, &Uuid::new_v4().to_string(), &path, "refs/topic").unwrap();
        assert_eq!(result["state"], "succeeded", "{result}");
        let repo = Repository::open(temp.path().join("project")).unwrap();
        assert_eq!(
            repo.find_reference("HEAD")
                .unwrap()
                .symbolic_target()
                .unwrap(),
            Some("refs/heads/refs/topic")
        );
    }
    #[test]
    fn existing_and_nested_metadata_are_never_replaced() {
        let (temp, path, journal) = setup();
        let project = temp.path().join("project");
        fs::create_dir(project.join(".git")).unwrap();
        let before = fs::metadata(project.join(".git")).unwrap().ino();
        assert_eq!(
            init(&journal, &Uuid::new_v4().to_string(), &path, "main")
                .unwrap_err()
                .code,
            "ALREADY_REPOSITORY"
        );
        assert_eq!(fs::metadata(project.join(".git")).unwrap().ino(), before);
        fs::remove_dir(project.join(".git")).unwrap();
        Repository::init(temp.path()).unwrap();
        assert_eq!(
            init(&journal, &Uuid::new_v4().to_string(), &path, "main")
                .unwrap_err()
                .code,
            "ALREADY_REPOSITORY"
        );
        assert!(!project.join(".git").exists());
    }
    #[test]
    fn publication_refuses_even_an_empty_destination_directory() {
        let (temp, path, _) = setup();
        let project = temp.path().join("project");
        let (_, directory, _) = root(&path).unwrap();
        fs::create_dir(project.join("prepared")).unwrap();
        fs::write(project.join("prepared/HEAD"), "prepared").unwrap();
        fs::create_dir(project.join(".git")).unwrap();
        let inode = fs::metadata(project.join(".git")).unwrap().ino();
        assert_eq!(
            publish(&directory, Path::new("prepared")).unwrap_err().code,
            "ALREADY_REPOSITORY"
        );
        assert_eq!(fs::metadata(project.join(".git")).unwrap().ino(), inode);
        assert_eq!(
            fs::read(project.join("prepared/HEAD")).unwrap(),
            b"prepared"
        );
    }
    #[test]
    fn invalid_roots_and_concurrent_initialization_are_refused() {
        let (temp, path, journal) = setup();
        let (_, _, identity) = root(&path).unwrap();
        let _lock = journal.lock_repository(&identity).unwrap();
        assert_eq!(
            init(&journal, &Uuid::new_v4().to_string(), &path, "main")
                .unwrap_err()
                .code,
            "REPOSITORY_BUSY"
        );
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(temp.path().join("project"), &link).unwrap();
        assert_eq!(
            init(
                &journal,
                &Uuid::new_v4().to_string(),
                &WirePath::new(link.as_os_str().as_bytes()),
                "main"
            )
            .unwrap_err()
            .code,
            "DIRECTORY_REQUIRED"
        );
        assert_eq!(
            init(
                &journal,
                &Uuid::new_v4().to_string(),
                &WirePath::new(b"relative"),
                "main"
            )
            .unwrap_err()
            .code,
            "INVALID_REQUEST"
        );
    }
}
