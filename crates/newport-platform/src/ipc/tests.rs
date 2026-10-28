use super::{Listener, Stream};
use std::{
    io::{Read, Write},
    net::Shutdown,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[test]
fn timeout_keeps_partial_frame_and_allows_a_later_read() {
    let (mut reader, mut writer) = Stream::pair().unwrap();
    reader
        .set_read_timeout(Some(Duration::from_millis(30)))
        .unwrap();
    writer.write_all(b"abc").unwrap();
    let mut first = [0; 3];
    reader.read_exact(&mut first).unwrap();
    let error = reader.read(&mut [0]).unwrap_err();
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    writer.write_all(b"def").unwrap();
    reader.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"def");
}

#[test]
fn closing_writer_preserves_the_final_response_and_reports_eof() {
    let (mut reader, mut writer) = Stream::pair().unwrap();
    reader
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let payload = vec![42; 128 * 1024];
    let sent = payload.clone();
    let worker = thread::spawn(move || writer.write_all(&sent).unwrap());
    let mut received = Vec::new();
    reader.read_to_end(&mut received).unwrap();
    worker.join().unwrap();
    assert_eq!(received, payload);
}

#[test]
fn clone_shutdown_interrupts_a_blocked_reader_and_closes_the_peer() {
    let (mut reader, mut peer) = Stream::pair().unwrap();
    let cancel = reader.try_clone().unwrap();
    let (done, result) = mpsc::channel();
    let worker = thread::spawn(move || {
        let value = reader.read(&mut [0]);
        done.send(value).unwrap();
    });
    cancel.shutdown(Shutdown::Both).unwrap();
    let value = result.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(value, Ok(0) | Err(_)));
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    worker.join().unwrap();
}

#[test]
fn clone_shutdown_interrupts_backpressured_writes() {
    let (mut writer, _unread_peer) = Stream::pair().unwrap();
    let cancel = writer.try_clone().unwrap();
    let (done, result) = mpsc::channel();
    let worker = thread::spawn(move || {
        let bytes = [0; 64 * 1024];
        loop {
            if let Err(error) = writer.write_all(&bytes) {
                done.send(error.kind()).unwrap();
                break;
            }
        }
    });
    thread::sleep(Duration::from_millis(30));
    cancel.shutdown(Shutdown::Both).unwrap();
    result
        .recv_timeout(Duration::from_secs(2))
        .expect("write remained blocked after cancellation");
    worker.join().unwrap();
}

#[test]
fn listener_is_nonblocking_and_rejects_a_second_owner() {
    let path = std::env::temp_dir().join(format!("np-ipc-{}.sock", uuid::Uuid::new_v4()));
    let listener = Listener::bind(&path).unwrap();
    assert!(Listener::bind(&path).is_err());
    listener.set_nonblocking(true).unwrap();
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    let mut client = Stream::connect(&path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut server = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(1))
            }
            Err(error) => panic!("accept: {error}"),
        }
    };
    server.set_nonblocking(false).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    server.write_all(b"ready").unwrap();
    let mut bytes = [0; 5];
    client.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"ready");
    drop(listener);
    let _ = std::fs::remove_file(path);
}

#[test]
fn concurrent_connections_wait_for_accept_without_losing_requests() {
    let path = std::env::temp_dir().join(format!("np-ipc-{}.sock", uuid::Uuid::new_v4()));
    let listener = Listener::bind(&path).unwrap();
    let workers: Vec<_> = (0u8..12)
        .map(|value| {
            let path = path.clone();
            thread::spawn(move || {
                let mut stream = Stream::connect(path).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream.write_all(&[value]).unwrap();
                let mut reply = [0];
                stream.read_exact(&mut reply).unwrap();
                assert_eq!(reply, [value]);
            })
        })
        .collect();
    let mut received = Vec::new();
    for _ in 0..workers.len() {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut value = [0];
        stream.read_exact(&mut value).unwrap();
        received.push(value[0]);
        stream.write_all(&value).unwrap();
    }
    for worker in workers {
        worker.join().unwrap();
    }
    received.sort_unstable();
    assert_eq!(received, (0u8..12).collect::<Vec<_>>());
    drop(listener);
    let _ = std::fs::remove_file(path);
}

#[test]
fn private_directories_are_new_and_files_are_readable() {
    let directory = std::env::temp_dir().join(format!("np-private-{}", uuid::Uuid::new_v4()));
    crate::filesystem::create_private_directory(&directory).unwrap();
    assert!(crate::filesystem::create_private_directory(&directory).is_err());
    let path = directory.join("probe");
    let mut file = crate::filesystem::private_file_options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(b"private").unwrap();
    drop(file);
    assert_eq!(std::fs::read(&path).unwrap(), b"private");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
