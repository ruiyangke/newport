//! Hosts from the user's OpenSSH config, offered on first run. Read-only: the
//! file is never written, and nothing is saved until the user imports.
use newport_domain::model::{Proxy, Server};
use serde::Serialize;
use ssh2_config::{HostParams, ParseRule, SshConfig};
use uuid::Uuid;

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
    /// The id the server gets when imported, which other hosts' jump hosts
    /// refer to.
    pub id: Uuid,
    pub alias: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<String>,
    pub proxy: Option<Proxy>,
    /// The aliases of the hosts it jumps through, which are imported with it.
    pub through: Vec<String>,
    /// Newport can connect to it.
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

/// Why a host can't be imported when its jump hosts lead back to it.
const LOOP: &str = "Its jump hosts lead back to it";

/// How a host is reached, as its config says.
enum Route {
    Direct,
    Command(String),
    /// The hops as written, nearest to this computer first.
    Jump(Vec<String>),
}

/// A host on its way to the list.
struct Entry {
    id: Uuid,
    alias: String,
    host: String,
    user: String,
    port: u16,
    identity_file: Option<String>,
    route: Route,
    /// Set once resolved; `Err` says why it can't be imported.
    proxy: Option<Result<Option<Proxy>, String>>,
    /// The hosts it jumps through, nearest first.
    through: Vec<usize>,
    /// Added as a jump host rather than found as an alias.
    added: bool,
    /// For an added host, the aliases that jump through it.
    jump_host_for: Vec<String>,
}

/// One entry per concrete alias, in file order, each after the hosts it
/// jumps through. Patterns (`Host *.lan`) only supply defaults, which
/// `query` merges in the way ssh resolves them.
fn hosts(config: &SshConfig, local_user: &str, home: &std::path::Path) -> Vec<ConfigHost> {
    let mut seen = std::collections::HashSet::new();
    let mut importer = Importer {
        config,
        local_user,
        home,
        entries: vec![],
    };
    for block in config.get_hosts() {
        for clause in &block.pattern {
            let alias = clause.pattern.as_str();
            if clause.negated || alias.contains(['*', '?', '!']) || !seen.insert(alias.to_owned()) {
                continue;
            }
            importer.push(alias, &config.query(alias));
        }
    }
    for index in 0..importer.entries.len() {
        importer.resolve(index, &mut vec![]);
    }
    let mut order = vec![];
    for index in 0..importer.entries.len() {
        importer.order(index, &mut order);
    }
    let entries = importer.entries;
    let mut importable = vec![false; entries.len()];
    // Several aliases often name one machine; offer it once.
    let mut targets = std::collections::HashMap::new();
    let mut result = vec![];
    for index in order {
        let entry = &entries[index];
        let proxy = entry.proxy.clone().expect("every host is resolved");
        let candidate = Server {
            id: entry.id,
            name: entry.alias.clone(),
            ssh_user: entry.user.clone(),
            ssh_host: entry.host.clone(),
            ssh_port: entry.port,
            identity_file: entry.identity_file.clone(),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: Default::default(),
            clipboard_enabled: false,
            browser_enabled: false,
            agent_install_approved: false,
            setup_complete: false,
            proxy: proxy.clone().ok().flatten(),
        };
        let unimportable_jump = entry.through.iter().find(|&&jump| !importable[jump]);
        let invalid = match (&proxy, unimportable_jump) {
            (Err(reason), _) => Some(reason.clone()),
            (Ok(_), Some(&jump)) => Some(format!(
                "Its jump host {} can’t be imported",
                entries[jump].alias
            )),
            (Ok(_), None) => candidate.validate().err(),
        };
        importable[index] = invalid.is_none();
        let git = GIT_HOSTS.contains(&entry.host.to_ascii_lowercase().as_str());
        let note = if let Some(reason) = invalid {
            Some(reason)
        } else if git {
            Some("Git hosting".to_owned())
        } else if entry.added {
            Some(format!("Jump host for {}", entry.jump_host_for.join(", ")))
        } else {
            None
        };
        let duplicate = importable[index]
            .then(|| {
                targets
                    .entry((
                        entry.user.clone(),
                        entry.host.to_ascii_lowercase(),
                        entry.port,
                        candidate.proxy.clone(),
                    ))
                    .or_insert_with(|| entry.alias.clone())
                    .clone()
            })
            .filter(|first| *first != entry.alias);
        let note = note.or(duplicate
            .as_ref()
            .map(|first| format!("Same server as {first}")));
        result.push(ConfigHost {
            id: entry.id,
            alias: entry.alias.clone(),
            host: entry.host.clone(),
            user: entry.user.clone(),
            port: entry.port,
            identity_file: entry.identity_file.clone(),
            proxy: candidate.proxy,
            through: entry
                .through
                .iter()
                .map(|&jump| entries[jump].alias.clone())
                .collect(),
            importable: importable[index],
            // An added jump host comes with the hosts that need it.
            suggested: importable[index] && !git && duplicate.is_none() && !entry.added,
            note,
        });
    }
    result
}

struct Importer<'a> {
    config: &'a SshConfig,
    local_user: &'a str,
    home: &'a std::path::Path,
    entries: Vec<Entry>,
}

impl Importer<'_> {
    fn push(&mut self, alias: &str, params: &HostParams) -> usize {
        let identity_file = params
            .identity_file
            .as_ref()
            .and_then(|files| files.first())
            .map(|file| match file.strip_prefix(self.home) {
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => file.display().to_string(),
            });
        let command = [&params.unsupported_fields, &params.ignored_fields]
            .iter()
            .find_map(|fields| fields.get("proxycommand"))
            .map(|words| words.join(" "));
        let jumps = params
            .proxy_jump
            .clone()
            // `ProxyJump none` exempts a host (often the bastion) from a default.
            .filter(|jumps| {
                !jumps.is_empty() && !jumps.iter().all(|jump| jump.eq_ignore_ascii_case("none"))
            });
        // A host's own ProxyCommand usually comes before a `Host *` jump
        // default, and ssh uses whichever comes first.
        let route = match (command, jumps) {
            (Some(command), _) => Route::Command(command),
            (None, Some(jumps)) => Route::Jump(jumps),
            (None, None) => Route::Direct,
        };
        self.entries.push(Entry {
            id: Uuid::new_v4(),
            alias: alias.to_owned(),
            host: params.host_name.clone().unwrap_or_else(|| alias.to_owned()),
            user: params
                .user
                .clone()
                .unwrap_or_else(|| self.local_user.to_owned()),
            port: params.port.unwrap_or(22),
            identity_file,
            route,
            proxy: None,
            through: vec![],
            added: false,
            jump_host_for: vec![],
        });
        self.entries.len() - 1
    }

    /// Settles how the host at `index` is reached, adding the jump hosts its
    /// chain needs. `resolving` holds the hosts waiting on this one.
    fn resolve(&mut self, index: usize, resolving: &mut Vec<usize>) {
        if self.entries[index].proxy.is_some() || resolving.contains(&index) {
            return;
        }
        let hops = match &self.entries[index].route {
            Route::Direct => return self.settle(index, None),
            Route::Command(command) => {
                self.entries[index].proxy = Some(Ok(Some(Proxy::Command(command.clone()))));
                return;
            }
            Route::Jump(hops) => hops.clone(),
        };
        resolving.push(index);
        let mut previous = None;
        let mut failure = None;
        for hop in &hops {
            match self.hop(hop, previous, resolving) {
                Ok(jump) => previous = Some(jump),
                Err(reason) => {
                    failure = Some(reason);
                    break;
                }
            }
        }
        resolving.pop();
        if self.entries[index].proxy.is_some() {
            return;
        }
        match failure {
            Some(reason) => self.entries[index].proxy = Some(Err(reason)),
            None => self.settle(index, previous),
        }
    }

    /// Reaches the host at `index` through `jump`, or directly.
    fn settle(&mut self, index: usize, jump: Option<usize>) {
        let Some(jump) = jump else {
            self.entries[index].proxy = Some(Ok(None));
            return;
        };
        let mut through = vec![jump];
        through.extend(self.entries[jump].through.iter().copied());
        if through.contains(&index) {
            self.entries[index].proxy = Some(Err(LOOP.into()));
            return;
        }
        if !self.entries[index].added {
            let alias = self.entries[index].alias.clone();
            for &host in &through {
                if self.entries[host].added {
                    self.entries[host].jump_host_for.push(alias.clone());
                }
            }
        }
        self.entries[index].proxy = Some(Ok(Some(Proxy::Jump(self.entries[jump].id))));
        self.entries[index].through = through;
    }

    /// The host a chain's hop names, reached through `previous`, as ssh's
    /// `-J` reaches it: the first hop is an alias with its own settings, and
    /// a later hop goes through the one before it. A hop that names no host
    /// reached that way gets an entry of its own.
    fn hop(
        &mut self,
        hop: &str,
        previous: Option<usize>,
        resolving: &mut Vec<usize>,
    ) -> Result<usize, String> {
        let expected = previous.map(|jump| Proxy::Jump(self.entries[jump].id));
        if let Some(alias) = self.entries.iter().position(|entry| entry.alias == hop) {
            if let Some(start) = resolving.iter().position(|&host| host == alias) {
                // Every host from that one on waits for the next: a loop.
                for &host in &resolving[start..] {
                    self.entries[host].proxy = Some(Err(LOOP.into()));
                }
                return Err(LOOP.into());
            }
            self.resolve(alias, resolving);
            match &self.entries[alias].proxy {
                Some(Ok(proxy)) if previous.is_none() || *proxy == expected => return Ok(alias),
                Some(Err(_)) if previous.is_none() => return Ok(alias),
                _ => {}
            }
        }
        let (user, host, port) = split_hop(hop);
        let mut params = self.config.query(host);
        params.host_name.get_or_insert_with(|| host.to_owned());
        if let Some(user) = user {
            params.user = Some(user.to_owned());
        }
        if let Some(port) = port {
            let port = port
                .parse()
                .map_err(|_| format!("Its jump host {hop} has an invalid port"))?;
            params.port = Some(port);
        }
        let target = (
            params
                .user
                .clone()
                .unwrap_or_else(|| self.local_user.to_owned()),
            params
                .host_name
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase(),
            params.port.unwrap_or(22),
        );
        // The same server reached the same way is one jump host.
        let reached = Some(Ok(expected));
        if let Some(existing) = self.entries.iter().position(|entry| {
            (
                entry.user.clone(),
                entry.host.to_ascii_lowercase(),
                entry.port,
            ) == target
                && entry.proxy == reached
        }) {
            return Ok(existing);
        }
        let mut name = match previous {
            Some(jump) if self.taken(hop) => format!("{hop} through {}", self.entries[jump].alias),
            _ => hop.to_owned(),
        };
        let base = name.clone();
        let mut copy = 1;
        while self.taken(&name) {
            copy += 1;
            name = format!("{base} {copy}");
        }
        let index = self.push(&name, &params);
        self.entries[index].added = true;
        self.settle(index, previous);
        Ok(index)
    }

    fn taken(&self, alias: &str) -> bool {
        self.entries.iter().any(|entry| entry.alias == alias)
    }

    /// Lists the host at `index` after the hosts it jumps through.
    fn order(&self, index: usize, order: &mut Vec<usize>) {
        if order.contains(&index) {
            return;
        }
        if let Some(&jump) = self.entries[index].through.first() {
            self.order(jump, order);
        }
        order.push(index);
    }
}

/// A `ProxyJump` hop: `[user@]host[:port]`, or the same as an `ssh://` URI.
fn split_hop(hop: &str) -> (Option<&str>, &str, Option<&str>) {
    let hop = hop.strip_prefix("ssh://").unwrap_or(hop);
    let (user, rest) = match hop.rsplit_once('@') {
        Some((user, rest)) => (Some(user), rest),
        None => (None, hop),
    };
    if let Some(bracketed) = rest.strip_prefix('[')
        && let Some((host, port)) = bracketed.split_once(']')
    {
        return (user, host, port.strip_prefix(':'));
    }
    match rest.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (user, host, Some(port)),
        _ => (user, rest, None),
    }
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

    fn host<'a>(hosts: &'a [ConfigHost], alias: &str) -> &'a ConfigHost {
        hosts
            .iter()
            .find(|host| host.alias == alias)
            .unwrap_or_else(|| panic!("no {alias} in {hosts:#?}"))
    }

    fn position(hosts: &[ConfigHost], alias: &str) -> usize {
        hosts.iter().position(|host| host.alias == alias).unwrap()
    }

    #[test]
    fn git_hosting_is_importable_but_not_suggested() {
        let hosts = parse(
            "Host gh\n  HostName github.com\n  User git\n\
             Host plain\n  HostName plain.example.com\n",
        );
        assert!(hosts[0].importable && !hosts[0].suggested);
        assert_eq!(hosts[0].note.as_deref(), Some("Git hosting"));
        assert_eq!(hosts[1].user, "local", "ssh falls back to the local user");
        assert!(hosts[1].suggested);
        assert_eq!(hosts[1].proxy, None);
    }

    #[test]
    fn proxy_commands_match_blocks_and_proxy_jump_none_resolve_like_ssh() {
        let hosts = parse(
            "Match host *.corp\n  User admin\n\
             Host bastion\n  HostName bastion.example.com\n  User ops\n  ProxyJump none\n\
             Host inner\n  HostName 10.1.0.3\n  User ops\n  ProxyCommand ssh -W %h:%p bastion\n\
             Host *\n  ProxyJump bastion\n",
        );
        let bastion = host(&hosts, "bastion");
        assert!(bastion.importable, "ProxyJump none isn't a jump host");
        assert_eq!(bastion.proxy, None);
        assert_eq!(bastion.user, "ops", "Match settings don't leak into hosts");
        let inner = host(&hosts, "inner");
        assert!(inner.importable && inner.suggested, "{inner:?}");
        assert_eq!(
            inner.proxy,
            Some(Proxy::Command("ssh -W %h:%p bastion".into())),
            "a host's own ProxyCommand comes before the default jump host"
        );
        assert_eq!(inner.note, None);
    }

    #[test]
    fn a_proxy_command_keeps_its_tokens_as_written() {
        let hosts = parse(
            "Host vm\n  HostName 127.0.0.1\n  ProxyCommand nc -X connect -x proxy:8080 %h %p\n",
        );
        assert!(hosts[0].importable);
        assert_eq!(
            hosts[0].proxy,
            Some(Proxy::Command("nc -X connect -x proxy:8080 %h %p".into()))
        );
    }

    #[test]
    fn a_jump_alias_is_imported_first_and_jumped_through() {
        let hosts = parse(
            "Host inner\n  HostName 10.1.0.2\n  User ops\n  ProxyJump bastion\n\
             Host bastion\n  HostName bastion.example.com\n  User ops\n",
        );
        assert_eq!(hosts.len(), 2);
        let (bastion, inner) = (host(&hosts, "bastion"), host(&hosts, "inner"));
        assert!(position(&hosts, "bastion") < position(&hosts, "inner"));
        assert_eq!(inner.proxy, Some(Proxy::Jump(bastion.id)));
        assert_eq!(inner.through, ["bastion"]);
        assert!(inner.importable && inner.suggested && inner.note.is_none());
        assert!(bastion.suggested && bastion.note.is_none(), "{bastion:?}");
    }

    #[test]
    fn a_jump_host_written_as_an_address_is_imported_as_its_own_server() {
        let hosts = parse(
            "Host inner\n  HostName 10.1.0.2\n  ProxyJump ops@jump.example.com:2222\n\
             Host other\n  HostName 10.1.0.3\n  ProxyJump ops@jump.example.com:2222\n",
        );
        assert_eq!(hosts.len(), 3, "{hosts:#?}");
        let jump = &hosts[0];
        assert_eq!(jump.alias, "ops@jump.example.com:2222");
        assert_eq!(
            (jump.user.as_str(), jump.host.as_str(), jump.port),
            ("ops", "jump.example.com", 2222)
        );
        assert_eq!(jump.proxy, None);
        assert!(jump.importable && !jump.suggested);
        assert_eq!(jump.note.as_deref(), Some("Jump host for inner, other"));
        for alias in ["inner", "other"] {
            assert_eq!(host(&hosts, alias).proxy, Some(Proxy::Jump(jump.id)));
            assert_eq!(host(&hosts, alias).through, [jump.alias.as_str()]);
        }
    }

    #[test]
    fn an_address_that_names_an_imported_host_jumps_through_that_host() {
        let hosts = parse(
            "Host bastion\n  HostName bastion.example.com\n  User ops\n\
             Host inner\n  HostName 10.1.0.2\n  ProxyJump ops@bastion.example.com\n",
        );
        assert_eq!(hosts.len(), 2);
        assert_eq!(
            host(&hosts, "inner").proxy,
            Some(Proxy::Jump(host(&hosts, "bastion").id))
        );
    }

    #[test]
    fn several_jump_hosts_become_a_chain() {
        let hosts = parse(
            "Host a\n  HostName a.example.com\n\
             Host b\n  HostName 10.0.0.2\n\
             Host c\n  HostName 10.0.0.3\n  ProxyJump a\n\
             Host inner\n  HostName 10.1.0.2\n  ProxyJump a,b\n\
             Host deeper\n  HostName 10.1.0.3\n  ProxyJump a,c\n",
        );
        let a = host(&hosts, "a");
        // `b` itself connects directly, so the hop through `a` is another server.
        let b = host(&hosts, "b through a");
        assert_eq!(b.host, "10.0.0.2");
        assert_eq!(b.proxy, Some(Proxy::Jump(a.id)));
        assert_eq!(b.note.as_deref(), Some("Jump host for inner"));
        assert_eq!(host(&hosts, "b").proxy, None);
        let inner = host(&hosts, "inner");
        assert_eq!(inner.proxy, Some(Proxy::Jump(b.id)));
        assert_eq!(inner.through, ["b through a", "a"]);
        assert!(position(&hosts, "a") < position(&hosts, "b through a"));
        assert!(position(&hosts, "b through a") < position(&hosts, "inner"));
        // `c` already jumps through `a`, so the chain reuses it.
        let deeper = host(&hosts, "deeper");
        assert_eq!(deeper.proxy, Some(Proxy::Jump(host(&hosts, "c").id)));
        assert_eq!(deeper.through, ["c", "a"]);
        assert_eq!(hosts.len(), 6, "{hosts:#?}");
    }

    #[test]
    fn jump_hosts_that_lead_back_are_not_importable() {
        let hosts = parse(
            "Host a\n  HostName a.example.com\n  ProxyJump b\n\
             Host b\n  HostName b.example.com\n  ProxyJump a\n\
             Host bastion\n  HostName bastion.example.com\n\
             Host inner\n  HostName 10.1.0.2\n\
             Host *\n  ProxyJump bastion\n",
        );
        for alias in ["a", "b", "bastion"] {
            let host = host(&hosts, alias);
            assert!(!host.importable && !host.suggested, "{host:?}");
            assert_eq!(host.note.as_deref(), Some(LOOP));
        }
        let inner = host(&hosts, "inner");
        assert!(!inner.importable);
        assert_eq!(
            inner.note.as_deref(),
            Some("Its jump host bastion can’t be imported")
        );
    }

    #[test]
    fn a_second_alias_for_the_same_server_is_not_suggested() {
        let hosts = parse(
            "Host sample\n  HostName 192.0.2.10\n  User developer\n\
             Host 192.0.2.10\n  User developer\n",
        );
        assert!(hosts[0].suggested);
        assert!(hosts[1].importable && !hosts[1].suggested);
        assert_eq!(hosts[1].note.as_deref(), Some("Same server as sample"));
    }
}
