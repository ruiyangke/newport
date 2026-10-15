//! Visible repository observation. Changes outside Newport arrive without a
//! manual refresh; unchanged snapshots keep the user's loaded pages and diff.
use super::*;

impl Newport {
    pub(super) fn poll_git_status(&mut self) -> bool {
        self.poll_git_status_at(Instant::now())
    }

    pub(super) fn poll_git_status_at(&mut self, now: Instant) -> bool {
        let visible = self.page == Page::Projects
            && self.window_visible
            && self.configuration_available()
            && self.git.repo["repoId"].is_string()
            && self.git.repo["bare"] != true
            && !self.git_write_pending()
            && self.git.switching_path.is_none()
            && !self.project_dialog_open
            && self.git.sync_form.is_none();
        if !self
            .resources
            .take_poll("git-sync-status", now, Duration::from_secs(15), visible)
        {
            return false;
        }
        let presentation = |view: &Self| {
            view.resources.get("git-sync-status").map(|row| {
                (
                    row.display_pending(),
                    row.error.clone(),
                    row.presentation_revision,
                )
            })
        };
        let previous = presentation(self);
        let generation = self.resources.begin_refresh("git-sync-status", false);
        self.dispatch_request(
            "git-sync-status",
            "git_request",
            json!({"serverId":self.server,"request":{
                "method":"repo.status", "params":{"repoId":self.git_repository_id(),"pageSize":1}
            }}),
            generation,
        );
        previous != presentation(self)
    }

    pub(super) fn git_watch_completed(&mut self) -> bool {
        if self.page != Page::Projects || self.git_write_pending() {
            return false;
        }
        let status = self.value("git-sync-status");
        let head_changed = status["metadata"]["head"].is_object()
            && self.git.repo["head"] != status["metadata"]["head"];
        if head_changed {
            self.git.repo["head"] = status["metadata"]["head"].clone();
        }
        let changed = self.git.observe_status_snapshot(&status);
        let failed_listing = self
            .resources
            .get("git-list")
            .is_some_and(|row| row.error.is_some());
        // A successful status poll proves reads can run again, even when the
        // repository is unchanged since an authentication/transport failure.
        if !(changed || failed_listing)
            || self.git.tab != 0
            || self.git.changes.searching()
            || self.git.diff_recovery.pending()
        {
            return head_changed;
        }
        // Coalesce with an existing first-page read. Abandon continuation reads
        // before a changed snapshot can replace the page they belonged to.
        if self.resources.get("git-list").is_some_and(|r| r.pending) {
            return head_changed;
        }
        self.resources.invalidate("git-more");
        self.git.continuation = None;
        let generation = self.resources.begin_refresh("git-list", false);
        self.dispatch_request(
            "git-list",
            "git_request",
            json!({"serverId":self.server,"request":{
                "method":"repo.status", "params":self.git_params()
            }}),
            generation,
        );
        true
    }
}
