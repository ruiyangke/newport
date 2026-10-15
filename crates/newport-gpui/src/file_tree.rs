//! Lazy folder navigation. Expansion is presentation state; the shared client
//! owns scoped reads, caching, cancellation and concurrency limits.
use super::*;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::skeleton::Skeleton;
use newport_client::file_browser::folders::Folders;
use std::{collections::HashSet, rc::Rc};

#[cfg(test)]
#[path = "file_tree_tests.rs"]
pub(crate) mod tests;

const MAX_DEPTH: usize = 16;
type ScopeGuard = Rc<dyn Fn(&App) -> bool>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Node {
    root: u8,
    path: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Content {
    Folder(String),
    Loading,
    Refreshing,
    Error(String),
    Empty,
    Truncated,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    node: Node,
    depth: usize,
    content: Content,
}
pub(super) struct Navigate(pub String);
pub(super) struct FolderTree {
    client: Folders,
    home: Option<String>,
    current: String,
    hidden: bool,
    active: bool,
    scope_guard: Option<ScopeGuard>,
    expanded: HashSet<Node>,
    rows: Rc<Vec<Row>>,
    revision: u64,
    notice: Option<String>,
    scroll: UniformListScrollHandle,
    horizontal: ScrollHandle,
}
impl EventEmitter<Navigate> for FolderTree {}

impl FolderTree {
    pub(super) fn new(client: Folders) -> Self {
        Self {
            client,
            home: None,
            current: String::new(),
            hidden: false,
            active: false,
            scope_guard: None,
            expanded: HashSet::new(),
            rows: Rc::default(),
            revision: 0,
            notice: None,
            scroll: UniformListScrollHandle::new(),
            horizontal: ScrollHandle::new(),
        }
    }
    pub(super) fn with_scope_guard(mut self, guard: impl Fn(&App) -> bool + 'static) -> Self {
        self.scope_guard = Some(Rc::new(guard));
        self
    }
    pub(super) fn configure(
        &mut self,
        home: Option<String>,
        current: String,
        hidden: bool,
        active: bool,
        cx: &mut Context<Self>,
    ) {
        if (self.home.as_ref(), &self.current, self.hidden, self.active)
            == (home.as_ref(), &current, hidden, active)
        {
            return;
        }
        if self.home != home {
            self.expanded.retain(|node| node.root != 0);
        }
        if !active {
            self.expanded.clear();
            self.client.suspend();
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        self.home = home;
        self.current = current;
        self.hidden = hidden;
        self.active = active;
        self.rebuild();
        self.cancel_hidden();
        cx.notify();
    }
    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        self.client.clear();
        self.expanded.clear();
        self.notice = None;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.rebuild();
        cx.notify();
    }
    pub(super) fn poll(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.client.poll() {
            return false;
        }
        self.rebuild();
        self.cancel_hidden();
        // Deactivation already painted the disabled tree. Late cancellation
        // replies must drain without repeatedly invalidating a hidden view.
        if self.active {
            cx.notify();
        }
        true
    }
    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        if let Some(home) = &self.home {
            self.branch(
                Node {
                    root: 0,
                    path: home.clone(),
                },
                "Home".into(),
                0,
                &mut rows,
            );
        }
        self.branch(
            Node {
                root: 1,
                path: "/".into(),
            },
            "Server".into(),
            0,
            &mut rows,
        );
        self.rows = Rc::new(rows);
        self.revision = self.revision.wrapping_add(1);
    }
    fn branch(&self, node: Node, label: String, depth: usize, rows: &mut Vec<Row>) {
        rows.push(Row {
            node: node.clone(),
            depth,
            content: Content::Folder(label),
        });
        if depth >= MAX_DEPTH || !self.expanded.contains(&node) {
            return;
        }
        let folder = self.client.get(&node.path);
        let note = match folder {
            None => Some(Content::Loading),
            Some(folder) if !folder.loaded && folder.pending => Some(Content::Loading),
            Some(folder) if !folder.loaded && folder.error.is_some() => {
                Some(Content::Error(folder.error.clone().unwrap()))
            }
            Some(folder) => {
                let mut empty = true;
                for child in &folder.directories {
                    if self.hidden || !child.name.starts_with('.') {
                        empty = false;
                        self.branch(
                            Node {
                                root: node.root,
                                path: child.path.clone(),
                            },
                            child.name.clone(),
                            depth + 1,
                            rows,
                        );
                    }
                }
                if let Some(error) = &folder.error {
                    Some(Content::Error(format!("{error} Showing cached folders.")))
                } else if folder.pending {
                    Some(Content::Refreshing)
                } else if folder.truncated {
                    Some(Content::Truncated)
                } else if empty {
                    Some(Content::Empty)
                } else {
                    None
                }
            }
        };
        if let Some(content) = note {
            rows.push(Row {
                node,
                depth: depth + 1,
                content,
            });
        }
    }
    fn cancel_hidden(&mut self) {
        let visible: HashSet<_> = self
            .rows
            .iter()
            .filter_map(|row| matches!(row.content, Content::Folder(_)).then_some(row.node.clone()))
            .collect();
        // Collapsing an ancestor or hiding dot folders unmounts those child
        // branches, matching the reference. Reopening never revives old work.
        self.expanded.retain(|node| visible.contains(node));
        let wanted = self
            .expanded
            .iter()
            .map(|node| node.path.clone())
            .collect::<Vec<_>>();
        self.client.cancel_except(&wanted);
    }
    fn current_row(&self, row: &Row, revision: u64, cx: &App) -> bool {
        self.active
            && self.revision == revision
            && self.rows.contains(row)
            && self.scope_guard.as_ref().is_none_or(|guard| guard(cx))
    }
    fn toggle(&mut self, row: &Row, revision: u64, cx: &mut Context<Self>) {
        if !self.current_row(row, revision, cx) || row.depth >= MAX_DEPTH {
            return;
        }
        self.notice = None;
        if self.expanded.remove(&row.node) {
            self.rebuild();
            self.cancel_hidden();
        } else {
            let alias_expanded = self.expanded.iter().any(|node| node.path == row.node.path);
            if !alias_expanded
                && !self.client.request(row.node.path.clone())
                && !self
                    .client
                    .get(&row.node.path)
                    .is_some_and(|folder| folder.pending)
            {
                self.notice = Some("Close a folder branch before opening another.".into());
                cx.notify();
                return;
            }
            self.expanded.insert(row.node.clone());
            self.rebuild();
            self.cancel_hidden();
        }
        cx.notify();
    }
    fn retry(&mut self, row: &Row, revision: u64, cx: &mut Context<Self>) {
        if self.current_row(row, revision, cx) && self.expanded.contains(&row.node) {
            self.client.request(row.node.path.clone());
            self.rebuild();
            cx.notify();
        }
    }
    fn navigate(&mut self, row: &Row, revision: u64, cx: &mut Context<Self>) {
        if self.current_row(row, revision, cx) && matches!(row.content, Content::Folder(_)) {
            cx.emit(Navigate(row.node.path.clone()));
        }
    }
}

impl Render for FolderTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        let revision = self.revision;
        let row_width = rows.iter().map(|row| row.depth).max().unwrap_or(0) as f32 * 12. + 100.;
        div()
            .id("file-tree")
            .test_support()
            .debug_selector(|| "file-tree".into())
            .role(Role::Navigation)
            .aria_label("Folder tree")
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .p(px(crate::controls::PAGE_PADDING))
            .child(
                div()
                    .id("file-tree-scroll")
                    .debug_selector(|| "file-tree-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .overflow_y_hidden()
                    .track_scroll(&self.horizontal)
                    .child(
                        uniform_list(
                            "file-tree-rows",
                            rows.len(),
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|i| {
                                        let row = &rows[i];
                                        let expanded = this.expanded.contains(&row.node);
                                        let base = div()
                                            .h(px(32.))
                                            .w_full()
                                            .pl(px(row.depth as f32 * 12.))
                                            .flex()
                                            .items_center()
                                            .min_w_0();
                                        match &row.content {
                                            Content::Folder(label) => {
                                                let toggle = row.clone();
                                                let navigate = row.clone();
                                                base.id(("folder-row", i))
                                                    .debug_selector(move || format!("folder-row-{i}"))
                                                    .rounded(px(6.))
                                                    .when(this.current == row.node.path, |row| {
                                                        row.bg(theme::git_selection(cx.theme().is_dark()))
                                                    })
                                                    .when(this.active && this.current != row.node.path, |row| {
                                                        row.hover(|row| row.bg(theme::row_hover(cx)))
                                                    })
                                                    .child(
                                                        crate::controls::icon_button((
                                                            "folder-expand",
                                                            i,
                                                        ))
                                                        .w(px(20.))
                                                        .min_w(px(20.))
                                                        .max_w(px(20.))
                                                        .flex_shrink_0()
                                                        .custom(gpui_kit::component::button::ButtonCustomVariant::new(cx))
                                                        .icon(if expanded {
                                                            IconName::ChevronDown
                                                        } else {
                                                            IconName::ChevronRight
                                                        })
                                                        .accessibility_label(format!(
                                                            "{} {label}",
                                                            if expanded {
                                                                "Collapse"
                                                            } else {
                                                                "Expand"
                                                            }
                                                        ))
                                                        .disabled(
                                                            row.depth >= MAX_DEPTH || !this.active,
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.toggle(&toggle, revision, cx)
                                                            },
                                                        )),
                                                    )
                                                    .child(
                                                        crate::controls::button((
                                                            "folder-navigate",
                                                            i,
                                                        ))
                                                        .custom(gpui_kit::component::button::ButtonCustomVariant::new(cx))
                                                        .px_0()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .justify_start()
                                                        .accessibility_label(format!(
                                                            "Open {label} folder"
                                                        ))
                                                        .tooltip(row.node.path.clone())
                                                        .disabled(!this.active)
                                                        .child(
                                                            div()
                                                                .w_full()
                                                                .min_w_0()
                                                                .flex()
                                                                .items_center()
                                                                .gap(px(6.))
                                                                .child(
                                                                    Icon::new(if expanded {
                                                                        IconName::FolderOpen
                                                                    } else {
                                                                        IconName::Folder
                                                                    })
                                                                    .size(px(14.))
                                                                    .text_color(
                                                                        cx.theme().muted_foreground,
                                                                    ),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .flex_1()
                                                                        .min_w_0()
                                                                        .text_ellipsis()
                                                                        .child(label.clone()),
                                                                ),
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.navigate(
                                                                    &navigate, revision, cx,
                                                                )
                                                            },
                                                        )),
                                                    )
                                                    .into_any_element()
                                            }
                                            Content::Loading => base
                                                .id(("folder-loading", i))
                                                .test_support()
                                                .role(Role::Status)
                                                .aria_label("Loading folders")
                                                .px_1()
                                                .child(Skeleton::new().w_full().h(px(12.)))
                                                .into_any_element(),
                                            Content::Refreshing => base
                                                .id(("folder-refreshing", i))
                                                .test_support()
                                                .role(Role::Status)
                                                .aria_label("Refreshing folders")
                                                .text_size(px(11.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Refreshing…")
                                                .into_any_element(),
                                            Content::Error(error) => {
                                                let retry = row.clone();
                                                base.id(("folder-error", i))
                                                    .test_support()
                                                    .role(Role::Alert)
                                                    .aria_label(error.clone())
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .text_ellipsis()
                                                            .text_size(px(11.))
                                                            .text_color(cx.theme().danger)
                                                            .child(error.clone()),
                                                    )
                                                    .child(
                                                        crate::controls::compact_button((
                                                            "folder-retry",
                                                            i,
                                                        ))
                                                        .ghost()
                                                        .label("Retry")
                                                        .tooltip(error.clone())
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.retry(&retry, revision, cx)
                                                            },
                                                        )),
                                                    )
                                                    .into_any_element()
                                            }
                                            Content::Empty | Content::Truncated => base
                                                .text_size(px(11.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child(if row.content == Content::Empty {
                                                    "No subfolders"
                                                } else {
                                                    "More folders not returned"
                                                })
                                                .into_any_element(),
                                        }
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(&self.scroll)
                        .w_full()
                        .min_w(px(row_width))
                        .h_full(),
                    ),
            )
            .when_some(self.notice.clone(), |view, notice| {
                view.child(
                    div()
                        .id("folder-notice")
                        .role(Role::Status)
                        .text_size(px(11.))
                        .p_1()
                        .child(notice),
                )
            })
    }
}
