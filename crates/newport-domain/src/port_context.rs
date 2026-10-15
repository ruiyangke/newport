//! Observed service identity and useful process context for discovered listeners.
//!
//! Mirrors the Tauri `portDetails` and `portProjects` rules. Never infer a service
//! from a port number or thread name, and never tokenize a reconstructed command.
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detail {
    pub label: &'static str,
    pub value: String,
}

fn text<'a>(port: &'a Value, field: &str) -> Option<&'a str> {
    port[field].as_str().filter(|value| !value.is_empty())
}

pub fn owner(port: &Value) -> Option<&str> {
    ["containerName", "applicationName", "processName"]
        .iter()
        .find_map(|field| text(port, field))
}

pub fn name(port: &Value) -> Option<&str> {
    text(port, "composeService").or_else(|| owner(port))
}

fn executable(port: &Value) -> &str {
    port["executable"]
        .as_str()
        .map(|value| value.strip_suffix(" (deleted)").unwrap_or(value))
        .and_then(|value| value.rsplit('/').next())
        .or_else(|| port["processName"].as_str())
        .unwrap_or_default()
}

fn arguments(port: &Value) -> Vec<&str> {
    port["arguments"]
        .as_array()
        .map(|values| {
            // Keep positions intact even for malformed input. Removing a value
            // could make a later argument look like a preceding option's value.
            values
                .iter()
                .skip(1)
                .map(|value| value.as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

fn python(exe: &str) -> bool {
    exe == "python"
        || exe.strip_prefix("python").is_some_and(|version| {
            version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
        })
}

fn workspace(directory: &str) -> Option<&str> {
    let value = directory.trim_end_matches('/');
    if !value.starts_with('/')
        || matches!(value, "/root" | "/home")
        || value
            .strip_prefix("/home/")
            .is_some_and(|rest| !rest.contains('/'))
        || [
            "/usr",
            "/bin",
            "/sbin",
            "/etc",
            "/proc",
            "/sys",
            "/dev",
            "/run",
            "/nix/store",
            "/var/lib",
            "/var/log",
        ]
        .iter()
        .any(|root| value == *root || value.strip_prefix(root).is_some_and(|s| s.starts_with('/')))
    {
        return None;
    }
    Some(value)
}

fn option<'a>(argv: &[&'a str], flag: &str) -> Option<&'a str> {
    let end = argv
        .iter()
        .position(|arg| *arg == "--")
        .unwrap_or(argv.len());
    let options = &argv[..end];
    if let Some(index) = options.iter().position(|arg| *arg == flag) {
        options.get(index + 1).copied()
    } else {
        options
            .iter()
            .find_map(|arg| arg.strip_prefix(flag)?.strip_prefix('='))
    }
}

/// At most two curated details. Known services retain operational paths even
/// under system directories; arbitrary flags, inline code and passwords stay out.
pub fn process_details(port: &Value) -> Vec<Detail> {
    if text(port, "containerName").is_some() {
        return Vec::new();
    }
    let exe = executable(port);
    let argv = arguments(port);
    let mut details = Vec::new();
    let mut add = |label, value: Option<&str>| {
        if let Some(value) = value.filter(|value| {
            !value.is_empty() && !value.starts_with('-') && !value.contains(['\n', '\r'])
        }) {
            details.push(Detail {
                label,
                value: value.to_owned(),
            });
        }
    };
    match exe {
        "postgres" => add("Data directory", option(&argv, "-D")),
        "mysqld" | "mariadbd" => {
            add("Data directory", option(&argv, "--datadir"));
            add("Configuration", option(&argv, "--defaults-file"));
        }
        "nginx" => add("Configuration", option(&argv, "-c")),
        "sshd" => add("Configuration", option(&argv, "-f")),
        "redis-server" => add(
            "Configuration",
            argv.first().copied().filter(|s| s.ends_with(".conf")),
        ),
        "caddy" => add("Configuration", option(&argv, "--config")),
        _ => {
            if python(exe) {
                let module = option(&argv, "-m").filter(|value| {
                    !value.is_empty()
                        && value
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
                });
                if module.is_some() {
                    add("Module", module);
                } else {
                    add(
                        "Script",
                        argv.first().copied().filter(|s| s.ends_with(".py")),
                    );
                }
            } else if exe == "java" {
                add("Application", option(&argv, "-jar"));
            } else if exe == "dotnet" {
                add(
                    "Application",
                    argv.first().copied().filter(|s| s.ends_with(".dll")),
                );
            } else if matches!(exe, "ruby" | "php") {
                add(
                    "Script",
                    argv.first()
                        .copied()
                        .filter(|s| s.ends_with(".rb") || s.ends_with(".php")),
                );
            }
            if !exe.is_empty() && exe != "MainThread" {
                let runtime = python(exe)
                    || matches!(
                        exe,
                        "node"
                            | "nodejs"
                            | "bun"
                            | "deno"
                            | "ruby"
                            | "php"
                            | "java"
                            | "dotnet"
                            | "uvicorn"
                            | "gunicorn"
                    );
                add(
                    if runtime {
                        "Project directory"
                    } else {
                        "Working directory"
                    },
                    text(port, "workingDirectory").and_then(workspace),
                );
            }
        }
    }
    details.truncate(2);
    details
}

fn service(exe: &str) -> Option<&'static str> {
    Some(match exe {
        "postgres" | "postgresql" | "postgis" | "timescaledb" => "Postgres",
        "mysqld" | "mysql" => "MySQL",
        "mariadbd" | "mariadb" => "MariaDB",
        "redis-server" | "redis" | "redis-stack" => "Redis",
        "valkey-server" | "valkey" => "Valkey",
        "mongod" | "mongo" => "MongoDB",
        "memcached" => "Memcached",
        "nginx" => "nginx",
        "caddy" => "Caddy",
        "httpd" | "apache2" => "Apache",
        "traefik" => "Traefik",
        "haproxy" => "HAProxy",
        "envoy" => "Envoy",
        "sshd" | "sshd-session" => "SSH",
        "elasticsearch" => "Elasticsearch",
        "opensearch" => "OpenSearch",
        "clickhouse-server" | "clickhouse" => "ClickHouse",
        "rabbitmq" => "RabbitMQ",
        "nats-server" | "nats" => "NATS",
        "kafka" => "Kafka",
        "minio" => "MinIO",
        "etcd" => "etcd",
        "consul" => "Consul",
        "vault" => "Vault",
        "grafana" | "grafana-server" => "Grafana",
        "prometheus" => "Prometheus",
        "ollama" => "Ollama",
        "code-server" => "code-server",
        "cloudflared" => "Cloudflare Tunnel",
        "tailscaled" => "Tailscale",
        "dnsmasq" | "systemd-resolve" | "systemd-resolved" => "DNS",
        "docker-proxy" => "Docker",
        _ => return None,
    })
}

fn runtime(exe: &str) -> Option<&'static str> {
    Some(match exe {
        "node" | "nodejs" => "Node",
        "bun" => "Bun",
        "deno" => "Deno",
        "ruby" => "Ruby",
        "php" | "php-fpm" => "PHP",
        "java" => "Java",
        "dotnet" => ".NET",
        "beam.smp" => "Erlang",
        _ => return None,
    })
}

fn framework(exe: &str) -> Option<&'static str> {
    Some(match exe {
        "vite" | "vite.js" => "Vite",
        "next" | "next-server" => "Next.js",
        "nuxt" | "nuxi" => "Nuxt",
        "astro" => "Astro",
        "remix" => "Remix",
        "react-router" => "React Router",
        "webpack-dev-server" | "webpack" => "webpack",
        "react-scripts" => "Create React App",
        "storybook" => "Storybook",
        "ng" => "Angular",
        "uvicorn" => "Uvicorn",
        "gunicorn" => "Gunicorn",
        "hypercorn" => "Hypercorn",
        "flask" => "Flask",
        "fastapi" => "FastAPI",
        "streamlit" => "Streamlit",
        "jupyter" | "jupyter-lab" | "jupyter-notebook" => "Jupyter",
        "http.server" => "HTTP server",
        "rails" => "Rails",
        "puma" => "Puma",
        "unicorn" => "Unicorn",
        "artisan" => "Laravel",
        _ => return None,
    })
}

pub fn is_runtime_kind(kind: &str) -> bool {
    matches!(
        kind,
        "Node" | "Bun" | "Deno" | "Ruby" | "PHP" | "Java" | ".NET" | "Erlang" | "Python" | "Docker"
    )
}

/// Service/framework first, runtime second. Only observed executable, individual
/// argv values and image names participate; deduplicated and bounded to 3 tags.
pub fn kinds(port: &Value) -> Vec<&'static str> {
    let mut result = Vec::new();
    let mut add = |kind| {
        if let Some(kind) = kind {
            if !result.contains(&kind) && result.len() < 3 {
                result.push(kind);
            }
        }
    };
    if text(port, "containerName").is_some() {
        if let Some(image) = text(port, "image") {
            // Strip the digest, then registry, then tag. A registry may contain a
            // port, so stripping ':' before '/' would misclassify its image.
            let repository = image.split('@').next().unwrap_or(image);
            let basename = repository.rsplit('/').next().unwrap_or(repository);
            let image = basename.split(':').next().unwrap_or(basename);
            add(service(image)
                .or_else(|| image.rsplit_once('-').and_then(|(base, _)| service(base))));
        }
        add(Some("Docker"));
        return result;
    }
    let exe = executable(port).to_lowercase();
    let argv = arguments(port);
    add(service(&exe));
    let is_python = python(&exe);
    if is_python {
        if let Some(index) = argv.iter().position(|arg| *arg == "-m") {
            add(argv.get(index + 1).and_then(|module| framework(module)));
        }
    }
    for arg in &argv {
        if arg.starts_with('-') {
            continue;
        }
        let base = arg.rsplit('/').next().unwrap_or(arg);
        if base == "manage.py" && argv.contains(&"runserver") {
            add(Some("Django"));
        }
        let stem = [".js", ".cjs", ".mjs"]
            .iter()
            .find_map(|suffix| base.strip_suffix(suffix));
        add(framework(base).or_else(|| stem.and_then(framework)));
    }
    add(framework(&exe));
    add(if is_python {
        Some("Python")
    } else {
        runtime(&exe)
    });
    result
}

pub fn service_name(port: &Value) -> Option<String> {
    text(port, "composeService")
        .or_else(|| text(port, "containerName"))
        .or_else(|| kinds(port).into_iter().find(|kind| !is_runtime_kind(kind)))
        .or_else(|| text(port, "applicationName"))
        .or_else(|| text(port, "processName"))
        .or_else(|| name(port))
        .map(str::to_owned)
}

pub fn forward_name(port: &Value) -> String {
    let name = service_name(port).unwrap_or_default();
    match text(port, "worktreeBranch")
        .filter(|_| text(port, "worktreePath").is_some() && port["worktreeMain"] == false)
    {
        Some(branch) if name.is_empty() => branch.to_owned(),
        Some(branch) => format!("{name} · {branch}"),
        None => name,
    }
}

/// The supporting row line, omitting information already present in its title
/// or parent project. Unabridged operational paths remain available in tooltip.
pub fn service_detail(port: &Value, addresses: &[String], base: Option<&str>) -> Vec<String> {
    let name = service_name(port);
    let mut parts = Vec::new();
    let short = |path: &str| short_path(path, &[text(port, "user")], 40);
    for detail in process_details(port) {
        if !matches!(detail.label, "Project directory" | "Working directory") {
            parts.push(short(&detail.value));
        } else if let Some(relative) =
            base.and_then(|base| detail.value.strip_prefix(base)?.strip_prefix('/'))
        {
            parts.insert(0, format!("{relative}/"));
        } else if Some(detail.value.as_str()) != base {
            parts.insert(0, short(&detail.value));
        }
    }
    let tags = kinds(port);
    for kind in &tags {
        if Some(*kind) != name.as_deref() {
            parts.push((*kind).to_owned());
        }
    }
    if text(port, "composeService").is_none() {
        if let Some(container) =
            text(port, "containerName").filter(|container| Some(*container) != name.as_deref())
        {
            parts.push(container.to_owned());
        }
    }
    if text(port, "containerName").is_none() && !tags.iter().any(|kind| is_runtime_kind(kind)) {
        if let Some(process) = text(port, "processName").filter(|process| {
            Some(process.to_lowercase()) != name.as_ref().map(|s| s.to_lowercase())
        }) {
            parts.push(process.to_owned());
        }
    }
    if addresses
        .iter()
        .any(|address| matches!(address.as_str(), "0.0.0.0" | "::" | "*" | "[::]"))
    {
        parts.push("all interfaces".to_owned());
    }
    if port["workingDirectoryDeleted"] == true {
        parts.insert(0, "folder deleted".to_owned());
    }
    let mut seen: HashSet<String> = name.map(|s| s.to_lowercase()).into_iter().collect();
    parts.retain(|part| seen.insert(part.to_lowercase()));
    parts
}

/// Condense only paths that belong to the same observed process owner.
pub fn tildify(path: &str, users: &[Option<&str>]) -> String {
    let Some(user) = users
        .iter()
        .flatten()
        .copied()
        .find(|user| !user.is_empty())
    else {
        return path.to_owned();
    };
    if users
        .iter()
        .flatten()
        .any(|other| !other.is_empty() && *other != user)
    {
        return path.to_owned();
    }
    let home = if user == "root" {
        "/root".to_owned()
    } else {
        format!("/home/{user}")
    };
    if path == home {
        "~".to_owned()
    } else if let Some(rest) = path
        .strip_prefix(&home)
        .filter(|rest| rest.starts_with('/'))
    {
        format!("~{rest}")
    } else {
        path.to_owned()
    }
}

pub fn short_path(path: &str, users: &[Option<&str>], budget: usize) -> String {
    if !path.starts_with('/') {
        return path.to_owned();
    }
    let tilded = tildify(path, users);
    if tilded.encode_utf16().count() <= budget {
        return tilded;
    }
    let parts: Vec<_> = tilded.split('/').filter(|part| !part.is_empty()).collect();
    let mut kept = parts[parts.len().saturating_sub(2)..].join("/");
    for part in parts[..parts.len().saturating_sub(2)].iter().rev() {
        let next = format!("{part}/{kept}");
        if next.encode_utf16().count().saturating_add(2) > budget {
            break;
        }
        kept = next;
    }
    format!("…/{kept}")
}

pub fn tooltip(port: &Value, addresses: &[String]) -> String {
    let mut lines = Vec::new();
    if let Some(container) = text(port, "containerName") {
        lines.push(format!("Container: {container}"));
    }
    lines.extend(
        process_details(port)
            .into_iter()
            .map(|detail| format!("{}: {}", detail.label, detail.value)),
    );
    if port["workingDirectoryDeleted"] == true {
        lines.push(
            "Its working directory was deleted while it kept running; it may be left over."
                .to_owned(),
        );
    }
    if port["executableDeleted"] == true {
        lines.push(
            "Its program file changed since it started; restart it to run the new one.".to_owned(),
        );
    }
    lines.push(format!("Listening on {}", addresses.join(", ")));
    if let Some(pid) = port["pid"].as_u64().filter(|pid| *pid != 0) {
        lines.push(match text(port, "user") {
            Some(user) => format!("PID {pid} · {user}"),
            None => format!("PID {pid}"),
        });
    }
    if let Some(command) = text(port, "command") {
        lines.push(command.to_owned());
    }
    lines.join("\n")
}

/// Lowercase port-specific search corpus. Callers prepend project identity and
/// append saved-forward names/ports, keeping the Tauri cross-field search order.
pub fn search_text(port: &Value, addresses: &[String]) -> String {
    let mut parts = Vec::new();
    if let Some(number) = port["port"].as_u64() {
        parts.push(number.to_string());
    }
    parts.extend_from_slice(addresses);
    for field in [
        "containerName",
        "composeService",
        "applicationName",
        "processName",
        "user",
    ] {
        if let Some(value) = port[field].as_str() {
            parts.push(value.to_owned());
        }
    }
    if let Some(pid) = port["pid"].as_u64() {
        parts.push(pid.to_string());
    }
    parts.extend(kinds(port).into_iter().map(str::to_owned));
    parts.extend(process_details(port).into_iter().map(|detail| detail.value));
    // Tauri only exposes worktree context when discovery supplied its path.
    if text(port, "worktreePath").is_some() {
        if let Some(value) = port["worktreeBranch"].as_str() {
            parts.push(value.to_owned());
        }
        if let Some(path) = text(port, "worktreePath") {
            let trimmed = path.trim_end_matches('/');
            parts.push(if trimmed.is_empty() { "/" } else { trimmed }.to_owned());
        }
    }
    parts.join(" ").to_lowercase()
}

#[cfg(test)]
mod tests;
