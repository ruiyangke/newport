use gpui::{
    Context, IntoElement, Modifiers, Render, TestAppContext, UniformListScrollHandle, Window, div,
    point, prelude::*, px, rgb, uniform_list,
};
use gpui_kit as gpui;
use newport_gpui::sticky_list_headers::StickyListHeaders;
use std::{cell::Cell, rc::Rc};

struct Harness {
    scroll: UniformListScrollHandle,
    header_focus: gpui::FocusHandle,
    row_focus: gpui::FocusHandle,
    header_clicks: Rc<Cell<usize>>,
    row_clicks: Rc<Cell<usize>>,
    rendered: Rc<Cell<usize>>,
}
impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let header_focus = self.header_focus.clone();
        let row_focus = self.row_focus.clone();
        let header_clicks = self.header_clicks.clone();
        let row_clicks = self.row_clicks.clone();
        let rendered = self.rendered.clone();
        div().w(px(300.)).h(px(112.)).child(
            uniform_list("list", 10_000, move |range, _, _| {
                rendered.set(range.len());
                range
                    .map(|index| {
                        let clicks = row_clicks.clone();
                        div()
                            .id(("row", index))
                            .h(px(28.))
                            .when(index != 0 && index != 4, |row| {
                                row.tab_group()
                                    .tab_index(index as isize)
                                    .when(index == 1, |row| row.track_focus(&row_focus))
                                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .size_full()
            .tab_group()
            .track_scroll(&self.scroll)
            .with_decoration(StickyListHeaders::new(
                vec![0, 4],
                move |index, _, _| {
                    let clicks = header_clicks.clone();
                    div()
                        .id(("header", index))
                        .when(index == 0, |header| header.track_focus(&header_focus))
                        .h(px(28.))
                        .w_full()
                        .bg(rgb(0xffffff))
                        .debug_selector(move || format!("header-{index}"))
                        .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                        .into_any_element()
                },
            )),
        )
    }
}
#[gpui::test]
fn virtualized_headers_pin_push_and_intercept_covered_row_clicks(cx: &mut TestAppContext) {
    let (view, visual) = cx.add_window_view(|_, cx| Harness {
        header_focus: cx.focus_handle().tab_stop(true),
        row_focus: cx.focus_handle().tab_stop(true),
        scroll: UniformListScrollHandle::new(),
        header_clicks: Rc::default(),
        row_clicks: Rc::default(),
        rendered: Rc::default(),
    });
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.blur(cx);
        window.focus_next(cx);
        assert!(view.read(cx).header_focus.is_focused(window));
        window.focus_next(cx);
        assert!(view.read(cx).row_focus.is_focused(window));
    });
    assert_eq!(visual.debug_bounds("header-0").unwrap().top(), px(0.));
    visual.update(|window, cx| {
        let state = view.read(cx);
        assert!(state.rendered.get() <= 5, "must retain virtualization");
        state
            .scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(-42.)));
        window.refresh();
        window.draw(cx).clear(cx);
    });
    assert_eq!(visual.debug_bounds("header-0").unwrap().top(), px(0.));
    assert_eq!(visual.debug_bounds("header-4").unwrap().top(), px(70.));
    visual.simulate_click(point(px(10.), px(10.)), Modifiers::default());
    visual.update(|window, cx| {
        let state = view.read(cx);
        assert_eq!(state.header_clicks.get(), 1);
        assert_eq!(
            state.row_clicks.get(),
            0,
            "covered row must not be clickable"
        );
        state
            .scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(-100.)));
        window.refresh();
        window.draw(cx).clear(cx);
    });
    assert_eq!(visual.debug_bounds("header-0").unwrap().top(), px(-16.));
    assert_eq!(visual.debug_bounds("header-4").unwrap().top(), px(12.));
    visual.update(|window, cx| {
        view.read(cx)
            .scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(-140.)));
        window.refresh();
        window.draw(cx).clear(cx);
    });
    assert!(visual.debug_bounds("header-0").is_none());
    assert_eq!(visual.debug_bounds("header-4").unwrap().top(), px(0.));
    visual.simulate_click(point(px(10.), px(10.)), Modifiers::default());
    visual.update(|_, cx| {
        assert_eq!(view.read(cx).header_clicks.get(), 2);
        assert_eq!(view.read(cx).row_clicks.get(), 0);
    });
}
