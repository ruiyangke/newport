//! Explicit reset modes for an immutable, reviewed history target.
use super::*;
use gpui_kit::component::tab::Tab;

const MODES: [&str; 3] = ["soft", "mixed", "hard"];

pub(super) fn ready(action: &Value, confirmed: bool) -> bool {
    match action["mode"].as_str() {
        Some("soft" | "mixed") => true,
        Some("hard") => confirmed,
        _ => false,
    }
}

impl Newport {
    pub(super) fn reset_review_controls(
        &self,
        review: &git_integration::Review,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode = review.action["mode"].as_str().unwrap_or_default();
        let busy = self.git_mutation_blocks_write();
        div().flex().flex_col().gap_2()
            .child(crate::project_ui::tabs("git-reset-mode").segmented()
                .selected_index(MODES.iter().position(|value| *value == mode).unwrap_or(0))
                .children(["Soft", "Mixed", "Hard"].into_iter().map(|name| Tab::new().label(name)))
                .on_click(cx.listener(|this, index: &usize, _, cx| {
                    if this.git_mutation_blocks_write() { return; }
                    if let Some(mode) = MODES.get(*index)
                        && let Some(review) = this.git.integration_review.as_mut()
                        && review.action["kind"] == "reset" {
                        review.action["mode"] = json!(mode);
                        review.reset_confirmed = false;
                        cx.notify();
                    }
                })))
            .child(crate::project_ui::label(match mode {
                "soft" => "Moves HEAD only. Staging and working files stay unchanged.",
                "mixed" => "Resets staging to the selected commit. Working files stay unchanged; differences become unstaged.",
                "hard" => "Replaces tracked working files and staging with the selected commit. Uncommitted tracked changes will be lost. Untracked files obstructing target paths may also be removed.",
                _ => "Select a reset mode.",
            }))
            .when(mode == "hard", |view| view.child(
                crate::project_ui::checkbox("git-confirm-hard-reset")
                    .label("I understand tracked edits and obstructing untracked files may be lost")
                    .checked(review.reset_confirmed)
                    .disabled(busy)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        if !this.git_mutation_blocks_write()
                            && let Some(review) = this.git.integration_review.as_mut()
                            && review.action["mode"] == "hard" {
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
    use super::ready;
    use serde_json::json;
    #[test]
    fn hard_reset_requires_acknowledgement_and_unknown_modes_are_refused() {
        assert!(ready(&json!({"mode":"soft"}), false));
        assert!(ready(&json!({"mode":"mixed"}), false));
        assert!(!ready(&json!({"mode":"hard"}), false));
        assert!(ready(&json!({"mode":"hard"}), true));
        assert!(!ready(&json!({"mode":"invalid"}), true));
    }
}
