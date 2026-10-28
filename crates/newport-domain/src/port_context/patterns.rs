//! Recognition catalog for executable names, framework arguments and Docker images.
//!
//! Inputs are normalized by port_context before matching. Keep matches exact:
//! publisher-qualified image rules precede image aliases and service fallbacks.
//! These rules return service metadata, never container or Compose display names.
//! Do not infer identity from port numbers, user labels or command substrings.

pub(super) fn service(exe: &str) -> Option<&'static str> {
    Some(match exe {
        "postgres" | "postgresql" | "postgis" | "timescaledb" => "Postgres",
        "mysqld" | "mysql" => "MySQL",
        "mariadbd" | "mariadb" => "MariaDB",
        "redis-server" | "redis" | "redis-stack" => "Redis",
        "valkey-server" | "valkey" => "Valkey",
        "mongod" | "mongo" => "MongoDB",
        "memcached" => "Memcached",
        "qdrant" => "Qdrant",
        "meilisearch" => "Meilisearch",
        "influxd" | "influxdb3" => "InfluxDB",
        "cockroach" => "CockroachDB",
        "redpanda" => "Redpanda",
        "loki" => "Loki",
        "tempo" => "Tempo",
        "alertmanager" => "Alertmanager",
        "gitea" => "Gitea",
        "n8n" => "n8n",
        "portainer" => "Portainer",
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
        "opencode" => "OpenCode",
        "cloudflared" => "Cloudflare Tunnel",
        "tailscaled" => "Tailscale",
        "dnsmasq" | "systemd-resolve" | "systemd-resolved" => "DNS",
        "docker-proxy" => "Docker",
        _ => return None,
    })
}

pub(super) fn runtime(exe: &str) -> Option<&'static str> {
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

pub(super) fn framework(exe: &str) -> Option<&'static str> {
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

pub(super) fn python(exe: &str) -> bool {
    exe == "python"
        || exe.strip_prefix("python").is_some_and(|version| {
            version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
        })
}

pub(super) fn image_service(repository: &str) -> Option<&'static str> {
    let basename = repository.rsplit('/').next().unwrap_or(repository);
    let publisher = repository.rsplit('/').nth(1);
    // Generic names require the final publisher/repository pair, including mirrors.
    let qualified = match (publisher, basename) {
        (Some("temporalio"), "auto-setup" | "server") => Some("Temporal"),
        (Some("temporalio"), "admin-tools") => Some("Temporal tools"),
        (Some("temporalio"), "ui") => Some("Temporal UI"),
        // https://docs.redpanda.com/labs/docker-compose/single-broker/
        (Some("redpandadata"), "console") => Some("Redpanda Console"),
        // https://grafana.com/docs/loki/latest/setup/install/docker/
        // https://hub.docker.com/r/grafana/tempo
        (Some("grafana"), "loki") => Some("Loki"),
        (Some("grafana"), "tempo") => Some("Tempo"),
        // https://docs.portainer.io/start/install-ce/server/docker/linux
        (Some("portainer"), "portainer-ce") => Some("Portainer"),
        // https://distribution.github.io/distribution/about/deploying/
        (None | Some("library"), "registry") => Some("Distribution Registry"),
        (Some("docker.io" | "index.docker.io"), "registry")
            if matches!(
                repository,
                "docker.io/registry" | "index.docker.io/registry"
            ) =>
        {
            Some("Distribution Registry")
        }
        _ => None,
    };
    if qualified.is_some() {
        return qualified;
    }
    match basename {
        // Distinctive product names work with private registry mirrors.
        // https://qdrant.tech/documentation/installation/
        // https://www.meilisearch.com/integrations/docker
        "qdrant" => Some("Qdrant"),
        "meilisearch" => Some("Meilisearch"),
        // https://hub.docker.com/_/influxdb
        // https://hub.docker.com/r/cockroachdb/cockroach
        // https://hub.docker.com/_/cassandra
        // https://hub.docker.com/_/couchdb
        // https://neo4j.com/docs/operations-manual/current/docker/introduction/
        "influxdb" => Some("InfluxDB"),
        "cockroach" => Some("CockroachDB"),
        "cassandra" => Some("Cassandra"),
        "couchdb" => Some("CouchDB"),
        "neo4j" => Some("Neo4j"),
        // https://www.keycloak.org/server/containers
        // https://docs.n8n.io/hosting/installation/docker/
        // https://docs.gitea.com/installation/install-with-docker/
        "keycloak" => Some("Keycloak"),
        "n8n" => Some("n8n"),
        "gitea" => Some("Gitea"),
        // https://docs.redpanda.com/labs/docker-compose/single-broker/
        // https://hub.docker.com/r/prom/alertmanager
        "redpanda" => Some("Redpanda"),
        "alertmanager" => Some("Alertmanager"),
        // Executable aliases must not bypass publisher scoping.
        "loki" | "tempo" | "registry" | "portainer" => None,
        "pgvector" | "timescaledb-ha" => Some("Postgres"),
        "redis-stack-server" => Some("Redis"),
        "valkey-bundle" => Some("Valkey"),
        "mongodb" | "mongodb-community-server" => Some("MongoDB"),
        "rabbitmq-server" => Some("RabbitMQ"),
        "nginx-unprivileged" => Some("nginx"),
        _ => service(basename),
    }
}

/// Framework launch conventions; called with one normalized argument at a time.
pub(super) fn framework_argument(base: &str, argv: &[&str]) -> Option<&'static str> {
    if base == "manage.py" && argv.contains(&"runserver") {
        return Some("Django");
    }
    let stem = [".js", ".cjs", ".mjs"]
        .iter()
        .find_map(|suffix| base.strip_suffix(suffix));
    framework(base).or_else(|| stem.and_then(framework))
}

/// Recognize kubectl's command, not a resource name or an option value.
/// argv excludes argv[0]. Unknown prefix flags leave the identity at kubectl.
/// https://kubernetes.io/docs/reference/kubectl/generated/kubectl_port-forward/
pub(super) fn kubectl(argv: &[&str]) -> &'static str {
    let mut args = argv.iter().copied();
    while let Some(arg) = args.next() {
        match arg {
            "port-forward" => return "kubectl port-forward",
            "proxy" => return "kubectl proxy",
            "--disable-compression" | "--insecure-skip-tls-verify" | "--match-server-version" => {}
            _ => {
                let (flag, inline) = arg.split_once('=').map_or((arg, false), |(f, _)| (f, true));
                let takes_value = matches!(
                    flag,
                    "-n" | "--namespace"
                        | "--context"
                        | "--cluster"
                        | "--user"
                        | "--kubeconfig"
                        | "--kuberc"
                        | "-s"
                        | "--server"
                        | "--request-timeout"
                        | "--as"
                        | "--as-group"
                        | "--as-uid"
                        | "--as-user-extra"
                        | "--cache-dir"
                        | "--certificate-authority"
                        | "--client-certificate"
                        | "--client-key"
                        | "--token"
                        | "--username"
                        | "--password"
                        | "--proxy-url"
                        | "--profile"
                        | "--profile-output"
                        | "-v"
                        | "--v"
                        | "--vmodule"
                );
                if takes_value {
                    if !inline && args.next().is_none() {
                        break;
                    }
                } else if (arg.starts_with("-n") || arg.starts_with("-s") || arg.starts_with("-v"))
                    && !arg.starts_with("--")
                    && arg.len() > 2
                {
                    // Cobra's attached short flag value, e.g. -nproduction.
                } else if inline
                    && matches!(
                        flag,
                        "--disable-compression"
                            | "--insecure-skip-tls-verify"
                            | "--match-server-version"
                    )
                {
                } else {
                    break;
                }
            }
        }
    }
    "kubectl"
}
