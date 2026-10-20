//! Installing the Newport Remote agent on a server. This is its own process,
//! apart from Needle's setup: the agent installs into its own cache and needs
//! programs of its own on the server.
//!
//! Full builds carry the compiled agent for each server architecture (see
//! `build.rs`). Install sends the one for the server over Newport's connection,
//! where the agent's installer checks the upload against its digest, unpacks
//! it, tries it and publishes it. No source reaches the server and nothing is
//! built there.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// How much of an install's upload has been sent.
#[derive(Clone, Debug)]
pub(crate) struct Progress {
    sent: Arc<AtomicUsize>,
    total: usize,
}

impl Progress {
    #[cfg_attr(not(any(newport_remote_apps, test)), allow(dead_code))]
    fn new(total: usize) -> Self {
        Self {
            sent: Default::default(),
            total,
        }
    }

    /// Progress partway through an upload, for tests of the page.
    #[cfg(test)]
    pub(crate) fn at(total: usize, sent: usize) -> Self {
        let progress = Self::new(total);
        progress.sent.store(sent, Ordering::Relaxed);
        progress
    }

    /// From 0 to 1.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.;
        }
        (self.sent.load(Ordering::Relaxed) as f32 / self.total as f32).min(1.)
    }

    /// Everything was sent; the server is unpacking and trying the agent.
    pub fn uploaded(&self) -> bool {
        self.sent.load(Ordering::Relaxed) >= self.total
    }
}

/// Whether this build can install the agent.
pub(crate) fn available() -> bool {
    cfg!(newport_remote_apps)
}

/// The compiled agent for a server whose `uname -m` is `architecture`.
#[cfg(newport_remote_apps)]
fn archive(architecture: &str) -> Option<&'static [u8]> {
    match architecture {
        "x86_64" => Some(include_bytes!(env!("NEWPORT_REMOTE_AGENT_X86_64"))),
        "aarch64" => Some(include_bytes!(env!("NEWPORT_REMOTE_AGENT_AARCH64"))),
        _ => None,
    }
}

/// The release this build installs on such a server: its archive's digest,
/// which names the release directory there.
#[cfg(newport_remote_apps)]
pub(crate) fn revision(architecture: &str) -> Option<&'static str> {
    use std::sync::OnceLock;
    static X86_64: OnceLock<String> = OnceLock::new();
    static AARCH64: OnceLock<String> = OnceLock::new();
    let cell = match architecture {
        "x86_64" => &X86_64,
        "aarch64" => &AARCH64,
        _ => return None,
    };
    let data = archive(architecture)?;
    Some(cell.get_or_init(|| newport_remote_app::deploy::revision(data)))
}

#[cfg(not(newport_remote_apps))]
pub(crate) fn revision(_: &str) -> Option<&'static str> {
    None
}

/// Start installing on a server whose `uname -m` is `architecture`. The
/// returned progress tracks the upload until the install ends.
#[cfg(newport_remote_apps)]
pub(crate) fn start(
    server: &str,
    architecture: &str,
) -> Result<(Progress, async_channel::Receiver<Result<String, String>>), String> {
    if architecture.is_empty() {
        return Err(
            "Newport couldn’t tell which processor the server has. Check again, then install."
                .into(),
        );
    }
    let data = archive(architecture).ok_or_else(|| {
        format!("Newport Remote doesn’t have an agent for {architecture} servers yet.")
    })?;
    let revision = revision(architecture).expect("revision of a bundled archive");
    let progress = Progress::new(data.len());
    let (sender, receiver) = async_channel::bounded(1);
    let (server, sent) = (server.to_owned(), progress.clone());
    // Blocking I/O for up to minutes: keep it off GPUI's executors.
    std::thread::spawn(move || {
        let _ = sender.send_blocking(install(&server, data, revision, &sent));
    });
    Ok((progress, receiver))
}

#[cfg(not(newport_remote_apps))]
pub(crate) fn start(
    _: &str,
    _: &str,
) -> Result<(Progress, async_channel::Receiver<Result<String, String>>), String> {
    Err("Installing Newport Remote isn’t part of this build.".into())
}

/// Longer than any healthy install: a stalled one is stopped.
#[cfg(newport_remote_apps)]
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(600);

/// Upload `data` to the server's installer and return the installed agent.
#[cfg(newport_remote_apps)]
fn install(
    server: &str,
    data: &[u8],
    revision: &str,
    progress: &Progress,
) -> Result<String, String> {
    use newport_remote_app::deploy;
    use std::io::{Read, Write};
    // Wayland needs the fewest programs; X11 apps report Xvfb separately.
    let options = deploy::Options {
        backend: deploy::Backend::Wayland,
        cache_directory: None,
    };
    // The installer reads exactly the archive, so the input stays open until
    // it ends: if Newport goes away, the backend stops it.
    let command = deploy::remote_command(revision, data.len(), &options)
        .map_err(|error| error.to_string())?;
    let (reader, mut writer, close, outcome) =
        crate::remote_apps_sessions::open_stream(server, &command)?;
    let (finished, deadline) = std::sync::mpsc::channel::<()>();
    let stop = close.clone();
    let watchdog = std::thread::spawn(move || {
        if deadline.recv_timeout(DEADLINE) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            stop();
        }
    });
    // Read while uploading: an installer that refuses early stops reading.
    let output = std::thread::spawn(move || {
        let mut output = Vec::new();
        reader.take(8193).read_to_end(&mut output).map(|_| output)
    });
    let mut upload = Ok(());
    for chunk in data.chunks(64 * 1024) {
        if let Err(error) = writer.write_all(chunk) {
            upload = Err(error);
            break;
        }
        progress.sent.fetch_add(chunk.len(), Ordering::Relaxed);
    }
    let output = output.join();
    drop(writer);
    drop(finished);
    let _ = watchdog.join();
    close();
    let failure = outcome
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .and_then(|outcome| {
            let failure = outcome.failure()?;
            Some(reason(&outcome.stderr).unwrap_or(failure))
        });
    if let Some(failure) = failure {
        return Err(failure);
    }
    upload.map_err(|error| format!("The upload stopped: {error}"))?;
    let output = output
        .map_err(|_| "The install stopped unexpectedly.".to_owned())?
        .map_err(|error| format!("The connection to the server ended: {error}"))?;
    if output.len() > 8192 {
        return Err("The server’s installer replied with too much output.".into());
    }
    deploy::installed(&output, revision)
        .map(|installation| installation.program)
        .map_err(|_| {
            "The server’s installer ended without confirming the install. Check again in a moment."
                .to_owned()
        })
}

/// The installer's own explanation of a failure: its last message, unless
/// that only announced the install.
#[cfg_attr(not(newport_remote_apps), allow(dead_code))]
fn reason(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("Newport Remote: "))
        .filter(|line| !line.starts_with("installing agent"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    #[test]
    fn failures_use_the_installers_explanation() {
        assert_eq!(
            super::reason("Newport Remote: Missing programs on the server: gzip dbus-daemon. Install them, then retry.\n").as_deref(),
            Some("Missing programs on the server: gzip dbus-daemon. Install them, then retry.")
        );
        assert_eq!(
            super::reason("Newport Remote: installing agent in /c/g\ntar: unexpected end\n"),
            None
        );
        assert_eq!(super::reason("plain error"), None);
    }

    #[test]
    fn progress_is_a_bounded_fraction() {
        let progress = super::Progress::new(200);
        assert_eq!(progress.fraction(), 0.);
        assert!(!progress.uploaded());
        progress
            .sent
            .fetch_add(100, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(progress.fraction(), 0.5);
        progress
            .sent
            .fetch_add(150, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(progress.fraction(), 1.);
        assert!(progress.uploaded());
        assert_eq!(super::Progress::new(0).fraction(), 0.);
    }

    #[test]
    fn free_builds_only_explain() {
        if !super::available() {
            assert!(super::revision("x86_64").is_none());
            assert!(super::start("server", "x86_64").is_err());
        }
    }
}
