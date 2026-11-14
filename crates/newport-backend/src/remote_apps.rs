//! Read models for Remote Apps: the shared agent product and the
//! server's applications. Sessions are live resources and stay out of here.
use crate::server_session::ServerSession;
use crate::ssh::shell_quote as quote;
use newport_protocol::desktop::applications::{
    self as dto, Availability, Elevation, Finding, Gap, Host, Inactive, Requirement, Updater,
};
use newport_protocol::remote_apps::apps::Catalogue;
use std::sync::Arc;

mod display;

/// Sets `agent` to the installed agent, or empty. Each release's selected,
/// validated generation (`current`) counts; incomplete installs are skipped.
/// Among installed releases, the most recently installed one is used.
const FIND_AGENT: &str = r#"cache="${NEWPORT_REMOTE_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/newport-remote}"
set --
for release in "$cache"/releases/system/* "$cache"/releases/nix/*; do
    if [ -x "$release/current/run-agent" ] && [ -f "$release/current/.validated-v1" ]; then
        set -- "$@" "$release/current/run-agent"
    fi
done
agent=
if [ "$#" -gt 0 ]; then agent=$(ls -t "$@" | head -n 1); fi
"#;

/// Reports `gap=gles missing` when apps can't draw with OpenGL. `root` is the
/// file system's root, empty on a server.
macro_rules! opengl_check {
    () => {
        r#"# GTK 4 loads OpenGL ES through libepoxy where EGL is installed, and exits
# when it is missing, as on minimal cloud images; without EGL it draws with
# Cairo and needs none. NixOS apps find the OpenGL driver only in
# /run/opengl-driver, which hardware.graphics.enable provides; without it
# every app that draws with OpenGL fails.
# musl's ldconfig rejects -p but exits 0, so look for the library first.
if [ -e "$root/etc/NIXOS" ]; then
    driver=
    for f in "$root"/run/opengl-driver/share/glvnd/egl_vendor.d/*.json; do
        [ -e "$f" ] && driver=1
    done
    [ -n "$driver" ] || echo 'gap=gles missing'
else
    gles=
    egl=
    for f in "$root"/usr/lib/libEGL.so.1 "$root"/usr/lib64/libEGL.so.1 "$root"/usr/lib/*-linux-gnu/libEGL.so.1; do
        [ -e "$f" ] && egl=1
    done
    [ -n "$egl" ] || { ldconfig -p 2>/dev/null || /sbin/ldconfig -p 2>/dev/null; } | grep -q 'libEGL\.so\.1 ' || gles=1
    for f in "$root"/usr/lib/libGLESv2.so.2 "$root"/usr/lib64/libGLESv2.so.2 "$root"/usr/lib/*-linux-gnu/libGLESv2.so.2; do
        [ -e "$f" ] && gles=1
    done
    [ -n "$gles" ] || { ldconfig -p 2>/dev/null || /sbin/ldconfig -p 2>/dev/null; } | grep -q 'libGLESv2\.so\.2 ' || echo 'gap=gles missing'
fi
"#
    };
}

/// Reports the installed releases and the host's dependencies separately.
/// Nix resources of a release do not satisfy the shared full agent’s dependencies.
const REPORT: &str = concat!(
    r#"printf 'agent=%s\n' "$agent"
for release in "$@"; do
    printf 'release=%s\n' "$release"
    built=$(head -c 32 "${release%/run-agent}/built-at" 2>/dev/null) && printf 'built=%s %s\n' "$release" "$built"
done
printf 'system=%s\n' "$(uname -s)"
printf 'architecture=%s\n' "$(uname -m)"
# How the account runs an install as root, and how an image-based system
# adds packages instead of its package manager.
if [ "$(id -u)" = 0 ]; then echo 'elevation=root'
elif command -v sudo >/dev/null 2>&1; then echo 'elevation=sudo'
elif command -v doas >/dev/null 2>&1; then echo 'elevation=doas'
else echo 'elevation=unavailable'
fi
if command -v rpm-ostree >/dev/null 2>&1 && [ -e /run/ostree-booted ]; then
    echo 'updater=rpmOstree'
elif command -v transactional-update >/dev/null 2>&1 && awk '$2 == "/" { exit $4 !~ /^ro(,|$)/ }' /proc/mounts 2>/dev/null; then
    echo 'updater=transactionalUpdate'
fi
[ -d /run/systemd/system ] && echo 'userServices=1'
# A session bus has to start, not only be installed.
if ! command -v dbus-daemon >/dev/null 2>&1 || ! command -v dbus-run-session >/dev/null 2>&1; then
    echo 'gap=dbus missing'
elif command -v timeout >/dev/null 2>&1 && ! timeout 5 dbus-run-session -- true >/dev/null 2>&1; then
    echo 'gap=dbus failed'
fi
root=
"#,
    opengl_check!(),
    r#"# Snap and Flatpak apps get file dialogs from the GTK portal backend.
if command -v snap >/dev/null 2>&1 || command -v flatpak >/dev/null 2>&1; then
    portal=
    for f in /usr/libexec/xdg-desktop-portal-gtk /usr/lib/xdg-desktop-portal-gtk /usr/lib/*/xdg-desktop-portal-gtk; do
        [ -x "$f" ] && portal=1
    done
    [ -n "$portal" ] || echo 'gap=portal missing'
fi
# Sound needs the account's pulse server on its standard socket, or a
# pulseaudio Needle starts itself. PipeWire's user services refuse root.
if [ -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/pulse/native" ] || command -v pulseaudio >/dev/null 2>&1; then
    :
elif [ "$(id -u)" = 0 ]; then
    echo 'gap=audio root'
elif command -v pipewire-pulse >/dev/null 2>&1; then
    echo 'gap=audio stopped'
else
    echo 'gap=audio missing'
fi
# Apps draw text through fontconfig; with no font at all they can't open
# a window. Without fontconfig there is nothing that uses it yet.
if command -v fc-list >/dev/null 2>&1 && [ -z "$(fc-list : family | head -n 1)" ]; then
    echo 'gap=fonts missing'
fi
echo '--os-release--'
cat /etc/os-release 2>/dev/null || cat /usr/lib/os-release 2>/dev/null
exit 0"#
);

/// Rendering icons for many applications can take a while on a first run.
const CATALOGUE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Selected runtime, default installation, and host dependencies. Only a
/// verified, compatible full runtime receives the launch marker.
pub(crate) async fn status(owner: &Arc<ServerSession>) -> Result<Availability, String> {
    let server = owner.server().clone();
    if owner.is_revoked() {
        return Err(crate::server_session::STALE.into());
    }
    let probe = format!(
        "sh -c {}",
        quote(&format!("{}\n{FIND_AGENT}{REPORT}", display::PROBE))
    );
    let output = owner
        .unless_unreachable(crate::remote::command::stdout(owner.link(), &probe, None))
        .await?;
    let mut status = parse_status(&output, &server);
    let payload = crate::agent_payload::payload(status.architecture.as_deref().unwrap_or_default());
    status.bundled_revision = payload.map(|p| p.sha256.to_owned());
    status.total = payload.map(|p| p.size as u64);
    if status.system.as_deref() != Some("Linux") || payload.is_none() {
        return Ok(status);
    }
    let installation = owner.check_installation().await?;
    let needle = owner.needle(&server)?;
    let selected = owner
        .unless_unreachable(owner.agent_runtime.ensure(
            owner.link(),
            owner.integration_session_id(),
            || needle.validate(),
        ))
        .await;
    apply_installation(&mut status, &installation, selected.as_ref());
    Ok(status)
}

/// The probe's report for `server`, before the installation is checked.
fn parse_status(output: &str, server: &crate::model::Server) -> Availability {
    let (head, release) = output.split_once("--os-release--").unwrap_or((output, ""));
    let field = |name: &str| {
        head.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let agent = field("agent");
    // `<cache>/releases/<runtime>/<revision>/...`: the archive's digest.
    let revision_of = |agent: &str| {
        agent
            .rsplit_once("/releases/")
            .and_then(|(_, rest)| rest.split('/').nth(1))
            .map(str::to_owned)
    };
    let revision = agent.and_then(revision_of);
    // Release identities are evidence only: they do not select launches or
    // take part in the unified product's update ordering.
    let built: std::collections::HashMap<&str, u64> = head
        .lines()
        .filter_map(|line| line.strip_prefix("built="))
        .filter_map(|line| {
            let (path, seconds) = line.trim().rsplit_once(' ')?;
            Some((path, seconds.trim().parse().ok()?))
        })
        .collect();
    let agents = head
        .lines()
        .filter_map(|line| line.strip_prefix("release="))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .filter_map(|path| {
            Some(dto::LegacyAgent {
                revision: revision_of(path)?,
                agent: path.to_owned(),
                built_at: built.get(path).copied(),
            })
        })
        .collect();
    let mut gaps: Vec<Gap> = head
        .lines()
        .filter_map(|line| line.strip_prefix("gap="))
        .filter_map(parse_gap)
        .collect();
    let host = Host {
        elevation: match field("elevation") {
            Some("root") => Elevation::Root,
            Some("doas") => Elevation::Doas,
            Some("unavailable") => Elevation::Unavailable,
            _ => Elevation::Sudo,
        },
        updater: match field("updater") {
            Some("rpmOstree") => Some(Updater::RpmOstree),
            Some("transactionalUpdate") => Some(Updater::TransactionalUpdate),
            _ => None,
        },
        user_services: field("userServices").is_some(),
    };
    // The display starts XWayland when an app asks for X11.
    if let Some(gap) = display::gap(field("xwayland")) {
        gaps.insert(0, gap);
    }
    Availability {
        agent: None,
        legacy_agent: agent.map(str::to_owned),
        legacy_revision: revision,
        legacy_agents: agents,
        system: field("system").map(str::to_owned),
        architecture: field("architecture").map(str::to_owned),
        gaps,
        host,
        os_release: release.trim().to_owned(),
        expected_server: server.into(),
        bundled_variant: crate::agent_payload::variant(),
        bundled_revision: None,
        total: None,
        installed_variant: None,
        revision: None,
        build_relation: newport_protocol::desktop::configuration::BuildRelation::Unknown,
        installation_error: None,
        selected_runtime: None,
        agent_error: None,
        automatic_update: None,
    }
}

#[cfg(test)]
mod xwayland_tests {
    use super::*;
    #[test]
    fn xwayland_is_offered_never_required() {
        for version in ["missing", "22.1.9", "24.1.6", "unverified"] {
            let report = format!(
                "system=Linux\narchitecture=aarch64\nxwayland={version}\ngap=dbus missing\n--os-release--\nID=debian\n"
            );
            let status = parse_status(&report, &tests::server());
            let usable = version == "24.1.6";
            let gaps = &status.gaps;
            assert_eq!(gaps.len(), if usable { 1 } else { 2 }, "{version}");
            if !usable {
                assert_eq!(gaps[0].requirement, Requirement::Xwayland);
                assert!(newport_domain::requirements::optional(&gaps[0]));
            }
            assert_eq!(gaps.last().unwrap().requirement, Requirement::Dbus);
        }
    }
}

/// The default entry point can belong to another desktop. Its diagnostics
/// remain separate from the initialized runtime that admits this one's workers.
fn apply_installation(
    status: &mut Availability,
    installation: &crate::agent_lifecycle::Status,
    selected: Result<&newport_protocol::runtime::Hello, &crate::agent_runtime::Error>,
) {
    use crate::agent_runtime::Error;
    use crate::rpc::configuration::{build_relation, update_outcome};
    use newport_protocol::product::Variant;
    status.agent = None;
    status.installed_variant = installation.installed_variant;
    status.revision = installation.installed_hash.clone();
    status.build_relation = build_relation(installation.build_relation);
    status.installation_error = installation.error.clone();
    status.selected_runtime = None;
    status.agent_error = None;
    status.automatic_update = installation.automatic_update.clone().map(update_outcome);
    let hello = match selected {
        Ok(hello) => hello,
        Err(Error::NotInstalled) if installation.installed == Some(false) => {
            status.agent_error = installation.error.clone();
            return;
        }
        Err(error) => {
            status.agent_error = Some(match error {
                Error::OlderProtocol => installation
                    .error
                    .clone()
                    .unwrap_or_else(|| error.to_string()),
                _ => error.to_string(),
            });
            return;
        }
    };
    let inspected = match inspect_selected(installation, hello) {
        Ok(inspected) => inspected,
        Err(error) => {
            status.agent_error = Some(error);
            return;
        }
    };
    status.selected_runtime = Some(dto::SelectedRuntime {
        build_sha256: hello.build_sha256.clone(),
        product: inspected.product.variant,
    });
    if let Some(unmet) = inspected.unmet {
        status.agent_error = Some(unusable(&unmet).to_owned());
        return;
    }
    if inspected.product.variant == Variant::Full {
        use newport_protocol::worker::admission;
        // Other services' optional dependencies do not prevent GUI workers
        // from running, but every GUI worker needs stream admission.
        if let Some(error) = std::iter::once((admission::CAPABILITY, admission::VERSION))
            .chain(Variant::Full.needs().iter().copied())
            .find_map(|(service, version)| hello.require(service, version).err())
        {
            status.agent_error = Some(error.message);
            return;
        }
        // This marker never supplies an executable to a shell. Runtime
        // admission chooses the worker's immutable build.
        status.agent = Some("newport-agent".into());
    }
}

fn inspect_selected(
    installation: &crate::agent_lifecycle::Status,
    hello: &newport_protocol::runtime::Hello,
) -> Result<crate::agent_runtime::Inspected, String> {
    hello.validate().map_err(|error| error.to_string())?;
    let is_default = installation.installed_hash.as_deref() == Some(hello.build_sha256.as_str());
    let expected_variant =
        if installation.bundled_hash.as_deref() == Some(hello.build_sha256.as_str()) {
            Some(installation.bundled_variant)
        } else if is_default {
            installation.installed_variant
        } else {
            return Err("Cannot verify the selected Needle runtime's executable identity.".into());
        };
    if expected_variant.is_some_and(|variant| variant != hello.product) {
        return Err("The selected Needle runtime does not match its verified product.".into());
    }
    if is_default && let Some(error) = &installation.error {
        return Err(error.clone());
    }
    let product = newport_protocol::product::Product {
        variant: hello.product,
        control_versions: hello.versions.clone(),
        services: hello
            .services
            .iter()
            .map(|(name, capability)| (name.clone(), capability.versions.clone()))
            .collect(),
    };
    product.validate_form()?;
    Ok(crate::agent_runtime::Inspected::of(product))
}

/// Only a build newer in what Remote apps needs has a newer Newport Remote;
/// one newer only in what the rest of Newport needs has a newer agent.
fn unusable(unmet: &crate::agent_runtime::Unmet) -> &'static str {
    let remote_apps = newport_protocol::product::Variant::Full
        .needs()
        .iter()
        .any(|(service, _)| unmet.services.iter().any(|short| short == service));
    match (unmet.newer, remote_apps) {
        (true, true) => {
            "This server has a newer Newport Remote than this Newport. Update Newport to use it."
        }
        (true, false) => {
            "This server has a newer Newport agent than this Newport. Update Newport to use it."
        }
        (false, _) => {
            "The Newport agent on this server does not work with this Newport. Update Newport or reinstall its full agent."
        }
    }
}

/// openSUSE moved `dbus-daemon` and `dbus-run-session` from `dbus-1` into
/// `dbus-1-daemon` in Tumbleweed and Leap 16 (SUSE Linux 16); Leap and SLES
/// 15 still ship them in `dbus-1`.
/// A `gap=` line: a requirement and what the probe found.
fn parse_gap(line: &str) -> Option<Gap> {
    let (requirement, state) = line.trim().split_once(' ')?;
    let requirement = match requirement {
        "dbus" => Requirement::Dbus,
        "fonts" => Requirement::Fonts,
        "gles" => Requirement::Gles,
        "portal" => Requirement::Portal,
        "audio" => Requirement::Audio,
        _ => return None,
    };
    let finding = match state {
        "missing" => Finding::Missing(Box::default()),
        "root" => Finding::Inactive(Box::new(Inactive::Root)),
        "stopped" => Finding::Inactive(Box::new(Inactive::Stopped)),
        "failed" => Finding::Inactive(Box::new(Inactive::Failed)),
        _ => return None,
    };
    Some(Gap {
        requirement,
        finding,
    })
}

/// The server's validated application catalogue.
pub(crate) async fn catalogue(owner: &Arc<ServerSession>) -> Result<Catalogue, String> {
    let needle = owner.needle(owner.server())?;
    let (channel, worker) = needle
        .launch_channel(crate::agent::Service::RemoteApps {
            arguments: vec!["--catalog".into()],
        })
        .await?;
    let output = read_channel(
        channel,
        newport_protocol::remote_apps::apps::wire::MAX_FRAMED_BYTES,
        CATALOGUE_TIMEOUT,
    )
    .await
    .map_err(|error| match error {
        ReadError::TimedOut => "Listing applications took longer than a minute.".to_owned(),
        ReadError::Failed(message) => message,
    })?;
    drop(worker);
    needle.validate()?;
    decode_catalogue(&output)
}

/// Decoding validates the catalogue; a service error becomes its message.
fn decode_catalogue(output: &[u8]) -> Result<Catalogue, String> {
    newport_protocol::remote_apps::apps::wire::decode_complete(output)
        .map_err(|error| format!("The agent returned an invalid catalogue: {error}"))?
        .into_result()
        .map_err(|error| error.message)
}

enum ReadError {
    TimedOut,
    Failed(String),
}

/// A command's stdout up to `limit` bytes within `timeout`; a failure reports
/// its error output. The command is closed on every early return, so it
/// doesn't keep running on the server.
async fn read_channel(
    mut channel: crate::service_channel::Channel,
    limit: usize,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, ReadError> {
    use russh::ChannelMsg;
    let deadline = tokio::time::Instant::now() + timeout;
    let (mut out, mut err, mut status) = (Vec::new(), Vec::new(), None);
    loop {
        let message = tokio::select! {
            message = channel.wait() => message.map_err(ReadError::Failed)?,
            _ = tokio::time::sleep_until(deadline) => {
                let _ = channel.close().await;
                return Err(ReadError::TimedOut);
            }
        };
        let Some(message) = message else { break };
        match message {
            ChannelMsg::Data { data } => {
                if out.len() + data.len() > limit {
                    let _ = channel.close().await;
                    return Err(ReadError::Failed(format!(
                        "The agent's output exceeded {} MiB.",
                        limit / (1024 * 1024)
                    )));
                }
                out.extend_from_slice(&data);
            }
            ChannelMsg::ExtendedData { data, .. } => keep_tail(&mut err, &data),
            ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
            ChannelMsg::Failure => {
                let _ = channel.close().await;
                return Err(ReadError::Failed(
                    "The server refused to run Newport Remote’s command.".into(),
                ));
            }
            _ => {}
        }
    }
    match status {
        Some(0) => Ok(out),
        status => {
            let detail = String::from_utf8_lossy(&err).trim().to_owned();
            Err(ReadError::Failed(if !detail.is_empty() {
                detail
            } else if let Some(code) = status {
                format!("The agent exited with status {code}.")
            } else {
                "The agent ended without an exit status.".to_owned()
            }))
        }
    }
}

/// Keep the last part of a command's error output.
pub(crate) fn keep_tail(tail: &mut Vec<u8>, data: &[u8]) {
    const LIMIT: usize = 8 * 1024;
    tail.extend_from_slice(data);
    if tail.len() > LIMIT {
        tail.drain(..tail.len() - LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::Availability;
    use newport_protocol::{desktop::configuration::BuildRelation, product::Variant as Product};

    #[test]
    fn the_catalogue_keeps_every_field_and_reports_service_errors() {
        use newport_protocol::{
            common::{Error, Retry},
            remote_apps::apps::{Application, Catalogue, DesktopAction, Reply, wire},
        };
        let result = Reply::Catalogue(Catalogue {
            apps: vec![Application {
                id: "editor.desktop".into(),
                name: "Éditeur".into(),
                description: "Text editor".into(),
                keywords: vec!["text".into()],
                icon: None,
                actions: vec![DesktopAction {
                    id: "new-window".into(),
                    name: "New window".into(),
                }],
            }],
        });
        let Reply::Catalogue(expected) = result.clone() else {
            unreachable!()
        };
        assert_eq!(
            super::decode_catalogue(&wire::encode(&result).unwrap()).unwrap(),
            expected
        );
        let result = Reply::Failed(Error {
            code: "catalogue_failed".into(),
            message: "Directory read failed".into(),
            retry: Retry::No,
        });
        assert_eq!(
            super::decode_catalogue(&wire::encode(&result).unwrap()).unwrap_err(),
            "Directory read failed"
        );
        assert!(super::decode_catalogue(&[5, 10]).is_err());
    }

    pub(super) fn server() -> crate::model::Server {
        serde_json::from_value(serde_json::json!({
            "id": "7a9c1d4e-0000-4000-8000-000000000001", "name": "Box",
            "sshHost": "host", "sshUser": "user", "sshPort": 22, "identityFile": null,
        }))
        .unwrap()
    }

    fn parse(report: &str) -> Availability {
        super::parse_status(report, &server())
    }

    fn empty() -> Availability {
        parse("--os-release--\n")
    }

    fn selected_hello(
        variant: newport_protocol::product::Variant,
    ) -> newport_protocol::runtime::Hello {
        use newport_protocol::{product, runtime, worker::admission};
        let mut hello = runtime::Hello {
            product: variant,
            versions: vec![runtime::VERSION],
            instance_id: uuid::Uuid::new_v4().to_string(),
            build_sha256: "b".repeat(64),
            agent_version: None,
            worker_socket: "/tmp/remote-apps-status.sock".into(),
            services: product::CORE_SERVICES
                .iter()
                .chain(&["terminal"])
                .map(|name| {
                    (
                        (*name).into(),
                        runtime::Capability {
                            contract_id: None,
                            versions: vec![1],
                            unavailable: None,
                        },
                    )
                })
                .collect(),
            dependencies: Default::default(),
            limits: Default::default(),
        };
        hello.services.insert(
            admission::CAPABILITY.into(),
            runtime::Capability {
                contract_id: None,
                versions: vec![admission::VERSION],
                unavailable: None,
            },
        );
        if variant == product::Variant::Full {
            hello.services.insert(
                "remote-apps".into(),
                runtime::Capability {
                    contract_id: None,
                    versions: vec![1],
                    unavailable: None,
                },
            );
        }
        hello
    }

    fn retained_installation() -> crate::agent_lifecycle::Status {
        use crate::agent_lifecycle::{BuildRelation, Status};
        use newport_protocol::product::Variant;
        Status {
            installed: Some(true),
            installed_hash: Some("a".repeat(64)),
            bundled_hash: Some("b".repeat(64)),
            bundled_variant: Variant::Full,
            bundled_staged: true,
            build_relation: BuildRelation::Older,
            error: Some("The default product cannot be verified".into()),
            ..Status::default()
        }
    }

    #[test]
    fn a_verified_selected_full_runtime_preserves_the_defaults_diagnostics() {
        use newport_protocol::product::Variant;
        let report = "xwayland=missing\nagent=/c/releases/system/old/current/run-agent\ngap=dbus missing\n--os-release--\nID=debian";
        let hello = selected_hello(Variant::Full);
        for staged in [false, true] {
            let mut value = parse(report);
            let status = crate::agent_lifecycle::Status {
                bundled_staged: staged,
                ..retained_installation()
            };
            super::apply_installation(&mut value, &status, Ok(&hello));
            assert_eq!(value.agent.as_deref(), Some("newport-agent"));
            assert!(value.agent_error.is_none());
            let selected = value.selected_runtime.as_ref().unwrap();
            assert_eq!(selected.product, Product::Full);
            assert_eq!(selected.build_sha256, hello.build_sha256);
            assert!(value.installed_variant.is_none());
            assert_eq!(value.revision, Some("a".repeat(64)));
            assert_eq!(value.build_relation, BuildRelation::Older);
            assert_eq!(
                value.installation_error.as_deref(),
                Some("The default product cannot be verified")
            );
            assert!(value.legacy_agent.is_some());
            assert_eq!(value.gaps.len(), 2);
        }
    }

    #[test]
    fn staged_bytes_do_not_hide_a_failed_selected_runtime() {
        use crate::agent_runtime::Error;
        let status = retained_installation();
        for (error, message) in [
            (
                Error::OlderProtocol,
                "The default product cannot be verified",
            ),
            (
                Error::Unavailable("Candidate identity changed".into()),
                "Candidate identity changed",
            ),
            (
                Error::Interrupted("Connection lost".into()),
                "Connection lost",
            ),
        ] {
            let mut value = empty();
            super::apply_installation(&mut value, &status, Err(&error));
            assert!(value.agent.is_none());
            assert!(value.selected_runtime.is_none());
            assert_eq!(value.agent_error.as_deref(), Some(message));
            assert_eq!(
                value.installation_error.as_deref(),
                Some("The default product cannot be verified")
            );
        }
        let missing = crate::agent_lifecycle::Status {
            installed: Some(false),
            ..Default::default()
        };
        let mut value = empty();
        super::apply_installation(&mut value, &missing, Err(&Error::NotInstalled));
        assert!(value.agent.is_none());
        assert!(value.agent_error.is_none());
    }

    #[test]
    fn the_selected_core_runtime_does_not_inherit_a_full_default() {
        use newport_protocol::product::Variant;
        let status = crate::agent_lifecycle::Status {
            installed_variant: Some(Variant::Full),
            bundled_variant: Variant::Core,
            error: None,
            ..retained_installation()
        };
        let mut value = empty();
        super::apply_installation(&mut value, &status, Ok(&selected_hello(Variant::Core)));
        assert!(value.agent.is_none());
        assert!(value.agent_error.is_none());
        assert_eq!(value.installed_variant, Some(Product::Full));
        assert_eq!(value.selected_runtime.unwrap().product, Product::Core);
        let mut value = empty();
        super::apply_installation(&mut value, &status, Ok(&selected_hello(Variant::Full)));
        assert!(value.agent.is_none());
        assert!(value.selected_runtime.is_none());
        assert_eq!(
            value.agent_error.as_deref(),
            Some("The selected Needle runtime does not match its verified product.")
        );
    }

    #[test]
    fn selected_runtime_identity_must_be_well_formed_and_verified() {
        use newport_protocol::product::Variant;
        let status = retained_installation();
        let valid = selected_hello(Variant::Full);
        let mut invalid = Vec::new();
        let mut hello = valid.clone();
        hello.build_sha256 = "not-a-digest".into();
        invalid.push(hello);
        let mut hello = valid.clone();
        hello.build_sha256 = "c".repeat(64);
        invalid.push(hello);
        let mut hello = valid.clone();
        hello.instance_id.clear();
        invalid.push(hello);
        let mut hello = valid.clone();
        hello.services.remove("lsp");
        invalid.push(hello);
        let mut hello = valid;
        hello.build_sha256 = status.installed_hash.clone().unwrap();
        invalid.push(hello);
        for hello in invalid {
            let mut value = empty();
            super::apply_installation(&mut value, &status, Ok(&hello));
            assert!(value.agent.is_none(), "{hello:?}");
            assert!(value.selected_runtime.is_none());
            assert!(value.agent_error.is_some());
        }
    }

    #[test]
    fn a_selected_full_runtime_must_serve_every_required_protocol() {
        use newport_protocol::product::Variant;
        let status = retained_installation();
        for (service, _) in crate::agent_runtime::NEEDS
            .iter()
            .chain(Variant::Full.needs())
        {
            for missing in [false, true] {
                let mut hello = selected_hello(Variant::Full);
                if missing {
                    hello.services.remove(*service);
                } else {
                    hello.services.get_mut(*service).unwrap().versions = vec![2];
                }
                let mut value = empty();
                super::apply_installation(&mut value, &status, Ok(&hello));
                assert!(value.agent.is_none(), "{service} missing={missing}");
                assert!(value.agent_error.is_some());
            }
        }
        let mut hello = selected_hello(Variant::Full);
        hello.services.get_mut("remote-apps").unwrap().versions = vec![2];
        let mut value = empty();
        super::apply_installation(&mut value, &status, Ok(&hello));
        assert!(value.agent.is_none());
        assert_eq!(
            value.agent_error.as_deref(),
            Some(
                "This server has a newer Newport Remote than this Newport. Update Newport to use it."
            )
        );
    }

    #[test]
    fn unavailable_core_service_dependencies_do_not_block_gui_workers() {
        use newport_protocol::product::Variant;
        let status = retained_installation();
        for (service, _) in crate::agent_runtime::NEEDS {
            let mut hello = selected_hello(Variant::Full);
            hello.services.get_mut(service).unwrap().unavailable =
                Some("Optional dependency is missing".into());
            let mut value = empty();
            super::apply_installation(&mut value, &status, Ok(&hello));
            assert_eq!(value.agent.as_deref(), Some("newport-agent"), "{service}");
            assert!(value.agent_error.is_none());
        }
    }

    #[test]
    fn gui_workers_require_available_compatible_stream_admission_and_service() {
        use newport_protocol::{product::Variant, worker::admission};
        let status = retained_installation();
        for (service, version) in std::iter::once((admission::CAPABILITY, admission::VERSION))
            .chain(Variant::Full.needs().iter().copied())
        {
            for failure in ["missing", "incompatible", "unavailable"] {
                let mut hello = selected_hello(Variant::Full);
                match failure {
                    "missing" => {
                        hello.services.remove(service);
                    }
                    "incompatible" => {
                        hello.services.get_mut(service).unwrap().versions = vec![version + 1]
                    }
                    _ => {
                        hello.services.get_mut(service).unwrap().unavailable =
                            Some("Service dependency failed".into())
                    }
                }
                let mut value = empty();
                super::apply_installation(&mut value, &status, Ok(&hello));
                assert!(value.agent.is_none(), "{service} {failure}");
                assert!(value.agent_error.is_some());
                if failure == "unavailable" {
                    assert_eq!(
                        value.agent_error.as_deref(),
                        Some("Service dependency failed")
                    );
                }
            }
        }
    }

    #[test]
    fn only_a_build_newer_in_what_remote_apps_needs_has_a_newer_newport_remote() {
        use crate::agent_runtime::{Inspected, NEEDS};
        use newport_protocol::product::{CORE_SERVICES, Product, Variant};
        let needed = |service: &str| {
            NEEDS
                .iter()
                .chain(Variant::Full.needs())
                .find(|(name, _)| *name == service)
                .map_or(1, |(_, version)| *version)
        };
        // A full build offering what this Newport needs, but a later version
        // of each of `later`, and none of `missing`.
        let build = |later: &[&str], missing: &[&str]| {
            let services = CORE_SERVICES
                .iter()
                .copied()
                .chain(["terminal", "remote-apps"])
                .filter(|service| !missing.contains(service))
                .map(|service| {
                    let version = needed(service) + u32::from(later.contains(&service));
                    (service.to_owned(), vec![version])
                })
                .collect();
            Inspected::of(Product {
                variant: Variant::Full,
                control_versions: vec![1],
                services,
            })
        };
        let error = |inspected: Inspected| inspected.unmet.as_ref().map(super::unusable);
        assert_eq!(error(build(&[], &[])), None);
        let remote =
            "This server has a newer Newport Remote than this Newport. Update Newport to use it.";
        let agent =
            "This server has a newer Newport agent than this Newport. Update Newport to use it.";
        for (later, message) in [
            (&["remote-apps"][..], remote),
            (&["remote-apps", "integration"][..], remote),
            (&["integration"][..], agent),
            (&["git"][..], agent),
        ] {
            assert_eq!(error(build(later, &[])), Some(message), "{later:?}");
        }
        // Without a service it needs, nothing says it is newer.
        assert_eq!(
            error(build(&[], &["remote-apps"])),
            Some(
                "The Newport agent on this server does not work with this Newport. Update Newport or reinstall its full agent."
            )
        );
    }

    #[test]
    fn a_checked_installation_reports_the_failed_update_to_the_page() {
        let lifecycle = crate::agent_lifecycle::State::default();
        let status = crate::agent_lifecycle::Status::default();
        lifecycle.finish_automatic_update(
            std::time::Instant::now(),
            &status,
            &Err("upload failed".into()),
        );
        let checked = lifecycle.checked(1, Ok(status)).unwrap();
        let mut value = parse("xwayland=24.1.6\n--os-release--\nID=debian");
        super::apply_installation(
            &mut value,
            &checked,
            Err(&crate::agent_runtime::Error::NotInstalled),
        );
        assert_eq!(
            value.automatic_update.unwrap().error.as_deref(),
            Some("upload failed")
        );
    }

    #[test]
    fn status_reads_the_agent_and_distribution() {
        use newport_protocol::{common::Present, desktop::applications as dto};
        let parsed = parse(
            "xwayland=24.1.6\nagent=/home/u/.cache/newport-remote/releases/nix/a/run-agent\n--os-release--\nID=nixos\n",
        );
        assert_eq!(
            parsed.legacy_agent.as_deref(),
            Some("/home/u/.cache/newport-remote/releases/nix/a/run-agent")
        );
        assert_eq!(parsed.os_release, "ID=nixos");
        assert_eq!(parsed.legacy_revision.as_deref(), Some("a"));
        let current = parse(
            "xwayland=24.1.6\nagent=/c/releases/system/f00d/current/run-agent\nsystem=Linux\narchitecture=aarch64\ngap=dbus missing\n--os-release--\n",
        );
        assert_eq!(current.legacy_revision.as_deref(), Some("f00d"));
        assert_eq!(current.system.as_deref(), Some("Linux"));
        assert_eq!(current.architecture.as_deref(), Some("aarch64"));
        assert_eq!(current.gaps[0].requirement, dto::Requirement::Dbus);
        assert!(
            parse("xwayland=24.1.6\nagent=\n--os-release--\n")
                .agent
                .is_none()
        );
        let missing = parse(
            "xwayland=missing\nagent=/a/run-agent\ngap=audio missing\n--os-release--\nID=debian\n",
        );
        assert_eq!(
            missing.gaps,
            [
                dto::Gap {
                    requirement: dto::Requirement::Xwayland,
                    finding: dto::Finding::Missing(Box::new(Present {})),
                },
                dto::Gap {
                    requirement: dto::Requirement::Audio,
                    finding: dto::Finding::Missing(Box::new(Present {})),
                },
            ]
        );
    }

    /// Everything the desktop receives for a probed full runtime.
    #[test]
    fn a_probed_server_reports_its_whole_availability() {
        use newport_protocol::{
            common::Present,
            desktop::{applications as dto, configuration as config},
        };
        let mut status = parse(
            "xwayland=22.1.9\nagent=/c/releases/system/f00d/current/run-agent\n\
             release=/c/releases/system/f00d/current/run-agent\n\
             built=/c/releases/system/f00d/current/run-agent 1792000000\n\
             system=Linux\narchitecture=aarch64\nelevation=doas\nupdater=transactionalUpdate\n\
             gap=dbus failed\n--os-release--\nID=debian\n",
        );
        status.bundled_revision = Some("b".repeat(64));
        status.total = Some(1024);
        let installation = crate::agent_lifecycle::Status {
            installed_variant: Some(Product::Full),
            automatic_update: Some(crate::agent_lifecycle::UpdateOutcome {
                at: 7,
                from: Some("old".into()),
                to: None,
                error: Some("upload failed".into()),
            }),
            ..retained_installation()
        };
        let hello = selected_hello(Product::Full);
        super::apply_installation(&mut status, &installation, Ok(&hello));
        let server = server();
        assert_eq!(
            status,
            Availability {
                agent: Some("newport-agent".into()),
                legacy_agent: Some("/c/releases/system/f00d/current/run-agent".into()),
                legacy_revision: Some("f00d".into()),
                legacy_agents: vec![dto::LegacyAgent {
                    revision: "f00d".into(),
                    agent: "/c/releases/system/f00d/current/run-agent".into(),
                    built_at: Some(1_792_000_000),
                }],
                system: Some("Linux".into()),
                architecture: Some("aarch64".into()),
                gaps: vec![
                    dto::Gap {
                        requirement: dto::Requirement::Xwayland,
                        finding: dto::Finding::Outdated(Box::new("22.1.9".into())),
                    },
                    dto::Gap {
                        requirement: dto::Requirement::Dbus,
                        finding: dto::Finding::Inactive(Box::new(dto::Inactive::Failed)),
                    },
                ],
                host: dto::Host {
                    elevation: dto::Elevation::Doas,
                    updater: Some(dto::Updater::TransactionalUpdate),
                    user_services: false,
                },
                os_release: "ID=debian".into(),
                expected_server: config::Server {
                    id: server.id,
                    name: "Box".into(),
                    ssh_user: "user".into(),
                    ssh_host: "host".into(),
                    ssh_port: 22,
                    identity_file: None,
                    agent_source: None,
                    agent_key_fingerprint: None,
                    auth_method: config::AuthMethod::PublicKey,
                    clipboard_enabled: false,
                    browser_enabled: false,
                    agent_install_approved: false,
                    setup_complete: false,
                    proxy: None,
                },
                bundled_variant: crate::agent_payload::variant(),
                bundled_revision: Some("b".repeat(64)),
                total: Some(1024),
                installed_variant: Some(Product::Full),
                revision: Some("a".repeat(64)),
                build_relation: BuildRelation::Older,
                installation_error: Some("The default product cannot be verified".into()),
                selected_runtime: Some(dto::SelectedRuntime {
                    build_sha256: hello.build_sha256.clone(),
                    product: Product::Full,
                }),
                agent_error: None,
                automatic_update: Some(config::UpdateOutcome {
                    at: 7,
                    from: Some("old".into()),
                    to: None,
                    error: Some("upload failed".into()),
                }),
            }
        );
        let unverified = parse("xwayland=unverified\n--os-release--\n");
        assert_eq!(
            unverified.gaps[0].finding,
            dto::Finding::Unverified(Box::new(Present {}))
        );
    }

    /// The OpenGL check's report for a file system at `root`.
    #[cfg(unix)]
    fn opengl_gap(root: &std::path::Path) -> String {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(concat!("root=\"$1\"\n", opengl_check!()))
            .arg("sh")
            .arg(root)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn nixos_needs_a_graphics_driver_for_opengl() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("etc")).unwrap();
        std::fs::write(root.path().join("etc/NIXOS"), "").unwrap();
        // hardware.graphics off: no driver in /run/opengl-driver.
        assert_eq!(opengl_gap(root.path()), "gap=gles missing\n");
        let vendors = root
            .path()
            .join("run/opengl-driver/share/glvnd/egl_vendor.d");
        std::fs::create_dir_all(&vendors).unwrap();
        assert_eq!(opengl_gap(root.path()), "gap=gles missing\n");
        std::fs::write(vendors.join("50_mesa.json"), "{}").unwrap();
        assert_eq!(opengl_gap(root.path()), "");
    }

    #[test]
    fn probe_lines_become_gaps_and_what_the_account_can_do() {
        use newport_protocol::{common::Present, desktop::applications as dto};
        let parsed = parse(
            "xwayland=24.1.6\nagent=\nelevation=root\nupdater=rpmOstree\nuserServices=1\n\
             gap=audio root\ngap=dbus failed\ngap=fonts missing\ngap=gles missing\n\
             gap=portal missing\ngap=tar missing\ngap=audio loud\n--os-release--\nID=fedora\n",
        );
        let inactive = |requirement, reason| dto::Gap {
            requirement,
            finding: dto::Finding::Inactive(Box::new(reason)),
        };
        let missing = |requirement| dto::Gap {
            requirement,
            finding: dto::Finding::Missing(Box::new(Present {})),
        };
        assert_eq!(
            parsed.gaps,
            [
                inactive(dto::Requirement::Audio, dto::Inactive::Root),
                inactive(dto::Requirement::Dbus, dto::Inactive::Failed),
                missing(dto::Requirement::Fonts),
                missing(dto::Requirement::Gles),
                missing(dto::Requirement::Portal),
            ]
        );
        assert_eq!(
            parsed.host,
            dto::Host {
                elevation: dto::Elevation::Root,
                updater: Some(dto::Updater::RpmOstree),
                user_services: true,
            }
        );
        // Without an elevation line, sudo.
        assert_eq!(
            parse("xwayland=24.1.6\nagent=\n--os-release--\n").host,
            dto::Host {
                elevation: dto::Elevation::Sudo,
                updater: None,
                user_services: false,
            }
        );
    }

    #[test]
    fn status_lists_every_installed_release() {
        let parsed = parse(
            "xwayland=24.1.6\nagent=/c/releases/system/new/current/run-agent\nrelease=/c/releases/system/old/current/run-agent\nrelease=/c/releases/system/new/current/run-agent\n--os-release--\n",
        );
        assert_eq!(parsed.legacy_revision.as_deref(), Some("new"));
        assert_eq!(parsed.legacy_agents[0].revision, "old");
        assert_eq!(
            parsed.legacy_agents[0].agent,
            "/c/releases/system/old/current/run-agent"
        );
        assert_eq!(parsed.legacy_agents[1].revision, "new");
        assert!(parsed.legacy_agents[0].built_at.is_none());
        let built = parse(
            "xwayland=24.1.6\nagent=/c/releases/system/new/current/run-agent\nrelease=/c/releases/system/new/current/run-agent\nbuilt=/c/releases/system/new/current/run-agent 1792000000\n--os-release--\n",
        );
        assert_eq!(built.legacy_agents[0].built_at, Some(1_792_000_000));
    }
}
