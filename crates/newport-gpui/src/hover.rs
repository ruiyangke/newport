//! Shared hover presentation: brief action hints and wrapped read-only details.
use gpui_kit::component::{ActiveTheme, tooltip::Tooltip};
use gpui_kit::{AnyView, App, FontWeight, SharedString, Window, div, prelude::*, px, relative};

pub(crate) const PADDING: f32 = 12.;

pub(crate) fn width(window: &Window) -> f32 {
    (f32::from(window.viewport_size().width) - 48.).clamp(80., 380.)
}

pub(crate) fn height(window: &Window) -> f32 {
    (f32::from(window.viewport_size().height) - 72.).clamp(60., 360.)
}

pub(crate) fn surface(tooltip: Tooltip) -> Tooltip {
    tooltip
        .rounded(px(crate::ui::RADIUS))
        .border_1()
        .text_size(px(crate::ui::FONT_SIZE))
}

pub(crate) trait HoverExt: InteractiveElement + Sized {
    fn hover_hint(mut self, content: impl Into<SharedString>) -> Self {
        let content = content.into();
        if content.trim().is_empty() {
            return self;
        }
        if content.len() > 64 || content.contains('\n') {
            self.interactivity()
                .hoverable_tooltip(move |window, cx| text(content.clone(), window, cx));
        } else {
            self.interactivity()
                .tooltip(move |window, cx| text(content.clone(), window, cx));
        }
        self
    }

    fn hover_path(mut self, title: impl Into<SharedString>, path: impl Into<SharedString>) -> Self {
        let title = title.into();
        let path = path.into();
        if path.trim().is_empty() {
            return self;
        }
        self.interactivity().hoverable_tooltip(move |window, cx| {
            detail(title.clone(), path.clone(), true, window, cx)
        });
        self
    }
}
impl<T: InteractiveElement> HoverExt for T {}

pub(crate) fn text(content: SharedString, window: &mut Window, cx: &mut App) -> AnyView {
    let max_width = width(window) - 16.;
    let height = height(window);
    let long = content.len() > 64 || content.contains('\n');
    surface(Tooltip::element(move |_, cx| {
        div()
            .id("hover-hint")
            .debug_selector(|| "hover-hint-content".into())
            .max_h(px(height))
            .overflow_y_scroll()
            .when(long, |view| view.w(px(max_width)))
            .max_w(px(max_width))
            .min_w_0()
            .text_color(cx.theme().popover_foreground)
            .text_size(px(crate::ui::FONT_SIZE))
            .line_height(relative(1.45))
            .whitespace_normal()
            .child(content.clone())
    }))
    .px(px(8.))
    .py(px(6.))
    .build(window, cx)
}

pub(crate) fn detail(
    title: SharedString,
    body: SharedString,
    monospace: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let width = width(window);
    let height = height(window);
    surface(Tooltip::element(move |_, cx| {
        div()
            .id("hover-detail")
            .debug_selector(|| "hover-detail-content".into())
            .max_h(px(height))
            .overflow_y_scroll()
            .w(px(width - 2. * PADDING))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .text_color(cx.theme().popover_foreground)
            .line_height(relative(1.45))
            .when(!title.trim().is_empty(), |view| {
                view.child(
                    div()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .whitespace_normal()
                        .child(title.clone()),
                )
            })
            .child(
                div()
                    .debug_selector(|| "hover-detail-body".into())
                    .flex_shrink_0()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .when(monospace, |view| {
                        view.font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(crate::ui::FONT_SIZE))
                    })
                    .child(body.clone()),
            )
    }))
    .w(px(width))
    .p(px(PADDING))
    .build(window, cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Context, Render, TestAppContext, VisualTestContext, point, size};
    use std::time::Duration;

    struct Host {
        content: String,
        rich: bool,
        right: bool,
        bottom: bool,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                crate::controls::button("hover-test-trigger")
                    .absolute()
                    .w(px(40.))
                    .h(px(28.))
                    .when(self.right, |v| v.right(px(4.)))
                    .when(!self.right, |v| v.left(px(4.)))
                    .when(self.bottom, |v| v.bottom(px(4.)))
                    .when(!self.bottom, |v| v.top(px(4.)))
                    .map(|v| {
                        if self.rich {
                            v.hover_path("Path", self.content.clone())
                        } else {
                            v.hover_hint(self.content.clone())
                        }
                    })
                    .child("Info"),
            )
        }
    }
    fn settle(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        crate::page_perf_tests::frame(cx);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    #[gpui_kit::test]
    fn long_hover_can_be_entered_and_scrolled_to_the_end(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, cx) = cx.add_window_view(|_, _| Host {
            content: format!("{}last-line", "long-path-component/".repeat(200)),
            rich: true,
            right: false,
            bottom: false,
        });
        cx.simulate_resize(size(px(320.), px(260.)));
        crate::page_perf_tests::frame(cx);
        cx.simulate_mouse_move(point(px(24.), px(18.)), None, Default::default());
        settle(cx);
        let viewport = cx.debug_bounds("hover-detail-content").unwrap();
        assert!(cx.debug_bounds("hover-detail-body").unwrap().bottom() > viewport.bottom());
        cx.simulate_mouse_move(viewport.center(), None, Default::default());
        settle(cx);
        assert!(
            cx.debug_bounds("hover-detail-content").is_some(),
            "entering the popup keeps it open"
        );
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: viewport.center(),
            delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-100_000.))),
            ..Default::default()
        });
        settle(cx);
        let body = cx.debug_bounds("hover-detail-body").unwrap();
        assert!(
            body.bottom() <= viewport.bottom() + px(1.),
            "full detail is reachable: {body:?}, {viewport:?}"
        );
        cx.simulate_mouse_move(point(px(-100.), px(-100.)), None, Default::default());
        settle(cx);
        assert!(cx.debug_bounds("hover-detail-content").is_none());
    }

    #[gpui_kit::test]
    fn hover_content_fits_edges_and_empty_hints_do_not_paint(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (host, cx) = cx.add_window_view(|_, _| Host {
            content: "Short hint".into(),
            rich: false,
            right: false,
            bottom: false,
        });
        for dark in [false, true] {
            cx.update(|_, cx| crate::theme::install(dark, cx));
            for (width, height) in [(800., 600.), (320., 260.)] {
                cx.simulate_resize(size(px(width), px(height)));
                for (right, bottom) in [(false, false), (true, false), (false, true), (true, true)]
                {
                    for (content, rich) in [
                        ("Short hint".to_owned(), false),
                        (
                            format!("/home/user/{}/file.rs", "long-directory/".repeat(30)),
                            true,
                        ),
                        ("x".repeat(500), false),
                        (" \n\t ".to_owned(), false),
                    ] {
                        cx.simulate_mouse_move(
                            point(px(width / 2.), px(height / 2.)),
                            None,
                            Default::default(),
                        );
                        settle(cx);
                        host.update(cx, |host, cx| {
                            host.content = content.clone();
                            host.rich = rich;
                            host.right = right;
                            host.bottom = bottom;
                            cx.notify();
                        });
                        crate::page_perf_tests::frame(cx);
                        let at = point(
                            px(if right { width - 24. } else { 24. }),
                            px(if bottom { height - 18. } else { 18. }),
                        );
                        cx.simulate_mouse_move(at, None, Default::default());
                        settle(cx);
                        let selector = if rich {
                            "hover-detail-content"
                        } else {
                            "hover-hint-content"
                        };
                        let bounds = cx.debug_bounds(selector);
                        if content.trim().is_empty() {
                            assert!(bounds.is_none(), "empty tooltip must not render chrome");
                            continue;
                        }
                        let bounds = bounds.expect("hover must appear on a component button");
                        assert!(
                            bounds.left() >= px(0.) && bounds.right() <= px(width),
                            "{bounds:?}"
                        );
                        assert!(
                            bounds.top() >= px(0.) && bounds.bottom() <= px(height),
                            "{bounds:?}"
                        );
                        if content == "Short hint" {
                            assert!(
                                bounds.size.width < px(150.) && bounds.size.height < px(40.),
                                "short hints stay compact: {bounds:?}"
                            );
                        } else {
                            assert!(
                                bounds.size.width >= px(200.),
                                "long text should not collapse to a word-wide column: {bounds:?}"
                            );
                        }
                        if content == "Short hint" {
                            cx.simulate_click(at, Default::default());
                        } else {
                            // Rich content remains open while entered for scrolling.
                            // Leave both trigger and popup to dismiss it.
                            cx.simulate_mouse_move(
                                point(px(-100.), px(-100.)),
                                None,
                                Default::default(),
                            );
                        }
                        settle(cx);
                        assert!(
                            cx.debug_bounds(selector).is_none(),
                            "hover dismisses: rich={rich}, length={}, right={right}, bottom={bottom}, original={bounds:?}",
                            content.len()
                        );
                    }
                }
            }
        }
    }
}
