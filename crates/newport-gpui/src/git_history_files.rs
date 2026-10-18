//! Historical file list: immutable comparison identity, bounded pagination and
//! explicit recovery without hiding the previously loaded file rows.
use super::*;
use gpui_kit::base::Selectable;

use crate::git_page_demand::{Demand, identity};

/// Compact files sit above the diff. Reserve the full cap for skeletons and
/// notices; completed short lists give their unused height back to the diff.
fn compact_height(page: &Value, unsettled: bool) -> f32 {
    let Some(entries) = page["entries"].as_array() else {
        return 130.;
    };
    if unsettled || page["nextCursor"].is_string() || page["metadata"]["truncated"] == true {
        return 130.;
    }
    if entries.is_empty() {
        return 80.;
    }
    (34. + 28. * entries.len().min(4) as f32 + 6.).min(130.)
}

fn count_label(page: &Value) -> String {
    let loaded = page["entries"].as_array().map(Vec::len).unwrap_or(0);
    let partial = page["nextCursor"].is_string() || page["metadata"]["truncated"] == true;
    match page["metadata"]["totalFiles"].as_u64() {
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
fn select_current(page: &Value, owner: &Value, entry: &Value) -> bool {
    page["metadata"] == *owner
        && page["entries"]
            .as_array()
            .is_some_and(|entries| entries.contains(entry))
}
impl git_history::History {
    /// A first page starts a new comparison. Continuations keep the user's file
    /// and loaded diff; an initially empty page may acquire its first file later.
    pub(super) fn accept_files(&mut self, page: Value, continuation: bool) -> Option<Value> {
        if !continuation {
            self.file_demand = Demand::default();
        }
        self.files = page;
        if continuation && self.path["bytesB64"].is_string() {
            return None;
        }
        self.path = Value::Null;
        self.diff = Value::Null;
        self.diff_cursor = None;
        self.files["entries"]
            .as_array()?
            .iter()
            .find(|entry| {
                git_history::file_path(entry)["bytesB64"]
                    .as_str()
                    .is_some_and(|path| !path.is_empty())
            })
            .cloned()
    }
    fn select_file(&mut self, owner: &Value, entry: &Value) -> bool {
        let path = git_history::file_path(entry);
        if self.error.is_some()
            || !select_current(&self.files, owner, entry)
            || path["bytesB64"].as_str().is_none_or(str::is_empty)
            || path["bytesB64"] == self.path["bytesB64"]
        {
            return false;
        }
        self.path = path;
        self.diff = Value::Null;
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
    if more && history.error.is_none() && history.files["nextCursor"].is_string() {
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
        history.files = Value::Null;
        history.path = Value::Null;
        history.diff = Value::Null;
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
        compact_height(&self.git.history.files, unsettled)
    }
    pub(super) fn git_history_select_file(&mut self, owner: &Value, entry: &Value) {
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
            && self.git.history.files["nextCursor"].is_string()
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
            self.git.history.files["entries"]
                .as_array()
                .map_or(0, Vec::len),
            28.,
            ready,
        );
        if self
            .git
            .history
            .file_demand
            .take(&self.git.history.files, ready, at_end)
            || (ready && scrolled)
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
        let page = &self.git.history.files;
        let has_page = page["entries"].is_array();
        let entries = rows(&page["entries"]);
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
        let more = page["nextCursor"].is_string();
        let truncated = page["metadata"]["truncated"] == true;
        let owner = page["metadata"].clone();
        let scroll_key = identity(page);
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .when(has_page, |view| {
                view.child(
                    div()
                        .id("commit-files-count")
                        .role(accesskit::Role::Label)
                        .aria_label(count_label(page))
                        .px_3()
                        .py_2()
                        .text_size(px(crate::project_ui::FONT_SIZE))
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
                    crate::project_ui::label(
                        "The file list exceeds the agent’s limit. Some files are not listed.",
                    )
                    .p_3()
                    .text_size(px(crate::project_ui::FONT_SIZE)),
                )
            })
            .when(loading && !has_page, |view| {
                view.child(project_loading::listing("commit files", 0, 8))
            })
            .when(
                has_page && entries.is_empty() && !first_failed && !loading,
                |view| {
                    view.child(empty::compact(
                        if more
                            || truncated
                            || page["metadata"]["totalFiles"]
                                .as_u64()
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
                    .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, _| {
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
                                        let title = path["display"]
                                            .as_str()
                                            .unwrap_or("Unknown file")
                                            .to_owned();
                                        let mark = git_file_row::mark(
                                            entry["status"].as_str().unwrap_or_default(),
                                        );
                                        let description = git_file_row::description(
                                            &entry,
                                            &title,
                                            git_file_row::name(mark),
                                        );
                                        let valid = path["bytesB64"]
                                            .as_str()
                                            .is_some_and(|path| !path.is_empty());
                                        let selected = valid
                                            && path["bytesB64"]
                                                == this.git.history.path["bytesB64"];
                                        div().h(px(28.)).w_full().min_w_0().px(px(6.)).child(
                                            crate::project_ui::button(("git-history-file", i))
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
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.git_history_select_file(&owner, &entry);
                                                    cx.notify();
                                                })),
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
                    crate::project_ui::compact_button("git-more-commit-files")
                        .label(if first_failed {
                            "Reload commit files"
                        } else {
                            "Retry loading files"
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
    use super::{Demand, compact_height, count_label, select_current};
    use serde_json::json;
    #[test]
    fn compact_file_list_gives_short_completed_lists_space_without_squashing_feedback() {
        assert_eq!(compact_height(&json!({"entries":[{}]}), false), 68.);
        assert_eq!(compact_height(&json!({"entries":[{},{}]}), false), 96.);
        assert_eq!(
            compact_height(&json!({"entries":[{},{},{},{}]}), false),
            130.
        );
        assert_eq!(compact_height(&json!({"entries":[]}), false), 80.);
        assert_eq!(compact_height(&json!({"entries":[{}]}), true), 130.);
        assert_eq!(
            compact_height(&json!({"entries":[{}],"nextCursor":"more"}), false),
            130.
        );
        assert_eq!(
            compact_height(&json!({"entries":[],"metadata":{"truncated":true}}), false),
            130.
        );
        assert_eq!(compact_height(&serde_json::Value::Null, false), 130.);
    }
    #[test]
    fn first_file_opens_once_and_continuations_preserve_the_selected_diff() {
        let deleted = json!({"oldPath":{"display":"removed","bytesB64":"cmVtb3ZlZA=="}});
        let next = json!({"newPath":{"display":"second","bytesB64":"c2Vjb25k"}});
        let page = json!({"metadata":{"commitOid":{"hex":"one"},"parentIndex":0},"entries":[{},deleted,next],"nextCursor":"next"});
        let mut history = crate::git_history::History::default();
        let first = history.accept_files(page.clone(), false).unwrap();
        assert_eq!(first, deleted, "deleted files use their original path");
        assert!(history.select_file(&page["metadata"], &first));
        assert!(history.select_file(&page["metadata"], &next));
        let loaded_diff = json!({"entries":[{"text":"loaded"}],"nextCursor":"diff-next"});
        history.diff = loaded_diff.clone();
        history.diff_cursor = Some("diff-next".into());
        assert!(
            !history.select_file(&page["metadata"], &next),
            "clicking the selected file must not refetch"
        );
        let mut appended = page.clone();
        appended["entries"]
            .as_array_mut()
            .unwrap()
            .push(json!({"newPath":{"bytesB64":"dGhpcmQ="}}));
        assert!(history.accept_files(appended, true).is_none());
        assert_eq!(history.path, next["newPath"]);
        assert_eq!(history.diff, loaded_diff);
        assert_eq!(history.diff_cursor.as_deref(), Some("diff-next"));

        let mut comparison = page.clone();
        comparison["metadata"]["parentIndex"] = json!(1);
        let first = history.accept_files(comparison.clone(), false).unwrap();
        assert!(
            !history.select_file(&page["metadata"], &next),
            "retained rows cannot select an old parent"
        );
        assert!(history.select_file(&comparison["metadata"], &first));
        assert_eq!(history.path, deleted["oldPath"]);
        assert!(history.diff.is_null() && history.diff_cursor.is_none());
        history.error = Some("Commit detail does not match the selection.".into());
        assert!(!history.select_file(&comparison["metadata"], &next));
        assert!(
            history.error.is_some(),
            "selection cannot hide a comparison validation error"
        );
    }
    #[test]
    fn empty_first_page_waits_for_a_selectable_file_and_new_empty_comparison_clears_diff() {
        let mut history = crate::git_history::History::default();
        let mut page = json!({"metadata":{"commitOid":{"hex":"one"}},"entries":[{}, {"newPath":{"bytesB64":""}}],"nextCursor":"next"});
        assert!(history.accept_files(page.clone(), false).is_none());
        page["entries"]
            .as_array_mut()
            .unwrap()
            .push(json!({"newPath":{"display":"file","bytesB64":"ZmlsZQ=="}}));
        let first = history.accept_files(page.clone(), true).unwrap();
        assert!(history.select_file(&page["metadata"], &first));
        history.diff = json!({"entries":[{}]});
        let empty = json!({"metadata":{"commitOid":{"hex":"two"}},"entries":[]});
        assert!(history.accept_files(empty, false).is_none());
        assert!(history.path.is_null() && history.diff.is_null());
        assert!(!history.select_file(&page["metadata"], &first));
    }
    #[test]
    fn failed_continuation_can_retry_without_losing_loaded_files_or_selection() {
        let page = json!({"entries":[{"fileIndex":0}],"nextCursor":"cursor"});
        let mut history = crate::git_history::History::default();
        history.oid = "commit".into();
        history.files = page.clone();
        history.file_cursor = Some("cursor".into());
        history.path = json!({"bytesB64":"path"});
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
        assert_eq!(history.files, page);
        assert_eq!(history.path["bytesB64"], "path");
        assert!(history.file_cursor.is_none());
        assert!(resources.get("git-history-files-more").is_none());
        assert!(resources.begin("git-history-files-more").is_some());
    }
    #[test]
    fn invalid_comparison_retry_resets_pages_but_keeps_commit_identity() {
        let mut history = crate::git_history::History::default();
        history.oid = "commit".into();
        history.parent = 1;
        history.files = json!({"entries":[{}],"nextCursor":"old"});
        history.error = Some("Invalid page".into());
        history.path = json!({"bytesB64":"path"});
        let mut resources = crate::Resources::default();
        assert_eq!(
            super::prepare_retry(&mut history, &mut resources, true),
            Some(false)
        );
        assert_eq!(history.oid, "commit");
        assert_eq!(history.parent, 1);
        assert!(history.files.is_null() && history.path.is_null() && history.error.is_none());
    }
    #[test]
    fn scroll_demand_is_consumed_and_cannot_cross_comparisons_or_pages() {
        let page = json!({"snapshot":"a","metadata":{"commitOid":{"hex":"one"}},"nextCursor":"first","entries":[{}]});
        let mut demand = Demand::default();
        assert!(!demand.take(&page, true, true));
        demand.arm(super::identity(&page));
        assert!(demand.take(&page, true, true));
        assert!(!demand.take(&page, true, true));
        let mut next = page.clone();
        next["nextCursor"] = json!("second");
        demand.arm(super::identity(&page));
        assert!(!demand.take(&next, true, true));
        demand.arm(super::identity(&page));
        assert!(!demand.take(&page, false, true));
        assert!(!demand.take(&page, true, true));
        demand.arm(super::identity(&page));
        assert!(!demand.take(&page, true, false));
    }
    #[test]
    fn counts_distinguish_total_loaded_and_truncated() {
        assert_eq!(
            count_label(&json!({"metadata":{"totalFiles":1},"entries":[{}]})),
            "1 changed file"
        );
        assert_eq!(
            count_label(&json!({"metadata":{"totalFiles":120},"entries":[{}],"nextCursor":"c"})),
            "1 of 120 files loaded"
        );
        assert_eq!(
            count_label(&json!({"metadata":{"truncated":true},"entries":[{}]})),
            "1 file loaded"
        );
    }
    #[test]
    fn delayed_file_selection_must_belong_to_current_comparison() {
        let file = json!({"newPath":{"display":"file","bytesB64":"ZmlsZQ=="}});
        let page = json!({"metadata":{"commitOid":{"hex":"current"}},"entries":[file.clone()]});
        assert!(select_current(&page, &page["metadata"], &file));
        assert!(!select_current(
            &page,
            &json!({"commitOid":{"hex":"previous"}}),
            &file
        ));
        assert!(!select_current(&page, &page["metadata"], &json!({})));
    }
}
