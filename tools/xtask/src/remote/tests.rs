use super::*;
use std::{io::Read, process::Stdio};

#[test]
#[ignore = "requires a local Docker socket in DOCKER_HOST"]
fn agent_docker_context_contains_every_local_copy_source() -> Result<()> {
    let endpoint =
        std::env::var("DOCKER_HOST").context("Set DOCKER_HOST to a local unix socket")?;
    ensure!(
        endpoint
            .strip_prefix("unix://")
            .is_some_and(|path| Path::new(path).is_absolute()),
        "The context fixture requires an absolute local Docker socket"
    );
    let source = fs::read_to_string(root().join("tools/agent/Dockerfile"))?;
    let mut fixture = String::from("FROM scratch\n");
    for line in source.lines().map(str::trim) {
        let Some((instruction, arguments)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        ensure!(
            !instruction.eq_ignore_ascii_case("ADD"),
            "Include ADD inputs in the Docker context fixture before using them"
        );
        if !instruction.eq_ignore_ascii_case("COPY") {
            continue;
        }
        ensure!(
            !line.ends_with('\\'),
            "Fold continued COPY instructions before checking the Docker context"
        );
        if arguments.trim_start().starts_with("--from=") {
            continue;
        }
        fixture.push_str(line);
        fixture.push('\n');
    }
    ensure!(fixture.contains("COPY "), "No local COPY inputs found");
    let temp = tempfile::tempdir()?;
    let dockerfile = temp.path().join("Dockerfile");
    fs::write(&dockerfile, fixture)?;
    let ignore = fs::read_to_string(root().join("tools/agent/Dockerfile.dockerignore"))?;
    let ignore_path = temp.path().join("Dockerfile.dockerignore");
    fs::write(&ignore_path, &ignore)?;
    let config = temp.path().join("docker-config");
    let plugins = config.join("cli-plugins");
    fs::create_dir_all(&plugins)?;
    let docker = docker();
    let buildx = process::text(
        command(&docker)
            .env_remove("DOCKER_CONTEXT")
            .env("DOCKER_HOST", &endpoint)
            .args([
                "info",
                "--format",
                "{{range .ClientInfo.Plugins}}{{if eq .Name \"buildx\"}}{{.Path}}{{end}}{{end}}",
            ]),
    )?;
    ensure!(Path::new(&buildx).is_file(), "Docker buildx is unavailable");
    std::os::unix::fs::symlink(buildx, plugins.join("docker-buildx"))?;
    let output = temp.path().join("output");
    // BuildKit applies the real ignore rules without running or compiling the agent.
    let build = || {
        let log_path = temp.path().join("build.log");
        let log = fs::File::create(&log_path)?;
        let status = process::Process::spawn(
            command(&docker)
                .env_remove("DOCKER_CONTEXT")
                .env_remove("SSH_AUTH_SOCK")
                .env("DOCKER_HOST", &endpoint)
                .env("DOCKER_CONFIG", &config)
                .args([
                    "buildx",
                    "build",
                    "--builder",
                    "default",
                    "--network",
                    "none",
                    "--pull=false",
                    "--file",
                ])
                .arg(&dockerfile)
                .arg("--output")
                .arg(format!("type=local,dest={}", output.display()))
                .arg(".")
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log)),
        )?
        .wait(Duration::from_secs(120))?;
        let mut diagnostic = String::new();
        fs::File::open(log_path)?
            .take(65537)
            .read_to_string(&mut diagnostic)?;
        ensure!(
            diagnostic.len() <= 65536,
            "Docker context output exceeded its limit"
        );
        eprintln!("{diagnostic}");
        Ok::<_, anyhow::Error>((status, diagnostic))
    };
    ensure!(build()?.0.success(), "A Docker COPY input is unavailable");
    assert_eq!(
        fs::read(output.join("checks/private-directory.sh"))?,
        fs::read(root().join("tests/agent-runtime/private-directory.sh"))?
    );
    fs::write(
        ignore_path,
        format!("{ignore}\ntests/agent-runtime/private-directory.sh\n"),
    )?;
    let (status, diagnostic) = build()?;
    ensure!(
        !status.success()
            && diagnostic.contains("private-directory.sh")
            && diagnostic.contains("not found"),
        "The explicit denial did not reject the required COPY input"
    );
    Ok(())
}
