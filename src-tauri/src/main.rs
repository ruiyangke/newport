#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod agent;
mod agent_keys;
mod app;
mod clipboard;
mod cockpit;
mod commands;
mod config;
mod connectivity;
mod credentials;
mod desktop;
mod files;
mod known_hosts;
mod manager;
mod metrics_db;
mod metrics_sampler;
mod model;
mod monitoring;
mod platform;
mod plugins;
mod ports;
mod preferences;
mod ssh;
mod system_events;
mod terminal;
mod updates;
mod vault;
fn main() {
    if let Err(error) = app::run() {
        eprintln!("Could not start Porthop: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod remote_tests;
