//! Immutable conflict-side previews and snapshot-bound resolutions.
use super::*;
use gpui_kit::component::tab::Tab;
use newport_client::git_blob::Content;
use newport_protocol::git::Request as GitRequest;
use std::cell::RefCell;

const SIDES: [&str; 3] = ["ours", "theirs", "base"];
const LABELS: [&str; 3] = ["Ours", "Theirs", "Base"];
#[derive(Default)]
pub(super) struct Preview {
    side: usize,
    content: Content,
    error: Option<String>,
    editor: RefCell<Option<Entity<git_blob_text::BlobText>>>,
}

fn resolution(
    entry: Option<&newport_protocol::git::StatusEntry>,
    side: usize,
) -> Option<newport_protocol::git::Action> {
    newport_client::git_conflict::resolution(entry?, SIDES.get(side)?)
}

impl Newport {
    fn conflict_blob_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.initial_loading("git-conflict-preview") {
            return project_loading::blob();
        }
        let preview = &self.git.conflict;
        let mut slot = preview.editor.borrow_mut();
        let editor = slot.get_or_insert_with(|| cx.new(|_| git_blob_text::BlobText::default()));
        if let Some(text) = &preview.content.text {
            editor.update(cx, |editor, cx| editor.set_content(text.clone(), cx));
        }
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(editor.clone())
            .into_any_element()
    }
    pub(super) fn reset_conflict_preview(&mut self) {
        for key in ["git-conflict-preview", "git-conflict-more"] {
            self.resources.invalidate(key);
        }
        self.git.conflict = Preview::default();
    }
    pub(super) fn select_conflict_side(&mut self, side: usize) {
        self.reset_conflict_preview();
        if side >= SIDES.len() {
            return;
        }
        self.git.conflict.side = side;
        if let Some(newport_protocol::git::Action::ConflictResolve { expected_oid, .. }) =
            resolution(self.git.entry(), side)
        {
            self.git.conflict.content.oid = expected_oid.unwrap_or_default();
            self.request_conflict_preview(false);
        }
    }
    fn request_conflict_preview(&mut self, more: bool) {
        let preview = &self.git.conflict;
        if self.git.tab != 0
            || !self.git.entry().is_some_and(|entry| entry.conflicted)
            || preview.content.oid.is_empty()
            || preview.content.binary
            || preview.error.is_some()
        {
            return;
        }
        if ["git-conflict-preview", "git-conflict-more"]
            .iter()
            .any(|key| {
                self.resources
                    .get(key)
                    .is_some_and(|r| r.pending || r.error.is_some())
            })
        {
            return;
        }
        let cursor = if more {
            let Some(cursor) = &preview.content.cursor else {
                return;
            };
            Some(cursor.clone())
        } else {
            None
        };
        let request = GitRequest::BlobPage {
            repo_id: self.git_repository_id(),
            oid: preview.content.oid.clone(),
            max_bytes: Some(65536),
            cursor,
        };
        self.request_git_typed(
            if more {
                "git-conflict-more"
            } else {
                "git-conflict-preview"
            },
            request,
        );
    }
    pub(super) fn conflict_preview_completed(&mut self, key: &str) {
        if self.resources.get(key).is_some_and(|r| r.error.is_some()) {
            return;
        }
        let result = match self.resources.get(key).and_then(|row| row.git()) {
            Some(newport_protocol::git::Reply::BlobPage(page)) => {
                self.git.conflict.content.append(page)
            }
            _ => Err("Conflict preview response does not match the requested content.".into()),
        };
        if let Err(error) = result {
            self.git.conflict.error = Some(error);
        }
    }
    pub(super) fn poll_conflict_preview(&mut self, cx: &App) {
        if self.page == Page::Projects
            && self.git.tab == 0
            && self.git.entry().is_some_and(|entry| entry.conflicted)
            && self
                .git
                .conflict
                .editor
                .borrow()
                .as_ref()
                .is_some_and(|editor| editor.read(cx).take_scroll_demand(cx))
        {
            self.request_conflict_preview(true);
        }
    }
    fn review_conflict_resolution(&mut self, cx: &mut Context<Self>) {
        if !self.git_can_change_index(true) || self.git.integration_review.is_some() {
            return;
        }
        let Some(action) = resolution(self.git.entry(), self.git.conflict.side) else {
            return;
        };
        let side = LABELS[self.git.conflict.side];
        let path = self
            .git
            .entry()
            .map(|entry| entry.path.display.clone())
            .unwrap_or_default();
        let newport_protocol::git::Action::ConflictResolve { expected_oid, .. } = &action else {
            return;
        };
        let detail = match expected_oid {
            None => format!(
                "{side} has no file. Remove {path} and stage its deletion as the resolution."
            ),
            Some(oid) => format!(
                "Replace {path} with the recorded {side} content and stage the resolution. This replaces any manual edits in the working file. Recorded blob: {oid}"
            ),
        };
        self.git.integration_review = Some(git_integration::Review {
            origin: git_integration::ReviewOrigin::Changes,
            id: uuid::Uuid::new_v4(),
            action,
            snapshot: self.git.snapshot().to_owned(),
            title: format!("Resolve {path} using {side}"),
            detail,
            reset_confirmed: false,
            mainline: None,
            message: None,
            _subscription: None,
        });
        cx.notify();
    }
    pub(super) fn conflict_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let preview = &self.git.conflict;
        let action = resolution(self.git.entry(), preview.side);
        let absent = matches!(
            &action,
            Some(newport_protocol::git::Action::ConflictResolve {
                expected_oid: None,
                ..
            })
        );
        let side = preview.side;
        let has_error = preview.error.is_some()
            || ["git-conflict-preview", "git-conflict-more"]
                .iter()
                .any(|key| self.resources.get(key).is_some_and(|r| r.error.is_some()));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                div().p_3().child(
                    self.git
                        .entry()
                        .map_or_else(|| "—".into(), |entry| entry.path.display.clone()),
                ),
            )
            .child(
                crate::label(
                    "Choose a recorded side, or edit the working file and stage your resolution.",
                )
                .px_3(),
            )
            .child(
                div().flex().p_3().child(
                    crate::controls::tabs("conflict-sides")
                        .gap_0()
                        .selected_index(side)
                        .children(LABELS.map(|title| Tab::new().label(title)))
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            this.select_conflict_side(*index);
                            cx.notify();
                        })),
                ),
            )
            .child(
                crate::label(match side {
                    0 => "Index stage 2. During rebase, Ours is the upstream version.",
                    1 => "Index stage 3. During rebase, Theirs is the commit being replayed.",
                    _ => "Index stage 1: the common ancestor.",
                })
                .text_size(px(crate::ui::FONT_SIZE))
                .px_3(),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .p_3()
                    .child(
                        crate::controls::button("review-conflict-side")
                            .label(if absent {
                                "Resolve as deleted"
                            } else {
                                "Use selected side…"
                            })
                            .disabled(
                                action.is_none()
                                    || !self.git_can_change_index(true)
                                    || self.git.integration_review.is_some(),
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.review_conflict_resolution(cx)),
                            ),
                    )
                    .child(
                        crate::controls::button("stage-manual-resolution")
                            .label("Stage resolution")
                            .disabled(
                                !self.git_can_change_index(true)
                                    || self.git.integration_review.is_some(),
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.git_change_index(true, cx)),
                            ),
                    ),
            )
            .child(self.status("git-conflict-preview", cx))
            .child(self.status("git-conflict-more", cx))
            .when_some(preview.error.clone(), |view, error| {
                view.child(crate::label(error).p_3())
            })
            .when(has_error, |view| {
                view.child(
                    crate::controls::button("retry-conflict-preview")
                        .label("Try again")
                        .accessibility_label("Try loading the preview again")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_conflict_side(side);
                            cx.notify();
                        })),
                )
            })
            .when(action.is_none(), |view| {
                view.child(
                    crate::label(
                        "Recorded conflict sides are unavailable. Refresh the repository.",
                    )
                    .p_3(),
                )
            })
            .when(absent, |view| {
                view.child(crate::label("This side deleted the file.").p_3())
            })
            .when(preview.content.binary, |view| {
                view.child(crate::label("Binary content. This side can still be chosen.").p_3())
            })
            .when(
                !preview.content.binary
                    && !absent
                    && (preview.content.text.is_some()
                        || self.initial_loading("git-conflict-preview")),
                |view| view.child(self.conflict_blob_editor(cx)),
            )
            .when(!preview.content.snapshot.is_empty(), |view| {
                view.child(
                    crate::label(format!(
                        "{} of {} bytes loaded",
                        preview.content.bytes.len(),
                        preview.content.size.unwrap_or(0)
                    ))
                    .text_size(px(crate::ui::FONT_SIZE))
                    .p_3(),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::resolution;
    use newport_protocol::git::{Conflict, ConflictEntry, Path};
    fn recorded(hex: &str) -> ConflictEntry {
        ConflictEntry {
            mode: 0o100644,
            oid: crate::git_fixture::object(hex),
            path: Path::new(b"file"),
        }
    }
    #[test]
    fn resolutions_capture_recorded_blob_or_explicit_absence() {
        let oid = "a".repeat(40);
        let mut entry = crate::git_fixture::status_entry("entry", "file");
        entry.conflicted = true;
        entry.conflict = Some(Conflict {
            base: Some(recorded("")),
            ours: Some(recorded(&oid)),
            theirs: None,
        });
        assert!(
            matches!(resolution(Some(&entry), 0), Some(newport_protocol::git::Action::ConflictResolve { expected_oid: Some(value), .. }) if value == oid)
        );
        assert!(matches!(
            resolution(Some(&entry), 1),
            Some(newport_protocol::git::Action::ConflictResolve {
                expected_oid: None,
                ..
            })
        ));
        assert!(resolution(Some(&entry), 2).is_none());
        assert!(resolution(Some(&entry), 3).is_none());
        assert!(resolution(None, 0).is_none());
        let mut malformed = entry;
        malformed.conflict.as_mut().unwrap().ours = Some(recorded("invalid"));
        assert!(resolution(Some(&malformed), 0).is_none());
    }
}
