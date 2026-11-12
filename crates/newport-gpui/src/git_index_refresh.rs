//! Staging changes the index snapshot, not the workspace the user is viewing.
//! Assemble a replacement for the loaded status window before displaying it.
use super::*;
use newport_protocol::{
    desktop,
    git::{Request as GitRequest, StatusFilter},
};

pub(super) struct Refresh {
    page: Value,
    count: usize,
    repo_id: String,
    filter: StatusFilter,
}
impl Refresh {
    fn new(page: &Value, repo_id: &str, filter: StatusFilter) -> Self {
        Self {
            page: Value::Null,
            count: page["entries"].as_array().map_or(100, Vec::len).max(100),
            repo_id: repo_id.into(),
            filter,
        }
    }
    fn accept(&mut self, incoming: Value) -> Result<bool, String> {
        self.page = if let Some(cursor) = self.page["nextCursor"].as_str() {
            newport_client::git_page::merge(&self.page, incoming, cursor)?
        } else {
            newport_client::git_page::validate(&incoming)?;
            incoming
        };
        Ok(
            self.page["entries"].as_array().map_or(0, Vec::len) >= self.count
                || self.page["nextCursor"].is_null(),
        )
    }
    fn request(&self) -> GitRequest {
        GitRequest::Status {
            repo_id: self.repo_id.clone(),
            filter: Some(self.filter.clone()),
            page_size: 200,
            cursor: self.page["nextCursor"].as_str().map(str::to_owned),
        }
    }
}

impl Newport {
    pub(super) fn git_cancel_index_refresh(&mut self) {
        self.git.index_refresh = None;
        for key in ["git-index-refresh", "git-index-status"] {
            self.resources.invalidate(key);
        }
    }
    pub(super) fn git_refresh_index(&mut self) {
        self.git_cancel_index_refresh();
        let Ok(server) = uuid::Uuid::parse_str(&self.server) else {
            return;
        };
        self.git.index_refresh = Some(Refresh::new(
            &self.git.page,
            &self.git_repository_id(),
            self.git.changes.filter(),
        ));
        // Retire in-flight results and cursors without clearing presentation.
        for key in [
            "git-list",
            "git-more",
            "git-diff",
            "git-diff-more",
            "git-sync-status",
        ] {
            self.resources.invalidate(key);
        }
        self.git.continuation = None;
        self.git.diff_recovery.reset();
        self.request_refresh(
            "git-index-refresh",
            desktop::Operation::Git(Box::new(desktop::git::Operation::RefreshRepository(
                Box::new(desktop::git::RefreshRepositoryArgs {
                    server,
                    repo_id: self.git_repository_id(),
                }),
            ))),
        );
        self.git_index_refresh_error("git-index-refresh");
    }
    fn git_index_refresh_error(&mut self, key: &str) -> bool {
        if let Some(error) = self.resources.get(key).and_then(|r| r.error.clone()) {
            self.git_cancel_index_refresh();
            // Preserve the old display, but it cannot authorize another write.
            self.git.error = Some(error);
            true
        } else {
            false
        }
    }
    pub(super) fn git_index_refresh_completed(&mut self, key: &str) {
        if self.git.index_refresh.is_none() || self.git.tab != 0 {
            return;
        }
        if self.git_index_refresh_error(key) {
            return;
        }
        if key == "git-index-refresh" {
            if self.value(key)["repoId"] != self.git.repo["repoId"] {
                self.git_cancel_index_refresh();
                self.git.error = Some("Repository changed. Refresh before staging.".into());
                return;
            }
        } else {
            let incoming = self.value(key);
            match self.git.index_refresh.as_mut().unwrap().accept(incoming) {
                Ok(true) => {
                    let refresh = self.git.index_refresh.take().unwrap();
                    if let Some(generation) = self.resources.begin_refresh("git-list", false) {
                        self.resources
                            .complete("git-list", generation, Ok(refresh.page));
                        self.git_listing_completed(true);
                    }
                    return;
                }
                Ok(false) => {}
                Err(error) => {
                    self.git_cancel_index_refresh();
                    self.git.error = Some(error);
                    return;
                }
            }
        }
        let request = self.git.index_refresh.as_ref().unwrap().request();
        self.request_git_refresh("git-index-status", request);
        self.git_index_refresh_error("git-index-status");
    }
}

#[cfg(test)]
mod tests {
    use super::{Refresh, StatusFilter};
    fn params(refresh: &Refresh) -> Value {
        serde_json::to_value(refresh.request()).unwrap()["params"].clone()
    }
    use serde_json::{Value, json};

    fn page(snapshot: &str, count: usize, cursor: Option<&str>) -> Value {
        json!({"snapshot":snapshot,"entries":vec![json!({"entryId":"file"});count],"nextCursor":cursor,"metadata":{}})
    }
    #[test]
    fn index_refresh_preserves_the_loaded_window_until_replacement_is_complete() {
        let mut refresh = Refresh::new(
            &page("old", 350, Some("old-cursor")),
            "repo",
            StatusFilter {
                text: String::new(),
                group: Some("all".into()),
            },
        );
        assert!(params(&refresh)["cursor"].is_null());
        assert!(!refresh.accept(page("new", 200, Some("next"))).unwrap());
        assert_eq!(params(&refresh)["cursor"], "next");
        assert_eq!(params(&refresh)["filter"]["group"], "all");
        assert!(refresh.accept(page("new", 160, None)).unwrap());
        assert_eq!(refresh.page["entries"].as_array().unwrap().len(), 360);
    }
    #[test]
    fn shrinking_lists_finish_and_mixed_snapshots_are_rejected() {
        let mut refresh = Refresh::new(&page("old", 350, None), "repo", StatusFilter::default());
        assert!(refresh.accept(page("new", 2, None)).unwrap());
        let mut refresh = Refresh::new(&page("old", 350, None), "repo", StatusFilter::default());
        assert!(!refresh.accept(page("new", 200, Some("next"))).unwrap());
        assert!(refresh.accept(page("changed-again", 150, None)).is_err());
        assert_eq!(refresh.page["entries"].as_array().unwrap().len(), 200);
    }
}
