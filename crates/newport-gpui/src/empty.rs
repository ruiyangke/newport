//! Shared empty-state presentation. Callers own the evidence that a read succeeded.
//! Keep actions outside the status node, so assistive technology can reach them.
use super::*;

/// A quiet, wrapping message for menus, virtual-list rows and embedded sections.
/// No fixed height: narrow panes and longer messages may grow naturally.
pub(super) fn compact(message: impl Into<SharedString>, cx: &App) -> Div {
    let message = message.into();
    div()
        .min_w_0()
        .px(px(12.))
        .py(px(16.))
        .text_size(px(crate::ui::FONT_SIZE))
        .line_height(px(18.))
        .text_color(cx.theme().muted_foreground)
        .child(
            div()
                .id(SharedString::from(format!("empty-message:{message}")))
                .role(accesskit::Role::Status)
                .aria_label(message.clone())
                .child(message),
        )
}

/// Content shared by whole-page and detail-pane states, including clean checkouts.
pub(super) fn content(
    icon: IconName,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    cx: &App,
) -> Div {
    let title = title.into();
    let description = description.into();
    div()
        .w_full()
        .max_w(px(360.))
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.))
        .text_center()
        .child(
            Icon::new(icon)
                .size(px(28.))
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            div()
                .id(SharedString::from(format!("empty-title:{title}")))
                .role(accesskit::Role::Status)
                .aria_label(if description.is_empty() {
                    title.to_string()
                } else {
                    format!("{title}. {description}")
                })
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(14.))
                        .line_height(px(20.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .child(title),
                )
                .when(!description.is_empty(), |view| {
                    view.child(
                        div()
                            .text_size(px(crate::ui::FONT_SIZE))
                            .line_height(px(18.))
                            .text_color(cx.theme().muted_foreground)
                            .child(description),
                    )
                }),
        )
}

/// Flexible pane; keep surrounding headers, filters and footers mounted.
pub(super) fn pane(
    icon: IconName,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    cx: &App,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(16.))
        .p(px(24.))
        .child(content(icon, title, description, cx))
}
