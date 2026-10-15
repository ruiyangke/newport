//! Docker inventory presentation. Reads, cache lifetime, and cleanup ownership
//! remain in newport-client; this view owns only ordering, scrolling and review.
use super::*;
use newport_domain::containers::{DiskTotals, DockerImage, DockerVolume, PruneTarget};

#[cfg(test)]
#[path = "containers_disk_layout_tests.rs"]
mod layout_tests;

#[derive(Clone)]
enum InventoryRow {
    Header,
    Images,
    Image(usize, String),
    Volumes,
    Volume(usize, String),
}
impl InventoryRow {
    fn same_item(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Header, Self::Header)
            | (Self::Images, Self::Images)
            | (Self::Volumes, Self::Volumes) => true,
            (Self::Image(_, a), Self::Image(_, b)) | (Self::Volume(_, a), Self::Volume(_, b)) => {
                a == b
            }
            _ => false,
        }
    }
}

#[derive(PartialEq)]
struct HeaderState {
    disk_pending: bool,
    disk_error: Option<String>,
    action_pending: bool,
    action_error: Option<String>,
    width: f32,
}

pub(super) struct DiskView {
    pub(super) list: ListState,
    rows: Vec<InventoryRow>,
    revision: u64,
    header_state: Option<HeaderState>,
}
impl DiskView {
    pub(super) fn new(_cx: &mut Context<ContainersPanel>) -> Self {
        Self {
            list: ListState::new(1, ListAlignment::Top, px(160.)),
            rows: vec![InventoryRow::Header],
            revision: 0,
            header_state: None,
        }
    }
    pub(super) fn sync(&mut self, client: &Containers, width: f32) {
        let revision = client
            .resource("docker-disk")
            .map_or(0, |r| r.data_revision);
        if self.revision != revision {
            self.revision = revision;
            let mut rows = vec![InventoryRow::Header];
            if let Some(data) = client.disk() {
                rows.push(InventoryRow::Images);
                let mut images: Vec<_> = (0..data.images.len()).collect();
                images.sort_by_key(|&i| std::cmp::Reverse(data.images[i].size));
                rows.extend(
                    images
                        .into_iter()
                        .map(|i| InventoryRow::Image(i, data.images[i].id.clone())),
                );
                if !data.volumes.is_empty() {
                    rows.push(InventoryRow::Volumes);
                    let mut volumes: Vec<_> = (0..data.volumes.len()).collect();
                    volumes.sort_by_key(|&i| std::cmp::Reverse(data.volumes[i].size));
                    rows.extend(
                        volumes
                            .into_iter()
                            .map(|i| InventoryRow::Volume(i, data.volumes[i].name.clone())),
                    );
                }
            }
            let mut offset = self.list.logical_scroll_top();
            offset.item_ix = self
                .rows
                .get(offset.item_ix)
                .and_then(|anchor| rows.iter().position(|row| row.same_item(anchor)))
                .unwrap_or(offset.item_ix.min(rows.len().saturating_sub(1)));
            self.list.splice(0..self.rows.len(), rows.len());
            self.list.scroll_to(offset);
            self.rows = rows;
        }
        // Invalidate the variable-height header only when its content changes.
        // Refreshes retain the current scroll offset, including in large lists.
        let header_state = HeaderState {
            width,
            disk_pending: client
                .resource("docker-disk")
                .is_some_and(|r| r.display_pending()),
            disk_error: client.resource("docker-disk").and_then(|r| r.error.clone()),
            action_pending: client.resource("action").is_some_and(|r| r.pending),
            action_error: client.resource("action").and_then(|r| r.error.clone()),
        };
        if self.header_state.as_ref() != Some(&header_state) {
            self.header_state = Some(header_state);
            let offset = self.list.logical_scroll_top();
            self.list.splice(0..1, 1);
            self.list.scroll_to(offset);
        }
    }
    pub(super) fn show_summary(&self) {
        self.list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
    }
}

impl ContainersPanel {
    pub(super) fn disk_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("containers-disk")
            .debug_selector(|| "containers-disk".into())
            .role(accesskit::Role::Region)
            .aria_label("Disk usage")
            .track_focus(&self.detail_focus)
            .tab_stop(true)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if !this.detail_focus.is_focused(window)
                    || event.keystroke.modifiers.modified()
                    || window.has_active_dialog(cx)
                {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "up" => this.disk_view.list.scroll_by(px(-28.)),
                    "down" => this.disk_view.list.scroll_by(px(28.)),
                    "pageup" => this
                        .disk_view
                        .list
                        .scroll_by(-window.viewport_size().height * 0.7),
                    "pagedown" => this
                        .disk_view
                        .list
                        .scroll_by(window.viewport_size().height * 0.7),
                    "home" => this.disk_view.list.scroll_to(ListOffset {
                        item_ix: 0,
                        offset_in_item: px(0.),
                    }),
                    "end" => this.disk_view.list.scroll_to_end(),
                    _ => return,
                }
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .p(px(crate::controls::PAGE_PADDING))
            .child(
                div()
                    .id("docker-inventory-table")
                    .debug_selector(|| "docker-inventory-table".into())
                    .role(accesskit::Role::Table)
                    .test_support()
                    .aria_label("Docker disk inventory")
                    .aria_row_count(
                        self.disk_view
                            .rows
                            .len()
                            .saturating_sub(1 + self.empty_image_section()),
                    )
                    .aria_column_count(if self.disk_narrow() { 2 } else { 4 })
                    .size_full()
                    .child(
                        list(
                            self.disk_view.list.clone(),
                            cx.processor(|this, index: usize, _, cx| {
                                // The empty Images section has a heading/message but no
                                // column row. Keep later volume rows contiguous in AX.
                                let row_index = index.saturating_sub(this.empty_image_section());
                                match this.disk_view.rows[index] {
                                    InventoryRow::Header => this.disk_header(cx),
                                    InventoryRow::Images => this.disk_columns(false, row_index, cx),
                                    InventoryRow::Volumes => this.disk_columns(true, row_index, cx),
                                    InventoryRow::Image(i, _) => this
                                        .client
                                        .disk()
                                        .map(|data| {
                                            image_row(
                                                i,
                                                row_index,
                                                &data.images[i],
                                                this.disk_narrow(),
                                                cx,
                                            )
                                        })
                                        .unwrap_or_else(|| div().into_any_element()),
                                    InventoryRow::Volume(i, _) => this
                                        .client
                                        .disk()
                                        .map(|data| {
                                            volume_row(
                                                i,
                                                row_index,
                                                &data.volumes[i],
                                                this.disk_narrow(),
                                                cx,
                                            )
                                        })
                                        .unwrap_or_else(|| div().into_any_element()),
                                }
                            }),
                        )
                        .size_full()
                        .pb(px(16.)),
                    ),
            )
            .into_any_element()
    }
    fn empty_image_section(&self) -> usize {
        usize::from(
            self.client
                .disk()
                .is_some_and(|data| data.images.is_empty()),
        )
    }
    fn disk_narrow(&self) -> bool {
        let detail_width = if self.width <= 600. {
            self.width - 32.
        } else {
            self.width - 312.
        };
        detail_width < 460.
    }
    fn disk_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let detail_width = if self.width <= 600. {
            self.width - 32.
        } else {
            self.width - 312.
        };
        // Match repeat(auto-fill, minmax(140px, 1fr)): each row shares tracks,
        // including an incomplete final row, rather than stretching its cards.
        let columns = ((detail_width + 10.) / 150.).floor().max(1.) as u16;
        let data = self.client.disk();
        let error = self.error("docker-disk");
        let pending = self.pending("docker-disk");
        let subtitle = data
            .map(|data| {
                format!(
                    "Docker uses {} on this server{}",
                    bytes(&json!(data.totals.size())),
                    if data.totals.reclaimable() > 0 {
                        format!(
                            " · {} can be freed",
                            bytes(&json!(data.totals.reclaimable()))
                        )
                    } else {
                        String::new()
                    }
                )
            })
            .unwrap_or_else(|| {
                if error.is_some() {
                    "Disk usage unavailable".into()
                } else {
                    "Reading disk usage…".into()
                }
            });
        div()
            .w_full()
            .id("disk-header")
            .role(accesskit::Role::Caption)
            .debug_selector(|| "disk-header".into())
            .flex()
            .flex_col()
            .gap(px(16.))
            .pb(px(16.))
            .when(self.width <= 600., |v| {
                v.child(
                    div().flex().items_start().child(
                        crate::controls::compact_button("disk-back")
                            .ghost()
                            .label("All containers")
                            .icon(IconName::ArrowLeft)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.selected = None;
                                this.sync_selection(cx);
                                this.list_focus.focus(window, cx);
                                cx.notify();
                            })),
                    ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(heading("Disk usage", 2))
                            .child(
                                quiet(subtitle.clone(), cx)
                                    .id("disk-summary")
                                    .role(accesskit::Role::Status)
                                    .test_support()
                                    .aria_label(subtitle)
                                    .mt(px(2.)),
                            ),
                    )
                    .child(
                        crate::controls::icon_button("refresh-docker-disk")
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .accessibility_label("Refresh disk usage")
                            .loading(pending)
                            .disabled(pending)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.client.retry_disk();
                                cx.notify();
                            })),
                    ),
            )
            .when_some(error, |v, error| {
                v.child(alert("docker-disk-error", error, cx))
            })
            .when(
                data.is_none() && !pending && self.error("docker-disk").is_some(),
                |v| {
                    v.child(
                        div().flex().items_start().child(
                            crate::controls::button("retry-docker-disk")
                                .label("Retry disk usage")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.client.retry_disk();
                                    cx.notify();
                                })),
                        ),
                    )
                },
            )
            .when(data.is_none() && pending, |v| {
                v.child(
                    div()
                        .id("docker-disk-skeleton")
                        .debug_selector(|| "docker-disk-skeleton".into())
                        .grid()
                        .grid_cols(columns)
                        .w_full()
                        .gap(px(10.))
                        .children(
                            (0..4).map(|_| Skeleton::new().h(px(112.)).w_full().rounded(px(10.))),
                        ),
                )
            })
            .when_some(data, |v, data| {
                v.child(
                    div()
                        .grid()
                        .grid_cols(columns)
                        .w_full()
                        .gap(px(10.))
                        .child(self.disk_tile(
                            "Images",
                            &data.totals.images,
                            ["image", "images"],
                            self.image_cleanup(cx),
                            cx,
                        ))
                        .child(self.disk_tile(
                            "Containers",
                            &data.totals.containers,
                            ["container", "containers"],
                            self.prune_button(PruneTarget::Containers, "Remove stopped", cx),
                            cx,
                        ))
                        .child(self.disk_tile(
                            "Volumes",
                            &data.totals.volumes,
                            ["volume", "volumes"],
                            self.prune_button(PruneTarget::Volumes, "Remove unused", cx),
                            cx,
                        ))
                        .child(self.disk_tile(
                            "Build cache",
                            &data.totals.build_cache,
                            ["entry", "entries"],
                            self.prune_button(PruneTarget::BuildCache, "Clear", cx),
                            cx,
                        )),
                )
            })
            .into_any_element()
    }
    fn disk_tile(
        &self,
        label: &'static str,
        totals: &DiskTotals,
        unit: [&str; 2],
        action: AnyElement,
        cx: &App,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("disk-tile-{label}")))
            .debug_selector(move || format!("disk-tile-{label}"))
            .role(accesskit::Role::Region)
            .test_support()
            .aria_label(format!(
                "{label}: {}, {} {}{}",
                bytes(&json!(totals.size)),
                totals.count,
                unit[usize::from(totals.count != 1)],
                if totals.reclaimable > 0 {
                    format!(", {} can be freed", bytes(&json!(totals.reclaimable)))
                } else {
                    String::new()
                }
            ))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .rounded(px(10.))
            .border_1()
            .border_color(cx.theme().border)
            .px(px(14.))
            .py(px(12.))
            .child(
                quiet(label, cx)
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(bytes(&json!(totals.size))),
            )
            .child(
                quiet(
                    format!(
                        "{} {}{}",
                        totals.count,
                        unit[usize::from(totals.count != 1)],
                        if totals.reclaimable > 0 {
                            format!(" · {} can be freed", bytes(&json!(totals.reclaimable)))
                        } else {
                            String::new()
                        }
                    ),
                    cx,
                )
                .text_size(px(11.5)),
            )
            .child(
                div()
                    .mt_auto()
                    .pt(px(4.))
                    .flex()
                    .items_start()
                    .child(action),
            )
            .into_any_element()
    }
    fn prune_running(&self, what: PruneTarget) -> bool {
        self.client
            .action_status(&ActionTarget::Prune(what))
            .is_some_and(|s| s.pending)
    }
    fn review_prune(&mut self, what: PruneTarget, window: &mut Window, cx: &mut Context<Self>) {
        let action = Action::Prune { what };
        if self.active_selection.as_deref() != Some(domain::DISK)
            || self.client.disk().is_none()
            || !self.client.can_action(&action)
        {
            return;
        }
        self.review_action(action, window, cx);
    }
    fn prune_button(
        &self,
        what: PruneTarget,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        crate::controls::button(SharedString::from(format!("disk-prune-{what:?}")))
            .label(label)
            .loading(self.prune_running(what))
            .disabled(!self.client.can_action(&Action::Prune { what }))
            .on_click(cx.listener(move |this, _, window, cx| this.review_prune(what, window, cx)))
            .into_any_element()
    }
    fn image_cleanup(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let dangling = self.client.disk().map_or(0, |disk| {
            disk.images.iter().filter(|image| image.dangling).count()
        });
        crate::controls::button("disk-image-cleanup")
            .label("Clean up")
            .child(Icon::new(IconName::ChevronDown).size(px(12.)))
            .loading(
                self.prune_running(PruneTarget::DanglingImages)
                    || self.prune_running(PruneTarget::UnusedImages),
            )
            .disabled(!self.client.can_action(&Action::Prune {
                what: PruneTarget::DanglingImages,
            }))
            .dropdown_menu(move |mut menu, _, _| {
                for (what, label) in [
                    (
                        PruneTarget::DanglingImages,
                        format!("Remove dangling images ({dangling})"),
                    ),
                    (
                        PruneTarget::UnusedImages,
                        "Remove all unused images".to_owned(),
                    ),
                ] {
                    let weak = weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| this.review_prune(what, window, cx));
                    }));
                }
                menu
            })
            .into_any_element()
    }
    fn disk_columns(&self, volumes: bool, row_index: usize, cx: &App) -> AnyElement {
        let count = self.client.disk().map_or(0, |d| {
            if volumes {
                d.volumes.len()
            } else {
                d.images.len()
            }
        });
        div()
            .w_full()
            .id(if volumes {
                "disk-columns-volumes"
            } else {
                "disk-columns-images"
            })
            .debug_selector(move || {
                if volumes {
                    "disk-columns-volumes".into()
                } else {
                    "disk-columns-images".into()
                }
            })
            .flex()
            .flex_col()
            .gap(px(6.))
            .pt(px(if volumes { 20. } else { 0. }))
            .pb(px(4.))
            .child(
                div()
                    .id(if volumes {
                        "disk-volume-heading"
                    } else {
                        "disk-image-heading"
                    })
                    .role(accesskit::Role::Heading)
                    .aria_label(format!(
                        "{}: {count}",
                        if volumes { "Volumes" } else { "Images" }
                    ))
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!(
                        "{}  {count}",
                        if volumes { "Volumes" } else { "Images" }
                    )),
            )
            .when(!volumes && count == 0, |v| {
                v.child(crate::empty::compact("No images on this server.", cx))
            })
            .when(count > 0, |v| {
                v.child(
                    div()
                        .id(if volumes {
                            "disk-volume-columns"
                        } else {
                            "disk-image-columns"
                        })
                        .role(accesskit::Role::Row)
                        .aria_row_index(row_index)
                        .flex()
                        .gap(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .text_size(px(11.))
                        .child(
                            inventory_cell(
                                "name",
                                1,
                                if volumes { "Volume" } else { "Image" },
                                true,
                            )
                            .flex_1()
                            .min_w_0()
                            .child(if volumes {
                                "Volume"
                            } else {
                                "Image"
                            }),
                        )
                        .child(
                            inventory_cell("size", 2, "Size", true)
                                .w(px(74.))
                                .text_right()
                                .child("Size"),
                        )
                        .when(!self.disk_narrow(), |v| {
                            v.when(!volumes, |v| {
                                v.child(
                                    inventory_cell("created", 3, "Created", true)
                                        .w(px(85.))
                                        .child("Created"),
                                )
                            })
                            .child(
                                inventory_cell("usage", 4, "Used by", true)
                                    .w(px(94.))
                                    .child("Used by"),
                            )
                        }),
                )
            })
            .into_any_element()
    }
}

pub(super) fn prune_copy(what: PruneTarget) -> (&'static str, &'static str, &'static str, bool) {
    match what {
        PruneTarget::Containers => (
            "Remove all stopped containers?",
            "Remove stopped",
            "Every container that isn’t running is deleted, with its logs. Images and volumes stay. Compose recreates its services on the next “up”.",
            true,
        ),
        PruneTarget::DanglingImages => (
            "Remove dangling images?",
            "Remove dangling",
            "Untagged image layers left behind by rebuilds and pulls are deleted. Nothing that runs or is tagged is touched.",
            false,
        ),
        PruneTarget::UnusedImages => (
            "Remove every unused image?",
            "Remove unused",
            "Every image no container uses is deleted, tagged ones included. Starting something from them later means pulling or building again.",
            true,
        ),
        PruneTarget::BuildCache => (
            "Clear the build cache?",
            "Clear cache",
            "Cached build layers are deleted. The next builds take longer while the cache fills again.",
            false,
        ),
        PruneTarget::Volumes => (
            "Remove unused anonymous volumes?",
            "Remove volumes",
            "The data in them is deleted for good. Only anonymous volumes no container uses are removed; named volumes stay.",
            true,
        ),
    }
}
fn used_by(count: Option<u64>, empty: &str) -> String {
    match count {
        Some(n) if n > 0 => format!("{n} {}", if n == 1 { "container" } else { "containers" }),
        Some(_) => empty.into(),
        None => "unknown".into(),
    }
}
fn inventory_cell(
    id: &'static str,
    column: usize,
    label: impl Into<SharedString>,
    header: bool,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .role(if header {
            accesskit::Role::ColumnHeader
        } else {
            accesskit::Role::Cell
        })
        .aria_column_index(column)
        .aria_label(label)
}
fn inventory_row(
    id: String,
    row_index: usize,
    label: String,
    cx: &App,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .role(accesskit::Role::Row)
        .test_support()
        .aria_row_index(row_index)
        .aria_label(label)
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .gap(px(12.))
        .border_t_1()
        .border_color(cx.theme().border)
        .py(px(5.))
        .text_size(px(12.))
}
fn image_row(
    index: usize,
    row_index: usize,
    image: &DockerImage,
    narrow: bool,
    cx: &App,
) -> AnyElement {
    let name = if image.tags.is_empty() {
        format!(
            "untagged {}",
            image
                .id
                .strip_prefix("sha256:")
                .unwrap_or(&image.id)
                .chars()
                .take(12)
                .collect::<String>()
        )
    } else {
        image.tags.join(", ")
    };
    let size = bytes(&json!(image.size));
    let age = format!(
        "{} ago",
        newport_domain::services::duration(
            chrono::Local::now()
                .timestamp_millis()
                .saturating_sub(image.created)
        )
    );
    let usage = used_by(
        Some(image.containers),
        if image.dangling { "dangling" } else { "unused" },
    );
    inventory_row(
        format!("docker-image-{index}"),
        row_index,
        format!("{name}, {size}, created {age}, {usage}"),
        cx,
    )
    .child(
        inventory_cell(
            "name",
            1,
            if narrow {
                format!("{name}, created {age}, {usage}")
            } else {
                name.clone()
            },
            false,
        )
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(11.5))
                .child(name),
        )
        .when(narrow, |v| {
            v.child(quiet(format!("{age} · {usage}"), cx).text_size(px(11.)))
        }),
    )
    .child(
        inventory_cell("size", 2, size.clone(), false)
            .w(px(74.))
            .flex_shrink_0()
            .text_right()
            .child(size),
    )
    .when(!narrow, |v| {
        v.child(
            inventory_cell("created", 3, age.clone(), false)
                .child(quiet(age, cx))
                .w(px(85.))
                .flex_shrink_0(),
        )
        .child(
            inventory_cell("usage", 4, usage.clone(), false)
                .child(quiet(usage, cx))
                .w(px(94.))
                .flex_shrink_0(),
        )
    })
    .into_any_element()
}
fn volume_row(
    index: usize,
    row_index: usize,
    volume: &DockerVolume,
    narrow: bool,
    cx: &App,
) -> AnyElement {
    let name = if volume.anonymous {
        format!("{}…", volume.name.chars().take(12).collect::<String>())
    } else {
        volume.name.clone()
    };
    let size = volume
        .size
        .map(|n| bytes(&json!(n)))
        .unwrap_or_else(|| "—".into());
    let usage = used_by(volume.ref_count, "unused");
    inventory_row(
        format!("docker-volume-{index}"),
        row_index,
        format!("{}, {size}, {usage}", volume.name),
        cx,
    )
    .child(
        inventory_cell(
            "name",
            1,
            format!(
                "{}{}{}{}",
                volume.name,
                if volume.anonymous { ", anonymous" } else { "" },
                volume
                    .compose
                    .as_ref()
                    .map(|p| format!(", project {p}"))
                    .unwrap_or_default(),
                if narrow {
                    format!(", {usage}")
                } else {
                    String::new()
                }
            ),
            false,
        )
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(11.5))
                        .child(name),
                )
                .when_some(volume.compose.clone(), |v, project| {
                    v.child(
                        quiet(project, cx)
                            .text_size(px(10.5))
                            .bg(cx.theme().muted)
                            .rounded(px(4.))
                            .px(px(4.)),
                    )
                })
                .when(volume.anonymous, |v| {
                    v.child(quiet("anonymous", cx).text_size(px(11.)))
                }),
        )
        .when(narrow, |v| {
            v.child(quiet(usage.clone(), cx).text_size(px(11.)))
        }),
    )
    .child(
        inventory_cell("size", 2, size.clone(), false)
            .w(px(74.))
            .flex_shrink_0()
            .text_right()
            .child(size),
    )
    .when(!narrow, |v| {
        v.child(
            inventory_cell("usage", 4, usage.clone(), false)
                .child(quiet(usage, cx))
                .w(px(94.))
                .flex_shrink_0(),
        )
    })
    .into_any_element()
}
