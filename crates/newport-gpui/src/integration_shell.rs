//! Retained selection and copy feedback for the Integration shell snippet.
use super::SHELL_CONFIGURATION;
use crate::*;
use gpui_kit::component::input::{Textarea, TextareaState};

pub(crate) struct ShellCode {
    editor: Entity<TextareaState>,
    copied: bool,
}

impl ShellCode {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).soft_wrap(false);
            state.set_value(SHELL_CONFIGURATION, window, cx);
            state
        });
        Self {
            editor,
            copied: false,
        }
    }
}

impl Render for ShellCode {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .mt(px(16.))
            .mb(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .bg(cx.theme().muted)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("integration-shell-label")
                            .role(Role::Label)
                            .aria_label("Shell configuration")
                            .test_support()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child("Shell configuration"),
                    )
                    .child(
                        crate::controls::button("integration-copy-shell")
                            .label(if self.copied { "Copied" } else { "Copy" })
                            .icon(if self.copied {
                                IconName::Check
                            } else {
                                IconName::Copy
                            })
                            .ghost()
                            .accessibility_label(if self.copied {
                                "Shell configuration copied"
                            } else {
                                "Copy shell configuration"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    SHELL_CONFIGURATION.into(),
                                ));
                                this.copied = true;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Textarea::new(&self.editor)
                    .readonly(true)
                    .appearance(false)
                    .bordered(false)
                    .content_padding(Edges::default())
                    .aria_label("Shell configuration")
                    .h(px(SHELL_CONFIGURATION.lines().count() as f32 * 16.5 + 24.))
                    .font_family(if cfg!(target_os = "macos") {
                        SharedString::from(".AppleSystemUIFontMonospaced")
                    } else {
                        cx.theme().mono_font_family.clone()
                    })
                    .font_weight(FontWeight::NORMAL)
                    .text_size(px(11.))
                    .line_height(relative(1.5))
                    .p(px(12.)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{SHELL_CONFIGURATION, ShellCode};
    use crate::theme;
    use gpui_kit::{self as gpui, TestAppContext, VisualTestContext, px, size};

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui::test]
    fn shell_selection_survives_redraw_and_is_readonly(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
        });
        let (view, cx) = cx.add_window_view(ShellCode::new);
        cx.simulate_resize(size(px(400.), px(300.)));
        draw(cx);
        let editor = view.read_with(cx, |view, _| view.editor.clone());
        cx.update(|window, cx| editor.update(cx, |state, cx| state.focus(window, cx)));
        crate::test_shortcuts(cx, "cmd-a");
        view.update(cx, |_, cx| cx.notify());
        draw(cx);
        crate::test_shortcuts(cx, "cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            SHELL_CONFIGURATION
        );
        cx.simulate_input("must not overwrite the shell snippet");
        cx.simulate_keystrokes("BackSpace");
        assert_eq!(
            editor.read_with(cx, |state, _| state.value().to_string()),
            SHELL_CONFIGURATION
        );
        // Both keyboard focus and selection remain available at narrow widths.
        cx.simulate_resize(size(px(280.), px(300.)));
        draw(cx);
        crate::test_shortcuts(cx, "cmd-a cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            SHELL_CONFIGURATION
        );
    }

    #[gpui::test]
    fn copy_button_copies_entire_snippet_and_keeps_inline_feedback(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            theme::install(false, cx);
        });
        let (view, cx) = cx.add_window_view(ShellCode::new);
        draw(cx);
        let point = cx.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .find(|node| node.label() == Some("Copy shell configuration"))
                .expect("Copy button")
                .bounds()
                .center()
        });
        cx.simulate_click(point, Default::default());
        draw(cx);
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            SHELL_CONFIGURATION
        );
        view.update(cx, |_, cx| cx.notify());
        draw(cx);
        assert!(cx.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .iter()
                .any(|node| node.label() == Some("Shell configuration copied"))
        }));
    }
}
