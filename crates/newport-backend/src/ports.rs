//! Listener discovery through the server's agent, which reads the sockets
//! from /proc (no ss or netstat), and attribution of each listener to a
//! process, container and project.
use crate::{monitor, server_session::ServerSession};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

// Walking every process's descriptors and project roots on a busy host.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// The key is (server session incarnation, listening ports). The session owns
/// the cache across windows/navigation; replacing credentials creates a new key.
pub(crate) fn query(owner: &Arc<ServerSession>) -> &crate::query::Query<Vec<DiscoveredPort>> {
    let query = owner.ports.get_or_init(|| {
        let weak = Arc::downgrade(owner);
        crate::query::Query::new(
            crate::query::Policy {
                stale_after: Duration::from_secs(30),
                retry_after: Duration::from_secs(10),
                timeout: Duration::from_secs(35),
                observation_lease: Duration::from_secs(5),
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
                        result = discover(&owner) => result,
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

/// One reading from the agent, uncached.
pub async fn discover(owner: &Arc<ServerSession>) -> Result<Vec<DiscoveredPort>, String> {
    let request = serde_json::json!({"op": "ports"});
    let reply = match monitor::ask(owner, &request, REPLY_TIMEOUT).await {
        Err(error) if error.starts_with("Unknown monitor request") => {
            return Err(update_needle(owner).await);
        }
        reply => reply?,
    };
    let reply: Reply =
        serde_json::from_value(reply).map_err(|e| format!("Invalid ports reply: {e}"))?;
    Ok(assemble(reply))
}

/// Shown while Needle is replaced; the page reads it as progress, not failure.
pub const UPDATING: &str =
    "Updating Needle on this server. Ports appear here as soon as it finishes.";

/// The installed Needle predates port discovery. Replace it in the
/// background, as a newer release would be; the next refresh asks the new
/// one. Returns what to show until then.
async fn update_needle(owner: &Arc<ServerSession>) -> String {
    use crate::agent_lifecycle::RequiredUpdate;
    let failed = |error: &str| {
        format!(
            "Port discovery needs a newer Needle on this server, and updating it didn’t work: \
             {error} Choose Update Needle in Integrations to try again."
        )
    };
    match owner.lifecycle.required_update() {
        Some(RequiredUpdate::Running) => return UPDATING.into(),
        Some(RequiredUpdate::Failed { error, at }) if at.elapsed() < Duration::from_secs(600) => {
            return failed(&error);
        }
        _ => {}
    }
    let lease = match owner.acquire(owner.server()).await {
        Ok(lease) => lease,
        Err(error) => return failed(&error),
    };
    tokio::spawn(async move {
        if let Err(RequiredUpdate::Failed { error, .. }) = lease.update_needle_for_feature().await {
            log::warn!("Needle update for port discovery did not complete: {error}");
        }
    });
    UPDATING.into()
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

/// The agent's `ports` reply: what the server shows, not yet attributed.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    listeners: Vec<Listener>,
    #[serde(default)]
    processes: Vec<Process>,
    #[serde(default)]
    containers: Vec<Container>,
}
#[derive(Debug, Default, Deserialize)]
struct Listener {
    address: String,
    port: u16,
    /// The socket's owner, known even when its process is not visible.
    user: Option<String>,
    pid: Option<u32>,
    process: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Process {
    pid: u32,
    executable: Option<String>,
    working_directory: Option<String>,
    #[serde(default)]
    arguments: Vec<String>,
    user: Option<String>,
    project: Option<Project>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Container {
    name: String,
    image: Option<String>,
    project: Option<String>,
    service: Option<String>,
    workdir: Option<String>,
    #[serde(default)]
    bindings: Vec<Binding>,
    /// The project around the Compose working directory.
    workdir_project: Option<Project>,
}
#[derive(Debug, Default, Deserialize)]
struct Binding {
    address: String,
    port: u16,
}
/// A directory's project as the server resolved it.
#[derive(Debug, Default, Deserialize)]
struct Project {
    root: String,
    marker: String,
    worktree: Option<String>,
    branch: Option<String>,
    main: Option<bool>,
}
impl Project {
    /// Version-control roots (and an explicit `.projectile`) name a project
    /// more precisely than the Compose directory inside them.
    fn is_repository(&self) -> bool {
        matches!(
            self.marker.as_str(),
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
    }
    fn apply(&self, port: &mut DiscoveredPort) {
        port.project_root = nonempty(&self.root);
        port.project_marker = nonempty(&self.marker);
        port.worktree_path = self.worktree.as_deref().and_then(nonempty);
        port.worktree_branch = self.branch.as_deref().and_then(nonempty);
        port.worktree_main = self.main;
    }
}

/// The listener's key: the IPv4 and IPv6 wildcards on one port are one
/// service (Docker publishes both, and dual-stack servers bind both).
fn key(port: u16, address: &str) -> (u16, String) {
    match address {
        "0.0.0.0" | "::" => (port, "*".into()),
        address => (port, address.into()),
    }
}

fn assemble(reply: Reply) -> Vec<DiscoveredPort> {
    let mut ports: BTreeMap<(u16, String), DiscoveredPort> = BTreeMap::new();
    for listener in reply.listeners.into_iter().filter(|l| l.port > 0) {
        let address = listener.address.trim_matches(['[', ']']).to_owned();
        let entry = ports
            .entry(key(listener.port, &address))
            .or_insert(DiscoveredPort {
                port: listener.port,
                address,
                ..Default::default()
            });
        if entry.process_name.is_none() && listener.process.is_some() {
            entry.process_name = listener.process;
            entry.pid = listener.pid;
        }
        if entry.user.is_none() {
            entry.user = listener.user.as_deref().and_then(nonempty);
        }
    }
    for container in reply.containers {
        let compose_working_dir = container.workdir.as_deref().and_then(nonempty);
        for binding in container.bindings.iter().filter(|b| b.port > 0) {
            let address = binding.address.trim_matches(['[', ']']).to_owned();
            let entry = ports
                .entry(key(binding.port, &address))
                .or_insert(DiscoveredPort {
                    port: binding.port,
                    address,
                    ..Default::default()
                });
            entry.container_name = Some(container.name.clone());
            entry.image = container.image.as_deref().and_then(nonempty);
            entry.compose_project = container.project.as_deref().and_then(nonempty);
            entry.compose_service = container.service.as_deref().and_then(nonempty);
            entry.compose_working_dir = compose_working_dir.clone();
            if let Some(workdir) = &compose_working_dir {
                match container
                    .workdir_project
                    .as_ref()
                    .filter(|project| project.is_repository())
                {
                    Some(project) => project.apply(entry),
                    None => {
                        entry.project_root = Some(workdir.clone());
                        entry.project_marker = Some("compose".into());
                    }
                }
            }
        }
    }
    for process in reply.processes {
        for port in ports.values_mut().filter(|p| p.pid == Some(process.pid)) {
            let (executable, executable_deleted) =
                deleted(process.executable.as_deref().unwrap_or_default());
            let (directory, directory_deleted) =
                deleted(process.working_directory.as_deref().unwrap_or_default());
            port.executable = nonempty(executable);
            port.executable_deleted = executable_deleted;
            port.working_directory = nonempty(directory);
            port.working_directory_deleted = directory_deleted;
            if let Some(user) = process.user.as_deref().and_then(nonempty) {
                port.user = Some(user);
            }
            // A container's Compose directory outranks its proxy process's directory.
            if port.project_root.is_none()
                && let Some(project) = &process.project
            {
                project.apply(port);
            }
            let argv: Vec<&str> = process
                .arguments
                .iter()
                .map(String::as_str)
                .filter(|s| !s.is_empty())
                .collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn ports(reply: Value) -> Vec<DiscoveredPort> {
        assemble(serde_json::from_value(reply).unwrap())
    }
    fn listener(address: &str, port: u16, pid: u32, process: &str) -> Value {
        json!({"address": address, "port": port, "pid": pid, "process": process})
    }
    fn node(pid: u32, cwd: &str, entry: &str) -> Value {
        json!({"pid": pid, "executable": "/usr/bin/node", "workingDirectory": cwd,
            "arguments": ["node", entry], "user": "dev"})
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- ports::tests::session_query"]
    async fn session_query_without_the_agent_says_it_is_needed() {
        let server = serde_json::from_value(json!({
            "id":uuid::Uuid::new_v4(), "name":"Query SSH fixture", "sshUser":"fixture",
            "sshHost":"127.0.0.1", "sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(),
            "agentInstallApproved":false, "clipboardEnabled":false, "browserEnabled":false
        })).unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 1);
        let error = query(&owner).refresh().await.unwrap_err();
        assert!(
            error.contains("Port discovery needs the Newport agent"),
            "{error}"
        );
        // Discovery never installs the agent on its own.
        assert!(owner.lifecycle.progress.snapshot().is_none());
        registry.invalidate(server.id);
        assert!(query(&owner).snapshot().data.is_none());
        registry.shutdown().await;
    }
    #[test]
    fn deleted_paths_lose_the_suffix_and_say_so() {
        // A preview server outlives its build folder, and its binary is upgraded.
        let record = |cwd: &str| {
            ports(json!({
                "listeners": [listener("127.0.0.1", 7070, 310, "node")],
                "processes": [{"pid": 310, "executable": "/usr/bin/node (deleted)",
                    "workingDirectory": cwd, "arguments": ["node", "serve.js"], "user": "ci"}]
            }))
        };
        let port = &record("/tmp/build/preview (deleted)")[0];
        assert_eq!(
            port.working_directory.as_deref(),
            Some("/tmp/build/preview")
        );
        assert!(port.working_directory_deleted);
        assert_eq!(port.executable.as_deref(), Some("/usr/bin/node"));
        assert!(port.executable_deleted);
        // The suffix does not leak into the name built from the folder.
        assert_eq!(port.application_name.as_deref(), Some("node · preview"));
        assert_eq!(
            serde_json::to_value(port).unwrap()["workingDirectoryDeleted"],
            true
        );
        // An ordinary folder carries no flag at all.
        let json = serde_json::to_value(&record("/tmp/build/preview")[0]).unwrap();
        assert!(json.get("workingDirectoryDeleted").is_none());
    }
    #[test]
    fn names_vite_from_its_entry_point_not_the_thread_name() {
        // Node reports its main thread's name; the entry point says what runs.
        let record = |entry: &str| {
            ports(json!({
                "listeners": [listener("0.0.0.0", 4173, 520, "MainThread")],
                "processes": [{"pid": 520, "executable": "/opt/node/bin/node",
                    "workingDirectory": "/srv/site", "arguments": ["node", entry, "--port", "4173"],
                    "user": "www"}]
            }))
        };
        let vite = &record("/srv/site/node_modules/.bin/vite")[0];
        assert_eq!(vite.application_name.as_deref(), Some("Vite · site"));
        assert_eq!(vite.process_name.as_deref(), Some("MainThread"));
        assert_eq!(vite.user.as_deref(), Some("www"));
        assert_eq!(vite.executable.as_deref(), Some("/opt/node/bin/node"));
        assert_eq!(
            vite.arguments,
            ["node", "/srv/site/node_modules/.bin/vite", "--port", "4173"]
        );
        // Mentioning vite anywhere else does not make a process Vite.
        let script = &record("/srv/site/scripts/serve-vite-assets.js")[0];
        assert_eq!(script.application_name.as_deref(), Some("node · site"));
    }
    #[test]
    fn missing_process_details_preserve_the_listener() {
        for processes in [
            json!([]),
            json!([{"pid": 9}]),
            json!([{"pid": 10, "executable": "/usr/bin/node"}]),
        ] {
            let found = ports(json!({
                "listeners": [listener("0.0.0.0", 5174, 9, "MainThread")],
                "processes": processes
            }));
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].application_name, None);
            assert_eq!(found[0].pid, Some(9));
        }
    }
    #[test]
    fn preserves_spaces_and_does_not_misidentify_command_arguments() {
        let mut process = node(9, "/srv/my project", "server.js");
        process["arguments"] = json!(["node", "server.js", "--description=vite"]);
        let port = &ports(json!({
            "listeners": [listener("0.0.0.0", 5174, 9, "MainThread")],
            "processes": [process]
        }))[0];
        assert_eq!(port.application_name.as_deref(), Some("node · my project"));
        assert_eq!(port.working_directory.as_deref(), Some("/srv/my project"));
    }
    #[test]
    fn keeps_richer_details_and_distinct_bindings() {
        let found = ports(json!({"listeners": [
            {"address": "127.0.0.1", "port": 8080},
            listener("127.0.0.1", 8080, 20, "node"),
            listener("10.0.0.2", 8080, 21, "other"),
            {"address": "::", "port": 22, "user": "root"},
        ]}));
        assert_eq!(found.len(), 3);
        let local = found.iter().find(|p| p.address == "127.0.0.1").unwrap();
        assert_eq!(local.process_name.as_deref(), Some("node"));
        assert_eq!(local.pid, Some(20));
        // Another account's service still says whose it is.
        let ssh = found.iter().find(|p| p.port == 22).unwrap();
        assert_eq!(ssh.user.as_deref(), Some("root"));
        assert_eq!(ssh.pid, None);
    }
    #[test]
    fn a_server_on_both_wildcards_is_one_port_and_loopback_stays_apart() {
        let found = ports(json!({
            "listeners": [
                listener("0.0.0.0", 80, 9, "nginx"),
                listener("[::]", 80, 9, "nginx"),
                listener("127.0.0.1", 80, 10, "node"),
            ]
        }));
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .any(|p| p.address == "0.0.0.0" && p.process_name.as_deref() == Some("nginx"))
        );
        assert!(
            found
                .iter()
                .any(|p| p.address == "127.0.0.1" && p.process_name.as_deref() == Some("node"))
        );
    }
    #[test]
    fn identifies_docker_bindings_without_guessing_processes() {
        let found = ports(json!({
            "listeners": [{"address": "0.0.0.0", "port": 8080}],
            "containers": [
                {"name": "web", "bindings": [{"address": "0.0.0.0", "port": 8080}, {"address": "::", "port": 8080}]},
                {"name": "workers", "bindings": [{"address": "127.0.0.1", "port": 9000}, {"address": "127.0.0.1", "port": 9001}]}
            ]
        }));
        // Its IPv4 and IPv6 wildcard bindings are one row.
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].container_name.as_deref(), Some("web"));
        assert_eq!(found[0].address, "0.0.0.0");
        assert_eq!(found[0].process_name, None);
        assert_eq!(found[2].port, 9001);
    }
    #[test]
    fn reports_project_roots_from_processes() {
        let mut mono = node(7, "/srv/mono/apps/web", "server.js");
        mono["project"] = json!({"root": "/srv/mono", "marker": ".git"});
        let found = ports(json!({
            "listeners": [listener("127.0.0.1", 3000, 7, "node"), listener("127.0.0.1", 3001, 8, "node")],
            "processes": [mono, node(8, "/srv/loose", "server.js")]
        }));
        assert_eq!(found[0].project_root.as_deref(), Some("/srv/mono"));
        assert_eq!(found[0].project_marker.as_deref(), Some(".git"));
        assert_eq!(found[1].project_root, None);
        assert_eq!(found[1].project_marker, None);
    }
    #[test]
    fn identifies_compose_projects() {
        let found = ports(json!({
            "listeners": [{"address": "0.0.0.0", "port": 5432}],
            "containers": [
                {"name": "shop-db-1", "image": "postgres:16", "project": "shop", "service": "db",
                    "workdir": "/srv/shop", "bindings": [{"address": "0.0.0.0", "port": 5432}]},
                {"name": "solo", "image": "redis", "bindings": [{"address": "127.0.0.1", "port": 6379}]}
            ]
        }));
        let db = found.iter().find(|p| p.port == 5432).unwrap();
        assert_eq!(db.image.as_deref(), Some("postgres:16"));
        assert_eq!(db.compose_project.as_deref(), Some("shop"));
        assert_eq!(db.compose_service.as_deref(), Some("db"));
        assert_eq!(db.compose_working_dir.as_deref(), Some("/srv/shop"));
        assert_eq!(db.project_root.as_deref(), Some("/srv/shop"));
        assert_eq!(db.project_marker.as_deref(), Some("compose"));
        let solo = found.iter().find(|p| p.port == 6379).unwrap();
        assert_eq!(solo.compose_project, None);
        assert_eq!(solo.project_root, None);
    }
    #[test]
    fn worktrees_of_one_repository_share_its_root() {
        let checkout = |pid: u32, cwd: &str, worktree: &str, branch: &str, main: bool| {
            let mut process = node(pid, cwd, "a.js");
            process["project"] = json!({"root": "/srv/shop", "marker": ".git",
                "worktree": worktree, "branch": branch, "main": main});
            process
        };
        let found = ports(json!({
            "listeners": [listener("127.0.0.1", 3000, 1, "node"), listener("127.0.0.1", 3001, 2, "node"), listener("127.0.0.1", 3002, 3, "node")],
            "processes": [
                checkout(1, "/srv/shop/web", "/srv/shop", "main", true),
                checkout(2, "/srv/shop-login/web", "/srv/shop-login", "feat/login", false),
                checkout(3, "/srv/shop-old", "/srv/shop-old", "0123456", false),
            ]
        }));
        assert!(
            found
                .iter()
                .all(|p| p.project_root.as_deref() == Some("/srv/shop"))
        );
        assert_eq!(found[0].worktree_path.as_deref(), Some("/srv/shop"));
        assert_eq!(found[0].worktree_branch.as_deref(), Some("main"));
        assert_eq!(found[0].worktree_main, Some(true));
        assert_eq!(found[1].worktree_path.as_deref(), Some("/srv/shop-login"));
        assert_eq!(found[1].worktree_branch.as_deref(), Some("feat/login"));
        assert_eq!(found[1].worktree_main, Some(false));
        assert_eq!(found[2].worktree_branch.as_deref(), Some("0123456"));
        assert_eq!(found[2].worktree_main, Some(false));
    }
    #[test]
    fn roots_without_git_carry_no_worktree() {
        let mut process = node(1, "/srv/app", "a.js");
        process["project"] = json!({"root": "/srv/app", "marker": "package.json"});
        let port = &ports(json!({
            "listeners": [listener("127.0.0.1", 3000, 1, "node")],
            "processes": [process]
        }))[0];
        assert_eq!(port.project_marker.as_deref(), Some("package.json"));
        assert_eq!(port.worktree_path, None);
        assert_eq!(port.worktree_branch, None);
        assert_eq!(port.worktree_main, None);
    }
    #[test]
    fn compose_projects_inside_a_worktree_join_its_repository() {
        let found = ports(json!({
            "listeners": [],
            "containers": [
                {"name": "login", "image": "postgres", "project": "login", "service": "db",
                    "workdir": "/srv/shop-login/deploy",
                    "bindings": [{"address": "127.0.0.1", "port": 5432}],
                    "workdirProject": {"root": "/srv/shop", "marker": ".git",
                        "worktree": "/srv/shop-login", "branch": "feat/login", "main": false}},
                {"name": "plain", "image": "postgres", "project": "plain", "service": "db",
                    "workdir": "/srv/plain",
                    "bindings": [{"address": "127.0.0.1", "port": 5433}],
                    "workdirProject": {"root": "/srv/plain", "marker": "compose.yaml"}}
            ]
        }));
        let login = found.iter().find(|p| p.port == 5432).unwrap();
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
        let plain = found.iter().find(|p| p.port == 5433).unwrap();
        assert_eq!(plain.project_root.as_deref(), Some("/srv/plain"));
        assert_eq!(plain.project_marker.as_deref(), Some("compose"));
        assert_eq!(plain.worktree_path, None);
    }
    #[test]
    fn missing_docker_keeps_listeners() {
        let found = ports(json!({"listeners": [{"address": "0.0.0.0", "port": 22}]}));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].process_name, None);
        assert_eq!(found[0].container_name, None);
    }
}
