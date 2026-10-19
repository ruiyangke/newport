//! Hosts from the user's OpenSSH config, offered on first run. Read-only: the
//! file is never written, and nothing is saved until the user imports.
use serde::Serialize;
use ssh2_config::{ParseRule, SshConfig};

/// Git hosting accepts SSH for pushes, not shells.
const GIT_HOSTS: &[&str] = &[
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "codeberg.org",
    "ssh.dev.azure.com",
    "vs-ssh.visualstudio.com",
];

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigHost {
    pub alias: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<String>,
    /// Newport can connect to it directly.
    pub importable: bool,
    /// Ticked by default: importable and not Git hosting.
    pub suggested: bool,
    pub note: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigHosts {
    pub path: String,
    pub hosts: Vec<ConfigHost>,
}

pub fn read() -> Result<ConfigHosts, String> {
    let Some(home) = dirs::home_dir() else {
        return Ok(ConfigHosts::default());
    };
    let path = home.join(".ssh").join("config");
    let display = "~/.ssh/config".to_owned();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigHosts {
                path: display,
                hosts: vec![],
            });
        }
        Err(error) => return Err(format!("Couldn’t read {display}: {error}")),
    };
    let config = parse(&text).map_err(|error| format!("Couldn’t read {display}: {error}"))?;
    // ssh defaults to the local account name, which Windows keeps in USERNAME.
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    Ok(ConfigHosts {
        path: display,
        hosts: hosts(&config, &user, &home),
    })
}

/// The parser has no `Match` support: it would apply a Match block's settings
/// to the preceding Host (or to every host). A `Host !*` block matches nothing.
fn parse(text: &str) -> ssh2_config::SshParserResult<SshConfig> {
    let text = text
        .lines()
        .map(|line| {
            let keyword = line
                .trim_start()
                .split([' ', '\t', '='])
                .next()
                .unwrap_or("");
            if keyword.eq_ignore_ascii_case("match") {
                "Host !*"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    // ProxyCommand is "unsupported" rather than unknown; keep it to detect it.
    SshConfig::default().parse(
        &mut text.as_bytes(),
        ParseRule::ALLOW_UNKNOWN_FIELDS | ParseRule::ALLOW_UNSUPPORTED_FIELDS,
    )
}

/// One entry per concrete alias, in file order. Patterns (`Host *.lan`) only
/// supply defaults, which `query` merges in the way ssh resolves them.
fn hosts(config: &SshConfig, local_user: &str, home: &std::path::Path) -> Vec<ConfigHost> {
    let mut seen = std::collections::HashSet::new();
    // Several aliases often name one machine; offer it once.
    let mut targets = std::collections::HashMap::new();
    let mut result = vec![];
    for block in config.get_hosts() {
        for clause in &block.pattern {
            let alias = clause.pattern.as_str();
            if clause.negated || alias.contains(['*', '?', '!']) || !seen.insert(alias.to_owned()) {
                continue;
            }
            let params = config.query(alias);
            let host = params.host_name.clone().unwrap_or_else(|| alias.to_owned());
            let user = params.user.clone().unwrap_or_else(|| local_user.to_owned());
            let port = params.port.unwrap_or(22);
            let identity_file = params
                .identity_file
                .as_ref()
                .and_then(|files| files.first())
                .map(|file| match file.strip_prefix(home) {
                    Ok(rest) => format!("~/{}", rest.display()),
                    Err(_) => file.display().to_string(),
                });
            let proxied = params
                .proxy_jump
                .as_ref()
                // `ProxyJump none` exempts a host (often the bastion) from a default.
                .is_some_and(|jumps| {
                    !jumps.is_empty() && !jumps.iter().all(|jump| jump.eq_ignore_ascii_case("none"))
                })
                || [&params.unsupported_fields, &params.ignored_fields]
                    .iter()
                    .any(|fields| fields.contains_key("proxycommand"));
            let git = GIT_HOSTS.contains(&host.to_ascii_lowercase().as_str());
            let candidate = newport_domain::model::Server {
                id: uuid::Uuid::nil(),
                name: alias.to_owned(),
                ssh_user: user.clone(),
                ssh_host: host.clone(),
                ssh_port: port,
                identity_file: identity_file.clone(),
                agent_source: None,
                agent_key_fingerprint: None,
                auth_method: Default::default(),
                clipboard_enabled: false,
                browser_enabled: false,
                agent_install_approved: false,
                setup_complete: false,
            };
            let invalid = candidate.validate().err();
            let note = if proxied {
                Some("Uses a jump host, which Newport doesn’t support yet".to_owned())
            } else if let Some(reason) = invalid.clone() {
                Some(reason)
            } else if git {
                Some("Git hosting".to_owned())
            } else {
                None
            };
            let importable = !proxied && invalid.is_none();
            let duplicate = importable
                .then(|| {
                    targets
                        .entry((user.clone(), host.to_ascii_lowercase(), port))
                        .or_insert_with(|| alias.to_owned())
                        .clone()
                })
                .filter(|first| first != alias);
            let note = note.or(duplicate
                .as_ref()
                .map(|first| format!("Same server as {first}")));
            result.push(ConfigHost {
                alias: alias.to_owned(),
                host,
                user,
                port,
                identity_file,
                importable,
                suggested: importable && !git && duplicate.is_none(),
                note,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Vec<ConfigHost> {
        hosts(
            &super::parse(text).unwrap(),
            "local",
            std::path::Path::new("/home/me"),
        )
    }

    #[test]
    fn concrete_aliases_resolve_defaults_and_skip_patterns() {
        let hosts = parse(
            "Host dev\n  HostName dev.example.com\n  User ada\n  IdentityFile /home/me/.ssh/id_dev\n\
             Host nas backup\n  HostName 10.0.0.4\n  Port 2222\n\
             Host *.lan !secret\n  User lan\n\
             Host *\n  User fallback\n",
        );
        let aliases: Vec<_> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, ["dev", "nas", "backup"]);
        assert_eq!(hosts[0].host, "dev.example.com");
        assert_eq!(hosts[0].user, "ada");
        assert_eq!(hosts[0].identity_file.as_deref(), Some("~/.ssh/id_dev"));
        assert!(hosts[0].suggested);
        assert_eq!((hosts[1].port, hosts[1].user.as_str()), (2222, "fallback"));
    }

    #[test]
    fn git_hosting_and_jump_hosts_are_not_suggested() {
        let hosts = parse(
            "Host gh\n  HostName github.com\n  User git\n\
             Host inner\n  HostName 10.1.0.2\n  User ops\n  ProxyJump bastion\n\
             Host plain\n  HostName plain.example.com\n",
        );
        assert!(hosts[0].importable && !hosts[0].suggested);
        assert_eq!(hosts[0].note.as_deref(), Some("Git hosting"));
        assert!(!hosts[1].importable && !hosts[1].suggested);
        assert_eq!(hosts[2].user, "local", "ssh falls back to the local user");
        assert!(hosts[2].suggested);
    }

    #[test]
    fn proxy_commands_match_blocks_and_proxy_jump_none_resolve_like_ssh() {
        let hosts = parse(
            "Match host *.corp\n  User admin\n\
             Host bastion\n  HostName bastion.example.com\n  User ops\n  ProxyJump none\n\
             Host inner\n  HostName 10.1.0.3\n  User ops\n  ProxyCommand ssh -W %h:%p bastion\n\
             Host *\n  ProxyJump bastion\n",
        );
        let bastion = &hosts[0];
        assert_eq!(bastion.alias, "bastion");
        assert!(bastion.importable, "ProxyJump none isn't a jump host");
        assert_eq!(bastion.user, "ops", "Match settings don't leak into hosts");
        assert!(!hosts[1].importable, "ProxyCommand needs a jump");
    }

    #[test]
    fn a_second_alias_for_the_same_server_is_not_suggested() {
        let hosts = parse(
            "Host gce\n  HostName 34.64.153.48\n  User ruiyang\n\
             Host 34.64.153.48\n  User ruiyang\n",
        );
        assert!(hosts[0].suggested);
        assert!(hosts[1].importable && !hosts[1].suggested);
        assert_eq!(hosts[1].note.as_deref(), Some("Same server as gce"));
    }
}
