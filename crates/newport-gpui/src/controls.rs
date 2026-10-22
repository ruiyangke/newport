//! Newport control metrics in logical pixels, independent of Kit's rem scale.
//! Commands and filters share a 28px row; dense actions use 24px, modal forms
//! and tab bars use 34px. Navigation rows, editors and the title bar own their
//! own layout.
use gpui_kit::AppContext as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::{
    Sizable, Size,
    button::Button,
    checkbox::Checkbox,
    input::{Input, InputState, Textarea, TextareaState},
    pagination::Pagination,
    select::{Select, SelectDelegate, SelectItem, SelectState},
    status_bar::StatusBar,
    switch::Switch,
    tab::TabBar,
};
use gpui_kit::{
    App, Div, ElementId, Entity, Focusable, ParentElement, SharedString, Styled, Window, div, px,
    size,
};

/// Shared outer page and pane inset in logical pixels.
pub const PAGE_PADDING: f32 = 16.;
pub const CONTROL_HEIGHT: f32 = 28.;
pub const COMPACT_HEIGHT: f32 = 24.;
pub const FORM_HEIGHT: f32 = 34.;
pub const FONT_SIZE: f32 = crate::ui::FONT_SIZE;
pub const ICON_SIZE: f32 = 16.;
/// The bar across the top of a list page: search, filters, then actions.
pub const TOOLBAR_HEIGHT: f32 = 44.;
pub const TOOLBAR_INSET: f32 = 12.;
pub const SEARCH_WIDTH: f32 = 260.;

/// A list page's toolbar. Connections, Services, Containers and Files share
/// its height, inset and full-width rule so switching pages doesn't shift it.
pub fn page_toolbar(cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(TOOLBAR_HEIGHT))
        .px(px(TOOLBAR_INSET))
        .flex_shrink_0()
        .border_b_1()
        .border_color(cx.theme().border)
}

/// Hand off after the editor has mounted and the menu's dismissal frame has
/// finished, without keeping a cancelled editor alive just to focus it.
pub fn focus_editor<T: Focusable + 'static>(state: &Entity<T>, window: &mut Window, cx: &mut App) {
    let focus = state.read(cx).focus_handle(cx);
    let state = state.downgrade();
    window.on_next_frame(move |window, cx| {
        if state.upgrade().is_some() {
            focus.focus(window, cx);
        }
    });
}

fn sized_button(id: impl Into<ElementId>, height: f32) -> Button {
    let mut content = div().text_size(px(FONT_SIZE)).gap(px(6.));
    Button::new(id)
        .small()
        .h(px(height))
        .min_h(px(height))
        .max_h(px(height))
        .px(px(9.))
        .content_style(content.style().clone(), Size::Size(px(ICON_SIZE)))
}
pub fn button(id: impl Into<ElementId>) -> Button {
    sized_button(id, CONTROL_HEIGHT)
}
pub fn compact_button(id: impl Into<ElementId>) -> Button {
    sized_button(id, COMPACT_HEIGHT)
}
pub fn form_button(id: impl Into<ElementId>) -> Button {
    sized_button(id, FORM_HEIGHT)
}
pub fn icon_button(id: impl Into<ElementId>) -> Button {
    button(id)
        .w(px(CONTROL_HEIGHT))
        .min_w(px(CONTROL_HEIGHT))
        .max_w(px(CONTROL_HEIGHT))
        .p_0()
}
pub fn compact_icon_button(id: impl Into<ElementId>) -> Button {
    compact_button(id)
        .w(px(COMPACT_HEIGHT))
        .min_w(px(COMPACT_HEIGHT))
        .max_w(px(COMPACT_HEIGHT))
        .p_0()
}
fn sized_input(state: &Entity<InputState>, height: f32) -> Input {
    // Input::h configures multiline content, not a single-line frame.
    Styled::h(Input::new(state).small(), px(height))
        .min_h(px(height))
        .max_h(px(height))
        .text_size(px(FONT_SIZE))
        .px(px(9.))
        .py(px(2.))
}
pub fn input(state: &Entity<InputState>) -> Input {
    sized_input(state, CONTROL_HEIGHT)
}
/// A page title: 24px semibold, with an optional muted description 6px below.
pub fn page_title(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    cx: &App,
) -> Div {
    use gpui_kit::prelude::FluentBuilder as _;
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(crate::ui::PAGE_TITLE))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .when_some(description, |v, description| {
            v.child(
                div()
                    .text_size(px(FONT_SIZE))
                    .line_height(gpui_kit::relative(1.6))
                    .text_color(cx.theme().muted_foreground)
                    .child(description),
            )
        })
}

/// The title of a detail pane, announced as a heading.
pub fn detail_title(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
) -> gpui_kit::Stateful<Div> {
    use gpui_kit::{InteractiveElement as _, prelude::*};
    let title = title.into();
    div()
        .id(id)
        .role(gpui_kit::accesskit::Role::Heading)
        .aria_level(2)
        .aria_label(title.clone())
        .text_size(px(crate::ui::DETAIL_TITLE))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .child(title)
}

/// A dialog title, or a step or section heading that reads like one.
pub fn dialog_title(title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(crate::ui::DIALOG_TITLE))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .child(title.into())
}

/// An inline error: error-coloured text announced as an alert. Callers place
/// it; it carries no padding, border or tint of its own.
pub fn alert(
    id: impl Into<ElementId>,
    message: impl Into<SharedString>,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    use gpui_kit::{InteractiveElement as _, prelude::*};
    let message = message.into();
    div()
        .id(id)
        .role(gpui_kit::accesskit::Role::Alert)
        .aria_label(message.clone())
        .text_size(px(FONT_SIZE))
        .line_height(gpui_kit::relative(1.5))
        .text_color(crate::theme::error(cx.theme().is_dark()))
        .child(message)
}

/// A warning that part of a page can't work yet, and why: a warning-tinted
/// row with an accent edge, the warning icon, a title and an explanation.
/// Callers add the way forward as children, after the text, and place it.
pub fn warning_banner(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    use gpui_kit::{InteractiveElement as _, TestSupportExt as _, component::Icon, prelude::*};
    let (title, message) = (title.into(), message.into());
    let color = crate::theme::warning(cx.theme().is_dark());
    div()
        .id(id)
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(16.))
        .gap_y(px(10.))
        .w_full()
        .px(px(14.))
        .py(px(10.))
        .border_l(px(3.))
        .border_color(color)
        .bg(color.opacity(0.1))
        .child(
            div()
                .flex()
                .flex_1()
                .min_w(px(220.))
                .gap(px(10.))
                .child(
                    Icon::new(gpui_kit::assets::IconName::TriangleAlert)
                        .size(px(ICON_SIZE))
                        .flex_shrink_0()
                        .mt(px(2.))
                        .text_color(color),
                )
                .child(
                    div()
                        .id("warning-text")
                        .role(gpui_kit::accesskit::Role::Status)
                        .aria_label(format!("{title}. {message}"))
                        .test_support()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .min_w_0()
                        .text_size(px(FONT_SIZE))
                        .line_height(px(20.))
                        .child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child(title),
                        )
                        .child(div().text_color(cx.theme().muted_foreground).child(message)),
                ),
        )
}

/// Every code view's editor state. Scrolling follows the finger with no
/// elastic edge, as in Zed's editor: a stretched edge takes a while to settle,
/// and a drag that starts before it has does not move the text at all.
pub fn code_editor(
    window: &mut Window,
    cx: &mut gpui_kit::Context<gpui_kit::component::input::EditorState>,
) -> gpui_kit::component::input::EditorState {
    gpui_kit::component::input::EditorState::new(window, cx).scroll_bounce(false)
}

/// A 7px status dot. `None` draws the hollow "inactive" ring.
pub fn status_dot(color: Option<gpui_kit::Hsla>, cx: &App) -> Div {
    let dot = div()
        .size(px(crate::ui::DOT_SIZE))
        .flex_shrink_0()
        .rounded_full();
    match color {
        Some(color) => dot.bg(color),
        None => dot
            .border(px(1.5))
            .border_color(cx.theme().muted_foreground.opacity(0.4)),
    }
}

/// A list's keyboard-focus outline, drawn inside its edges so the rows keep
/// the full width instead of making room for a border.
pub fn inset_focus_ring(style: gpui_kit::StyleRefinement, cx: &App) -> gpui_kit::StyleRefinement {
    style.shadow(vec![gpui_kit::BoxShadow {
        color: cx.theme().ring,
        offset: gpui_kit::point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(2.),
        inset: true,
    }])
}

/// A page toolbar's search: the shared search field at the same width on
/// every page, giving way to the toolbar's other controls when narrow.
pub fn page_search(state: &Entity<InputState>, label: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .w(px(SEARCH_WIDTH))
        .min_w(px(120.))
        .flex_shrink(1.)
        .child(search_input(state, cx).aria_label(label))
}

/// A search field: the shared input with a muted 14px search icon and a
/// clear button, so every page's search reads the same.
pub fn search_input(state: &Entity<InputState>, cx: &App) -> Input {
    use gpui_kit::component::Icon;
    input(state)
        .prefix(
            Icon::new(gpui_kit::assets::IconName::Search)
                .size(px(14.))
                .text_color(cx.theme().muted_foreground),
        )
        .cleanable(true)
}
pub fn form_input(state: &Entity<InputState>) -> Input {
    sized_input(state, FORM_HEIGHT)
}
pub fn textarea(state: &Entity<TextareaState>) -> Textarea {
    Textarea::new(state)
        .small()
        .text_size(px(FONT_SIZE))
        .px(px(9.))
        .py(px(6.))
}
fn sized_select<D>(state: &Entity<SelectState<D>>, height: f32) -> Select<D>
where
    D: SelectDelegate + 'static,
    <D::Item as SelectItem>::Value: PartialEq + Clone,
{
    Select::new(state)
        // Medium keeps the dropdown's built-in search at the standard input
        // height; Small also shrinks that nested input, despite the frame below.
        .with_size(Size::Medium)
        .h(px(height))
        .min_h(px(height))
        .max_h(px(height))
        .text_size(px(FONT_SIZE))
        .px(px(9.))
        .py(px(2.))
        .menu_item_min_height(px(CONTROL_HEIGHT))
}
pub fn select<D>(state: &Entity<SelectState<D>>) -> Select<D>
where
    D: SelectDelegate + 'static,
    <D::Item as SelectItem>::Value: PartialEq + Clone,
{
    sized_select(state, CONTROL_HEIGHT)
}
pub fn form_select<D>(state: &Entity<SelectState<D>>) -> Select<D>
where
    D: SelectDelegate + 'static,
    <D::Item as SelectItem>::Value: PartialEq + Clone,
{
    sized_select(state, FORM_HEIGHT)
}
pub fn switch(id: impl Into<ElementId>) -> Switch {
    Switch::new(id)
        .track_size(size(px(36.), px(22.)))
        .label_size(px(FONT_SIZE))
        .h(px(CONTROL_HEIGHT))
        .flex()
        .items_center()
}
pub fn checkbox(id: impl Into<ElementId>) -> Checkbox {
    Checkbox::new(id)
        .with_size(px(16.))
        .text_size(px(FONT_SIZE))
}
pub fn tabs(id: impl Into<ElementId>) -> TabBar {
    TabBar::new(id)
        .with_size(px(crate::ui::HEADER_HEIGHT))
        .label_size(px(FONT_SIZE))
}

/// Widths for a resizable horizontal split, kept while its view stays mounted.
/// Each distinct key (for example, per combination of visible panes) has its own.
pub fn split_state(
    key: &SharedString,
    window: &mut Window,
    cx: &mut App,
) -> Entity<gpui_kit::component::resizable::ResizableState> {
    window
        .use_keyed_state(key.clone(), cx, |_, cx| {
            cx.new(|_| gpui_kit::component::resizable::ResizableState::default())
        })
        .read(cx)
        .clone()
}

/// The bar along a page's bottom edge: status on the left, freshness or
/// page actions on the right. Every page uses this one shape.
pub fn status_bar(cx: &App) -> StatusBar {
    StatusBar::new()
        .h(px(crate::ui::STATUS_HEIGHT))
        .min_h(px(crate::ui::STATUS_HEIGHT))
        .max_h(px(crate::ui::STATUS_HEIGHT))
        .flex_shrink_0()
        .px(px(PAGE_PADDING))
        .py_0()
        .gap(px(8.))
        .border_color(cx.theme().border)
        .bg(cx.theme().transparent)
        .text_size(px(FONT_SIZE))
        .text_color(cx.theme().muted_foreground)
}

/// The same status line beneath a pane's own content, such as a log box,
/// aligned to that content rather than spanning the page.
pub fn pane_status_bar(cx: &App) -> StatusBar {
    status_bar(cx).px_0().border_t_0()
}

/// A neutral inline label beside a name or control, such as "Docker" or
/// "Always on". States with meaning of their own use colour, not this.
pub fn tag(value: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex_shrink_0()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(crate::ui::RADIUS))
        .bg(cx.theme().muted)
        .text_color(cx.theme().muted_foreground)
        .text_size(px(FONT_SIZE))
        .child(value.into())
}

/// A small tinted label beside a name, such as a plan.
pub fn status_tag(
    value: impl Into<SharedString>,
    fill: gpui_kit::Hsla,
    ink: gpui_kit::Hsla,
) -> Div {
    div()
        .flex_shrink_0()
        .px(px(5.))
        .rounded(px(crate::ui::RADIUS))
        .bg(fill)
        .text_color(ink)
        .text_size(px(12.))
        .line_height(px(17.))
        .child(value.into())
}

/// A foldable list group: chevron, name, and its count as a tag. Callers add
/// `on_click`; the button announces whether the group is expanded.
pub fn group_heading(
    id: impl Into<ElementId>,
    name: &str,
    count: usize,
    open: bool,
    cx: &App,
) -> Button {
    use gpui_kit::component::{Icon, button::ButtonVariants as _};
    // Kit centres a button's content row; a list heading reads from the left,
    // its chevron centred over the rows' status dots.
    let mut content = div().text_size(px(FONT_SIZE)).gap(px(6.)).justify_start();
    compact_button(id)
        .ghost()
        .w_full()
        .justify_start()
        .px(px(11.))
        .content_style(content.style().clone(), Size::Size(px(12.)))
        .text_color(cx.theme().muted_foreground)
        .icon(
            Icon::new(if open {
                gpui_kit::assets::IconName::ChevronDown
            } else {
                gpui_kit::assets::IconName::ChevronRight
            })
            .size(px(12.)),
        )
        .label(name.to_owned())
        .child(tag(count.to_string(), cx))
        .accessibility_label(format!("{name}, {count}"))
}

pub fn pagination(id: impl Into<ElementId>) -> Pagination {
    let mut item = div()
        .h(px(CONTROL_HEIGHT))
        .min_h(px(CONTROL_HEIGHT))
        .max_h(px(CONTROL_HEIGHT))
        .min_w(px(CONTROL_HEIGHT))
        .px(px(9.));
    let mut content = div().text_size(px(FONT_SIZE)).gap(px(6.));
    Pagination::new(id)
        .small()
        .item_style(item.style().clone())
        .content_style(content.style().clone(), Size::Size(px(ICON_SIZE)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::component::{Disableable, IconName, tab::Tab};
    use gpui::prelude::FluentBuilder;
    use gpui::{
        AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
        TestAppContext, Window,
    };
    use gpui_kit as gpui;

    struct MenuEditor {
        field: Option<Entity<InputState>>,
    }
    impl Render for MenuEditor {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            use gpui::component::menu::{DropdownMenu, PopupMenuItem};
            if let Some(field) = &self.field {
                return div().w(px(300.)).child(input(field));
            }
            let weak = cx.entity().downgrade();
            div()
                .flex()
                .flex_col()
                .w(px(300.))
                .child(
                    button("editor-menu")
                        .label("Actions")
                        .dropdown_menu(move |menu, _, _| {
                            let weak = weak.clone();
                            menu.item(PopupMenuItem::new("New tag").on_click(
                                move |_, window, cx| {
                                    let _ = weak.update(cx, |view, cx| {
                                        let field = cx.new(|cx| InputState::new(window, cx));
                                        focus_editor(&field, window, cx);
                                        view.field = Some(field);
                                        cx.notify();
                                    });
                                },
                            ))
                        }),
                )
                .when_some(self.field.as_ref(), |view, field| view.child(input(field)))
        }
    }

    #[gpui::test]
    fn menu_editor_receives_typing_after_menu_restores_trigger_focus(cx: &mut TestAppContext) {
        use gpui::base::test_support::snapshots;
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| MenuEditor { field: None });
            editor = Some(view.clone());
            gpui::base::Root::new(view, window, cx)
        });
        let editor = editor.unwrap();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        for label in ["Actions", "New tag"] {
            let at = cx.update(|window, _| {
                snapshots(window)
                    .into_iter()
                    .find(|node| node.label() == Some(label))
                    .unwrap()
                    .bounds()
                    .center()
            });
            cx.simulate_click(at, Default::default());
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
        cx.simulate_input("v1.2.3");
        assert_eq!(
            editor.read_with(cx, |view, cx| view
                .field
                .as_ref()
                .unwrap()
                .read(cx)
                .value()
                .to_string()),
            "v1.2.3"
        );

        // A navigation/cancellation in the same event must not focus an editor
        // that no longer belongs to the visible view.
        cx.update(|window, cx| {
            let active = editor.read(cx).field.as_ref().unwrap().clone();
            let abandoned = cx.new(|cx| InputState::new(window, cx));
            focus_editor(&abandoned, window, cx);
            active.read(cx).focus_handle(cx).focus(window, cx);
            drop(abandoned);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        cx.simulate_input("-rc1");
        assert_eq!(
            editor.read_with(cx, |view, cx| view
                .field
                .as_ref()
                .unwrap()
                .read(cx)
                .value()
                .to_string()),
            "v1.2.3-rc1"
        );
    }

    struct Gallery {
        field: Entity<InputState>,
        form_field: Entity<InputState>,
        choice: Entity<SelectState<Vec<&'static str>>>,
        form_choice: Entity<SelectState<Vec<&'static str>>>,
        pending: bool,
    }
    fn probe(id: &'static str, child: impl IntoElement) -> impl IntoElement {
        div().debug_selector(move || id.into()).child(child)
    }
    impl Render for Gallery {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(10.))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .child(probe(
                            "command",
                            button("command")
                                .label("Refresh")
                                .icon(IconName::RefreshCw)
                                .loading(self.pending)
                                .disabled(self.pending),
                        ))
                        .child(probe("icon", icon_button("icon").icon(IconName::Search)))
                        .child(probe("filter", input(&self.field).w(px(150.))))
                        .child(probe("select", select(&self.choice).w(px(150.)))),
                )
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .child(probe(
                            "form-command",
                            form_button("form-command")
                                .label("Save")
                                .disabled(self.pending),
                        ))
                        .child(probe(
                            "form-field",
                            form_input(&self.form_field).w(px(150.)),
                        ))
                        .child(probe(
                            "form-select",
                            form_select(&self.form_choice).w(px(150.)),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .child(probe("compact", compact_button("compact").label("Retry")))
                        .child(probe(
                            "compact-icon",
                            compact_icon_button("compact-icon").icon(IconName::Close),
                        )),
                )
                .child(probe(
                    "switch",
                    switch("switch")
                        .label("Live")
                        .checked(self.pending)
                        .disabled(self.pending),
                ))
                .child(probe(
                    "checkbox",
                    checkbox("checkbox")
                        .checked(self.pending)
                        .disabled(self.pending),
                ))
                .child(probe(
                    "segmented",
                    tabs("segmented")
                        .gap_0()
                        .selected_index(0)
                        .children([Tab::new().label("Changes"), Tab::new().label("History")]),
                ))
                .child(probe(
                    "underline",
                    tabs("underline")
                        .underline()
                        .selected_index(0)
                        .children([Tab::new().label("Logs"), Tab::new().label("Unit file")]),
                ))
                .child(probe(
                    "pagination",
                    pagination("pages").total_pages(10).current_page(5).p_0(),
                ))
        }
    }
    #[gpui::test]
    fn control_geometry_is_consistent_across_font_scales_and_states(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let mut gallery = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| Gallery {
                field: cx.new(|cx| InputState::new(window, cx).default_value("Search")),
                form_field: cx.new(|cx| InputState::new(window, cx).default_value("Server")),
                choice: cx.new(|cx| SelectState::new(vec!["All", "Active"], None, window, cx)),
                form_choice: cx
                    .new(|cx| SelectState::new(vec!["Automatic", "Custom"], None, window, cx)),
                pending: false,
            });
            gallery = Some(view.clone());
            gpui::base::Root::new(view, window, cx)
        });
        let gallery = gallery.unwrap();
        cx.simulate_resize(size(px(850.), px(650.)));
        let mut command_size = None;
        for rem in [14., 16.] {
            for pending in [false, true] {
                gallery.update(cx, |view, cx| {
                    view.pending = pending;
                    cx.notify();
                });
                cx.update(|window, cx| {
                    window.set_rem_size(px(rem));
                    window.draw(cx).clear(cx);
                });
                cx.run_until_parked();
                cx.update(|window, cx| window.draw(cx).clear(cx));
                for (id, height) in [
                    ("command", CONTROL_HEIGHT),
                    ("icon", CONTROL_HEIGHT),
                    ("filter", CONTROL_HEIGHT),
                    ("select", CONTROL_HEIGHT),
                    ("form-command", FORM_HEIGHT),
                    ("form-field", FORM_HEIGHT),
                    ("form-select", FORM_HEIGHT),
                    ("compact", COMPACT_HEIGHT),
                    ("compact-icon", COMPACT_HEIGHT),
                    ("switch", CONTROL_HEIGHT),
                    ("checkbox", 16.),
                    ("segmented", crate::ui::HEADER_HEIGHT),
                    ("underline", crate::ui::HEADER_HEIGHT),
                    ("pagination", CONTROL_HEIGHT),
                ] {
                    let bounds = cx
                        .debug_bounds(id)
                        .unwrap_or_else(|| panic!("missing {id}"));
                    assert_eq!(
                        bounds.size.height,
                        px(height),
                        "{id}, rem={rem}, pending={pending}"
                    );
                    if id == "icon" || id == "compact-icon" || id == "checkbox" {
                        assert_eq!(bounds.size.width, px(height), "{id} must remain square");
                    }
                }
                let measured = cx.debug_bounds("command").unwrap().size;
                assert_eq!(
                    *command_size.get_or_insert(measured),
                    measured,
                    "loading/font scale must not move the command"
                );
            }
        }
    }
}
