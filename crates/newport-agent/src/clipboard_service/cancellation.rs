use std::{
    io,
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex, Weak},
};

#[derive(Default)]
struct State {
    started: bool,
    cancelled: bool,
    stream: Weak<UnixStream>,
}

/// Cancellation for one `Reader::read_with_cancellation` operation. Create it
/// before starting a task; any clone can interrupt a blocked read. It cannot be
/// reused for another read. Cancelling never revokes another consumer's offer.
#[derive(Clone, Default)]
pub struct ReadCancellation(Arc<Mutex<State>>);
impl ReadCancellation {
    pub fn cancel(&self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.cancelled = true;
        if let Some(stream) = state.stream.upgrade() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
    pub(super) fn begin(&self) -> io::Result<()> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.started || state.cancelled {
            return Err(io::Error::other(
                "Clipboard read cancelled or already started",
            ));
        }
        state.started = true;
        Ok(())
    }
    pub(super) fn attach(&self, stream: &UnixStream) -> io::Result<Arc<UnixStream>> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.cancelled {
            return Err(io::Error::other("Clipboard read cancelled"));
        }
        let control = Arc::new(stream.try_clone()?);
        state.stream = Arc::downgrade(&control);
        Ok(control)
    }
}

#[derive(Default)]
struct PreparationState {
    started: bool,
    cancelled: bool,
    committed: bool,
    streams: Vec<Weak<UnixStream>>,
}
/// Explicit cancellation of one clipboard preparation. It covers both
/// authenticated sockets until commit. Dropping a clone does not cancel.
#[derive(Clone, Default)]
pub struct PreparationCancellation(Arc<Mutex<PreparationState>>);
impl PreparationCancellation {
    pub fn cancel(&self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.committed {
            return;
        }
        state.cancelled = true;
        for stream in state
            .streams
            .drain(..)
            .filter_map(|stream| stream.upgrade())
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
    pub(super) fn begin(&self) -> io::Result<()> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.started || state.cancelled {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Clipboard preparation cancelled or already started",
            ));
        }
        state.started = true;
        Ok(())
    }
    pub(super) fn attach(&self, stream: &UnixStream) -> io::Result<Arc<UnixStream>> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.cancelled || state.committed {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Clipboard preparation cancelled or committed",
            ));
        }
        let control = Arc::new(stream.try_clone()?);
        state.streams.push(Arc::downgrade(&control));
        Ok(control)
    }
    pub(super) fn check(&self) -> io::Result<()> {
        if self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancelled
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Clipboard preparation cancelled",
            ));
        }
        Ok(())
    }
    pub(super) fn commit(&self, ready: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.cancelled || state.committed {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Clipboard preparation cancelled or committed",
            ));
        }
        ready()?;
        state.committed = true;
        state.streams.clear();
        Ok(())
    }
}
