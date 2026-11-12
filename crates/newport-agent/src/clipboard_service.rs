//! Runtime-owned clipboard shared by admitted workers. Native publication and
//! host export require separate assignments from runtime control. This service
//! creates no display or host connection and never infers export permission.
mod cancellation;
mod client;
mod deadline;
mod server;
mod state;

pub use cancellation::{PreparationCancellation, ReadCancellation};
pub(crate) use client::Producer;
pub use client::{CopySession, NativePublisher, PreparedClipboard, Read, Reader};
pub use newport_protocol::clipboard::{
    Access, Chunk, CopyLease, ExportOffer, NativeRequest, Offer, Origin, ProviderLease,
    ProviderPoll, Route,
};
pub(crate) use server::Authority;
pub(crate) use server::Service;

#[cfg(test)]
mod tests;
