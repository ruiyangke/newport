//! Selectable, read-only Git content in Kit's virtualized editor.
use super::*;
use gpui_kit::component::input::{Editor, EditorState};
use std::{cell::Cell, sync::Arc};

#[derive(Default)]
pub(super) struct BlobText {
    editor: Option<Entity<EditorState>>,
    source: Arc<String>,
    rendered: Option<Arc<String>>,
    display: String,
    line_count: usize,
    scroll_demand: Cell<bool>,
}
impl BlobText {
    pub(super) fn set_content(&mut self, content: Arc<String>, cx: &mut Context<Self>) {
        if !Arc::ptr_eq(&self.source, &content) {
            self.source = content;
            self.scroll_demand.set(false);
            cx.notify();
        }
    }
    pub(super) fn take_scroll_demand(&self, cx: &App) -> bool {
        if !self.scroll_demand.replace(false) {
            return false;
        }
        self.editor.as_ref().is_some_and(|editor| {
            let editor = editor.read(cx);
            editor
                .visible_row_range()
                .is_some_and(|range| range.end >= self.line_count)
        })
    }
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<EditorState> {
        let editor = self
            .editor
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    EditorState::new(window, cx)
                        .line_number(true)
                        .searchable(true)
                        .replaceable(false)
                        .soft_wrap(false)
                        .scroll_beyond_last_line(Some(0))
                        .placeholder("(Empty file)")
                })
            })
            .clone();
        if self
            .rendered
            .as_ref()
            .is_none_or(|previous| !Arc::ptr_eq(previous, &self.source))
        {
            // Normalize CR/LF before checking a prefix, including a CRLF split
            // across pages. The previous trailing CR already supplied its LF.
            let display = self.source.replace("\r\n", "\n").replace('\r', "\n");
            let append = self.rendered.is_some() && display.starts_with(&self.display);
            editor.update(cx, |editor, cx| {
                if append {
                    let selected = editor.selected_range();
                    let scroll = editor.scroll_offset();
                    let end = editor.value().len();
                    editor.set_selected_range(end..end, cx);
                    editor.insert(display[self.display.len()..].to_owned(), window, cx);
                    editor.set_selected_range(selected, cx);
                    editor.set_scroll_offset(scroll, cx);
                } else {
                    editor.set_value(display.clone(), window, cx);
                }
            });
            self.display = display;
            self.line_count = self.display.bytes().filter(|b| *b == b'\n').count() + 1;
            self.rendered = Some(self.source.clone());
        }
        editor
    }
}
impl Render for BlobText {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.sync(window, cx);
        let owner = cx.entity().downgrade();
        div()
            .id("git-blob-content")
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(
                // The editor consumes wheel events even at its scroll limit.
                // Observe intent during capture, without stealing its scroll.
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, _| {
                        window.on_mouse_event(
                            move |event: &ScrollWheelEvent, phase, window, cx| {
                                if phase.capture()
                                    && hitbox.id.should_handle_scroll(window)
                                    && event.delta.pixel_delta(px(20.)).y < px(0.)
                                {
                                    let _ =
                                        owner.update(cx, |this, _| this.scroll_demand.set(true));
                                }
                            },
                        );
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(
                Editor::new(&editor)
                    .readonly(true)
                    .appearance(false)
                    .bordered(false)
                    .aria_label("Recorded Git content")
                    .h_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(crate::project_ui::FONT_SIZE))
                    .line_height(relative(1.67))
                    .p(px(12.)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::BlobText;
    use gpui::{
        Context, Entity, IntoElement, Render, ScrollDelta, ScrollWheelEvent, TestAppContext,
        VisualTestContext, Window, div, point, prelude::*, px, size,
    };
    use gpui_kit as gpui;
    use std::sync::Arc;
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    #[gpui::test]
    fn append_preserves_selection_scroll_and_readonly_content(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let content = (0..200)
            .map(|i| format!("row {i} snow 雪 {}\n", "wide ".repeat(60)))
            .collect::<String>();
        let (view, cx) = cx.add_window_view(|_, _| BlobText {
            source: Arc::new(content.clone()),
            ..Default::default()
        });
        cx.update(|window, _| window.resize(size(px(500.), px(300.))));
        draw(cx);
        let editor = view.read_with(cx, |v, _| v.editor.clone().unwrap());
        cx.update(|window, cx| {
            editor.update(cx, |e, cx| {
                e.focus(window, cx);
                e.set_selected_range(0..5, cx);
                e.set_scroll_offset(point(px(-50.), px(-200.)), cx);
            })
        });
        draw(cx);
        let scroll = editor.read_with(cx, |e, _| e.scroll_offset());
        assert!(scroll.y < px(0.));
        crate::test_shortcuts(cx, "cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            "row 0"
        );
        cx.simulate_input("must not edit");
        cx.simulate_keystrokes("backspace");
        draw(cx);
        assert_eq!(editor.read_with(cx, |e, _| e.value().to_string()), content);
        let extended = format!("{content}last page\r");
        view.update(cx, |v, cx| v.set_content(Arc::new(extended.clone()), cx));
        draw(cx);
        assert_eq!(editor.read_with(cx, |e, _| e.selected_range()), 0..5);
        assert_eq!(editor.read_with(cx, |e, _| e.scroll_offset()), scroll);
        view.update(cx, |v, cx| {
            v.set_content(Arc::new(format!("{extended}\nfinal")), cx)
        });
        draw(cx);
        assert_eq!(
            editor.read_with(cx, |e, _| e.value().to_string()),
            format!("{content}last page\nfinal")
        );
        assert_eq!(editor.read_with(cx, |e, _| e.selected_range()), 0..5);
        view.update(cx, |v, cx| {
            v.set_content(Arc::new("different blob".into()), cx)
        });
        draw(cx);
        assert_eq!(editor.read_with(cx, |e, _| e.selected_range()), 0..0);
    }
    #[gpui::test]
    fn scrolling_requests_one_page_only_at_the_bottom(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| BlobText {
            source: Arc::new("line\n".repeat(400)),
            ..Default::default()
        });
        cx.update(|window, _| window.resize(size(px(500.), px(300.))));
        draw(cx);
        assert!(!view.read_with(cx, |v, cx| v.take_scroll_demand(cx)));
        // Ordinary scrolling in the middle must not fetch the next page.
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(250.), px(150.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-100.))),
            ..Default::default()
        });
        draw(cx);
        assert!(!view.read_with(cx, |v, cx| v.take_scroll_demand(cx)));
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(250.), px(150.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-10000.))),
            ..Default::default()
        });
        draw(cx);
        assert!(view.read_with(cx, |v, cx| v.take_scroll_demand(cx)));
        assert!(!view.read_with(cx, |v, cx| v.take_scroll_demand(cx)));
        // At the bottom, outside, horizontal, and upward gestures are inert.
        for (position, delta) in [
            (point(px(1500.), px(1500.)), point(px(0.), px(-100.))),
            (point(px(250.), px(150.)), point(px(-100.), px(0.))),
            (point(px(250.), px(150.)), point(px(0.), px(1.))),
        ] {
            cx.simulate_event(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(delta),
                ..Default::default()
            });
            draw(cx);
            assert!(!view.read_with(cx, |v, cx| v.take_scroll_demand(cx)));
        }
        view.update(cx, |v, cx| {
            v.scroll_demand.set(true);
            v.set_content(Arc::new("new blob".into()), cx);
        });
        assert!(!view.read_with(cx, |v, cx| v.take_scroll_demand(cx)));
    }

    #[gpui::test]
    fn an_overlay_does_not_request_hidden_content(cx: &mut TestAppContext) {
        struct Host {
            editor: Entity<BlobText>,
            covered: bool,
        }
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .child(self.editor.clone())
                    .when(self.covered, |view| {
                        view.child(div().absolute().top_0().left_0().size_full().occlude())
                    })
            }
        }
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| Host {
            editor: cx.new(|_| BlobText {
                source: Arc::new("first fragment".into()),
                ..Default::default()
            }),
            covered: true,
        });
        cx.update(|window, _| window.resize(size(px(500.), px(300.))));
        draw(cx);
        let editor = host.read_with(cx, |host, _| host.editor.clone());
        for covered in [true, false] {
            host.update(cx, |host, cx| {
                host.covered = covered;
                cx.notify();
            });
            draw(cx);
            cx.simulate_event(ScrollWheelEvent {
                position: point(px(250.), px(150.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-100.))),
                ..Default::default()
            });
            draw(cx);
            assert_eq!(
                editor.read_with(cx, |v, cx| v.take_scroll_demand(cx)),
                !covered
            );
        }
    }

    #[gpui::test]
    fn a_short_byte_limited_page_can_continue_without_a_button(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| BlobText {
            source: Arc::new("first fragment".into()),
            ..Default::default()
        });
        cx.update(|window, _| window.resize(size(px(500.), px(300.))));
        draw(cx);
        assert!(!view.read_with(cx, |v, cx| v.take_scroll_demand(cx)));
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(250.), px(150.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-100.))),
            ..Default::default()
        });
        draw(cx);
        assert!(view.read_with(cx, |v, cx| v.take_scroll_demand(cx)));
        assert!(!view.read_with(cx, |v, cx| v.take_scroll_demand(cx)));
    }
}
