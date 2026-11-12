//! Shared Newport contracts and validation, independent of transport and execution.
//! The `protobuf` feature provides bounded encoding at byte-stream boundaries.
extern crate self as newport_protocol;

pub mod browser;
pub mod clipboard;
pub mod common;
pub mod desktop;
pub mod display;
#[cfg(feature = "protobuf")]
pub mod encoding;
pub mod git;
pub mod integration;
pub mod monitor;
pub mod product;
pub mod remote_apps;
pub mod runtime;
pub mod terminal;
pub mod worker;
