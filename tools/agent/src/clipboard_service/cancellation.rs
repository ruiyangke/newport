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
        let mut state = self.0.lock().unwrap();
        state.cancelled = true;
        if let Some(stream) = state.stream.upgrade() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
    pub(super) fn begin(&self) -> io::Result<()> {
        let mut state = self.0.lock().unwrap();
        if state.started || state.cancelled {
            return Err(io::Error::other(
                "Clipboard read cancelled or already started",
            ));
        }
        state.started = true;
        Ok(())
    }
    pub(super) fn attach(&self, stream: &UnixStream) -> io::Result<Arc<UnixStream>> {
        let mut state = self.0.lock().unwrap();
        if state.cancelled {
            return Err(io::Error::other("Clipboard read cancelled"));
        }
        let control = Arc::new(stream.try_clone()?);
        state.stream = Arc::downgrade(&control);
        Ok(control)
    }
}
