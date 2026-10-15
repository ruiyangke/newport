//! Shared worktree picker labels and action-review visibility.
use super::*;

pub(super) fn row_labels(entry: &Value) -> (String, String) {
    let title = entry["name"]["display"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(if entry["kind"] == "bare" {
            "Bare repository"
        } else {
            "Main worktree"
        })
        .to_owned();
    let branch = entry["head"]["name"]["display"]
        .as_str()
        .filter(|_| entry["head"]["detached"] != true)
        .filter(|s| !s.is_empty())
        .map(|name| name.strip_prefix("refs/heads/").unwrap_or(name))
        .unwrap_or(if entry["head"]["detached"] == true {
            "Detached HEAD"
        } else {
            "Branch unavailable"
        });
    let mut labels = vec![branch.to_owned()];
    if entry["current"] == true {
        labels.push("Current".into());
    }
    if entry["locked"] == true {
        labels.push("Locked".into());
    }
    match entry["state"].as_str() {
        Some("missing") => labels.push("Missing".into()),
        Some("available") => {}
        _ => labels.push("Details unavailable".into()),
    }
    (title, labels.join(" · "))
}
impl Newport {
    pub(super) fn git_worktree_review_open(&self) -> bool {
        self.git.worktree_create.is_some()
            || self.git.worktree_lock.is_some()
            || self.git.worktree_repair.is_some()
            || self.git.worktree_remove.is_some()
    }
}
#[cfg(test)]
mod tests {
    use super::row_labels;
    use serde_json::json;
    #[test]
    fn rows_distinguish_main_detached_locked_and_missing_checkouts() {
        assert_eq!(
            row_labels(
                &json!({"kind":"main","state":"available","current":true,"head":{"name":{"display":"refs/heads/main"}}})
            ),
            ("Main worktree".into(), "main · Current".into())
        );
        assert_eq!(
            row_labels(
                &json!({"kind":"linked","name":{"display":"feature"},"state":"missing","locked":true,"head":{"detached":true,"name":{"display":"HEAD"}}})
            ),
            ("feature".into(), "Detached HEAD · Locked · Missing".into())
        );
        assert_eq!(row_labels(&json!({"kind":"bare"})).0, "Bare repository");
        assert!(row_labels(&json!({})).1.contains("Branch unavailable"));
    }
}
