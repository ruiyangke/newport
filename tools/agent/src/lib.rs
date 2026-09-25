//! Linux clipboard and browser integration for Porthop.
pub mod agent;
pub mod browser;
pub mod clipboard;
mod diagnostics;
pub mod display;
pub mod environment;
pub mod install;
mod local_socket;
mod native;
mod paths;
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
