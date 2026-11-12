use super::*;
use serde_json::json;

fn listener(overrides: Value) -> Value {
    let mut value = json!({
        "port":7000,"address":"127.0.0.1","pid":40,"processName":"worker",
        "executable":"/opt/acme/bin/worker","workingDirectory":"/srv/acme/worker"
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    value
}

fn details(values: &[(&'static str, &str)]) -> Vec<Detail> {
    values
        .iter()
        .map(|(label, value)| Detail {
            label,
            value: (*value).to_owned(),
        })
        .collect()
}

#[test]
fn names_use_observed_service_identity_before_runtime_and_thread() {
    assert_eq!(
        owner(&listener(
            json!({"containerName":"queue","applicationName":"Vite · app"})
        )),
        Some("queue")
    );
    assert_eq!(
        name(&listener(
            json!({"containerName":"shop-db-1","composeService":"db"})
        )),
        Some("db")
    );
    assert_eq!(
        owner(&listener(
            json!({"processName":null,"applicationName":null})
        )),
        None
    );
    assert_eq!(
        service_name(&listener(
            json!({"executable":"/usr/bin/node","processName":"MainThread","applicationName":"Vite · site"})
        )),
        Some("Vite · site".into())
    );
    assert_eq!(
        service_name(&listener(
            json!({"executable":"/usr/bin/postgres","processName":"postgres"})
        )),
        Some("Postgres".into())
    );
    assert_eq!(service_name(&json!({"port":5432})), None);
}

#[test]
fn runtime_directories_are_projects_but_custom_programs_only_workspaces() {
    for executable in [
        "node",
        "nodejs",
        "bun",
        "deno",
        "python",
        "python3.11",
        "ruby",
        "php",
        "java",
        "dotnet",
        "uvicorn",
        "gunicorn",
    ] {
        assert_eq!(
            process_details(&listener(
                json!({"executable":format!("/usr/bin/{executable}")})
            )),
            details(&[("Project directory", "/srv/acme/worker")])
        );
    }
    assert_eq!(
        process_details(&listener(json!({}))),
        details(&[("Working directory", "/srv/acme/worker")])
    );
    assert_eq!(
        process_details(&listener(json!({"executable":"/usr/bin/node (deleted)"}))),
        details(&[("Project directory", "/srv/acme/worker")])
    );
    assert_eq!(
        process_details(&listener(
            json!({"executable":null,"processName":"MainThread"})
        )),
        vec![]
    );
}

#[test]
fn argv_preserves_spaces_and_limits_curated_details() {
    for (exe, argv, expected) in [
        (
            "python3",
            vec!["python3", "-m", "gunicorn", "site:app"],
            ("Module", "gunicorn"),
        ),
        (
            "python3.11",
            vec!["python", "app with spaces.py"],
            ("Script", "app with spaces.py"),
        ),
        (
            "java",
            vec!["java", "-jar", "/opt/acme/report server.jar"],
            ("Application", "/opt/acme/report server.jar"),
        ),
        (
            "dotnet",
            vec!["dotnet", "Billing.dll"],
            ("Application", "Billing.dll"),
        ),
        ("ruby", vec!["ruby", "server.rb"], ("Script", "server.rb")),
        ("php", vec!["php", "server.php"], ("Script", "server.php")),
    ] {
        let actual = process_details(&listener(json!({"executable":exe,"arguments":argv})));
        assert_eq!(actual[0], details(&[expected])[0]);
        assert_eq!(actual.len(), 2);
    }
}

#[test]
fn known_service_operational_paths_survive_system_path_suppression() {
    for (exe, argv, expected) in [
        (
            "postgres",
            vec!["postgres", "-D", "/var/lib/postgresql"],
            vec![("Data directory", "/var/lib/postgresql")],
        ),
        (
            "mysqld",
            vec![
                "mysqld",
                "--datadir=/data/mysql",
                "--defaults-file",
                "/etc/mysql.conf",
                "--password=x",
            ],
            vec![
                ("Data directory", "/data/mysql"),
                ("Configuration", "/etc/mysql.conf"),
            ],
        ),
        (
            "mariadbd",
            vec!["mariadbd", "--datadir=/data/maria", "--password=x"],
            vec![("Data directory", "/data/maria")],
        ),
        (
            "nginx",
            vec!["nginx", "-c", "/etc/nginx/site.conf"],
            vec![("Configuration", "/etc/nginx/site.conf")],
        ),
        (
            "sshd",
            vec!["sshd", "-f", "/etc/sshd.conf"],
            vec![("Configuration", "/etc/sshd.conf")],
        ),
        (
            "redis-server",
            vec!["redis-server", "/etc/redis.conf"],
            vec![("Configuration", "/etc/redis.conf")],
        ),
        (
            "caddy",
            vec!["caddy", "--config=/etc/Caddyfile"],
            vec![("Configuration", "/etc/Caddyfile")],
        ),
    ] {
        assert_eq!(
            process_details(&listener(json!({"executable":exe,"arguments":argv}))),
            details(&expected)
        );
    }
}

#[test]
fn directory_boundaries_suppress_system_homes_but_not_similar_names() {
    for path in [
        "/",
        "/root",
        "/home",
        "/home/ci",
        "/usr/share/x",
        "/var/lib/x",
        "/var/log/x",
        "/nix/store/foo",
        "/proc/1",
        "/sys/a",
        "/dev/a",
        "/run/a",
        "/etc/a",
        "/bin/a",
        "/sbin/a",
        "relative/path",
    ] {
        assert_eq!(
            process_details(&listener(json!({"workingDirectory":path}))),
            vec![],
            "{path}"
        );
    }
    for path in [
        "/home/ci/project",
        "/usr-local/project",
        "/var/library/project",
        "/root/project",
    ] {
        assert_eq!(
            process_details(&listener(json!({"workingDirectory":path}))),
            details(&[("Working directory", path)])
        );
    }
    assert_eq!(
        process_details(&listener(json!({"workingDirectory":null}))),
        vec![]
    );
    assert_eq!(
        process_details(&listener(json!({"containerName":"queue"}))),
        vec![]
    );
    assert_eq!(
        process_details(&listener(json!({"executable":"sshd"}))),
        vec![]
    );
}

#[test]
fn options_respect_end_of_options_and_reject_code_flags_and_line_breaks() {
    for argv in [
        json!(["nginx", "-c", "-g"]),
        json!(["nginx", "--", "-c", "/etc/ignored.conf"]),
        json!(["nginx", "-c", "/etc/file\nsecret"]),
        json!(["nginx", "-c"]),
        json!(["nginx", "-c", null, "/etc/not-a-value.conf"]),
    ] {
        assert!(
            process_details(&listener(json!({"executable":"nginx","arguments":argv}))).is_empty()
        );
    }
    for argv in [
        json!(["python3", "-c", "print(1)"]),
        json!(["python3", "-m", "module.with spaces"]),
        json!(["python3", "-m", "module\rsecret"]),
    ] {
        assert!(process_details(&listener(json!({"executable":"python3","workingDirectory":"/","command":"python3 -m gunicorn","arguments":argv}))).is_empty());
    }
    assert!(
        process_details(&listener(
            json!({"executable":"nginx","arguments":["nginx","-c","--","-c","/etc/ignored.conf"]})
        ))
        .is_empty()
    );
}

#[test]
fn kinds_use_argv_basename_framework_module_and_executable_not_port_numbers() {
    for (exe, argv, expected) in [
        (
            "node",
            vec!["node", "/srv/site/node_modules/.bin/astro", "dev"],
            vec!["Astro", "Node"],
        ),
        (
            "node",
            vec!["node", "/srv/site/vite.mjs"],
            vec!["Vite", "Node"],
        ),
        (
            "python3",
            vec!["python3", "-m", "streamlit", "run", "app.py"],
            vec!["Streamlit", "Python"],
        ),
        (
            "python3",
            vec!["python3", "manage.py", "runserver"],
            vec!["Django", "Python"],
        ),
        (
            "python3.11",
            vec!["python3", "-m", "http.server"],
            vec!["HTTP server", "Python"],
        ),
        ("redis-server", vec!["redis-server"], vec!["Redis"]),
        ("uvicorn", vec!["uvicorn"], vec!["Uvicorn"]),
    ] {
        assert_eq!(
            kinds(&listener(json!({"executable":exe,"arguments":argv}))),
            expected
        );
    }
    assert_eq!(kinds(&listener(json!({"port":5432}))), Vec::<&str>::new());
    assert_eq!(
        kinds(&listener(
            json!({"port":6379,"processName":null,"executable":null})
        )),
        Vec::<&str>::new()
    );
    assert_eq!(
        kinds(&listener(
            json!({"executable":"MainThread","command":"node /srv/vite.js"})
        )),
        Vec::<&str>::new()
    );
    assert_eq!(
        kinds(&listener(
            json!({"executable":"node (deleted)","arguments":["node","--token=astro","/srv/vite.js","vite","next","astro"]})
        )),
        vec!["Vite", "Next.js", "Astro"]
    );
    for exe in ["python.", "python3.", "python3.x", "pythonista"] {
        assert!(!kinds(&listener(json!({"executable":exe}))).contains(&"Python"));
    }
}

#[test]
fn docker_images_support_registry_ports_tags_and_digests_without_process_leakage() {
    for (image, expected) in [
        ("registry.example/mq/rabbitmq:3", "RabbitMQ"),
        ("registry.example:5000/mq/rabbitmq:3", "RabbitMQ"),
        ("[::1]:5000/postgres:17@sha256:1234", "Postgres"),
        ("valkey/valkey-bundle", "Valkey"),
    ] {
        assert_eq!(
            kinds(&listener(
                json!({"containerName":"q","image":image,"arguments":["node","vite"]})
            )),
            vec![expected, "Docker"]
        );
    }
    assert_eq!(
        kinds(&listener(
            json!({"containerName":"app","image":"custom/app","executable":"node"})
        )),
        vec!["Docker"]
    );
    assert!(is_runtime_kind("Node") && is_runtime_kind("Docker") && !is_runtime_kind("Redis"));
}

#[test]
fn row_context_hides_title_and_project_repetition_but_keeps_operational_context() {
    let base = "/home/ci/work/atlas";
    let app = listener(
        json!({"executable":"node","processName":"MainThread","arguments":["node","/site/vite.js"],"workingDirectory":format!("{base}/app")}),
    );
    assert_eq!(service_name(&app), Some("Vite".into()));
    assert_eq!(
        service_detail(&app, &["127.0.0.1".into()], Some(base)),
        vec!["app/", "Node"]
    );
    let compose =
        listener(json!({"containerName":"shop-db-1","composeService":"db","image":"postgres:17"}));
    assert_eq!(
        service_detail(&compose, &["127.0.0.1".into(), "::".into()], None),
        vec!["Postgres", "all interfaces"]
    );
    assert_eq!(
        service_detail(
            &listener(json!({"executable":"sshd","processName":"sshd"})),
            &[],
            None
        ),
        vec!["sshd"]
    );
    assert_eq!(
        service_detail(
            &listener(json!({"executable":"postgres","processName":"postgres"})),
            &[],
            None
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        service_detail(
            &listener(
                json!({"executable":"node","processName":"node","workingDirectory":"/tmp/build/preview","workingDirectoryDeleted":true})
            ),
            &[],
            None
        ),
        vec!["folder deleted", "/tmp/build/preview"]
    );
}

#[test]
fn tildify_and_short_paths_preserve_owner_and_directory_boundaries() {
    assert_eq!(
        tildify("/home/ci/work/atlas", &[Some("ci"), Some("ci")]),
        "~/work/atlas"
    );
    assert_eq!(tildify("/root/site", &[Some("root")]), "~/site");
    assert_eq!(
        tildify("/home/ci/work", &[Some("ci"), Some("deploy")]),
        "/home/ci/work"
    );
    assert_eq!(tildify("/home/cicd/work", &[Some("ci")]), "/home/cicd/work");
    assert_eq!(
        short_path(
            "/tmp/ci-1000/session-0123456789abcdef/build/jobs/wt/c3",
            &[],
            40
        ),
        "…/build/jobs/wt/c3"
    );
    assert_eq!(
        short_path("/home/ci/work/atlas/.worktrees/search", &[Some("ci")], 40),
        "~/work/atlas/.worktrees/search"
    );
    assert_eq!(short_path("/data/pg", &[], 40), "/data/pg");
    assert_eq!(short_path("app/", &[], 40), "app/");
    assert_eq!(short_path("/long/parent/🍀/😀", &[], 6), "…/🍀/😀");
    assert_eq!(
        short_path("/a/very-long-name", &[], 1),
        "…/a/very-long-name"
    );
}

#[test]
fn tooltip_carries_full_paths_both_addresses_identity_and_stale_file_flags() {
    let port = listener(
        json!({"executable":"node","user":"ci","workingDirectoryDeleted":true,"executableDeleted":true,"command":"node server.js"}),
    );
    assert_eq!(
        tooltip(&port, &["0.0.0.0".into(), "::".into()]),
        "Project directory: /srv/acme/worker\nIts working directory was deleted while it kept running; it may be left over.\nIts program file changed since it started; restart it to run the new one.\nListening on 0.0.0.0, ::\nPID 40 · ci\nnode server.js"
    );
    let container = listener(json!({"containerName":"shop-db-1","pid":null}));
    assert_eq!(
        tooltip(&container, &["::1".into()]),
        "Container: shop-db-1\nListening on ::1"
    );
}

#[test]
fn search_includes_curated_unabridged_context_and_secondary_bind_addresses() {
    let port = listener(
        json!({"port":8080,"executable":"python3","processName":"MainThread","applicationName":"Reporting","user":"CI","arguments":["python3","-m","gunicorn","--secret=NotSearchable"],"worktreePath":"/home/ci/WORK/feature/","worktreeBranch":"Feat/REPORTS"}),
    );
    let text = search_text(&port, &["127.0.0.1".into(), "::1".into()]);
    assert_eq!(
        text,
        "8080 127.0.0.1 ::1 reporting mainthread ci 40 gunicorn python gunicorn /srv/acme/worker feat/reports /home/ci/work/feature"
    );
    assert!(!text.contains("notsearchable"));
    assert!(!search_text(&listener(json!({"worktreeBranch":"ignored"})), &[]).contains("ignored"));
}

#[test]
fn forward_names_only_add_a_linked_checkout_branch() {
    assert_eq!(
        forward_name(&listener(
            json!({"executable":"node","arguments":["node","vite"],"worktreePath":"/srv/search","worktreeMain":false,"worktreeBranch":"feat/search"})
        )),
        "Vite · feat/search"
    );
    assert_eq!(
        forward_name(
            &json!({"worktreePath":"/srv/search","worktreeMain":false,"worktreeBranch":"feat/search"})
        ),
        "feat/search"
    );
    assert_eq!(
        forward_name(&listener(
            json!({"worktreeMain":true,"worktreeBranch":"main"})
        )),
        "worker"
    );
    assert_eq!(
        forward_name(&listener(json!({"worktreeMain":false,"worktreeBranch":""}))),
        "worker"
    );
    assert_eq!(
        forward_name(&listener(
            json!({"worktreeMain":false,"worktreeBranch":"ignored"})
        )),
        "worker"
    );
}

#[test]
fn nix_wrappers_normalize_display_and_detection_without_losing_raw_evidence() {
    for raw in [
        ".opencode-wrapped",
        ".opencode-wrapped__",
        "..opencode-wrapped-wrapped_",
    ] {
        let path = format!("/nix/store/hash-opencode/bin/{raw}");
        let port = json!({"processName":raw,"executable":path});
        assert_eq!(service_name(&port).as_deref(), Some("OpenCode"));
        assert_eq!(kinds(&port), vec!["OpenCode"]);
        assert!(tooltip(&port, &[]).contains(&path));
        assert!(
            !service_detail(&port, &[], None)
                .iter()
                .any(|d| d.contains("-wrapped"))
        );
        assert_eq!(port["processName"], raw);
    }
    let process_only = json!({"processName":".opencode-wrapped"});
    assert_eq!(service_name(&process_only).as_deref(), Some("OpenCode"));
    assert!(tooltip(&process_only, &[]).contains(".opencode-wrapped"));
    let python = json!({"executable":"/nix/store/hash/bin/.python3.12-wrapped (deleted)","arguments":["python3.12", "-m", "http.server"]});
    assert_eq!(kinds(&python), vec!["HTTP server", "Python"]);
    assert_eq!(
        service_name(&json!({"processName":".custom-tool-wrapped"})).as_deref(),
        Some("custom-tool")
    );
}

#[test]
fn wrapper_normalization_does_not_rename_regular_programs_or_containers() {
    for name in [
        "my-wrapped",
        ".hidden",
        ".-wrapped",
        "opencode-helper",
        ".opencode-wrapped-backup",
    ] {
        assert_eq!(unwrapped_name(name), name);
    }
    let container = json!({"containerName":".opencode-wrapped", "image":"custom:latest", "processName":".node-wrapped"});
    assert_eq!(
        service_name(&container).as_deref(),
        Some(".opencode-wrapped")
    );
    assert_eq!(kinds(&container), vec!["Docker"]);
}

#[test]
fn docker_icons_use_images_while_names_preserve_container_and_compose_identity() {
    for (image, expected) in [
        ("pgvector/pgvector:pg17", "Postgres"),
        (
            "registry.local:5000/mirror/temporalio/auto-setup:1.25@sha256:abcd",
            "Temporal",
        ),
        ("temporalio/server:latest", "Temporal"),
        ("temporalio/admin-tools:latest", "Temporal tools"),
        ("temporalio/ui:latest", "Temporal UI"),
        ("team/admin-tools:latest", "admin-tools"),
        ("timescale/timescaledb-ha:pg17", "Postgres"),
        ("redis/redis-stack-server:latest", "Redis"),
        ("team/postgres-exporter:latest", "postgres-exporter"),
        ("team/auto-setup:latest", "auto-setup"),
        ("nottemporalio/server:latest", "server"),
        ("team/custom-api:latest", "custom-api"),
    ] {
        let port = listener(
            json!({"containerName":"nginx", "composeService":"redis", "image":image,
            "executable":"postgres", "arguments":["node", "vite"]}),
        );
        assert_eq!(service_name(&port).as_deref(), Some("redis"), "{image}");
        let inferred = image_service(image)
            .unwrap_or_else(|| image_repository(image).rsplit('/').next().unwrap());
        assert_eq!(inferred, expected, "{image}");
        let standalone = json!({"containerName":"My-Original_Container", "image":image});
        assert_eq!(
            service_name(&standalone).as_deref(),
            Some("My-Original_Container")
        );
        assert!(is_docker(&port));
        assert!(tooltip(&port, &[]).contains(&format!("Image: {image}")));
        assert!(tooltip(&port, &[]).contains("Compose service: redis"));
        assert!(!kinds(&port).contains(&"Vite"));
    }
    let image_only = json!({"image":"postgres:17"});
    assert_eq!(service_name(&image_only).as_deref(), Some("postgres"));
    assert_eq!(kinds(&image_only), vec!["Postgres", "Docker"]);
    let missing_image = json!({"containerName":"postgres", "composeService":"redis"});
    assert_eq!(service_name(&missing_image).as_deref(), Some("redis"));
    assert_eq!(kinds(&missing_image), vec!["Docker"]);
}

#[test]
fn popular_service_images_support_registries_mirrors_tags_and_digests() {
    for (repository, expected) in [
        ("library/registry", "Distribution Registry"),
        ("qdrant/qdrant", "Qdrant"),
        ("getmeili/meilisearch", "Meilisearch"),
        ("library/influxdb", "InfluxDB"),
        ("cockroachdb/cockroach", "CockroachDB"),
        ("library/cassandra", "Cassandra"),
        ("library/couchdb", "CouchDB"),
        ("library/neo4j", "Neo4j"),
        ("keycloak/keycloak", "Keycloak"),
        ("n8nio/n8n", "n8n"),
        ("gitea/gitea", "Gitea"),
        ("redpandadata/redpanda", "Redpanda"),
        ("redpandadata/console", "Redpanda Console"),
        ("grafana/loki", "Loki"),
        ("grafana/tempo", "Tempo"),
        ("prom/alertmanager", "Alertmanager"),
        ("portainer/portainer-ce", "Portainer"),
    ] {
        for prefix in [
            "",
            "docker.io/",
            "ghcr.io/",
            "quay.io/",
            "mirror.local:5000/cache/",
        ] {
            for suffix in [
                "",
                ":latest",
                ":v1.2-alpine",
                "@sha256:abcd",
                ":v1@sha256:abcd",
            ] {
                let image = format!("{prefix}{repository}{suffix}");
                let port = json!({"image":image, "composeService":"My-Service", "containerName":"My_Container", "executable":"nginx"});
                assert_eq!(kinds(&port), vec![expected, "Docker"], "{image}");
                assert_eq!(service_name(&port).as_deref(), Some("My-Service"));
                let standalone = json!({"image":image, "containerName":"My_Container"});
                assert_eq!(service_name(&standalone).as_deref(), Some("My_Container"));
            }
        }
    }
    for image in [
        "registry:3",
        "docker.io/registry:3",
        "index.docker.io/registry@sha256:abcd",
    ] {
        assert_eq!(image_service(image), Some("Distribution Registry"));
    }
}

#[test]
fn image_patterns_do_not_guess_from_generic_names_or_related_products() {
    for image in [
        "team/registry:3",
        "mirror.local:5000/registry:3",
        "team/server",
        "team/console",
        "team/portainer-ce",
        "notredpandadata/console",
        "team/redpandadata-console",
        "team/grafana-loki",
        "team/loki",
        "team/tempo",
        "grafana/tempo-vulture",
        "qdrant/qdrant-backup",
        "team/meilisearch-exporter",
        "keycloak/keycloak-operator",
        "library/registry-ui",
        "gitea/act_runner",
        "team/n8n-worker",
        "grafana/loki/helper",
        "redpandadata/console/helper",
        "team/docker.io/registry",
    ] {
        let port = json!({"image":image, "containerName":"qdrant", "composeService":"registry", "port":6333});
        assert_eq!(kinds(&port), vec!["Docker"], "{image}");
    }
}

#[test]
fn new_native_services_recognize_nix_wrappers_without_matching_arguments() {
    for (exe, expected) in [
        ("qdrant", "Qdrant"),
        ("meilisearch", "Meilisearch"),
        ("influxd", "InfluxDB"),
        ("influxdb3", "InfluxDB"),
        ("cockroach", "CockroachDB"),
        ("redpanda", "Redpanda"),
        ("loki", "Loki"),
        ("tempo", "Tempo"),
        ("alertmanager", "Alertmanager"),
        ("gitea", "Gitea"),
        ("portainer", "Portainer"),
        ("n8n", "n8n"),
    ] {
        let port = json!({"executable":format!("/nix/store/hash/bin/.{exe}-wrapped")});
        assert_eq!(kinds(&port), vec![expected]);
        let unrelated = json!({"executable":"/usr/bin/custom", "arguments":["custom", exe]});
        assert!(kinds(&unrelated).is_empty());
    }
}

#[test]
fn kubectl_identifies_forwarding_and_proxy_after_global_flags() {
    for args in [
        vec!["kubectl", "port-forward", "service/postgres", "5432:5432"],
        vec![
            "kubectl",
            "-n",
            "production",
            "--context=dev",
            "port-forward",
            "deployment/api",
            "8080:80",
        ],
        vec![
            "kubectl",
            "-nproduction",
            "--context",
            "dev",
            "port-forward",
            "pod/vite",
            ":3000",
        ],
        vec![
            "kubectl",
            "--insecure-skip-tls-verify",
            "port-forward",
            "pod/api",
            "8080:80",
        ],
    ] {
        let port = json!({"executable":"/nix/store/hash/bin/.kubectl-wrapped", "arguments":args});
        assert_eq!(kinds(&port), vec!["kubectl port-forward"]);
        assert_eq!(service_name(&port).as_deref(), Some("kubectl port-forward"));
    }
    assert_eq!(
        kinds(
            &json!({"processName":"kubectl", "arguments":["kubectl", "--context", "dev", "proxy"]})
        ),
        vec!["kubectl proxy"]
    );
}

#[test]
fn kubectl_does_not_treat_option_values_or_remote_commands_as_forwarding() {
    for args in [
        vec!["kubectl", "--context", "port-forward", "get", "pods"],
        vec!["kubectl", "get", "pod/port-forward"],
        vec!["kubectl", "exec", "pod/api", "--", "port-forward"],
        vec!["kubectl", "--", "port-forward"],
        vec!["kubectl", "--unknown", "port-forward"],
        vec!["kubectl", "--context"],
        vec!["kubectl", "--namespace=proxy"],
    ] {
        assert_eq!(
            kinds(&json!({"executable":"kubectl", "arguments":args})),
            vec!["kubectl"]
        );
    }
    assert_eq!(kinds(&json!({"executable":"kubectl"})), vec!["kubectl"]);
    assert!(
        kinds(&json!({"executable":"custom", "arguments":["custom","kubectl","port-forward"]}))
            .is_empty()
    );
}
