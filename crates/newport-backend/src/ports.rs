//! Listener discovery and process/container attribution, independent of SSH transport.
use crate::{model::Server, ssh};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// The key is (server session incarnation, listening ports). The session owns
/// the cache across windows/navigation; replacing credentials creates a new key.
pub(crate) fn query(
    owner: &std::sync::Arc<crate::server_session::ServerSession>,
) -> &crate::query::Query<Vec<DiscoveredPort>> {
    let query = owner.ports.get_or_init(|| {
        let weak = std::sync::Arc::downgrade(owner);
        crate::query::Query::new(
            crate::query::Policy {
                stale_after: std::time::Duration::from_secs(30),
                retry_after: std::time::Duration::from_secs(10),
                timeout: std::time::Duration::from_secs(35),
                observation_lease: std::time::Duration::from_secs(5),
            },
            move || {
                let weak = weak.clone();
                async move {
                    let owner = weak.upgrade().ok_or("Server session closed.")?;
                    let mut cancelled = owner.cancellation();
                    if *cancelled.borrow() {
                        return Err("Server session closed.".into());
                    }
                    tokio::select! {
                        biased;
                        _ = cancelled.changed() => Err("Server session closed.".into()),
                        result = async {
                            let lease = owner.acquire(owner.server()).await?;
                            let output = lease.execute(DISCOVER_PORTS_COMMAND, None).await?;
                            Ok(parse_ports(&output))
                        } => result,
                    }
                }
            },
        )
    });
    if owner.is_revoked() {
        query.close();
    }
    query
}

pub async fn discover(server: &Server) -> Result<Vec<DiscoveredPort>, String> {
    let output = ssh::execute(server, DISCOVER_PORTS_COMMAND, None).await?;
    Ok(parse_ports(&output))
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredPort {
    pub port: u16,
    pub address: String,
    pub process_name: Option<String>,
    pub pid: Option<u32>,
    pub container_name: Option<String>,
    pub executable: Option<String>,
    pub working_directory: Option<String>,
    /// The working directory was removed while the process kept running.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub working_directory_deleted: bool,
    /// The executable was replaced or removed since the process started.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub executable_deleted: bool,
    pub command: Option<String>,
    pub arguments: Vec<String>,
    pub user: Option<String>,
    pub application_name: Option<String>,
    /// Projectile-style root of the working directory, or a Compose project's directory.
    pub project_root: Option<String>,
    /// What identified the root: `.git`, `package.json`, `Makefile`, `compose`…
    pub project_marker: Option<String>,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    pub compose_working_dir: Option<String>,
    pub image: Option<String>,
    /// The checkout the process runs in: a linked worktree, or the main one.
    /// `project_root` is then the repository every worktree shares.
    pub worktree_path: Option<String>,
    /// The branch checked out there, or a detached HEAD's short commit id.
    pub worktree_branch: Option<String>,
    pub worktree_main: Option<bool>,
}
/// A directory's project as the server resolved it.
#[derive(Default)]
struct ProjectInfo {
    root: Option<String>,
    marker: Option<String>,
    worktree: Option<String>,
    branch: Option<String>,
    main: Option<bool>,
}
impl ProjectInfo {
    /// Decoded root, marker, then optionally worktree, branch and main flag.
    fn parse(fields: &[String]) -> Self {
        let field = |i: usize| fields.get(i).and_then(|s| nonempty(s));
        let root = field(0);
        // Nothing describes a root that was not found.
        if root.is_none() {
            return Self::default();
        }
        Self {
            marker: field(1),
            worktree: field(2),
            branch: field(3),
            main: match fields.get(4).map(String::as_str) {
                Some("1") => Some(true),
                Some("0") => Some(false),
                _ => None,
            },
            root,
        }
    }
    /// Version-control roots (and an explicit `.projectile`) name a project
    /// more precisely than the Compose directory inside them.
    fn is_repository(&self) -> bool {
        matches!(
            self.marker.as_deref(),
            Some(
                ".projectile"
                    | ".git"
                    | ".hg"
                    | ".jj"
                    | ".sl"
                    | ".fslckout"
                    | "_FOSSIL_"
                    | ".bzr"
                    | "_darcs"
                    | ".svn"
            )
        )
    }
    fn apply(&self, port: &mut DiscoveredPort) {
        port.project_root = self.root.clone();
        port.project_marker = self.marker.clone();
        port.worktree_path = self.worktree.clone();
        port.worktree_branch = self.branch.clone();
        port.worktree_main = self.main;
    }
}
const DISCOVER_PORTS_COMMAND: &str = include_str!("discover-ports.sh");

pub fn parse_ports(output: &str) -> Vec<DiscoveredPort> {
    let (output, details) = output
        .split_once("__NEWPORT_PROCESS_DETAILS__\n")
        .unwrap_or((output, ""));
    let (listeners, docker) = output
        .split_once("__NEWPORT_DOCKER_PORTS__\n")
        .unwrap_or((output, ""));
    let (docker, compose_roots) = docker
        .split_once("__NEWPORT_COMPOSE_ROOTS__\n")
        .unwrap_or((docker, ""));
    // Compose working directory -> the project around it.
    let compose_roots: HashMap<String, ProjectInfo> = compose_roots
        .lines()
        .filter_map(|line| {
            let decoded: Option<Vec<_>> = line.split('|').map(decode_hex).collect();
            let decoded = decoded.filter(|d| d.len() == 6)?;
            Some((decoded[0].clone(), ProjectInfo::parse(&decoded[1..])))
        })
        .collect();
    let mut ports: BTreeMap<(u16, String), DiscoveredPort> = BTreeMap::new();
    for line in listeners.lines().filter(|line| line.contains("LISTEN")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        // Both ss -tlnp and netstat -tlnp put the local address in column four.
        let Some((address, port)) = fields.get(3).and_then(|field| field.rsplit_once(':')) else {
            continue;
        };
        let Some(port) = port.parse::<u16>().ok().filter(|p| *p > 0) else {
            continue;
        };
        let process_name = line
            .split("users:((\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .map(str::to_owned)
            .or_else(|| {
                fields
                    .last()
                    .and_then(|s| s.split_once('/'))
                    .map(|(_, name)| name.to_owned())
            });
        let pid = line
            .split("pid=")
            .nth(1)
            .map(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                fields
                    .last()
                    .and_then(|s| s.split_once('/'))
                    .and_then(|(pid, _)| pid.parse().ok())
            });
        let address = address.trim_matches(['[', ']']).to_owned();
        let entry = ports
            .entry((port, address.clone()))
            .or_insert(DiscoveredPort {
                port,
                address,
                ..Default::default()
            });
        if entry.process_name.is_none() && process_name.is_some() {
            entry.process_name = process_name;
            entry.pid = pid;
        }
    }
    for line in docker.lines() {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let (Some(name), Some(bindings)) = (row["name"].as_str(), row["ports"].as_str()) else {
            continue;
        };
        // Docker reports a missing label as an empty string.
        let label = |key: &str| row[key].as_str().and_then(nonempty);
        for mapping in bindings.split(',').map(str::trim) {
            let Some((host, target)) = mapping.split_once("->") else {
                continue;
            };
            if !target.ends_with("/tcp") {
                continue;
            }
            let Some((address, published)) = host.rsplit_once(':') else {
                continue;
            };
            let (start, end) = published.split_once('-').unwrap_or((published, published));
            let (Ok(start), Ok(end)) = (start.parse::<u16>(), end.parse::<u16>()) else {
                continue;
            };
            if start == 0 || end < start {
                continue;
            }
            let address = address.trim_matches(['[', ']']).to_owned();
            for port in start..=end {
                let entry = ports
                    .entry((port, address.clone()))
                    .or_insert(DiscoveredPort {
                        port,
                        address: address.clone(),
                        ..Default::default()
                    });
                entry.container_name = Some(name.to_owned());
                entry.image = label("image");
                entry.compose_project = label("project");
                entry.compose_service = label("service");
                entry.compose_working_dir = label("workdir");
                if let Some(workdir) = &entry.compose_working_dir {
                    match compose_roots
                        .get(workdir)
                        .filter(|info| info.is_repository())
                    {
                        Some(info) => info.apply(entry),
                        None => {
                            entry.project_root = Some(workdir.clone());
                            entry.project_marker = Some("compose".into());
                        }
                    }
                }
            }
        }
    }
    for line in details.lines() {
        let fields: Vec<_> = line.split('|').collect();
        // pid, exe, cwd, argv, user, then the project root, its marker, and
        // the worktree, branch and main flag. Older agents send fewer fields.
        if ![5, 7, 10].contains(&fields.len()) {
            continue;
        }
        let Ok(pid) = fields[0].parse::<u32>() else {
            continue;
        };
        let decoded: Option<Vec<_>> = fields[1..].iter().map(|s| decode_hex(s)).collect();
        let Some(decoded) = decoded else {
            continue;
        };
        for port in ports.values_mut().filter(|p| p.pid == Some(pid)) {
            let (executable, executable_deleted) = deleted(decoded[0].trim_end_matches('\n'));
            let (directory, directory_deleted) = deleted(decoded[1].trim_end_matches('\n'));
            port.executable = nonempty(executable);
            port.executable_deleted = executable_deleted;
            port.working_directory = nonempty(directory);
            port.working_directory_deleted = directory_deleted;
            port.user = nonempty(decoded[3].trim());
            // A container's Compose directory outranks its proxy process's directory.
            if port.project_root.is_none() {
                ProjectInfo::parse(&decoded[4..]).apply(port);
            }
            let argv: Vec<_> = decoded[2].split('\0').filter(|s| !s.is_empty()).collect();
            port.command = nonempty(&argv.join(" "));
            port.arguments = argv.iter().map(|arg| (*arg).to_owned()).collect();
            let executable = port
                .executable
                .as_deref()
                .and_then(|p| p.rsplit('/').next());
            let is_node = matches!(executable, Some("node" | "nodejs"));
            // Only identify Vite from its executable argument, not arbitrary flags/text.
            let vite = is_node
                && argv
                    .get(1)
                    .is_some_and(|p| p.ends_with("/vite") || p.ends_with("/vite/bin/vite.js"));
            let runtime = if vite { Some("Vite") } else { executable };
            let project = if is_node {
                port.working_directory
                    .as_deref()
                    .and_then(|p| p.rsplit('/').next())
                    .filter(|s| !s.is_empty())
            } else {
                None
            };
            port.application_name = runtime.map(|name| match project {
                Some(project) => format!("{name} · {project}"),
                None => name.to_owned(),
            });
        }
    }
    ports.into_values().collect()
}
/// Linux reports a path whose file was removed while in use with a
/// " (deleted)" suffix; the path and that fact are kept apart.
fn deleted(path: &str) -> (&str, bool) {
    match path.strip_suffix(" (deleted)") {
        Some(path) => (path, true),
        None => (path, false),
    }
}
fn nonempty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}
fn decode_hex(value: &str) -> Option<String> {
    if value.len() > 16384 || !value.len().is_multiple_of(2) || !value.is_ascii() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect();
    Some(String::from_utf8_lossy(&bytes?).into_owned())
}
#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh ports::tests::session_query"]
    async fn session_query_discovers_over_shared_ssh_without_installing_needle() {
        let server = serde_json::from_value(serde_json::json!({
            "id":uuid::Uuid::new_v4(), "name":"Query SSH fixture", "sshUser":"fixture",
            "sshHost":"127.0.0.1", "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(),
            "agentInstallApproved":false, "clipboardEnabled":false, "browserEnabled":false
        })).unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 1);
        let query = super::query(&owner);
        let ports = query.refresh().await.unwrap();
        assert!(ports.iter().any(|port| port.port == 5432));
        let cached = query.observe(false);
        assert_eq!(cached.data.as_ref(), Some(&ports));
        assert!(!cached.fetching);
        assert!(owner.agent_runtime.snapshot().instances.is_empty());
        assert!(owner.lifecycle.progress.snapshot().is_none());
        registry.invalidate(server.id);
        assert!(query.snapshot().data.is_none());
        registry.shutdown().await;
    }
    use super::*;
    fn detail<const N: usize>(pid: u32, values: [&str; N]) -> String {
        format!(
            "{}|{}",
            pid,
            values
                .map(|v| v
                    .as_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>())
                .join("|")
        )
    }
    #[test]
    fn deleted_paths_lose_the_suffix_and_say_so() {
        // A preview server outlives its build folder, and its binary is upgraded.
        let record = |cwd: &str| {
            format!(
                "LISTEN 0 128 127.0.0.1:7070 *:* users:((\"node\",pid=310,fd=19))\n__NEWPORT_PROCESS_DETAILS__\n{}\n",
                detail(
                    310,
                    ["/usr/bin/node (deleted)\n", cwd, "node\0serve.js\0", "ci\n"]
                )
            )
        };
        let port = &parse_ports(&record("/tmp/build/preview (deleted)\n"))[0];
        assert_eq!(
            port.working_directory.as_deref(),
            Some("/tmp/build/preview")
        );
        assert!(port.working_directory_deleted);
        assert_eq!(port.executable.as_deref(), Some("/usr/bin/node"));
        assert!(port.executable_deleted);
        // The suffix no longer leaks into the name built from the folder.
        assert_eq!(port.application_name.as_deref(), Some("node · preview"));
        assert_eq!(
            serde_json::to_value(port).unwrap()["workingDirectoryDeleted"],
            true
        );
        // An ordinary folder carries no flag at all.
        let json = serde_json::to_value(&parse_ports(&record("/tmp/build/preview\n"))[0]).unwrap();
        assert!(json.get("workingDirectoryDeleted").is_none());
    }
    #[test]
    fn names_vite_from_its_entry_point_not_the_thread_name() {
        // Node reports its main thread's name; the entry point says what runs.
        let record = |entry: &str| {
            format!(
                "LISTEN 0 511 0.0.0.0:4173 *:* users:((\"MainThread\",pid=520,fd=21))\n__NEWPORT_PROCESS_DETAILS__\n{}\n",
                detail(
                    520,
                    [
                        "/opt/node/bin/node\n",
                        "/srv/site\n",
                        &format!("node\0{entry}\0--port\x004173\0"),
                        "www\n"
                    ]
                )
            )
        };
        let vite = &parse_ports(&record("/srv/site/node_modules/.bin/vite"))[0];
        assert_eq!(vite.application_name.as_deref(), Some("Vite · site"));
        assert_eq!(vite.process_name.as_deref(), Some("MainThread"));
        assert_eq!(vite.user.as_deref(), Some("www"));
        assert_eq!(vite.executable.as_deref(), Some("/opt/node/bin/node"));
        assert_eq!(
            vite.arguments,
            ["node", "/srv/site/node_modules/.bin/vite", "--port", "4173"]
        );
        // Mentioning vite anywhere else does not make a process Vite.
        let script = &parse_ports(&record("/srv/site/scripts/serve-vite-assets.js"))[0];
        assert_eq!(script.application_name.as_deref(), Some("node · site"));
    }
    #[test]
    fn incomplete_or_invalid_metadata_preserves_listener() {
        let base = "LISTEN 0 511 0.0.0.0:5174 *:* users:((\"MainThread\",pid=9,fd=25))\n__NEWPORT_PROCESS_DETAILS__\n";
        for details in ["9||||", "9|zz|||", "9|é|||", "10|6e6f6465|||"] {
            let ports = parse_ports(&format!("{base}{details}\n"));
            assert_eq!(ports.len(), 1);
            assert_eq!(ports[0].application_name, None);
            assert_eq!(ports[0].pid, Some(9));
        }
    }
    #[test]
    fn preserves_spaces_and_does_not_misidentify_command_arguments() {
        let data = format!(
            "LISTEN 0 511 0.0.0.0:5174 *:* users:((\"MainThread\",pid=9,fd=25))\n__NEWPORT_PROCESS_DETAILS__\n{}",
            detail(
                9,
                [
                    "/usr/bin/node\n",
                    "/srv/my project\n",
                    "node\0server.js\0--description=vite\0",
                    "alice\n"
                ]
            )
        );
        let port = &parse_ports(&data)[0];
        assert_eq!(port.application_name.as_deref(), Some("node · my project"));
        assert_eq!(port.working_directory.as_deref(), Some("/srv/my project"));
    }
    #[test]
    fn discovers_ss_ipv6_and_deduplicates() {
        let p = parse_ports(
            "LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=45,fd=3))\nLISTEN 0 128 [::]:22 [::]:*\nLISTEN 0 128 [::1]:8080 [::]:*",
        );
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].process_name.as_deref(), Some("sshd"));
        assert_eq!(p[0].pid, Some(45));
        assert_eq!(p[2].address, "::1");
    }
    #[test]
    fn discovers_netstat() {
        let p = parse_ports("tcp 0 0 127.0.0.1:5432 0.0.0.0:* LISTEN 12/postgres");
        assert_eq!(p[0].port, 5432);
        assert_eq!(p[0].process_name.as_deref(), Some("postgres"));
    }
    #[test]
    fn keeps_richer_details_and_distinct_bindings() {
        let ports = parse_ports(
            "LISTEN 0 128 127.0.0.1:8080 *:*\nLISTEN 0 128 127.0.0.1:8080 *:* users:((\"node\",pid=20,fd=3))\nLISTEN 0 128 10.0.0.2:8080 *:* users:((\"other\",pid=21,fd=3))",
        );
        assert_eq!(ports.len(), 2);
        let local = ports.iter().find(|p| p.address == "127.0.0.1").unwrap();
        assert_eq!(local.process_name.as_deref(), Some("node"));
        assert_eq!(local.pid, Some(20));
    }
    #[test]
    fn identifies_docker_bindings_without_guessing_processes() {
        let ports = parse_ports(
            r#"LISTEN 0 128 0.0.0.0:8080 *:*
__NEWPORT_DOCKER_PORTS__
{"name":"web","ports":"0.0.0.0:8080->80/tcp, [::]:8080->80/tcp, 80/tcp, 0.0.0.0:5353->53/udp"}
{"name":"workers","ports":"127.0.0.1:9000-9001->9000-9001/tcp"}
not json
"#,
        );
        assert_eq!(ports.len(), 4);
        assert_eq!(ports[0].container_name.as_deref(), Some("web"));
        assert_eq!(ports[0].process_name, None);
        assert_eq!(ports[1].address, "::");
        assert_eq!(ports[3].port, 9001);
    }
    #[test]
    fn reports_project_roots_from_process_details() {
        let data = format!(
            "LISTEN 0 511 127.0.0.1:3000 *:* users:((\"node\",pid=7,fd=25))\nLISTEN 0 511 127.0.0.1:3001 *:* users:((\"node\",pid=8,fd=25))\n__NEWPORT_PROCESS_DETAILS__\n{}\n{}\n",
            detail(
                7,
                [
                    "/usr/bin/node\n",
                    "/srv/mono/apps/web",
                    "node\0server.js\0",
                    "alice\n",
                    "/srv/mono",
                    ".git"
                ]
            ),
            detail(
                8,
                [
                    "/usr/bin/node\n",
                    "/srv/loose",
                    "node\0server.js\0",
                    "alice\n",
                    "",
                    ""
                ]
            )
        );
        let ports = parse_ports(&data);
        assert_eq!(ports[0].project_root.as_deref(), Some("/srv/mono"));
        assert_eq!(ports[0].project_marker.as_deref(), Some(".git"));
        assert_eq!(ports[1].project_root, None);
        assert_eq!(ports[1].project_marker, None);
    }
    #[test]
    fn identifies_compose_projects() {
        let ports = parse_ports(
            r#"LISTEN 0 128 0.0.0.0:5432 *:*
__NEWPORT_DOCKER_PORTS__
{"name":"shop-db-1","ports":"0.0.0.0:5432->5432/tcp","image":"postgres:16","project":"shop","service":"db","workdir":"/srv/shop"}
{"name":"solo","ports":"127.0.0.1:6379->6379/tcp","image":"redis","project":"","service":"","workdir":""}
"#,
        );
        let db = ports.iter().find(|p| p.port == 5432).unwrap();
        assert_eq!(db.image.as_deref(), Some("postgres:16"));
        assert_eq!(db.compose_project.as_deref(), Some("shop"));
        assert_eq!(db.compose_service.as_deref(), Some("db"));
        assert_eq!(db.compose_working_dir.as_deref(), Some("/srv/shop"));
        assert_eq!(db.project_root.as_deref(), Some("/srv/shop"));
        assert_eq!(db.project_marker.as_deref(), Some("compose"));
        let solo = ports.iter().find(|p| p.port == 6379).unwrap();
        assert_eq!(solo.compose_project, None);
        assert_eq!(solo.project_root, None);
    }
    fn hex(value: &str) -> String {
        value.bytes().map(|b| format!("{b:02x}")).collect()
    }
    #[test]
    fn worktrees_of_one_repository_share_its_root() {
        let listeners = "LISTEN 0 511 127.0.0.1:3000 *:* users:((\"node\",pid=1,fd=3))\nLISTEN 0 511 127.0.0.1:3001 *:* users:((\"node\",pid=2,fd=3))\nLISTEN 0 511 127.0.0.1:3002 *:* users:((\"node\",pid=3,fd=3))\n";
        let main = detail(
            1,
            [
                "/usr/bin/node",
                "/srv/shop/web",
                "node\0a.js\0",
                "dev",
                "/srv/shop",
                ".git",
                "/srv/shop",
                "main",
                "1",
            ],
        );
        let linked = detail(
            2,
            [
                "/usr/bin/node",
                "/srv/shop-login/web",
                "node\0a.js\0",
                "dev",
                "/srv/shop",
                ".git",
                "/srv/shop-login",
                "feat/login",
                "0",
            ],
        );
        let detached = detail(
            3,
            [
                "/usr/bin/node",
                "/srv/shop-old",
                "node\0a.js\0",
                "dev",
                "/srv/shop",
                ".git",
                "/srv/shop-old",
                "0123456",
                "0",
            ],
        );
        let ports = parse_ports(&format!(
            "{listeners}__NEWPORT_PROCESS_DETAILS__\n{main}\n{linked}\n{detached}\n"
        ));
        assert!(
            ports
                .iter()
                .all(|p| p.project_root.as_deref() == Some("/srv/shop"))
        );
        assert_eq!(ports[0].worktree_path.as_deref(), Some("/srv/shop"));
        assert_eq!(ports[0].worktree_branch.as_deref(), Some("main"));
        assert_eq!(ports[0].worktree_main, Some(true));
        assert_eq!(ports[1].worktree_path.as_deref(), Some("/srv/shop-login"));
        assert_eq!(ports[1].worktree_branch.as_deref(), Some("feat/login"));
        assert_eq!(ports[1].worktree_main, Some(false));
        assert_eq!(ports[2].worktree_branch.as_deref(), Some("0123456"));
        assert_eq!(ports[2].worktree_main, Some(false));
    }
    #[test]
    fn roots_without_git_carry_no_worktree() {
        let data = format!(
            "LISTEN 0 511 127.0.0.1:3000 *:* users:((\"node\",pid=1,fd=3))\n__NEWPORT_PROCESS_DETAILS__\n{}\n",
            detail(
                1,
                [
                    "/usr/bin/node",
                    "/srv/app",
                    "node\0a.js\0",
                    "dev",
                    "/srv/app",
                    "package.json",
                    "",
                    "",
                    ""
                ]
            )
        );
        let port = &parse_ports(&data)[0];
        assert_eq!(port.project_marker.as_deref(), Some("package.json"));
        assert_eq!(port.worktree_path, None);
        assert_eq!(port.worktree_branch, None);
        assert_eq!(port.worktree_main, None);
    }
    #[test]
    fn compose_projects_inside_a_worktree_join_its_repository() {
        let row = |name: &str, port: u16, workdir: &str| {
            format!(
                r#"{{"name":"{name}","ports":"127.0.0.1:{port}->{port}/tcp","image":"postgres","project":"{name}","service":"db","workdir":"{workdir}"}}"#
            )
        };
        let data = format!(
            "__NEWPORT_DOCKER_PORTS__\n{}\n{}\n__NEWPORT_COMPOSE_ROOTS__\n{}|{}|{}|{}|{}|{}\n{}|{}|{}|||\n__NEWPORT_PROCESS_DETAILS__\n",
            row("login", 5432, "/srv/shop-login/deploy"),
            row("plain", 5433, "/srv/plain"),
            hex("/srv/shop-login/deploy"),
            hex("/srv/shop"),
            hex(".git"),
            hex("/srv/shop-login"),
            hex("feat/login"),
            hex("0"),
            hex("/srv/plain"),
            hex("/srv/plain"),
            hex("compose.yaml"),
        );
        let ports = parse_ports(&data);
        let login = ports.iter().find(|p| p.port == 5432).unwrap();
        assert_eq!(login.project_root.as_deref(), Some("/srv/shop"));
        assert_eq!(login.project_marker.as_deref(), Some(".git"));
        assert_eq!(login.worktree_path.as_deref(), Some("/srv/shop-login"));
        assert_eq!(login.worktree_branch.as_deref(), Some("feat/login"));
        assert_eq!(login.worktree_main, Some(false));
        assert_eq!(
            login.compose_working_dir.as_deref(),
            Some("/srv/shop-login/deploy")
        );
        // A project file is no better than the Compose directory itself.
        let plain = ports.iter().find(|p| p.port == 5433).unwrap();
        assert_eq!(plain.project_root.as_deref(), Some("/srv/plain"));
        assert_eq!(plain.project_marker.as_deref(), Some("compose"));
        assert_eq!(plain.worktree_path, None);
    }
    #[test]
    fn missing_docker_access_keeps_listeners() {
        let ports = parse_ports("LISTEN 0 128 0.0.0.0:22 *:*\n__NEWPORT_DOCKER_PORTS__\n");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].process_name, None);
        assert_eq!(ports[0].container_name, None);
    }
}
