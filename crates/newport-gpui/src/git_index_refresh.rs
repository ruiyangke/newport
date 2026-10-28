//! Staging changes the index snapshot, not the workspace the user is viewing.
//! Assemble a replacement for the loaded status window before displaying it.
use super::*;

pub(super) struct Refresh {
    page: Value,
    count: usize,
    params: Value,
}
impl Refresh {
    fn new(page: &Value, params: Value) -> Self {
        Self {
            page: Value::Null,
            count: page["entries"].as_array().map_or(100, Vec::len).max(100),
            params,
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
    fn request(&self) -> Value {
        let mut params = self.params.clone();
        params["pageSize"] = json!(200);
        if let Some(cursor) = self.page["nextCursor"].as_str() {
            params["cursor"] = json!(cursor);
        }
        params
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
        self.git.index_refresh = Some(Refresh::new(&self.git.page, self.git_params()));
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
            "git_refresh_repository",
            json!({"serverId":self.server,"repoId":self.git.repo["repoId"]}),
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
        let params = self.git.index_refresh.as_ref().unwrap().request();
        self.request_refresh(
            "git-index-status",
            "git_request",
            json!({"serverId":self.server,"request":{"method":"repo.status","params":params}}),
        );
        self.git_index_refresh_error("git-index-status");
    }
}

#[cfg(test)]
mod tests {
    use super::Refresh;
    use serde_json::{Value, json};

    fn page(snapshot: &str, count: usize, cursor: Option<&str>) -> Value {
        json!({"snapshot":snapshot,"entries":vec![json!({"entryId":"file"});count],"nextCursor":cursor,"metadata":{}})
    }
    #[test]
    fn index_refresh_preserves_the_loaded_window_until_replacement_is_complete() {
        let mut refresh = Refresh::new(
            &page("old", 350, Some("old-cursor")),
            json!({"repoId":"repo","filter":{"group":"all"}}),
        );
        assert!(refresh.request()["cursor"].is_null());
        assert!(!refresh.accept(page("new", 200, Some("next"))).unwrap());
        assert_eq!(refresh.request()["cursor"], "next");
        assert_eq!(refresh.request()["filter"]["group"], "all");
        assert!(refresh.accept(page("new", 160, None)).unwrap());
        assert_eq!(refresh.page["entries"].as_array().unwrap().len(), 360);
    }
    #[test]
    fn shrinking_lists_finish_and_mixed_snapshots_are_rejected() {
        let mut refresh = Refresh::new(&page("old", 350, None), json!({}));
        assert!(refresh.accept(page("new", 2, None)).unwrap());
        let mut refresh = Refresh::new(&page("old", 350, None), json!({}));
        assert!(!refresh.accept(page("new", 200, Some("next"))).unwrap());
        assert!(refresh.accept(page("changed-again", 150, None)).is_err());
        assert_eq!(refresh.page["entries"].as_array().unwrap().len(), 200);
    }
}
