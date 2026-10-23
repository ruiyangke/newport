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

/// This session behind its desktop's alias, while it is the newest one.
struct Alias {
    path: PathBuf,
    target: PathBuf,
}
impl Alias {
    /// Moves the alias to `target` atomically. Anything but a link there
    /// is left alone, and the session simply has no alias.
    fn point(path: PathBuf, target: &std::path::Path) -> io::Result<Option<Self>> {
        if fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_symlink()) {
            return Ok(None);
        }
        let staged = path.with_file_name(format!(
            ".{}.{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        let _ = fs::remove_file(&staged);
        std::os::unix::fs::symlink(target, &staged)?;
        fs::rename(&staged, &path)?;
        Ok(Some(Self {
            path,
            target: target.to_owned(),
        }))
    }
}
impl Drop for Alias {
    fn drop(&mut self) {
        // A newer session of this desktop owns it now: leave it.
        if fs::read_link(&self.path).is_ok_and(|t| t == self.target) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) struct Session {
    // Fields drop in order: stop readers before deleting files and releasing ownership.
    _alias: Option<Alias>,
    clipboard: bool,
    gui_notices: bool,
    pub(crate) gui_launch: Option<crate::gui_launch::Broker>,
    x11: Option<x_server::Server>,
    wayland: Option<wl_server::Server>,
    publication: Option<crate::routing::Publication>,
    _cleanup: Cleanup,
    _lock: fs::File,
    _endpoint: Endpoint,
    _directory: Option<crate::owned_directory::Directory>,
    pub(crate) snapshot: PathBuf,
    pub(crate) source: crate::clipboard_source::Source,
}
impl Session {
    pub(crate) fn set_clipboard(&mut self, enabled: bool) -> io::Result<()> {
        if self.clipboard == enabled {
            return Ok(());
        }
        let root = self.snapshot.parent().expect("session root");
        if !enabled {
            self.clipboard = false;
            if self.gui_notices {
                // The closed demand store prevents any clipboard reads, while
                // display requests can still guide the user to GUI setup.
                let _ = fs::remove_file(root.join("snapshot.tar"));
                return Ok(());
            }
            self.x11 = None;
            self.wayland = None;
            for name in ["snapshot.tar", "display", "Xauthority"] {
                match fs::remove_file(root.join(name)) {
                    Ok(()) => (),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error),
                }
            }
            return Ok(());
        }
        self.x11 = None;
        self.wayland = None;
        let mut source = crate::clipboard_source::Source::new(&self.snapshot);
        source.gui = self.source.gui.clone();
        let wayland = wl_server::Server::start_source(source.clone(), &root.join("wayland.sock"))?;
        let x11 = optional_x11(source.clone())?;
        if let Some(x11) = &x11 {
            private_write(&root.join("display"), x11.display.as_bytes())?;
            private_write(&root.join("Xauthority"), &fs::read(x11.authority())?)?;
        }
        self.clipboard = true;
        self.source = source;
        self.wayland = Some(wayland);
        self.x11 = x11;
        Ok(())
    }

    pub(crate) fn clipboard_enabled(&self) -> bool {
        self.clipboard
    }

    pub(crate) fn refresh_clipboard(&self) -> io::Result<()> {
        if let Some(wayland) = &self.wayland {
            wayland.refresh()?;
        }
        Ok(())
    }

    pub(crate) fn enable_gui_notices(&mut self) -> io::Result<()> {
        if self.gui_notices {
            return Ok(());
        }
        let clipboard = self.clipboard;
        self.set_clipboard(true)?;
        self.clipboard = clipboard;
        self.gui_notices = true;
        self.source.gui.enable();
        Ok(())
    }

    pub(crate) fn publish_features(&self, browser: bool) -> io::Result<()> {
        private_write(
            &self.snapshot.with_file_name("features"),
            format!(
                "{} {} {} {}",
                if self.clipboard_enabled() {
                    "clipboard"
                } else {
                    ""
                },
                if browser { "browser" } else { "" },
                if self.gui_notices { "gui" } else { "" },
                if self.gui_launch.is_some() {
                    "gui-launch"
                } else {
                    ""
                }
            )
            .as_bytes(),
        )
    }

    pub(crate) fn start(
        client: &str,
        clipboard: bool,
        browser: bool,
        scope: Option<crate::routing::Scope>,
    ) -> io::Result<(Self, UnixListener)> {
        let directory = scope
            .as_ref()
            .map(|s| crate::owned_directory::Directory::new(&s.directory, "i-"))
            .transpose()?;
        let root = match &directory {
            Some(d) => d.path().to_owned(),
            None => root()?,
        };
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
                io::Error::other("Integration is active in another Newport installation.")
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
        let _cleanup = Cleanup(root.clone());
        let snapshot = root.join("snapshot.tar");
        match fs::remove_file(&snapshot) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let source = crate::clipboard_source::Source::new(&snapshot);
        let wayland = clipboard
            .then(|| {
                wl_server::Server::start_source(source.clone(), &root.join("wayland.sock"))
                    .map_err(|e| io::Error::other(format!("Cannot start Wayland clipboard: {e}")))
            })
            .transpose()?;
        let x11 = clipboard
            .then(|| optional_x11(source.clone()))
            .transpose()?
            .flatten();
        private_write(&root.join("agent-client"), client.as_bytes())?;
        for name in [
            "snapshot.tar",
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

        let publication = scope
            .as_ref()
            .map(|scope| crate::routing::publish(scope, client, &root))
            .transpose()?;
        // A standalone root is already one fixed directory.
        let alias = match crate::paths::client_alias(&root, client) {
            Some(path) if scope.is_some() => Alias::point(path, &root)?,
            _ => None,
        };
        Ok((
            Self {
                _alias: alias,
                clipboard,
                gui_notices: false,
                gui_launch: None,
                publication,
                _directory: directory,
                x11,
                wayland,
                _cleanup,
                _lock: lock,
                _endpoint,
                snapshot,
                source,
            },
            listener,
        ))
    }
    pub(crate) fn check_backends(&mut self) -> io::Result<()> {
        if let Some(publication) = &mut self.publication {
            publication.check_current()?;
        }
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

// X11 clients require the standard public socket directory. If host policy
// forbids it, preserve Wayland, CLI clipboard, and browser integration.
fn optional_x11(source: crate::clipboard_source::Source) -> io::Result<Option<x_server::Server>> {
    match x_server::Server::start_source(source, None) {
        Ok(server) => Ok(Some(server)),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("Newport: X11 clipboard unavailable: {error}. Wayland and CLI clipboard remain available.");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
