//! Reads of locally authored data. Imported promises are served by their
//! recorded remote source in the parent router, never by recursively reading
//! our own native provider. This prevents echo and a helper self-read deadlock.
use super::{Event, MAX_BYTES, MAX_WAIT, Shared, invalid, wire};
use objc2_app_kit::NSPasteboard;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

pub(super) struct Read {
    pub id: u64,
    pub revision: isize,
    pub format: String,
    pub active: AtomicBool,
    deadline: Instant,
}
impl Read {
    pub fn new(id: u64, revision: isize, format: String) -> Arc<Self> {
        Arc::new(Self {
            id,
            revision,
            format,
            active: AtomicBool::new(true),
            deadline: Instant::now() + MAX_WAIT,
        })
    }
    pub fn current(&self, shared: &Shared) -> bool {
        self.active.load(Ordering::Acquire)
            && shared.connected.load(Ordering::Acquire)
            && self.deadline > Instant::now()
            && self.revision == shared.observed.load(Ordering::Acquire)
    }
}

struct Buffer {
    data: Vec<u8>,
    budget: Arc<AtomicUsize>,
    charge: usize,
}
impl Buffer {
    // Reserve remaining output capacity before asking AppKit to allocate a
    // representation. Existing queued payloads retain their own charges.
    fn reserve(budget: &Arc<AtomicUsize>) -> Option<Self> {
        let used = budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < MAX_BYTES).then_some(MAX_BYTES)
            })
            .ok()?;
        Some(Self {
            data: Vec::new(),
            budget: budget.clone(),
            charge: MAX_BYTES - used,
        })
    }
    fn fill(&mut self, data: Vec<u8>) {
        assert!(data.len() <= self.charge);
        self.budget
            .fetch_sub(self.charge - data.len(), Ordering::AcqRel);
        self.charge = data.len();
        self.data = data;
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.charge, Ordering::AcqRel);
    }
}

pub(super) struct Transfer {
    pub read: Arc<Read>,
    bytes: Buffer,
    offset: usize,
}
impl Transfer {
    /// Dedicated native read worker. Preserve native ownership checks around the
    /// potentially synchronous foreign provider, and do not render unadvertised
    /// formats or our own imported promises.
    pub fn capture(board: &NSPasteboard, read: Arc<Read>, shared: &Shared) -> Option<Self> {
        if !read.current(shared) {
            return None;
        }
        let (revision, formats) = crate::clipboard::macos::offer_from(board, None)?;
        if revision != read.revision || !formats.contains(&read.format) || !read.current(shared) {
            return None;
        }
        let mut bytes = Buffer::reserve(&shared.local_budget)?;
        let data =
            crate::clipboard::macos::read_bounded(board, revision, &read.format, bytes.charge)?;
        if board.changeCount() != revision || !read.current(shared) {
            return None;
        }
        bytes.fill(data);
        Some(Self {
            read,
            bytes,
            offset: 0,
        })
    }

    /// One bounded chunk per turn so metadata, revocation and native export
    /// demands can pass between local payload chunks.
    pub fn send_chunk(
        &mut self,
        stream: &mut std::os::unix::net::UnixStream,
        shared: &Shared,
    ) -> io::Result<bool> {
        if !self.read.current(shared) {
            wire::send(stream, &Event::LocalFailed { id: self.read.id }, &[])?;
            return Ok(true);
        }
        let end = self
            .offset
            .saturating_add(wire::CHUNK)
            .min(self.bytes.data.len());
        if end < self.offset {
            return Err(invalid("Invalid local clipboard transfer"));
        }
        let eof = end == self.bytes.data.len();
        wire::send(
            stream,
            &Event::LocalData {
                id: self.read.id,
                revision: self.read.revision,
                offset: self.offset,
                eof,
            },
            &self.bytes.data[self.offset..end],
        )?;
        self.offset = end;
        Ok(eof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservations_cover_queued_bytes_and_release_unused_capacity() {
        let budget = Arc::new(AtomicUsize::new(0));
        let mut first = Buffer::reserve(&budget).unwrap();
        assert!(Buffer::reserve(&budget).is_none());
        first.fill(vec![1, 2, 3]);
        let second = Buffer::reserve(&budget).unwrap();
        assert_eq!(second.charge, MAX_BYTES - 3);
        drop(first);
        assert_eq!(budget.load(Ordering::Acquire), MAX_BYTES - 3);
        drop(second);
        assert_eq!(budget.load(Ordering::Acquire), 0);
    }
}
