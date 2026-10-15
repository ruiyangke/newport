//! Newport backend services, independent of the desktop UI framework.
//! The native application owns service lifetime and supplies platform adapters.
pub mod agent_approval;
pub mod agent_relay;
pub mod application;
pub mod metrics_db;
pub mod preferences;
pub mod profile;
pub mod query;
pub mod rpc;

use newport_domain::model;
pub mod agent_keys;
pub mod connectivity;
pub mod credentials;
pub mod known_hosts;
pub mod recovery;
#[path = "../../../tools/agent/src/serialization.rs"]
mod serialization;
pub mod ssh;
pub mod terminal;

pub mod agent;
pub mod agent_install;
pub mod agent_lifecycle;
mod agent_payload;
pub mod agent_runtime;
pub mod clipboard;
pub mod cockpit;
pub mod commands;
pub mod compose;
pub mod containers;
pub mod desktop_integration;
pub mod files;
pub mod git;
mod integration_service;
pub mod lifecycle_receipts;
mod lifecycle_task;
pub mod manager;
pub mod metrics_sampler;
pub mod monitor;
mod monitoring;
pub mod ports;
pub mod services;
pub mod workers;

pub mod integration_permissions;
pub mod server_session;
pub mod server_setup;
pub mod session_transport;

#[cfg(test)]
mod remote_tests;

mod server_queries;
