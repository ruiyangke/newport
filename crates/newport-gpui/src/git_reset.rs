//! Explicit reset modes for an immutable, reviewed history target.
use super::*;
use gpui_kit::component::tab::Tab;
use newport_protocol::git::{Action, ResetMode};

const MODES: [ResetMode; 3] = [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard];

pub(super) fn ready(action: &Action, confirmed: bool) -> bool {
    match action {
        Action::Reset {
            mode: ResetMode::Soft | ResetMode::Mixed,
            ..
        } => true,
        Action::Reset {
            mode: ResetMode::Hard,
            ..
        } => confirmed,
        _ => false,
    }
}

impl Newport {
    pub(super) fn reset_review_controls(
        &self,
        review: &git_integration::Review,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Action::Reset { mode, .. } = &review.action else {
            return div().into_any_element();
        };
        let mode = *mode;
        let busy = self.git_mutation_blocks_write();
        div().flex().flex_col().gap_2()
            .child(crate::controls::tabs("git-reset-mode").gap_0()
                .selected_index(MODES.iter().position(|value| *value == mode).unwrap_or(0))
                .children(["Soft", "Mixed", "Hard"].into_iter().map(|name| Tab::new().label(name)))
                .on_click(cx.listener(|this, index: &usize, _, cx| {
                    if this.git_mutation_blocks_write() { return; }
                    if let Some(mode) = MODES.get(*index)
                        && let Some(review) = this.git.integration_review.as_mut()
                        && let Action::Reset { mode: selected, .. } = &mut review.action {
                        *selected = *mode;
                        review.reset_confirmed = false;
                        cx.notify();
                    }
                })))
            .child(crate::label(match mode {
                ResetMode::Soft => "Moves HEAD only. Staging and working files stay unchanged.",
                ResetMode::Mixed => "Resets staging to the selected commit. Working files stay unchanged; differences become unstaged.",
                ResetMode::Hard => "Replaces tracked working files and staging with the selected commit. Uncommitted tracked changes will be lost. Untracked files obstructing target paths may also be removed.",
            }))
            .when(mode == ResetMode::Hard, |view| view.child(
                crate::controls::checkbox("git-confirm-hard-reset")
                    .label("I understand tracked edits and obstructing untracked files may be lost")
                    .checked(review.reset_confirmed)
                    .disabled(busy)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        if !this.git_mutation_blocks_write()
                            && let Some(review) = this.git.integration_review.as_mut()
                            && matches!(&review.action, Action::Reset { mode: ResetMode::Hard, .. }) {
                            review.reset_confirmed = *checked;
                            cx.notify();
                        }
                    }))
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, ResetMode, ready};
    #[test]
    fn hard_reset_requires_acknowledgement_and_non_reset_actions_are_refused() {
        let reset = |mode| Action::Reset {
            target_oid: "a".repeat(40),
            expected_oid: "b".repeat(40),
            mode,
        };
        assert!(ready(&reset(ResetMode::Soft), false));
        assert!(ready(&reset(ResetMode::Mixed), false));
        assert!(!ready(&reset(ResetMode::Hard), false));
        assert!(ready(&reset(ResetMode::Hard), true));
        assert!(!ready(&Action::IntegrationAbort {}, true));
    }
}
