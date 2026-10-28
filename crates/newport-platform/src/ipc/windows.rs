use crate::security;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    future::{Future, poll_fn},
    io::{self, Read, Write},
    net::Shutdown,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
    time::Duration,
};
use tokio::{
    net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions},
    runtime::Runtime,
    sync::watch,
};

fn runtime() -> io::Result<&'static Runtime> {
    static RUNTIME: OnceLock<io::Result<Runtime>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("newport-local-io")
                .enable_all()
                .build()
        })
        .as_ref()
        .map_err(|error| io::Error::other(error.to_string()))
}

// Resolve the caller's absolute endpoint through its canonical parent and user
// SID to produce a stable, bounded, local pipe name.
pub(super) fn pipe_name(path: &Path) -> io::Result<OsString> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Local endpoint must be absolute",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing endpoint directory"))?
        .canonicalize()?;
    let file = path
        .file_name()
        .ok_or_else(|| io::Error::other("Missing endpoint name"))?;
    let mut hash = Sha256::new();
    hash.update(security::current_user()?.as_bytes());
    for unit in parent.join(file).as_os_str().encode_wide() {
        hash.update(unit.to_le_bytes());
    }
    let digest: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(format!(r"\\.\pipe\newport-{digest}").into())
}

#[derive(Debug)]
enum Pipe {
    Client(NamedPipeClient),
    Server(NamedPipeServer),
}
impl Pipe {
    fn try_read(&self, bytes: &mut [u8]) -> io::Result<usize> {
        let result = match self {
            Self::Client(p) => p.try_read(bytes),
            Self::Server(p) => p.try_read(bytes),
        };
        match result {
            // Match socket EOF semantics, including after the peer exits.
            Err(error)
                if error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_BROKEN_PIPE as i32) =>
            {
                Ok(0)
            }
            result => result,
        }
    }
    fn try_write(&self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Client(p) => p.try_write(bytes),
            Self::Server(p) => p.try_write(bytes),
        }
    }
    async fn ready(&self, read: bool) -> io::Result<()> {
        match (self, read) {
            (Self::Client(p), true) => p.readable().await,
            (Self::Client(p), false) => p.writable().await,
            (Self::Server(p), true) => p.readable().await,
            (Self::Server(p), false) => p.writable().await,
        }
    }
}
#[derive(Debug, Default, Clone, Copy)]
struct Options {
    read: Option<Duration>,
    write: Option<Duration>,
    nonblocking: bool,
}
#[derive(Debug)]
struct Shared {
    pipe: Mutex<Option<Arc<Pipe>>>,
    options: Mutex<Options>,
    stopped: watch::Sender<bool>,
}
#[derive(Debug)]
pub struct Stream(Arc<Shared>);
impl Stream {
    fn new(pipe: Pipe) -> Self {
        Self(Arc::new(Shared {
            pipe: Mutex::new(Some(Arc::new(pipe))),
            options: Mutex::new(Options::default()),
            stopped: watch::channel(false).0,
        }))
    }
    pub fn connect(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::open(&pipe_name(path.as_ref())?, true)
    }
    pub(super) fn connect_agent(path: &Path) -> io::Result<Self> {
        // The OS agent may run as a system service, so its identity policy is
        // distinct from the same-user Newport backend. Never allow remote pipes.
        if !path.as_os_str().to_string_lossy().starts_with(r"\\.\pipe\") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected a local SSH agent pipe",
            ));
        }
        Self::open(path.as_os_str(), false)
    }
    fn open(name: &std::ffi::OsStr, verify: bool) -> io::Result<Self> {
        // Named pipes have no socket-style accept backlog. Waiting for a free
        // instance is connection establishment only: no request bytes exist
        // yet, and a missing endpoint still fails immediately.
        let pipe = runtime()?.block_on(deadline(Some(Duration::from_secs(10)), async {
            loop {
                match ClientOptions::new().open(name) {
                    Err(error)
                        if error.raw_os_error()
                            == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32) =>
                    {
                        tokio::time::sleep(Duration::from_millis(2)).await
                    }
                    result => break result,
                }
            }
        }))?;
        if verify {
            security::verify_server(pipe.as_raw_handle())?;
        }
        Ok(Self::new(Pipe::Client(pipe)))
    }
    pub fn pair() -> io::Result<(Self, Self)> {
        let path = std::env::temp_dir().join(format!("newport-pair-{}", uuid::Uuid::new_v4()));
        let listener = Listener::bind(&path)?;
        let client = Self::connect(&path)?;
        let (server, _) = listener.accept()?;
        Ok((client, server))
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self(self.0.clone()))
    }
    pub fn set_nonblocking(&self, value: bool) -> io::Result<()> {
        self.0
            .options
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .nonblocking = value;
        Ok(())
    }
    pub fn set_read_timeout(&self, value: Option<Duration>) -> io::Result<()> {
        validate_timeout(value)?;
        self.0
            .options
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .read = value;
        Ok(())
    }
    pub fn set_write_timeout(&self, value: Option<Duration>) -> io::Result<()> {
        validate_timeout(value)?;
        self.0
            .options
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write = value;
        Ok(())
    }
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        if how != Shutdown::Both {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Local pipes require full shutdown",
            ));
        }
        self.0.stopped.send_replace(true);
        // In-flight operations select on stopped and release their Arc. Idle
        // clones retain no OS handle after shutdown; the peer sees EOF.
        self.0.pipe.lock().unwrap_or_else(|e| e.into_inner()).take();
        Ok(())
    }
    fn io(
        &self,
        read: bool,
        mut operation: impl FnMut(&Pipe) -> io::Result<usize>,
    ) -> io::Result<usize> {
        let mut stopped = self.0.stopped.subscribe();
        let pipe = self
            .0
            .pipe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(closed)?;
        let options = *self.0.options.lock().unwrap_or_else(|e| e.into_inner());
        if *stopped.borrow() {
            return Err(closed());
        }
        if options.nonblocking {
            return operation(&pipe);
        }
        runtime()?.block_on(async {
            let work = async {
                loop {
                    match operation(&pipe) {
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            pipe.ready(read).await?
                        }
                        result => return result,
                    }
                }
            };
            tokio::select! {
                biased;
                _ = stopped.changed() => Err(closed()),
                result = deadline(if read { options.read } else { options.write }, work) => result,
            }
        })
    }
}
fn closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "Local stream was shut down",
    )
}
fn validate_timeout(value: Option<Duration>) -> io::Result<()> {
    if value.is_some_and(|v| v.is_zero()) {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Timeout must be nonzero",
        ))
    } else {
        Ok(())
    }
}
async fn deadline<T>(
    timeout: Option<Duration>,
    work: impl Future<Output = io::Result<T>>,
) -> io::Result<T> {
    match timeout {
        Some(timeout) => tokio::time::timeout(timeout, work)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Local I/O timed out"))?,
        None => work.await,
    }
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.io(true, |pipe| pipe.try_read(bytes))
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.io(false, |pipe| pipe.try_write(bytes))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct Listener {
    name: OsString,
    next: Mutex<NamedPipeServer>,
    nonblocking: AtomicBool,
}
impl Listener {
    pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        let name = pipe_name(path.as_ref())?;
        let next = create(&name, true)?;
        Ok(Self {
            name,
            next: Mutex::new(next),
            nonblocking: AtomicBool::new(false),
        })
    }
    pub fn set_nonblocking(&self, value: bool) -> io::Result<()> {
        self.nonblocking.store(value, Ordering::Release);
        Ok(())
    }
    pub fn accept(&self) -> io::Result<(Stream, ())> {
        let mut pending = self.next.lock().unwrap_or_else(|e| e.into_inner());
        runtime()?.block_on(async {
            if self.nonblocking.load(Ordering::Acquire) {
                let mut connection = std::pin::pin!(pending.connect());
                poll_fn(|cx| match connection.as_mut().poll(cx) {
                    Poll::Ready(result) => Poll::Ready(result),
                    Poll::Pending => Poll::Ready(Err(io::ErrorKind::WouldBlock.into())),
                })
                .await?;
            } else {
                pending.connect().await?;
            }
            // Keep an instance alive continuously so another process cannot
            // claim this name between accepting and creating the next listener.
            let next = create(&self.name, false)?;
            Ok((
                Stream::new(Pipe::Server(std::mem::replace(&mut *pending, next))),
                (),
            ))
        })
    }
}
fn create(name: &std::ffi::OsStr, first: bool) -> io::Result<NamedPipeServer> {
    let _runtime = runtime()?.enter();
    create_on_current_runtime(name, first)
}

pub(super) fn create_on_current_runtime(
    name: &std::ffi::OsStr,
    first: bool,
) -> io::Result<NamedPipeServer> {
    security::with_attributes(|attributes| {
        // SECURITY_ATTRIBUTES and its descriptor live until CreateNamedPipeW
        // returns. Windows copies the descriptor; handles are non-inheritable.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(name, attributes)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{
        Security::{ImpersonateAnonymousToken, RevertToSelf},
        System::Threading::GetCurrentThread,
    };

    #[test]
    fn actual_pipe_acl_rejects_anonymous_clients() {
        let path = std::env::temp_dir().join(format!("np-acl-{}", uuid::Uuid::new_v4()));
        let _listener = Listener::bind(&path).unwrap();
        let name = pipe_name(&path).unwrap();
        let _runtime = runtime().unwrap().enter();
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                assert_ne!(
                    unsafe { RevertToSelf() },
                    0,
                    "Could not restore test identity"
                );
            }
        }
        let denied = {
            assert_ne!(unsafe { ImpersonateAnonymousToken(GetCurrentThread()) }, 0);
            let _restore = Restore;
            // Connect to the already-resolved name so this exercises the pipe
            // DACL, not filesystem permissions during endpoint resolution.
            ClientOptions::new().open(&name).unwrap_err()
        };
        assert_eq!(denied.kind(), io::ErrorKind::PermissionDenied);
        assert!(ClientOptions::new().open(&name).is_ok());
    }
}
