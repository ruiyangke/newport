//! Keep the application surface out of accessibility navigation while a Kit
//! dialog owns interaction. Kit already supplies pointer occlusion and a Tab
//! focus trap; its overlay is a sibling of this surface in the window root.
use super::*;

/// Focus reveal for a single editor scroll region. Each scope must wrap a
/// direct child of the tracked scroll container, in the same row order.
/// Moving focus into a control reveals its field and associated help/error.
pub(super) struct FormScroll {
    pub(super) handle: ScrollHandle,
    focus: Vec<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl FormScroll {
    pub(super) fn new<T: 'static>(rows: usize, window: &mut Window, cx: &mut Context<T>) -> Self {
        let handle = ScrollHandle::new();
        let focus: Vec<_> = (0..rows)
            .map(|_| cx.focus_handle().tab_stop(false))
            .collect();
        let subscriptions = focus
            .iter()
            .enumerate()
            .map(|(row, focus)| {
                let handle = handle.clone();
                cx.on_focus_in(focus, window, move |_, _, cx| {
                    handle.scroll_to_item(row);
                    cx.notify();
                })
            })
            .collect();
        Self {
            handle,
            focus,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn row(&self, index: usize, content: impl IntoElement) -> impl IntoElement {
        div()
            .id(("form-scroll-row", index))
            .debug_selector(move || format!("form-scroll-row-{index}"))
            .flex_shrink_0()
            .track_focus(&self.focus[index])
            .child(content)
    }

    pub(super) fn reveal(&self, row: usize) {
        self.handle.scroll_to_item(row);
    }
}

impl Newport {
    pub(super) fn observe_modal_visibility(window: &Window, cx: &mut Context<Self>) {
        // The Kit root is installed after the application view is constructed.
        cx.defer_in(window, |this, window, cx| {
            let Some(root) = window.root::<gpui_kit::base::Root>().flatten() else {
                return;
            };
            this.sync_modal_visibility(window, cx);
            cx.observe_in(&root, window, |this, _, window, cx| {
                this.sync_modal_visibility(window, cx);
            })
            .detach();
        });
    }

    fn sync_modal_visibility(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = window.has_active_dialog(cx);
        if self.modal_open != open {
            self.modal_open = open;
            if open {
                self.server_picker.update(cx, |picker, cx| picker.close(cx));
            }
            cx.notify();
        }
    }
}
