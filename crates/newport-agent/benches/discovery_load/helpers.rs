use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs, io,
    os::unix::{fs::MetadataExt, net::UnixDatagram},
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum Operation {
    Listeners,
    Projects,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Measurement {
    pub operation: Operation,
    pub effective_uid: u32,
    pub peak_bytes: u64,
    pub total_bytes: u64,
    pub retained_bytes: i64,
    pub elapsed_us: u128,
    pub exit_code: i32,
}

#[derive(Deserialize, Serialize)]
enum Event {
    Started { pid: u32 },
    Finished { pid: u32, measurement: Measurement },
}

fn path() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .with_extension("helpers.sock")
}

pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not change credentials.
    unsafe { libc::geteuid() }
}

/// Measures the synchronous production helper; reports do not enter its wire stream.
pub fn run(args: &[OsString]) -> i32 {
    let operation = match args[1].to_str().unwrap() {
        "listeners" => Operation::Listeners,
        "projects" => Operation::Projects,
        _ => unreachable!(),
    };
    let socket = UnixDatagram::unbound().unwrap();
    socket.connect(path()).unwrap();
    socket
        .set_write_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let send = |event| {
        let bytes = serde_json::to_vec(&event).unwrap();
        assert!(bytes.len() < 1024);
        assert_eq!(socket.send(&bytes).unwrap(), bytes.len());
    };
    let pid = std::process::id();
    send(Event::Started { pid });
    let started = Instant::now();
    let mut result = Ok(0);
    let allocation = allocation_counter::measure(|| {
        result = newport_agent::cli::run(args, &[]);
    });
    let elapsed_us = started.elapsed().as_micros();
    let exit_code = result.unwrap_or(1);
    send(Event::Finished {
        pid,
        measurement: Measurement {
            operation,
            effective_uid: effective_uid(),
            peak_bytes: allocation.bytes_max,
            total_bytes: allocation.bytes_total,
            retained_bytes: allocation.bytes_current,
            elapsed_us,
            exit_code,
        },
    });
    exit_code
}

/// A socket next to the runner's uniquely copied benchmark, owned by this run.
pub struct Reports {
    socket: UnixDatagram,
    path: PathBuf,
    identity: fs::Metadata,
}

impl Reports {
    pub fn bind() -> Self {
        let path = path();
        let socket = UnixDatagram::bind(&path).unwrap();
        let identity = fs::symlink_metadata(&path).unwrap();
        socket.set_nonblocking(true).unwrap();
        Self {
            socket,
            path,
            identity,
        }
    }

    pub fn take(&self) -> Vec<Measurement> {
        let mut active = BTreeMap::new();
        let mut measurements = Vec::new();
        let mut buffer = [0; 1024];
        let mut events = 0;
        loop {
            let size = match self.socket.recv(&mut buffer) {
                Ok(size) => size,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => panic!("Reading helper measurement: {error}"),
            };
            events += 1;
            assert!(events <= 8 && size < buffer.len());
            match serde_json::from_slice(&buffer[..size]).unwrap() {
                Event::Started { pid } => {
                    assert!(active.insert(pid, ()).is_none());
                }
                Event::Finished { pid, measurement } => {
                    assert!(active.remove(&pid).is_some());
                    assert_eq!(measurement.exit_code, 0);
                    assert!(measurement.peak_bytes > 0);
                    assert!(measurement.total_bytes >= measurement.peak_bytes);
                    measurements.push(measurement);
                }
            }
        }
        assert!(
            active.is_empty(),
            "A helper exited without completing its measurement"
        );
        measurements
    }
}

impl Drop for Reports {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|current| {
            current.dev() == self.identity.dev() && current.ino() == self.identity.ino()
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
