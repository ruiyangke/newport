//! Shared Projects typography. Control density never changes label size.
use super::*;
use gpui_kit::component::{Size, input::TextareaState, tab::TabBar};

pub(super) const FONT_SIZE: f32 = 14.;
pub(super) const PATH_ROW_HEIGHT: f32 = 32.;
pub(super) const HEADER_HEIGHT: f32 = 34.;

/// Same geometry for Explorer, navigation tabs and document tabs. Paint rules
/// as overlays so borders cannot shift the content's vertical center.
pub(super) fn header() -> Div {
    div()
        .h(px(HEADER_HEIGHT))
        .min_h(px(HEADER_HEIGHT))
        .max_h(px(HEADER_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .relative()
        .text_size(px(FONT_SIZE))
        .line_height(relative(1.25))
        .font_weight(FontWeight::MEDIUM)
}
pub(super) fn header_rule(cx: &App) -> Div {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h(px(1.))
        .bg(cx.theme().border)
}

fn size_label(button: Button) -> Button {
    let mut content = div().text_size(px(FONT_SIZE)).gap(px(6.));
    button.content_style(
        content.style().clone(),
        Size::Size(px(crate::controls::ICON_SIZE)),
    )
}
pub(super) fn button(id: impl Into<ElementId>) -> Button {
    size_label(crate::controls::button(id))
}
pub(super) fn compact_button(id: impl Into<ElementId>) -> Button {
    size_label(crate::controls::compact_button(id))
}
pub(super) fn input(state: &Entity<InputState>) -> gpui_kit::component::input::Input {
    crate::controls::input(state).text_size(px(FONT_SIZE))
}
pub(super) fn textarea(state: &Entity<TextareaState>) -> gpui_kit::component::input::Textarea {
    crate::controls::textarea(state).text_size(px(FONT_SIZE))
}
pub(super) fn tabs(id: impl Into<ElementId>) -> TabBar {
    crate::controls::tabs(id).label_size(px(FONT_SIZE))
}

/// Secondary copy keeps its tone, never a smaller font.
pub(super) fn label(value: impl Into<SharedString>) -> Div {
    super::label(value).text_size(px(FONT_SIZE))
}

pub(super) fn form_button(id: impl Into<ElementId>) -> Button {
    size_label(crate::controls::form_button(id))
}
pub(super) fn checkbox(id: impl Into<ElementId>) -> gpui_kit::component::checkbox::Checkbox {
    crate::controls::checkbox(id).text_size(px(FONT_SIZE))
}
pub(super) fn select<D>(
    state: &Entity<gpui_kit::component::select::SelectState<D>>,
) -> gpui_kit::component::select::Select<D>
where
    D: gpui_kit::component::select::SelectDelegate + 'static,
    <D::Item as gpui_kit::component::select::SelectItem>::Value: PartialEq + Clone,
{
    crate::controls::select(state).text_size(px(FONT_SIZE))
}
