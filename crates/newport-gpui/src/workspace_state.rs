//! Native navigation preferences stay in the opened data profile.
use super::Page;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Selection {
    pub server: String,
    pub page: Page,
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
fn valid_path(value: &serde_json::Value) -> bool {
    use base64::Engine as _;
    value["display"].is_string()
        && value["bytesB64"]
            .as_str()
            .filter(|s| s.len() <= 32768)
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
            .is_some_and(|bytes| bytes.starts_with(b"/") && !bytes.contains(&0))
}
/// A restored selection with anything invalid dropped; an impossible server
/// identity refuses the whole selection.
pub(super) fn sanitize(mut selection: Selection) -> io::Result<Selection> {
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

    /// A deleted server's remembered views go with it.
    pub(super) fn forget_server_state(&mut self, server: &String) {
        self.git_destinations.remove(server);
        self.overview_state.preferences.remove(server);
        self.overview_state.interfaces.remove(server);
        self.project_preferences.remove(server);
        self.save_workspace_selection();
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
        self.ui_state.workspace(Selection {
            server: self.server.clone(),
            page: self.page,
            workspace_page: self.workspace_page,
            git: self.git_destination(),
            network_interfaces: self.overview_state.interfaces.clone(),
            overview_preferences: self.overview_state.preferences.clone(),
            git_destinations: self.git_destinations.clone(),
            project_preferences: self.project_preferences.clone(),
        });
    }
}
#[cfg(test)]
#[path = "connection_recovery_tests.rs"]
mod connection_recovery_tests;

#[cfg(test)]
mod tests {
    use super::{Selection, sanitize};
    use crate::Page;
    fn decode(bytes: &[u8]) -> std::io::Result<Selection> {
        sanitize(serde_json::from_slice(bytes).map_err(std::io::Error::other)?)
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
            "a":{"metric":3,"minutes":60,"virtual_open":true,"sort":1,"ascending":true,"process_filter":"node"},
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
        assert_eq!(restored.overview_preferences["bad"].metric, 0);
        assert_eq!(restored.overview_preferences["bad"].minutes, 5);
        assert_eq!(restored.overview_preferences["bad"].sort, 0);
        assert!(!restored.overview_preferences["bad"].ascending);
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
