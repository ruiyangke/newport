//! Native navigation preferences stay in the explicitly selected data profile.
use super::Page;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
pub(super) struct Selection {
    pub server: String,
    #[serde(deserialize_with = "known_page")]
    pub page: Page,
    #[serde(deserialize_with = "known_page")]
    pub workspace_page: Page,
    #[serde(default)]
    pub git: Option<GitDestination>,
    #[serde(default)]
    pub overview_preferences: std::collections::BTreeMap<String, super::overview::Preferences>,
    #[serde(default)]
    pub network_interfaces: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub git_destinations: std::collections::BTreeMap<String, GitDestination>,
    #[serde(default)]
    pub project_preferences:
        std::collections::BTreeMap<String, super::project_preferences::Preferences>,
}
impl Default for Selection {
    fn default() -> Self {
        Self {
            server: String::new(),
            page: Page::Overview,
            workspace_page: Page::Overview,
            git: None,
            network_interfaces: Default::default(),
            overview_preferences: Default::default(),
            git_destinations: Default::default(),
            project_preferences: Default::default(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct GitDestination {
    server: String,
    project_path: serde_json::Value,
    root: serde_json::Value,
    tab: usize,
}
/// Another Newport build may have saved a page this one doesn't have. Open
/// Overview instead of discarding the whole workspace with its server.
fn known_page<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Page, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or(Page::Overview))
}
fn valid_path(value: &serde_json::Value) -> bool {
    use base64::Engine as _;
    value["display"].is_string()
        && value["bytesB64"]
            .as_str()
            .filter(|s| s.len() <= 32768)
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
            .is_some_and(|bytes| bytes.starts_with(b"/") && !bytes.contains(&0))
}
fn path() -> Option<PathBuf> {
    std::env::var_os("NEWPORT_DATA_DIR")
        .filter(|s| !s.is_empty())
        .map(|base| PathBuf::from(base).join("gpui-workspace-state.json"))
}
fn decode(bytes: &[u8]) -> io::Result<Selection> {
    let mut selection: Selection =
        serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if selection.server.len() > 256 || selection.server.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid server identity",
        ));
    }
    if selection.workspace_page == Page::Settings {
        selection.workspace_page = Page::Overview;
    }
    if selection.git.as_ref().is_some_and(|git| {
        git.server != selection.server
            || git.tab >= 7
            || !valid_path(&git.project_path)
            || !valid_path(&git.root)
    }) {
        selection.git = None;
    }
    selection.git_destinations.retain(|server, git| {
        server == &git.server
            && server.len() <= 256
            && !server.contains('\0')
            && git.tab < 7
            && valid_path(&git.project_path)
            && valid_path(&git.root)
    });
    if let Some(git) = &selection.git {
        selection
            .git_destinations
            .insert(selection.server.clone(), git.clone());
    }
    selection.overview_preferences.retain(|server, pref| {
        pref.normalize();
        !server.is_empty() && server.len() <= 256 && !server.contains('\0')
    });
    Ok(selection)
}
pub(super) fn load() -> Selection {
    let Some(path) = path() else {
        return Selection::default();
    };
    match fs::read(path).and_then(|bytes| decode(&bytes)) {
        Ok(value) => value,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!("Could not restore native workspace: {error}");
            }
            Selection::default()
        }
    }
}
fn write(path: &Path, selection: &Selection) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(selection)?;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
impl super::Newport {
    pub(super) fn restore_git_destination(&mut self) {
        if self.page != Page::Projects || self.selection.is_some() {
            return;
        }
        if !self
            .resources
            .get("page")
            .is_some_and(|r| !r.pending && r.error.is_none())
        {
            return;
        }
        let Some(saved) = self.restore_git.take() else {
            return;
        };
        if saved.server != self.server {
            return;
        }
        let Some(project) = super::rows(&self.value("page"))
            .into_iter()
            .find(|project| {
                project["serverId"].as_str() == Some(&self.server)
                    && project["path"]["bytesB64"] == saved.project_path["bytesB64"]
            })
        else {
            return;
        };
        self.selection = Some(project);
        self.git = super::git::Workspace::default();
        self.git.tab = if saved.tab == 2 { 0 } else { saved.tab };
        self.git_request_open(saved.root);
    }
    pub(super) fn git_destination(&self) -> Option<GitDestination> {
        if self.page != Page::Projects {
            return None;
        }
        if let Some(saved) = &self.restore_git {
            return Some(saved.clone());
        }
        let project = self.selection.as_ref()?;
        let root = if self.git.repo["repoId"].is_string() {
            &self.git.repo["root"]
        } else {
            &self.git.opening_path
        };
        (valid_path(&project["path"]) && valid_path(root)).then(|| GitDestination {
            server: self.server.clone(),
            project_path: project["path"].clone(),
            root: root.clone(),
            tab: self.git.tab,
        })
    }

    pub(super) fn save_workspace_selection(&mut self) {
        if !self.configuration_available() {
            return;
        }
        if self.page == Page::Projects {
            if let Some(destination) = self.git_destination() {
                self.git_destinations
                    .insert(self.server.clone(), destination);
            } else {
                self.git_destinations.remove(&self.server);
            }
        }
        if let Some(path) = path()
            && let Err(error) = write(
                &path,
                &Selection {
                    server: self.server.clone(),
                    page: self.page,
                    workspace_page: self.workspace_page,
                    git: self.git_destination(),
                    network_interfaces: self.overview_state.interfaces.clone(),
                    overview_preferences: self.overview_state.preferences.clone(),
                    git_destinations: self.git_destinations.clone(),
                    project_preferences: self.project_preferences.clone(),
                },
            )
        {
            eprintln!("Could not save native workspace: {error}");
        }
    }
}
#[cfg(test)]
#[path = "connection_recovery_tests.rs"]
mod connection_recovery_tests;

#[cfg(test)]
mod tests {
    use super::{Selection, decode};
    use crate::Page;
    #[test]
    fn a_page_from_another_build_opens_overview_and_keeps_the_server() {
        let input = serde_json::json!({
            "server": "fixture", "page": "remote_apps", "workspace_page": "remote_apps"
        });
        let restored = decode(&serde_json::to_vec(&input).unwrap()).unwrap();
        assert!(restored.page == Page::Overview);
        assert!(restored.workspace_page == Page::Overview);
        assert_eq!(restored.server, "fixture");
    }
    #[test]
    fn container_session_choices_never_enter_persistent_workspace_state() {
        let input = serde_json::json!({
            "server": "fixture", "page": "containers", "workspace_page": "containers",
            "containers_preferences": {"fixture": {
                "query": "private-project", "log_query": "private-search",
                "selected": "p:private-project", "live": false
            }}
        });
        let restored = decode(&serde_json::to_vec(&input).unwrap()).unwrap();
        assert!(restored.page == Page::Containers);
        let saved = serde_json::to_value(restored).unwrap();
        assert!(saved.get("containers_preferences").is_none());
        assert!(!saved.to_string().contains("private-"));
    }
    #[test]
    fn overview_preferences_roundtrip_per_server_and_normalize_invalid_choices() {
        let input = serde_json::json!({"server":"a","page":"overview","workspace_page":"overview","overview_preferences":{
            "a":{"metric":3,"auto_refresh":false,"minutes":60,"virtual_open":true,"sort":1,"ascending":true,"all":true,"process_filter":"node"},
            "b":{"metric":1,"minutes":15,"process_filter":"postgres"},
            "bad":{"metric":9,"minutes":42,"sort":99,"ascending":true},
            "":{"metric":2}
        }});
        let selection = decode(&serde_json::to_vec(&input).unwrap()).unwrap();
        assert_eq!(selection.overview_preferences.len(), 3);
        let restored = decode(&serde_json::to_vec(&selection).unwrap()).unwrap();
        assert_eq!(
            restored.overview_preferences,
            selection.overview_preferences
        );
        assert_eq!(restored.overview_preferences["a"].process_filter, "node");
        assert_eq!(restored.overview_preferences["b"].minutes, 15);
        // Legacy paused preferences are ignored: Overview always refreshes.
        assert!(
            serde_json::to_value(&restored.overview_preferences).unwrap()["a"]
                .get("auto_refresh")
                .is_none()
        );
        assert_eq!(restored.overview_preferences["bad"].metric, 0);
        assert_eq!(restored.overview_preferences["bad"].minutes, 5);
        assert_eq!(restored.overview_preferences["bad"].sort, 0);
        assert!(!restored.overview_preferences["bad"].ascending);
        let legacy =
            decode(br#"{"server":"a","page":"overview","workspace_page":"overview"}"#).unwrap();
        assert!(legacy.overview_preferences.is_empty());
    }
    #[test]
    fn per_server_destinations_survive_other_pages_and_validate_identity() {
        let destination = serde_json::json!({"server":"fixture","project_path":{"display":"/repo","bytesB64":"L3JlcG8="},"root":{"display":"/linked","bytesB64":"L2xpbmtlZA=="},"tab":1});
        let mut other = destination.clone();
        other["server"] = serde_json::json!("other");
        let mut invalid = destination.clone();
        invalid["root"]["bytesB64"] = serde_json::json!("invalid");
        let input = serde_json::json!({"server":"fixture","page":"files","workspace_page":"files","git_destinations":{"fixture":destination,"other":other,"mismatch":invalid}});
        let restored = decode(&serde_json::to_vec(&input).unwrap()).unwrap();
        assert!(restored.git.is_none());
        assert_eq!(restored.git_destinations.len(), 2);
        let again = decode(&serde_json::to_vec(&restored).unwrap()).unwrap();
        assert_eq!(again.git_destinations["fixture"].tab, 1);
        assert_eq!(again.git_destinations["other"].server, "other");
    }
    #[test]
    fn invalid_git_destination_does_not_discard_shell_selection() {
        let input = serde_json::json!({"server":"fixture","page":"projects","workspace_page":"projects","git":{"server":"other","project_path":{},"root":{},"tab":1}});
        let restored = decode(&serde_json::to_vec(&input).unwrap()).unwrap();
        assert_eq!(restored.server, "fixture");
        assert!(restored.git.is_none());
        let valid = serde_json::json!({"server":"fixture","page":"projects","workspace_page":"projects","git":{"server":"fixture","project_path":{"display":"/repo","bytesB64":"L3JlcG8="},"root":{"display":"/linked","bytesB64":"L2xpbmtlZA=="},"tab":1}});
        let restored = decode(&serde_json::to_vec(&valid).unwrap()).unwrap();
        assert_eq!(restored.git_destinations["fixture"].tab, 1);
        assert_eq!(restored.git.unwrap().tab, 1);
    }
    #[test]
    fn selection_roundtrips_and_rejects_unknown_pages() {
        let value = Selection {
            server: "fixture-id".into(),
            page: Page::Projects,
            workspace_page: Page::Projects,
            git: None,
            network_interfaces: Default::default(),
            overview_preferences: Default::default(),
            git_destinations: Default::default(),
            project_preferences: Default::default(),
        };
        let decoded = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(decoded.server, "fixture-id");
        assert!(decoded.page == Page::Projects);
        assert!(decode(br#"{"server":"x","page":"removed","workspace_page":"overview"}"#).is_err());
        let settings =
            decode(br#"{"server":"x","page":"settings","workspace_page":"settings"}"#).unwrap();
        assert!(settings.workspace_page == Page::Overview);
    }
}
