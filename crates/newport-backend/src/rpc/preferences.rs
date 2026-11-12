//! Typed preference routes. The persisted preference schema remains unchanged.
use super::failure::operation;
use crate::preferences::{self, Preferences, projects, remote_apps};
use newport_protocol::{
    common::Present,
    desktop::{Failure, preferences as dto},
};
use serde_json::{Map, Value};

fn project(value: projects::Project) -> dto::Project {
    dto::Project {
        id: value.id,
        server: value.server_id,
        name: value.name,
        path: value.path,
    }
}
fn stored_project(value: &dto::Project) -> projects::Project {
    projects::Project {
        id: value.id,
        server_id: value.server,
        name: value.name.clone(),
        path: value.path.clone(),
    }
}
fn settings(value: remote_apps::Settings) -> dto::Settings {
    dto::Settings {
        audio: value.audio,
        codec: match value.codec {
            remote_apps::Codec::Auto => dto::Codec::Auto,
            remote_apps::Codec::Lossless => dto::Codec::Lossless,
            remote_apps::Codec::Fast => dto::Codec::Fast,
        },
        fps_goal: value.fps_goal,
        hud: value.hud,
        separate_apps: value.separate_apps,
    }
}
fn stored_settings(value: &dto::Settings) -> remote_apps::Settings {
    remote_apps::Settings {
        audio: value.audio,
        codec: match value.codec {
            dto::Codec::Auto => remote_apps::Codec::Auto,
            dto::Codec::Lossless => remote_apps::Codec::Lossless,
            dto::Codec::Fast => remote_apps::Codec::Fast,
        },
        fps_goal: value.fps_goal,
        hud: value.hud,
        separate_apps: value.separate_apps,
    }
}
fn window(value: &dto::WindowPatch) -> Value {
    let mut fields = Map::new();
    if let Some(width) = value.sidebar_width {
        fields.insert("sidebarWidth".into(), Value::from(width));
    }
    if let Some(compact) = value.sidebar_compact {
        fields.insert("sidebarCompact".into(), Value::Bool(compact));
    }
    if let Some(appearance) = value.appearance {
        fields.insert(
            "appearance".into(),
            Value::String(
                match appearance {
                    dto::Appearance::System => "system",
                    dto::Appearance::Light => "light",
                    dto::Appearance::Dark => "dark",
                }
                .into(),
            ),
        );
    }
    if let Some(meta) = value.terminal_option_as_meta {
        fields.insert("terminalOptionAsMeta".into(), Value::Bool(meta));
    }
    Value::Object(fields)
}

pub async fn dispatch(
    command: &dto::Operation,
    state: &Preferences,
) -> Result<dto::Reply, Failure> {
    use dto::{Operation as O, Reply as R};
    Ok(match command {
        O::SetWindow(args) => {
            preferences::set_window_preferences(state, window(&args.patch))
                .await
                .map_err(operation)?;
            R::SetWindow(Box::new(Present {}))
        }
        O::ProjectsList(args) => R::ProjectsList(Box::new(dto::Projects {
            projects: projects::git_projects_list(state, args.server)
                .await
                .map_err(operation)?
                .into_iter()
                .map(project)
                .collect(),
        })),
        O::ProjectsSave(args) => R::ProjectsSave(Box::new(project(
            projects::git_projects_save(state, stored_project(&args.project))
                .await
                .map_err(operation)?,
        ))),
        O::ProjectsRemove(args) => {
            projects::git_projects_remove(state, args.server, args.project)
                .await
                .map_err(operation)?;
            R::ProjectsRemove(Box::new(Present {}))
        }
        O::Settings(_) => R::Settings(Box::new(settings(
            remote_apps::remote_apps_settings(state)
                .await
                .map_err(operation)?,
        ))),
        O::SettingsSave(args) => R::SettingsSave(Box::new(settings(
            remote_apps::remote_apps_settings_save(state, stored_settings(&args.settings))
                .await
                .map_err(operation)?,
        ))),
        O::Installed(_) => R::Installed(Box::new(dto::Installed {
            servers: remote_apps::remote_apps_installed(state)
                .await
                .map_err(operation)?,
        })),
        O::InstalledSave(args) => R::InstalledSave(Box::new(dto::Installed {
            servers: remote_apps::remote_apps_installed_save(state, args.server, args.installed)
                .await
                .map_err(operation)?,
        })),
    })
}
