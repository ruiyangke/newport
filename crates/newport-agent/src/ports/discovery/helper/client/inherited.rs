use rustix::fs::{Mode, OFlags, PROC_SUPER_MAGIC, RawDir, fstatfs, open};
use std::{ffi::CStr, io, mem::MaybeUninit};

const MAX_ENTRIES: usize = 65_536;

pub(super) fn mark() -> io::Result<()> {
    // SAFETY: close_range takes only scalar arguments and marks this child's descriptors.
    if unsafe {
        libc::syscall(
            libc::SYS_close_range,
            3u32,
            u32::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        )
    } == 0
    {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if !without_close_range(&error) {
        return Err(error);
    }
    from_directory(c"/proc/self/fd")
}

/// Older kernels lack close_range or its flag, and older container seccomp
/// profiles deny syscalls they don't know with EPERM.
fn without_close_range(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS | libc::EINVAL | libc::EPERM)
    )
}

fn from_directory(path: &CStr) -> io::Result<()> {
    let directory = open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    if fstatfs(&directory)?.f_type != PROC_SUPER_MAGIC {
        return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
    }
    let mut buffer = [MaybeUninit::uninit(); 4096];
    let mut entries = RawDir::new(directory, &mut buffer);
    let mut visited = 0;
    while let Some(entry) = entries.next() {
        let entry = entry?;
        if visited == MAX_ENTRIES {
            return Err(io::Error::from_raw_os_error(libc::EMFILE));
        }
        visited += 1;
        let name = entry.file_name().to_bytes();
        if matches!(name, b"." | b"..") {
            continue;
        }
        let descriptor = name
            .iter()
            .try_fold(0i32, |number, byte| {
                byte.is_ascii_digit()
                    .then_some(i32::from(*byte) - i32::from(b'0'))
                    .and_then(|digit| number.checked_mul(10)?.checked_add(digit))
            })
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        if descriptor >= 3 {
            // SAFETY: fcntl takes scalar arguments; this single-threaded child owns its fd table.
            if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::Command,
    };

    #[test]
    fn proc_fallback_closes_sparse_high_descriptors_and_preserves_exec_error_reporting() {
        let sentinel = tempfile::NamedTempFile::new().unwrap();
        let sentinel_fd = sentinel.as_raw_fd();
        let mut limits = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            // SAFETY: `limits` is a valid out-pointer.
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) },
            0
        );
        // As high as the hard limit allows, capped to keep the test quick.
        let high = libc::rlim_t::min(65_536, limits.rlim_max - 1);
        assert!(high > 1024, "the hard descriptor limit is too low");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "test ! -e /proc/self/fd/$1", "discovery-fd-test"])
            .arg(high.to_string());
        // SAFETY: the hook calls only async-signal-safe functions.
        unsafe {
            command.pre_exec(move || {
                let raised = libc::rlimit {
                    rlim_cur: high + 1,
                    ..limits
                };
                if libc::setrlimit(libc::RLIMIT_NOFILE, &raised) < 0
                    || libc::dup2(sentinel_fd, high as i32) < 0
                    || libc::setrlimit(libc::RLIMIT_NOFILE, &limits) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                from_directory(c"/proc/self/fd")
            });
        }
        assert!(command.status().unwrap().success());
        let mut missing = Command::new("/newport-discovery-missing-program");
        // SAFETY: the hook calls only async-signal-safe functions.
        unsafe {
            missing.pre_exec(|| from_directory(c"/proc/self/fd"));
        }
        assert_eq!(missing.spawn().unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(
            // SAFETY: fcntl takes scalar arguments.
            unsafe { libc::fcntl(sentinel_fd, libc::F_GETFD) },
            libc::FD_CLOEXEC
        );
    }

    #[test]
    fn a_denied_or_missing_close_range_falls_back_to_the_proc_scan() {
        for errno in [libc::ENOSYS, libc::EINVAL, libc::EPERM] {
            assert!(without_close_range(&io::Error::from_raw_os_error(errno)));
        }
        assert!(!without_close_range(&io::Error::from_raw_os_error(
            libc::EBADF
        )));
    }

    #[test]
    fn proc_fallback_refuses_an_unverified_directory() {
        assert_eq!(
            from_directory(c"/tmp").unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            from_directory(c"/newport-discovery-missing-directory")
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
