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
    assert!(process_details(&listener(
        json!({"executable":"nginx","arguments":["nginx","-c","--","-c","/etc/ignored.conf"]})
    ))
    .is_empty());
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
        vec!["Postgres", "Docker", "all interfaces"]
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
    assert_eq!(tooltip(&port, &["0.0.0.0".into(), "::".into()]), "Project directory: /srv/acme/worker\nIts working directory was deleted while it kept running; it may be left over.\nIts program file changed since it started; restart it to run the new one.\nListening on 0.0.0.0, ::\nPID 40 · ci\nnode server.js");
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
    assert_eq!(text, "8080 127.0.0.1 ::1 reporting mainthread ci 40 gunicorn python gunicorn /srv/acme/worker feat/reports /home/ci/work/feature");
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
