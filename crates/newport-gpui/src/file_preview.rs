//! Retained file preview presentation; expanding does not refetch or reset browsing.
use super::*;
#[path = "file_preview_animation.rs"]
mod animation;
#[path = "file_preview_text.rs"]
mod text;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{DialogHandle, Selectable, button::ButtonGroup, dialog::Dialog};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

// The rendered dialog retains only the shared liveness flag, never this lease.
// Replacing/closing a preview invalidates its modal even without a Window.
struct ModalLease {
    live: Rc<Cell<bool>>,
    handle: Rc<RefCell<Option<DialogHandle>>>,
}
impl Drop for ModalLease {
    fn drop(&mut self) {
        self.live.set(false);
    }
}

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

fn dialog_frame(
    dialog: Dialog,
    viewport: Size<Pixels>,
    expanded: bool,
    name: String,
    cx: &App,
) -> Dialog {
    let inset = px(if viewport.width <= px(800.) || expanded {
        12.
    } else {
        24.
    });
    let width = (viewport.width - inset * 2.).max(px(0.));
    let height = (viewport.height - inset * 2.).max(px(0.));
    dialog
        .aria_label(name)
        .w(if expanded {
            width
        } else {
            width.min(px(1040.))
        })
        .h(if expanded || viewport.width <= px(800.) {
            height
        } else {
            height.min(px(760.))
        })
        .p_0()
        .gap_0()
        .rounded(px(14.))
        .when(!cx.theme().is_dark(), |dialog| dialog.border_0())
        .overflow_hidden()
        .centered(true)
        .close_button(false)
        // Enter inside a read-only preview is never an implicit close action.
        .on_ok(|_, _, _| false)
}

fn header(
    identity: &Identity,
    viewport: Size<Pixels>,
    actions: AnyElement,
    cx: &App,
) -> AnyElement {
    let compact = viewport.width <= px(540.) || viewport.height <= px(560.);
    let gap = if compact || viewport.width <= px(800.) {
        8.
    } else {
        12.
    };
    div()
        .id("file-preview-header")
        .debug_selector(|| "file-preview-header".into())
        .flex()
        .items_center()
        .gap(px(gap))
        .px(px(if compact { 12. } else { 20. }))
        .py(px(if compact { 12. } else { 16. }))
        .flex_shrink_0()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .size(px(22.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(file_icon::icon(&identity.name, &identity.kind)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(
                    div()
                        .id("file-preview-title")
                        .role(Role::Heading)
                        .aria_label(identity.name.clone())
                        .text_size(px(13.))
                        .line_height(px(17.55))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_ellipsis()
                        .child(identity.name.clone())
                        .test_support(),
                )
                .child(
                    div()
                        .id("file-preview-path")
                        .role(Role::Label)
                        .aria_label(identity.path.clone())
                        .mt(px(2.))
                        .text_size(px(11.))
                        .line_height(px(16.5))
                        .text_color(cx.theme().muted_foreground)
                        .text_ellipsis()
                        .child(identity.path.clone())
                        .test_support(),
                ),
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
        .min_h(px(40.))
        .px(px(20.))
        .py(px(10.))
        .flex_shrink_0()
        .border_t_1()
        .border_color(cx.theme().border)
        .text_size(px(11.))
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
    viewport: Size<Pixels>,
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
        .child(header(identity, viewport, actions, cx))
        .child(body)
        .child(transfer)
        .child(footer(identity, footer_actions, cx))
        .into_any_element()
}

fn preserve_opener(fallback: &FocusHandle, window: &mut Window, cx: &mut App) {
    if window.focused(cx).is_none() {
        fallback.focus(window, cx);
    }
}

fn dismiss_owned(handle: &Rc<RefCell<Option<DialogHandle>>>, window: &mut Window, cx: &mut App) {
    if let Some(handle) = handle.borrow().clone() {
        window.defer(cx, move |window, cx| {
            window.close_dialog_if(handle.focus_handle(), cx);
        });
    }
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
        .text_size(px(13.))
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
    pub expanded: bool,
    pub zoom: Option<u16>,
    pub wrap: bool,
    pub copied: bool,
    modal: Option<ModalLease>,
    content_focus: Option<FocusHandle>,
    text: Option<text::PreviewText>,
    pdf: Option<Entity<file_pdf_view::PdfPageView>>,
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
    fn file_pdf_page(
        &mut self,
        target: &files::PreviewTarget,
        page: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.file_preview_target_current(target) || self.file_preview_pdf.is_none() {
            return;
        }
        self.request(
            "preview",
            "native.pdf_page",
            json!({"id":self.server,"page":page}),
        );
        cx.notify();
    }
    pub(super) fn open_file_preview_dialog(
        &mut self,
        entry: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.file_preview_target() else {
            return;
        };
        let identity = Identity::from_entry(&entry);
        let live = Rc::new(Cell::new(true));
        let handle = Rc::new(RefCell::new(None::<DialogHandle>));
        self.file_preview_viewport.modal = Some(ModalLease {
            live: live.clone(),
            handle: handle.clone(),
        });
        self.file_preview_viewport.content_focus = Some(cx.focus_handle().tab_stop(true));
        self.file_preview_viewport.text = Some(text::PreviewText::new(window, cx));
        self.file_preview_viewport.pdf = Some(cx.new(file_pdf_view::PdfPageView::new));
        let view = cx.weak_entity();
        // A direct file-row opener stays focused until Kit captures it. The
        // context-menu caller supplies a stable fallback before entering here.
        preserve_opener(&self.focus, window, cx);
        window.open_dialog(cx, move |dialog, window, cx| {
            let current = live.get()
                && view
                    .upgrade()
                    .is_some_and(|view| view.read(cx).file_preview_selection_current(&target));
            if !current {
                dismiss_owned(&handle, window, cx);
                return dialog.close_button(false).on_ok(|_, _, _| false);
            }
            let expanded = view
                .upgrade()
                .is_some_and(|view| view.read(cx).file_preview_viewport.expanded);
            let viewport = window.viewport_size();
            let body_view = view.clone();
            let body_identity = identity.clone();
            let close_view = view.clone();
            let close_target = target.clone();
            let close_live = live.clone();
            dialog_frame(
                dialog,
                window.viewport_size(),
                expanded,
                identity.name.clone(),
                cx,
            )
            .on_close(move |_, _, cx| {
                if close_live.get() {
                    let _ = close_view.update(cx, |this, cx| {
                        if this.file_preview_selection_current(&close_target) {
                            this.close_file_preview();
                            cx.notify();
                        }
                    });
                }
            })
            .content(move |content, window, cx| {
                let body = body_view
                    .update(cx, |this, cx| {
                        this.file_preview_panel(&body_identity, viewport, window, cx)
                    })
                    .unwrap_or_else(|_| div().into_any_element());
                content.size_full().min_h_0().gap_0().p_0().child(body)
            })
        });
        if let Some(modal) = &self.file_preview_viewport.modal {
            *modal.handle.borrow_mut() = window.active_dialog_handle(cx);
        }
    }

    fn file_preview_panel(
        &self,
        identity: &Identity,
        viewport: Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(target) = self.file_preview_target() else {
            return div().into_any_element();
        };
        let data = self.value("preview");
        let expanded = self.file_preview_viewport.expanded;
        let zoom = self.file_preview_viewport.zoom;
        let name = identity.name.clone();
        let download = self.file_action_target(&identity.path).filter(|target| {
            target.supports_transfer(newport_client::file_transfer::Direction::Download)
        });
        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_shrink_0()
            .child(
                crate::controls::button("files-download-preview")
                    .label("Download")
                    .icon(IconName::Download)
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
                crate::controls::icon_button("files-expand-preview")
                    .icon(if expanded {
                        IconName::Minimize2
                    } else {
                        IconName::Maximize2
                    })
                    .ghost()
                    .accessibility_label(if expanded {
                        "Collapse preview"
                    } else {
                        "Expand preview"
                    })
                    .on_click({
                        let target = target.clone();
                        cx.listener(move |this, _, _, cx| {
                            if !this.file_preview_target_current(&target) {
                                return;
                            }
                            this.file_preview_viewport.expanded =
                                !this.file_preview_viewport.expanded;
                            cx.notify();
                        })
                    }),
            )
            .child(
                crate::controls::icon_button("files-close-preview")
                    .icon(IconName::X)
                    .ghost()
                    .accessibility_label("Close dialog")
                    .on_click({
                        let target = target.clone();
                        cx.listener(move |this, _, window, cx| {
                            if !this.file_preview_target_current(&target) {
                                return;
                            }
                            if let Some(handle) = this
                                .file_preview_viewport
                                .modal
                                .as_ref()
                                .and_then(|modal| modal.handle.borrow().clone())
                            {
                                window.close_dialog_if(handle.focus_handle(), cx);
                            }
                            this.close_file_preview();
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
                view.child(message(error, cx))
            })
            .when(error.is_none() && data["kind"] == "text", |panel| {
                let content = text(&data, "content");
                let wrap = self.file_preview_viewport.wrap;
                panel.when_some(
                    self.file_preview_viewport.text.as_ref(),
                    |panel, preview| panel.child(preview.view(&content, wrap, window, cx)),
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
                    );
                    panel
                        .child(
                            div()
                                .id("image-preview-viewport")
                                .role(Role::Region)
                                .aria_label(format!("File preview: {name}"))
                                .flex_1()
                                .min_h_0()
                                .w_full()
                                .overflow_scroll()
                                .bg(cx.theme().muted)
                                .when(zoom.is_none(), |frame| {
                                    frame.flex().items_center().justify_center().p(px(20.))
                                })
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
                                    div()
                                        // Like the original img's max-width/max-height:
                                        // Fit shrinks large images but never enlarges small ones.
                                        .w(px(data["width"].as_u64().unwrap_or(1) as f32))
                                        .h(px(data["height"].as_u64().unwrap_or(1) as f32))
                                        .max_w_full()
                                        .max_h_full()
                                        .flex_shrink_0()
                                        .child(raster)
                                        .into_any_element()
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
                                        .text_size(px(11.)),
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
                                    .text_size(px(11.)),
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
            .when(error.is_none() && data["kind"] == "pdf", |panel| {
                let page = data["page"].as_u64().unwrap_or(1) as usize;
                let pages = data["pages"].as_u64().unwrap_or(1) as usize;
                let pending = self
                    .resources
                    .get("preview")
                    .is_some_and(|resource| resource.pending);
                panel
                    .child(
                        div()
                            .id("pdf-navigation")
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_3()
                            .p(px(8.))
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .text_size(px(11.))
                            .flex_shrink_0()
                            .child(
                                crate::controls::icon_button("pdf-previous")
                                    .icon(IconName::ChevronLeft)
                                    .accessibility_label("Previous PDF page")
                                    .disabled(pending || page <= 1)
                                    .on_click({
                                        let target = target.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            if !this.file_preview_target_current(&target) {
                                                return;
                                            }
                                            this.file_pdf_page(&target, page.saturating_sub(1), cx);
                                        })
                                    }),
                            )
                            .child(
                                div()
                                    .id("pdf-page-number")
                                    .role(Role::Status)
                                    .aria_label(format!("Page {page} of {pages}"))
                                    .child(format!("Page {page} of {pages}")),
                            )
                            .child(
                                crate::controls::icon_button("pdf-next")
                                    .icon(IconName::ChevronRight)
                                    .accessibility_label("Next PDF page")
                                    .disabled(pending || page >= pages)
                                    .on_click({
                                        let target = target.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            if !this.file_preview_target_current(&target) {
                                                return;
                                            }
                                            this.file_pdf_page(&target, page + 1, cx);
                                        })
                                    }),
                            ),
                    )
                    .when(pending, |panel| panel.child(message(None, cx)))
                    .when(!pending, |panel| {
                        panel
                            .when_some(data["textError"].as_str(), |panel, error| {
                                panel.child(
                                    div()
                                        .id("pdf-text-error")
                                        .role(Role::Status)
                                        .aria_label(error.to_owned())
                                        .p(px(8.))
                                        .text_size(px(11.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(error.to_owned()),
                                )
                            })
                            .when_some(self.file_preview_viewport.pdf.as_ref(), |panel, viewer| {
                                if let (Some(image), Some(text)) =
                                    (&self.file_preview_image, &self.file_preview_pdf_text)
                                {
                                    viewer.update(cx, |viewer, cx| {
                                        viewer.set_page(
                                            image.clone(),
                                            text.clone(),
                                            data["naturalWidth"].as_f64().unwrap_or(1.) as f32,
                                            data["naturalHeight"].as_f64().unwrap_or(1.) as f32,
                                            cx,
                                        );
                                    });
                                    panel.child(viewer.clone())
                                } else {
                                    panel.child(message(None, cx))
                                }
                            })
                    })
            })
            .into_any_element();
        let footer_actions = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_shrink_0()
            .when(data["kind"] == "text", |row| {
                let content = text(&data, "content");
                let wrap = self.file_preview_viewport.wrap;
                row.child(
                    crate::controls::button("text-copy")
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
                                cx.write_to_clipboard(ClipboardItem::new_string(content.clone()));
                                this.file_preview_viewport.copied = true;
                                cx.notify();
                            })
                        }),
                )
                .child(
                    crate::controls::button("text-wrap")
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
            .when(data["kind"] == "image", |row| row.child("Image preview"))
            .when(data["kind"] == "pdf", |row| row.child("PDF preview"))
            .into_any_element();
        panel_frame(
            identity,
            viewport,
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
