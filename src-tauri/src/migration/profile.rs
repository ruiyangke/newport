//! Resolve existing profiles and import their older plaintext formats.
use crate::{
    config::Store,
    model::{Config, Server, Tunnel},
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Keep the old override for scripts and isolated profiles during the rebrand.
pub fn data_directory_override() -> Option<PathBuf> {
    std::env::var_os("NEWPORT_DATA_DIR")
        .or_else(|| std::env::var_os(super::legacy::DATA_DIR_ENV))
        .map(PathBuf::from)
}

pub fn profile_directory(base: &Path) -> Result<PathBuf, String> {
    // Vault key accounts on both platforms hash the canonical directory path.
    // Adopt the entire legacy profile in place, including its lock, metrics,
    // and sync client identities. Never copy a live vault or SQLite database.
    let legacy = base.join(super::legacy::APP_NAME);
    match fs::metadata(&legacy) {
        Ok(metadata) if metadata.is_dir() => Ok(legacy),
        Ok(_) => Err("The existing Porthop profile path is not a directory.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(base.join("Newport")),
        Err(error) => Err(format!(
            "Cannot access the existing Porthop profile: {error}"
        )),
    }
}

pub fn import_profiles(store: &Store) -> Result<Config, String> {
    let target = store.directory.join("config.json");
    if target.exists() {
        let config = read_config(&target)?;
        store.save(&config)?;
        remove_plaintext(&store.directory)?;
        return Ok(config);
    }
    let legacy = if store.directory.join("tunnels.json").exists()
        || store.directory.join("servers.json").exists()
    {
        store.directory.clone()
    } else if data_directory_override().is_none() {
        store.directory.with_file_name("SSHTunnelBar")
    } else {
        store.directory.clone()
    };
    let mut config = Config::default();
    if legacy.join("servers.json").exists() {
        config.servers = serde_json::from_slice(&read(&legacy.join("servers.json"))?)
            .map_err(|e| format!("Cannot import servers.json: {e}"))?;
        if legacy.join("tunnels.json").exists() {
            config.tunnels = serde_json::from_slice(&read(&legacy.join("tunnels.json"))?)
                .map_err(|e| format!("Cannot import tunnels.json: {e}"))?;
        }
    } else if legacy.join("tunnels.json").exists() {
        let values: Vec<Value> = serde_json::from_slice(&read(&legacy.join("tunnels.json"))?)
            .map_err(|e| e.to_string())?;
        for mut value in values {
            let mut server: Server = serde_json::from_value(value.clone())
                .map_err(|e| format!("Cannot import legacy tunnel: {e}"))?;
            let id = if let Some(found) = config.servers.iter().find(|s| {
                s.ssh_host == server.ssh_host
                    && s.ssh_user == server.ssh_user
                    && s.ssh_port == server.ssh_port
                    && s.identity_file == server.identity_file
            }) {
                found.id
            } else {
                server.id = Uuid::new_v4();
                server.name = server.ssh_host.clone();
                let id = server.id;
                config.servers.push(server);
                id
            };
            value["serverId"] = serde_json::json!(id);
            config
                .tunnels
                .push(serde_json::from_value::<Tunnel>(value).map_err(|e| e.to_string())?);
        }
    }
    config.validate()?;
    store.save(&config)?;
    remove_plaintext(&store.directory)?;
    Ok(config)
}
pub fn remove_plaintext(directory: &Path) -> Result<(), String> {
    for name in ["config.json", "servers.json", "tunnels.json"] {
        match fs::remove_file(directory.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!(
                    "Profiles encrypted, but cannot remove old {name}: {e}"
                ))
            }
        }
    }
    Ok(())
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))
}
fn read_config(path: &Path) -> Result<Config, String> {
    let c: Config = serde_json::from_slice(&read(path)?)
        .map_err(|e| format!("Cannot read saved profiles; original file was preserved: {e}"))?;
    c.validate()?;
    Ok(c)
}
