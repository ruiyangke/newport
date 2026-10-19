//! Literal file text in Kit's read-only, virtualized editor. The preview owns
//! this state so redraws and wrapping preserve selection and scrolling.
use gpui_kit::{
    AnyElement, App, AppContext, Edges, Entity, FontWeight, IntoElement, ParentElement, Styled,
    Window,
    component::{
        ActiveTheme,
        input::{Editor, EditorState, TabSize, Textarea, TextareaState, WrappingIndent},
    },
    div,
    prelude::*,
    px, relative,
};
use std::cell::{Cell, RefCell};

// WebKit resolves ui-monospace to the system monospaced UI family on macOS.
fn preview_font(cx: &App) -> gpui_kit::SharedString {
    if cfg!(target_os = "macos") {
        ".AppleSystemUIFontMonospaced".into()
    } else {
        cx.theme().mono_font_family.clone()
    }
}

pub(super) struct PreviewText {
    editor: Entity<TextareaState>,
    code: Option<Entity<EditorState>>,
    content: RefCell<String>,
    wrap: Cell<bool>,
}

impl PreviewText {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            editor: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .tab_size(TabSize {
                        tab_size: 4,
                        hard_tabs: true,
                    })
                    .searchable(true)
                    .replaceable(false)
                    .soft_wrap(false)
                    .wrapping_indent(WrappingIndent::None)
                    .placeholder("(Empty file)")
            }),
            code: None,
            content: RefCell::new(String::new()),
            wrap: Cell::new(false),
        }
    }

    pub(super) fn for_path(path: &str, window: &mut Window, cx: &mut App) -> Self {
        let mut preview = Self::new(window, cx);
        if let Some(language) = crate::git_diff_syntax::language(path) {
            preview.code = Some(cx.new(|cx| {
                EditorState::new(window, cx)
                    .language(language)
                    .line_number(true)
                    .folding(false)
                    .searchable(true)
                    .replaceable(false)
                    .soft_wrap(false)
                    .wrapping_indent(WrappingIndent::None)
                    .tab_size(TabSize {
                        tab_size: 4,
                        hard_tabs: true,
                    })
                    .placeholder("(Empty file)")
            }));
        }
        preview
    }

    pub(super) fn view(
        &self,
        content: &str,
        wrap: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        // Replacing the actual document invalidates old selection; changing
        // only its presentation must not call set_value and reset that state.
        if *self.content.borrow() != content {
            if let Some(code) = &self.code {
                code.update(cx, |editor, cx| {
                    editor.set_value(content.to_owned(), window, cx)
                });
            } else {
                self.editor.update(cx, |editor, cx| {
                    editor.set_value(content.to_owned(), window, cx)
                });
            }
            content.clone_into(&mut self.content.borrow_mut());
        }
        if self.wrap.replace(wrap) != wrap {
            if let Some(code) = &self.code {
                code.update(cx, |editor, cx| editor.set_soft_wrap(wrap, window, cx));
            } else {
                self.editor
                    .update(cx, |editor, cx| editor.set_soft_wrap(wrap, window, cx));
            }
        }
        div()
            .id("file-preview-text")
            .debug_selector(|| "file-preview-text".into())
            .flex_1()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(if let Some(code) = &self.code {
                Editor::new(code)
                    .readonly(true)
                    .appearance(false)
                    .bordered(false)
                    .aria_label("Source file preview")
                    .h_full()
                    .font_family(preview_font(cx))
                    .text_size(px(crate::ui::FONT_SIZE))
                    .line_height(relative(1.7))
                    .into_any_element()
            } else {
                Textarea::new(&self.editor)
                    .readonly(true)
                    .appearance(false)
                    .bordered(false)
                    // Keep the inset inside the editor's scrolling surface.
                    // Frame padding clips text early and leaves a permanent
                    // blank band above the footer, unlike the original <pre>.
                    .content_padding(Edges {
                        top: px(20.),
                        right: px(20.),
                        bottom: px(20.),
                        left: px(20.),
                    })
                    .aria_label("File preview text")
                    .h_full()
                    .font_family(preview_font(cx))
                    .font_weight(FontWeight::NORMAL)
                    .text_size(px(crate::ui::FONT_SIZE))
                    .line_height(relative(1.7))
                    .p_0()
                    .into_any_element()
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext, point, size};
    use gpui_kit as gpui;

    struct Harness {
        text: PreviewText,
        content: String,
        wrap: bool,
        width: gpui::Pixels,
    }
    impl Render for Harness {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(self.width)
                .h_full()
                .child(self.text.view(&self.content, self.wrap, window, cx))
        }
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    fn fixture<'a>(
        cx: &'a mut TestAppContext,
        content: &str,
    ) -> (Entity<Harness>, &'a mut VisualTestContext) {
        cx.update(gpui::init);
        let content = content.to_owned();
        let (view, cx) = cx.add_window_view(|window, cx| Harness {
            text: PreviewText::new(window, cx),
            content,
            wrap: false,
            width: px(500.),
        });
        cx.update(|window, _| window.resize(size(px(500.), px(300.))));
        draw(cx);
        (view, cx)
    }

    #[gpui::test]
    fn source_preview_uses_tree_sitter_and_keeps_code_read_only(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::init(cx);
            crate::grammars::init().unwrap();
        });
        let source = "fn main() {\n    let greeting = \"雪\";\n}\n";
        let (view, cx) = cx.add_window_view(|window, cx| Harness {
            text: PreviewText::for_path("/src/main.rs", window, cx),
            content: source.into(),
            wrap: false,
            width: px(500.),
        });
        draw(cx);
        let editor = view.read_with(cx, |view, _| view.text.code.clone().expect("code editor"));
        assert_eq!(
            editor.read_with(cx, |state, _| state.language_name().to_string()),
            "rust"
        );
        // Verify the registered native grammar actually produces token styles.
        use gpui::component::highlighter::{HighlightTheme, SyntaxHighlighter};
        let mut syntax = SyntaxHighlighter::new("rust");
        assert!(syntax.update(None, &gpui::base::input::Rope::from_str(source), None));
        assert!(
            !syntax
                .styles(&(0..source.len()), HighlightTheme::default_light().as_ref())
                .is_empty()
        );
        cx.update(|window, cx| editor.update(cx, |state, cx| state.focus(window, cx)));
        crate::test_shortcuts(cx, "cmd-a cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            source
        );
        cx.simulate_input("must not edit");
        crate::test_shortcuts(cx, "backspace cmd-v");
        draw(cx);
        assert_eq!(
            editor.read_with(cx, |state, _| state.value().to_string()),
            source
        );
        view.update(cx, |view, cx| {
            view.wrap = true;
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            editor.read_with(cx, |state, _| state.value().to_string()),
            source
        );
        crate::test_shortcuts(cx, "cmd-f");
        draw(cx);
        assert!(editor.read_with(cx, |state, _| state.search_session().open));
    }

    #[gpui::test]
    #[ignore = "manual Files text-preview CPU benchmark; excludes GPU presentation and SSH"]
    fn benchmark_files_text_frames(cx: &mut TestAppContext) {
        for count in [50, 5000] {
            let content = (0..count)
                .map(|i| format!("Line {i}: native read-only preview with literal text\n"))
                .collect::<String>();
            let (view, cx) = fixture(cx, &content);
            let start = std::time::Instant::now();
            for _ in 0..30 {
                view.update(cx, |_, cx| cx.notify());
                draw(cx);
            }
            eprintln!(
                "Files text {count} lines: {:.3} ms/CPU frame",
                start.elapsed().as_secs_f64() * 1000. / 30.
            );
        }
    }

    #[gpui::test]
    fn plain_text_preview_has_reference_inset_without_code_gutters(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, "First line\nSecond line");
        let editor = view.read_with(cx, |view, _| view.text.editor.clone());
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| state.focus(window, cx));
        });
        draw(cx);
        editor.read_with(cx, |state, _| {
            let (caret, line_height) = state.cursor_layout().expect("focused preview caret");
            assert!((line_height - px(crate::ui::FONT_SIZE * 1.7)).abs() < px(0.001));
            assert_eq!(caret.left(), px(20.));
            assert_eq!(state.input_bounds().top(), px(20.));
        });
        crate::test_shortcuts(cx, "cmd-f");
        draw(cx);
        assert!(editor.read_with(cx, |state, _| state.search_session().open));
        cx.simulate_keystrokes("escape");
        crate::test_shortcuts(cx, "cmd-a cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            "First line\nSecond line"
        );
    }

    #[gpui::test]
    fn preview_search_exposes_named_controls_and_result_state(cx: &mut TestAppContext) {
        use gpui::accesskit::Role;
        use gpui::base::test_support::snapshots;
        let (view, cx) = fixture(cx, "Docker docker");
        let editor = view.read_with(cx, |view, _| view.text.editor.clone());
        cx.update(|window, cx| editor.update(cx, |state, cx| state.focus(window, cx)));
        crate::test_shortcuts(cx, "cmd-f");
        draw(cx);
        let labels = |cx: &mut VisualTestContext| {
            cx.update(|window, _| {
                snapshots(window)
                    .iter()
                    .filter_map(|node| node.label().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
        };
        let click = |label: &str, cx: &mut VisualTestContext| {
            let at = cx.update(|window, _| {
                snapshots(window)
                    .iter()
                    .find(|node| node.label() == Some(label))
                    .unwrap_or_else(|| panic!("missing {label}"))
                    .bounds()
                    .center()
            });
            cx.simulate_click(at, Default::default());
            draw(cx);
        };
        let names = labels(cx);
        for expected in [
            "Find in text",
            "Match case",
            "Previous match",
            "Next match",
            "Close search",
            "Enter search text",
        ] {
            assert!(
                names.iter().any(|name| name == expected),
                "missing {expected}: {names:?}"
            );
        }
        assert!(
            !names
                .iter()
                .any(|name| name == "Toggle replace" || name == "Replace with")
        );
        cx.simulate_input("Docker");
        draw(cx);
        assert!(labels(cx).iter().any(|name| name == "Match 1 of 2"));
        click("Next match", cx);
        assert_eq!(
            editor.read_with(cx, |state, _| state
                .search_session()
                .matcher
                .current_match_index()),
            1
        );
        assert!(labels(cx).iter().any(|name| name == "Match 2 of 2"));
        click("Previous match", cx);
        assert_eq!(
            editor.read_with(cx, |state, _| state
                .search_session()
                .matcher
                .current_match_index()),
            0
        );
        click("Match case", cx);
        assert!(!editor.read_with(cx, |state, _| state.search_session().case_insensitive));
        assert!(labels(cx).iter().any(|name| name == "Match 1 of 1"));
        click("Find in text", cx);
        crate::test_shortcuts(cx, "cmd-a");
        cx.simulate_input("no matching text");
        draw(cx);
        cx.update(|window, _| {
            let nodes = snapshots(window);
            assert!(nodes.iter().any(
                |node| node.role() == Some(Role::Status) && node.label() == Some("No matches")
            ));
        });
        click("Close search", cx);
        assert!(!editor.read_with(cx, |state, _| state.search_session().open));
        assert_eq!(
            editor.read_with(cx, |state, _| state.value().to_string()),
            "Docker docker"
        );
    }

    #[gpui::test]
    fn literal_tabs_align_with_four_spaces_like_the_reference(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, "\ttext\n    text");
        let editor = view.read_with(cx, |view, _| view.text.editor.clone());
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_selected_range(1..1, cx);
            });
        });
        draw(cx);
        let after_tab = editor.read_with(cx, |state, _| state.cursor_layout().unwrap().0.left());
        editor.update(cx, |state, cx| state.set_selected_range(10..10, cx));
        draw(cx);
        let after_spaces = editor.read_with(cx, |state, _| state.cursor_layout().unwrap().0.left());
        assert_eq!(after_tab, after_spaces);
        assert!(after_tab > px(20.));
    }

    #[gpui::test]
    fn tabs_keep_stops_copy_and_wrapping_in_sync(cx: &mut TestAppContext) {
        let content = "a\tb\t\n雪\tend\t";
        let (view, cx) = fixture(cx, content);
        let editor = view.read_with(cx, |view, _| view.text.editor.clone());
        cx.update(|window, cx| editor.update(cx, |state, cx| state.focus(window, cx)));
        let caret = |offset, cx: &mut VisualTestContext| {
            editor.update(cx, |state, cx| state.set_selected_range(offset..offset, cx));
            draw(cx);
            editor.read_with(cx, |state, _| state.cursor_layout().unwrap().0.origin)
        };
        let start = caret(0, cx);
        let one = caret(1, cx).x - start.x;
        assert!((caret(2, cx).x - start.x - one * 4.).abs() < px(0.01));
        assert!((caret(4, cx).x - start.x - one * 8.).abs() < px(0.01));
        // A trailing tab still owns its advance, including after UTF-8 text.
        assert!(caret(content.len(), cx).x > caret(content.len() - 1, cx).x);
        crate::test_shortcuts(cx, "cmd-a cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            content
        );
        view.update(cx, |view, cx| {
            view.content = "\t\tX\n        X".into();
            view.wrap = true;
            // Include the two 20px insets and Kit's 10px caret margin.
            view.width = px(50.) + one * 8.5;
            cx.notify();
        });
        // Eight spaces fit but X must wrap on both equivalent lines.
        draw(cx);
        let tab_x = caret(3, cx);
        let space_x = caret(13, cx);
        assert!(
            (tab_x.x - space_x.x).abs() < px(0.01),
            "tab={tab_x:?} spaces={space_x:?} cell={one:?} bounds={:?}",
            editor.read_with(cx, |state, _| state.input_bounds())
        );
        assert!(
            (tab_x.x - px(20.) - one).abs() < px(0.01),
            "tab={tab_x:?} spaces={space_x:?} cell={one:?} bounds={:?}",
            editor.read_with(cx, |state, _| state.input_bounds())
        );
        assert!(tab_x.y > px(20.));
    }

    #[gpui::test]
    fn unicode_selection_copies_literal_text_and_edits_are_rejected(cx: &mut TestAppContext) {
        let content = "<script>literal</script>\n雪 café 🦀\n\tindented";
        let (view, cx) = fixture(cx, content);
        let editor = view.read_with(cx, |v, _| v.text.editor.clone());
        cx.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.focus(window, cx);
                v.set_selected_range(24..content.len(), cx);
            })
        });
        crate::test_shortcuts(cx, "cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            &content[24..]
        );
        cx.simulate_input("must not edit");
        crate::test_shortcuts(cx, "backspace cmd-v");
        draw(cx);
        assert_eq!(editor.read_with(cx, |v, _| v.value().to_string()), content);
        crate::test_shortcuts(cx, "cmd-a cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            content
        );
    }

    #[gpui::test]
    fn rerender_retains_selection_scroll_and_wrap_keeps_same_document(cx: &mut TestAppContext) {
        let content = (0..2000)
            .map(|i| format!("line {i}: {}\n", "wide ".repeat(40)))
            .collect::<String>();
        let (view, cx) = fixture(cx, &content);
        let editor = view.read_with(cx, |v, _| v.text.editor.clone());
        editor.update(cx, |v, cx| {
            v.set_selected_range(5..10, cx);
            v.set_scroll_offset(point(px(-40.), px(-400.)), cx);
        });
        draw(cx);
        let before = editor.read_with(cx, |v, _| v.scroll_offset());
        assert!(before.y < px(0.));
        view.update(cx, |_, cx| cx.notify());
        draw(cx);
        assert_eq!(editor.read_with(cx, |v, _| v.scroll_offset()), before);
        assert_eq!(editor.read_with(cx, |v, _| v.selected_range()), 5..10);
        view.update(cx, |v, cx| {
            v.wrap = true;
            cx.notify();
        });
        draw(cx);
        assert_eq!(editor.read_with(cx, |v, _| v.selected_range()), 5..10);
        assert_eq!(editor.read_with(cx, |v, _| v.value().to_string()), content);
        assert_eq!(editor.read_with(cx, |v, _| v.scroll_offset().x), px(0.));
        view.update(cx, |v, cx| {
            v.content = "replacement".into();
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            editor.read_with(cx, |v, _| v.value().to_string()),
            "replacement"
        );
        assert_eq!(editor.read_with(cx, |v, _| v.selected_range()), 0..0);
    }
}
