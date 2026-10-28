//! Agent locations only; selection, consent and retries belong to the backend.
use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf};

/// Which SSH agent a server's sign-in uses.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    System,
    OnePassword,
}
impl Source {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("system") {
            "system" => Ok(Self::System),
            "onePassword" => Ok(Self::OnePassword),
            _ => Err("Unknown SSH agent".into()),
        }
    }
}

#[cfg(unix)]
pub fn source_path(source: Source) -> io::Result<PathBuf> {
    match source {
        Source::System => std::env::var_os("SSH_AUTH_SOCK")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SSH_AUTH_SOCK is not set")),
        Source::OnePassword => dirs::home_dir()
            .map(|home| home.join("Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Cannot find home directory")),
    }
}

#[cfg(windows)]
pub fn source_path(_source: Source) -> io::Result<PathBuf> {
    // 1Password implements OpenSSH's standard pipe. Explicit selection does
    // not imply that a second provider can own a different socket.
    Ok(PathBuf::from(r"\\.\pipe\openssh-ssh-agent"))
}

pub fn system_known_hosts() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/ssh/ssh_known_hosts"))
    }
    #[cfg(windows)]
    {
        std::env::var_os("ProgramData").map(|root| PathBuf::from(root).join("ssh/ssh_known_hosts"))
    }
}

#[cfg(unix)]
pub const SOURCES: &[(&str, &str)] = &[("system", "System agent"), ("onePassword", "1Password")];
#[cfg(windows)]
pub const SOURCES: &[(&str, &str)] = &[("system", "OpenSSH agent (including 1Password)")];
