//! Retained, selectable presentation of already terminal-processed Git output.
use super::*;
use crate::selectable_log::SelectableLog;
use newport_gpui::git_log_text::{LogColor, LogLine, Style};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub(super) struct Document {
    pub focus: FocusHandle,
    pub selection: SelectableLog,
    rows: Arc<Vec<LogLine>>,
}
#[derive(Default)]
pub(super) struct Cache {
    document: RefCell<Option<Rc<RefCell<Document>>>>,
    trigger: RefCell<Option<FocusHandle>>,
}
impl Cache {
    pub fn trigger(&self, cx: &mut App) -> FocusHandle {
        self.trigger
            .borrow_mut()
            .get_or_insert_with(|| cx.focus_handle())
            .clone()
    }
    pub fn clear(&self, cx: &mut App) {
        if let Some(document) = self.document.borrow_mut().take() {
            document.borrow_mut().selection.clear(cx);
        }
    }
    pub fn prepare(
        &self,
        rows: &Arc<Vec<LogLine>>,
        window: &Window,
        cx: &mut App,
    ) -> Rc<RefCell<Document>> {
        let mut stored = self.document.borrow_mut();
        let document = stored.get_or_insert_with(|| {
            let focus = cx.focus_handle();
            Rc::new(RefCell::new(Document {
                selection: SelectableLog::new(focus.clone(), window, cx),
                focus,
                rows: Arc::default(),
            }))
        });
        let mut state = document.borrow_mut();
        if !Arc::ptr_eq(&state.rows, rows) {
            let mut continuation = false;
            state.selection.set_segments(
                rows.iter()
                    .map(|row| {
                        let segment = (Some(row.text.clone()), continuation);
                        continuation = row.wraps;
                        segment
                    })
                    .collect(),
                cx,
            );
            state.rows = rows.clone();
        }
        drop(state);
        document.clone()
    }
}

pub(super) fn columns(width: f32, window: &mut Window, cx: &App) -> u16 {
    let run = TextRun {
        len: 1,
        font: font(cx.theme().mono_font_family.clone()),
        color: rgb(0).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let cell = f32::from(
        window
            .text_system()
            .shape_line("M".into(), px(crate::project_ui::FONT_SIZE), &[run], None)
            .width,
    );
    ((width - 24.).max(0.) / cell.max(1.))
        .floor()
        .clamp(2., 500.) as u16
}
fn color(value: &LogColor, cx: &App) -> Hsla {
    let channels = match value {
        LogColor::Palette(256) => return cx.theme().foreground,
        LogColor::Palette(257) => return cx.theme().background,
        LogColor::Palette(index) => {
            let value = crate::terminal_color::palette(*index, cx.theme().is_dark());
            [value.r, value.g, value.b]
        }
        LogColor::Rgb(channels) => *channels,
    };
    rgb((u32::from(channels[0]) << 16) | (u32::from(channels[1]) << 8) | u32::from(channels[2]))
        .into()
}
fn highlight(style: &Style, cx: &App) -> HighlightStyle {
    let mut foreground = color(&style.foreground, cx);
    let mut background = color(&style.background, cx);
    if style.dim {
        foreground = foreground.opacity(0.66);
    }
    if style.inverse {
        std::mem::swap(&mut foreground, &mut background);
    }
    HighlightStyle {
        color: Some(foreground),
        background_color: (style.inverse || style.background != LogColor::Palette(257))
            .then_some(background),
        font_weight: style.bold.then_some(FontWeight::BOLD),
        font_style: style.italic.then_some(FontStyle::Italic),
        underline: style.underline.then_some(UnderlineStyle {
            thickness: px(1.),
            color: Some(
                style
                    .underline_color
                    .as_ref()
                    .map_or(foreground, |value| color(value, cx)),
            ),
            wavy: style.undercurl,
        }),
        strikethrough: style.strike.then_some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(foreground),
        }),
        ..Default::default()
    }
}

pub(super) fn line(document: &Rc<RefCell<Document>>, row: usize, cx: &App) -> AnyElement {
    div()
        .id(("git-output-line", row))
        .role(accesskit::Role::Label)
        .aria_label(document.borrow().rows[row].text.clone())
        .h(px(18.))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(crate::project_ui::FONT_SIZE))
        .whitespace_nowrap()
        .child(
            document
                .borrow()
                .selection
                .full_row(row)
                .with_highlights(
                    document.borrow().rows[row]
                        .spans
                        .iter()
                        .map(|span| (span.range.clone(), highlight(&span.style, cx)))
                        .collect::<Vec<_>>(),
                )
                .selection_color(cx.theme().primary.opacity(0.22)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Cache, Document, line};
    use gpui_kit as gpui;
    use gpui_kit::{
        AppContext, Context, IntoElement, MouseButton, Render, TestAppContext,
        UniformListScrollHandle, VisualTestContext, Window, div, point, prelude::*, px, size,
        uniform_list,
    };
    use newport_gpui::git_log_text::{LogLine, plain_text, render_styled, render_styled_at};
    use std::{cell::RefCell, rc::Rc, sync::Arc};
    struct View {
        rows: Arc<Vec<LogLine>>,
        cache: Cache,
        document: Option<Rc<RefCell<Document>>>,
        scroll: UniformListScrollHandle,
    }
    impl Render for View {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let document = self.cache.prepare(&self.rows, window, cx);
            self.document = Some(document.clone());
            let lines = document.clone();
            let list = uniform_list(
                "output",
                self.rows.len(),
                cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                    range.map(|i| line(&lines, i, cx)).collect()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full();
            let content = document.borrow().selection.viewport(list);
            div()
                .size_full()
                .flex()
                .track_focus(&document.borrow().focus)
                .child(content)
        }
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    #[gpui::test]
    fn wrapped_selection_copies_logical_lines_and_survives_reflow(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let source = "\x1b[31mCafé 🦀 tail\x1b[0m\n雪 done";
        let expected = "Café 🦀 tail\n雪 done";
        let mut entity = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| View {
                rows: Arc::new(render_styled_at(source, 8).unwrap()),
                cache: Cache::default(),
                document: None,
                scroll: UniformListScrollHandle::new(),
            });
            entity = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = entity.unwrap();
        cx.simulate_resize(size(px(400.), px(200.)));
        draw(cx);
        let doc = view.read_with(cx, |v, _| v.document.clone().unwrap());
        let last = doc.borrow().rows.len() - 1;
        let end = doc.borrow().rows[last].text.len();
        let from = doc.borrow().selection.text_position(0, 0).unwrap() + point(px(0.1), px(5.));
        let to = doc.borrow().selection.text_position(last, end).unwrap() + point(px(0.1), px(5.));
        cx.simulate_mouse_down(from, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Default::default());
        draw(cx);
        crate::test_shortcuts(cx, "cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            expected
        );
        assert_eq!(plain_text(&doc.borrow().rows), expected);
        view.update(cx, |v, cx| {
            v.rows = Arc::new(render_styled_at(source, 40).unwrap());
            cx.notify();
        });
        draw(cx);
        assert_eq!(doc.borrow().rows.len(), 2);
        assert_eq!(
            cx.update(gpui_kit::base::TextSelection::selected_text),
            expected
        );
        crate::test_shortcuts(cx, "cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            expected
        );
    }
    #[gpui::test]
    fn output_selection_copies_literal_rows_survives_append_and_clears_on_eviction(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut entity = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| View {
                rows: Arc::new(
                    render_styled(
                        "$ git status\n\x1b[1;3;31mCafé 🦀\x1b[0m\n\x1b[4;44m雪\x1b[0m done",
                    )
                    .unwrap(),
                ),
                cache: Cache::default(),
                document: None,
                scroll: UniformListScrollHandle::new(),
            });
            entity = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = entity.unwrap();
        cx.simulate_resize(size(px(600.), px(200.)));
        draw(cx);
        let doc = view.read_with(cx, |v, _| v.document.clone().unwrap());
        assert!(!doc.borrow().rows[1].spans.is_empty());
        assert!(!doc.borrow().rows[2].spans.is_empty());
        let at = |row, byte| {
            doc.borrow().selection.text_position(row, byte).unwrap() + point(px(0.1), px(5.))
        };
        let from = at(1, 0);
        let to = at(2, "雪 done".len());
        cx.simulate_mouse_down(from, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Default::default());
        draw(cx);
        crate::test_shortcuts(cx, "cmd-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            "Café 🦀\n雪 done"
        );
        view.update(cx, |v, cx| {
            Arc::make_mut(&mut v.rows).push("next command".into());
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            cx.update(gpui_kit::base::TextSelection::selected_text),
            "Café 🦀\n雪 done"
        );
        view.update(cx, |v, cx| {
            Arc::make_mut(&mut v.rows).remove(0);
            cx.notify();
        });
        draw(cx);
        assert_eq!(cx.update(gpui_kit::base::TextSelection::selected_text), "");
        view.update(cx, |v, cx| {
            v.cache.clear(cx);
            cx.notify();
        });
        draw(cx);
        let replacement = view.read_with(cx, |v, _| v.document.clone().unwrap());
        assert!(!Rc::ptr_eq(&doc, &replacement));
    }
}
