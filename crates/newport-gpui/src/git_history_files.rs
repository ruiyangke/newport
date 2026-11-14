//! Historical file list: immutable comparison identity, bounded pagination and
//! explicit recovery without hiding the previously loaded file rows.
use super::*;
use gpui_kit::base::Selectable;

use crate::git_page_demand::Demand;
use git_history::CommitFiles;
use newport_protocol::git::{Delta, DiffMetadata};

/// What identifies a file page for paging demand: its envelope and row
/// count, without the rows.
pub(super) type Identity = Option<(CommitFiles, usize)>;

pub(super) fn identity(page: Option<&CommitFiles>) -> Identity {
    page.map(|page| (git_listing::envelope(page), page.entries.len()))
}

fn partial(page: &CommitFiles) -> bool {
    page.next_cursor.is_some() || page.metadata.truncated == Some(true)
}

/// Compact files sit above the diff. Reserve the full cap for skeletons and
/// notices; completed short lists give their unused height back to the diff.
fn compact_height(page: Option<&CommitFiles>, unsettled: bool) -> f32 {
    let Some(page) = page else {
        return 130.;
    };
    if unsettled || partial(page) {
        return 130.;
    }
    if page.entries.is_empty() {
        return 80.;
    }
    (34. + 28. * page.entries.len().min(4) as f32 + 6.).min(130.)
}

fn count_label(page: &CommitFiles) -> String {
    let loaded = page.entries.len();
    let partial = partial(page);
    match page.metadata.total_files {
        Some(total) if partial => format!("{loaded} of {total} files loaded"),
        Some(1) => "1 changed file".into(),
        Some(total) => format!("{total} changed files"),
        None if partial => format!(
            "{loaded} {} loaded",
            if loaded == 1 { "file" } else { "files" }
        ),
        None if loaded == 1 => "1 changed file".into(),
        _ => format!("{loaded} changed files"),
    }
}
fn select_current(page: Option<&CommitFiles>, owner: &DiffMetadata, entry: &Delta) -> bool {
    page.is_some_and(|page| page.metadata == *owner && page.entries.contains(entry))
}
impl git_history::History {
    /// A first page starts a new comparison. Continuations keep the user's file
    /// and loaded diff; an initially empty page may acquire its first file later.
    pub(super) fn accept_files(&mut self, page: CommitFiles, continuation: bool) -> Option<Delta> {
        if !continuation {
            self.file_demand = Demand::default();
        }
        self.files = Some(page);
        if continuation && self.path.is_some() {
            return None;
        }
        self.path = None;
        self.diff = None;
        self.diff_cursor = None;
        self.files
            .as_ref()?
            .entries
            .iter()
            .find(|entry| !git_history::file_path(entry).bytes_b64.is_empty())
            .cloned()
    }
    fn select_file(&mut self, owner: &DiffMetadata, entry: &Delta) -> bool {
        let path = git_history::file_path(entry);
        if self.error.is_some()
            || !select_current(self.files.as_ref(), owner, entry)
            || path.bytes_b64.is_empty()
            || Some(&path.bytes_b64) == self.path.as_ref().map(|path| &path.bytes_b64)
        {
            return false;
        }
        self.path = Some(path);
        self.diff = None;
        self.diff_cursor = None;
        true
    }
}
fn prepare_retry(
    history: &mut git_history::History,
    resources: &mut Resources,
    more: bool,
) -> Option<bool> {
    if ["git-history-files", "git-history-files-more"]
        .iter()
        .any(|key| resources.get(key).is_some_and(|r| r.pending))
    {
        return None;
    }
    history.file_cursor = None;
    history.file_demand = Demand::default();
    if more
        && history.error.is_none()
        && history
            .files
            .as_ref()
            .is_some_and(|files| files.next_cursor.is_some())
    {
        resources.invalidate("git-history-files-more");
        Some(true)
    } else {
        for key in [
            "git-history-files",
            "git-history-files-more",
            "git-history-diff",
            "git-history-diff-more",
        ] {
            resources.invalidate(key);
        }
        history.files = None;
        history.path = None;
        history.diff = None;
        history.diff_cursor = None;
        history.error = None;
        history.scroll = UniformListScrollHandle::default();
        Some(false)
    }
}
impl Newport {
    pub(super) fn git_history_file_height(&self) -> f32 {
        let unsettled = self.git.history.error.is_some()
            || ["git-history-files", "git-history-files-more"]
                .iter()
                .any(|key| {
                    self.resources
                        .get(key)
                        .is_some_and(|resource| resource.pending || resource.error.is_some())
                });
        compact_height(self.git.history.files.as_ref(), unsettled)
    }
    pub(super) fn git_history_select_file(&mut self, owner: &DiffMetadata, entry: &Delta) {
        if !self.git.history.select_file(owner, entry) {
            return;
        }
        self.git.diff_scroll = UniformListScrollHandle::default();
        for key in ["git-history-diff", "git-history-diff-more"] {
            self.resources.invalidate(key);
        }
        self.git_history_diff(false);
    }
    pub(super) fn git_history_files_scroll(&mut self) {
        let ready = self.page == Page::Projects
            && self.git_is_inspection()
            && self.git.remote_refs.is_none()
            && !self.project_dialog_open
            && (!git_layout::compact(self.content_width, self.git.tab) || self.git.detail_open)
            && self.git.history.error.is_none()
            && self
                .git
                .history
                .files
                .as_ref()
                .is_some_and(|files| files.next_cursor.is_some())
            && self.git.history.file_cursor.is_none()
            && ["git-history-files", "git-history-files-more"]
                .iter()
                .all(|key| {
                    self.resources
                        .get(key)
                        .is_none_or(|r| !r.pending && r.error.is_none())
                });
        let at_end = self.git.history.scroll.is_scrolled_to_end() != Some(false);
        let scrolled = self.git.history.file_demand.take_scrolled(
            &self.git.history.scroll,
            self.git
                .history
                .files
                .as_ref()
                .map_or(0, |files| files.entries.len()),
            28.,
            ready,
        );
        if self.git.history.file_demand.take(
            &identity(self.git.history.files.as_ref()),
            ready,
            at_end,
        ) || (ready && scrolled)
        {
            self.git_history_files(true);
        }
    }
    fn retry_history_files(&mut self, more: bool) {
        if !self.git_is_inspection() {
            return;
        }
        let Some(more) = prepare_retry(&mut self.git.history, &mut self.resources, more) else {
            return;
        };
        if !more {
            self.git.diff_scroll = UniformListScrollHandle::default();
        }
        self.git_history_files(more);
    }
    pub(super) fn git_history_file_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = self.git.history.files.as_ref();
        let has_page = page.is_some();
        let entries = page.map(|page| page.entries.clone()).unwrap_or_default();
        let loading = self
            .resources
            .get("git-history-files")
            .is_some_and(|r| r.pending);
        let more_loading = self
            .resources
            .get("git-history-files-more")
            .is_some_and(|r| r.pending);
        let first_failed = self.git.history.error.is_some()
            || self
                .resources
                .get("git-history-files")
                .is_some_and(|r| r.error.is_some());
        let more_failed = self
            .resources
            .get("git-history-files-more")
            .is_some_and(|r| r.error.is_some());
        let more = page.is_some_and(|page| page.next_cursor.is_some());
        let truncated = page.is_some_and(|page| page.metadata.truncated == Some(true));
        let owner = page.map(|page| page.metadata.clone());
        let scroll_key = identity(page);
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .when_some(page, |view, page| {
                view.child(
                    div()
                        .id("commit-files-count")
                        .role(accesskit::Role::Label)
                        .aria_label(count_label(page))
                        .px_3()
                        .py_2()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(cx.theme().muted_foreground)
                        .child(count_label(page)),
                )
            })
            .when(first_failed, |view| {
                view.child(self.status("git-history-files", cx))
            })
            .when(more_failed, |view| {
                view.child(self.status("git-history-files-more", cx))
            })
            .when(truncated, |view| {
                view.child(
                    crate::label(
                        "The file list exceeds the agent’s limit. Some files are not listed.",
                    )
                    .p_3()
                    .text_size(px(crate::ui::FONT_SIZE)),
                )
            })
            .when(
                loading && !has_page && self.placeholder_due("git-history-files"),
                |view| view.child(project_loading::listing("commit files", 0, 8)),
            )
            .when(
                has_page && entries.is_empty() && !first_failed && !loading,
                |view| {
                    view.child(empty::compact(
                        if more
                            || truncated
                            || page
                                .and_then(|page| page.metadata.total_files)
                                .is_some_and(|n| n > 0)
                        {
                            "No files in the loaded page."
                        } else {
                            "No changed files against this parent."
                        },
                        cx,
                    ))
                },
            )
            .child(
                div()
                    .id("history-file-scroll-region")
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, _| {
                        if event.delta.pixel_delta(px(28.)).y < px(0.) {
                            this.git.history.file_demand.arm(scroll_key.clone());
                        }
                    }))
                    .child(
                        uniform_list(
                            "git-history-files-list",
                            entries.len(),
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|i| {
                                        let entry = entries[i].clone();
                                        let owner = owner.clone();
                                        let path = git_history::file_path(&entry);
                                        let title = path.display.clone();
                                        let mark = git_file_row::mark(&entry.status);
                                        let description = git_file_row::description(
                                            git_file_row::origin(
                                                Some(&entry.old_path),
                                                &entry.new_path,
                                            ),
                                            &title,
                                            git_file_row::name(mark),
                                        );
                                        let valid = !path.bytes_b64.is_empty();
                                        let selected = valid
                                            && Some(&path.bytes_b64)
                                                == this
                                                    .git
                                                    .history
                                                    .path
                                                    .as_ref()
                                                    .map(|path| &path.bytes_b64);
                                        div().h(px(28.)).w_full().min_w_0().px(px(6.)).child(
                                            git_file_row::row_focus(
                                                crate::controls::button(("git-history-file", i)),
                                                cx,
                                            )
                                            .accessibility_label(description.clone())
                                            .hover_hint(description)
                                            .custom(git_file_row::selection_style(selected, cx))
                                            .selected(selected)
                                            .w_full()
                                            .min_w_0()
                                            .disabled(!valid)
                                            .toggled(selected)
                                            .child(git_file_row::badge(mark, cx))
                                            .child(git_file_row::path(title, cx))
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    if let Some(owner) = &owner {
                                                        this.git_history_select_file(owner, &entry);
                                                    }
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(&self.git.history.scroll)
                        .flex_1()
                        .min_h_0(),
                    ),
            )
            .when(more_loading, |view| {
                view.child(project_loading::listing("more commit files", 0, 1))
            })
            .when(first_failed || more_failed, |view| {
                view.child(
                    crate::controls::compact_button("git-more-commit-files")
                        .label("Try again")
                        .accessibility_label(if first_failed {
                            "Try loading commit files again"
                        } else {
                            "Try loading more commit files again"
                        })
                        .disabled(loading || more_loading)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.retry_history_files(!first_failed);
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Demand, compact_height, count_label, identity, select_current};
    use crate::git_history::CommitFiles;
    use newport_protocol::git::{Delta, Page, Path};
    fn delta(path: &str) -> Delta {
        Delta {
            old_path: Path::new(path.as_bytes()),
            new_path: Path::new(path.as_bytes()),
            old_oid: None,
            new_oid: None,
            old_mode: 0o100644,
            new_mode: 0o100644,
            status: "Modified".into(),
        }
    }
    fn files(count: usize, next: Option<&str>) -> CommitFiles {
        Page {
            snapshot: "a".into(),
            entries: (0..count).map(|i| delta(&format!("file-{i}"))).collect(),
            next_cursor: next.map(Into::into),
            metadata: crate::git_fixture::commit_metadata("one", b"file"),
        }
    }
    #[test]
    fn compact_file_list_gives_short_completed_lists_space_without_squashing_feedback() {
        assert_eq!(compact_height(Some(&files(1, None)), false), 68.);
        assert_eq!(compact_height(Some(&files(2, None)), false), 96.);
        assert_eq!(compact_height(Some(&files(4, None)), false), 130.);
        assert_eq!(compact_height(Some(&files(0, None)), false), 80.);
        assert_eq!(compact_height(Some(&files(1, None)), true), 130.);
        assert_eq!(compact_height(Some(&files(1, Some("more"))), false), 130.);
        let mut truncated = files(0, None);
        truncated.metadata.truncated = Some(true);
        assert_eq!(compact_height(Some(&truncated), false), 130.);
        assert_eq!(compact_height(None, false), 130.);
    }
    #[test]
    fn first_file_opens_once_and_continuations_preserve_the_selected_diff() {
        let mut unnamed = delta("");
        unnamed.new_path.bytes_b64.clear();
        let deleted = Delta {
            status: "Deleted".into(),
            ..delta("removed")
        };
        let next = delta("second");
        let mut page = files(0, Some("next"));
        page.entries = vec![unnamed, deleted.clone(), next.clone()];
        let mut history = crate::git_history::History::default();
        let first = history.accept_files(page.clone(), false).unwrap();
        assert_eq!(first, deleted, "deleted files use their original path");
        assert!(history.select_file(&page.metadata, &first));
        assert!(history.select_file(&page.metadata, &next));
        let loaded_diff = crate::git_fixture::diff_page(
            "loaded",
            crate::git_fixture::commit_metadata("one", b"second"),
            vec![],
            Some("diff-next"),
        );
        history.diff = Some(loaded_diff.clone());
        history.diff_cursor = Some("diff-next".into());
        assert!(
            !history.select_file(&page.metadata, &next),
            "clicking the selected file must not refetch"
        );
        let mut appended = page.clone();
        appended.entries.push(delta("third"));
        assert!(history.accept_files(appended, true).is_none());
        assert_eq!(history.path, Some(next.new_path.clone()));
        assert_eq!(history.diff, Some(loaded_diff));
        assert_eq!(history.diff_cursor.as_deref(), Some("diff-next"));

        let mut comparison = page.clone();
        comparison.metadata.parent_index = Some(1);
        let first = history.accept_files(comparison.clone(), false).unwrap();
        assert!(
            !history.select_file(&page.metadata, &next),
            "retained rows cannot select an old parent"
        );
        assert!(history.select_file(&comparison.metadata, &first));
        assert_eq!(history.path, Some(deleted.old_path.clone()));
        assert!(history.diff.is_none() && history.diff_cursor.is_none());
        history.error = Some("Commit detail does not match the selection.".into());
        assert!(!history.select_file(&comparison.metadata, &next));
        assert!(
            history.error.is_some(),
            "selection cannot hide a comparison validation error"
        );
    }
    #[test]
    fn empty_first_page_waits_for_a_selectable_file_and_new_empty_comparison_clears_diff() {
        let mut history = crate::git_history::History::default();
        let mut unnamed = delta("");
        unnamed.new_path.bytes_b64.clear();
        let mut page = files(0, Some("next"));
        page.entries = vec![unnamed];
        assert!(history.accept_files(page.clone(), false).is_none());
        page.entries.push(delta("file"));
        let first = history.accept_files(page.clone(), true).unwrap();
        assert!(history.select_file(&page.metadata, &first));
        history.diff = Some(crate::git_fixture::diff_page(
            "loaded",
            crate::git_fixture::commit_metadata("one", b"file"),
            vec![],
            None,
        ));
        let mut empty = files(0, None);
        empty.metadata = crate::git_fixture::commit_metadata("two", b"file");
        assert!(history.accept_files(empty, false).is_none());
        assert!(history.path.is_none() && history.diff.is_none());
        assert!(!history.select_file(&page.metadata, &first));
    }
    #[test]
    fn failed_continuation_can_retry_without_losing_loaded_files_or_selection() {
        let page = files(1, Some("cursor"));
        let mut history = crate::git_history::History::default();
        history.oid = "commit".into();
        history.files = Some(page.clone());
        history.file_cursor = Some("cursor".into());
        history.path = Some(Path::new(b"path"));
        let mut resources = crate::Resources::default();
        let generation = resources.begin("git-history-files-more").unwrap();
        assert_eq!(
            super::prepare_retry(&mut history, &mut resources, true),
            None
        );
        assert!(history.file_cursor.is_some());
        resources.complete(
            "git-history-files-more",
            generation,
            Err("Disconnected".into()),
        );
        assert_eq!(
            super::prepare_retry(&mut history, &mut resources, true),
            Some(true)
        );
        assert_eq!(history.files.as_ref(), Some(&page));
        assert_eq!(history.path, Some(Path::new(b"path")));
        assert!(history.file_cursor.is_none());
        assert!(resources.get("git-history-files-more").is_none());
        assert!(resources.begin("git-history-files-more").is_some());
    }
    #[test]
    fn invalid_comparison_retry_resets_pages_but_keeps_commit_identity() {
        let mut history = crate::git_history::History::default();
        history.oid = "commit".into();
        history.parent = 1;
        history.files = Some(files(1, Some("old")));
        history.error = Some("Invalid page".into());
        history.path = Some(Path::new(b"path"));
        let mut resources = crate::Resources::default();
        assert_eq!(
            super::prepare_retry(&mut history, &mut resources, true),
            Some(false)
        );
        assert_eq!(history.oid, "commit");
        assert_eq!(history.parent, 1);
        assert!(history.files.is_none() && history.path.is_none() && history.error.is_none());
    }
    #[test]
    fn scroll_demand_is_consumed_and_cannot_cross_comparisons_or_pages() {
        let page = files(1, Some("first"));
        let mut demand: Demand<super::Identity> = Demand::default();
        assert!(!demand.take(&identity(Some(&page)), true, true));
        demand.arm(identity(Some(&page)));
        assert!(demand.take(&identity(Some(&page)), true, true));
        assert!(!demand.take(&identity(Some(&page)), true, true));
        let next = files(1, Some("second"));
        demand.arm(identity(Some(&page)));
        assert!(!demand.take(&identity(Some(&next)), true, true));
        demand.arm(identity(Some(&page)));
        assert!(!demand.take(&identity(Some(&page)), false, true));
        assert!(!demand.take(&identity(Some(&page)), true, true));
        demand.arm(identity(Some(&page)));
        assert!(!demand.take(&identity(Some(&page)), true, false));
    }
    #[test]
    fn counts_distinguish_total_loaded_and_truncated() {
        let mut page = files(1, None);
        page.metadata.total_files = Some(1);
        assert_eq!(count_label(&page), "1 changed file");
        page.metadata.total_files = Some(120);
        page.next_cursor = Some("c".into());
        assert_eq!(count_label(&page), "1 of 120 files loaded");
        page.metadata.total_files = None;
        page.next_cursor = None;
        page.metadata.truncated = Some(true);
        assert_eq!(count_label(&page), "1 file loaded");
    }
    #[test]
    fn delayed_file_selection_must_belong_to_current_comparison() {
        let page = files(1, None);
        let file = page.entries[0].clone();
        assert!(select_current(Some(&page), &page.metadata, &file));
        let previous = crate::git_fixture::commit_metadata("previous", b"file");
        assert!(!select_current(Some(&page), &previous, &file));
        assert!(!select_current(
            Some(&page),
            &page.metadata,
            &delta("other")
        ));
    }
}
