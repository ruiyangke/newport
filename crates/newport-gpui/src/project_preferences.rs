//! Per-server library preferences. Bookmarks and repository contents are separate.
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    pub fn visible(&self, projects: Vec<Value>, query: &str) -> Vec<Value> {
        let query = query.trim().to_lowercase();
        let mut projects: Vec<_> = projects
            .into_iter()
            .filter(|p| {
                (!self.favorites_only
                    || p["id"]
                        .as_str()
                        .is_some_and(|id| self.favorites.contains(id)))
                    && format!(
                        "{} {}",
                        super::text(p, "name"),
                        super::text(&p["path"], "display")
                    )
                    .to_lowercase()
                    .contains(&query)
            })
            .collect();
        projects.sort_by_cached_key(|p| {
            (if self.sort_path {
                super::text(&p["path"], "display")
            } else {
                super::text(p, "name")
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
    use serde_json::json;
    #[test]
    fn preferences_round_trip_and_combine_favorites_query_and_sort() {
        let projects = vec![
            json!({"id":"a","name":"Zulu","path":{"display":"/a"}}),
            json!({"id":"b","name":"Alpha","path":{"display":"/z"}}),
        ];
        let mut prefs = Preferences::default();
        assert_eq!(prefs.visible(projects.clone(), "")[0]["id"], "b");
        prefs.sort_path = true;
        assert_eq!(prefs.visible(projects.clone(), "")[0]["id"], "a");
        prefs.sort_descending = true;
        assert_eq!(prefs.visible(projects.clone(), "")[0]["id"], "b");
        prefs.favorites.insert("a".into());
        prefs.favorites_only = true;
        let restored: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert_eq!(restored.visible(projects.clone(), " zulu ").len(), 1);
        assert!(restored.visible(projects, "Alpha").is_empty());
    }
}
