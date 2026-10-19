//! Diff presentation controls, matching React's persistent settings popover.
use super::*;
use git_diff_split::Layout;
use gpui_kit::component::{
    popover::Popover,
    select::{SelectEvent, SelectItem, SelectState},
};

#[derive(Clone, PartialEq, Eq)]
struct Scope {
    server: String,
    repo: Value,
    tab: usize,
}
impl Scope {
    fn current(app: &Newport) -> Self {
        Self {
            server: app.server.clone(),
            repo: app.git.repo["repoId"].clone(),
            tab: app.git.tab,
        }
    }
    fn matches(&self, app: &Newport) -> bool {
        app.page == Page::Projects && *self == Self::current(app)
    }
}
#[derive(Clone)]
struct ContextChoice(u8);
impl SelectItem for ContextChoice {
    type Value = u8;
    fn title(&self) -> SharedString {
        format!(
            "{} context {}",
            self.0,
            if self.0 == 1 { "line" } else { "lines" }
        )
        .into()
    }
    fn value(&self) -> &u8 {
        &self.0
    }
}
struct Panel {
    owner: WeakEntity<Newport>,
    scope: Scope,
    context: Entity<SelectState<Vec<ContextChoice>>>,
    radio_focus: [FocusHandle; 2],
    _subscriptions: Vec<Subscription>,
}
impl Panel {
    fn new(owner: Entity<Newport>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scope = Scope::current(owner.read(cx));
        let selected = owner.read(cx).git.diff_context.value();
        let context = cx.new(|cx| {
            let mut state = SelectState::new(
                git_diff::ContextLines::CHOICES
                    .into_iter()
                    .map(ContextChoice)
                    .collect::<Vec<_>>(),
                None,
                window,
                cx,
            );
            state.set_selected_value(&selected, window, cx);
            state
        });
        let changed = cx.subscribe(&context, |panel, _, event, cx| {
            if let SelectEvent::Confirm(Some(value)) = event {
                let _ = panel.owner.update(cx, |app, cx| {
                    if panel.scope.matches(app)
                        && !app.git_is_inspection()
                        && app.git.diff_context.set(*value)
                    {
                        app.git_select_side(app.git.diff_staged);
                        cx.notify();
                    }
                });
            }
        });
        // Observe the input, never the owner: keyed state is already observed
        // by its owning view, so observing back would create a notify cycle.
        let observed = cx.observe(&context, |_, _, cx| cx.notify());
        Self {
            owner: owner.downgrade(),
            scope,
            context,
            radio_focus: [cx.focus_handle(), cx.focus_handle()],
            _subscriptions: vec![changed, observed],
        }
    }
    fn choose_layout(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let choice = if index == 0 {
            Layout::Unified
        } else {
            Layout::Split
        };
        let _ = self.owner.update(cx, |app, cx| {
            if self.scope.matches(app) && app.diff_layout() != choice {
                app.git_diff_layouts.insert(app.server.clone(), choice);
                app.git.diff_scroll = UniformListScrollHandle::default();
                cx.notify();
            }
        });
        self.radio_focus[index].focus(window, cx);
    }
}
impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self
            .owner
            .upgrade()
            .filter(|owner| self.scope.matches(owner.read(cx)));
        let Some(owner) = current else {
            return div().into_any_element();
        };
        let app = owner.read(cx);
        let index = usize::from(app.diff_layout() == Layout::Split);
        let working = !app.git_is_inspection();
        let context = app.git.diff_context.value();
        if self.context.read(cx).selected_value() != Some(&context) {
            self.context.update(cx, |state, cx| {
                state.set_selected_value(&context, window, cx)
            });
        }
        let primary = cx.theme().primary;
        let border = cx.theme().input;
        div()
            .id("diff-settings-panel")
            .role(accesskit::Role::Group)
            .aria_label("Diff settings")
            .w(px(196.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_size(px(crate::ui::FONT_SIZE))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Diff Settings"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(crate::label("Diff display").text_size(px(crate::ui::FONT_SIZE)))
                    .child(
                        gpui_kit::base::RadioGroup::new("diff-layout")
                            .aria_label("Diff display")
                            .axis(Axis::Vertical)
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .on_key_down(cx.listener(
                                move |panel, event: &KeyDownEvent, window, cx| {
                                    if event.keystroke.modifiers.platform
                                        || event.keystroke.modifiers.control
                                        || event.keystroke.modifiers.alt
                                    {
                                        return;
                                    }
                                    let next = match event.keystroke.key.as_str() {
                                        "up" | "left" | "down" | "right" => 1 - index,
                                        "home" => 0,
                                        "end" => 1,
                                        _ => return,
                                    };
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    panel.choose_layout(next, window, cx);
                                },
                            ))
                            .children(["Unified", "Split"].into_iter().enumerate().map(
                                |(i, title)| {
                                    let owner = cx.entity().downgrade();
                                    gpui_kit::base::Radio::new(title)
                                        .accessibility_label(title)
                                        .checked(index == i)
                                        .set_position(i + 1, 2)
                                        .track_focus(&self.radio_focus[i])
                                        .tab_stop(index == i)
                                        .h(px(18.))
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .focus_visible(|style| {
                                            crate::controls::inset_focus_ring(style, cx)
                                        })
                                        .child(
                                            div()
                                                .size(px(14.))
                                                .rounded_full()
                                                .border_1()
                                                .border_color(if index == i {
                                                    primary
                                                } else {
                                                    border
                                                })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .when(index == i, |view| {
                                                    view.child(
                                                        div()
                                                            .size(px(6.))
                                                            .rounded_full()
                                                            .bg(primary),
                                                    )
                                                }),
                                        )
                                        .child(title)
                                        .on_change(move |_, _, window, cx| {
                                            let _ = owner.update(cx, |panel, cx| {
                                                panel.choose_layout(i, window, cx)
                                            });
                                        })
                                },
                            )),
                    ),
            )
            .when(working, |view| {
                view.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(crate::label("Context").text_size(px(crate::ui::FONT_SIZE)))
                        .child(
                            crate::controls::select(&self.context)
                                .w_full()
                                .accessibility_label("Context lines"),
                        ),
                )
            })
            .into_any_element()
    }
}
#[derive(IntoElement)]
pub(super) struct Settings {
    owner: Entity<Newport>,
}
pub(super) fn control(owner: Entity<Newport>) -> Settings {
    Settings { owner }
}
impl RenderOnce for Settings {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let app = self.owner.read(cx);
        let scope = Scope::current(app);
        let id = SharedString::from(format!(
            "diff-settings-{}-{}-{}",
            scope.server, scope.repo, scope.tab
        ));
        let panel = window.use_keyed_state(id.clone(), cx, |window, cx| {
            Panel::new(self.owner, window, cx)
        });
        let nested_open = panel.read(cx).context.read(cx).is_open();
        Popover::new(id)
            .overlay_closable(!nested_open)
            .anchor(Anchor::TopRight)
            .p(px(12.))
            .offset(px(4.))
            .trigger(
                crate::controls::compact_icon_button("diff-settings")
                    .ghost()
                    .icon(IconName::Settings2)
                    .accessibility_label("Diff settings")
                    .tooltip("Diff settings"),
            )
            .content(move |_, _, _| panel.clone())
    }
}
