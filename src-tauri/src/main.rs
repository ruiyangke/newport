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
mod keychain;
mod known_hosts;
mod manager;
mod metrics_db;
mod metrics_sampler;
mod model;
mod monitoring;
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
