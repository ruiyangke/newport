//! Shared literal log-row presentation; collection, filtering and polling stay
//! with each screen's retained log view and client.
use crate::hover::HoverExt as _;
use crate::selectable_log::SelectableLog;
use gpui_kit::component::ActiveTheme;
use gpui_kit::{prelude::*, *};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Normal,
    Warning,
    Error,
}

/// `source` is an already formatted prefix, including its trailing separator.
/// Register `"{time} {source}{message}"` as this row's selectable document text
/// when `time` is present, otherwise just `"{source}{message}"`.
pub(crate) struct LogLine<'a> {
    pub(crate) at: Option<i64>,
    pub(crate) time: Option<&'a str>,
    pub(crate) source: &'a str,
    pub(crate) message: &'a str,
    pub(crate) tone: Tone,
}

pub(crate) fn render(
    prefix: &'static str,
    index: usize,
    line: LogLine<'_>,
    query: &str,
    selection: &SelectableLog,
    cx: &App,
) -> AnyElement {
    let text = format!("{}{}", line.source, line.message);
    let message_start = line.time.map_or(0, |time| time.len() + 1);
    let mut highlights = vec![(
        0..line.source.len(),
        HighlightStyle {
            color: Some(cx.theme().muted_foreground),
            ..Default::default()
        },
    )];
    highlights.extend(matches(&text, query).into_iter().map(|range| {
        (
            range,
            HighlightStyle {
                background_color: Some(cx.theme().primary.opacity(0.16)),
                ..Default::default()
            },
        )
    }));
    let accent = match line.tone {
        Tone::Normal => None,
        Tone::Warning => Some(crate::theme::warning(cx.theme().is_dark())),
        Tone::Error => Some(crate::theme::error(cx.theme().is_dark())),
    };
    div()
        .id((SharedString::from(format!("{prefix}-line")), index))
        .role(accesskit::Role::Label)
        .aria_label(
            line.time
                .map_or_else(|| text.clone(), |time| format!("{time} {text}")),
        )
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .gap(px(12.))
        .border_l(px(2.))
        .border_color(accent.unwrap_or(cx.theme().transparent))
        .bg(accent
            .map(|color| color.opacity(0.06))
            .unwrap_or(cx.theme().transparent))
        .pl(px(8.))
        .py(px(1.))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(11.5))
        .line_height(relative(1.6))
        .when_some(line.time, |v, time| {
            v.child(
                div()
                    .id((SharedString::from(format!("{prefix}-time")), index))
                    .min_w(px(64.))
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .when_some(line.at, |v, at| {
                        v.hover_hint(crate::localized_time::full_date(at))
                    })
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        selection
                            .text(index, 0..time.len())
                            .selection_color(cx.theme().primary.opacity(0.22)),
                    ),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(accent.unwrap_or(cx.theme().foreground))
                .child(
                    selection
                        .text(index, message_start..message_start + text.len())
                        .with_highlights(highlights)
                        .selection_color(cx.theme().primary.opacity(0.22)),
                ),
        )
        .into_any_element()
}

/// Shared visual strip; the caller owns heading semantics and sticky geometry.
pub(crate) fn day_strip(day: &str, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .py(px(2.))
        .font_family(crate::theme::UI_FONT)
        .text_size(px(10.5))
        .line_height(relative(1.6))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .bg(rgb(if cx.theme().mode.is_dark() {
            0x2a292d
        } else {
            0xf5f4f6
        }))
        .child(day.to_uppercase())
        .child(div().h(px(1.)).flex_1().bg(cx.theme().border))
}

/// Create fresh geometry for each rendered virtual list. Mounted rows fill it
/// during prepaint, so follow-tail offsets need no special interpretation.
pub(crate) type RowBounds = Rc<RefCell<Vec<(usize, Bounds<Pixels>)>>>;

pub(crate) fn track_row(
    index: usize,
    content: impl IntoElement,
    geometry: RowBounds,
) -> AnyElement {
    div()
        .relative()
        .w_full()
        .child(content)
        .child(
            canvas(
                move |bounds, _, _| {
                    geometry.borrow_mut().push((index, bounds));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .into_any_element()
}

/// Repeat only the date scrolled above the viewport. `days` contains separator
/// indices and labels in row order. This decorative paint layer owns no hitbox
/// or focus; the original separator keeps its accessibility name in the list.
pub(crate) fn sticky_day_context(
    id: &'static str,
    days: Rc<Vec<(usize, String)>>,
    geometry: RowBounds,
    dimmed: bool,
) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            let geometry = geometry.borrow();
            let first = geometry
                .iter()
                .filter(|(_, row)| row.bottom() > bounds.top())
                .min_by_key(|(index, _)| *index)?
                .0;
            let (index, day) = days.iter().rev().find(|(index, _)| *index <= first)?;
            if geometry
                .iter()
                .any(|(row, rect)| row == index && rect.top() >= bounds.top())
            {
                return None;
            }
            let day_selector = format!("{id}-{day}");
            let mut header = div()
                .id(id)
                .debug_selector(move || id.into())
                .child(day_strip(day, cx).debug_selector(move || day_selector.clone()))
                .when(dimmed, |header| header.opacity(0.55))
                .into_any_element();
            let size = header.layout_as_root(
                size(
                    AvailableSpace::Definite(bounds.size.width),
                    AvailableSpace::MinContent,
                ),
                window,
                cx,
            );
            let next = geometry
                .iter()
                .filter(|(row, _)| {
                    row > index && days.binary_search_by_key(row, |(day, _)| *day).is_ok()
                })
                .min_by_key(|(row, _)| *row)
                .map(|(_, rect)| rect.top());
            let top = next
                .map(|next| (next - size.height).min(bounds.top()))
                .unwrap_or(bounds.top());
            header.prepaint_at(point(bounds.left(), top), window, cx);
            Some(header)
        },
        |_, header, window, cx| {
            if let Some(mut header) = header {
                header.paint(window, cx);
            }
        },
    )
    .absolute()
    .inset_0()
}

/// Map case-insensitive matches back to original UTF-8 boundaries, including
/// characters whose lowercase representation expands to more than one scalar.
fn matches(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return vec![];
    }
    let mut result = vec![];
    let mut next = 0;
    for (start, _) in text.char_indices() {
        if start < next {
            continue;
        }
        let mut candidate = String::new();
        for (offset, ch) in text[start..].char_indices() {
            candidate.extend(ch.to_lowercase());
            if candidate == needle {
                let end = start + offset + ch.len_utf8();
                result.push(start..end);
                next = end;
                break;
            }
            if !needle.starts_with(&candidate) {
                break;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::matches;
    use super::{LogLine, SelectableLog, Tone};
    use gpui::{
        Context, FocusHandle, IntoElement, ListAlignment, ListState, Modifiers, MouseButton,
        Render, TestAppContext, Window, div, list, point, prelude::*, px, size,
    };
    use gpui_kit as gpui;

    #[test]
    fn case_insensitive_highlights_respect_original_utf8_boundaries() {
        assert_eq!(
            matches("İstanbul UTF-8 Café CAFÉ", "café"),
            vec![16..21, 22..27]
        );
        assert_eq!(matches("İstanbul", "i\u{307}"), vec![0..2]);
        assert!(matches("abc", "").is_empty());
    }

    #[gpui::test]
    fn command_rows_omit_timestamp_space_and_copy_the_literal_document(cx: &mut TestAppContext) {
        const SOURCE: &str = "api: ";
        const MESSAGE: &str = "Café **literal** 🦀";
        const TIME: &str = "12:00:00";
        struct Fixture {
            selection: SelectableLog,
            list: ListState,
            focus: FocusHandle,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div().size_full().track_focus(&self.focus).child(
                    self.selection.viewport(
                        list(
                            self.list.clone(),
                            cx.processor(|this, index, _, cx| {
                                super::render(
                                    "test-log",
                                    index,
                                    LogLine {
                                        at: None,
                                        time: (index == 1).then_some(TIME),
                                        source: SOURCE,
                                        message: MESSAGE,
                                        tone: Tone::Normal,
                                    },
                                    "café",
                                    &this.selection,
                                    cx,
                                )
                            }),
                        )
                        .size_full(),
                    ),
                )
            }
        }
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let literal = format!("{SOURCE}{MESSAGE}");
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(|cx| {
                let focus = cx.focus_handle();
                let mut selection = SelectableLog::new(focus.clone(), window, cx);
                selection.set_rows(
                    vec![Some(literal.clone()), Some(format!("{TIME} {literal}"))],
                    cx,
                );
                Fixture {
                    selection,
                    list: ListState::new(2, ListAlignment::Top, px(0.)),
                    focus,
                }
            });
            view = Some(content.clone());
            gpui::base::Root::new(content, window, cx)
        });
        let view = view.unwrap();
        cx.simulate_resize(size(px(420.), px(160.)));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let (from, to, timestamp, timed_message) = view.read_with(cx, |view, _| {
            (
                view.selection.text_position(0, 0).unwrap(),
                view.selection.text_position(0, literal.len()).unwrap(),
                view.selection.text_position(1, 0).unwrap(),
                view.selection.text_position(1, TIME.len() + 1).unwrap(),
            )
        });
        assert_eq!(from.x, timestamp.x, "no blank timestamp column");
        assert!(timed_message.x - from.x >= px(76.));
        let offset = point(px(0.1), px(5.));
        cx.simulate_mouse_down(from + offset, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(to + offset, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(to + offset, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(cx.update(gpui::base::TextSelection::selected_text), literal);
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        });
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some(literal),
        );
    }
}
