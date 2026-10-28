use crate::{
    ipc::windows::{create_on_current_runtime, pipe_name},
    security,
};
use std::{
    ffi::OsString,
    io,
    os::windows::io::AsRawHandle,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer},
};

#[derive(Debug)]
pub enum Stream {
    Client(NamedPipeClient),
    Server(NamedPipeServer),
}
impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Client(pipe) => Pin::new(pipe).poll_read(cx, buf),
            Self::Server(pipe) => Pin::new(pipe).poll_read(cx, buf),
        }
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Client(pipe) => Pin::new(pipe).poll_write(cx, buf),
            Self::Server(pipe) => Pin::new(pipe).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Client(pipe) => Pin::new(pipe).poll_flush(cx),
            Self::Server(pipe) => Pin::new(pipe).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Client(pipe) => Pin::new(pipe).poll_shutdown(cx),
            Self::Server(pipe) => Pin::new(pipe).poll_shutdown(cx),
        }
    }
}
pub struct Listener {
    name: OsString,
    next: NamedPipeServer,
}
impl Listener {
    pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        super::directory(path.as_ref())?;
        let name = pipe_name(path.as_ref())?;
        let next = create_on_current_runtime(&name, true)?;
        Ok(Self { name, next })
    }
    /// Windows releases abandoned pipe names automatically. A living owner
    /// is never replaced, even when the caller owns the profile lock.
    pub fn bind_owned(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::bind(path)
    }
    pub async fn accept(&mut self) -> io::Result<Stream> {
        loop {
            self.next.connect().await?;
            let next = create_on_current_runtime(&self.name, false)?;
            let connected = std::mem::replace(&mut self.next, next);
            if security::verify_client(connected.as_raw_handle()).is_ok() {
                return Ok(Stream::Server(connected));
            }
        }
    }
}
pub fn peer_process_id(stream: &Stream) -> io::Result<u32> {
    match stream {
        Stream::Client(pipe) => security::peer_pid(pipe.as_raw_handle(), false),
        Stream::Server(pipe) => security::peer_pid(pipe.as_raw_handle(), true),
    }
}
pub async fn connect_agent(path: impl AsRef<Path>) -> io::Result<Stream> {
    let path = path.as_ref();
    if !path.as_os_str().to_string_lossy().starts_with(r"\\.\pipe\") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected a local SSH agent pipe",
        ));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match ClientOptions::new().open(path) {
                Err(error)
                    if error.raw_os_error()
                        == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32) =>
                {
                    tokio::time::sleep(Duration::from_millis(2)).await
                }
                result => break result.map(Stream::Client),
            }
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SSH agent pipe remained busy"))?
}
pub async fn pair() -> io::Result<(Stream, Stream)> {
    let path = std::env::temp_dir().join(format!("np-pair-{}", uuid::Uuid::new_v4()));
    // No new profile directory: this is a unique name in the existing temp dir.
    let name = pipe_name(&path)?;
    let server = create_on_current_runtime(&name, true)?;
    let client = ClientOptions::new().open(&name)?;
    security::verify_server(client.as_raw_handle())?;
    server.connect().await?;
    security::verify_client(server.as_raw_handle())?;
    Ok((Stream::Client(client), Stream::Server(server)))
}
