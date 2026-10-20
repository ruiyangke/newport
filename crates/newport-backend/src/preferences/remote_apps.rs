//! Remote apps preferences: streaming settings for every server, the
//! commands a user saved per server, and the servers last seen with the agent
//! installed. Nothing here is secret.
use super::Preferences;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const SETTINGS: &str = "remoteAppsSettings";
const COMMANDS: &str = "remoteAppsCommands";
/// Servers last seen with the agent installed, so a first visit after
/// starting Newport can show their apps loading instead of the welcome screen.
const INSTALLED: &str = "remoteAppsInstalled";
const MAX_COMMANDS: usize = 500;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    #[default]
    Auto,
    Lossless,
    Fast,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Display {
    #[default]
    Auto,
    X11,
    Wayland,
}

/// Stored, so it reads what an older or newer build wrote: unknown fields
/// are ignored and missing ones take their default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub audio: bool,
    pub codec: Codec,
    /// 15, 30 or 60.
    pub fps_goal: u32,
    pub hud: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            audio: true,
            codec: Codec::Auto,
            fps_goal: 60,
            hud: false,
        }
    }
}

/// A command saved as an application tile on one server. Stored, so unknown
/// fields from another build are ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub id: Uuid,
    pub server_id: Uuid,
    pub name: String,
    /// As typed; the page parses it on every launch.
    pub command: String,
    #[serde(default)]
    pub display: Display,
}

fn validate_settings(settings: &Settings) -> Result<(), String> {
    if ![15, 30, 60].contains(&settings.fps_goal) {
        return Err("Frame rate must be 15, 30 or 60 fps.".into());
    }
    Ok(())
}

fn validate_command(command: &Command) -> Result<(), String> {
    if command.name.trim().is_empty()
        || command.name.len() > 256
        || command.name.chars().any(char::is_control)
    {
        return Err("A command name must contain 1–256 bytes without control characters.".into());
    }
    if command.command.trim().is_empty()
        || command.command.len() > 16384
        || command.command.contains(['\n', '\r', '\0'])
    {
        return Err("A command must be one line of 1–16384 bytes.".into());
    }
    Ok(())
}

fn decode_settings(value: Option<serde_json::Value>) -> Result<Settings, String> {
    let Some(value) = value else {
        return Ok(Settings::default());
    };
    let settings: Settings = serde_json::from_value(value)
        .map_err(|e| format!("Remote apps settings could not be read: {e}"))?;
    validate_settings(&settings)?;
    Ok(settings)
}

fn decode_commands(value: Option<serde_json::Value>) -> Result<Vec<Command>, String> {
    let rows: Vec<Command> = match value {
        None => Vec::new(),
        Some(value) => serde_json::from_value(value)
            .map_err(|e| format!("Saved commands could not be read: {e}"))?,
    };
    if rows.len() > MAX_COMMANDS {
        return Err("Saved command limit exceeded.".into());
    }
    let mut ids = std::collections::HashSet::new();
    for row in &rows {
        validate_command(row)?;
        if !ids.insert(row.id) {
            return Err("Saved commands contain duplicate entries.".into());
        }
    }
    Ok(rows)
}

fn for_server(rows: Vec<Command>, server_id: Uuid) -> Vec<Command> {
    rows.into_iter()
        .filter(|row| row.server_id == server_id)
        .collect()
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

pub async fn remote_apps_settings(state: &Preferences) -> Result<Settings, String> {
    let state = state.clone();
    blocking(move || decode_settings(state.get(SETTINGS)?)).await
}

pub async fn remote_apps_settings_save(
    state: &Preferences,
    settings: Settings,
) -> Result<Settings, String> {
    validate_settings(&settings)?;
    let state = state.clone();
    blocking(move || {
        state.update(|values| {
            values.insert(
                SETTINGS.into(),
                serde_json::to_value(&settings).map_err(|e| e.to_string())?,
            );
            Ok(settings)
        })
    })
    .await
}

pub async fn remote_apps_commands(
    state: &Preferences,
    server_id: Uuid,
) -> Result<Vec<Command>, String> {
    let state = state.clone();
    blocking(move || {
        Ok(for_server(
            decode_commands(state.get(COMMANDS)?)?,
            server_id,
        ))
    })
    .await
}

fn decode_installed(value: Option<serde_json::Value>) -> Result<Vec<Uuid>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    serde_json::from_value(value).map_err(|e| format!("Remote apps servers could not be read: {e}"))
}

/// Servers last seen with the agent installed.
pub async fn remote_apps_installed(state: &Preferences) -> Result<Vec<Uuid>, String> {
    let state = state.clone();
    blocking(move || decode_installed(state.get(INSTALLED)?)).await
}

/// Record whether a server has the agent; returns every such server.
pub async fn remote_apps_installed_save(
    state: &Preferences,
    server_id: Uuid,
    installed: bool,
) -> Result<Vec<Uuid>, String> {
    let state = state.clone();
    blocking(move || {
        state.update(|values| {
            let mut servers = decode_installed(values.get(INSTALLED).cloned())?;
            servers.retain(|server| *server != server_id);
            if installed {
                servers.push(server_id);
            }
            values.insert(
                INSTALLED.into(),
                serde_json::to_value(&servers).map_err(|e| e.to_string())?,
            );
            Ok(servers)
        })
    })
    .await
}

/// Adds or replaces a command; returns the server's commands.
pub async fn remote_apps_command_save(
    state: &Preferences,
    command: Command,
) -> Result<Vec<Command>, String> {
    validate_command(&command)?;
    let state = state.clone();
    blocking(move || {
        state.update(|values| {
            let mut rows = decode_commands(values.get(COMMANDS).cloned())?;
            match rows.iter().position(|row| row.id == command.id) {
                Some(index) if rows[index].server_id != command.server_id => {
                    return Err("A saved command cannot move to another server.".into());
                }
                Some(index) => rows[index] = command.clone(),
                None if rows.len() >= MAX_COMMANDS => {
                    return Err("Remove an unused command before adding another.".into());
                }
                None => rows.push(command.clone()),
            }
            values.insert(
                COMMANDS.into(),
                serde_json::to_value(&rows).map_err(|e| e.to_string())?,
            );
            Ok(for_server(rows, command.server_id))
        })
    })
    .await
}

/// Removes a command; returns the server's remaining commands.
pub async fn remote_apps_command_remove(
    state: &Preferences,
    server_id: Uuid,
    id: Uuid,
) -> Result<Vec<Command>, String> {
    let state = state.clone();
    blocking(move || {
        state.update(|values| {
            let mut rows = decode_commands(values.get(COMMANDS).cloned())?;
            rows.retain(|row| row.server_id != server_id || row.id != id);
            values.insert(
                COMMANDS.into(),
                serde_json::to_value(&rows).map_err(|e| e.to_string())?,
            );
            Ok(for_server(rows, server_id))
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_from_another_build_still_read() {
        let settings: Settings =
            serde_json::from_value(serde_json::json!({"audio": false, "future": 1})).unwrap();
        assert_eq!(
            settings,
            Settings {
                audio: false,
                ..Settings::default()
            }
        );
        let command: Command = serde_json::from_value(serde_json::json!({
            "id": Uuid::nil(), "serverId": Uuid::nil(), "name": "Tool",
            "command": "tool", "future": true
        }))
        .unwrap();
        assert_eq!(command.display, Display::Auto);
    }

    #[tokio::test]
    async fn installed_servers_are_remembered_and_forgotten() {
        let directory = tempfile::tempdir().unwrap();
        let state = Preferences::open(directory.path()).unwrap();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(remote_apps_installed(&state).await.unwrap().is_empty());
        remote_apps_installed_save(&state, a, true).await.unwrap();
        // Saving twice keeps one entry.
        assert_eq!(
            remote_apps_installed_save(&state, a, true).await.unwrap(),
            [a]
        );
        remote_apps_installed_save(&state, b, true).await.unwrap();
        assert_eq!(
            remote_apps_installed_save(&state, a, false).await.unwrap(),
            [b]
        );
        let reopened = Preferences::open(directory.path()).unwrap();
        assert_eq!(remote_apps_installed(&reopened).await.unwrap(), [b]);
    }

    #[tokio::test]
    async fn settings_and_commands_round_trip_per_server() {
        let directory = tempfile::tempdir().unwrap();
        let state = Preferences::open(directory.path()).unwrap();
        assert_eq!(
            remote_apps_settings(&state).await.unwrap(),
            Settings::default()
        );
        let quiet = Settings {
            audio: false,
            fps_goal: 30,
            ..Settings::default()
        };
        remote_apps_settings_save(&state, quiet.clone())
            .await
            .unwrap();
        assert_eq!(remote_apps_settings(&state).await.unwrap(), quiet);
        assert!(
            remote_apps_settings_save(
                &state,
                Settings {
                    fps_goal: 24,
                    ..Settings::default()
                }
            )
            .await
            .is_err()
        );

        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let command = Command {
            id: Uuid::new_v4(),
            server_id: a,
            name: "Viewer".into(),
            command: "GDK_SCALE=2 /opt/viewer/bin/viewer --safe-mode".into(),
            display: Display::Wayland,
        };
        let saved = remote_apps_command_save(&state, command.clone())
            .await
            .unwrap();
        assert_eq!(saved, std::slice::from_ref(&command));
        assert!(remote_apps_commands(&state, b).await.unwrap().is_empty());
        let renamed = Command {
            name: "Safe viewer".into(),
            ..command.clone()
        };
        remote_apps_command_save(&state, renamed.clone())
            .await
            .unwrap();
        assert_eq!(remote_apps_commands(&state, a).await.unwrap(), [renamed]);
        let moved = Command {
            server_id: b,
            ..command.clone()
        };
        assert!(remote_apps_command_save(&state, moved).await.is_err());
        let multiline = Command {
            command: "a\nb".into(),
            ..command.clone()
        };
        assert!(remote_apps_command_save(&state, multiline).await.is_err());
        assert!(
            remote_apps_command_remove(&state, a, command.id)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
