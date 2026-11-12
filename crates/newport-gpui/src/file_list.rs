//! The current folder as one virtualized, sortable list. The list is a single
//! tab stop: arrow keys move the selection and Enter opens it. Row actions
//! appear on hover, focus or selection; the status bar describes the folder.
use super::*;
use gpui_kit::component::skeleton::Skeleton;

pub(super) const ROW_HEIGHT: f32 = crate::ui::ROW_HEIGHT;
const MODIFIED_WIDTH: f32 = 132.;
const SIZE_WIDTH: f32 = 84.;
const ACTIONS_WIDTH: f32 = 60.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SortColumn {
    #[default]
    Name,
    Modified,
    Size,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Sort {
    pub column: SortColumn,
    pub descending: bool,
}
impl Sort {
    /// Choosing the active column flips its direction; another column starts
    /// with the order people usually want first (newest, largest, A–Z).
    pub(super) fn choose(self, column: SortColumn) -> Self {
        if self.column == column {
            Self {
                column,
                descending: !self.descending,
            }
        } else {
            Self {
                column,
                descending: column != SortColumn::Name,
            }
        }
    }
}

fn name_key(entry: &Value) -> (String, String) {
    let name = text(entry, "name");
    (name.to_lowercase(), name)
}

/// Folders stay first in every order; the chosen column orders each group.
pub(super) fn sort_entries(entries: &mut [Value], sort: Sort) {
    entries.sort_by(|a, b| {
        let file = |entry: &Value| entry["kind"] != "directory";
        file(a).cmp(&file(b)).then_with(|| {
            let order = match sort.column {
                SortColumn::Name => name_key(a).cmp(&name_key(b)),
                SortColumn::Modified => a["modified"]
                    .as_i64()
                    .cmp(&b["modified"].as_i64())
                    .then_with(|| name_key(a).cmp(&name_key(b))),
                // Folders carry SFTP attribute sizes the list never shows.
                SortColumn::Size if file(a) => a["size"]
                    .as_u64()
                    .cmp(&b["size"].as_u64())
                    .then_with(|| name_key(a).cmp(&name_key(b))),
                SortColumn::Size => name_key(a).cmp(&name_key(b)),
            };
            if sort.descending {
                order.reverse()
            } else {
                order
            }
        })
    });
}

/// "35 items · 14 folders, 21 files · 3 hidden", or the filtered share of a
/// trimmed query.
pub(super) fn folder_summary(
    listing: &Value,
    shown: &[Value],
    query: &str,
    hidden_shown: bool,
) -> String {
    let all = listing["entries"].as_array().map_or(&[][..], Vec::as_slice);
    let hidden = all
        .iter()
        .filter(|entry| text(entry, "name").starts_with('.'))
        .count();
    let plural = |count: usize, one: &str, many: &str| {
        format!("{count} {}", if count == 1 { one } else { many })
    };
    if !query.is_empty() {
        let total = if hidden_shown {
            all.len()
        } else {
            all.len() - hidden
        };
        return format!("{} of {total} items match", shown.len());
    }
    let folders = shown
        .iter()
        .filter(|entry| entry["kind"] == "directory")
        .count();
    let mut summary = plural(shown.len(), "item", "items");
    if !shown.is_empty() {
        summary.push_str(&format!(
            " · {}, {}",
            plural(folders, "folder", "folders"),
            plural(shown.len() - folders, "file", "files")
        ));
    }
    if !hidden_shown && hidden > 0 {
        summary.push_str(&format!(" · {hidden} hidden"));
    }
    summary
}

#[derive(Clone, Copy)]
enum ListingBody {
    Loading,
    Empty,
    NoMatches,
    Rows,
}

fn column_header(
    id: &'static str,
    name: &'static str,
    column: SortColumn,
    sort: Sort,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let active = sort.column == column;
    div()
        .id(id)
        .role(Role::ColumnHeader)
        .aria_label(if active {
            format!(
                "{name}, sorted {}",
                if sort.descending {
                    "descending"
                } else {
                    "ascending"
                }
            )
        } else {
            name.to_owned()
        })
        .flex()
        .items_center()
        .gap(px(4.))
        .cursor_pointer()
        .text_color(if active {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        })
        .hover(|style| style.text_color(cx.theme().foreground))
        .test_support()
        .child(name)
        .when(active, |header| {
            header.child(
                Icon::new(if sort.descending {
                    IconName::ArrowDown
                } else {
                    IconName::ArrowUp
                })
                .size(px(12.)),
            )
        })
        .on_click(on_click)
}

fn metadata_cell(id: impl Into<ElementId>, value: String) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Cell)
        .aria_label(value.clone())
        .flex_shrink_0()
        .text_right()
        .truncate()
        .child(value)
}

fn file_name(entry: &Value, cx: &App) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(file_icon::icon(&text(entry, "name"), &text(entry, "kind")))
        .child(div().min_w_0().truncate().child(text(entry, "name")))
        .when(entry["kind"] == "symlink", |name| {
            name.child(crate::controls::tag("Link", cx))
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

// Each visible row keeps its own layout and paint cache. No parent reads occur
// during row rendering: data and action identities arrive as explicit props.
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
    identity: impl Into<ElementId>,
    props: K,
    render: impl Fn(&K, &mut Window, &mut App) -> AnyElement + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let row = window.use_keyed_state(identity, cx, |_, _| RetainedRow {
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
    // upstream caching supports that tree.
    if window.is_a11y_active() {
        row.into_any_element()
    } else {
        row.cached(
            div()
                .w_full()
                .h(px(ROW_HEIGHT))
                .flex_shrink_0()
                .style()
                .clone(),
        )
        .into_any_element()
    }
}

// A row's identity follows the remote file, so its cache and menu state move
// with it when sorting or a refresh reorders the folder.
fn file_row_identity(server: &str, path: &str) -> SharedString {
    SharedString::from(serde_json::to_string(&("file-row", server, path)).expect("string tuple"))
}

#[derive(Clone, PartialEq)]
struct FileRowProps {
    index: usize,
    entry: Value,
    group: SharedString,
    target: Option<FileTarget>,
    selected: bool,
    show_modified: bool,
    project_pending: bool,
    saving_this: bool,
    transfer_pending: bool,
    /// Minute-resolution clock for relative dates; rows repaint when it ticks.
    now: i64,
}

fn file_listener(
    view: WeakEntity<Newport>,
    action: impl Fn(&mut Newport, &mut Window, &mut Context<Newport>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        let _ = view.update(cx, |this, cx| action(this, window, cx));
    }
}

fn file_row(props: &FileRowProps, view: &WeakEntity<Newport>, cx: &App) -> AnyElement {
    let entry = &props.entry;
    let path = text(entry, "path");
    let name = text(entry, "name");
    let directory = entry["kind"] == "directory";
    let target = props.target.clone();
    let action_target = target.clone();
    let menu_target = target.clone();
    let menu_view = view.clone();
    let modified = entry["modified"]
        .as_i64()
        .and_then(|seconds| seconds.checked_mul(1000));
    let primary = if directory {
        Some(
            crate::controls::compact_icon_button("file-project")
                .ghost()
                .icon(IconName::FolderGit2)
                .tooltip("Open as project")
                .accessibility_label(format!("Open {name} as project"))
                .disabled(action_target.is_none() || props.project_pending)
                .loading(props.saving_this)
                .on_click(file_listener(view.clone(), move |this, _, cx| {
                    if let Some(target) = &action_target {
                        this.file_open_project(target, cx);
                    }
                })),
        )
    } else if action_target
        .as_ref()
        .is_some_and(|target| target.supports_transfer(Direction::Download))
    {
        Some(
            crate::controls::compact_icon_button("file-download")
                .ghost()
                .icon(IconName::Download)
                .tooltip("Download")
                .accessibility_label(format!("Download {name}"))
                .disabled(props.transfer_pending)
                .on_click(file_listener(view.clone(), move |this, _, cx| {
                    if let Some(target) = &action_target {
                        this.start_file_transfer(target, Direction::Download, cx);
                    }
                })),
        )
    } else {
        None
    };
    let group = props.group.clone();
    div()
        .id("file-row")
        .group(group.clone())
        .role(Role::Row)
        .aria_row_index(props.index + 2)
        .aria_label(file_name_label(entry))
        .aria_selected(props.selected)
        .test_support()
        .h(px(ROW_HEIGHT))
        .w_full()
        .pl(px(crate::controls::PAGE_PADDING))
        .pr(px(8.))
        .flex()
        .items_center()
        .gap(px(12.))
        .cursor_default()
        .when(props.selected, |row| {
            row.bg(theme::selected_fill(cx.theme().is_dark()))
        })
        .when(!props.selected, |row| {
            row.hover(|row| row.bg(theme::row_hover(cx)))
        })
        .on_click(file_listener(view.clone(), move |this, window, cx| {
            let Some(target) = &target else {
                return;
            };
            if this.file_target_current(target) {
                this.files_activate(path.clone(), window, cx);
            }
        }))
        .child(
            div()
                .id("file-name-cell")
                .role(Role::Cell)
                .flex_1()
                .min_w_0()
                .flex()
                .child(file_name(entry, cx)),
        )
        .when(props.show_modified, |row| {
            row.child(
                metadata_cell(
                    "file-modified",
                    modified
                        .map(|at| crate::localized_time::file_modified(at, props.now))
                        .unwrap_or_default(),
                )
                .w(px(MODIFIED_WIDTH))
                .text_color(cx.theme().muted_foreground)
                .when_some(modified, |cell, at| {
                    cell.hover_hint(crate::localized_time::detail_date(at))
                }),
            )
        })
        .child(
            metadata_cell(
                "file-size",
                if directory || entry["size"].is_null() {
                    String::new()
                } else {
                    bytes(&entry["size"])
                },
            )
            .w(px(SIZE_WIDTH))
            .text_color(cx.theme().muted_foreground),
        )
        .child(
            div()
                .id("file-actions-cell")
                .role(Role::Cell)
                .w(px(ACTIONS_WIDTH))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(2.))
                .when_some(primary, |cell, button| {
                    cell.child(
                        button
                            .opacity(if props.selected { 1. } else { 0. })
                            .group_hover(group.clone(), |style| style.opacity(1.)),
                    )
                })
                .child(quiet_action::Menu {
                    id: SharedString::from(format!("{}-menu", props.group)),
                    group,
                    engaged: props.selected,
                    button: crate::controls::compact_icon_button("file-actions")
                        .icon(IconName::Ellipsis)
                        .ghost()
                        .accessibility_label(format!("Actions for {name}"))
                        .tooltip("More actions"),
                    builder: Box::new(move |menu, _, cx| match &menu_target {
                        Some(target) => file_menu(menu, menu_view.clone(), target.clone(), cx),
                        None => menu,
                    }),
                }),
        )
        .into_any_element()
}

fn skeleton_rows(show_modified: bool) -> AnyElement {
    div()
        .id("file-list-loading")
        .debug_selector(|| "file-list-loading".into())
        .role(Role::Status)
        .aria_label("Reading folder…")
        .test_support()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .children((0usize..8).map(|index| {
            div()
                .id(("file-skeleton-row", index))
                .debug_selector(move || format!("file-skeleton-row-{index}"))
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .gap(px(12.))
                .pl(px(crate::controls::PAGE_PADDING))
                .pr(px(8.))
                .child(Skeleton::new().size(px(16.)).rounded(px(crate::ui::RADIUS)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Skeleton::new().h(px(12.)).w(relative(match index % 3 {
                            0 => 0.42,
                            1 => 0.58,
                            _ => 0.3,
                        }))),
                )
                .when(show_modified, |row| {
                    row.child(
                        div()
                            .w(px(MODIFIED_WIDTH))
                            .flex()
                            .justify_end()
                            .child(Skeleton::new().h(px(11.)).w(px(64.))),
                    )
                })
                .child(
                    div()
                        .w(px(SIZE_WIDTH))
                        .flex()
                        .justify_end()
                        .child(Skeleton::new().h(px(11.)).w(px(42.))),
                )
                .child(div().w(px(ACTIONS_WIDTH)))
        }))
        .into_any_element()
}

impl Newport {
    pub(super) fn file_listing(
        &self,
        list_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let browser = &self.file_browsers[&self.server];
        let path = browser.reads.canonical_path().unwrap_or_default();
        let pending = browser.reads.listing_pending();
        let entries = browser.entries(cx);
        let count = entries.len();
        let sort = browser.sort;
        let show_modified = list_width >= 520.;
        let state = if pending && browser.reads.listing().is_none() {
            ListingBody::Loading
        } else if count == 0 && !pending && !path.is_empty() {
            if browser.query(cx).is_empty() {
                ListingBody::Empty
            } else {
                ListingBody::NoMatches
            }
        } else {
            ListingBody::Rows
        };
        let scope = (self.server.clone(), browser.connection.clone());
        let header_click = |column: SortColumn| {
            let (server, connection) = scope.clone();
            cx.listener(move |this: &mut Newport, _: &ClickEvent, _, cx| {
                if this.file_browser_current(&server, &connection)
                    && let Some(browser) = this.file_browsers.get_mut(&server)
                {
                    browser.sort = browser.sort.choose(column);
                    cx.notify();
                }
            })
        };
        let header =
            div()
                .id("file-list-columns")
                .debug_selector(|| "file-list-columns".into())
                .role(Role::Row)
                .aria_row_index(1)
                .flex()
                .items_center()
                .gap(px(12.))
                .pl(px(crate::controls::PAGE_PADDING))
                .pr(px(8.))
                .h(px(28.))
                .flex_shrink_0()
                .border_b_1()
                .border_color(cx.theme().border)
                .text_size(px(crate::ui::FONT_SIZE))
                .child(div().flex_1().min_w_0().flex().child(column_header(
                    "file-sort-name",
                    "Name",
                    SortColumn::Name,
                    sort,
                    header_click(SortColumn::Name),
                    cx,
                )))
                .when(show_modified, |row| {
                    row.child(div().w(px(MODIFIED_WIDTH)).flex().justify_end().child(
                        column_header(
                            "file-sort-modified",
                            "Modified",
                            SortColumn::Modified,
                            sort,
                            header_click(SortColumn::Modified),
                            cx,
                        ),
                    ))
                })
                .child(
                    div()
                        .w(px(SIZE_WIDTH))
                        .flex()
                        .justify_end()
                        .child(column_header(
                            "file-sort-size",
                            "Size",
                            SortColumn::Size,
                            sort,
                            header_click(SortColumn::Size),
                            cx,
                        )),
                )
                .child(div().w(px(ACTIONS_WIDTH)).flex_shrink_0());
        let body = match state {
            ListingBody::Loading => skeleton_rows(show_modified),
            ListingBody::Empty | ListingBody::NoMatches => {
                let empty = matches!(state, ListingBody::Empty);
                let message = if empty {
                    "This folder is empty"
                } else {
                    "No matching files"
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
                        if empty {
                            IconName::Folder
                        } else {
                            IconName::Search
                        },
                        message,
                        if empty {
                            "Upload a file to add it here."
                        } else {
                            "Try another name or clear the filter."
                        },
                        cx,
                    ))
                    .into_any_element()
            }
            ListingBody::Rows => {
                let directory = self.file_action_target(path);
                let selected = browser.selected.clone();
                let project_pending = self
                    .resources
                    .get("file-project")
                    .is_some_and(|row| row.pending);
                let project_path = browser.project_path.clone();
                let transfer_pending = self.file_transfer.is_some();
                let server = self.server.clone();
                let now = chrono::Utc::now().timestamp() / 60 * 60_000;
                let view = cx.entity().downgrade();
                let _ = window;
                uniform_list(
                    "file-rows",
                    count,
                    cx.processor(move |_, range: std::ops::Range<usize>, window, cx| {
                        range
                            .filter_map(|index| {
                                let entry = entries.get(index)?;
                                let path = text(entry, "path");
                                let identity = file_row_identity(&server, &path);
                                let props = FileRowProps {
                                    index,
                                    group: identity.clone(),
                                    entry: entry.clone(),
                                    target: directory
                                        .as_ref()
                                        .and_then(|target| target.for_entry(entry)),
                                    selected: selected.as_deref() == Some(path.as_str()),
                                    show_modified,
                                    project_pending,
                                    saving_this: project_pending
                                        && project_path.as_deref() == Some(path.as_str()),
                                    transfer_pending,
                                    now,
                                };
                                let view = view.clone();
                                Some(retained_row(
                                    identity,
                                    props,
                                    move |props, _, cx| file_row(props, &view, cx),
                                    window,
                                    cx,
                                ))
                            })
                            .collect()
                    }),
                )
                .track_scroll(&browser.scroll)
                .flex_1()
                .min_h_0()
                .into_any_element()
            }
        };
        div()
            .id("file-list")
            .debug_selector(|| "file-list".into())
            .role(Role::Table)
            .aria_label("Files in current folder")
            .aria_column_count(if show_modified { 4 } else { 3 })
            .aria_row_count(count + 1)
            .track_focus(&browser.list_focus)
            .tab_stop(true)
            .focus_visible(|style| crate::controls::inset_focus_ring(style, cx))
            .on_key_down(cx.listener(Self::files_key_down))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub(super) fn file_listing_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let browser = &self.file_browsers[&self.server];
        let entries = browser.entries(cx);
        let query = browser.query(cx);
        let selected = browser
            .selected
            .as_ref()
            .and_then(|path| entries.iter().find(|entry| entry["path"] == *path));
        let summary = match (selected, browser.reads.listing()) {
            (Some(entry), _) => {
                let kind = if entry["kind"] == "directory" {
                    "Folder".to_owned()
                } else if entry["size"].is_null() {
                    "File".to_owned()
                } else {
                    bytes(&entry["size"])
                };
                let modified = entry["modified"]
                    .as_i64()
                    .and_then(|seconds| seconds.checked_mul(1000))
                    .map(|at| format!(" · Modified {}", crate::localized_time::detail_date(at)))
                    .unwrap_or_default();
                let permissions = text(entry, "permissions");
                let permissions = if permissions.trim().is_empty() {
                    String::new()
                } else {
                    format!(" · Permissions {permissions}")
                };
                format!("{} · {kind}{modified}{permissions}", text(entry, "name"))
            }
            // A refresh keeps the listing on screen, so its summary stays too.
            (None, None) if browser.reads.listing_pending() => "Reading folder…".into(),
            (None, None) => String::new(),
            (None, Some(listing)) => folder_summary(listing, &entries, &query, browser.hidden),
        };
        div()
            .id("files-footer")
            .debug_selector(|| "files-footer".into())
            .flex_shrink_0()
            .child(
                crate::controls::status_bar(cx).left(
                    div()
                        .id("files-count")
                        .role(Role::Status)
                        .aria_label(summary.clone())
                        .truncate()
                        .child(summary),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "file_list_tests.rs"]
mod tests;
