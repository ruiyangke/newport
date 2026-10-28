use gpui::{Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px, size};
use gpui_kit as gpui;
struct Editor;
impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(crate::project_dialog::scroll_body(
                super::editor(
                    "Save changes",
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .children(
                            (0..12).map(|_| div().h(px(28.)).flex_shrink_0().child("Form field")),
                        )
                        .child(
                            div()
                                .h(px(28.))
                                .flex_shrink_0()
                                .debug_selector(|| "submit-row".into())
                                .child("Save / Cancel"),
                        ),
                    cx,
                )
                .into_any_element(),
                window.viewport_size().height,
            ))
    }
}
#[gpui::test]
fn modal_body_scrolls_actions_into_short_narrow_panes(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui::component::init(cx);
        crate::theme::install(false, cx);
    });
    let (_, visual) = cx.add_window_view(|_, _| Editor);
    visual.simulate_resize(size(px(260.), px(160.)));
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(visual.debug_bounds("submit-row").unwrap().top() > px(160.));
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: gpui::point(px(120.), px(80.)),
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-2000.))),
        ..Default::default()
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let row = visual.debug_bounds("submit-row").unwrap();
    assert!(
        row.top() >= px(0.) && row.bottom() <= px(160.),
        "actions must remain reachable: {row:?}"
    );
    assert!(row.right() <= px(260.));
}
