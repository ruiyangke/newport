//! A terminal's reaper: the shell's parent, between the holder and the shell.
//! The holder keeps the PTY and its clients and is no one else's parent. The
//! reaper adopts what the shell's programs orphan, so a cleanup that kills
//! "the parent" of a process it started reaches the reaper, which ignores the
//! signals such a cleanup sends, and never the holder. A reaper killed anyway
//! leaves the terminal running: the holder takes the shell back.
//!
//! Under a user manager it first moves into a scope of its own, as tmux does
//! each pane, so a memory killer that takes the shell's scope leaves the
//! holder to report how it ended. Without one it does the same work in the
//! holder's process tree.
use std::{ffi::CString, io, os::fd::RawFd};

/// The PTY's terminal side, and the pipe that takes the shell's pid and
/// then its wait status to the holder, where the holder hands them over.
pub(super) const TERMINAL_FD: RawFd = 3;
pub(super) const STATUS_FD: RawFd = 4;

/// What a mistaken kill sends, and the job control and alarm signals a
/// process that never reads a terminal has no use for. The shell gets them
/// back at their defaults.
const IGNORED: [libc::c_int; 11] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGUSR1,
    libc::SIGUSR2,
    libc::SIGALRM,
    libc::SIGPIPE,
    libc::SIGTSTP,
    libc::SIGTTIN,
    libc::SIGTTOU,
];

/// `terminal reaper <id> <command>`, with the PTY and status pipe at their
/// descriptors and the shell's environment as its own.
pub(crate) fn run(args: &[String]) -> io::Result<()> {
    let [id, command] = args else {
        return Err(io::Error::other("Invalid terminal reaper arguments."));
    };
    let id = super::valid_id(id)?;
    let shell =
        CString::new(command.as_str()).map_err(|_| io::Error::other("Invalid shell command."))?;
    for signal in IGNORED {
        // SAFETY: signal takes scalar arguments.
        unsafe { libc::signal(signal, libc::SIG_IGN) };
    }
    // A group of its own: a kill aimed at the group of "the parent" must not
    // reach the holder, whose group the reaper would otherwise share.
    // SAFETY: setpgid takes scalar arguments.
    unsafe { libc::setpgid(0, 0) };
    become_subreaper();
    scope::enter(&id);
    let pid = spawn(&shell);
    // SAFETY: the reaper has no other use for the terminal side; the PTY
    // reports the shell's end only once nothing has it open.
    unsafe { libc::close(TERMINAL_FD) };
    match pid {
        Ok(pid) => {
            send(pid);
            reap(pid);
        }
        Err(_) => send(-1),
    }
    crate::linger::reconcile();
    Ok(())
}

/// Orphans of the shell's programs become the reaper's to collect.
fn become_subreaper() {
    #[cfg(target_os = "linux")]
    // SAFETY: prctl with this option takes scalar arguments.
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }
}

/// Every signal at its default and none blocked, as a new program expects.
/// Only async-signal-safe calls, for a child between fork and exec.
pub(super) fn default_signals() {
    // SAFETY: signal and sigprocmask take scalar arguments and a valid set;
    // the numbers no system has are refused.
    unsafe {
        for signal in 1..65 {
            libc::signal(signal, libc::SIG_DFL);
        }
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
    }
}

/// Starts `sh -c command` as the session leader of the PTY.
fn spawn(command: &CString) -> io::Result<libc::pid_t> {
    let shell = c"/bin/sh";
    let argv = [
        c"sh".as_ptr(),
        c"-c".as_ptr(),
        command.as_ptr(),
        std::ptr::null(),
    ];
    // SAFETY: the reaper runs no other thread, and the child only calls
    // async-signal-safe functions on memory prepared before the fork.
    match unsafe { libc::fork() } {
        -1 => Err(io::Error::last_os_error()),
        // SAFETY: as above; the descriptors are the ones the holder handed over.
        0 => unsafe {
            default_signals();
            libc::setsid();
            libc::ioctl(TERMINAL_FD, libc::TIOCSCTTY as _, 0);
            for fd in 0..3 {
                libc::dup2(TERMINAL_FD, fd);
            }
            libc::close(TERMINAL_FD);
            libc::close(STATUS_FD);
            libc::execv(shell.as_ptr(), argv.as_ptr());
            libc::_exit(127)
        },
        pid => Ok(pid),
    }
}

/// Writes one value to the holder; a holder that is gone reads nothing.
fn send(value: libc::c_int) {
    let bytes = value.to_ne_bytes();
    // SAFETY: the pointer and length describe `bytes`.
    unsafe { libc::write(STATUS_FD, bytes.as_ptr().cast(), bytes.len()) };
}

/// Collects every child until none is left, telling the holder how the shell
/// ended. Orphans that outlive the shell keep the reaper, not the holder.
fn reap(shell: libc::pid_t) {
    loop {
        let mut status = 0;
        // SAFETY: `status` is a valid out-pointer.
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
        if pid == -1 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if pid == shell {
            send(status);
            // SAFETY: nothing else is sent; the holder sees the pipe close.
            unsafe { libc::close(STATUS_FD) };
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod scope {
    //! The shell's own scope, beside its holder's.
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    /// How long the move may take before the shell starts where the reaper is.
    const SCOPE_WAIT: Duration = Duration::from_secs(2);

    /// The scope a held terminal's shell runs in. The holder's scope pattern
    /// covers it, so lingering lasts while either runs.
    pub(crate) fn name(id: &str) -> String {
        format!("newport-terminal-{id}-shell.scope")
    }

    /// The holder's scope and its slice, when `cgroup`, the reaper's own
    /// `/proc/self/cgroup` before it moves, puts it in the holder's scope.
    pub(super) fn holder(cgroup: &str, id: &str) -> Option<(String, String)> {
        let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
        let mut parts = path.rsplit('/');
        let scope = parts.next()?;
        let slice = parts.next()?;
        (scope == format!("newport-terminal-{id}.scope") && slice.ends_with(".slice"))
            .then(|| (scope.to_owned(), slice.to_owned()))
    }

    /// `busctl` arguments that start the shell's scope with `pid` in it. One
    /// killed process must not stop the rest, as systemd's default for a
    /// scope does; shells ignore SIGTERM, so a stop also hangs them up. It
    /// stops before the holder's, so the holder can report how it ended.
    pub(super) fn arguments(
        id: &str,
        (holder, slice): (&str, &str),
        pid: u32,
        oom_policy: bool,
    ) -> Vec<String> {
        let mut properties: Vec<Vec<String>> = vec![
            vec![
                "Description".into(),
                "s".into(),
                "Newport terminal shell".into(),
            ],
            vec!["PIDs".into(), "au".into(), "1".into(), pid.to_string()],
            vec!["Slice".into(), "s".into(), slice.into()],
            vec!["Before".into(), "as".into(), "1".into(), holder.into()],
            vec!["SendSIGHUP".into(), "b".into(), "true".into()],
            vec![
                "CollectMode".into(),
                "s".into(),
                "inactive-or-failed".into(),
            ],
        ];
        if oom_policy {
            properties.push(vec!["OOMPolicy".into(), "s".into(), "continue".into()]);
        }
        let mut arguments: Vec<String> = [
            "--user",
            "--quiet",
            "--timeout=2",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
        ]
        .map(String::from)
        .into();
        arguments.push(name(id));
        arguments.push("fail".into());
        arguments.push(properties.len().to_string());
        arguments.extend(properties.into_iter().flatten());
        arguments.push("0".into());
        arguments
    }

    fn own_cgroup() -> String {
        std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default()
    }

    /// Moves the reaper into the shell's scope before it starts the shell, when
    /// the holder runs in a scope of its own. Without a user manager, or when
    /// it refuses, the shell runs where the reaper is.
    pub(super) fn enter(id: &str) {
        let Some((holder, slice)) = holder(&own_cgroup(), id) else {
            return;
        };
        let deadline = Instant::now() + SCOPE_WAIT;
        // Older systemd refuses OOMPolicy on a scope.
        for oom_policy in [true, false] {
            let mut busctl = Command::new("busctl");
            busctl
                .args(arguments(
                    id,
                    (&holder, &slice),
                    std::process::id(),
                    oom_policy,
                ))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            match crate::detached::bounded::status(&mut busctl, deadline) {
                Ok(status) if status.success() => {
                    // The move is queued as a job; the shell must start after it.
                    let moved = format!("/{}", name(id));
                    while !own_cgroup().lines().any(|line| line.ends_with(&moved))
                        && Instant::now() < deadline
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    return;
                }
                Ok(status) if status.code().is_some() => {}
                _ => return,
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod scope {
    /// Only Linux has user managers.
    pub(super) fn enter(_id: &str) {}
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::scope;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn a_reaper_in_its_holders_scope_finds_the_scope_and_its_slice() {
        let cgroup = format!(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/newport-terminal-{ID}.scope\n"
        );
        assert_eq!(
            scope::holder(&cgroup, ID),
            Some((format!("newport-terminal-{ID}.scope"), "app.slice".into()))
        );
    }

    #[test]
    fn a_reaper_outside_its_holders_scope_moves_nowhere() {
        for cgroup in [
            "0::/\n".to_owned(),
            "0::/init.scope\n".to_owned(),
            "0::/user.slice/user-1000.slice/session-3.scope\n".to_owned(),
            format!("0::/app.slice/newport-terminal-{ID}-shell.scope\n"),
            format!("12:pids:/app.slice/newport-terminal-{ID}.scope\n"),
            "0::/app.slice/newport-terminal-ffffffffffffffffffffffffffffffff.scope\n".to_owned(),
        ] {
            assert_eq!(scope::holder(&cgroup, ID), None, "{cgroup}");
        }
    }

    #[test]
    fn the_shells_scope_keeps_running_when_one_process_is_killed_and_stops_first() {
        let holder = format!("newport-terminal-{ID}.scope");
        let arguments = scope::arguments(ID, (&holder, "app.slice"), 42, true);
        let text = arguments.join(" ");
        assert!(text.starts_with("--user --quiet --timeout=2 call org.freedesktop.systemd1 "));
        assert!(text.contains(&format!(
            "StartTransientUnit ssa(sv)a(sa(sv)) newport-terminal-{ID}-shell.scope fail 7 "
        )));
        for property in [
            "PIDs au 1 42".to_owned(),
            "Slice s app.slice".to_owned(),
            format!("Before as 1 {holder}"),
            "SendSIGHUP b true".to_owned(),
            "CollectMode s inactive-or-failed".to_owned(),
            "OOMPolicy s continue".to_owned(),
        ] {
            assert!(text.contains(&property), "{property} missing from {text}");
        }
        assert!(text.ends_with(" 0"));
        let older = scope::arguments(ID, (&holder, "app.slice"), 42, false).join(" ");
        assert!(older.contains(" fail 6 ") && !older.contains("OOMPolicy"));
    }
}
