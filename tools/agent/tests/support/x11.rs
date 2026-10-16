use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::Path,
};

// Retain a hard link on the X socket's filesystem. This prevents inode reuse
// from making a new display look like the socket that the test just stopped.
pub fn socket_witness() -> tempfile::TempDir {
    let public = Path::new("/tmp/.X11-unix");
    match fs::DirBuilder::new().mode(0o1777).create(public) {
        Ok(()) => fs::set_permissions(public, fs::Permissions::from_mode(0o1777)).unwrap(),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => panic!("create X socket directory: {error}"),
    }
    tempfile::Builder::new()
        .prefix("newport-test-x11-")
        .tempdir_in(public)
        .unwrap()
}

pub fn assert_socket_released(socket: &Path, witness: &Path) {
    let original = fs::symlink_metadata(witness).unwrap();
    match fs::symlink_metadata(socket) {
        Ok(current) => assert_ne!(
            (current.dev(), current.ino()),
            (original.dev(), original.ino()),
            "original X11 socket retained: {}",
            socket.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => panic!("inspect X11 socket: {error}"),
    }
}
