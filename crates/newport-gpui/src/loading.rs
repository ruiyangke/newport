//! Shared placeholders for reads. Cached content remains visible during refresh.
use super::*;
use gpui_kit::component::skeleton::Skeleton;

pub(super) fn lines(name: &str, count: usize) -> AnyElement {
    line_content(name, count, true)
}

fn line_content(name: &str, count: usize, padded: bool) -> AnyElement {
    div()
        .id(SharedString::from(format!("loading-{name}")))
        .role(accesskit::Role::Status)
        .aria_label(format!("Loading {name}"))
        .w_full()
        .min_w_0()
        .when(padded, |view| view.p_3())
        .flex()
        .flex_col()
        .gap_3()
        .children((0..count).map(|index| {
            div().w_full().child(
                Skeleton::new()
                    .h(px(14.))
                    .rounded(px(4.))
                    .when(index % 3 == 2, |line| line.w(relative(0.6))),
            )
        }))
        .into_any_element()
}

pub(super) fn overview_columns(width: f32) -> u16 {
    if width < 764. { 2 } else { 4 }
}

pub(super) fn history(metric: usize) -> AnyElement {
    div()
        .id("overview-history-loading")
        .role(accesskit::Role::Status)
        .aria_label("Loading metric history")
        .flex()
        .flex_col()
        .w_full()
        .child(
            div()
                .h(px(208.))
                .flex_shrink_0()
                .flex()
                .child(
                    div()
                        .w(px(if metric == 3 { 64. } else { 40. }))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .justify_between()
                        .children((0..3).map(|_| Skeleton::new().w(px(28.)).h(px(10.)))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Skeleton::new().h_full().rounded(px(4.))),
                ),
        )
        .child(div().h(px(14.)).mt_1().flex_shrink_0())
        .into_any_element()
}

pub(super) fn page(page: Page, width: f32) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .p(px(crate::controls::PAGE_PADDING))
        .flex()
        .flex_col()
        .gap_4()
        .child(line_content("page", 1, false))
        .when(page == Page::Overview, |view| {
            view.child(
                div()
                    .grid()
                    .grid_cols(overview_columns(width))
                    .gap(px(10.))
                    .children((0..4).map(|_| {
                        div()
                            .flex_1()
                            .child(Skeleton::new().h(px(134.)).rounded(px(10.)))
                    })),
            )
            .child(Skeleton::new().h(px(208.)).rounded(px(10.)))
        })
        .child(line_content("content", 8, false))
        .into_any_element()
}

impl Newport {
    pub(super) fn initial_loading(&self, key: &str) -> bool {
        self.resources
            .get(key)
            .is_some_and(|r| r.pending && r.value.is_null())
    }
}
