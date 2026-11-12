use std::{
    cell::RefCell,
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, BorrowedFd},
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(10);

pub(super) struct Nonblocking<'a> {
    descriptor: BorrowedFd<'a>,
    flags: libc::c_int,
}

impl<'a> Nonblocking<'a> {
    pub fn pipe(descriptor: BorrowedFd<'a>) -> io::Result<Self> {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstat initializes the live output object on success.
        if unsafe { libc::fstat(descriptor.as_raw_fd(), metadata.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let kind = unsafe { metadata.assume_init() }.st_mode & libc::S_IFMT;
        if !matches!(kind, libc::S_IFIFO | libc::S_IFSOCK) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Discovery helper requires pipes",
            ));
        }
        // SAFETY: fcntl operates on a borrowed live descriptor with no pointers.
        let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe {
                libc::fcntl(
                    descriptor.as_raw_fd(),
                    libc::F_SETFL,
                    flags | libc::O_NONBLOCK,
                )
            } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { descriptor, flags })
    }
}

impl Drop for Nonblocking<'_> {
    fn drop(&mut self) {
        // SAFETY: the borrow keeps the descriptor alive until flags are restored.
        unsafe {
            libc::fcntl(self.descriptor.as_raw_fd(), libc::F_SETFL, self.flags);
        }
    }
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "Discovery helper time limit reached",
            )
        })
}

pub(super) struct Input<'a> {
    pub file: &'a File,
    pub deadline: Instant,
}

impl Read for Input<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            remaining(self.deadline)?;
            match self.file.read(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL.min(remaining(self.deadline)?));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                result => return result,
            }
        }
    }
}

pub(super) struct Control<'a> {
    input: &'a File,
    deadline: Instant,
    failure: RefCell<Option<io::Error>>,
}

impl<'a> Control<'a> {
    pub fn new(input: &'a File, deadline: Instant) -> Self {
        Self {
            input,
            deadline,
            failure: RefCell::new(None),
        }
    }

    fn probe(&self) -> io::Result<()> {
        remaining(self.deadline)?;
        let mut input = self.input;
        match input.read(&mut [0]) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Discovery helper caller disconnected",
            )),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected input after discovery request",
            )),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub fn cancelled(&self) -> bool {
        let mut failure = self.failure.borrow_mut();
        if failure.is_none() {
            *failure = self.probe().err();
        }
        failure.is_some()
    }

    pub fn finish<T>(&self, result: io::Result<T>) -> io::Result<T> {
        self.cancelled();
        match self.failure.borrow_mut().take() {
            Some(error) => Err(error),
            None => result,
        }
    }

    /// The cause already seen, or `result`; reads nothing more from the pipe.
    pub fn settle<T>(&self, result: io::Result<T>) -> io::Result<T> {
        match self.failure.borrow_mut().take() {
            Some(error) => Err(error),
            None => result,
        }
    }

    fn check(&self) -> io::Result<()> {
        if self.cancelled() {
            // write_all retries Interrupted; finish returns the retained cause.
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Discovery helper cancelled",
            ))
        } else {
            Ok(())
        }
    }
}

pub(super) struct Output<'a, 'b> {
    pub file: &'a File,
    pub control: &'a Control<'b>,
}

impl Write for Output<'_, '_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            self.control.check()?;
            match self.file.write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL.min(remaining(self.control.deadline)?));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                result => return result,
            }
        }
    }

    // Not a cancellation point: once the last byte is written, the caller may
    // already be sending its next request.
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
