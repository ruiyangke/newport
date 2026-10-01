use russh::{server::Msg, Channel, ChannelMsg};
use std::{
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::PathBuf,
    sync::Arc,
};
use tokio::{io::unix::AsyncFd, task::JoinHandle};

// The shell starts its own process group; aborting a session also stops its children.
struct ProcessGroup(i32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}
pub struct Pty {
    master: Arc<AsyncFd<OwnedFd>>,
    slave: Option<File>,
}
impl Pty {
    pub fn new(cols: u32, rows: u32) -> io::Result<Self> {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: openpty initializes both descriptors; ownership transfers exactly once below.
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        // Keep the master out of children; nonblocking I/O is driven by Tokio readiness.
        if unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0
            || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let pty = Self {
            master: Arc::new(AsyncFd::new(master)?),
            slave: Some(slave),
        };
        pty.resize(cols, rows)?;
        Ok(pty)
    }
    pub fn resize(&self, cols: u32, rows: u32) -> io::Result<()> {
        let size = libc::winsize {
            ws_row: rows as u16,
            ws_col: cols as u16,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: descriptor is live and size is a valid winsize.
        if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn start(&mut self, channel: Channel<Msg>, home: PathBuf) -> io::Result<JoinHandle<()>> {
        let slave = self
            .slave
            .take()
            .ok_or_else(|| io::Error::other("Shell already started"))?;
        let mut command = std::process::Command::new("/bin/bash");
        command
            .args(["--noprofile", "--norc", "-i"])
            .current_dir(&home)
            .env("HOME", home)
            .env("PS1", "fixture> ")
            .env("TERM", "xterm-256color")
            .env("HISTFILE", "/dev/null")
            .stdin(slave.try_clone()?)
            .stdout(slave.try_clone()?)
            .stderr(slave);
        // SAFETY: only async-signal-safe syscalls run between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut command = tokio::process::Command::from(command);
        command.kill_on_drop(true);
        let mut child = command.spawn()?;
        let group = ProcessGroup(child.id().expect("new child has PID") as i32);
        let master = self.master.clone();
        Ok(tokio::spawn(async move {
            let _group = group;
            let (mut reader, writer) = channel.split();
            let mut bytes = [0; 32768];
            loop {
                tokio::select! {
                    result = read(&master, &mut bytes) => match result { Ok(0) | Err(_) => break, Ok(n) => if writer.data(&bytes[..n]).await.is_err() { break; } },
                    msg = reader.wait() => match msg { Some(ChannelMsg::Data { data }) => if write(&master, &data).await.is_err() { break; }, Some(ChannelMsg::Close | ChannelMsg::Eof) | None => break, _ => {} },
                    status = child.wait() => { let _ = writer.exit_status(status.ok().and_then(|s|s.code()).unwrap_or(1) as u32).await; let _ = writer.eof().await; let _ = writer.close().await; return; }
                }
            }
            // A hung shell (or its children) must never outlive a disconnected fixture session.
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let status = child.wait().await;
            let _ = writer
                .exit_status(status.ok().and_then(|s| s.code()).unwrap_or(1) as u32)
                .await;
            let _ = writer.close().await;
        }))
    }
}
async fn read(fd: &AsyncFd<OwnedFd>, bytes: &mut [u8]) -> io::Result<usize> {
    loop {
        let mut ready = fd.readable().await?;
        match ready.try_io(|fd| {
            let n = unsafe { libc::read(fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
            if n < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        }) {
            Ok(result) => return result,
            Err(_) => continue,
        }
    }
}
async fn write(fd: &AsyncFd<OwnedFd>, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let mut ready = fd.writable().await?;
        if let Ok(result) = ready.try_io(|fd| {
            let n = unsafe { libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
            if n < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        }) {
            let n = result?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            bytes = &bytes[n..];
        }
    }
    Ok(())
}
