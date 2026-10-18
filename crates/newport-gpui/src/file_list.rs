//! Bounded current-directory table with keyboard-reachable rows and a fixed footer.
use super::*;
use gpui_kit::{base::TestSupportExt as _, component::skeleton::Skeleton};

#[derive(Clone, Copy)]
enum ListingBody {
    Loading,
    Empty,
    NoMatches,
    Rows,
}

// The source paginates at 40 rows. Keep that bounded page in the focus tree:
// virtualizing its controls makes Tab skip every offscreen file. Row scopes
// reveal keyboard focus once without pinning subsequent mouse scrolling.
fn paged_rows(
    scroll: &modal::FormScroll,
    rows: impl IntoIterator<Item = AnyElement>,
) -> AnyElement {
    div()
        .id("files")
        .debug_selector(|| "file-page-rows".into())
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&scroll.handle)
        .children(
            rows.into_iter()
                .enumerate()
                .map(|(i, row)| scroll.row(i, row)),
        )
        .into_any_element()
}

fn listing_panel(
    state: ListingBody,
    show_modified: bool,
    row_count: usize,
    rows: AnyElement,
    cx: &App,
) -> AnyElement {
    div()
        .id("file-list")
        .debug_selector(|| "file-list".into())
        .role(Role::Table)
        .aria_label("Files in current folder")
        .aria_column_count(if show_modified { 4 } else { 3 })
        .aria_row_count(row_count + 1)
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .child(
            div()
                .id("file-list-columns")
                .debug_selector(|| "file-list-columns".into())
                .role(Role::Row)
                .aria_row_index(1)
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(30.))
                .flex_shrink_0()
                .border_b_1()
                .border_color(cx.theme().border)
                .bg(rgb(if cx.theme().is_dark() {
                    0x2a292d
                } else {
                    0xf5f4f6
                }))
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().foreground)
                .child(column_header("Name").flex_1().child("Name"))
                .child(
                    column_header("Size")
                        .w(px(76.))
                        .flex_shrink_0()
                        .text_right()
                        .child("Size"),
                )
                .when(show_modified, |row| {
                    row.child(
                        column_header("Modified")
                            .w(px(94.))
                            .flex_shrink_0()
                            .text_color(cx.theme().muted_foreground)
                            .child("Modified"),
                    )
                })
                .child(column_header("Actions").w(px(56.)).flex_shrink_0()),
        )
        .child(match state {
            ListingBody::Loading => div()
                .id("file-list-loading")
                .debug_selector(|| "file-list-loading".into())
                .role(Role::Status)
                .aria_label("Reading folder…")
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .children((0usize..6).map(|index| {
                    div()
                        .id(("file-skeleton-row", index))
                        .debug_selector(move || format!("file-skeleton-row-{index}"))
                        .h(px(36.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .child(Skeleton::new().size(px(16.)).rounded(px(3.)))
                        .child(div().flex_1().min_w_0().child(Skeleton::new().h(px(12.)).w(
                            relative(match index % 3 {
                                0 => 0.62,
                                1 => 0.82,
                                _ => 0.44,
                            }),
                        )))
                        .child(
                            div()
                                .w(px(76.))
                                .flex_shrink_0()
                                .flex()
                                .justify_end()
                                .child(Skeleton::new().h(px(11.)).w(px(42.))),
                        )
                        .when(show_modified, |row| {
                            row.child(
                                div()
                                    .w(px(94.))
                                    .flex_shrink_0()
                                    .child(Skeleton::new().h(px(11.)).w(px(70.))),
                            )
                        })
                        .child(div().w(px(56.)).flex_shrink_0())
                }))
                .into_any_element(),
            ListingBody::Empty | ListingBody::NoMatches => {
                let message = if matches!(state, ListingBody::Empty) {
                    "Empty folder."
                } else {
                    "No matching files."
                };
                div()
                    .id("file-list-empty")
                    .debug_selector(|| "file-list-empty".into())
                    .role(Role::Status)
                    .aria_label(message)
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(empty::pane(
                        if matches!(state, ListingBody::Empty) {
                            IconName::Folder
                        } else {
                            IconName::Search
                        },
                        message,
                        if matches!(state, ListingBody::Empty) {
                            "Use Upload to add a file to this folder."
                        } else {
                            "Try another search or clear the file filter."
                        },
                        cx,
                    ))
                    .into_any_element()
            }
            ListingBody::Rows => rows,
        })
        .into_any_element()
}

fn column_header(name: &'static str) -> Stateful<Div> {
    div().id(name).role(Role::ColumnHeader).aria_label(name)
}

fn metadata_cell(id: impl Into<ElementId>, value: String) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Cell)
        .aria_label(value.clone())
        .text_size(px(11.))
        .opacity(0.7)
        .child(value)
}

fn file_name(entry: &Value) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(7.))
        .font_weight(FontWeight::NORMAL)
        .child(file_icon::icon(&text(entry, "name"), &text(entry, "kind")))
        .child(div().min_w_0().text_ellipsis().child(text(entry, "name")))
        .when(entry["kind"] == "symlink", |name| {
            name.child(div().flex_shrink_0().text_size(px(10.)).child("Link"))
        })
        .into_any_element()
}

fn file_name_label(entry: &Value) -> String {
    let name = text(entry, "name");
    if entry["kind"] == "symlink" {
        format!("{name} · Link")
    } else {
        name
    }
}

// Retain the complete bounded page for Tab/AX navigation, while giving each
// row its own layout/paint cache. No parent reads occur during row rendering:
// all data and action identities are explicit props, invalidated together.
type RowRenderer<K> = Box<dyn Fn(&K, &mut Window, &mut App) -> AnyElement>;
struct RetainedRow<K: 'static> {
    props: K,
    render: RowRenderer<K>,
}
impl<K: 'static> Render for RetainedRow<K> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.render)(&self.props, window, cx)
    }
}
fn retained_row<K: PartialEq + Clone + 'static>(
    index: usize,
    props: K,
    render: impl Fn(&K, &mut Window, &mut App) -> AnyElement + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let row = window.use_keyed_state(("retained-file-row", index), cx, |_, _| RetainedRow {
        props: props.clone(),
        render: Box::new(render),
    });
    row.update(cx, |row, cx| {
        if row.props != props {
            row.props = props;
            cx.notify();
        }
    });
    // GPUI 0.3.8 reuses prepaint/paint ranges without replaying their native
    // accessibility nodes. Keep the real controls in every AX frame until
    // upstream caching supports that tree; test-support snapshots alone do
    // not detect this loss.
    if window.is_a11y_active() {
        row.into_any_element()
    } else {
        row.cached(div().w_full().h(px(36.)).flex_shrink_0().style().clone())
            .into_any_element()
    }
}

#[derive(Clone, PartialEq)]
struct FileRowProps {
    entry: Value,
    target: Option<FileTarget>,
    selected: bool,
    show_modified: bool,
    has_next: bool,
    project_pending: bool,
    saving_this: bool,
    transfer_pending: bool,
}

fn file_listener(
    view: WeakEntity<Newport>,
    action: impl Fn(&mut Newport, &mut Window, &mut Context<Newport>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        let _ = view.update(cx, |this, cx| action(this, window, cx));
    }
}

fn file_row(props: &FileRowProps, i: usize, view: &WeakEntity<Newport>, cx: &App) -> AnyElement {
    let entry = &props.entry;
    let path = text(entry, "path");
    let directory = entry["kind"] == "directory";
    let is_selected = props.selected;
    let show_modified = props.show_modified;
    let project_pending = props.project_pending;
    let saving_this = props.saving_this;
    let target = props.target.clone();
    let menu_target = target.clone();
    let action_target = target.clone();
    let menu_view = view.clone();
    div()
        .id(("file-row", i))
        .role(Role::Row)
        .aria_row_index(i + 2)
        .h(px(36.))
        .w_full()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .when(props.has_next, |row| {
            row.border_b_1().border_color(cx.theme().border)
        })
        .when(!is_selected, |row| {
            row.hover(|row| row.bg(theme::row_hover(cx)))
        })
        .when(is_selected, |row| {
            row.bg(theme::git_selection(cx.theme().is_dark()))
        })
        .child(
            div()
                .id(("file-name-cell", i))
                .role(Role::Cell)
                .aria_label(file_name_label(entry))
                .flex_1()
                .min_w_0()
                .child(
                    crate::controls::button(("file-entry", i))
                        .accessibility_label(file_name_label(entry))
                        .child(file_name(entry))
                        .w_full()
                        .min_w_0()
                        .px_0()
                        .justify_start()
                        .hover_detail(
                            text(entry, "name"),
                            format!("Permissions: {}", text(entry, "permissions")),
                        )
                        // The row owns hover and selection; keep the name hit area clear.
                        .custom(gpui_kit::component::button::ButtonCustomVariant::new(cx))
                        .disabled(target.is_none())
                        .on_click(file_listener(view.clone(), move |this, window, cx| {
                            let Some(target) = &target else {
                                return;
                            };
                            if !this.file_target_current(target) {
                                return;
                            }
                            if directory {
                                this.files_navigate(path.clone(), window, cx);
                            } else {
                                this.files_preview(path.clone(), window, cx);
                            }
                        })),
                ),
        )
        .child(
            metadata_cell(
                ("file-size", i),
                if directory || entry["size"].is_null() {
                    "—".into()
                } else {
                    bytes(&entry["size"])
                },
            )
            .text_size(px(11.))
            .w(px(76.))
            .flex_shrink_0()
            .text_right(),
        )
        .when(show_modified, |row| {
            row.child(
                metadata_cell(("file-modified", i), modified_date(&entry["modified"]))
                    .w(px(94.))
                    .flex_shrink_0()
                    .text_size(px(11.)),
            )
        })
        .child(
            div()
                .id(("file-actions-cell", i))
                .role(Role::Cell)
                .flex()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .child(if directory {
                    crate::controls::compact_icon_button(("file-project", i))
                        .ghost()
                        .flex_shrink_0()
                        .icon(IconName::FolderGit2)
                        .accessibility_label(format!("Open {} as project", text(entry, "name")))
                        .disabled(action_target.is_none() || project_pending)
                        .loading(saving_this)
                        .on_click(file_listener(view.clone(), move |this, _, cx| {
                            if let Some(target) = &action_target {
                                this.file_open_project(target, cx);
                            }
                        }))
                        .into_any_element()
                } else if action_target
                    .as_ref()
                    .is_some_and(|target| target.supports_transfer(Direction::Download))
                {
                    crate::controls::compact_icon_button(("file-download", i))
                        .ghost()
                        .flex_shrink_0()
                        .icon(IconName::Download)
                        .accessibility_label(format!("Download {}", text(entry, "name")))
                        .disabled(props.transfer_pending)
                        .on_click(file_listener(view.clone(), move |this, _, cx| {
                            if let Some(target) = &action_target {
                                this.start_file_transfer(target, Direction::Download, cx);
                            }
                        }))
                        .into_any_element()
                } else {
                    div().w(px(24.)).flex_shrink_0().into_any_element()
                })
                .child(
                    crate::controls::compact_icon_button(("file-actions", i))
                        .icon(IconName::Ellipsis)
                        .flex_shrink_0()
                        .ghost()
                        .accessibility_label(format!("Actions for {}", text(entry, "name")))
                        .dropdown_menu(move |menu, _, cx| match &menu_target {
                            Some(target) => file_menu(menu, menu_view.clone(), target.clone(), cx),
                            None => menu,
                        }),
                ),
        )
        .into_any_element()
}

impl Newport {
    pub(super) fn file_listing(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let browser = &self.file_browsers[&self.server];
        let path = browser.reads.canonical_path().unwrap_or_default();
        let pending = browser.reads.listing_pending();
        let all = browser.entries(cx);
        let count = all.len();
        let range = page_range(count, browser.page);
        let entries = &all[range];
        let selected = self.file_preview_path.as_ref();
        let directory_target = self.file_action_target(path);
        let project_pending = self
            .resources
            .get("file-project")
            .is_some_and(|row| row.pending);
        let view = cx.entity().downgrade();
        let show_modified = self.content_width
            - if browser.tree_visible {
                tree_width(self.content_width)
            } else {
                0.
            }
            >= 520.;
        let state = if pending && browser.reads.listing().is_none() {
            ListingBody::Loading
        } else if count == 0 && !pending && !path.is_empty() {
            if browser.filter.read(cx).value().is_empty() {
                ListingBody::Empty
            } else {
                ListingBody::NoMatches
            }
        } else {
            ListingBody::Rows
        };
        listing_panel(
            state,
            show_modified,
            entries.len(),
            paged_rows(
                &browser.scroll,
                (0..entries.len()).map(|i| {
                    let entry = &entries[i];
                    let path = text(entry, "path");
                    let props = FileRowProps {
                        entry: entry.clone(),
                        target: directory_target
                            .as_ref()
                            .and_then(|target| target.for_entry(entry)),
                        selected: selected == Some(&path),
                        show_modified,
                        has_next: i + 1 < entries.len(),
                        project_pending,
                        saving_this: project_pending
                            && browser.project_path.as_deref() == Some(path.as_str()),
                        transfer_pending: self.file_transfer.is_some(),
                    };
                    let view = view.clone();
                    retained_row(
                        i,
                        props,
                        move |props, _, cx| file_row(props, i, &view, cx),
                        window,
                        cx,
                    )
                }),
            ),
            cx,
        )
    }

    pub(super) fn file_listing_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let browser = &self.file_browsers[&self.server];
        let query = browser.filter.read(cx).value().to_string();
        let count = browser.entries(cx).len();
        let page = page_range(count, browser.page).start / PAGE_SIZE;
        let pages = count.div_ceil(PAGE_SIZE).max(1);
        let pending = browser.reads.listing_pending();
        let server = self.server.clone();
        let count_label = if pending {
            "Reading folder…".into()
        } else {
            format!("{count} {}", if count == 1 { "item" } else { "items" })
        };
        let connection = browser.connection.clone();
        let revision = browser.reads.listing_revision();
        let hidden = browser.hidden;
        let weak = cx.entity().downgrade();
        let click = move |destination: usize| {
            let server = server.clone();
            let connection = connection.clone();
            let query = query.clone();
            let weak = weak.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    if !this.file_browser_current(&server, &connection) {
                        return;
                    }
                    let browser = this.file_browsers.get_mut(&server).unwrap();
                    if browser.reads.listing_pending()
                        || browser.reads.listing_revision() != revision
                        || browser.filter.read(cx).value().as_ref() != query
                        || browser.hidden != hidden
                    {
                        return;
                    }
                    browser.page = destination;
                    browser.scroll.handle.scroll_to_top_of_item(0);
                    cx.notify();
                });
            }
        };
        div()
            .id("files-footer")
            .debug_selector(|| "files-footer".into())
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .p(px(crate::controls::PAGE_PADDING))
            .min_h(px(36.))
            .flex_shrink_0()
            .text_size(px(11.))
            .text_color(cx.theme().muted_foreground)
            .child(
                div()
                    .id("files-count")
                    .role(Role::Status)
                    .aria_label(count_label.clone())
                    .child(count_label),
            )
            .when(pages > 1, |view| {
                view.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            crate::controls::button("files-previous")
                                .ghost()
                                .label("Previous")
                                .disabled(pending || page == 0)
                                .on_click(click(page.saturating_sub(1))),
                        )
                        .child(format!("{} / {pages}", page + 1))
                        .child(
                            crate::controls::button("files-next")
                                .ghost()
                                .label("Next")
                                .disabled(pending || page + 1 >= pages)
                                .on_click(click((page + 1).min(pages - 1))),
                        ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "file_list_tests.rs"]
mod tests;
