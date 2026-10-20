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
    if [ -x "$release/current/run-agent" ] && [ -f "$release/current/.validated-v6" ]; then
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
printf 'system=%s\n' "$(uname -s)"
printf 'architecture=%s\n' "$(uname -m)"
case "$agent" in
*/releases/nix/*) ;;
*)
    command -v Xvfb >/dev/null 2>&1 || echo 'missing=xvfb'
    command -v dbus-daemon >/dev/null 2>&1 && command -v dbus-run-session >/dev/null 2>&1 || echo 'missing=dbus'
    command -v pw-record >/dev/null 2>&1 || command -v parec >/dev/null 2>&1 || echo 'missing=audio'
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
    let revision = agent
        .and_then(|agent| agent.rsplit_once("/releases/"))
        .and_then(|(_, rest)| rest.split('/').nth(1));
    let missing: Vec<Value> = head
        .lines()
        .filter_map(|line| line.strip_prefix("missing="))
        .filter_map(|program| match program.trim() {
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
                    "zypper": "dbus-1",
                    "pacman": "dbus",
                    "apk": "dbus",
                    "xbps": "dbus",
                    "emerge": "sys-apps/dbus",
                    "nix": "dbus",
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
                    "pacman": "pipewire",
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
        "system": field("system"),
        "architecture": field("architecture"),
        "missing": missing,
        "osRelease": release.trim(),
    })
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
}
