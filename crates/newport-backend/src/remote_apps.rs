//! Read models for Remote Apps: the shared agent product and the
//! server's applications. Sessions are live resources and stay out of here.
use crate::server_session::ServerSession;
use crate::ssh::shell_quote as quote;
use newport_domain::requirements::{Finding, Gap, Host, Inactive, Requirement};
use newport_domain::server_os::{Elevation, Updater};
use serde_json::{Value, json};
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

/// Reports legacy installations for migration and host dependencies separately.
/// Legacy Nix resources do not satisfy the shared full agent’s dependencies.
const REPORT: &str = r#"printf 'agent=%s\n' "$agent"
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
# Where EGL is installed, GTK 4 loads OpenGL ES through libepoxy and exits
# when it is missing, as on minimal cloud images. Without EGL it draws with
# Cairo and needs none. NixOS's libepoxy falls back to the libglvnd in its
# own closure, so apps there always find it.
# musl's ldconfig rejects -p but exits 0, so look for the library first.
gles=
[ -e /etc/NIXOS ] && gles=1
egl=
for f in /usr/lib/libEGL.so.1 /usr/lib64/libEGL.so.1 /usr/lib/*-linux-gnu/libEGL.so.1; do
    [ -e "$f" ] && egl=1
done
[ -n "$egl" ] || { ldconfig -p 2>/dev/null || /sbin/ldconfig -p 2>/dev/null; } | grep -q 'libEGL\.so\.1 ' || gles=1
for f in /usr/lib/libGLESv2.so.2 /usr/lib64/libGLESv2.so.2 /usr/lib/*-linux-gnu/libGLESv2.so.2; do
    [ -e "$f" ] && gles=1
done
[ -n "$gles" ] || { ldconfig -p 2>/dev/null || /sbin/ldconfig -p 2>/dev/null; } | grep -q 'libGLESv2\.so\.2 ' || echo 'gap=gles missing'
# Snap and Flatpak apps get file dialogs from the GTK portal backend.
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
exit 0"#;

/// The agent's own limit for a catalogue, icons included.
const MAX_CATALOGUE: usize = 8 * 1024 * 1024;
/// Rendering icons for many applications can take a while on a first run.
const CATALOGUE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Verified shared product, legacy migration evidence, and host dependencies.
/// The agent marker only becomes present for a compatible full installation.
pub(crate) async fn status(owner: &Arc<ServerSession>) -> Result<Value, String> {
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
    let mut value = parse_status(&output);
    value["expectedConnection"] = json!(server);
    value["bundledVariant"] = json!(crate::agent_payload::variant());
    let architecture = value["architecture"].as_str().unwrap_or_default();
    let payload = crate::agent_payload::payload(architecture);
    value["bundledRevision"] = json!(payload.map(|p| p.sha256));
    value["total"] = json!(payload.map(|p| p.size));
    if value["system"] != "Linux" || payload.is_none() {
        return Ok(value);
    }
    let installation = owner.check_installation().await?;
    let capability = if installation.installed_variant
        == Some(newport_release::agent_product::Variant::Full)
        && installation.error.is_none()
    {
        let product = owner
            .inspect_product(
                installation
                    .installed_hash
                    .as_deref()
                    .ok_or("Missing full agent identity")?,
            )
            .await?;
        product
            .services
            .get("remote-apps")
            .is_some_and(|versions| versions.contains(&1))
            && product
                .services
                .get("gui-launch")
                .is_some_and(|versions| versions.contains(&4))
    } else {
        false
    };
    apply_installation(&mut value, &installation, capability);
    Ok(value)
}

fn parse_status(output: &str) -> Value {
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
    // Keep old release identities as migration evidence. They do not select
    // launches or participate in the unified product's update ordering.
    let built: std::collections::HashMap<&str, u64> = head
        .lines()
        .filter_map(|line| line.strip_prefix("built="))
        .filter_map(|line| {
            let (path, seconds) = line.trim().rsplit_once(' ')?;
            Some((path, seconds.trim().parse().ok()?))
        })
        .collect();
    let agents: Vec<Value> = head
        .lines()
        .filter_map(|line| line.strip_prefix("release="))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .filter_map(|path| {
            Some(json!({
                "revision": revision_of(path)?,
                "agent": path,
                "builtAt": built.get(path),
            }))
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
    json!({
        "agent": null,
        "legacyAgent": agent,
        "legacyRevision": revision,
        "legacyAgents": agents,
        "system": field("system"),
        "architecture": field("architecture"),
        "gaps": gaps,
        "host": host,
        "osRelease": release.trim(),
    })
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
            let status = parse_status(&report);
            let usable = version == "24.1.6";
            let gaps: Vec<Gap> = serde_json::from_value(status["gaps"].clone()).unwrap();
            assert_eq!(gaps.len(), if usable { 1 } else { 2 }, "{version}");
            if !usable {
                assert_eq!(gaps[0].requirement, Requirement::Xwayland);
                assert!(gaps[0].optional());
            }
            assert_eq!(gaps.last().unwrap().requirement, Requirement::Dbus);
        }
    }
}

/// Only a verified full product with a supported service is launchable. Host
/// dependencies and entitlement remain independent from installation identity.
fn apply_installation(
    value: &mut Value,
    installation: &crate::agent_lifecycle::Status,
    compatible: bool,
) {
    use newport_release::agent_product::Variant;
    value["agent"] = Value::Null;
    value["installedVariant"] = json!(installation.installed_variant);
    value["revision"] = json!(installation.installed_hash);
    value["buildRelation"] = json!(installation.build_relation);
    value["agentError"] = json!(installation.error);
    value["automaticUpdate"] = json!(installation.automatic_update);
    if installation.installed_variant == Some(Variant::Full) && installation.error.is_none() {
        if compatible {
            // This is an identity marker, never an executable supplied to a shell.
            // Workers launch the runtime's immutable build through admission.
            value["agent"] = json!("newport-agent");
        } else {
            value["agentError"] = json!(
                "The installed agent does not support this Remote Apps protocol. Update Newport or reinstall its full agent."
            );
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
        "missing" => Finding::Missing,
        "root" => Finding::Inactive {
            reason: Inactive::Root,
        },
        "stopped" => Finding::Inactive {
            reason: Inactive::Stopped,
        },
        "failed" => Finding::Inactive {
            reason: Inactive::Failed,
        },
        _ => return None,
    };
    Some(Gap {
        requirement,
        finding,
    })
}

/// The agent's catalogue as it prints it: `{"apps": [...]}` with PNG icons.
pub(crate) async fn catalogue(owner: &Arc<ServerSession>) -> Result<Value, String> {
    let needle = owner.needle(owner.server())?;
    let (channel, worker) = needle
        .launch_channel(crate::agent::Service::RemoteApps {
            arguments: vec!["--catalog".into()],
        })
        .await?;
    let output = read_channel(channel, MAX_CATALOGUE, CATALOGUE_TIMEOUT)
        .await
        .map_err(|error| match error {
            ReadError::TimedOut => "Listing applications took longer than a minute.".to_owned(),
            ReadError::Failed(message) => message,
        })?;
    drop(worker);
    needle.validate()?;
    let catalogue: Value = serde_json::from_slice(&output)
        .map_err(|error| format!("The agent returned an invalid catalogue: {error}"))?;
    if !catalogue["apps"].is_array() {
        return Err("The agent returned an invalid catalogue.".into());
    }
    Ok(catalogue)
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
    #[test]
    fn installation_identity_is_independent_of_dependencies_and_legacy_agents() {
        use crate::agent_lifecycle::{BuildRelation, Status};
        use newport_release::agent_product::Variant;
        let report = "xwayland=missing\nagent=/c/releases/system/old/current/run-agent\ngap=dbus missing\n--os-release--\nID=debian";
        for variant in [None, Some(Variant::Core), Some(Variant::Full)] {
            let mut value = super::parse_status(report);
            let status = Status {
                installed: Some(true),
                installed_variant: variant,
                installed_hash: Some("a".repeat(64)),
                build_relation: BuildRelation::Different,
                ..Status::default()
            };
            super::apply_installation(&mut value, &status, true);
            assert_eq!(value["agent"].is_string(), variant == Some(Variant::Full));
            assert!(value["legacyAgent"].is_string());
            assert_eq!(value["gaps"].as_array().unwrap().len(), 2);
        }
        let mut value = super::parse_status(report);
        let failed = Status {
            installed: Some(true),
            automatic_update: Some(crate::agent_lifecycle::UpdateOutcome {
                at: 1,
                from: Some("a".repeat(64)),
                to: Some("b".repeat(64)),
                error: Some("upload failed".into()),
            }),
            ..Status::default()
        };
        super::apply_installation(&mut value, &failed, true);
        assert_eq!(value["automaticUpdate"]["error"], "upload failed");
        assert_eq!(value["automaticUpdate"]["to"], "b".repeat(64));
        let mut full = Status {
            installed: Some(true),
            installed_variant: Some(Variant::Full),
            ..Status::default()
        };
        super::apply_installation(&mut value, &full, false);
        assert!(value["agent"].is_null());
        assert!(value["agentError"].as_str().unwrap().contains("protocol"));
        full.error = Some("Unverified identity".into());
        super::apply_installation(&mut value, &full, true);
        assert!(value["agent"].is_null());
        assert_eq!(value["agentError"], "Unverified identity");
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
        let mut value = super::parse_status("xwayland=24.1.6\n--os-release--\nID=debian");
        super::apply_installation(&mut value, &checked, true);
        assert_eq!(value["automaticUpdate"]["error"], "upload failed");
    }

    #[test]
    fn status_reads_the_agent_and_distribution() {
        let parsed = super::parse_status(
            "xwayland=24.1.6\nagent=/home/u/.cache/newport-remote/releases/nix/a/run-agent\n--os-release--\nID=nixos\n",
        );
        assert_eq!(
            parsed["legacyAgent"],
            "/home/u/.cache/newport-remote/releases/nix/a/run-agent"
        );
        assert_eq!(parsed["osRelease"], "ID=nixos");
        assert_eq!(parsed["legacyRevision"], "a");
        let current = super::parse_status(
            "xwayland=24.1.6\nagent=/c/releases/system/f00d/current/run-agent\nsystem=Linux\narchitecture=aarch64\ngap=dbus missing\n--os-release--\n",
        );
        assert_eq!(current["legacyRevision"], "f00d");
        assert_eq!(current["system"], "Linux");
        assert_eq!(current["architecture"], "aarch64");
        assert_eq!(current["gaps"][0]["requirement"], "dbus");
        assert!(
            super::parse_status("xwayland=24.1.6\nagent=\n--os-release--\n")["agent"].is_null()
        );
        let missing = super::parse_status(
            "xwayland=missing\nagent=/a/run-agent\ngap=audio missing\n--os-release--\nID=debian\n",
        );
        assert_eq!(
            missing["gaps"],
            serde_json::json!([
                {"requirement": "xwayland", "state": "missing"},
                {"requirement": "audio", "state": "missing"},
            ])
        );
    }

    #[test]
    fn probe_lines_become_gaps_and_what_the_account_can_do() {
        let parsed = super::parse_status(
            "xwayland=24.1.6\nagent=\nelevation=root\nupdater=rpmOstree\nuserServices=1\n\
             gap=audio root\ngap=dbus failed\ngap=fonts missing\ngap=gles missing\n\
             gap=portal missing\ngap=tar missing\ngap=audio loud\n--os-release--\nID=fedora\n",
        );
        assert_eq!(
            parsed["gaps"],
            serde_json::json!([
                {"requirement": "audio", "state": "inactive", "reason": "root"},
                {"requirement": "dbus", "state": "inactive", "reason": "failed"},
                {"requirement": "fonts", "state": "missing"},
                {"requirement": "gles", "state": "missing"},
                {"requirement": "portal", "state": "missing"},
            ])
        );
        assert_eq!(
            parsed["host"],
            serde_json::json!({"elevation": "root", "updater": "rpmOstree", "userServices": true})
        );
        // Without an elevation line, sudo.
        let older = super::parse_status("xwayland=24.1.6\nagent=\n--os-release--\n");
        assert_eq!(
            older["host"],
            serde_json::json!({"elevation": "sudo", "updater": null, "userServices": false})
        );
    }

    #[test]
    fn status_lists_every_installed_release() {
        let parsed = super::parse_status(
            "xwayland=24.1.6\nagent=/c/releases/system/new/current/run-agent\nrelease=/c/releases/system/old/current/run-agent\nrelease=/c/releases/system/new/current/run-agent\n--os-release--\n",
        );
        assert_eq!(parsed["legacyRevision"], "new");
        assert_eq!(parsed["legacyAgents"][0]["revision"], "old");
        assert_eq!(
            parsed["legacyAgents"][0]["agent"],
            "/c/releases/system/old/current/run-agent"
        );
        assert_eq!(parsed["legacyAgents"][1]["revision"], "new");
        assert!(parsed["legacyAgents"][0]["builtAt"].is_null());
        let built = super::parse_status(
            "xwayland=24.1.6\nagent=/c/releases/system/new/current/run-agent\nrelease=/c/releases/system/new/current/run-agent\nbuilt=/c/releases/system/new/current/run-agent 1792000000\n--os-release--\n",
        );
        assert_eq!(built["legacyAgents"][0]["builtAt"], 1_792_000_000);
    }
}
