use russh::keys::agent::client::AgentClient;
use tokio::net::windows::named_pipe::NamedPipeClient;

pub async fn connect(source: Option<&str>) -> Result<AgentClient<NamedPipeClient>, String> {
    // 1Password exposes the standard OpenSSH pipe on Windows. Only one provider
    // can own it; explicit provider selection must not imply a separate socket.
    match source.unwrap_or("system") {
        "system" | "onePassword" => AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
            .await
            .map_err(|error| error.to_string()),
        _ => Err("Unknown SSH agent".into()),
    }
}

#[cfg(not(test))]
pub fn system_known_hosts() -> Option<std::path::PathBuf> {
    std::env::var_os("ProgramData")
        .map(|root| std::path::PathBuf::from(root).join("ssh/ssh_known_hosts"))
}

pub const SOURCES: &[(&str, &str)] = &[("system", "OpenSSH agent (including 1Password)")];
