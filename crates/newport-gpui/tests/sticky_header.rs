use gpui::{
    Context, FocusHandle, IntoElement, Modifiers, Render, ScrollHandle, TestAppContext, Window,
    div, point, prelude::*, px, rgb,
};
use gpui_kit as gpui;
use newport_gpui::sticky_header::StickyHeader;
use std::{cell::Cell, rc::Rc};

struct Harness {
    scroll: ScrollHandle,
    header: FocusHandle,
    body: FocusHandle,
    header_clicks: Rc<Cell<usize>>,
    body_clicks: Rc<Cell<usize>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let header_clicks = self.header_clicks.clone();
        let body_clicks = self.body_clicks.clone();
        div()
            .id("viewport")
            .w(px(300.))
            .h(px(200.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .child(div().h(px(80.)).flex_shrink_0())
            .child(StickyHeader::new(
                div()
                    .id("header")
                    .h(px(40.))
                    .flex_shrink_0()
                    .track_focus(&self.header)
                    .tab_stop(true)
                    .bg(rgb(0xffffff))
                    .debug_selector(|| "header".into())
                    .on_click(move |_, _, _| header_clicks.set(header_clicks.get() + 1)),
                div().h(px(600.)).flex_shrink_0().child(
                    div()
                        .id("first-row")
                        .h(px(40.))
                        .track_focus(&self.body)
                        .tab_stop(true)
                        .debug_selector(|| "first-row".into())
                        .on_click(move |_, _, _| body_clicks.set(body_clicks.get() + 1)),
                ),
            ))
            .child(div().h(px(200.)).flex_shrink_0())
    }
}

#[gpui::test]
fn header_sticks_without_losing_flow_keyboard_order_or_hit_testing(cx: &mut TestAppContext) {
    let (view, visual) = cx.add_window_view(|_, cx| Harness {
        scroll: ScrollHandle::new(),
        header: cx.focus_handle().tab_stop(true),
        body: cx.focus_handle().tab_stop(true),
        header_clicks: Rc::default(),
        body_clicks: Rc::default(),
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(visual.debug_bounds("header").unwrap().top(), px(80.));
    assert_eq!(visual.debug_bounds("first-row").unwrap().top(), px(120.));
    visual.update(|window, cx| {
        window.blur(cx);
        window.focus_next(cx);
        assert!(view.read(cx).header.is_focused(window));
        window.focus_next(cx);
        assert!(view.read(cx).body.is_focused(window));
        view.read(cx).scroll.set_offset(point(px(0.), px(-120.)));
        window.refresh();
        window.draw(cx).clear(cx);
    });
    assert_eq!(visual.debug_bounds("header").unwrap().top(), px(0.));
    visual.simulate_click(point(px(10.), px(10.)), Modifiers::default());
    visual.update(|_, cx| {
        assert_eq!(view.read(cx).header_clicks.get(), 1);
        assert_eq!(
            view.read(cx).body_clicks.get(),
            0,
            "row under header must not receive the click"
        );
    });
    visual.update(|window, cx| {
        view.read(cx).scroll.set_offset(point(px(0.), px(-700.)));
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let header = visual.debug_bounds("header").unwrap();
    assert_eq!(header.top(), px(-20.));
    assert_eq!(
        header.bottom(),
        px(20.),
        "header must leave with its table, before the footer"
    );
}
