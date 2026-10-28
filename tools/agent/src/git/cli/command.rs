//! Bounded, noninteractive Git processes. Arguments never pass through a shell.
use super::{Error, failure};
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 32 * 1024 * 1024;
/// How long one Git command may run, network transfers included. The desktop
/// waits a little longer, so a timeout reaches the user as this command's error.
pub(crate) const COMMAND_LIMIT: Duration = Duration::from_secs(60);
/// After the limit, Git gets this long to remove its lock files before the kill.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(2);
fn drain(mut input: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut overflow = false;
    let mut buffer = [0; 8192];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return Ok((bytes, overflow));
        }
        let keep = count.min(limit - bytes.len());
        bytes.extend_from_slice(&buffer[..keep]);
        overflow |= keep < count;
    }
}

/// Asks the command's process group to stop, so Git can remove its lock files,
/// and waits up to [`STOP_GRACE`]; the caller kills whatever is left.
fn stop(child: &mut Child) {
    // SAFETY: kill takes no pointers; the group is the one this command leads.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGTERM);
    }
    let asked = Instant::now();
    while asked.elapsed() < STOP_GRACE {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn run(root: &Path, args: &[&str]) -> Result<Option<Vec<u8>>, Error> {
    run_input(root, args, Vec::new())
}
pub(super) fn run_input(
    root: &Path,
    args: &[&str],
    input: Vec<u8>,
) -> Result<Option<Vec<u8>>, Error> {
    run_os(
        root,
        &args
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>(),
        input,
    )
}
pub(super) fn run_os(
    root: &Path,
    args: &[std::ffi::OsString],
    input: Vec<u8>,
) -> Result<Option<Vec<u8>>, Error> {
    let output = execute(root, args, input)?;
    Ok(
        (output.success || (output.code == Some(1) && args.iter().any(|a| a == "--no-index")))
            .then_some(output.stdout),
    )
}
/// Like `run_os`, with Git's last error line when it fails.
pub(super) fn run_os_reason(
    root: &Path,
    args: &[std::ffi::OsString],
    input: Vec<u8>,
) -> Result<Result<Vec<u8>, String>, Error> {
    let output = execute(root, args, input)?;
    if output.success {
        return Ok(Ok(output.stdout));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default();
    let reason = reason
        .strip_prefix("fatal: ")
        .or_else(|| reason.strip_prefix("error: "))
        .unwrap_or(reason);
    Ok(Err(reason.chars().take(300).collect()))
}
struct Completed {
    success: bool,
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
pub(super) fn write(root: &Path, args: &[std::ffi::OsString], input: Vec<u8>) -> Result<(), Error> {
    write_inner(root, args, input, None)
}

/// Supply an editor response to Git's sequencer without changing its state
/// files or bypassing its author, hook and multi-commit continuation behavior.
/// Consume it once so a later commit in the same rebase retains its message.
pub(super) fn write_with_message(
    root: &Path,
    args: &[std::ffi::OsString],
    message: &str,
) -> Result<(), Error> {
    let mut file = tempfile::NamedTempFile::new_in(
        super::super::temporary_directory().map_err(super::io_error)?,
    )
    .map_err(super::io_error)?;
    file.write_all(message.as_bytes())
        .map_err(super::io_error)?;
    let result = write_inner(root, args, vec![], Some(file.path()));
    if result.is_ok() && file.path().exists() {
        return Err(Error::new(
            "OUTCOME_UNKNOWN",
            "Git completed without accepting the edited message. Inspect the commit before continuing.",
        ));
    }
    result
}

fn write_inner(
    root: &Path,
    args: &[std::ffi::OsString],
    input: Vec<u8>,
    message: Option<&Path>,
) -> Result<(), Error> {
    let output = execute_capture(root, args, input, None, message).map_err(|e| {
        if matches!(e.code.as_str(), "GIT_UNAVAILABLE" | "CHECKOUT_UNAVAILABLE") {
            e
        } else {
            Error::new(
                "OUTCOME_UNKNOWN",
                "The Git process ended without a confirmed result. Inspect the saved outcome.",
            )
        }
    })?;
    if output.success {
        return Ok(());
    }
    if output.code.is_none() {
        return Err(Error::new(
            "OUTCOME_UNKNOWN",
            "Git was terminated by a signal. Inspect the saved outcome before retrying.",
        ));
    }
    let error = String::from_utf8_lossy(&output.stderr);
    let (code, message) = if error.contains("Authentication failed")
        || error.contains("could not read Username")
        || error.contains("Permission denied (publickey)")
    {
        (
            "AUTH_REQUIRED",
            "Git authentication failed. Check the server's configured credential helper or SSH agent.",
        )
    } else if error.contains("Host key verification failed")
        || error.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || error.contains("SSL certificate problem")
        || error.contains("server certificate verification failed")
    {
        (
            "CERTIFICATE_REJECTED",
            "Git rejected the remote host key or TLS certificate. Verify the server identity and trust configuration.",
        )
    } else if args.first().is_some_and(|arg| arg == "push")
        && output
            .stdout
            .split(|b| *b == b'\n')
            .any(|line| line.starts_with(b"!\t"))
    {
        if output
            .stdout
            .windows(b"(stale info)".len())
            .any(|w| w == b"(stale info)")
        {
            (
                "STALE_REMOTE_REFERENCE",
                "The remote reference changed. Refresh before continuing.",
            )
        } else {
            (
                "PUSH_REJECTED",
                "The remote rejected the push. Refresh the remote references before continuing.",
            )
        }
    } else if error.contains("not possible to fast-forward")
        || error.contains("Not possible to fast-forward")
    {
        (
            "NON_FAST_FORWARD",
            "The branches have diverged. Choose merge or rebase instead of a fast-forward pull.",
        )
    } else if error.contains("would be overwritten") {
        (
            "LOCAL_CHANGES",
            "Local changes would be overwritten. Commit or stash them first.",
        )
    } else if error.contains("already exists") && error.starts_with("fatal: tag '") {
        (
            "TAG_EXISTS",
            "A tag with this name already exists. Choose another name; the existing tag was not replaced.",
        )
    } else if error.contains("already exists") && error.starts_with("fatal: a branch named '") {
        (
            "BRANCH_EXISTS",
            "A branch with this name already exists. Choose another name or switch to the existing branch.",
        )
    } else if error.contains("Author identity unknown")
        || error.contains("Committer identity unknown")
        || error.contains("unable to auto-detect email address")
        || error.contains("empty ident name")
    {
        (
            "IDENTITY_REQUIRED",
            "Git author identity is missing. Configure user.name and user.email in the repository on the server, then retry.",
        )
    } else if error.contains("nothing to commit") {
        (
            "NOTHING_TO_COMMIT",
            "There are no staged changes to commit.",
        )
    } else if error.contains(".lock") && error.contains("File exists") {
        (
            "REPOSITORY_BUSY",
            "Git found an existing lock. No lock was removed.",
        )
    } else {
        (
            "GIT_ERROR",
            "Git reported that the operation failed. Refresh the repository before retrying.",
        )
    };
    Err(Error::new(code, message))
}
fn execute(root: &Path, args: &[std::ffi::OsString], input: Vec<u8>) -> Result<Completed, Error> {
    execute_capture(root, args, input, None, None)
}
pub(super) fn window(
    root: &Path,
    args: &[&str],
    offset: usize,
    length: usize,
) -> Result<Vec<u8>, Error> {
    let args = args
        .iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    let output = execute_capture(root, &args, vec![], Some((offset, length)), None)?;
    if output.success {
        Ok(output.stdout)
    } else {
        Err(failure())
    }
}
fn drain_window(
    mut input: impl Read,
    offset: usize,
    length: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut result = Vec::with_capacity(length);
    let mut position = 0usize;
    let mut buffer = [0; 8192];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        let start = offset.saturating_sub(position).min(n);
        let end = offset
            .saturating_add(length)
            .saturating_sub(position)
            .min(n);
        if start < end {
            result.extend_from_slice(&buffer[start..end]);
        }
        position = position.saturating_add(n);
    }
    Ok((result, false))
}
fn execute_capture(
    root: &Path,
    args: &[std::ffi::OsString],
    input: Vec<u8>,
    window: Option<(usize, usize)>,
    message: Option<&Path>,
) -> Result<Completed, Error> {
    if !root.is_dir() {
        return Err(checkout_unavailable());
    }
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .env("GIT_PAGER", "cat")
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(path) = message {
        // Git invokes its editor via a shell. Only this constant script is
        // executable text; the private path and message are never interpolated.
        command.env("NEWPORT_GIT_CONTINUE_MESSAGE", path).env(
            "GIT_EDITOR",
            "sh -c 'if test -f \"$NEWPORT_GIT_CONTINUE_MESSAGE\"; then cat -- \"$NEWPORT_GIT_CONTINUE_MESSAGE\" > \"$1\" && rm -- \"$NEWPORT_GIT_CONTINUE_MESSAGE\"; fi' --",
        );
    } else {
        command.env_remove("NEWPORT_GIT_CONTINUE_MESSAGE");
    }
    // A repository is selected by the request, never by the agent's parent.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_PREFIX",
    ] {
        command.env_remove(key);
    }
    let mut child = command.spawn().map_err(|e| {
        // ENOENT can identify either the executable or current_dir. Check the
        // checkout before attributing a failed spawn to a missing Git install.
        if !root.is_dir() {
            checkout_unavailable()
        } else if e.kind() == std::io::ErrorKind::NotFound {
            Error::new(
                "GIT_UNAVAILABLE",
                "The CLI backend requires Git on the server.",
            )
        } else {
            failure()
        }
    })?;
    super::super::metrics::git_started();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || match window {
        Some((offset, length)) => drain_window(stdout, offset, length),
        None => drain(stdout, OUTPUT_LIMIT),
    });
    let err = std::thread::spawn(move || drain(stderr, 64 * 1024));
    let start = Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < COMMAND_LIMIT => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Ok(None) => {
                stop(&mut child);
                break Err(Error::new(
                    "TIMEOUT",
                    "The Git command exceeded its time limit.",
                ));
            }
            Err(_) => break Err(failure()),
        }
    };
    // Also close inherited pipes held by descendants before joining readers.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
    let output = out.join().map_err(|_| failure())?.map_err(|_| failure())?;
    let stderr = err.join().map_err(|_| failure())?.map_err(|_| failure())?;
    let _ = writer.join().map_err(|_| failure())?;
    super::super::command_log::record(
        args,
        start.elapsed(),
        result.as_ref().ok().and_then(|s| s.code()),
        (&output.0, output.1 || window.is_some()),
        (&stderr.0, stderr.1),
        result.is_err(),
    );
    let status = result?;
    if output.1 {
        return Err(Error::new(
            "LIMIT_EXCEEDED",
            "Git output exceeds the CLI backend's memory limit.",
        ));
    }
    Ok(Completed {
        success: status.success(),
        code: status.code(),
        stdout: output.0,
        stderr: stderr.0,
    })
}

fn checkout_unavailable() -> Error {
    Error::new(
        "CHECKOUT_UNAVAILABLE",
        "The checkout directory is missing or inaccessible. Locate the checkout or open the saved project.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_stopped_command_gets_to_clean_up_before_the_kill() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("cleaned");
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("trap 'touch \"$0\"; exit 0' TERM; while :; do sleep 0.05; done")
            .arg(&marker)
            .process_group(0)
            .spawn()
            .unwrap();
        // Let the shell install its trap.
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        stop(&mut child);
        assert!(
            started.elapsed() < STOP_GRACE,
            "the command exits on SIGTERM"
        );
        assert!(matches!(child.try_wait(), Ok(Some(_))));
        assert!(marker.exists(), "SIGTERM came before any kill");
    }
    #[test]
    fn unavailable_checkout_is_not_reported_as_missing_git_or_unknown_write() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-checkout");
        let file = directory.path().join("file");
        std::fs::write(&file, b"not a directory").unwrap();
        for unavailable in [&missing, &file] {
            assert_eq!(
                run(unavailable, &["status"]).unwrap_err().code,
                "CHECKOUT_UNAVAILABLE"
            );
            assert_eq!(
                write(unavailable, &["status".into()], vec![])
                    .unwrap_err()
                    .code,
                "CHECKOUT_UNAVAILABLE"
            );
        }
    }
    #[test]
    fn drain_caps_memory_but_consumes_the_entire_pipe() {
        let mut input = std::io::Cursor::new(vec![42; 100_000]);
        let (bytes, overflow) = drain(&mut input, 32).unwrap();
        assert_eq!(bytes, vec![42; 32]);
        assert!(overflow);
        assert_eq!(input.position(), 100_000);
    }
}
