//! Small, local process hints. Never infer ownership from a port or saved alias.
use super::*;

// Brand SVG provenance and notices: assets/process-icons/.
const BRANDS: &[(&str, &str, &[u8])] = &[
    (
        "kubectl",
        "process-icons/kubernetes.svg",
        include_bytes!("../assets/process-icons/kubernetes.svg"),
    ),
    (
        "kubectl port-forward",
        "process-icons/kubernetes.svg",
        include_bytes!("../assets/process-icons/kubernetes.svg"),
    ),
    (
        "kubectl proxy",
        "process-icons/kubernetes.svg",
        include_bytes!("../assets/process-icons/kubernetes.svg"),
    ),
    (
        "CockroachDB",
        "process-icons/cockroachlabs.svg",
        include_bytes!("../assets/process-icons/cockroachlabs.svg"),
    ),
    (
        "Gitea",
        "process-icons/gitea.svg",
        include_bytes!("../assets/process-icons/gitea.svg"),
    ),
    (
        "InfluxDB",
        "process-icons/influxdb.svg",
        include_bytes!("../assets/process-icons/influxdb.svg"),
    ),
    (
        "Meilisearch",
        "process-icons/meilisearch.svg",
        include_bytes!("../assets/process-icons/meilisearch.svg"),
    ),
    (
        "n8n",
        "process-icons/n8n.svg",
        include_bytes!("../assets/process-icons/n8n.svg"),
    ),
    (
        "Neo4j",
        "process-icons/neo4j.svg",
        include_bytes!("../assets/process-icons/neo4j.svg"),
    ),
    (
        "Portainer",
        "process-icons/portainer.svg",
        include_bytes!("../assets/process-icons/portainer.svg"),
    ),
    (
        "Alertmanager",
        "process-icons/prometheus.svg",
        include_bytes!("../assets/process-icons/prometheus.svg"),
    ),
    (
        "Loki",
        "process-icons/grafana.svg",
        include_bytes!("../assets/process-icons/grafana.svg"),
    ),
    (
        "Tempo",
        "process-icons/grafana.svg",
        include_bytes!("../assets/process-icons/grafana.svg"),
    ),
    (
        "Distribution Registry",
        "process-icons/docker.svg",
        include_bytes!("../assets/process-icons/docker.svg"),
    ),
    (
        "Temporal tools",
        "process-icons/temporal.svg",
        include_bytes!("../assets/process-icons/temporal.svg"),
    ),
    (
        "Temporal UI",
        "process-icons/temporal.svg",
        include_bytes!("../assets/process-icons/temporal.svg"),
    ),
    (
        "Temporal",
        "process-icons/temporal.svg",
        include_bytes!("../assets/process-icons/temporal.svg"),
    ),
    (
        "OpenCode",
        "process-icons/opencode.svg",
        include_bytes!("../assets/process-icons/opencode.svg"),
    ),
    (
        "Vite",
        "process-icons/vite.svg",
        include_bytes!("../assets/process-icons/vite.svg"),
    ),
    (
        "Node",
        "process-icons/nodedotjs.svg",
        include_bytes!("../assets/process-icons/nodedotjs.svg"),
    ),
    (
        "Bun",
        "process-icons/bun.svg",
        include_bytes!("../assets/process-icons/bun.svg"),
    ),
    (
        "Docker",
        "process-icons/docker.svg",
        include_bytes!("../assets/process-icons/docker.svg"),
    ),
    (
        "Python",
        "process-icons/python.svg",
        include_bytes!("../assets/process-icons/python.svg"),
    ),
    (
        "Rails",
        "process-icons/rubyonrails.svg",
        include_bytes!("../assets/process-icons/rubyonrails.svg"),
    ),
    (
        "Laravel",
        "process-icons/laravel.svg",
        include_bytes!("../assets/process-icons/laravel.svg"),
    ),
    (
        ".NET",
        "process-icons/dotnet.svg",
        include_bytes!("../assets/process-icons/dotnet.svg"),
    ),
    (
        "Postgres",
        "process-icons/postgresql.svg",
        include_bytes!("../assets/process-icons/postgresql.svg"),
    ),
    (
        "MySQL",
        "process-icons/mysql.svg",
        include_bytes!("../assets/process-icons/mysql.svg"),
    ),
    (
        "MariaDB",
        "process-icons/mariadb.svg",
        include_bytes!("../assets/process-icons/mariadb.svg"),
    ),
    (
        "Redis",
        "process-icons/redis.svg",
        include_bytes!("../assets/process-icons/redis.svg"),
    ),
    (
        "MongoDB",
        "process-icons/mongodb.svg",
        include_bytes!("../assets/process-icons/mongodb.svg"),
    ),
    (
        "ClickHouse",
        "process-icons/clickhouse.svg",
        include_bytes!("../assets/process-icons/clickhouse.svg"),
    ),
    (
        "Elasticsearch",
        "process-icons/elasticsearch.svg",
        include_bytes!("../assets/process-icons/elasticsearch.svg"),
    ),
    (
        "OpenSearch",
        "process-icons/opensearch.svg",
        include_bytes!("../assets/process-icons/opensearch.svg"),
    ),
    (
        "etcd",
        "process-icons/etcd.svg",
        include_bytes!("../assets/process-icons/etcd.svg"),
    ),
    (
        "nginx",
        "process-icons/nginx.svg",
        include_bytes!("../assets/process-icons/nginx.svg"),
    ),
    (
        "Caddy",
        "process-icons/caddy.svg",
        include_bytes!("../assets/process-icons/caddy.svg"),
    ),
    (
        "Traefik",
        "process-icons/traefikproxy.svg",
        include_bytes!("../assets/process-icons/traefikproxy.svg"),
    ),
    (
        "Envoy",
        "process-icons/envoyproxy.svg",
        include_bytes!("../assets/process-icons/envoyproxy.svg"),
    ),
    (
        "NATS",
        "process-icons/natsdotio.svg",
        include_bytes!("../assets/process-icons/natsdotio.svg"),
    ),
    (
        "RabbitMQ",
        "process-icons/rabbitmq.svg",
        include_bytes!("../assets/process-icons/rabbitmq.svg"),
    ),
    (
        "Cloudflare Tunnel",
        "process-icons/cloudflare.svg",
        include_bytes!("../assets/process-icons/cloudflare.svg"),
    ),
    (
        "Tailscale",
        "process-icons/tailscale.svg",
        include_bytes!("../assets/process-icons/tailscale.svg"),
    ),
    (
        "Next.js",
        "process-icons/nextdotjs.svg",
        include_bytes!("../assets/process-icons/nextdotjs.svg"),
    ),
    (
        "Nuxt",
        "process-icons/nuxt.svg",
        include_bytes!("../assets/process-icons/nuxt.svg"),
    ),
    (
        "Astro",
        "process-icons/astro.svg",
        include_bytes!("../assets/process-icons/astro.svg"),
    ),
    (
        "Remix",
        "process-icons/remix.svg",
        include_bytes!("../assets/process-icons/remix.svg"),
    ),
    (
        "React Router",
        "process-icons/react.svg",
        include_bytes!("../assets/process-icons/react.svg"),
    ),
    (
        "Create React App",
        "process-icons/react.svg",
        include_bytes!("../assets/process-icons/react.svg"),
    ),
    (
        "Flask",
        "process-icons/flask.svg",
        include_bytes!("../assets/process-icons/flask.svg"),
    ),
    (
        "FastAPI",
        "process-icons/fastapi.svg",
        include_bytes!("../assets/process-icons/fastapi.svg"),
    ),
    (
        "Streamlit",
        "process-icons/streamlit.svg",
        include_bytes!("../assets/process-icons/streamlit.svg"),
    ),
    (
        "Jupyter",
        "process-icons/jupyter.svg",
        include_bytes!("../assets/process-icons/jupyter.svg"),
    ),
    (
        "Django",
        "process-icons/django.svg",
        include_bytes!("../assets/process-icons/django.svg"),
    ),
    (
        "Grafana",
        "process-icons/grafana.svg",
        include_bytes!("../assets/process-icons/grafana.svg"),
    ),
    (
        "Prometheus",
        "process-icons/prometheus.svg",
        include_bytes!("../assets/process-icons/prometheus.svg"),
    ),
    (
        "MinIO",
        "process-icons/minio.svg",
        include_bytes!("../assets/process-icons/minio.svg"),
    ),
    (
        "Consul",
        "process-icons/consul.svg",
        include_bytes!("../assets/process-icons/consul.svg"),
    ),
    (
        "Vault",
        "process-icons/vault.svg",
        include_bytes!("../assets/process-icons/vault.svg"),
    ),
    (
        "Ollama",
        "process-icons/ollama.svg",
        include_bytes!("../assets/process-icons/ollama.svg"),
    ),
];

pub(super) fn asset(path: &str) -> Option<&'static [u8]> {
    BRANDS
        .iter()
        .find(|(_, name, _)| *name == path)
        .map(|(_, _, bytes)| *bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProcessIcon {
    Asset(&'static str),
    Brand(&'static str),
    Network,
    Web,
    Process,
}

impl ProcessIcon {
    pub(super) fn for_port(port: &Value, saved_only: bool) -> Self {
        if saved_only {
            return Self::Network;
        }
        for kind in newport_domain::port_context::kinds(port) {
            if let Some((_, path, _)) = BRANDS.iter().find(|(name, _, _)| *name == kind) {
                return Self::Brand(path);
            }
            let file = match kind {
                "Node" => "package.json",
                "Bun" => "bun.lock",
                "Docker" => "dockerfile",
                "Python" => ".py",
                "Ruby" | "Rails" | "Puma" | "Unicorn" => ".rb",
                "Java" => ".java",
                "PHP" | "Laravel" => ".php",
                ".NET" => ".cs",
                "Postgres" | "MySQL" | "MariaDB" | "Redis" | "Valkey" | "MongoDB" | "Memcached"
                | "ClickHouse" | "Elasticsearch" | "OpenSearch" | "etcd" | "Qdrant"
                | "Cassandra" | "CouchDB" => ".sql",
                "SSH" => ".sh",
                "nginx" | "Caddy" | "Apache" | "Traefik" | "HAProxy" | "Envoy" | "HTTP server"
                | "Keycloak" => {
                    return Self::Web;
                }
                "NATS" | "Kafka" | "RabbitMQ" | "DNS" | "Cloudflare Tunnel" | "Tailscale"
                | "Redpanda" | "Redpanda Console" => {
                    return Self::Network;
                }
                _ => continue,
            };
            if let Some(asset) = crate::file_icon_assets::lookup(file) {
                return Self::Asset(asset);
            }
        }
        Self::Process
    }

    pub(super) fn render(self, cx: &App) -> AnyElement {
        let icon = match self {
            Self::Asset(path) => return img(path).size(px(16.)).flex_shrink_0().into_any_element(),
            Self::Brand(path) => {
                return svg()
                    .path(path)
                    .size(px(16.))
                    .flex_shrink_0()
                    .text_color(cx.theme().foreground.opacity(0.8))
                    .into_any_element();
            }
            Self::Network => IconName::Network,
            Self::Web => IconName::Globe,
            Self::Process => IconName::Cpu,
        };
        Icon::new(icon)
            .size(px(16.))
            .flex_shrink_0()
            .text_color(cx.theme().muted_foreground)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::ProcessIcon;
    use serde_json::json;

    #[test]
    fn observed_process_identity_wins_over_ports_and_aliases() {
        let node = json!({"executable":"/usr/bin/node", "arguments":["node", "/app/node_modules/vite/bin/vite.js"], "port":5432, "name":"Postgres"});
        assert_eq!(
            ProcessIcon::for_port(&node, false),
            ProcessIcon::Brand("process-icons/vite.svg")
        );
        assert_eq!(ProcessIcon::for_port(&node, true), ProcessIcon::Network);
        assert_eq!(
            ProcessIcon::for_port(
                &json!({"port":5432,"name":"postgres","processName":"custom-server"}),
                false
            ),
            ProcessIcon::Process
        );
    }

    #[test]
    fn nix_wrapped_opencode_uses_its_bundled_logo() {
        let port = json!({"executable":"/nix/store/hash/bin/.opencode-wrapped_", "processName":".opencode-wrapped"});
        assert_eq!(
            ProcessIcon::for_port(&port, false),
            ProcessIcon::Brand("process-icons/opencode.svg")
        );
        assert!(super::asset("process-icons/opencode.svg").is_some());
    }

    #[test]
    fn expanded_service_icons_are_bundled_and_generic_names_stay_generic() {
        for (image, slug) in [
            ("getmeili/meilisearch:v1", "meilisearch"),
            ("cockroachdb/cockroach:latest", "cockroachlabs"),
            ("library/influxdb:3", "influxdb"),
            ("library/neo4j:5", "neo4j"),
            ("gitea/gitea:latest", "gitea"),
            ("n8nio/n8n:latest", "n8n"),
            ("portainer/portainer-ce:lts", "portainer"),
            ("grafana/loki:3", "grafana"),
            ("prom/alertmanager:latest", "prometheus"),
        ] {
            let path = format!("process-icons/{slug}.svg");
            let port = json!({"image":image, "containerName":"unchanged"});
            assert!(
                matches!(ProcessIcon::for_port(&port, false), ProcessIcon::Brand(p) if p == path),
                "{image}: {:?}",
                ProcessIcon::for_port(&port, false)
            );
            assert!(super::asset(&path).is_some(), "{path}");
        }
        let port = json!({"image":"team/console", "containerName":"redpanda"});
        assert_eq!(
            ProcessIcon::for_port(&port, false),
            ProcessIcon::Brand("process-icons/docker.svg")
        );
    }

    #[test]
    fn kubectl_uses_kubernetes_icon_without_guessing_the_target_service() {
        let port = json!({"executable":"kubectl", "arguments":["kubectl", "port-forward", "service/postgres", "5432:5432"]});
        assert_eq!(
            ProcessIcon::for_port(&port, false),
            ProcessIcon::Brand("process-icons/kubernetes.svg")
        );
        assert!(super::asset("process-icons/kubernetes.svg").is_some());
    }

    #[test]
    fn container_image_precedes_docker_fallback() {
        let postgres = json!({"containerName":"redis", "composeService":"nginx", "image":"registry.example:5000/pgvector/pgvector:pg17"});
        assert_eq!(
            ProcessIcon::for_port(&postgres, false),
            ProcessIcon::Brand("process-icons/postgresql.svg")
        );
        let temporal = json!({"containerName":"worker", "image":"temporalio/auto-setup:1.25"});
        assert_eq!(
            ProcessIcon::for_port(&temporal, false),
            ProcessIcon::Brand("process-icons/temporal.svg")
        );
        let custom = json!({"containerName":"postgres", "image":"team/custom:latest"});
        assert_eq!(
            ProcessIcon::for_port(&custom, false),
            ProcessIcon::Brand("process-icons/docker.svg")
        );
    }
}
