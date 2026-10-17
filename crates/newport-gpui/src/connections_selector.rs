//! Controlled, exclusive Connections selectors. Selection belongs to the page;
//! this presentation component owns only roving keyboard focus.
use gpui_kit::assets::IconName;
use gpui_kit::base::{Radio, RadioGroup};
use gpui_kit::component::{ActiveTheme, Icon, ThemeStyled, tooltip::Tooltip};
use gpui_kit::{prelude::*, *};
use std::{collections::HashMap, rc::Rc};

pub(super) struct Choice {
    pub value: usize,
    pub label: &'static str,
    pub icon: Option<IconName>,
    pub tooltip: Option<&'static str>,
    pub danger: bool,
    pub disabled: bool,
}

impl Choice {
    pub(super) fn text(value: usize, label: &'static str) -> Self {
        Self {
            value,
            label,
            icon: None,
            tooltip: None,
            danger: false,
            disabled: false,
        }
    }
}

type SelectHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub(super) struct Selector {
    id: SharedString,
    label: &'static str,
    selected: usize,
    choices: Vec<Choice>,
    on_select: SelectHandler,
}

impl Selector {
    pub(super) fn new(
        id: impl Into<SharedString>,
        label: &'static str,
        selected: usize,
        choices: Vec<Choice>,
        on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label,
            selected,
            choices,
            on_select: Rc::new(on_select),
        }
    }
}

struct Slot {
    focus: FocusHandle,
    _subscription: Subscription,
}

struct FocusState {
    slots: HashMap<usize, Slot>,
    focused: Option<usize>,
    selected: usize,
}

impl FocusState {
    fn sync(
        &mut self,
        choices: &[Choice],
        selected: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<FocusHandle> {
        let enabled = |value| {
            choices
                .iter()
                .any(|choice| choice.value == value && !choice.disabled)
        };
        let displaced = self
            .slots
            .iter()
            .any(|(value, slot)| slot.focus.is_focused(window) && !enabled(*value));
        self.slots
            .retain(|value, _| choices.iter().any(|choice| choice.value == *value));
        for choice in choices {
            self.slots.entry(choice.value).or_insert_with(|| {
                let value = choice.value;
                let focus = cx.focus_handle();
                let subscription = cx.on_focus_in(&focus, window, move |state, _, cx| {
                    state.focused = Some(value);
                    cx.notify();
                });
                Slot {
                    focus,
                    _subscription: subscription,
                }
            });
        }
        if self.selected != selected || self.focused.is_none_or(|value| !enabled(value)) {
            self.focused = choices
                .iter()
                .find(|choice| choice.value == selected && !choice.disabled)
                .or_else(|| choices.iter().find(|choice| !choice.disabled))
                .map(|choice| choice.value);
        }
        self.selected = selected;
        // Repair only focus that belonged to a vanished or disabled choice.
        // Snapshot refreshes must not pull focus out of the filter or another control.
        displaced
            .then(|| {
                self.focused
                    .and_then(|value| self.slots.get(&value))
                    .map(|slot| slot.focus.clone())
            })
            .flatten()
    }

    fn navigate(
        &mut self,
        keys: &[usize],
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.modified() || keys.is_empty() {
            return;
        }
        let current = keys
            .iter()
            .position(|value| self.slots[value].focus.is_focused(window));
        let Some(current) = current else { return };
        let next = match event.keystroke.key.as_str() {
            "left" => (current + keys.len() - 1) % keys.len(),
            "right" => (current + 1) % keys.len(),
            "home" | "pageup" => 0,
            "end" | "pagedown" => keys.len() - 1,
            _ => return,
        };
        self.focused = Some(keys[next]);
        self.slots[&keys[next]].focus.focus(window, cx);
        // Radix's single ToggleGroup moves focus without changing the value.
        // Space, Enter, or a pointer click commits the focused radio instead.
        cx.stop_propagation();
        cx.notify();
    }
}

impl Selector {
    fn group(self, window: &mut Window, cx: &mut App) -> RadioGroup {
        let state = window.use_keyed_state(
            SharedString::from(format!("{}-focus", self.id)),
            cx,
            |_, _| FocusState {
                slots: HashMap::new(),
                focused: Some(self.selected),
                selected: self.selected,
            },
        );
        let fallback = state.update(cx, |state, cx| {
            state.sync(&self.choices, self.selected, window, cx)
        });
        if let Some(fallback) = fallback {
            fallback.focus(window, cx);
        }
        let enabled = self
            .choices
            .iter()
            .filter(|choice| !choice.disabled)
            .map(|choice| choice.value)
            .collect::<Vec<_>>();
        let focused = state.read(cx).focused;
        let navigation = state.clone();
        let count = self.choices.len();
        let selected_background: Hsla = cx.theme().tokens.background.into();
        RadioGroup::new(self.id.clone())
            .axis(Axis::Horizontal)
            .aria_label(self.label)
            .flex()
            .items_center()
            .flex_shrink_0()
            .h(px(crate::controls::CONTROL_HEIGHT))
            .p(px(3.))
            .gap(px(2.))
            .rounded(cx.theme().radius)
            .bg(cx.theme().tokens.tab_bar_segmented)
            .on_key_down(move |event, window, cx| {
                navigation.update(cx, |state, cx| state.navigate(&enabled, event, window, cx));
            })
            .children(self.choices.into_iter().enumerate().map(|(index, choice)| {
                let focus = state.read(cx).slots[&choice.value].focus.clone();
                let state = state.clone();
                let on_select = self.on_select.clone();
                let checked = self.selected == choice.value;
                let icon_only = choice.icon.is_some();
                Radio::new(SharedString::from(format!("{}-{}", self.id, choice.value)))
                    .accessibility_label(choice.label)
                    .checked(checked)
                    .disabled(choice.disabled)
                    .set_position(index + 1, count)
                    .track_focus(&focus)
                    .tab_stop(focused == Some(choice.value))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .cursor_default()
                    .h(px(crate::controls::CONTROL_HEIGHT - 6.))
                    .min_h(px(crate::controls::CONTROL_HEIGHT - 6.))
                    .max_h(px(crate::controls::CONTROL_HEIGHT - 6.))
                    .px(px(9.))
                    .when(icon_only, |item| {
                        item.w(px(crate::controls::CONTROL_HEIGHT))
                            .min_w(px(crate::controls::CONTROL_HEIGHT))
                            .max_w(px(crate::controls::CONTROL_HEIGHT))
                            .px_0()
                    })
                    .text_size(px(crate::controls::FONT_SIZE))
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(relative(1.25))
                    .rounded(px(5.))
                    .when(checked, |item| item.shadow_sm())
                    .bg(if checked {
                        selected_background
                    } else {
                        cx.theme().transparent
                    })
                    .text_color(if choice.danger {
                        crate::theme::error(cx.theme().is_dark())
                    } else {
                        if checked {
                            cx.theme().tab_active_foreground
                        } else {
                            cx.theme().muted_foreground
                        }
                    })
                    .when(choice.disabled, |item| item.opacity(0.5))
                    .when(!choice.disabled, |item| {
                        item.hover(|style| style.text_color(cx.theme().foreground))
                    })
                    .when(focus.is_focused(window) && !choice.disabled, |item| {
                        item.focus_ring_style(window, cx)
                    })
                    .when_some(choice.tooltip, |item, tooltip| {
                        item.tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
                    })
                    .when_some(choice.icon, |item, icon| {
                        item.child(Icon::new(icon).size(px(crate::controls::ICON_SIZE)))
                    })
                    .when(!icon_only, |item| item.child(choice.label))
                    .on_change(move |_, _, window, cx| {
                        state.update(cx, |state, cx| {
                            state.focused = Some(choice.value);
                            cx.notify();
                        });
                        on_select(choice.value, window, cx);
                    })
            }))
    }
}

impl RenderOnce for Selector {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.group(window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::{Choice, Selector};
    use gpui::{
        App, Context, Element, FocusHandle, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke,
        Render, RenderOnce, Role, TestAppContext, VisualTestContext, Window, accesskit, div,
        prelude::*, px,
    };
    use gpui_kit as gpui;
    use gpui_kit::base::{
        Button,
        test_support::{self, ElementSnapshot},
    };
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    type Nodes = Rc<RefCell<HashMap<&'static str, accesskit::Node>>>;

    #[derive(IntoElement)]
    struct GroupProbe {
        selector: Selector,
        name: &'static str,
        nodes: Nodes,
    }

    impl RenderOnce for GroupProbe {
        fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
            // Inspect the same native group element that this probe paints.
            let element = self
                .selector
                .group(window, cx)
                .render(window, cx)
                .into_element();
            let mut node = accesskit::Node::new(element.a11y_role().unwrap());
            element.write_a11y_info(&mut node);
            self.nodes.borrow_mut().insert(self.name, node);
            element
        }
    }

    struct Harness {
        before: FocusHandle,
        after: FocusHandle,
        values: [[usize; 2]; 2],
        server: usize,
        attention: bool,
        disabled_forwarded: bool,
        changes: usize,
        nodes: Nodes,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let owner = cx.weak_entity();
            let server = self.server;
            let show = Selector::new(
                format!("show-{server}"),
                "Show",
                self.values[server][0],
                ["All", "Saved", "Needs attention"]
                    .into_iter()
                    .enumerate()
                    .filter(|(index, _)| *index != 2 || self.attention)
                    .map(|(value, label)| Choice {
                        disabled: value == 1 && self.disabled_forwarded,
                        ..Choice::text(value, label)
                    })
                    .collect(),
                move |value, _, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        if view.server != server {
                            return;
                        }
                        view.values[server][0] = value;
                        view.changes += 1;
                        cx.notify();
                    });
                },
            );
            let owner = cx.weak_entity();
            let view = Selector::new(
                format!("view-{server}"),
                "View",
                self.values[server][1],
                vec![
                    Choice {
                        icon: Some(super::IconName::ListTree),
                        ..Choice::text(0, "Projects view")
                    },
                    Choice {
                        icon: Some(super::IconName::Table2),
                        ..Choice::text(1, "Ports view")
                    },
                ],
                move |value, _, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        if view.server != server {
                            return;
                        }
                        view.values[server][1] = value;
                        view.changes += 1;
                        cx.notify();
                    });
                },
            );
            div()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(8.))
                .child(
                    Button::new("before")
                        .track_focus(&self.before)
                        .child("Before"),
                )
                .child(GroupProbe {
                    selector: show,
                    name: "Show",
                    nodes: self.nodes.clone(),
                })
                .child(GroupProbe {
                    selector: view,
                    name: "View",
                    nodes: self.nodes.clone(),
                })
                .child(Button::new("after").track_focus(&self.after).child("After"))
        }
    }

    fn radio(cx: &mut VisualTestContext, id: &str) -> ElementSnapshot {
        cx.update(|window, app| {
            window.draw(app).clear(app);
            test_support::find(window, &[], &id.to_owned().into()).expect("painted radio")
        })
    }

    fn key(cx: &mut VisualTestContext, key: &str) {
        let keystroke = Keystroke::parse(key).unwrap();
        cx.simulate_event(KeyDownEvent {
            physical_key: None,
            key_metadata: None,
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent {
            physical_key: None,
            key_metadata: None,
            keystroke,
        });
        cx.update(|window, app| window.draw(app).clear(app));
    }

    fn harness(cx: &mut TestAppContext) -> (gpui::Entity<Harness>, &mut VisualTestContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|_, cx| Harness {
            before: cx.focus_handle(),
            after: cx.focus_handle(),
            values: [[0, 0], [1, 1]],
            server: 0,
            attention: true,
            disabled_forwarded: false,
            changes: 0,
            nodes: Rc::default(),
        });
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear(cx);
        });
        (view, cx)
    }

    #[gpui::test]
    fn named_radio_groups_have_one_tab_stop_and_exclusive_controlled_selection(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = harness(cx);
        view.read_with(cx, |view, _| {
            for name in ["Show", "View"] {
                let nodes = view.nodes.borrow();
                let node = &nodes[name];
                assert_eq!(node.role(), Role::RadioGroup);
                assert_eq!(node.label(), Some(name));
                assert_eq!(node.orientation(), Some(accesskit::Orientation::Horizontal));
            }
        });
        for (id, label, checked) in [
            ("show-0-0", "All", true),
            ("show-0-1", "Saved", false),
            ("show-0-2", "Needs attention", false),
            ("view-0-0", "Projects view", true),
            ("view-0-1", "Ports view", false),
        ] {
            let node = radio(cx, id);
            assert_eq!(node.role(), Some(Role::RadioButton));
            assert_eq!(node.label(), Some(label));
            assert_eq!(node.checked(), Some(checked));
            assert_eq!(node.selected(), Some(checked));
            assert_eq!(
                node.bounds().size.height,
                px(crate::controls::CONTROL_HEIGHT - 6.),
                "the selected segment fits inside the 28px toolbar frame with 3px inset"
            );
            if id.starts_with("view-") {
                assert_eq!(
                    node.bounds().size.width,
                    px(crate::controls::CONTROL_HEIGHT),
                    "icon choices match the square toolbar actions"
                );
            }
        }
        cx.update(|window, cx| {
            window.focus_next(cx);
            assert!(view.read(cx).before.is_focused(window));
            window.focus_next(cx);
        });
        assert_eq!(radio(cx, "show-0-0").focused(), Some(true));
        cx.update(|window, cx| window.focus_next(cx));
        assert_eq!(radio(cx, "view-0-0").focused(), Some(true));
        cx.update(|window, cx| {
            window.focus_next(cx);
            assert!(view.read(cx).after.is_focused(window));
            window.focus_prev(cx);
        });
        assert_eq!(radio(cx, "view-0-0").focused(), Some(true));
        key(cx, "right");
        assert_eq!(radio(cx, "view-0-1").focused(), Some(true));
        assert_eq!(
            radio(cx, "view-0-0").checked(),
            Some(true),
            "arrows move focus only"
        );
        key(cx, "space");
        assert_eq!(radio(cx, "view-0-1").checked(), Some(true));
        assert_eq!(radio(cx, "view-0-0").checked(), Some(false));
        key(cx, "enter");
        assert_eq!(
            view.read_with(cx, |view, _| view.changes),
            1,
            "selected radio cannot deselect"
        );
        cx.update(|window, cx| window.focus_prev(cx));
        for (key_name, expected) in [("left", 2), ("right", 0), ("end", 2), ("home", 0)] {
            key(cx, key_name);
            assert_eq!(
                radio(cx, &format!("show-0-{expected}")).focused(),
                Some(true)
            );
            assert_eq!(radio(cx, "show-0-0").checked(), Some(true));
        }
        key(cx, "right");
        key(cx, "enter");
        assert_eq!(radio(cx, "show-0-1").checked(), Some(true));
        assert_eq!(radio(cx, "show-0-0").checked(), Some(false));
        let target = radio(cx, "show-0-2").bounds().center();
        cx.simulate_click(target, Default::default());
        assert_eq!(radio(cx, "show-0-2").checked(), Some(true));
        assert_eq!(radio(cx, "show-0-1").checked(), Some(false));
        cx.simulate_click(target, Default::default());
        assert_eq!(view.read_with(cx, |view, _| view.changes), 3);
    }

    #[gpui::test]
    fn disabled_and_removed_choices_do_not_trap_focus_or_leak_selection_between_servers(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = harness(cx);
        view.update(cx, |view, cx| {
            view.disabled_forwarded = true;
            cx.notify();
        });
        radio(cx, "show-0-0");
        cx.update(|window, cx| {
            window.focus_next(cx);
            window.focus_next(cx);
        });
        key(cx, "right");
        assert_eq!(
            radio(cx, "show-0-2").focused(),
            Some(true),
            "skip disabled choice"
        );
        let disabled = radio(cx, "show-0-1").bounds().center();
        cx.simulate_click(disabled, Default::default());
        assert_eq!(view.read_with(cx, |view, _| view.changes), 0);
        assert_eq!(radio(cx, "show-0-2").focused(), Some(true));
        view.update(cx, |view, cx| {
            view.attention = false;
            cx.notify();
        });
        assert_eq!(
            radio(cx, "show-0-0").focused(),
            Some(true),
            "removing the focused option moves actual focus to the enabled fallback"
        );
        key(cx, "right");
        assert_eq!(radio(cx, "show-0-0").focused(), Some(true));
        cx.update(|window, cx| window.focus_next(cx));
        assert_eq!(
            radio(cx, "view-0-0").focused(),
            Some(true),
            "the repaired group retains one Tab stop"
        );
        cx.update(|window, cx| window.focus_prev(cx));
        view.update(cx, |view, cx| {
            view.disabled_forwarded = false;
            cx.notify();
        });
        radio(cx, "show-0-0");
        key(cx, "right");
        assert_eq!(radio(cx, "show-0-1").focused(), Some(true));
        view.update(cx, |view, cx| {
            view.disabled_forwarded = true;
            cx.notify();
        });
        assert_eq!(
            radio(cx, "show-0-0").focused(),
            Some(true),
            "disabling the focused option moves focus to fallback"
        );
        cx.update(|window, cx| view.read(cx).before.clone().focus(window, cx));
        view.update(cx, |view, cx| {
            view.disabled_forwarded = false;
            view.attention = true;
            view.values[0][0] = 1;
            cx.notify();
        });
        assert_eq!(radio(cx, "show-0-1").checked(), Some(true));
        cx.update(|window, cx| {
            assert!(
                view.read(cx).before.is_focused(window),
                "controlled snapshot changes must not steal outside focus"
            )
        });
        view.update(cx, |view, cx| {
            view.server = 1;
            cx.notify();
        });
        assert_eq!(radio(cx, "show-1-1").checked(), Some(true));
        assert_eq!(radio(cx, "view-1-1").checked(), Some(true));
        view.update(cx, |view, cx| {
            view.server = 0;
            cx.notify();
        });
        assert_eq!(radio(cx, "show-0-1").checked(), Some(true));
        assert_eq!(radio(cx, "view-0-0").checked(), Some(true));
    }
}
