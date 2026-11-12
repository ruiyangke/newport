//! Explicit typed reply projections into existing UI render state.
use newport_protocol::desktop::preferences::*;
use serde_json::{Value, json};
pub(super) fn project(value: Project) -> Value {
    json!({"id": value.id, "serverId": value.server, "name": value.name, "path": json!({"bytesB64":value.path.bytes_b64,"display":value.path.display})})
}
pub(super) fn codec(value: Codec) -> &'static str {
    match value {
        Codec::Auto => "auto",
        Codec::Lossless => "lossless",
        Codec::Fast => "fast",
    }
}
pub(super) fn settings(value: Settings) -> Value {
    json!({"audio": value.audio, "codec": codec(value.codec), "fpsGoal": value.fps_goal, "hud": value.hud, "separateApps": value.separate_apps})
}
pub(super) fn project_reply(reply: Reply) -> Value {
    match reply {
        Reply::SetWindow(_) | Reply::ProjectsRemove(_) => Value::Null,
        Reply::ProjectsList(value) => {
            Value::Array(value.projects.into_iter().map(project).collect())
        }
        Reply::ProjectsSave(value) => project(*value),
        Reply::Settings(value) | Reply::SettingsSave(value) => settings(*value),
        Reply::Installed(value) | Reply::InstalledSave(value) => json!(value.servers),
    }
}
