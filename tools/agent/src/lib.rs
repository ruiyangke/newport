//! Linux clipboard and browser integration for Newport.
pub mod agent;
pub mod browser;
pub mod clipboard;
pub mod compose;
pub mod containers;
pub mod deployment;
mod diagnostics;
pub mod display;
mod docker;
pub mod environment;
pub mod git;
pub mod install;
pub use runtime_protocol::integration as integration_control;
mod legacy_install;
mod local_socket;
pub mod lsp;
pub mod monitor;
mod native;
mod owned_directory;
mod paths;
pub mod ports;
pub mod routing;
pub mod runtime;
pub mod runtime_protocol;
mod serialization;
pub mod services;
mod session;
mod snapshot;
mod transport;
pub mod wire;
mod wl_protocol;
pub mod wl_server;
mod wl_transfer;
mod worker_session;
mod x_endpoint;
mod x_protocol;
pub mod x_server;

mod clipboard_source;
mod clipboard_wire;
mod demand;

mod shell_setup;

mod file_watch;

pub mod gui_launch;
mod gui_notice;
pub mod gui_protocol;
