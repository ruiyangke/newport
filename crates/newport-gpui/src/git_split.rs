//! Named keyboard-operable Git dividers, retaining Kit's pointer resize handle.
use crate::git_layout::{LIST_WIDTH, MAX_LIST_WIDTH, MIN_LIST_WIDTH};
use gpui::base::ElementExt;
use gpui::component::{
    ActiveTheme,
    resizable::{ResizablePanel, ResizableState, h_resizable, resize_handle_appearance},
};
use gpui::{prelude::*, *};
use gpui_kit as gpui;
use std::rc::Rc;

struct Presentation {
    layout: Entity<ResizableState>,
    focus: FocusHandle,
    preferred: Pixels,
    container: Pixels,
}

#[derive(IntoElement)]
pub(super) struct Group {
    id: &'static str,
    label: &'static str,
    detail_min: f32,
    panels: Vec<ResizablePanel>,
}

pub(super) fn group(id: &'static str, label: &'static str, detail_min: f32) -> Group {
    Group {
        id,
        label,
        detail_min,
        panels: Vec::new(),
    }
}
impl Group {
    pub(super) fn child(mut self, panel: ResizablePanel) -> Self {
        self.panels.push(panel);
        self
    }
}

fn resize(presentation: &Entity<Presentation>, target: Pixels, window: &mut Window, cx: &mut App) {
    let layout = presentation.read(cx).layout.clone();
    layout.update(cx, |state, cx| state.resize_panel(0, target, window, cx));
    if let Some(width) = layout.read(cx).sizes().first().copied() {
        presentation.update(cx, |state, cx| {
            state.preferred = width;
            cx.notify();
        });
    }
}

impl RenderOnce for Group {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id;
        let label = self.label;
        let detail_min = self.detail_min;
        let paired = self.panels.len() == 2;
        let state =
            window.use_keyed_state(format!("{id}-presentation"), cx, |_, cx| Presentation {
                layout: cx.new(|_| ResizableState::default()),
                focus: cx.focus_handle(),
                preferred: px(LIST_WIDTH),
                container: px(0.),
            });
        let layout = state.read(cx).layout.clone();
        let focus = state.read(cx).focus.clone();
        let appearance = resize_handle_appearance();
        let handle_state = state.clone();
        let dragged = state.clone();
        let group = h_resizable(id)
            .with_state(&layout)
            .children(self.panels)
            .on_resize(move |layout, _, cx| {
                if !paired {
                    return;
                }
                if let Some(width) = layout.read(cx).sizes().first().copied() {
                    dragged.update(cx, |state, cx| {
                        state.preferred = width;
                        cx.notify();
                    });
                }
            })
            .with_handle_appearance(Rc::new(move |handle, window, cx| {
                let layout = handle_state.read(cx).layout.clone();
                let sizes = layout.read(cx).sizes();
                let width = sizes.first().copied().unwrap_or(px(LIST_WIDTH));
                let total = sizes.iter().map(|size| f32::from(*size)).sum::<f32>();
                let maximum = MAX_LIST_WIDTH.min((total - detail_min).max(MIN_LIST_WIDTH));
                let keys = handle_state.clone();
                let clicks = handle_state.clone();
                let click_focus = focus.clone();
                Some(
                    div()
                        .relative()
                        .size_full()
                        .children(appearance(handle, window, cx))
                        .child(
                            div()
                                .id("git-pane-width")
                                .debug_selector(move || format!("{id}-handle"))
                                .absolute()
                                .left(px(-4.))
                                .top_0()
                                .bottom_0()
                                .w(px(9.))
                                .role(accesskit::Role::Splitter)
                                .aria_label(label)
                                .aria_orientation(accesskit::Orientation::Vertical)
                                .aria_value(format!("{width:.0}"))
                                .track_focus(&focus)
                                .tab_stop(true)
                                .focus_visible(|style| style.bg(cx.theme().primary.opacity(0.25)))
                                .on_click(move |event, window, cx| {
                                    click_focus.focus(window, cx);
                                    if event.click_count() == 2 {
                                        resize(&clicks, px(LIST_WIDTH), window, cx);
                                    }
                                })
                                .on_key_down(move |event, window, cx| {
                                    if event.keystroke.modifiers.platform
                                        || event.keystroke.modifiers.control
                                        || event.keystroke.modifiers.alt
                                    {
                                        return;
                                    }
                                    let current = keys
                                        .read(cx)
                                        .layout
                                        .read(cx)
                                        .sizes()
                                        .first()
                                        .copied()
                                        .unwrap_or(px(LIST_WIDTH));
                                    let step = if event.keystroke.modifiers.shift {
                                        30.
                                    } else {
                                        10.
                                    };
                                    let target = match event.keystroke.key.as_str() {
                                        "left" => current - px(step),
                                        "right" => current + px(step),
                                        "home" => px(MIN_LIST_WIDTH),
                                        "end" => px(maximum),
                                        _ => return,
                                    };
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    resize(&keys, target, window, cx);
                                }),
                        )
                        .into_any_element(),
                )
            }));
        div()
            .relative()
            .size_full()
            .child(group)
            .on_prepaint(move |bounds, window, cx| {
                if !paired || bounds.size.width == state.read(cx).container {
                    return;
                }
                state.update(cx, |state, _| state.container = bounds.size.width);
                // Kit scales both measured panels after a container resize. Restore
                // the chosen pixel width after that layout; only the diff grows.
                // Keep the preference when a small container temporarily clamps it.
                let state = state.clone();
                window.defer(cx, move |window, cx| {
                    let (layout, preferred) = {
                        let state = state.read(cx);
                        (state.layout.clone(), state.preferred)
                    };
                    layout.update(cx, |state, cx| state.resize_panel(0, preferred, window, cx));
                });
            })
    }
}

#[cfg(test)]
mod tests {
    use super::group;
    use gpui::component::resizable::resizable_panel;
    use gpui::{Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px, size};
    use gpui_kit as gpui;

    struct Inspector;
    impl Render for Inspector {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                group("test-git-split", "Resize the commit file list", 240.)
                    .child(
                        resizable_panel()
                            .size(px(290.))
                            .size_range(px(240.)..px(560.))
                            .flex_none()
                            .child(div().size_full().debug_selector(|| "files".into())),
                    )
                    .child(
                        resizable_panel()
                            .size_range(px(240.)..gpui::Pixels::MAX)
                            .child(div().size_full().debug_selector(|| "diff".into())),
                    ),
            )
        }
    }

    #[gpui::test]
    fn keyboard_resize_is_bounded_and_does_not_consume_modified_arrows(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, visual) = cx.add_window_view(|_, _| Inspector);
        visual.simulate_resize(size(px(900.), px(400.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(visual.debug_bounds("files").unwrap().size.width, px(290.));
        let handle = visual.debug_bounds("test-git-split-handle").unwrap();
        visual.simulate_click(handle.center(), Default::default());
        for (keys, expected) in [
            ("right", 300.),
            ("shift-right", 330.),
            ("left", 320.),
            ("home", 240.),
            ("end", 560.),
            ("right", 560.),
            ("cmd-left", 560.),
        ] {
            visual.simulate_keystrokes(keys);
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let files = visual.debug_bounds("files").unwrap();
            let diff = visual.debug_bounds("diff").unwrap();
            assert_eq!(files.size.width, px(expected), "{keys}");
            assert!(diff.size.width >= px(240.));
            assert!(diff.right() <= px(900.));
        }
        for (width, expected) in [(1200., 560.), (600., 360.), (900., 560.)] {
            visual.simulate_resize(size(px(width), px(400.)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            visual.run_until_parked();
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let files = visual.debug_bounds("files").unwrap();
            let diff = visual.debug_bounds("diff").unwrap();
            assert_eq!(files.size.width, px(expected), "viewport {width}");
            assert!(diff.size.width >= px(240.));
            assert!(diff.right() <= px(width));
        }
    }
}
