//! Read models for Remote apps: the installed streaming agent and the
//! server's applications. Sessions are live resources and stay out of here.
use crate::server_session::ServerSession;
use crate::ssh::shell_quote as quote;
use serde_json::{Value, json};
use std::sync::Arc;

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

/// Reports the installed agent, the programs it needs that the server lacks,
/// the system and architecture that select the agent to install, and the
/// distribution, in one round trip. Agents built on the server with Nix carry
/// their own programs.
const REPORT: &str = r#"printf 'agent=%s\n' "$agent"
for release in "$@"; do
    printf 'release=%s\n' "$release"
    built=$(head -c 32 "${release%/run-agent}/built-at" 2>/dev/null) && printf 'built=%s %s\n' "$release" "$built"
done
printf 'system=%s\n' "$(uname -s)"
printf 'architecture=%s\n' "$(uname -m)"
case "$agent" in
*/releases/nix/*) ;;
*)
    for program in tar gzip; do command -v "$program" >/dev/null 2>&1 || echo "missing=$program"; done
    command -v Xvfb >/dev/null 2>&1 || echo 'missing=xvfb'
    command -v dbus-daemon >/dev/null 2>&1 && command -v dbus-run-session >/dev/null 2>&1 || echo 'missing=dbus'
    command -v pw-record >/dev/null 2>&1 || command -v parec >/dev/null 2>&1 || echo 'missing=audio'
    # Apps draw text through fontconfig; with no font at all they can't open
    # a window. Without fontconfig there is nothing that uses it yet.
    if command -v fc-list >/dev/null 2>&1 && [ -z "$(fc-list : family | head -n 1)" ]; then
        echo 'missing=fonts'
    fi
    ;;
esac
echo '--os-release--'
cat /etc/os-release 2>/dev/null || cat /usr/lib/os-release 2>/dev/null
exit 0"#;

/// Lists the installed agent's applications. Finding the agent in the same
/// command means the listing never uses a path an earlier check found.
const LIST: &str = r#"[ -n "$agent" ] || { echo 'Newport Remote isn’t installed on this server.' >&2; exit 3; }
exec "$agent" --catalog"#;

/// The agent's own limit for a catalogue, icons included.
const MAX_CATALOGUE: usize = 8 * 1024 * 1024;
/// Rendering icons for many applications can take a while on a first run.
const CATALOGUE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// `{"agent": path | null, "revision": release | null, "system": uname -s,
/// "architecture": uname -m, "missing": [...], "osRelease": text}`.
pub(crate) async fn status(owner: &Arc<ServerSession>) -> Result<Value, String> {
    let server = owner.server().clone();
    let lease = owner.acquire(&server).await?;
    let output = lease
        .execute(
            &format!("sh -c {}", quote(&format!("{FIND_AGENT}{REPORT}"))),
            None,
        )
        .await?;
    Ok(parse_status(&output))
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
    let zypper_dbus = zypper_dbus(release);
    // Every installed release, so Newport can launch the one it carries even
    // when another Newport installed a different one more recently.
    // When each release's agent was built, for ordering them; releases
    // installed before agents said so have none.
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
    let missing: Vec<Value> = head
        .lines()
        .filter_map(|line| line.strip_prefix("missing="))
        .filter_map(|program| match program.trim() {
            // The installer unpacks the agent with these; minimal images
            // leave them out. Its other programs come with every base system.
            program @ ("tar" | "gzip") => Some(json!({
                "id": program,
                "name": program,
                "purpose": "Installing Newport Remote",
                "required": true,
                "neededToInstall": true,
                "packages": {
                    "apt": program,
                    "dnf": program,
                    "zypper": program,
                    "pacman": program,
                    "apk": program,
                    "xbps": program,
                    "emerge": if program == "tar" { "app-arch/tar" } else { "app-arch/gzip" },
                    "nix": if program == "tar" { "gnutar" } else { "gzip" },
                },
            })),
            "xvfb" => Some(json!({
                "id": "xvfb",
                "name": "Xvfb",
                "purpose": "Applications",
                "required": true,
                "packages": {
                    "apt": "xvfb",
                    "dnf": "xorg-x11-server-Xvfb",
                    "zypper": "xorg-x11-server-Xvfb",
                    "pacman": "xorg-server-xvfb",
                    "apk": "xvfb",
                    "xbps": "xorg-server-xvfb",
                    "emerge": "x11-base/xorg-server[xvfb]",
                    "nix": "xorg.xvfb",
                },
            })),
            "dbus" => Some(json!({
                "id": "dbus",
                "name": "D-Bus",
                "purpose": "Applications",
                "required": true,
                "neededToInstall": true,
                "packages": {
                    "apt": "dbus",
                    "dnf": "dbus-daemon",
                    "zypper": zypper_dbus,
                    "pacman": "dbus",
                    "apk": "dbus",
                    "xbps": "dbus",
                    "emerge": "sys-apps/dbus",
                    "nix": "dbus",
                },
            })),
            "fonts" => Some(json!({
                "id": "fonts",
                // Every command below installs this face; with no font at
                // all it is the one apps fall back to.
                "name": "DejaVu Sans Mono",
                "purpose": "Text in apps",
                "required": true,
                "packages": {
                    "apt": "fonts-dejavu-core",
                    "dnf": "dejavu-sans-mono-fonts",
                    "zypper": "dejavu-fonts",
                    "pacman": "ttf-dejavu",
                    "apk": "font-dejavu",
                    "xbps": "dejavu-fonts-ttf",
                    "emerge": "media-fonts/dejavu",
                    "nix": "dejavu_fonts",
                },
            })),
            "audio" => Some(json!({
                "id": "audio",
                "name": "PipeWire tools",
                "purpose": "Sound from remote apps",
                "required": false,
                "packages": {
                    "apt": "pipewire-bin",
                    "dnf": "pipewire-utils",
                    "zypper": "pipewire-tools",
                    "pacman": "pipewire-audio",
                    "apk": "pipewire-tools",
                    "xbps": "pipewire",
                    "emerge": "media-video/pipewire",
                    "nix": "pipewire",
                },
            })),
            _ => None,
        })
        .collect();
    json!({
        "agent": agent,
        "revision": revision,
        "agents": agents,
        "system": field("system"),
        "architecture": field("architecture"),
        "missing": missing,
        "osRelease": release.trim(),
    })
}

/// openSUSE moved `dbus-daemon` and `dbus-run-session` from `dbus-1` into
/// `dbus-1-daemon` in Tumbleweed and Leap 16 (SUSE Linux 16); Leap and SLES
/// 15 still ship them in `dbus-1`.
fn zypper_dbus(os_release: &str) -> &'static str {
    let field = |name: &str| {
        os_release.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.strip_prefix('=')?;
            Some(value.trim().trim_matches('"'))
        })
    };
    let major =
        field("VERSION_ID").and_then(|version| version.split('.').next()?.parse::<u32>().ok());
    match (field("ID"), major) {
        (Some("opensuse-leap" | "sles" | "sled" | "sle-micro"), Some(major)) if major < 16 => {
            "dbus-1"
        }
        _ => "dbus-1-daemon",
    }
}

/// The agent's catalogue as it prints it: `{"apps": [...]}` with PNG icons.
pub(crate) async fn catalogue(owner: &Arc<ServerSession>) -> Result<Value, String> {
    let server = owner.server().clone();
    let lease = owner.acquire(&server).await?;
    let output = read_command(
        &lease,
        &format!("sh -c {}", quote(&format!("{FIND_AGENT}{LIST}"))),
        MAX_CATALOGUE,
        CATALOGUE_TIMEOUT,
    )
    .await
    .map_err(|error| match error {
        ReadError::TimedOut => "Listing applications took longer than a minute.".to_owned(),
        ReadError::Failed(message) => message,
    })?;
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
async fn read_command(
    lease: &crate::server_session::Lease,
    command: &str,
    limit: usize,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, ReadError> {
    use russh::ChannelMsg;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut channel = lease
        .exec_channel(command)
        .await
        .map_err(ReadError::Failed)?;
    let (mut out, mut err, mut status) = (Vec::new(), Vec::new(), None);
    loop {
        let message = tokio::select! {
            message = channel.wait() => message,
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
    fn status_reads_the_agent_and_distribution() {
        let parsed = super::parse_status(
            "agent=/home/u/.cache/newport-remote/releases/nix/a/run-agent\n--os-release--\nID=nixos\n",
        );
        assert_eq!(
            parsed["agent"],
            "/home/u/.cache/newport-remote/releases/nix/a/run-agent"
        );
        assert_eq!(parsed["osRelease"], "ID=nixos");
        assert_eq!(parsed["revision"], "a");
        let current = super::parse_status(
            "agent=/c/releases/system/f00d/current/run-agent\nsystem=Linux\narchitecture=aarch64\nmissing=dbus\n--os-release--\n",
        );
        assert_eq!(current["revision"], "f00d");
        assert_eq!(current["system"], "Linux");
        assert_eq!(current["architecture"], "aarch64");
        assert_eq!(current["missing"][0]["name"], "D-Bus");
        assert!(super::parse_status("agent=\n--os-release--\n")["agent"].is_null());
        let missing = super::parse_status(
            "agent=/a/run-agent\nmissing=xvfb\nmissing=audio\n--os-release--\nID=debian\n",
        );
        assert_eq!(missing["missing"][0]["name"], "Xvfb");
        assert_eq!(missing["missing"][0]["required"], true);
        assert_eq!(missing["missing"][1]["packages"]["apt"], "pipewire-bin");
    }

    #[test]
    fn package_names_follow_the_distribution_release() {
        let dbus = |release: &str| {
            super::parse_status(&format!("agent=\nmissing=dbus\n--os-release--\n{release}"))
                ["missing"][0]["packages"]["zypper"]
                .clone()
        };
        assert_eq!(
            dbus("ID=\"opensuse-tumbleweed\"\nVERSION_ID=\"20261018\""),
            "dbus-1-daemon"
        );
        assert_eq!(
            dbus("ID=\"opensuse-leap\"\nVERSION_ID=\"16.0\""),
            "dbus-1-daemon"
        );
        assert_eq!(dbus("ID=\"opensuse-leap\"\nVERSION_ID=\"15.6\""), "dbus-1");
        assert_eq!(dbus("ID=\"sles\"\nVERSION_ID=\"15.7\""), "dbus-1");
        let unpack = super::parse_status("agent=\nmissing=tar\nmissing=gzip\n--os-release--\n");
        assert_eq!(unpack["missing"][0]["name"], "tar");
        assert_eq!(unpack["missing"][1]["packages"]["zypper"], "gzip");
        assert_eq!(unpack["missing"][1]["neededToInstall"], true);
        let fonts = super::parse_status("agent=\nmissing=fonts\n--os-release--\n");
        assert_eq!(fonts["missing"][0]["packages"]["zypper"], "dejavu-fonts");
        assert_eq!(fonts["missing"][0]["required"], true);
        let audio = super::parse_status("agent=\nmissing=audio\n--os-release--\nID=arch\n");
        assert_eq!(audio["missing"][0]["packages"]["pacman"], "pipewire-audio");
    }

    #[test]
    fn status_lists_every_installed_release() {
        let parsed = super::parse_status(
            "agent=/c/releases/system/new/current/run-agent\nrelease=/c/releases/system/old/current/run-agent\nrelease=/c/releases/system/new/current/run-agent\n--os-release--\n",
        );
        assert_eq!(parsed["revision"], "new");
        assert_eq!(parsed["agents"][0]["revision"], "old");
        assert_eq!(
            parsed["agents"][0]["agent"],
            "/c/releases/system/old/current/run-agent"
        );
        assert_eq!(parsed["agents"][1]["revision"], "new");
        assert!(parsed["agents"][0]["builtAt"].is_null());
        let built = super::parse_status(
            "agent=/c/releases/system/new/current/run-agent\nrelease=/c/releases/system/new/current/run-agent\nbuilt=/c/releases/system/new/current/run-agent 1792000000\n--os-release--\n",
        );
        assert_eq!(built["agents"][0]["builtAt"], 1_792_000_000);
    }
}
