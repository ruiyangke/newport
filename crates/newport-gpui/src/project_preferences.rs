//! Per-server library preferences. Bookmarks and repository contents are separate.
use newport_protocol::desktop::preferences::Project;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub favorites: BTreeSet<String>,
    pub favorites_only: bool,
    pub sort_path: bool,
    pub sort_descending: bool,
}
impl Preferences {
    pub fn visible(&self, projects: Vec<Project>, query: &str) -> Vec<Project> {
        let query = query.trim().to_lowercase();
        let mut projects: Vec<_> = projects
            .into_iter()
            .filter(|p| {
                (!self.favorites_only || self.favorites.contains(&p.id.to_string()))
                    && format!("{} {}", p.name, p.path.display)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect();
        projects.sort_by_cached_key(|p| {
            (if self.sort_path {
                &p.path.display
            } else {
                &p.name
            })
            .to_lowercase()
        });
        if self.sort_descending {
            projects.reverse();
        }
        projects
    }
}
impl super::Newport {
    pub(super) fn project_preferences_changed(&mut self) {
        self.project_library_state().list = super::project_library_rows::LibraryList::default();
        self.project_checks = None;
        self.save_workspace_selection();
    }
}
#[cfg(test)]
mod tests {
    use super::Preferences;
    use crate::project_library_state::typed_fixture::{TYPED_SERVER, bookmark};
    #[test]
    fn preferences_round_trip_and_combine_favorites_query_and_sort() {
        let a = bookmark(1, TYPED_SERVER, "Zulu", "/a");
        let b = bookmark(2, TYPED_SERVER, "Alpha", "/z");
        let projects = vec![a.clone(), b.clone()];
        let mut prefs = Preferences::default();
        assert_eq!(prefs.visible(projects.clone(), "")[0], b);
        prefs.sort_path = true;
        assert_eq!(prefs.visible(projects.clone(), "")[0], a);
        prefs.sort_descending = true;
        assert_eq!(prefs.visible(projects.clone(), "")[0], b);
        prefs.favorites.insert(a.id.to_string());
        prefs.favorites_only = true;
        let restored: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert_eq!(restored.visible(projects.clone(), " zulu ").len(), 1);
        assert!(restored.visible(projects, "Alpha").is_empty());
    }
}
