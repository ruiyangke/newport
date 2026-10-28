//! Local bookmarks only; credentials and server connection profiles remain in the vault.
use super::Preferences;
use crate::git::protocol::Path;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const KEY: &str = "gitProjects";
const MAX_PROJECTS: usize = 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Project {
    id: Uuid,
    server_id: Uuid,
    name: String,
    path: Path,
}

fn validate(project: &Project) -> Result<(), String> {
    if project.name.trim().is_empty()
        || project.name.len() > 256
        || project.name.chars().any(char::is_control)
    {
        return Err("Project name must contain 1–256 bytes without control characters.".into());
    }
    let bytes = project.path.decode().map_err(|e| e.to_string())?;
    if !bytes.starts_with(b"/") || project.path.display.len() > 16384 {
        return Err("Save the absolute repository path returned by Git.".into());
    }
    Ok(())
}

fn decode(value: Option<serde_json::Value>) -> Result<Vec<Project>, String> {
    let rows: Vec<Project> = match value {
        None => Vec::new(),
        Some(value) => serde_json::from_value(value)
            .map_err(|e| format!("Saved projects could not be read: {e}"))?,
    };
    if rows.len() > MAX_PROJECTS {
        return Err("Saved project limit exceeded.".into());
    }
    let mut ids = std::collections::HashSet::new();
    let mut paths = std::collections::HashSet::new();
    for row in &rows {
        validate(row)?;
        if !ids.insert(row.id) || !paths.insert((row.server_id, row.path.bytes_b64.clone())) {
            return Err("Saved projects contain duplicate entries.".into());
        }
    }
    Ok(rows)
}

fn upsert(rows: &mut Vec<Project>, project: Project) -> Result<(), String> {
    validate(&project)?;
    if rows.iter().any(|row| {
        row.id != project.id
            && row.server_id == project.server_id
            && row.path.bytes_b64 == project.path.bytes_b64
    }) {
        return Err("This repository is already in the project list.".into());
    }
    if let Some(row) = rows.iter_mut().find(|row| row.id == project.id) {
        if row.server_id != project.server_id || row.path.bytes_b64 != project.path.bytes_b64 {
            return Err("An existing project's server and repository cannot be changed.".into());
        }
        *row = project;
    } else {
        if rows.len() >= MAX_PROJECTS {
            return Err("Remove an unused project before adding another.".into());
        }
        rows.push(project);
    }
    Ok(())
}

pub async fn git_projects_list(
    state: &Preferences,
    server_id: Uuid,
) -> Result<Vec<Project>, String> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        Ok(decode(state.get(KEY)?)?
            .into_iter()
            .filter(|row| row.server_id == server_id)
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}
pub async fn git_projects_save(state: &Preferences, project: Project) -> Result<Project, String> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        state.update(|values| {
            let mut rows = decode(values.get(KEY).cloned())?;
            upsert(&mut rows, project.clone())?;
            values.insert(
                KEY.into(),
                serde_json::to_value(&rows).map_err(|e| e.to_string())?,
            );
            Ok(project)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}
pub async fn git_projects_remove(
    state: &Preferences,
    server_id: Uuid,
    project_id: Uuid,
) -> Result<(), String> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        state.update(|values| {
            let mut rows = decode(values.get(KEY).cloned())?;
            rows.retain(|row| row.server_id != server_id || row.id != project_id);
            values.insert(
                KEY.into(),
                serde_json::to_value(&rows).map_err(|e| e.to_string())?,
            );
            Ok(())
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Drops a deleted server's projects.
pub(super) fn forget_server(
    values: &mut serde_json::Map<String, serde_json::Value>,
    server_id: Uuid,
) -> Result<(), String> {
    let mut rows = decode(values.get(KEY).cloned())?;
    rows.retain(|row| row.server_id != server_id);
    values.insert(
        KEY.into(),
        serde_json::to_value(&rows).map_err(|e| e.to_string())?,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn project(server_id: Uuid) -> Project {
        Project {
            id: Uuid::new_v4(),
            server_id,
            name: "Example".into(),
            path: Path::new(b"/home/user/project\xff"),
        }
    }
    #[test]
    fn bookmarks_preserve_paths_and_scope_duplicates_to_server() {
        let original = project(Uuid::new_v4());
        let mut rows = vec![];
        upsert(&mut rows, original.clone()).unwrap();
        let mut duplicate = original.clone();
        duplicate.id = Uuid::new_v4();
        assert!(upsert(&mut rows, duplicate.clone()).is_err());
        duplicate.server_id = Uuid::new_v4();
        upsert(&mut rows, duplicate).unwrap();
        let decoded = decode(Some(serde_json::to_value(&rows).unwrap())).unwrap();
        assert_eq!(decoded[0].path.decode().unwrap(), b"/home/user/project\xff");
        let mut renamed = original.clone();
        renamed.name = "New name".into();
        upsert(&mut rows, renamed.clone()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "New name");
        renamed.server_id = Uuid::new_v4();
        assert!(upsert(&mut rows, renamed).is_err());
    }
    #[test]
    fn corrupt_storage_and_invalid_input_are_not_silently_reset() {
        assert!(decode(Some(serde_json::json!({}))).is_err());
        let mut row = project(Uuid::new_v4());
        assert!(decode(Some(serde_json::json!([row, row]))).is_err());
        row.path = Path::new(b"relative/path");
        assert!(validate(&row).is_err());
        row.path = Path::new(b"/valid");
        row.name = "\n".into();
        assert!(validate(&row).is_err());
    }
}
