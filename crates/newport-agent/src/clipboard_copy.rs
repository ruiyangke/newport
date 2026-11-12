//! A program's copy on a terminal's clipboard display goes to the terminal as
//! an OSC 52 copy, as tmux sends one. The terminal applies it on the computer
//! it is shown on: a held shell's active computer, or a plain terminal's own.
use base64::Engine;
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, OnceLock},
    thread,
};

/// The terminal device a display's copies go to, once it is known.
pub(crate) type Terminal = Arc<OnceLock<PathBuf>>;

/// The copy waiting for the writer, and whether the display still sends any.
#[derive(Default)]
struct Pending {
    copy: Mutex<(Option<String>, bool)>,
    ready: Condvar,
}

/// Ends the writer once the display sends no more copies.
struct Sender(Arc<Pending>);
impl Drop for Sender {
    fn drop(&mut self) {
        self.0.copy.lock().unwrap_or_else(|p| p.into_inner()).1 = true;
        self.0.ready.notify_one();
    }
}

/// Writes copies to `terminal` in order, as output of its programs, from one
/// writer. A terminal whose output is stopped blocks that writer while the
/// display keeps serving; a newer copy replaces one still waiting, so the
/// latest wins and only one waits. Copies made before the terminal is known
/// go nowhere.
pub(crate) fn to_terminal(terminal: Terminal) -> crate::clipboard_source::Copy {
    let pending = Arc::new(Pending::default());
    let sender = Sender(pending.clone());
    let writer = OnceLock::new();
    Arc::new(move |text: String| {
        let Some(tty) = terminal.get().cloned() else {
            return;
        };
        sender.0.copy.lock().unwrap_or_else(|p| p.into_inner()).0 = Some(text);
        sender.0.ready.notify_one();
        writer.get_or_init(|| {
            let pending = pending.clone();
            thread::spawn(move || write_copies(&pending, &tty))
        });
    })
}

fn write_copies(pending: &Pending, tty: &std::path::Path) {
    loop {
        let text = {
            let mut state = pending.copy.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if let Some(text) = state.0.take() {
                    break text;
                }
                if state.1 {
                    return;
                }
                state = pending.ready.wait(state).unwrap_or_else(|p| p.into_inner());
            }
        };
        // Not as a controlling terminal: the writer may have none.
        if let Ok(mut terminal) = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open(tty)
        {
            let _ = terminal.write_all(&osc52(&text));
        }
    }
}

/// The sequence that copies `text` to the standard clipboard, written whole
/// so a program's output can't split it.
pub(crate) fn osc52(text: &str) -> Vec<u8> {
    let mut sequence = b"\x1b]52;c;".to_vec();
    sequence.extend(
        base64::engine::general_purpose::STANDARD
            .encode(text)
            .bytes(),
    );
    sequence.push(0x07);
    sequence
}

/// The terminal device on one of this process's standard streams.
pub(crate) fn own_terminal() -> Option<PathBuf> {
    (0..3).find_map(terminal_of)
}

fn terminal_of(fd: libc::c_int) -> Option<PathBuf> {
    // SAFETY: isatty and ttyname only read the descriptor's state; the name
    // is copied before another call can replace it.
    unsafe {
        if libc::isatty(fd) != 1 {
            return None;
        }
        let name = libc::ttyname(fd);
        (!name.is_null()).then(|| {
            PathBuf::from(
                <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(
                    std::ffi::CStr::from_ptr(name).to_bytes(),
                ),
            )
        })
    }
}

#[cfg(test)]
#[path = "clipboard_copy_tests.rs"]
mod display_tests;

#[cfg(test)]
mod tests {
    use super::*;

    use std::{io::Read, os::fd::FromRawFd};

    /// A pseudo-terminal: its controlling side, its device and the
    /// device's open descriptor.
    fn pty() -> (std::fs::File, PathBuf, std::fs::File) {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: openpty writes two descriptors this test then owns.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(opened, 0);
        // SAFETY: both descriptors came from openpty and are used only here.
        let (master, slave) = unsafe {
            (
                std::fs::File::from_raw_fd(master),
                std::fs::File::from_raw_fd(slave),
            )
        };
        let tty = terminal_of(std::os::fd::AsRawFd::as_raw_fd(&slave)).unwrap();
        (master, tty, slave)
    }

    #[test]
    fn copies_made_while_the_terminal_is_stopped_arrive_in_order_and_only_the_latest_waits() {
        let (mut master, tty, slave) = pty();
        let copy = to_terminal(Arc::new(OnceLock::from(tty)));
        // Larger than the terminal holds unread, so its writer blocks on it.
        let first = "a".repeat(1 << 20);
        copy(first.clone());
        let mut received = vec![0; 16];
        master.read_exact(&mut received).unwrap();
        // Copied while the writer is blocked: the second never goes.
        copy("second".into());
        copy("third".into());
        let expected = [osc52(&first), osc52("third")].concat();
        received.resize(expected.len(), 0);
        master.read_exact(&mut received[16..]).unwrap();
        assert!(
            received == expected,
            "the copies arrived out of order or the stale one went"
        );
        drop(slave);
    }

    #[test]
    fn a_copy_reaches_the_terminal_as_one_osc52_sequence_the_terminal_applies() {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: openpty writes two descriptors this test then owns.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(opened, 0);
        // SAFETY: both descriptors came from openpty and are used only here.
        let (mut master, slave) = unsafe {
            (
                std::fs::File::from_raw_fd(master),
                std::fs::File::from_raw_fd(slave),
            )
        };
        let tty = terminal_of(std::os::fd::AsRawFd::as_raw_fd(&slave)).unwrap();
        to_terminal(Arc::new(OnceLock::from(tty)))("copied on the server, 世界".into());
        let expected = osc52("copied on the server, 世界");
        let mut received = vec![0; expected.len()];
        master.read_exact(&mut received).unwrap();
        assert_eq!(received, expected);
        let mut engine = newport_terminal::Engine::new(80, 24).unwrap();
        assert_eq!(
            engine.advance(&received).unwrap().copy.as_deref(),
            Some("copied on the server, 世界")
        );
        drop(slave);
    }
}
