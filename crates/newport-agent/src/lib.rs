//! Linux clipboard and browser integration for Newport.
/// The product release shared by Newport and its bundled agent.
pub const VERSION: &str = env!("NEWPORT_AGENT_VERSION");

pub mod agent;
pub mod browser;
pub mod cli;
pub mod clipboard;
pub mod clipboard_service;
pub mod compose;
pub mod composition;
pub mod containers;
pub mod deployment;
mod deployment_product;
pub mod detached;
mod diagnostics;
pub mod display;
mod docker;
pub mod environment;
pub mod git;
pub mod install;
mod legacy_install;
pub mod linger;
mod local_socket;
pub mod lsp;
pub mod monitor;
mod native;
mod owned_directory;
mod paths;
#[cfg(target_os = "linux")]
pub mod ports;
pub mod routing;
pub mod runtime;
pub mod services;
mod session;
mod snapshot;
pub mod terminal;
mod transport;
mod wl_protocol;
pub mod wl_server;
mod wl_transfer;
mod worker_session;
mod worker_stream;
mod worker_stream_io;
mod x_endpoint;
mod x_protocol;
pub mod x_server;

mod clipboard_channel;
mod clipboard_copy;
mod clipboard_export_relay;
mod clipboard_source;
mod demand;

mod shell_setup;

mod file_watch;

mod gui_attachment;
mod gui_notice;
#[cfg(test)]
mod version_source;
