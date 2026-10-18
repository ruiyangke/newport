//! Destructive Git edits require an explicit review of an immutable target.
use super::*;
use gpui_kit::component::tab::Tab;
use newport_client::git_discard::{Review, Source};
use std::rc::Rc;

type Partial = (
    Rc<newport_client::git_selection::Review>,
    Vec<String>,
    Option<Vec<String>>,
);
struct Confirmation {
    owner: WeakEntity<Newport>,
    server: String,
    repo: String,
    path: String,
    review: Review,
    comparison: Option<(Rc<newport_client::git_selection::Review>, String)>,
    source: Source,
    _subscription: Subscription,
}
impl Newport {
    pub(super) fn git_can_discard(&self) -> bool {
        (self.git_can_change_index(true) || self.git_can_change_index(false))
            && Review::file(&self.git.page, &self.git.selected).is_ok()
    }
    pub(super) fn prepare_git_discard(
        &mut self,
        partial: Option<Partial>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) || !self.git_can_discard() {
            return;
        }
        let (review, comparison) = if let Some((review, hunks, lines)) = partial {
            let snapshot = text(self.diff_page(), "snapshot");
            if !git_diff_actions::current(
                self,
                &self.server,
                &self.git_repository_id(),
                &snapshot,
                &review,
            ) {
                return;
            }
            let Ok(target) = Review::selection(&review, &hunks, lines.as_deref()) else {
                return;
            };
            (target, Some((review, snapshot)))
        } else {
            let Ok(review) = Review::file(&self.git.page, &self.git.selected) else {
                return;
            };
            (review, None)
        };
        let owner = cx.entity();
        let server = self.server.clone();
        let repo = self.git_repository_id();
        let path = self.diff_title();
        let form = cx.new(|cx| Confirmation {
            owner: owner.downgrade(),
            server,
            repo,
            path,
            comparison,
            source: if review.supports_index() {
                Source::Index
            } else {
                Source::Head
            },
            review,
            _subscription: cx.observe(&owner, |_, _, cx| cx.notify()),
        });
        window.open_dialog(cx, move |dialog, window, cx| {
            let title = form.read(cx).title();
            dialog
                .aria_label(title.clone())
                .title(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .w(px(500.))
                .max_w((window.viewport_size().width - px(32.)).max(px(240.)))
                .p(px(24.))
                .rounded(px(14.))
                .border_0()
                .centered(true)
                .overlay_closable(false)
                .on_ok(|_, _, _| false)
                .child(form.clone())
        });
    }
}
impl Confirmation {
    fn title(&self) -> String {
        match self.review.line_count() {
            Some(count) => format!(
                "Discard {count} {}?",
                if count == 1 { "line" } else { "lines" }
            ),
            None if self.review.partial() => "Discard this hunk?".into(),
            None => "Discard file changes".into(),
        }
    }
    fn current(&self, owner: &Newport) -> bool {
        owner.page == Page::Projects
            && owner.server == self.server
            && owner.git_repository_id() == self.repo
            && owner.git_can_discard()
            && self.review.matches(
                &text(&owner.git.page, "snapshot"),
                &text(&owner.git.selected, "entryId"),
            )
            && self.comparison.as_ref().is_none_or(|(review, diff)| {
                git_diff_actions::current(owner, &self.server, &self.repo, diff, review)
            })
            && self.review.action(self.source).is_ok()
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let started = self
            .owner
            .update(cx, |owner, cx| {
                if !self.current(owner) {
                    return false;
                }
                let review = self.review.clone();
                let source = self.source;
                owner.start_git_mutation(
                    self.repo.clone(),
                    format!("{} · {}", self.title(), self.path),
                    move |backend, server, repo, id| {
                        backend.discard_changes(server, repo, &review, source, id)
                    },
                    cx,
                );
                true
            })
            .unwrap_or(false);
        // The durable, session-owned operation panel now owns success/failure
        // and ambiguous outcomes. Closing never cancels or repeats the write.
        if started {
            window.close_dialog(cx);
        }
    }
}
impl Render for Confirmation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self
            .owner
            .upgrade()
            .is_some_and(|owner| self.current(owner.read(cx)));
        div().flex().flex_col().gap_3().pt_3()
            .child(div().font_weight(FontWeight::MEDIUM).child(self.path.clone()))
            .when(!self.review.partial(), |view| view
                .child(crate::project_ui::label("Restore from"))
                .child(crate::project_ui::tabs("discard-restore-source").segmented()
                    .selected_index(usize::from(self.source == Source::Head))
                    .child(Tab::new().label("Staged version").disabled(!self.review.supports_index()))
                    .child(Tab::new().label("Last commit"))
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        let source = if *index == 0 {Source::Index} else {Source::Head};
                        if *index <= 1 && this.review.action(source).is_ok() { this.source = source; cx.notify(); }
                    }))))
            .child(crate::project_ui::label(if self.review.partial() {
                "The selected changes will be removed from the working file on the server. Other changes and the index are kept. There is no automatic undo."
            } else if self.source == Source::Index {
                "Discard this file’s unstaged changes and keep its staged changes. Files absent from the staged version will be deleted. There is no automatic undo."
            } else {
                "Discard this file’s staged and unstaged changes, restoring the last commit. Files absent from that commit will be deleted. There is no automatic undo."
            }))
            .when(!current, |view| view.child(crate::project_ui::label("The selection or repository state changed. Cancel and review the file again.").text_color(cx.theme().danger)))
            .child(project_dialog::actions()
                .child(crate::project_ui::form_button("cancel-discard-changes").label("Cancel")
                    .on_click(|_,window,cx| window.close_dialog(cx)))
                .child(crate::project_ui::form_button("confirm-discard-changes").danger().label("Discard")
                    .disabled(!current).on_click(cx.listener(|this,_,window,cx| this.submit(window,cx)))))
    }
}
