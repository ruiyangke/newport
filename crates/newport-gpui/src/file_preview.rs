//! Retained file preview presented beside the folder list.
use super::*;
#[path = "file_preview_animation.rs"]
mod animation;
#[path = "file_preview_text.rs"]
mod text;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{Selectable, button::ButtonGroup};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone)]
struct Identity {
    name: String,
    path: String,
    kind: String,
    metadata: String,
}
impl Identity {
    fn from_entry(entry: &Value) -> Self {
        let size = if entry["size"].is_null() {
            "—".into()
        } else {
            bytes(&entry["size"])
        };
        let permissions = text(entry, "permissions");
        Self {
            name: text(entry, "name"),
            path: text(entry, "path"),
            kind: text(entry, "kind"),
            metadata: if permissions.is_empty() {
                size
            } else {
                format!("{size} · Permissions {permissions}")
            },
        }
    }
}

/// One row, the height of the list's column header beside it. The full path
/// is on hover; the folder is already the page's location.
fn header(identity: &Identity, actions: AnyElement, cx: &App) -> AnyElement {
    div()
        .id("file-preview-header")
        .debug_selector(|| "file-preview-header".into())
        .h(px(28.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(8.))
        .pl(px(12.))
        .pr(px(4.))
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .child(file_icon::icon(&identity.name, &identity.kind)),
        )
        .child(
            div()
                .id("file-preview-title")
                .role(Role::Heading)
                .aria_label(identity.name.clone())
                .aria_description(identity.path.clone())
                .flex_1()
                .min_w_0()
                .text_size(px(crate::ui::FONT_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .child(identity.name.clone())
                .hover_hint(identity.path.clone())
                .test_support(),
        )
        .child(actions)
        .into_any_element()
}

fn footer(identity: &Identity, actions: AnyElement, cx: &App) -> AnyElement {
    div()
        .id("file-preview-footer")
        .debug_selector(|| "file-preview-footer".into())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .h(px(32.))
        .px(px(12.))
        .flex_shrink_0()
        .border_t_1()
        .border_color(cx.theme().border)
        .text_size(px(crate::ui::FONT_SIZE))
        .text_color(cx.theme().muted_foreground)
        .child(
            div()
                .id("file-preview-metadata")
                .role(Role::Label)
                .aria_label(identity.metadata.clone())
                .min_w_0()
                .flex_1()
                .text_ellipsis()
                .child(identity.metadata.clone())
                .test_support(),
        )
        .child(actions)
        .into_any_element()
}

fn panel_frame(
    identity: &Identity,
    actions: AnyElement,
    transfer: AnyElement,
    body: AnyElement,
    footer_actions: AnyElement,
    cx: &App,
) -> AnyElement {
    div()
        .id("file-preview-panel")
        .debug_selector(|| "file-preview-panel".into())
        .size_full()
        .flex()
        .flex_col()
        .min_h_0()
        .overflow_hidden()
        .child(header(identity, actions, cx))
        .child(body)
        .child(transfer)
        .child(footer(identity, footer_actions, cx))
        .into_any_element()
}

fn message(error: Option<&str>, cx: &App) -> AnyElement {
    div()
        .id("file-preview-message")
        .debug_selector(|| "file-preview-message".into())
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .overflow_y_scroll()
        .p(px(28.))
        .text_size(px(crate::ui::FONT_SIZE))
        .child(
            div()
                .w_full()
                .max_w(px(560.))
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .when_some(error, |view, error| {
                    view.child(
                        div()
                            .id("file-preview-error")
                            .role(Role::Alert)
                            .aria_label(error.to_owned())
                            .pb(px(8.))
                            .text_color(cx.theme().danger)
                            .child(error.to_owned())
                            .test_support(),
                    )
                    .child(
                        div()
                            .id("file-preview-recovery")
                            .role(Role::Label)
                            .aria_label("Download to open on your computer.")
                            .child("Download to open on your computer.")
                            .test_support(),
                    )
                })
                .when(error.is_none(), |view| {
                    view.child(
                        div()
                            .id("file-preview-loading")
                            .role(Role::Status)
                            .aria_label("Loading preview…")
                            .debug_selector(|| "file-preview-skeleton".into())
                            .w_full()
                            .child(loading::lines("file-preview", 3))
                            .test_support(),
                    )
                }),
        )
        .into_any_element()
}

const ZOOM_STEPS: [u16; 7] = [25, 50, 75, 100, 150, 200, 400];
#[derive(Default)]
pub struct Viewport {
    pub zoom: Option<u16>,
    pub wrap: bool,
    pub copied: bool,
    pub(super) content_focus: Option<FocusHandle>,
    text: Option<text::PreviewText>,
    animation: animation::Clock,
}
impl Viewport {
    fn step(&mut self, increase: bool) {
        let current = self.zoom.unwrap_or(100);
        self.zoom = Some(if increase {
            ZOOM_STEPS
                .iter()
                .copied()
                .find(|step| *step > current)
                .unwrap_or(400)
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|step| *step < current)
                .unwrap_or(25)
        });
    }
}
impl Newport {
    fn retry_file_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.file_preview_entry();
        let Some(entry) = entry else { return };
        self.close_file_preview();
        if let Some(browser) = self.file_browsers.get_mut(&self.server) {
            let path = entry["path"].as_str().unwrap_or_default().to_owned();
            if browser.reads.open_preview(path.clone()).is_some() {
                self.file_preview_path = Some(path);
                self.prepare_file_preview(&entry, window, cx);
            }
        }
        cx.notify();
    }
    fn file_preview_entry(&self) -> Option<Value> {
        let path = self.file_preview_path.as_ref()?;
        self.file_browsers.get(&self.server)?.reads.listing()?["entries"]
            .as_array()?
            .iter()
            .find(|entry| entry["path"] == *path)
            .cloned()
    }
    /// Reset presentation for a newly selected file; reads are already open.
    pub(super) fn prepare_file_preview(
        &mut self,
        entry: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let identity = Identity::from_entry(entry);
        self.file_preview_viewport.content_focus = Some(cx.focus_handle().tab_stop(true));
        self.file_preview_viewport.text =
            Some(text::PreviewText::for_path(&identity.path, window, cx));
    }
    pub(super) fn file_preview_side(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.file_preview_entry() else {
            return div().into_any_element();
        };
        let identity = Identity::from_entry(&entry);
        self.file_preview_panel(&identity, window, cx)
    }

    fn file_preview_panel(
        &self,
        identity: &Identity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(target) = self.file_preview_target() else {
            return div().into_any_element();
        };
        let data = self
            .resources
            .get("preview")
            .map(|row| &row.value)
            .unwrap_or(&Value::Null);
        let zoom = self.file_preview_viewport.zoom;
        let name = &identity.name;
        let download = self.file_action_target(&identity.path).filter(|target| {
            target.supports_transfer(newport_client::file_transfer::Direction::Download)
        });
        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_shrink_0()
            .child(
                crate::controls::compact_icon_button("files-download-preview")
                    .ghost()
                    .icon(IconName::Download)
                    .tooltip("Download")
                    .accessibility_label("Download")
                    .disabled(self.file_transfer_busy() || download.is_none())
                    .on_click({
                        let target = target.clone();
                        cx.listener(move |this, _, _, cx| {
                            if !this.file_preview_target_current(&target) {
                                return;
                            }
                            if let Some(target) = &download {
                                this.start_file_transfer(
                                    target,
                                    newport_client::file_transfer::Direction::Download,
                                    cx,
                                );
                            }
                        })
                    }),
            )
            .child(
                crate::controls::compact_icon_button("files-close-preview")
                    .icon(IconName::X)
                    .ghost()
                    .tooltip("Close preview")
                    .accessibility_label("Close preview")
                    .on_click({
                        let target = target.clone();
                        cx.listener(move |this, _, window, cx| {
                            if !this.file_preview_target_current(&target) {
                                return;
                            }
                            this.close_file_preview();
                            if let Some(browser) = this.file_browsers.get(&this.server) {
                                browser.list_focus.focus(window, cx);
                            }
                            cx.notify();
                        })
                    }),
            )
            .into_any_element();
        let content = div()
            .id("file-preview-content")
            .debug_selector(|| "file-preview-content".into())
            .role(Role::Region)
            .aria_label("File preview")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .when_some(
                self.file_preview_viewport.content_focus.as_ref(),
                |view, focus| view.track_focus(focus).tab_stop(true),
            );
        let error = self.file_preview_error();
        let body = content
            .when(data.is_null() || error.is_some(), |view| {
                view.child(message(error, cx)).when(
                    error.is_some_and(|error| {
                        !error.contains("Only text and supported images can be previewed")
                    }),
                    |view| {
                        view.child(
                            div().flex().justify_center().pb_4().child(
                                crate::controls::button("file-preview-retry")
                                    .label("Retry preview")
                                    .on_click({
                                        let target = target.clone();
                                        cx.listener(move |this, _, window, cx| {
                                            if this.file_preview_target_current(&target) {
                                                this.retry_file_preview(window, cx);
                                            }
                                        })
                                    }),
                            ),
                        )
                    },
                )
            })
            .when(error.is_none() && data["truncated"] == true, |panel| {
                panel.child(
                    div()
                        .id("file-preview-truncated")
                        .role(Role::Status)
                        .aria_label(
                            "Showing the first 1 MiB. Search and copy apply to this preview.",
                        )
                        .px_4()
                        .py_2()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(cx.theme().muted_foreground)
                        .child("Showing the first 1 MiB · Search and copy apply to this preview"),
                )
            })
            .when(error.is_none() && data["kind"] == "text", |panel| {
                let content = data["content"].as_str().unwrap_or_default();
                let wrap = self.file_preview_viewport.wrap;
                panel.when_some(
                    self.file_preview_viewport.text.as_ref(),
                    |panel, preview| panel.child(preview.view(content, wrap, window, cx)),
                )
            })
            .when_some(
                self.file_preview_image
                    .clone()
                    .filter(|_| error.is_none() && data["kind"] == "image"),
                |panel, image| {
                    let raster = animation::raster(
                        image,
                        data["animationPlays"].as_u64(),
                        self.file_preview_viewport.animation.clone(),
                        zoom.is_none(),
                    );
                    panel
                        .child(
                            div()
                                .id(if zoom.is_none() {
                                    "image-preview-fit"
                                } else {
                                    "image-preview-viewport"
                                })
                                .role(Role::Region)
                                .aria_label(format!("File preview: {name}"))
                                .flex_1()
                                .min_h_0()
                                .w_full()
                                .overflow_scroll()
                                .bg(cx.theme().muted)
                                .when(zoom.is_none(), |frame| frame.relative().overflow_hidden())
                                .child(if let Some(percent) = zoom {
                                    let scale = f32::from(percent) / 100.;
                                    let width =
                                        px(data["width"].as_u64().unwrap_or(1) as f32 * scale);
                                    let height =
                                        px(data["height"].as_u64().unwrap_or(1) as f32 * scale);
                                    // Center within the viewport when the raster fits. Let
                                    // the canvas grow with larger zooms so its leading edge
                                    // remains reachable instead of overflowing negatively.
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .min_w_full()
                                        .min_h_full()
                                        .w(width)
                                        .h(height)
                                        .child(
                                            div().w(width).h(height).flex_shrink_0().child(raster),
                                        )
                                        .into_any_element()
                                } else {
                                    raster
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_4()
                                .py_2()
                                .flex_shrink_0()
                                .child(
                                    label(format!("{} × {} pixels", data["width"], data["height"]))
                                        .text_size(px(crate::ui::FONT_SIZE)),
                                )
                                .child(div().flex_1())
                                .child(
                                    ButtonGroup::new("image-sizing")
                                        .children([
                                            crate::controls::button("image-fit")
                                                .label("Fit")
                                                .selected(zoom.is_none()),
                                            crate::controls::button("image-actual")
                                                .label("100%")
                                                .selected(zoom == Some(100)),
                                        ])
                                        .on_click({
                                            let target = target.clone();
                                            cx.listener(
                                                move |this, selected: &Vec<usize>, _, cx| {
                                                    if !this.file_preview_target_current(&target) {
                                                        return;
                                                    }
                                                    if let Some(index) = selected.first() {
                                                        this.file_preview_viewport.zoom =
                                                            if *index == 0 {
                                                                None
                                                            } else {
                                                                Some(100)
                                                            };
                                                        cx.notify();
                                                    }
                                                },
                                            )
                                        }),
                                )
                                .child(
                                    crate::controls::icon_button("image-zoom-out")
                                        .icon(IconName::ZoomOut)
                                        .ghost()
                                        .accessibility_label("Zoom out")
                                        .disabled(zoom == Some(25))
                                        .on_click({
                                            let target = target.clone();
                                            cx.listener(move |this, _, _, cx| {
                                                if !this.file_preview_target_current(&target) {
                                                    return;
                                                }
                                                this.file_preview_viewport.step(false);
                                                cx.notify();
                                            })
                                        }),
                                )
                                .child(
                                    label(
                                        zoom.map(|z| format!("{z}%"))
                                            .unwrap_or_else(|| "Fit".into()),
                                    )
                                    .text_size(px(crate::ui::FONT_SIZE)),
                                )
                                .child(
                                    crate::controls::icon_button("image-zoom-in")
                                        .icon(IconName::ZoomIn)
                                        .ghost()
                                        .accessibility_label("Zoom in")
                                        .disabled(zoom == Some(400))
                                        .on_click({
                                            let target = target.clone();
                                            cx.listener(move |this, _, _, cx| {
                                                if !this.file_preview_target_current(&target) {
                                                    return;
                                                }
                                                this.file_preview_viewport.step(true);
                                                cx.notify();
                                            })
                                        }),
                                ),
                        )
                },
            )
            .into_any_element();
        let footer_actions = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_shrink_0()
            .when(data["kind"] == "text", |row| {
                let wrap = self.file_preview_viewport.wrap;
                row.child(
                    crate::controls::compact_button("text-copy")
                        .label(if self.file_preview_viewport.copied {
                            "Copied"
                        } else {
                            "Copy text"
                        })
                        .icon(IconName::Copy)
                        .ghost()
                        .on_click({
                            let target = target.clone();
                            cx.listener(move |this, _, _, cx| {
                                if !this.file_preview_target_current(&target) {
                                    return;
                                }
                                let content = this
                                    .resources
                                    .get("preview")
                                    .and_then(|row| row.value["content"].as_str())
                                    .unwrap_or_default()
                                    .to_owned();
                                cx.write_to_clipboard(ClipboardItem::new_string(content));
                                this.file_preview_viewport.copied = true;
                                cx.notify();
                            })
                        }),
                )
                .child(
                    crate::controls::compact_button("text-wrap")
                        .label("Wrap lines")
                        .icon(IconName::TextWrap)
                        .ghost()
                        .toggled(wrap)
                        .on_click({
                            let target = target.clone();
                            cx.listener(move |this, _, _, cx| {
                                if !this.file_preview_target_current(&target) {
                                    return;
                                }
                                this.file_preview_viewport.wrap = !this.file_preview_viewport.wrap;
                                cx.notify();
                            })
                        }),
                )
            })
            .into_any_element();
        panel_frame(
            identity,
            actions,
            self.transfer_status(cx),
            body,
            footer_actions,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Viewport;
    #[test]
    fn zoom_steps_are_bounded_and_fit_has_a_defined_start() {
        let mut view = Viewport::default();
        view.step(true);
        assert_eq!(view.zoom, Some(150));
        for _ in 0..10 {
            view.step(true);
        }
        assert_eq!(view.zoom, Some(400));
        for _ in 0..10 {
            view.step(false);
        }
        assert_eq!(view.zoom, Some(25));
        view.zoom = None;
        view.step(false);
        assert_eq!(view.zoom, Some(75));
    }
}

#[cfg(test)]
#[path = "file_preview_tests.rs"]
mod rendered_tests;
