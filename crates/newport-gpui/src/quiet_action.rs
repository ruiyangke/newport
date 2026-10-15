//! Secondary actions stay discoverable to keyboard and accessibility users.
use super::*;

pub(super) type MenuBuilder = dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu;

/// Keep the secondary action quiet without removing its keyboard tab stop.
/// Popup visibility is local presentation state, scoped to this rendered row.
#[derive(IntoElement)]
pub(super) struct Menu {
    pub id: SharedString,
    pub group: SharedString,
    pub engaged: bool,
    pub button: Button,
    pub builder: Box<MenuBuilder>,
}

struct Presentation {
    open: bool,
    focus: FocusHandle,
    _subscriptions: [Subscription; 2],
}
impl RenderOnce for Menu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |window, cx| {
            let focus = cx.focus_handle();
            let entered = cx.on_focus_in(&focus, window, |_, _, cx| cx.notify());
            let exited = cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify());
            Presentation {
                open: false,
                focus,
                _subscriptions: [entered, exited],
            }
        });
        let presentation = state.read(cx);
        let focus = presentation.focus.clone();
        let visible = presentation.open || self.engaged || focus.contains_focused(window, cx);
        div().id(self.id).track_focus(&focus).tab_stop(false).child(
            self.button
                .opacity(if visible { 1. } else { 0. })
                .group_hover(self.group, |style| style.opacity(1.))
                .dropdown_menu(move |menu, window, cx| (self.builder)(menu, window, cx))
                .on_open_change(move |open, _, cx| {
                    state.update(cx, |state, cx| {
                        state.open = *open;
                        cx.notify();
                    })
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Menu;
    use gpui::component::{button::Button, menu::PopupMenuItem};
    use gpui::{
        Context, FocusHandle, IntoElement, Render, TestAppContext, VisualTestContext, Window, div,
        point, prelude::*, px, rgb,
    };
    use gpui_kit as gpui;

    const MARKER: u32 = 0xfc02d9;
    struct Gallery {
        action: FocusHandle,
        outside: FocusHandle,
    }
    impl Render for Gallery {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .pt(px(60.))
                .child(
                    div()
                        .id("row")
                        .group("project-row")
                        .flex()
                        .items_center()
                        .w(px(280.))
                        .h(px(40.))
                        .child(div().w(px(180.)).child("Project name"))
                        .child(Menu {
                            id: "menu-visibility".into(),
                            group: "project-row".into(),
                            engaged: false,
                            button: Button::new("actions")
                                .track_focus(&self.action)
                                .accessibility_label("Project actions")
                                .w(px(40.))
                                .h(px(28.))
                                .child(
                                    div()
                                        .size(px(10.))
                                        .bg(rgb(MARKER))
                                        .debug_selector(|| "action-marker".into()),
                                ),
                            builder: Box::new(|menu, _, _| {
                                menu.item(PopupMenuItem::new("Check status"))
                            }),
                        }),
                )
                .child(
                    Button::new("outside")
                        .track_focus(&self.outside)
                        .label("Outside")
                        .mt(px(60.)),
                )
        }
    }
    fn painted(visual: &mut VisualTestContext) -> bool {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window
                .painted_quads()
                .iter()
                .any(|q| q.background == rgb(MARKER).into())
        })
    }
    #[gpui::test]
    fn row_menu_reveals_without_layout_shift_and_restores_focus_in_both_appearances(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui::init);
        let (view, visual) = cx.add_window_view(|_, cx| Gallery {
            action: cx.focus_handle(),
            outside: cx.focus_handle(),
        });
        for dark in [false, true] {
            visual.update(|window, cx| {
                crate::theme::install(dark, cx);
                view.read(cx).outside.clone().focus(window, cx);
                window.refresh();
            });
            visual.simulate_mouse_move(point(px(350.), px(250.)), None, Default::default());
            assert!(!painted(visual));
            let bounds = visual.debug_bounds("action-marker").unwrap();
            visual.simulate_mouse_move(point(px(100.), px(75.)), None, Default::default());
            assert!(painted(visual), "hover anywhere in row reveals its menu");
            visual.simulate_mouse_move(point(px(350.), px(250.)), None, Default::default());
            assert!(!painted(visual));
            let focused = visual.update(|window, cx| {
                window.blur(cx);
                window.focus_next(cx);
                window.focused(cx).expect("menu has a tab stop")
            });
            assert!(painted(visual), "keyboard focus reveals the trigger");
            assert_eq!(visual.debug_bounds("action-marker").unwrap(), bounds);
            visual.simulate_click(bounds.center(), Default::default());
            visual.simulate_mouse_move(point(px(350.), px(250.)), None, Default::default());
            assert!(painted(visual), "open popup keeps the trigger visible");
            visual.simulate_keystrokes("escape");
            assert!(painted(visual));
            visual.update(|window, _cx| {
                assert!(
                    focused.is_focused(window),
                    "Escape returns focus to the trigger"
                )
            });
            visual.update(|window, cx| view.read(cx).outside.clone().focus(window, cx));
            assert!(
                !painted(visual),
                "closed menu must not retain its revealed state"
            );
        }
    }
}
