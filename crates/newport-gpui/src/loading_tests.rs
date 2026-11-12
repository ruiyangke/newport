use crate::page_perf_tests::{frame, owner};
use crate::{Newport, theme};
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px, size,
};
use gpui_kit as gpui;
use newport_client::resources::PLACEHOLDER_DELAY;

struct StatusHost(Entity<Newport>);
impl Render for StatusHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.0.read(cx).status("probe", cx);
        div().size_full().child(status)
    }
}

#[gpui::test]
fn a_fast_read_draws_no_placeholder(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let entity = cx.new(|cx| owner(window, cx));
        view = Some(entity.clone());
        StatusHost(entity)
    });
    visual.simulate_resize(size(px(640.), px(480.)));
    let view = view.unwrap();
    view.update(visual, |v, cx| {
        v.resources.begin("probe");
        cx.notify();
    });
    frame(visual);
    assert!(visual.debug_bounds("loading-probe").is_none());
    std::thread::sleep(PLACEHOLDER_DELAY);
    view.update(visual, |_, cx| cx.notify());
    frame(visual);
    assert!(visual.debug_bounds("loading-probe").is_some());
}
