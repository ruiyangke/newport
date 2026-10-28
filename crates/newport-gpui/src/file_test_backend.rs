//! Strict local RPC peer for Files tests. It never discovers the installed host
//! or reaches a real server; each response is released explicitly by its test.
use newport_client::backend::Backend;
use newport_platform::ipc::protocol::{Request, Response, VERSION};
use newport_platform::ipc::{Listener, Stream};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

pub(crate) struct Host {
    path: PathBuf,
    listener: Listener,
    connection: Value,
}
impl Host {
    pub(crate) fn with_connection(connection: Value) -> Self {
        let path =
            std::env::temp_dir().join(format!("np-files-{}.sock", uuid::Uuid::new_v4().simple()));
        let listener = Listener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            path,
            listener,
            connection,
        }
    }
    pub(crate) fn backend(&self) -> Backend {
        Backend::at_path(self.path.clone())
    }
    pub(crate) fn try_receive(&self) -> Option<(Stream, Request)> {
        let (mut stream, _) = match self.listener.accept() {
            Ok(pair) => pair,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return None,
            Err(error) => panic!("{error}"),
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut header = [0; 4];
        stream.read_exact(&mut header).unwrap();
        let size = u32::from_be_bytes(header) as usize;
        assert!(size < 1024 * 1024);
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).unwrap();
        let request: Request = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(request.version, VERSION);
        assert!(matches!(
            request.method.as_str(),
            "files_list" | "files_cancel"
        ));
        if request.method == "files_list" {
            assert_eq!(request.args["id"], "fixture");
            assert_eq!(request.args["expectedConnection"], self.connection);
        }
        uuid::Uuid::parse_str(request.args["operation"].as_str().unwrap()).unwrap();
        Some((stream, request))
    }
    fn receive(&self, method: &str) -> (Stream, Request) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some((stream, request)) = self.try_receive() {
                assert_eq!(request.method, method);
                return (stream, request);
            }
            assert!(Instant::now() < deadline, "missing {method} RPC");
            thread::sleep(Duration::from_millis(1));
        }
    }
    pub(crate) fn listing(&self, path: &str) -> Stream {
        let (stream, request) = self.receive("files_list");
        assert_eq!(request.args["path"], path);
        stream
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) fn reply(mut stream: Stream, result: Result<Value, &str>) {
    let response = match result {
        Ok(result) => Response {
            version: VERSION,
            result,
            error: None,
        },
        Err(error) => Response {
            version: VERSION,
            result: Value::Null,
            error: Some(json!(error)),
        },
    };
    let bytes = rmp_serde::to_vec_named(&response).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}
