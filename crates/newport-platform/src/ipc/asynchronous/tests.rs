use super::*;
use std::{
    io::{Read, Write},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cfg(unix)]
#[tokio::test]
async fn long_profiles_use_private_reusable_endpoints() {
    use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
    let root = std::env::temp_dir().join(format!("np-{}", uuid::Uuid::new_v4()));
    let profile = root.join("long-profile-".repeat(12));
    std::fs::create_dir_all(&profile).unwrap();
    let path = crate::ipc::profile_endpoint(&profile);
    assert!(path.as_os_str().as_bytes().len() < 104);
    assert!(!path.starts_with(&profile));
    let alias = root.join("alias");
    std::os::unix::fs::symlink(&profile, &alias).unwrap();
    assert_eq!(crate::ipc::profile_endpoint(&alias), path);
    assert_ne!(crate::ipc::profile_endpoint(&profile.join("other")), path);
    for _ in 0..2 {
        let mut listener = Listener::bind_owned(&path).unwrap();
        for (entry, mode) in [(path.parent().unwrap(), 0o700), (path.as_path(), 0o600)] {
            assert_eq!(
                std::fs::metadata(entry).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
        let client_path = path.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let mut stream = crate::ipc::connect_private(client_path).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream.write_all(b"ping").unwrap();
            let mut response = [0; 4];
            stream.read_exact(&mut response).unwrap();
            assert_eq!(&response, b"pong");
        });
        let mut stream = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut request = [0; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await.unwrap();
        worker.await.unwrap();
        drop(listener);
        assert!(!path.exists());
    }
    std::fs::remove_dir(path.parent().unwrap()).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn long_profile_endpoints_refuse_symlink_or_public_directories() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("np-{}", uuid::Uuid::new_v4()));
    let profile = root.join("long-profile-".repeat(12));
    std::fs::create_dir_all(&profile).unwrap();
    let path = crate::ipc::profile_endpoint(&profile);
    let parent = path.parent().unwrap();
    std::os::unix::fs::symlink(&root, parent).unwrap();
    assert!(Listener::bind_owned(&path).is_err());
    assert_eq!(
        crate::ipc::connect_private(&path).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    std::fs::remove_file(parent).unwrap();
    crate::filesystem::create_private_directory(parent).unwrap();
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        crate::ipc::connect_private(&path).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    std::fs::remove_dir(parent).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn blocking_clients_exchange_frames_and_report_real_peer_pids() {
    let directory = std::env::temp_dir().join(format!(
        "np-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    ));
    crate::filesystem::create_private_directory(&directory).unwrap();
    let path = directory.join("native-client/desktop.sock");
    let mut listener = Listener::bind(&path).unwrap();
    assert!(Listener::bind(&path).is_err());
    for value in 0..8u32 {
        let path = path.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let mut stream = crate::ipc::Stream::connect(path).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream.write_all(&value.to_be_bytes()).unwrap();
            let mut response = [0; 4];
            stream.read_exact(&mut response).unwrap();
            assert_eq!(u32::from_be_bytes(response), value + 1);
            assert_eq!(stream.read(&mut [0]).unwrap(), 0);
        });
        let mut stream = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(peer_process_id(&stream).unwrap(), std::process::id());
        let received = stream.read_u32().await.unwrap();
        stream.write_u32(received + 1).await.unwrap();
        drop(stream);
        worker.await.unwrap();
    }
    drop(listener);
    // On Windows no endpoint file exists; on Unix it was cleaned by the guard.
    assert!(!path.exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn cancelled_accept_does_not_lose_the_next_connection() {
    let directory = std::env::temp_dir().join(format!(
        "np-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    ));
    crate::filesystem::create_private_directory(&directory).unwrap();
    let path = directory.join("native-client/desktop.sock");
    let mut listener = Listener::bind(&path).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    let worker = tokio::task::spawn_blocking(move || crate::ipc::Stream::connect(path).unwrap());
    let stream = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    drop(stream);
    drop(worker.await.unwrap());
    drop(listener);
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn owned_endpoint_recovers_stale_sockets_but_preserves_other_files_and_replacements() {
    let directory = std::env::temp_dir().join(format!(
        "np-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    ));
    crate::filesystem::create_private_directory(&directory).unwrap();
    let path = directory.join("native-client/desktop.sock");
    crate::filesystem::create_private_directory(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "must stay").unwrap();
    assert!(Listener::bind_owned(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "must stay");
    std::fs::remove_file(&path).unwrap();
    drop(crate::ipc::Listener::bind(&path).unwrap()); // Stale Unix socket remains.
    let listener = Listener::bind_owned(&path).unwrap();
    let replacement = path.with_extension("replacement");
    let alternate = crate::ipc::Listener::bind(&replacement).unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    drop(listener);
    assert!(path.exists(), "must not delete the replacement socket");
    drop(alternate);
    std::fs::remove_dir_all(directory).unwrap();
}
