//! The settings OpenSSH would use for a host, as `ssh -G` reports them. Only
//! OpenSSH applies `Match` (including `exec`), `Include` and its defaults
//! exactly as `ssh` does; Newport still connects with its own client.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

/// What the SSH config sets for a host. A field is set only when it differs
/// from what OpenSSH uses with no configuration at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    /// As written, with `%h`, `%p`, `%r` and `%n` left for the dialer.
    pub proxy_command: Option<String>,
    pub proxy_jump: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No `ssh` on this computer.
    Unavailable,
    /// Not a host `ssh` could be asked about.
    InvalidHost,
    /// `ssh -G` failed or took too long; its message, when it gave one.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("OpenSSH isn’t installed on this computer."),
            Self::InvalidHost => f.write_str("Enter a host name without spaces."),
            Self::Failed(message) => write!(f, "Couldn’t read the SSH config: {message}"),
        }
    }
}

const TIMEOUT: Duration = Duration::from_secs(5);

/// How `ssh -G` is run. Tests name the binary and the config file; Newport
/// itself uses the installed `ssh` and the person's own configuration.
#[derive(Clone, Debug)]
pub struct Options {
    pub ssh: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            ssh: find_ssh(),
            config: None,
            timeout: TIMEOUT,
        }
    }
}

pub async fn resolve(host: &str) -> Result<Resolved, Error> {
    resolve_with(host, &Options::default()).await
}

pub async fn resolve_with(host: &str, options: &Options) -> Result<Resolved, Error> {
    if !valid_host(host) {
        return Err(Error::InvalidHost);
    }
    let ssh = options.ssh.as_deref().ok_or(Error::Unavailable)?;
    let (configured, defaults) = tokio::try_join!(
        run(ssh, options.config.as_deref(), host, options.timeout),
        run(ssh, Some(Path::new(NO_CONFIG)), host, options.timeout),
    )?;
    Ok(difference(
        &parse(&configured),
        &parse(&defaults),
        dirs::home_dir().as_deref(),
    ))
}

/// A host `ssh` can never read as an option or as several arguments.
fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && !host.starts_with('-')
        && !host.chars().any(|c| c.is_whitespace() || c.is_control())
}

#[cfg(unix)]
const NO_CONFIG: &str = "/dev/null";
#[cfg(windows)]
const NO_CONFIG: &str = "NUL";

fn find_ssh() -> Option<PathBuf> {
    #[cfg(unix)]
    let (known, name) = (vec![PathBuf::from("/usr/bin/ssh")], "ssh");
    #[cfg(windows)]
    let (known, name) = (
        std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join(r"System32\OpenSSH\ssh.exe"))
            .into_iter()
            .collect::<Vec<_>>(),
        "ssh.exe",
    );
    known
        .into_iter()
        .chain(
            std::env::var_os("PATH")
                .iter()
                .flat_map(std::env::split_paths)
                .map(|directory| directory.join(name)),
        )
        .find(|path| path.is_file())
}

async fn run(
    ssh: &Path,
    config: Option<&Path>,
    host: &str,
    timeout: Duration,
) -> Result<String, Error> {
    let mut command = tokio::process::Command::new(ssh);
    if let Some(config) = config {
        command.arg("-F").arg(config);
    }
    command
        .arg("-G")
        .arg(host)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => Error::Unavailable,
        _ => Error::Failed(error.to_string()),
    })?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| Error::Failed("ssh -G took too long.".into()))?
        .map_err(|error| Error::Failed(error.to_string()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(Error::Failed(if message.is_empty() {
            format!("ssh -G ended with {}.", output.status)
        } else {
            message
        }));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| Error::Failed("ssh -G printed invalid text.".into()))
}

/// `keyword value` lines; a keyword such as `identityfile` may repeat.
fn parse(text: &str) -> HashMap<String, Vec<String>> {
    let mut options = HashMap::<String, Vec<String>>::new();
    for line in text.lines() {
        if let Some((keyword, value)) = line.split_once(' ') {
            options
                .entry(keyword.to_ascii_lowercase())
                .or_default()
                .push(value.to_owned());
        }
    }
    options
}

fn difference(
    configured: &HashMap<String, Vec<String>>,
    defaults: &HashMap<String, Vec<String>>,
    home: Option<&Path>,
) -> Resolved {
    let changed = |keyword: &str| -> Option<String> {
        let value = configured.get(keyword)?.first()?;
        let default = defaults.get(keyword).and_then(|values| values.first());
        (default.is_none_or(|default| !default.eq_ignore_ascii_case(value))).then(|| value.clone())
    };
    let none = |value: String| (!value.eq_ignore_ascii_case("none")).then_some(value);
    let local_user = defaults
        .get("user")
        .and_then(|values| values.first())
        .map(String::as_str);
    let empty = Vec::new();
    let default_keys = defaults.get("identityfile").unwrap_or(&empty);
    let identity_file = configured
        .get("identityfile")
        .unwrap_or(&empty)
        .iter()
        .filter(|value| !default_keys.contains(value))
        .filter_map(|value| expand(value, home, local_user))
        .find(|path| Path::new(path).is_file());
    Resolved {
        hostname: changed("hostname"),
        user: changed("user"),
        port: changed("port").and_then(|port| port.parse().ok()),
        identity_file,
        proxy_command: changed("proxycommand").and_then(none),
        proxy_jump: changed("proxyjump").and_then(none),
    }
}

/// `~`, `%d` (home) and `%u` (local user), the tokens a key file usually
/// uses. A file named with any other token is left for the person to choose.
fn expand(value: &str, home: Option<&Path>, user: Option<&str>) -> Option<String> {
    let home = home.map(|home| home.to_string_lossy().into_owned());
    let value = match value.strip_prefix("~/") {
        Some(rest) => format!("%d/{rest}"),
        None => value.to_owned(),
    };
    let mut path = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            path.push(c);
            continue;
        }
        match chars.next()? {
            '%' => path.push('%'),
            'd' => path.push_str(home.as_deref()?),
            'u' => path.push_str(user?),
            _ => return None,
        }
    }
    Some(path)
}

#[cfg(test)]
mod tests;
