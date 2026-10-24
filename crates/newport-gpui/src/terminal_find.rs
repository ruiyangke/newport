//! Native terminal find controls; queries never enter the remote input queue.
use super::*;
use gpui_kit::component::{
    Disableable,
    button::ButtonVariants,
    input::{InputEvent, InputState},
};
use newport_terminal::SearchResults;
use std::time::Instant;

#[derive(Default)]
pub(super) struct FindDraft {
    query: String,
    match_case: bool,
}

pub(super) struct FindPanel {
    input: Entity<InputState>,
    _subscription: Subscription,
    match_case: bool,
    pub(super) results: SearchResults,
    pub(super) active: Option<usize>,
    searched_query: String,
    error: Option<String>,
    dirty: bool,
    last_refresh: Instant,
}

impl TerminalView {
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            let input = cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder("Find");
                input.set_value(self.find_draft.query.clone(), window, cx);
                input
            });
            let subscription = cx.subscribe_in(&input, window, |this, input, event, _, cx| {
                if !this.find.as_ref().is_some_and(|find| &find.input == input) {
                    return;
                }
                match event {
                    InputEvent::Change => {
                        if this.find.as_ref().is_some_and(|find| {
                            find.input.read(cx).value().as_str() != find.searched_query
                        }) {
                            this.refresh_find(true, true, cx);
                        }
                    }
                    InputEvent::PressEnter { shift, .. } => this.next_match(*shift, cx),
                    _ => {}
                }
            });
            self.find = Some(FindPanel {
                input,
                _subscription: subscription,
                match_case: self.find_draft.match_case,
                results: SearchResults::default(),
                active: None,
                searched_query: String::new(),
                error: None,
                dirty: false,
                last_refresh: Instant::now(),
            });
            // Reopening uses current output, not stale cell coordinates retained
            // while Find was hidden. Do not move the terminal's viewport yet.
            self.refresh_find(true, false, cx);
        }
        let input = self.find.as_ref().expect("initialized find").input.clone();
        input.update(cx, |input, cx| {
            input.select_all(window, cx);
            input.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    pub fn clear_find(&mut self, cx: &mut Context<Self>) {
        self.find_draft = FindDraft::default();
        if self.find.take().is_some() {
            cx.notify();
        }
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(find) = self.find.take() {
            self.find_draft = FindDraft {
                query: find.input.read(cx).value().to_string(),
                match_case: find.match_case,
            };
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn refresh_find(&mut self, reset: bool, reveal: bool, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        let previous = find
            .active
            .and_then(|index| find.results.matches.get(index))
            .cloned();
        find.searched_query = find.input.read(cx).value().to_string();
        match self.engine.search(&find.searched_query, find.match_case) {
            Ok(results) => {
                find.results = results;
                find.error = None;
            }
            Err(error) => {
                // Do not retain coordinates from a previous screen after a
                // failed refresh: they may now identify unrelated output.
                find.results = SearchResults::default();
                find.error = Some(format!("Couldn’t search terminal output. {error}"));
            }
        }
        find.active = if find.results.matches.is_empty() {
            None
        } else if reset {
            Some(0)
        } else {
            previous
                .and_then(|range| {
                    find.results
                        .matches
                        .iter()
                        .position(|candidate| candidate == &range)
                })
                .or(Some(
                    find.active.unwrap_or(0).min(find.results.matches.len() - 1),
                ))
        };
        find.dirty = false;
        find.last_refresh = Instant::now();
        if reveal {
            self.reveal_match();
        }
        cx.notify();
    }

    pub(super) fn refresh_find_output(&mut self, changed: bool, cx: &mut Context<Self>) {
        if let Some(find) = &mut self.find {
            find.dirty |= changed;
            if find.dirty && find.last_refresh.elapsed() >= Duration::from_millis(150) {
                self.refresh_find(false, false, cx);
            }
        }
    }

    fn reveal_match(&mut self) {
        if let Some(find) = &mut self.find
            && let Some(range) = find
                .active
                .and_then(|index| find.results.matches.get(index))
        {
            find.error = self
                .engine
                .reveal_position(*range.start())
                .err()
                .map(|error| format!("Couldn’t reveal the terminal match. {error}"));
        }
    }

    fn next_match(&mut self, previous: bool, cx: &mut Context<Self>) {
        if self.find.as_ref().is_some_and(|find| find.dirty) {
            self.refresh_find(false, false, cx);
        }
        if let Some(find) = &mut self.find {
            let count = find.results.matches.len();
            if count != 0 {
                let current = find.active.unwrap_or(0);
                find.active = Some(if previous {
                    (current + count - 1) % count
                } else {
                    (current + 1) % count
                });
                self.reveal_match();
                cx.notify();
            }
        }
    }

    pub(super) fn find_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(find) = &self.find else {
            return div().into_any_element();
        };
        let query_empty = find.input.read(cx).value().is_empty();
        let count = if find.error.is_some() {
            "Search unavailable".into()
        } else if query_empty {
            String::new()
        } else if let Some(active) = find.active {
            format!(
                "{}/{}{}",
                active + 1,
                find.results.matches.len(),
                if find.results.truncated { "+" } else { "" }
            )
        } else {
            "No results".into()
        };
        let disabled = find.results.matches.is_empty();
        div()
            .id("terminal-find")
            .role(Role::Search)
            .aria_label("Find in terminal")
            .absolute()
            .top(px(8.))
            .right(px(12.))
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(crate::ui::RADIUS))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                    this.close_find(window, cx)
                }),
            )
            .child(
                crate::controls::search_input(&find.input, cx)
                    .w(px(180.))
                    .aria_label("Find in terminal"),
            )
            .child(
                div()
                    .id("terminal-find-count")
                    .role(Role::Status)
                    .aria_label(find.error.clone().unwrap_or_else(|| count.clone()))
                    .min_w(px(52.))
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_color(cx.theme().muted_foreground)
                    .child(count),
            )
            .child(
                crate::controls::icon_button("terminal-find-previous")
                    .icon(gpui_kit::assets::IconName::ChevronUp)
                    .ghost()
                    .disabled(disabled)
                    .accessibility_label("Previous match")
                    .tooltip("Previous match")
                    .on_click(cx.listener(|this, _, _, cx| this.next_match(true, cx))),
            )
            .child(
                crate::controls::icon_button("terminal-find-next")
                    .icon(gpui_kit::assets::IconName::ChevronDown)
                    .ghost()
                    .disabled(disabled)
                    .accessibility_label("Next match")
                    .tooltip("Next match")
                    .on_click(cx.listener(|this, _, _, cx| this.next_match(false, cx))),
            )
            .child(
                crate::controls::icon_button("terminal-find-case")
                    .icon(gpui_kit::assets::IconName::CaseSensitive)
                    .ghost()
                    .toggled(find.match_case)
                    .accessibility_label("Match case")
                    .tooltip("Match case")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(find) = &mut this.find {
                            find.match_case = !find.match_case;
                        }
                        this.refresh_find(true, true, cx);
                    })),
            )
            .child(
                crate::controls::icon_button("terminal-find-close")
                    .icon(gpui_kit::assets::IconName::X)
                    .ghost()
                    .accessibility_label("Close search")
                    .tooltip("Close search")
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{FindDraft, TerminalView};
    use gpui_kit::{self as gpui, AppContext, Entity, TestAppContext, VisualTestContext, px, size};

    fn draw(cx: &mut VisualTestContext) {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    fn fixture(cx: &mut TestAppContext) -> (Entity<TerminalView>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
            crate::navigation::install_workspace_shortcuts(cx);
        });
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|cx| {
                let mut view = TerminalView::new("find-fixture-no-transport".into(), cx);
                view.close();
                view.engine.advance(b"Needle needle Needle\r\n").unwrap();
                view
            });
            terminal.read(cx).focus.clone().focus(window, cx);
            view = Some(terminal.clone());
            gpui::base::Root::new(terminal, window, cx)
        });
        cx.simulate_resize(size(px(600.), px(300.)));
        draw(cx);
        (view.unwrap(), cx)
    }

    #[gpui::test]
    fn closing_find_retains_draft_and_reopening_searches_current_output_without_remote_input(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = fixture(cx);
        cx.simulate_keystrokes("cmd-f ctrl-f");
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.find.is_none()));
        crate::test_shortcuts(cx, "cmd-shift-f");
        draw(cx);
        cx.simulate_input("Needle");
        draw(cx);
        view.update(cx, |view, cx| {
            assert_eq!(view.find.as_ref().unwrap().results.matches.len(), 3);
            view.find.as_mut().unwrap().match_case = true;
            view.refresh_find(true, false, cx);
            assert_eq!(view.find.as_ref().unwrap().results.matches.len(), 2);
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.find.as_ref().unwrap().active),
            Some(1)
        );
        cx.simulate_keystrokes("shift-enter");
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.find.as_ref().unwrap().active),
            Some(0)
        );
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, _| {
                assert!(view.find.is_none());
                assert_eq!(view.find_draft.query, "Needle");
                assert!(view.find_draft.match_case);
                assert!(view.focus.is_focused(window));
                view.engine.advance(b"Needle\r\n").unwrap();
            })
        });
        crate::test_shortcuts(cx, "cmd-shift-f");
        draw(cx);
        view.read_with(cx, |view, cx| {
            let find = view.find.as_ref().unwrap();
            assert_eq!(find.input.read(cx).value().as_str(), "Needle");
            assert!(find.match_case);
            assert_eq!(
                find.results.matches.len(),
                3,
                "reopening includes output received while hidden"
            );
            assert!(
                view.input.is_empty(),
                "Find typing and Enter never enter the SSH queue"
            );
            assert!(view.session.is_none());
        });
        cx.simulate_input("replacement");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.find.as_ref().unwrap().input.read(cx).value().as_str(),
                "replacement",
                "the retained query is selected on reopening"
            );
        });
    }

    #[gpui::test]
    fn retained_find_draft_cannot_cross_navigation_or_connection_scope(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx);
        for boundary in 0..3 {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.find_draft = FindDraft {
                        query: "Needle".into(),
                        match_case: true,
                    };
                    match boundary {
                        0 => view.deactivate(window, cx),
                        1 => view.reset_connection(cx),
                        _ => view.reconnect(cx),
                    }
                    view.close();
                    assert!(view.find.is_none());
                    assert!(view.find_draft.query.is_empty());
                    view.open_find(window, cx);
                    assert!(
                        view.find
                            .as_ref()
                            .unwrap()
                            .input
                            .read(cx)
                            .value()
                            .is_empty()
                    );
                    assert!(view.find.as_ref().unwrap().results.matches.is_empty());
                    assert!(view.input.is_empty() && view.session.is_none());
                    view.close_find(window, cx);
                })
            });
        }
    }
}
