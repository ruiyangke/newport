//! Per-repository drafts survive page changes; only confirmed commits clear them.
use super::*;
use gpui_kit::component::input::{InputEvent, TextareaState};

pub(super) struct Draft {
    summary: Entity<InputState>,
    description: Entity<TextareaState>,
    pub clear_if_unchanged: Option<(String, String)>,
    _subscriptions: Vec<Subscription>,
}
fn compose(summary: &str, description: &str) -> String {
    if description.trim().is_empty() {
        summary.trim().into()
    } else {
        format!("{}\n\n{}", summary.trim(), description.trim())
    }
}
fn commit_counts(status: &Value) -> (u64, u64, bool) {
    let counts = &status["metadata"]["groupCounts"];
    let entries = status["entries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let staged = counts["staged"].as_u64().unwrap_or_else(|| {
        entries
            .iter()
            .filter(|e| e["staged"] == true && e["conflicted"] != true)
            .count() as u64
    });
    let conflicts = counts["conflicted"]
        .as_u64()
        .unwrap_or_else(|| entries.iter().filter(|e| e["conflicted"] == true).count() as u64);
    let partial = status["nextCursor"].is_string()
        || status["metadata"]["truncated"] == true
        || status["metadata"]["totalEntries"]
            .as_u64()
            .is_some_and(|n| n > entries.len() as u64);
    (staged, conflicts, partial)
}

fn commit_title(status: &Value, head: &Value) -> String {
    let (staged, conflicts, partial) = commit_counts(status);
    if conflicts > 0 {
        return "Resolve conflicts before committing".into();
    }
    let exact = status["metadata"]["groupCounts"].is_object();
    if staged == 0 && (!partial || exact) {
        return "No staged changes to commit".into();
    }
    let mut title = if partial && !exact {
        "Commit staged changes".to_owned()
    } else {
        format!(
            "Commit {staged} {}",
            if staged == 1 { "file" } else { "files" }
        )
    };
    if head["detached"] != true
        && let Some(branch) = head["name"]["display"].as_str()
        && let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch)
        && !branch.is_empty()
    {
        title.push_str(" to ");
        title.push_str(branch);
    }
    title
}

fn long_summary(summary: &str) -> bool {
    // Match the reference's advisory threshold; this never rejects a commit.
    summary.trim().encode_utf16().count() > 50
}
impl Newport {
    fn git_draft_key(&self) -> Option<String> {
        let root = self.git.repo["root"]["bytesB64"].as_str()?;
        let server = self.selected_server()?;
        Some(json!([terminal_scope::connection(server, &self.snapshot), root]).to_string())
    }
    pub(super) fn sync_git_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for draft in self.git_drafts.values_mut() {
            if let Some((summary, description)) = draft.clear_if_unchanged.take()
                && draft.summary.read(cx).value().as_str() == summary
                && draft.description.read(cx).value().as_str() == description
            {
                draft
                    .summary
                    .update(cx, |input, cx| input.set_value("", window, cx));
                draft
                    .description
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        if self.page != Page::Projects {
            return;
        }
        let Some(key) = self.git_draft_key() else {
            return;
        };
        if self.git_drafts.contains_key(&key) {
            return;
        }
        let summary = cx.new(|cx| InputState::new(window, cx).placeholder("Summary (required)"));
        let description = cx.new(|cx| TextareaState::new(window, cx).placeholder("Description"));
        let submit_key = key.clone();
        let subscriptions = vec![
            cx.observe(&summary, |_, _, cx| cx.notify()),
            cx.observe(&description, |_, _, cx| cx.notify()),
            cx.subscribe(&summary, move |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. })
                    && this.git_draft_key().as_ref() == Some(&submit_key)
                    && this
                        .git_drafts
                        .get(&submit_key)
                        .is_some_and(|draft| draft.summary == input)
                {
                    // Use the same current-state validation and operation owner
                    // as the button. A cached draft cannot submit another view.
                    this.git_commit(cx);
                }
            }),
        ];
        self.git_drafts.insert(
            key,
            Draft {
                summary,
                description,
                clear_if_unchanged: None,
                _subscriptions: subscriptions,
            },
        );
    }
    fn project_commit_status<'a>(&'a self, cx: &'a App) -> (&'a Value, bool) {
        if let Some(editor) = self.code_workspace() {
            let e = editor.read(cx);
            if e.navigator.panel == 1 {
                let status = &e.navigator.collections[0];
                return (
                    status.page().as_ref(),
                    e.navigator.writable
                        && !status.loading
                        && status.error.is_none()
                        && !e.navigator.invalidated
                        && e.navigator.filter_changed.is_none(),
                );
            }
        }
        (
            &self.git.page,
            self.git.tab == 0
                && self.git.error.is_none()
                && !self
                    .resources
                    .get("git-list")
                    .is_some_and(|r| r.pending || r.error.is_some()),
        )
    }
    fn can_git_commit(&self, summary: &str, cx: &App) -> bool {
        let (status, current) = self.project_commit_status(cx);
        let (staged, conflicts, partial) = commit_counts(status);
        self.page == Page::Projects
            && current
            && !self.project_dialog_open
            && !self.modal_open
            && !self.git_mutation_blocks_write()
            && !summary.trim().is_empty()
            && (staged > 0 || (partial && status["metadata"]["groupCounts"].is_null()))
            && conflicts == 0
            && status["metadata"]["integration"].is_null()
            && self.git.repo["capabilities"]["readOnly"] == false
            && self.git.repo["bare"] == false
            && status["snapshot"].as_str().is_some_and(|s| !s.is_empty())
    }
    fn git_commit(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.git_draft_key() else {
            return;
        };
        let Some(draft) = self.git_drafts.get(&key) else {
            return;
        };
        let summary = draft.summary.read(cx).value().to_string();
        let description = draft.description.read(cx).value().to_string();
        if !self.can_git_commit(&summary, cx) {
            return;
        }
        let message = compose(&summary, &description);
        let snapshot = text(self.project_commit_status(cx).0, "snapshot");
        self.start_git_mutation(
            self.git_repository_id(),
            "Commit staged changes".into(),
            move |backend, server, repo, id| backend.commit(server, repo, &snapshot, &message, id),
            cx,
        );
        if let Some(operation) = self.git_mutation.as_mut() {
            operation.commit_draft = Some((key, summary, description));
        }
    }
    pub(super) fn git_commit_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let (status, _) = self.project_commit_status(cx);
        let Some(draft) = self
            .git_draft_key()
            .and_then(|key| self.git_drafts.get(&key))
        else {
            return div().into_any_element();
        };
        if !status["metadata"]["integration"].is_null() {
            return div().into_any_element();
        }
        let summary = draft.summary.read(cx).value();
        let (_, _, partial) = commit_counts(status);
        let head = status["metadata"]
            .get("head")
            .unwrap_or(&self.git.repo["head"]);
        let title = commit_title(status, head);
        let enabled = self.can_git_commit(summary.as_str(), cx);
        let busy = self.git_mutation_blocks_write();
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(6.))
            .bg(cx.theme().muted)
            .p(px(10.))
            .border_t_1()
            .border_color(cx.theme().border)
            .child(crate::controls::input(&draft.summary).aria_label("Commit summary").disabled(busy))
            .when(long_summary(summary.as_str()), |view| {
                view.child(crate::label("Summaries under 50 characters read best in logs. Put the detail in the description.").text_size(px(crate::ui::FONT_SIZE)))
            })
            .child(
                crate::controls::textarea(&draft.description)
                    .h(px(66.))
                    .disabled(busy)
                    .aria_label("Commit description"),
            )
            .when(partial, |view| {
                view.child(
                    crate::label("Commits all staged changes, including files not loaded here.")
                        .text_size(px(crate::ui::FONT_SIZE)),
                )
            })
            .child(
                crate::controls::button("git-commit")
                    .label(title)
                    .when(enabled, |button| button.primary())
                    .when(!enabled, |button| button.bg(cx.theme().background).text_color(cx.theme().muted_foreground).border_color(cx.theme().border))
                    .disabled(!enabled)
                    .on_click(cx.listener(|this, _, _, cx| this.git_commit(cx))),
            )
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{commit_counts, commit_title, compose, long_summary};
    use serde_json::json;
    #[test]
    fn message_uses_summary_blank_line_and_body() {
        assert_eq!(
            compose(" Summary ", " Body\nsecond line "),
            "Summary\n\nBody\nsecond line"
        );
        assert_eq!(compose(" Summary ", " \n"), "Summary");
    }
    #[test]
    fn counts_include_unloaded_staged_files_and_conflicts() {
        assert_eq!(
            commit_counts(
                &json!({"entries":[],"nextCursor":"next","metadata":{"groupCounts":{"staged":8,"conflicted":2}}})
            ),
            (8, 2, true)
        );
        assert_eq!(
            commit_counts(
                &json!({"entries":[{"staged":true},{"staged":true,"conflicted":true}],"metadata":{}})
            ),
            (1, 1, false)
        );
    }

    #[test]
    fn commit_labels_use_full_index_counts_even_when_the_list_is_filtered() {
        let head = json!({"name":{"display":"refs/heads/main"}, "detached":false});
        let mut status = json!({"entries":[], "nextCursor":"next", "metadata":{}});
        assert_eq!(
            commit_title(&status, &head),
            "Commit staged changes to main"
        );
        status["metadata"]["groupCounts"] = json!({"staged":6000, "conflicted":0});
        assert_eq!(commit_title(&status, &head), "Commit 6000 files to main");
        status["metadata"]["groupCounts"]["staged"] = json!(1);
        assert_eq!(commit_title(&status, &head), "Commit 1 file to main");
        assert_eq!(
            commit_title(&status, &json!({"detached":true})),
            "Commit 1 file"
        );
        assert_eq!(commit_title(&status, &json!({})), "Commit 1 file");
        status["metadata"]["groupCounts"]["staged"] = json!(0);
        assert_eq!(commit_title(&status, &head), "No staged changes to commit");
        status["metadata"]["groupCounts"]["conflicted"] = json!(1);
        assert_eq!(
            commit_title(&status, &head),
            "Resolve conflicts before committing"
        );
    }

    #[test]
    fn summary_length_advisory_matches_the_reference_without_changing_the_message() {
        let summary = "x".repeat(50);
        assert!(!long_summary(&format!("  {summary}  ")));
        assert!(long_summary(&format!("{summary}x")));
        assert!(!long_summary(&"雪".repeat(50)));
        assert!(long_summary(&"🦀".repeat(26)));
        let long = "Keep the entire detailed summary even when it exceeds the recommended length";
        assert!(long_summary(long));
        assert_eq!(compose(long, ""), long);
    }
}
