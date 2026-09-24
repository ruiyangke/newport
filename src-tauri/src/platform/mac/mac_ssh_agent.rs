use russh::keys::agent::client::AgentClient;
use tokio::net::UnixStream;

pub async fn connect(source: Option<&str>) -> Result<AgentClient<UnixStream>, String> {
    match source.unwrap_or("system") {
        "system" => AgentClient::connect_env().await.map_err(|e| e.to_string()),
        "onePassword" => {
            let path = dirs::home_dir()
                .ok_or("Cannot find home directory")?
                .join("Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock");
            AgentClient::connect_uds(path)
                .await
                .map_err(|e| e.to_string())
        }
        _ => Err("Unknown SSH agent".into()),
    }
}

#[cfg(not(test))]
pub fn system_known_hosts() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from("/etc/ssh/ssh_known_hosts"))
}

pub const SOURCES: &[(&str, &str)] = &[("system", "System agent"), ("onePassword", "1Password")];
