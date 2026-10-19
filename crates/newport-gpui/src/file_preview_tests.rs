//! Render the real preview frame in a side pane, with no backend or file I/O.
use super::{Identity, Viewport, message, panel_frame};
use gpui_kit as gpui;
use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName;
use gpui_kit::{
    AnyElement, Context, Entity, IntoElement, Render, Window, div, prelude::*, px, size,
};
use gpui_kit::{TestAppContext, VisualTestContext};
use serde_json::json;

struct PreviewHost {
    identity: Identity,
    viewport: Viewport,
    error: Option<String>,
    loading: bool,
    width: f32,
}
impl Render for PreviewHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().child(div().flex_1()).child(
            div()
                .debug_selector(|| "side-pane".into())
                .w(px(self.width))
                .h_full()
                .flex()
                .flex_col()
                .min_h_0()
                .child(self.body(window, cx)),
        )
    }
}
impl PreviewHost {
    fn body(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_shrink_0()
            .child(
                crate::controls::icon_button("download")
                    .icon(IconName::Download)
                    .accessibility_label("Download"),
            )
            .child(
                crate::controls::icon_button("close")
                    .icon(IconName::X)
                    .accessibility_label("Close preview"),
            )
            .into_any_element();
        let body = if self.loading || self.error.is_some() {
            message(self.error.as_deref(), None, cx)
        } else {
            self.viewport.text.as_ref().unwrap().view(
                &"line of retained preview text\n".repeat(500),
                self.viewport.wrap,
                window,
                cx,
            )
        };
        let footer = crate::controls::button("wrap")
            .label("Wrap lines")
            .icon(IconName::TextWrap)
            .into_any_element();
        // The side pane passes its compact viewport, as `file_preview_side` does.
        panel_frame(
            &self.identity,
            actions,
            div().into_any_element(),
            body,
            footer,
            cx,
        )
    }
}

fn fixture(cx: &mut TestAppContext, width: f32) -> (Entity<PreviewHost>, &mut VisualTestContext) {
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
            viewport: Viewport {
                text: Some(super::text::PreviewText::new(window, cx)),
                ..Viewport::default()
            },
            error: None,
            loading: false,
            width,
        });
        view = Some(content.clone());
        gpui::base::Root::new(content, window, cx)
    });
    cx.simulate_resize(size(px(1280.), px(800.)));
    draw(cx);
    (view.unwrap(), cx)
}
fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
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
fn side_pane_keeps_identity_and_controls_inside_narrow_widths(cx: &mut TestAppContext) {
    for width in [320., 420., 620.] {
        let (view, cx) = fixture(cx, width);
        view.update(cx, |view, cx| {
            view.identity.name = "remote-report-with-a-very-long-file-name".repeat(4);
            cx.notify();
        });
        draw(cx);
        let pane = cx.debug_bounds("side-pane").unwrap();
        let header = cx.debug_bounds("file-preview-header").unwrap();
        let footer = cx.debug_bounds("file-preview-footer").unwrap();
        let text = cx.debug_bounds("file-preview-text").unwrap();
        assert!(header.bottom() <= text.top() + px(1.), "{width}");
        assert!(text.bottom() <= footer.top() + px(1.), "{width}");
        assert!(footer.bottom() <= pane.bottom() + px(1.), "{width}");
        assert!(
            labels(cx)
                .iter()
                .any(|l| l == "2.0 KiB · Permissions -rw-r--r--")
        );
        assert!(
            labels(cx)
                .iter()
                .any(|l| *l == "remote-report-with-a-very-long-file-name".repeat(4))
        );
        assert_eq!(
            header.size.height,
            px(crate::ui::HEADER_HEIGHT),
            "one pane header row"
        );
        let controls = cx.update(|window, _| {
            gpui::base::test_support::snapshots(window)
                .iter()
                .filter(|node| node.role() == Some(Role::Button))
                .filter(|node| {
                    matches!(
                        node.label(),
                        Some("Download" | "Close preview" | "Wrap lines")
                    )
                })
                .map(|node| node.bounds())
                .collect::<Vec<_>>()
        });
        assert_eq!(controls.len(), 3, "{width}");
        for control in controls {
            assert!(
                control.left() >= pane.left() && control.right() <= pane.right() + px(1.),
                "{width}: {control:?} {pane:?}"
            );
        }
    }
}

#[gpui::test]
fn loading_and_error_messages_keep_their_recovery(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx, 420.);
    view.update(cx, |view, cx| {
        view.loading = true;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("file-preview-skeleton").is_some());
    view.update(cx, |view, cx| {
        view.loading = false;
        view.error = Some("Only text and supported images can be previewed".into());
        cx.notify();
    });
    draw(cx);
    // The reason and "Download to open on your computer." are the shared
    // empty state's description; the pane replaces the loading lines.
    assert!(cx.debug_bounds("file-preview-skeleton").is_none());
    let message = cx.debug_bounds("file-preview-message").unwrap();
    assert!(message.size.width > px(0.) && message.size.height > px(0.));
}
