//! Staging changes the index snapshot, not the workspace the user is viewing.
//! Assemble a replacement for the loaded status window before displaying it.
use super::*;
use git_listing::{Listing, StatusPage};
use newport_protocol::git::{Request as GitRequest, StatusFilter};

pub(super) struct Refresh {
    page: Option<StatusPage>,
    count: usize,
    repo_id: String,
    filter: StatusFilter,
}
impl Refresh {
    fn new(page: Option<&Listing>, repo_id: &str, filter: StatusFilter) -> Self {
        Self {
            page: None,
            count: page.map_or(100, Listing::len).max(100),
            repo_id: repo_id.into(),
            filter,
        }
    }
    fn accept(&mut self, incoming: StatusPage) -> Result<bool, String> {
        let page = match self
            .page
            .as_ref()
            .and_then(|page| Some((page, page.next_cursor.as_deref()?)))
        {
            Some((page, cursor)) => newport_client::git_page::merge_pages(page, incoming, cursor)?,
            None => {
                newport_client::git_page::validate_page(&incoming)?;
                incoming
            }
        };
        let done = page.entries.len() >= self.count || page.next_cursor.is_none();
        self.page = Some(page);
        Ok(done)
    }
    fn request(&self) -> GitRequest {
        GitRequest::Status {
            repo_id: self.repo_id.clone(),
            filter: Some(self.filter.clone()),
            page_size: 200,
            cursor: self.page.as_ref().and_then(|page| page.next_cursor.clone()),
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
            self.git.page.as_ref(),
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
        self.request_repository_refresh("git-index-refresh", server);
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
            if self.git.refreshed.get(key).map(String::as_str) != self.git.repo_id() {
                self.git_cancel_index_refresh();
                self.git.error = Some("Repository changed. Refresh before staging.".into());
                return;
            }
        } else {
            let Some(incoming) = self.git_status(key).cloned() else {
                self.git_cancel_index_refresh();
                self.git.error = Some("Invalid repository page. Refresh to retry.".into());
                return;
            };
            match self.git.index_refresh.as_mut().unwrap().accept(incoming) {
                Ok(true) => {
                    let refresh = self.git.index_refresh.take().unwrap();
                    if let Some(generation) = self.resources.begin_refresh("git-list", false)
                        && let Some(page) = refresh.page
                    {
                        self.resources.complete_git(
                            "git-list",
                            generation,
                            Ok(newport_protocol::git::Reply::Status(Box::new(page))),
                        );
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
        let generation = self.resources.begin_refresh("git-index-status", false);
        self.dispatch_git_typed("git-index-status", request, generation);
        self.git_index_refresh_error("git-index-status");
    }
}

#[cfg(test)]
mod tests {
    use super::{Listing, Refresh, StatusFilter, StatusPage};
    use crate::git_fixture::{status_entry, status_page};
    fn params(refresh: &Refresh) -> serde_json::Value {
        serde_json::to_value(refresh.request()).unwrap()["params"].clone()
    }

    fn page(snapshot: &str, count: usize, cursor: Option<&str>) -> StatusPage {
        status_page(snapshot, vec![status_entry("file", "file"); count], cursor)
    }
    #[test]
    fn index_refresh_preserves_the_loaded_window_until_replacement_is_complete() {
        let mut refresh = Refresh::new(
            Some(&Listing::Status(Box::new(page(
                "old",
                350,
                Some("old-cursor"),
            )))),
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
        assert_eq!(refresh.page.as_ref().unwrap().entries.len(), 360);
    }
    #[test]
    fn shrinking_lists_finish_and_mixed_snapshots_are_rejected() {
        let old = Listing::Status(Box::new(page("old", 350, None)));
        let mut refresh = Refresh::new(Some(&old), "repo", StatusFilter::default());
        assert!(refresh.accept(page("new", 2, None)).unwrap());
        let mut refresh = Refresh::new(Some(&old), "repo", StatusFilter::default());
        assert!(!refresh.accept(page("new", 200, Some("next"))).unwrap());
        assert!(refresh.accept(page("changed-again", 150, None)).is_err());
        assert_eq!(refresh.page.as_ref().unwrap().entries.len(), 200);
    }
}
