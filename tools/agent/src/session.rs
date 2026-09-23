//! Owns account locks, backend lifetimes, and published session metadata.
use crate::{
    local_socket::Endpoint,
    paths::{private_write, root},
    wl_server, x_server,
};
use std::{
    fs, io,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        net::UnixListener,
    },
    path::PathBuf,
    time::Duration,
};
struct Cleanup(std::path::PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for name in [
            "snapshot.tar",
            "display",
            "Xauthority",
            "agent-client",
            "features",
        ] {
            let _ = fs::remove_file(self.0.join(name));
        }
    }
}

pub(crate) struct Session {
    // Fields drop in order: stop readers before deleting files and releasing ownership.
    x11: Option<x_server::Server>,
    wayland: Option<wl_server::Server>,
    _cleanup: Cleanup,
    _lock: fs::File,
    _endpoint: Endpoint,
    pub(crate) snapshot: PathBuf,
}
impl Session {
    pub(crate) fn start(
        client: &str,
        clipboard: bool,
        browser: bool,
    ) -> io::Result<(Self, UnixListener)> {
        let root = root()?;
        let (listener, _endpoint) = Endpoint::fixed(&root.join("agent.sock")).map_err(|e| {
            if matches!(
                e.kind(),
                io::ErrorKind::AddrInUse | io::ErrorKind::WouldBlock
            ) && fs::read_to_string(root.join("agent-client"))
                .ok()
                .as_deref()
                == Some(client)
            {
                io::Error::new(io::ErrorKind::WouldBlock, "previous agent is still closing")
            } else if matches!(
                e.kind(),
                io::ErrorKind::AddrInUse | io::ErrorKind::WouldBlock
            ) {
                io::Error::other("Integration is active in another Porthop installation.")
            } else {
                e
            }
        })?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(root.join("lock"))?;
        let meta = lock.metadata()?;
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
            return Err(io::Error::other("invalid clipboard lock"));
        }
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|e| {
            if e.kind() != io::ErrorKind::WouldBlock {
                return e;
            }
            if fs::read_to_string(root.join("client"))
                .ok()
                .as_deref()
                .map(str::trim)
                == Some(client)
            {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "previous clipboard helper is still closing",
                )
            } else {
                io::Error::other("Clipboard is locked by another process.")
            }
        })?;
        let legacy_active = fs::metadata(root.join("session"))
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(120));
        if legacy_active
            && fs::read_to_string(root.join("client"))
                .ok()
                .as_deref()
                .map(str::trim)
                != Some(client)
        {
            return Err(io::Error::other(
                "Clipboard sync is active in another Porthop installation.",
            ));
        }
        let _cleanup = Cleanup(root.clone());
        let snapshot = root.join("snapshot.tar");
        match fs::remove_file(&snapshot) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let wayland = clipboard
            .then(|| {
                wl_server::Server::start_fixed(&snapshot, &root.join("wayland.sock"))
                    .map_err(|e| io::Error::other(format!("Cannot start Wayland clipboard: {e}")))
            })
            .transpose()?;
        let x11 = clipboard
            .then(|| x_server::Server::start(&snapshot, None))
            .transpose()?;
        private_write(&root.join("agent-client"), client.as_bytes())?;
        for name in [
            "snapshot.tar",
            "session",
            "client",
            "browser-url",
            "browser-lease",
            "native-display",
        ] {
            let _ = fs::remove_file(root.join(name));
        }
        private_write(
            &root.join("features"),
            format!(
                "{} {}",
                if clipboard { "clipboard" } else { "" },
                if browser { "browser" } else { "" }
            )
            .as_bytes(),
        )?;
        if let Some(x11) = &x11 {
            private_write(&root.join("display"), x11.display.as_bytes())?;
            private_write(&root.join("Xauthority"), &fs::read(x11.authority())?)?;
        } else {
            let _ = fs::remove_file(root.join("display"));
            let _ = fs::remove_file(root.join("Xauthority"));
        }

        Ok((
            Self {
                x11,
                wayland,
                _cleanup,
                _lock: lock,
                _endpoint,
                snapshot,
            },
            listener,
        ))
    }
    pub(crate) fn check_backends(&self) -> io::Result<()> {
        if self.x11.as_ref().is_some_and(|x| !x.is_running()) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "X11 clipboard service stopped",
            ));
        }
        if self.wayland.as_ref().is_some_and(|w| !w.is_running()) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Wayland clipboard service stopped",
            ));
        }
        Ok(())
    }
}
