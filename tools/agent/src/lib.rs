//! Linux clipboard and browser integration for Newport.
pub mod agent;
pub mod browser;
pub mod clipboard;
pub mod compose;
pub mod containers;
mod diagnostics;
pub mod display;
mod docker;
pub mod environment;
pub mod git;
pub mod install;
mod local_socket;
pub mod migration;
pub mod monitor;
mod native;
mod paths;
mod serialization;
pub mod services;
mod session;
mod snapshot;
mod transport;
pub mod wire;
mod wl_protocol;
pub mod wl_server;
mod wl_transfer;
mod x_protocol;
pub mod x_server;

mod clipboard_source;
mod clipboard_wire;
mod demand;

mod shell_setup;
