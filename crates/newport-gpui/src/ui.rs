//! Shared visual contract. Metrics never own application state or commands.
use super::*;
pub const FONT_SIZE: f32 = 14.;
pub const RADIUS: f32 = 0.;
pub const HEADER_HEIGHT: f32 = 34.;
pub const TITLEBAR_HEIGHT: f32 = 40.;
pub const PATH_ROW_HEIGHT: f32 = 32.;
pub const ICON_RAIL_WIDTH: f32 = 48.;
pub const STATUS_HEIGHT: f32 = 28.;
/// Single-line list rows, and rows that carry a title over a detail line.
pub const ROW_HEIGHT: f32 = 28.;
pub const TWO_LINE_ROW_HEIGHT: f32 = 44.;
/// Title of a full page (welcome, setup, empty libraries).
pub const PAGE_TITLE: f32 = 24.;
/// Title of a detail pane (a selected service, container or project).
pub const DETAIL_TITLE: f32 = 18.;
/// Title of a dialog, and of a step or section that acts like one.
pub const DIALOG_TITLE: f32 = 17.;
/// Inline icons inside rows and labels; icon buttons use controls::ICON_SIZE.
pub const SMALL_ICON: f32 = 14.;
/// Status dots in rows, headers and pickers.
pub const DOT_SIZE: f32 = 7.;

/// A fixed pane header; the rule overlays its bottom edge to preserve alignment.
pub fn header() -> Div {
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
pub fn header_rule(cx: &App) -> Div {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h(px(1.))
        .bg(cx.theme().border)
}
