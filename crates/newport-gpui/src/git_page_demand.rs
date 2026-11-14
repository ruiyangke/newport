//! Bounded pagination from committed list geometry or page-scoped scroll demand.

/// Read committed list geometry rather than wheel events consumed by the list.
/// A newly appended page must be laid out before its end can request another.
pub(super) fn near_uniform_end(
    handle: &gpui_kit::UniformListScrollHandle,
    count: usize,
    row_height: f32,
) -> bool {
    use gpui_kit::px;
    let scroll = handle.0.borrow();
    let maximum = scroll.base_handle.max_offset().y;
    scroll
        .last_item_size
        .is_some_and(|size| size.contents.height == px(count as f32 * row_height))
        && maximum > px(0.)
        && scroll.base_handle.offset().y < px(0.)
        && maximum + scroll.base_handle.offset().y <= px(row_height * 2.)
}

/// A scroll gesture's request for the page after the one identified by `K`.
pub(super) struct Demand<K> {
    gesture: Option<K>,
    last_requested_offset: Option<gpui_kit::Pixels>,
    scroll: Option<gpui_kit::UniformListScrollHandle>,
}
impl<K> Default for Demand<K> {
    fn default() -> Self {
        Self {
            gesture: None,
            last_requested_offset: None,
            scroll: None,
        }
    }
}
impl<K: PartialEq> Demand<K> {
    pub(super) fn arm(&mut self, key: K) {
        self.gesture = Some(key);
    }
    /// Whether an armed gesture asks for the page after `page`, which is
    /// still the page it was armed on.
    pub(super) fn take(&mut self, page: &K, ready: bool, at_end: bool) -> bool {
        self.gesture
            .take()
            .is_some_and(|key| ready && at_end && key == *page)
    }
    pub(super) fn take_scrolled(
        &mut self,
        handle: &gpui_kit::UniformListScrollHandle,
        count: usize,
        row_height: f32,
        ready: bool,
    ) -> bool {
        if self
            .scroll
            .as_ref()
            .is_none_or(|previous| !std::rc::Rc::ptr_eq(&previous.0, &handle.0))
        {
            self.scroll = Some(handle.clone());
            self.last_requested_offset = None;
        }
        if !ready || !near_uniform_end(handle, count, row_height) {
            return false;
        }
        let offset = handle.0.borrow().base_handle.offset().y;
        if self.last_requested_offset == Some(offset) {
            return false;
        }
        self.last_requested_offset = Some(offset);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Demand;
    use crate::git_fixture::{history_page, remote_names};
    use crate::git_listing::{Identity, Listing};

    fn identity(page: &Listing) -> Option<Identity> {
        Some(page.identity())
    }

    #[test]
    fn a_scroll_cannot_drain_pages_or_cross_a_changed_listing() {
        let first = remote_names("s", &["one"], Some("two"), 3);
        let next = remote_names("s", &["one", "two"], Some("three"), 3);
        let mut demand = Demand::default();
        assert!(!demand.take(&identity(&first), true, true));
        demand.arm(identity(&first));
        assert!(demand.take(&identity(&first), true, true));
        assert!(!demand.take(&identity(&next), true, true));
        demand.arm(identity(&first));
        assert!(!demand.take(&identity(&next), true, true));
        demand.arm(identity(&first));
        let replaced = remote_names("s", &["one"], Some("two"), 4);
        assert!(!demand.take(&identity(&replaced), true, true));
        demand.arm(identity(&first));
        assert!(!demand.take(&None, true, true));
    }

    #[test]
    fn scrolling_during_a_read_or_away_from_the_end_does_not_queue_a_read() {
        let page = history_page("s", vec![crate::git_fixture::commit("a", "A")], Some("two"));
        let mut demand = Demand::default();
        for (ready, end) in [(false, true), (true, false)] {
            demand.arm(identity(&page));
            assert!(!demand.take(&identity(&page), ready, end));
            assert!(!demand.take(&identity(&page), true, true));
        }
    }
}

#[cfg(test)]
mod layout_tests {
    use super::Demand;
    use gpui_kit::{
        self as gpui, Context, IntoElement, Render, ScrollStrategy, TestAppContext,
        UniformListScrollHandle, Window, div, prelude::*, px, size, uniform_list,
    };

    struct Listing {
        count: usize,
        height: f32,
        scroll: UniformListScrollHandle,
        demand: Demand<()>,
    }
    impl Listing {
        fn take(&mut self, ready: bool) -> bool {
            self.demand
                .take_scrolled(&self.scroll, self.count, self.height, ready)
        }
    }
    impl Render for Listing {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            uniform_list(
                "rows",
                self.count,
                cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                    range.map(|_| div().h(px(this.height))).collect()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
        }
    }
    #[gpui::test]
    fn committed_scroll_is_bounded_across_row_sizes_and_filtered_pages(cx: &mut TestAppContext) {
        for height in [28., 36., 46., 48., 54.] {
            let (view, visual) = cx.add_window_view(|_, _| Listing {
                count: 50,
                height,
                scroll: UniformListScrollHandle::new(),
                demand: Demand::default(),
            });
            visual.simulate_resize(size(px(400.), px(300.)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            view.update(visual, |view, cx| {
                assert!(!view.take(true), "initial layout is not demand");
                view.scroll.scroll_to_item(49, ScrollStrategy::Bottom);
                cx.notify();
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            view.update(visual, |view, cx| {
                assert!(
                    !view.take(false),
                    "hidden, pending and failed views cannot request"
                );
                assert!(view.take(true));
                assert!(!view.take(true), "same bottom cannot request twice");
                cx.notify();
            });
            // A page containing no matching filtered entries leaves row count and geometry unchanged.
            visual.update(|window, cx| window.draw(cx).clear(cx));
            view.update(visual, |view, cx| {
                assert!(!view.take(true), "nonmatching pages cannot be drained");
                view.count = 100;
                assert!(!view.take(true), "old layout cannot request a new cursor");
                cx.notify();
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            view.update(visual, |view, cx| {
                assert!(!view.take(true));
                view.scroll.scroll_to_item(99, ScrollStrategy::Bottom);
                cx.notify();
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            view.update(visual, |view, cx| {
                assert!(view.take(true));
                view.scroll = UniformListScrollHandle::new();
                cx.notify();
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            view.update(visual, |view, cx| {
                assert!(!view.take(true));
                view.scroll.scroll_to_item(99, ScrollStrategy::Bottom);
                cx.notify();
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            view.update(visual, |view, _| {
                assert!(
                    view.take(true),
                    "new comparison may use the same bottom offset"
                )
            });
        }
    }
}
