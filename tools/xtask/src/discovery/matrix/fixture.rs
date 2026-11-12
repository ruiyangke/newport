use super::{
    distro::{Architecture, Distro},
    supervisor::Supervisor,
};
use crate::{command, process, remote};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    process::{Command, Stdio},
    time::Duration,
};

#[cfg(all(test, unix))]
mod tests;

const OWNER: &str = "newport.discovery.matrix";
const USER_SETUP: &str = "useradd --create-home --shell /bin/sh newport && mkdir -p /etc/sudoers.d && printf 'newport ALL=(ALL) NOPASSWD: ALL\n' > /etc/sudoers.d/newport-discovery && chmod 0440 /etc/sudoers.d/newport-discovery";

pub(super) fn docker() -> Command {
    let mut cmd = command(remote::docker());
    if let Some(context) = std::env::var_os("NEWPORT_TEST_DOCKER_CONTEXT") {
        cmd.env("DOCKER_CONTEXT", context);
    }
    cmd
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Image {
    id: String,
    architecture: String,
    os: String,
    repo_digests: Vec<String>,
}

fn inspect(image: &str, platform: Option<Architecture>) -> Result<Image> {
    let mut cmd = docker();
    cmd.args(["image", "inspect"]);
    // Docker before 28 has no `image inspect --platform`. Its classic image
    // store keeps only the platform that was pulled, which provision checks.
    if let Some(arch) = platform.filter(|_| inspect_selects_platform()) {
        cmd.args(["--platform", arch.platform()]);
    }
    let mut images: Vec<Image> = serde_json::from_str(&process::text(cmd.arg(image))?)?;
    ensure!(images.len() == 1, "Expected one Docker image");
    images.pop().context("Missing Docker image identity")
}

fn inspect_selects_platform() -> bool {
    process::text(docker().args(["image", "inspect", "--help"]))
        .is_ok_and(|help| help.contains("--platform"))
}

#[derive(Serialize)]
pub(super) struct Fixture {
    pub name: String,
    pub image_reference: String,
    pub image: Option<Image>,
    pub image_reference_id: Option<String>,
    pub image_preexisting: bool,
    pub container_removed: bool,
    pub image_cleanup: Option<String>,
    pub os_release: Option<String>,
    pub kernel: Option<String>,
    owner: String,
}

impl Fixture {
    pub fn new(name: String, reference: String, owner: String) -> Result<Self> {
        let existing = process::Process::spawn(
            docker()
                .args(["image", "inspect", &reference])
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )?
        .wait(Duration::from_secs(30))?
        .success();
        if !existing {
            // An absent image and an unavailable daemon both fail inspect.
            process::text(docker().args(["info", "--format", "{{.ID}}"]))
                .context("Checking Docker before pulling an absent image")?;
        }
        Ok(Self {
            name,
            image_reference: reference,
            owner,
            image_preexisting: existing,
            image: None,
            image_reference_id: None,
            container_removed: false,
            image_cleanup: None,
            os_release: None,
            kernel: None,
        })
    }

    pub fn provision(
        &mut self,
        distro: Distro,
        arch: Architecture,
        runner: &mut Supervisor,
    ) -> Result<()> {
        if !self.image_preexisting {
            runner.run(
                docker().args(["pull", "--platform", arch.platform(), &self.image_reference]),
                "pull",
                Duration::from_secs(600),
            )?;
        }
        // An index's selected manifest may not be addressable by ID locally.
        // Pin the index for execution and cleanup, selecting its platform explicitly.
        self.image_reference_id = Some(inspect(&self.image_reference, None)?.id);
        let reference_id = self
            .image_reference_id
            .as_deref()
            .context("Missing image reference identity")?;
        self.image = Some(inspect(reference_id, Some(arch)).context(
            "Inspecting requested image platform; cached references are preserved, use --image to select an available image",
        )?);
        let image = self.image.as_ref().context("Missing pulled image")?;
        ensure!(
            image.os == "linux"
                && arch
                    .platform()
                    .ends_with(&format!("/{}", image.architecture)),
            "Cached image has the wrong platform; supply an explicit matching digest with --image"
        );
        runner.run(
            docker().args([
                "run",
                "--detach",
                "--init",
                "--platform",
                arch.platform(),
                "--name",
                &self.name,
                "--label",
                &format!("{OWNER}={}", self.owner),
                "--memory",
                "2g",
                "--cpus",
                "4",
                "--pids-limit",
                "512",
                "--entrypoint",
                "/bin/sh",
                reference_id,
                "-c",
                "while :; do sleep 3600; done",
            ]),
            "create",
            Duration::from_secs(60),
        )?;
        runner.container = Some(self.name.clone());
        runner.run(
            self.exec().args([
                "sh",
                "-c",
                &format!("{} && {USER_SETUP}", distro.packages()),
            ]),
            "setup",
            Duration::from_secs(600),
        )?;
        self.os_release = Some(process::text(self.exec().args(["cat", "/etc/os-release"]))?);
        self.kernel = Some(process::text(self.exec().args(["uname", "-a"]))?);
        Ok(())
    }

    pub fn exec(&self) -> Command {
        let mut cmd = docker();
        cmd.args(["exec", "--user", "root", &self.name]);
        cmd
    }

    pub fn check_empty(&self) -> Result<()> {
        let paths = process::text(self.exec().args([
            "sh", "-c", "for p in /tmp/newport-discovery.*; do if [ -e \"$p\" ] || [ -L \"$p\" ]; then printf '%s\\n' \"$p\"; fi; done",
        ]))?;
        let processes = process::text(docker().args(["top", &self.name, "-eo", "pid,args"]))?;
        ensure!(
            paths.is_empty() && !processes.contains("/tmp/newport-discovery."),
            "Discovery fixture files or processes remain"
        );
        Ok(())
    }

    pub fn cleanup(&mut self) -> Result<()> {
        // Check ownership even if `docker run` failed or its CLI was interrupted
        // after the daemon had already created the container.
        let owned = process::text(docker().args([
            "container",
            "ls",
            "--all",
            "--quiet",
            "--no-trunc",
            "--filter",
            &format!("name=^/{}$", self.name),
            "--filter",
            &format!("label={OWNER}={}", self.owner),
        ]))?;
        for id in owned.lines() {
            ensure!(
                id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid container identity during cleanup"
            );
            process::Process::spawn(docker().args(["rm", "--force", id]))?
                .success(Duration::from_secs(30))?;
        }
        self.container_removed = true;
        if self.image_preexisting {
            self.image_cleanup = Some("Preserved preexisting image".into());
        } else if let Some(reference_id) = &self.image_reference_id {
            let current = inspect(&self.image_reference, None)?;
            if current.id == *reference_id {
                let mut child = process::Process::spawn(
                    docker()
                        .args(["image", "rm", &self.image_reference])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null()),
                )?;
                let status = child.wait(Duration::from_secs(30))?;
                ensure!(
                    status.success(),
                    "Could not remove newly pulled image {}",
                    self.image_reference
                );
                self.image_cleanup = Some("Removed newly pulled image reference".into());
            } else {
                self.image_cleanup =
                    Some("Preserved image reference changed by another process".into());
            }
        } else {
            self.image_cleanup =
                Some("Pull did not yield a recorded image; partial Docker cache may remain".into());
        }
        Ok(())
    }
}
