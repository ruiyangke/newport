//! Render the real preview frame in Kit's modal root, with no backend or file I/O.
use super::{
    Identity, ModalLease, Viewport, dialog_frame, dismiss_owned, message, panel_frame,
    preserve_opener,
};
use gpui_kit as gpui;
use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName;
use gpui_kit::component::WindowExt;
use gpui_kit::{
    AnyElement, Context, Entity, FocusHandle, IntoElement, Pixels, Render, Size, Window, div,
    point, prelude::*, px, size,
};
use gpui_kit::{TestAppContext, VisualTestContext};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct PreviewHost {
    identity: Identity,
    viewport: Viewport,
    error: Option<String>,
    loading: bool,
    closed: usize,
    opener: FocusHandle,
    other: FocusHandle,
}
impl Render for PreviewHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                crate::controls::button("open-preview")
                    .label("Open preview")
                    .track_focus(&self.opener)
                    .on_click(cx.listener(|this, _, window, cx| this.open(window, cx))),
            )
            .child(
                crate::controls::button("other-file-row")
                    .label("Other file")
                    .track_focus(&self.other),
            )
    }
}
impl PreviewHost {
    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let live = Rc::new(Cell::new(true));
        let handle = Rc::new(RefCell::new(None));
        self.viewport.modal = Some(ModalLease {
            live: live.clone(),
            handle: handle.clone(),
        });
        self.viewport.text = Some(super::text::PreviewText::new(window, cx));
        preserve_opener(&self.opener, window, cx);
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            if !live.get() {
                dismiss_owned(&handle, window, cx);
                return dialog;
            }
            let name = view.read(cx).identity.name.clone();
            let close_view = view.clone();
            let close_live = live.clone();
            let body_view = view.clone();
            dialog_frame(dialog, window.viewport_size(), name, cx)
                .on_close(move |_, _, cx| {
                    if close_live.get() {
                        close_view.update(cx, |view, cx| {
                            view.viewport = Viewport::default();
                            view.closed += 1;
                            cx.notify();
                        });
                    }
                })
                .content(move |content, window, cx| {
                    content.size_full().min_h_0().gap_0().p_0().child(
                        body_view
                            .update(cx, |view, cx| view.body(window.viewport_size(), window, cx)),
                    )
                })
        });
        *self.viewport.modal.as_ref().unwrap().handle.borrow_mut() =
            window.active_dialog_handle(cx);
    }
    fn body(
        &self,
        viewport: Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_shrink_0()
            .child(
                crate::controls::button("download")
                    .label("Download")
                    .icon(IconName::Download),
            )
            .child(
                crate::controls::icon_button("close")
                    .icon(IconName::X)
                    .accessibility_label("Close dialog")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let handle = this
                            .viewport
                            .modal
                            .as_ref()
                            .unwrap()
                            .handle
                            .borrow()
                            .clone()
                            .unwrap();
                        window.close_dialog_if(handle.focus_handle(), cx);
                        this.viewport = Viewport::default();
                        this.closed += 1;
                        cx.notify();
                    })),
            )
            .into_any_element();
        let body = if self.loading || self.error.is_some() {
            message(self.error.as_deref(), cx)
        } else {
            self.viewport.text.as_ref().unwrap().view(
                &"line of retained preview text\n".repeat(500),
                self.viewport.wrap,
                window,
                cx,
            )
        };
        let actions_footer = crate::controls::button("wrap")
            .label("Wrap lines")
            .icon(IconName::TextWrap)
            .toggled(self.viewport.wrap)
            .on_click(cx.listener(|this, _, _, cx| {
                this.viewport.wrap = !this.viewport.wrap;
                cx.notify();
            }))
            .into_any_element();
        panel_frame(
            &self.identity,
            viewport,
            actions,
            div().into_any_element(),
            body,
            actions_footer,
            cx,
        )
    }
}

fn fixture(cx: &mut TestAppContext) -> (Entity<PreviewHost>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui::component::init(cx);
        crate::theme::install(false, cx);
        cx.set_reduce_motion(true);
    });
    let mut view = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let content = cx.new(|cx| PreviewHost {
            identity: Identity::from_entry(&json!({
                "name":"remote-report.unknown-extension", "path":"/srv/work/remote-report.unknown-extension",
                "kind":"file", "size":2048, "permissions":"-rw-r--r--"
            })),
            viewport: Viewport::default(), error: None, loading: false, closed: 0,
            opener: cx.focus_handle(),
            other: cx.focus_handle(),
        });
        view = Some(content.clone());
        gpui::base::Root::new(content, window, cx)
    });
    cx.update(|window, _| {
        window.activate_window();
    });
    cx.simulate_resize(size(px(1280.), px(900.)));
    let view = view.unwrap();
    view.update_in(cx, |view, window, cx| view.open(window, cx));
    draw(cx);
    (view, cx)
}
fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
}
fn click(cx: &mut VisualTestContext, label: &str) {
    let at = cx.update(|window, _| {
        gpui::base::test_support::snapshots(window)
            .iter()
            .find(|node| node.role() == Some(Role::Button) && node.label() == Some(label))
            .unwrap_or_else(|| panic!("missing button: {label}"))
            .bounds()
            .center()
    });
    cx.simulate_click(at, Default::default());
    draw(cx);
}
fn labels(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, _| {
        gpui::base::test_support::snapshots(window)
            .iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect()
    })
}

#[gpui::test]
fn preview_modal_matches_desktop_geometry_and_keeps_footer_with_long_text(cx: &mut TestAppContext) {
    let (_, cx) = fixture(cx);
    let panel = cx.debug_bounds("file-preview-panel").unwrap();
    assert!((panel.size.width - px(1040.)).abs() <= px(2.), "{panel:?}");
    assert!((panel.size.height - px(760.)).abs() <= px(2.), "{panel:?}");
    assert!((panel.center().x - px(640.)).abs() <= px(1.));
    assert!((panel.center().y - px(450.)).abs() <= px(1.));
    let header = cx.debug_bounds("file-preview-header").unwrap();
    let footer = cx.debug_bounds("file-preview-footer").unwrap();
    let text = cx.debug_bounds("file-preview-text").unwrap();
    assert!(header.bottom() <= text.top() + px(1.));
    assert!(text.bottom() <= footer.top() + px(1.));
    assert!(footer.bottom() <= panel.bottom() + px(1.));
    assert!(
        labels(cx)
            .iter()
            .any(|label| label == "2.0 KiB · Permissions -rw-r--r--")
    );
    assert!(
        labels(cx)
            .iter()
            .any(|label| label == "/srv/work/remote-report.unknown-extension")
    );
}

#[gpui::test]
fn compact_modal_keeps_identity_and_controls_inside_window(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |view, cx| {
        view.identity.name = "remote-report-with-a-very-long-file-name".repeat(6);
        view.identity.path = format!("/srv/{}/file", "long-directory/".repeat(30));
        cx.notify();
    });
    for (width, height) in [(640., 480.), (800., 600.), (960., 680.), (1280., 900.)] {
        cx.simulate_resize(size(px(width), px(height)));
        draw(cx);
        let panel = cx.debug_bounds("file-preview-panel").unwrap();
        let header = cx.debug_bounds("file-preview-header").unwrap();
        let footer = cx.debug_bounds("file-preview-footer").unwrap();
        assert!(
            panel.left() >= px(11.) && panel.right() <= px(width - 11.),
            "{panel:?}"
        );
        assert!(
            panel.top() >= px(11.) && panel.bottom() <= px(height - 11.),
            "{panel:?}"
        );
        assert!(
            header.bottom() < footer.top(),
            "content must have remaining space"
        );
        assert!(footer.bottom() <= panel.bottom() + px(1.));
        let controls = cx.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .iter()
                .filter(|node| node.role() == Some(Role::Button))
                .filter(|node| {
                    matches!(
                        node.label(),
                        Some("Download" | "Close dialog" | "Wrap lines")
                    )
                })
                .map(|node| node.bounds())
                .collect::<Vec<_>>()
        });
        assert_eq!(controls.len(), 3);
        for control in controls {
            assert!(
                control.left() >= panel.left() && control.right() <= panel.right() + px(1.),
                "{control:?} {panel:?}"
            );
            assert!(control.bottom() <= panel.bottom() + px(1.));
        }
    }
}

#[gpui::test]
fn enter_keeps_preview_open_and_escape_restores_focus(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |view, cx| {
        view.viewport.wrap = true;
        cx.notify();
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(
        cx.update(|window, cx| window.has_active_dialog(cx)),
        "bare Enter must not dismiss a preview"
    );
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(view.read_with(cx, |view, _| view.closed), 1);
    assert!(cx.update(|window, cx| view.read(cx).opener.is_focused(window)));
}

#[gpui::test]
fn direct_file_open_preserves_the_original_focused_row(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    click(cx, "Close dialog");
    view.update_in(cx, |view, window, cx| {
        view.other.focus(window, cx);
        view.open(window, cx);
    });
    draw(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.update(|window, cx| view.read(cx).other.is_focused(window)));
    assert!(!cx.update(|window, cx| view.read(cx).opener.is_focused(window)));
}

#[gpui::test]
fn tab_focus_stays_in_the_preview(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    for key in [
        "tab",
        "tab",
        "tab",
        "tab",
        "tab",
        "shift-tab",
        "shift-tab",
        "shift-tab",
        "shift-tab",
        "shift-tab",
    ] {
        cx.simulate_keystrokes(key);
        draw(cx);
        assert!(cx.update(|window, cx| {
            window
                .active_dialog_handle(cx)
                .unwrap()
                .focus_handle()
                .contains_focused(window, cx)
        }));
        assert!(!cx.update(|window, cx| view.read(cx).opener.is_focused(window)));
    }
}

#[gpui::test]
fn backdrop_and_close_dismiss_without_losing_file_metadata(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    click(cx, "Close dialog");
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    view.update_in(cx, |view, window, cx| view.open(window, cx));
    draw(cx);
    cx.simulate_click(point(px(5.), px(450.)), Default::default());
    draw(cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(view.read_with(cx, |view, _| view.closed), 2);
    assert_eq!(
        view.read_with(cx, |view, _| view.identity.metadata.clone()),
        "2.0 KiB · Permissions -rw-r--r--"
    );
}

#[gpui::test]
fn loading_and_error_keep_the_same_dialog_and_recovery_action(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |view, cx| {
        view.loading = true;
        cx.notify();
    });
    draw(cx);
    let before = cx.debug_bounds("file-preview-panel").unwrap();
    assert!(labels(cx).iter().any(|label| label == "Loading preview…"));
    let skeleton = cx.debug_bounds("file-preview-skeleton").unwrap();
    assert!(skeleton.size.width > px(100.) && skeleton.size.height > px(30.));
    view.update(cx, |view, cx| {
        view.error = Some("Permission denied".into());
        cx.notify();
    });
    draw(cx);
    let after = cx.debug_bounds("file-preview-panel").unwrap();
    assert_eq!(before, after);
    assert!(labels(cx).iter().any(|label| label == "Permission denied"));
    assert!(cx.debug_bounds("file-preview-skeleton").is_none());
    assert!(
        labels(cx)
            .iter()
            .any(|label| label == "Download to open on your computer.")
    );
    assert!(!labels(cx).iter().any(|label| label == "Loading preview…"));
    assert!(labels(cx).iter().any(|label| label == "Download"));
}

#[gpui::test]
fn retired_preview_dismisses_only_its_owned_dialog(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let owned = view.read_with(cx, |view, _| {
        view.viewport.modal.as_ref().unwrap().handle.clone()
    });
    cx.update(|window, cx| {
        window.open_dialog(cx, |dialog, _, _| {
            dialog
                .aria_label("Replacement dialog")
                .title("Replacement dialog")
        })
    });
    draw(cx);
    view.update(cx, |view, cx| {
        view.viewport = Viewport::default();
        cx.notify();
    });
    draw(cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(labels(cx).iter().any(|label| label == "Replacement dialog"));
    assert_eq!(view.read_with(cx, |view, _| view.closed), 0);
    assert!(!owned.borrow().as_ref().unwrap().is_active());
}
