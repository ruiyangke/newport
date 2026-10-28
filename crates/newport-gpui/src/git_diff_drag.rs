//! Track range drags at the viewport, including after their first row virtualizes.
use super::{Actions, Gesture, range::DisplayRows};
use gpui_kit::{
    AnyElement, App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    UniformListScrollHandle, Window, point, prelude::*, px,
};
use std::{rc::Rc, time::Instant};

impl Actions {
    pub(crate) fn viewport(
        &self,
        lane: usize,
        rows: impl FnOnce() -> Vec<(usize, String)>,
        scroll: &UniformListScrollHandle,
        child: impl IntoElement,
    ) -> AnyElement {
        let rows = self.range_selection.borrow().display_rows[lane]
            .get_or_init(|| Rc::new(rows()))
            .clone();
        Viewport {
            actions: self.clone(),
            lane,
            rows,
            scroll: scroll.clone(),
            child: child.into_any_element(),
        }
        .into_any_element()
    }
}

#[derive(Clone)]
struct DragView {
    actions: Actions,
    lane: usize,
    rows: DisplayRows,
    scroll: UniformListScrollHandle,
    bounds: Bounds<Pixels>,
}
impl DragView {
    fn active(&self) -> bool {
        self.actions.enabled
            && self.actions.range_selection.borrow().active_lane() == Some(self.lane)
    }
    fn end(&self, cx: &mut App) {
        if self.active() {
            (self.actions.select_range)(self.lane, "", Gesture::End, cx);
        }
    }
    fn extend(&self, position: Point<Pixels>, cx: &mut App) {
        if !self.active() {
            return;
        }
        // Even a retained frame must validate the current repository/comparison.
        (self.actions.select_range)(self.lane, "", Gesture::Extend, cx);
        if !self.active()
            || self.bounds.size.height < px(1.)
            || position.x < self.bounds.left()
            || position.x >= self.bounds.right()
        {
            return;
        }
        let scroll = &self.scroll.0.borrow().base_handle;
        let y = position
            .y
            .clamp(self.bounds.top(), self.bounds.bottom() - px(1.));
        let row = ((y - self.bounds.top() - scroll.offset().y) / px(22.)).max(0.) as usize;
        let state = self.actions.range_selection.borrow();
        let Some(anchor) = state.anchor_index().and_then(|i| self.rows.get(i)) else {
            return;
        };
        let index = self.rows.partition_point(|(index, _)| *index < row);
        // Context/blank rows extend only through changed rows already crossed.
        let index =
            if self.rows.get(index).is_some_and(|(index, _)| *index == row) || row < anchor.0 {
                index.min(self.rows.len().saturating_sub(1))
            } else {
                index.saturating_sub(1)
            };
        let line = self.rows.get(index).map(|(_, line)| line.clone());
        drop(state);
        if let Some(line) = line {
            (self.actions.select_range)(self.lane, &line, Gesture::Extend, cx);
        }
    }
    fn speed(&self, pointer: Point<Pixels>) -> f32 {
        if !self.active()
            || self.bounds.size.height < px(1.)
            || pointer.x < self.bounds.left()
            || pointer.x >= self.bounds.right()
        {
            return 0.;
        }
        let edge = px(24.).min(self.bounds.size.height / 2.);
        if pointer.y < self.bounds.top() + edge {
            -700. * ((self.bounds.top() + edge - pointer.y) / edge).clamp(0., 1.)
        } else if pointer.y > self.bounds.bottom() - edge {
            700. * ((pointer.y - self.bounds.bottom() + edge) / edge).clamp(0., 1.)
        } else {
            0.
        }
    }
    fn tick(&self, elapsed: f32, window: &mut Window, cx: &mut App) {
        if !window.is_window_active() {
            self.end(cx);
            return;
        }
        let pointer = window.mouse_position();
        self.extend(pointer, cx);
        let speed = self.speed(pointer);
        if speed == 0. {
            return;
        }
        let scroll = self.scroll.0.borrow().base_handle.clone();
        let before = scroll.offset();
        let y = (before.y - px(speed * elapsed.min(0.05))).clamp(-scroll.max_offset().y, px(0.));
        if y != before.y {
            scroll.set_offset(point(before.x, y));
            self.extend(pointer, cx);
            window.refresh();
        }
    }
}
struct Viewport {
    actions: Actions,
    lane: usize,
    rows: DisplayRows,
    scroll: UniformListScrollHandle,
    child: AnyElement,
}
impl IntoElement for Viewport {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Viewport {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(("diff-range-viewport", self.lane).into())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        let view = DragView {
            actions: self.actions.clone(),
            lane: self.lane,
            rows: self.rows.clone(),
            scroll: self.scroll.clone(),
            bounds,
        };
        let moved = view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture && moved.active() {
                if event.pressed_button == Some(MouseButton::Left) {
                    moved.extend(event.position, cx);
                    window.refresh();
                } else {
                    moved.end(cx);
                }
            }
        });
        let released = view.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                released.end(cx);
            }
        });
        let speed = view.speed(window.mouse_position());
        let scroll = view.scroll.0.borrow().base_handle.clone();
        let can_scroll = (speed < 0. && scroll.offset().y < px(0.))
            || (speed > 0. && scroll.offset().y > -scroll.max_offset().y);
        let epoch = can_scroll
            .then(|| view.actions.range_selection.borrow_mut().reserve_frame())
            .flatten();
        if let Some(epoch) = epoch {
            let started = Instant::now();
            window.on_next_frame(move |window, cx| {
                if view.actions.range_selection.borrow_mut().take_frame(epoch) {
                    view.tick(started.elapsed().as_secs_f32(), window, cx);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        git_diff::{Presentation, Row},
        git_diff_split, git_diff_text,
    };
    use gpui_kit as gpui;
    use gpui_kit::base::test_support::snapshots;
    use gpui_kit::{Context, Entity, FocusHandle, Render, TestAppContext, VisualTestContext, div};
    use serde_json::json;
    use std::{cell::OnceCell, collections::BTreeSet, time::Duration};

    struct View {
        presentation: Rc<Presentation>,
        scroll: UniformListScrollHandle,
        cache: git_diff_text::Cache,
        focus: FocusHandle,
        split: bool,
        enabled: bool,
    }
    impl Render for View {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let p = &self.presentation;
            let review = p.review.as_ref().unwrap().clone();
            p.range_selection
                .borrow_mut()
                .prepare(p.lines.iter().filter_map(|row| {
                    row.selection
                        .as_ref()
                        .map(|(_, line)| (line.clone(), row.origin == "+"))
                }));
            let weak = cx.weak_entity();
            let actions = Actions {
                stage: true,
                enabled: self.enabled,
                review,
                selection: p.selection.clone(),
                range_selection: p.range_selection.clone(),
                run_states: Rc::default(),
                apply: Rc::new(|_, _, _| {}),
                toggle: Rc::new(|_, _| {}),
                discard: None,
                select_range: Rc::new(move |lane, line, gesture, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        let mut state = this.presentation.range_selection.borrow_mut();
                        if !this.enabled {
                            state.reset();
                            return;
                        }
                        state.apply(
                            lane,
                            line,
                            gesture,
                            &mut this.presentation.selection.borrow_mut(),
                        );
                        cx.notify();
                    });
                }),
            };
            let content = if self.split {
                git_diff_split::render(p.clone(), &self.scroll, Some(actions), None, cx)
            } else {
                let documents = self.cache.prepare(
                    json!(["snapshot"]),
                    p,
                    || {
                        vec![
                            p.lines
                                .iter()
                                .map(|row| git_diff_text::row(Some(row)))
                                .collect(),
                        ]
                    },
                    &self.focus,
                    window,
                    cx,
                );
                crate::git_diff::render_unified(
                    p.clone(),
                    &self.scroll,
                    Some(actions),
                    documents[0].clone(),
                    cx,
                )
            };
            div().w(px(700.)).h(px(220.)).flex().child(content)
        }
    }
    fn fixture(cx: &mut TestAppContext, split: bool) -> (Entity<View>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let lines: Vec<_> = (1..=100)
            .flat_map(|i| {
                [false, true].map(move |added| Row {
                    origin: if added { "+" } else { "-" }.into(),
                    old: if added { String::new() } else { i.to_string() },
                    new: if added { i.to_string() } else { String::new() },
                    selection: Some((
                        "h".into(),
                        format!("{}-{i}", if added { "add" } else { "del" }),
                    )),
                    bytes: format!("{} row {i}\n", if added { "new" } else { "old" }).into_bytes(),
                    ..Default::default()
                })
            })
            .collect();
        let review = newport_client::git_selection::Review::from_page(&json!({
            "nextCursor":null,"metadata":{"readOnly":false,"totalFiles":1,"sourceSnapshot":"status","entryId":"entry","side":"index_to_worktree","contextLines":3},
            "entries":[{"fileIndex":0,"status":"Modified","oldMode":33188,"newMode":33188,"oldPath":{"bytesB64":"Zg=="},"newPath":{"bytesB64":"Zg=="},"hunks":[{"id":"h","lines":lines.iter().map(|row| json!({"id":row.selection.as_ref().unwrap().1,"origin":row.origin,"lineComplete":true})).collect::<Vec<_>>()}]}]
        })).unwrap();
        let p = Rc::new(Presentation {
            lines,
            review: Ok(Rc::new(review)),
            selection: Rc::default(),
            range_selection: Rc::default(),
            error: None,
            split: OnceCell::new(),
            runs: OnceCell::new(),
            width: OnceCell::new(),
        });
        let (root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| View {
                presentation: p,
                scroll: UniformListScrollHandle::new(),
                cache: Default::default(),
                focus: cx.focus_handle(),
                split,
                enabled: true,
            });
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = root.read_with(cx, |root, _| {
            root.view().clone().downcast::<View>().unwrap()
        });
        cx.update(|window, _| window.activate_window());
        draw(cx);
        (view, cx)
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    fn frame(cx: &mut VisualTestContext) {
        std::thread::sleep(Duration::from_millis(16));
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        draw(cx);
    }
    fn number(cx: &mut VisualTestContext, label: &str) -> Point<Pixels> {
        cx.update(|window, _| {
            snapshots(window)
                .into_iter()
                .find(|node| node.label() == Some(label))
                .unwrap()
                .bounds()
                .center()
        })
    }
    fn selected(view: &Entity<View>, cx: &VisualTestContext) -> BTreeSet<String> {
        view.read_with(cx, |v, _| v.presentation.selection.borrow().clone())
    }
    #[gpui::test]
    fn range_drag_scrolls_past_virtualized_rows_and_releases_outside(cx: &mut TestAppContext) {
        for split in [false, true] {
            let (view, cx) = fixture(cx, split);
            let from = number(cx, "Stage added line 1");
            cx.simulate_mouse_down(from, MouseButton::Left, Default::default());
            draw(cx);
            let bottom =
                view.read_with(cx, |v, _| v.scroll.0.borrow().base_handle.bounds().bottom());
            let below = point(from.x, bottom + px(20.));
            cx.simulate_mouse_move(below, Some(MouseButton::Left), Default::default());
            draw(cx);
            for _ in 0..25 {
                frame(cx);
            }
            let offset = view.read_with(cx, |v, _| v.scroll.0.borrow().base_handle.offset().y);
            assert!(offset < px(-100.), "edge drag must scroll: {offset:?}");
            assert!(
                selected(&view, cx).len() > 12,
                "drag must extend beyond initially visible rows"
            );
            assert!(selected(&view, cx).contains("add-1"));
            if split {
                assert!(selected(&view, cx).iter().all(|id| id.starts_with("add-")));
            }
            // The original row is gone; release outside the whole diff must still end it.
            let outside = point(px(750.), below.y);
            cx.simulate_mouse_up(outside, MouseButton::Left, Default::default());
            draw(cx);
            assert!(view.read_with(cx, |v, _| {
                v.presentation
                    .range_selection
                    .borrow()
                    .active_lane()
                    .is_none()
            }));
            let before = selected(&view, cx);
            for _ in 0..3 {
                frame(cx);
            }
            assert_eq!(selected(&view, cx), before);
            assert_eq!(
                view.read_with(cx, |v, _| v.scroll.0.borrow().base_handle.offset().y),
                offset
            );
        }
    }
    #[gpui::test]
    fn top_edge_retracts_selection_and_stops_at_the_start(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, true);
        let from = number(cx, "Stage added line 1");
        cx.simulate_mouse_down(from, MouseButton::Left, Default::default());
        draw(cx);
        cx.simulate_mouse_move(
            point(from.x, px(250.)),
            Some(MouseButton::Left),
            Default::default(),
        );
        draw(cx);
        for _ in 0..15 {
            frame(cx);
        }
        assert!(selected(&view, cx).len() > 10);
        cx.simulate_mouse_move(
            point(from.x, px(0.)),
            Some(MouseButton::Left),
            Default::default(),
        );
        draw(cx);
        for _ in 0..25 {
            frame(cx);
        }
        assert_eq!(
            view.read_with(cx, |v, _| v.scroll.0.borrow().base_handle.offset().y),
            px(0.)
        );
        assert_eq!(selected(&view, cx), BTreeSet::from(["add-1".to_owned()]));
        // There is no perpetual animation once the scroll boundary is reached.
        assert!(view.read_with(cx, |v, _| {
            v.presentation
                .range_selection
                .borrow_mut()
                .reserve_frame()
                .is_some()
        }));
        cx.simulate_mouse_up(point(from.x, px(0.)), MouseButton::Left, Default::default());
        draw(cx);
    }
    #[gpui::test]
    fn obsolete_drag_frame_does_not_scroll_or_change_selection(cx: &mut TestAppContext) {
        let (view, cx) = fixture(cx, false);
        let from = number(cx, "Stage added line 1");
        cx.simulate_mouse_down(from, MouseButton::Left, Default::default());
        draw(cx);
        cx.simulate_mouse_move(
            point(from.x, px(250.)),
            Some(MouseButton::Left),
            Default::default(),
        );
        draw(cx);
        let before = selected(&view, cx);
        let offset = view.read_with(cx, |v, _| v.scroll.0.borrow().base_handle.offset());
        view.update(cx, |v, cx| {
            v.enabled = false;
            cx.notify();
        });
        frame(cx);
        assert_eq!(selected(&view, cx), before);
        assert_eq!(
            view.read_with(cx, |v, _| v.scroll.0.borrow().base_handle.offset()),
            offset
        );
        assert!(view.read_with(cx, |v, _| {
            v.presentation
                .range_selection
                .borrow()
                .active_lane()
                .is_none()
        }));
    }
}
