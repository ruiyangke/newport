//! Project-oriented presentation of the Connections view.
use super::connections_selector::{Choice, Selector};
use super::*;
pub(super) use newport_domain::port_grouping::{Entry, Project};
use std::rc::Rc;

/// Per-server presentation cache. Action planning continues to regroup fresh data.
#[derive(Default)]
pub(super) struct ProjectsCache {
    revisions: Option<[u64; 3]>,
    projects: std::rc::Rc<Vec<Project>>,
    counts: Option<Counts>,
}
impl ProjectsCache {
    fn get(
        &mut self,
        revisions: [u64; 3],
        prepare: impl FnOnce() -> Vec<Project>,
    ) -> std::rc::Rc<Vec<Project>> {
        if self.revisions != Some(revisions) {
            self.projects = std::rc::Rc::new(prepare());
            self.revisions = Some(revisions);
            self.counts = None;
        }
        self.projects.clone()
    }

    fn counts(
        &mut self,
        saved: &[configuration::Tunnel],
        runtime: Option<&configuration::Runtime>,
        server: Option<uuid::Uuid>,
    ) -> Counts {
        *self.counts.get_or_insert_with(|| {
            let mine = saved
                .iter()
                .filter(|t| Some(t.server_id) == server)
                .map(|t| (t.id, runtime.and_then(|runtime| runtime.tunnel(t.id))));
            Counts {
                all: self.projects.iter().map(|p| p.entries.len()).sum(),
                saved: self
                    .projects
                    .iter()
                    .flat_map(|p| &p.entries)
                    .filter(|e| !e.forwards.is_empty())
                    .count(),
                listening: self
                    .projects
                    .iter()
                    .flat_map(|p| &p.entries)
                    .filter(|e| !e.is_saved())
                    .count(),
                projects: self
                    .projects
                    .iter()
                    .filter(|p| !p.key.is_empty() && p.key != newport_domain::port_grouping::SYSTEM)
                    .count(),
                active: mine
                    .clone()
                    .filter(|(_, state)| state.is_some_and(|state| active(state.status)))
                    .count(),
                attention: mine
                    .filter(|(id, state)| {
                        trouble(
                            *state,
                            runtime.map_or(&[], |runtime| runtime.tunnel_health(*id)),
                            None,
                        )
                    })
                    .count(),
            }
        })
    }
}

#[derive(Clone, Copy)]
struct Counts {
    all: usize,
    saved: usize,
    listening: usize,
    projects: usize,
    active: usize,
    attention: usize,
}

const ROWS_PER_CHUNK: usize = 12;

pub(super) struct Viewport {
    pub(super) list: ListState,
    keys: Rc<Vec<String>>,
    pub(super) focus: Rc<Vec<FocusHandle>>,
    width: f32,
}
impl Default for Viewport {
    fn default() -> Self {
        Self {
            list: ListState::new(0, ListAlignment::Top, px(120.)),
            keys: Rc::default(),
            focus: Rc::default(),
            width: 0.,
        }
    }
}
impl Viewport {
    pub(super) fn sync(
        &mut self,
        keys: Rc<Vec<String>>,
        width: f32,
        cx: &App,
    ) -> (ListState, Rc<Vec<FocusHandle>>) {
        // A new projection also invalidates measured heights when its stable
        // row keys are unchanged (for example, updated project labels).
        let changed = !Rc::ptr_eq(&keys, &self.keys);
        if changed || width != self.width {
            let mut offset = self.list.logical_scroll_top();
            offset.item_ix = self
                .keys
                .get(offset.item_ix)
                .and_then(|key| keys.iter().position(|new| new == key))
                .unwrap_or(offset.item_ix.min(keys.len().saturating_sub(1)));
            if changed {
                let old: std::collections::HashMap<_, _> =
                    self.keys.iter().zip(self.focus.iter()).collect();
                self.focus = Rc::new(
                    keys.iter()
                        .map(|key| {
                            old.get(key)
                                .map(|focus| (*focus).clone())
                                .unwrap_or_else(|| cx.focus_handle())
                        })
                        .collect(),
                );
            }
            self.list
                .splice_focusable(0..self.keys.len(), self.focus.iter().cloned().map(Some));
            self.list.scroll_to(offset);
            self.keys = keys;
            self.width = width;
        }
        (self.list.clone(), self.focus.clone())
    }
}

/// A single retained projection per server; changing mode or query replaces it.
/// Cloning a render snapshot shares rows, keys and focus handles in constant time.
#[derive(Default)]
pub(super) struct PreparedCache {
    key: Option<PreparedKey>,
    value: Option<Rc<PreparedRows>>,
}
impl PreparedCache {
    fn get(&mut self, key: PreparedKey, build: impl FnOnce() -> PreparedRows) -> Rc<PreparedRows> {
        if self.key.as_ref() != Some(&key) || self.value.is_none() {
            self.value = Some(Rc::new(build()));
            self.key = Some(key);
        }
        self.value.as_ref().unwrap().clone()
    }
}
/// What the prepared rows were built from.
#[derive(PartialEq)]
struct PreparedKey {
    revisions: [u64; 3],
    query: String,
    filter: usize,
    table: bool,
    sort: usize,
    descending: bool,
    collapsed: std::collections::HashSet<String>,
    expanded: std::collections::HashSet<String>,
    collapsed_worktrees: std::collections::HashSet<(String, String)>,
}
struct PreparedRows {
    keys: Rc<Vec<String>>,
    content: PreparedContent,
}
enum PreparedContent {
    Groups(Rc<Vec<ProjectChunk>>),
    Table(Rc<Vec<connections_rows::Row>>),
}
struct ProjectRows {
    project: Project,
    full: Vec<Entry>,
    rows: Vec<connections_rows::Row>,
    matched_count: usize,
    positions: Vec<(usize, usize)>,
}
#[derive(Default)]
pub(super) struct VisibleCache {
    key: Option<([u64; 3], String, usize)>,
    groups: std::rc::Rc<Vec<std::rc::Rc<ProjectRows>>>,
}
impl VisibleCache {
    fn get(
        &mut self,
        key: ([u64; 3], String, usize),
        build: impl FnOnce() -> Vec<std::rc::Rc<ProjectRows>>,
    ) -> std::rc::Rc<Vec<std::rc::Rc<ProjectRows>>> {
        if self.key.as_ref() != Some(&key) {
            self.groups = std::rc::Rc::new(build());
            self.key = Some(key);
        }
        self.groups.clone()
    }
}
impl ProjectRows {
    fn new(project: Project, full: Vec<Entry>, mut rows: Vec<connections_rows::Row>) -> Self {
        sort_project_rows(&mut rows);
        let show_branches =
            project.worktrees.len() > 1 || project.worktrees.iter().any(|w| w.main != Some(true));
        let mut positions = vec![(0, 0); rows.len()];
        let mut start = 0;
        while start < rows.len() {
            let section = row_section(&rows[start], show_branches);
            let end = (start + 1..rows.len())
                .find(|&i| row_section(&rows[i], show_branches) != section)
                .unwrap_or(rows.len());
            for (index, position) in positions[start..end].iter_mut().enumerate() {
                *position = (index + 1, end - start);
            }
            start = end;
        }
        Self {
            project,
            full,
            matched_count: rows.len(),
            rows,
            positions,
        }
    }
}
struct ProjectChunk {
    data: std::rc::Rc<ProjectRows>,
    range: std::ops::Range<usize>,
    index: usize,
    last: bool,
}
struct GroupLayout {
    query: String,
    projects: usize,
}

/// WorkspaceDetail uses a window media query, but ports use their containing
/// detail's width for density. Keep those two measurements distinct when the
/// sidebar is open or the page reaches the reference's maximum width.
#[derive(Clone, Copy)]
struct DetailLayout {
    horizontal: f32,
    top: f32,
    bottom: f32,
}

impl DetailLayout {
    fn new(_viewport: f32) -> Self {
        Self {
            horizontal: crate::controls::PAGE_PADDING,
            // The toolbar starts at the top edge, like the other list pages.
            top: 0.,
            bottom: crate::controls::PAGE_PADDING,
        }
    }
}

pub(super) fn inner_width(viewport: f32, pane: f32) -> f32 {
    (pane.min(1500.) - DetailLayout::new(viewport).horizontal * 2.).max(0.)
}

fn connections_skeleton(table: bool) -> AnyElement {
    use gpui_kit::component::skeleton::Skeleton;
    div()
        .id("connections-loading")
        .debug_selector(|| "connections-loading".into())
        .role(accesskit::Role::Status)
        .aria_label("Loading listening ports")
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(34.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px_3()
                .child(
                    Skeleton::new()
                        .w(px(if table { 240. } else { 180. }))
                        .h(px(13.)),
                ),
        )
        .children((0..6).map(|_| {
            div()
                .h(px(crate::ui::TWO_LINE_ROW_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .child(Skeleton::new().w(px(60.)).h(px(12.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Skeleton::new().w(relative(0.65)).max_w(px(230.)).h(px(12.))),
                )
                .child(Skeleton::new().w(px(36.)).h(px(22.)).rounded_full())
        }))
        .into_any_element()
}

/// The page's centred column; a notice about the page leads it.
fn connections_viewport(
    viewport: f32,
    notice: Option<AnyElement>,
    content: AnyElement,
) -> impl IntoElement {
    let layout = DetailLayout::new(viewport);
    div()
        .id("connections-scroll")
        .debug_selector(|| "connections-scroll".into())
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .debug_selector(|| "connections-detail".into())
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .w_full()
                .max_w(px(1500.))
                .mx_auto()
                .px(px(layout.horizontal))
                .pt(px(layout.top))
                .pb(px(layout.bottom))
                .when_some(notice, |column, notice| {
                    column.child(div().flex_shrink_0().mb(px(12.)).child(notice))
                })
                .child(content),
        )
}

#[derive(IntoElement)]
struct ConnectionsPage {
    owner: WeakEntity<Newport>,
}

impl RenderOnce for ConnectionsPage {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.owner
            .update(cx, |this, cx| this.render_grouped_ports(window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

/// Whether a forward's connection is up or on its way up.
pub(super) fn active(status: configuration::ConnectionStatus) -> bool {
    matches!(
        status,
        configuration::ConnectionStatus::Connected
            | configuration::ConnectionStatus::Connecting
            | configuration::ConnectionStatus::Reconnecting
    )
}
/// A forward definition's connection and destination checks in `runtime`.
pub(super) fn forward_state<'a>(
    runtime: Option<&'a configuration::Runtime>,
    forward: &configuration::Tunnel,
) -> (
    Option<&'a configuration::ConnectionState>,
    &'a [configuration::DestinationHealth],
) {
    match runtime {
        Some(runtime) => (
            runtime.tunnel(forward.id),
            runtime.tunnel_health(forward.id),
        ),
        None => (None, &[]),
    }
}
pub(super) fn trouble(
    state: Option<&configuration::ConnectionState>,
    health: &[configuration::DestinationHealth],
    remote_port: Option<u16>,
) -> bool {
    let Some(state) = state else {
        return false;
    };
    state.status == configuration::ConnectionStatus::Error
        || state
            .error_message
            .as_deref()
            .is_some_and(|message| !message.is_empty())
        || (state.status == configuration::ConnectionStatus::Connected
            && health
                .iter()
                .filter(|h| remote_port.is_none_or(|port| h.remote_port == port))
                .any(|h| h.message.as_deref().is_some_and(|s| !s.is_empty())))
}
pub(super) fn trouble_for(
    runtime: Option<&configuration::Runtime>,
    t: &configuration::Tunnel,
    remote_port: Option<u16>,
) -> bool {
    let (state, health) = forward_state(runtime, t);
    trouble(state, health, remote_port)
}
fn entry_needs_attention(runtime: Option<&configuration::Runtime>, entry: &Entry) -> bool {
    let remote_port = if entry.is_saved() {
        None
    } else {
        entry.port.number()
    };
    entry
        .forwards
        .iter()
        .any(|t| trouble_for(runtime, t, remote_port))
}

/// Saved forwards whose destination the scan didn't observe: the rows that
/// say "Destination not observed".
pub(super) fn unobserved_forwards(groups: &[Project]) -> Vec<configuration::Tunnel> {
    groups
        .iter()
        .flat_map(|group| &group.entries)
        .filter(|entry| entry.is_saved() && !entry.port.reverse())
        .flat_map(|entry| &entry.forwards)
        .cloned()
        .collect()
}

/// Group only real port rows. Heading controls remain siblings of the list,
/// rather than becoming extra items announced in its row count.
fn port_rows_list(project: &str, section: &str, name: &str, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .id(SharedString::from(format!(
            "connection-rows:{project}:{section}"
        )))
        .role(accesskit::Role::List)
        .aria_label(name.to_owned())
        .flex_shrink_0()
        .children(rows)
        .into_any_element()
}

/// The section is presentation-only; server discovery and project identity are
/// already resolved by the shared core. Reverse forwards follow saved locals.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RowSection {
    Project,
    Worktree(String),
    Saved,
    Reverse,
}

fn row_section(row: &connections_rows::Row, show_branches: bool) -> RowSection {
    if row.entry.is_saved() && row.entry.port.reverse() {
        RowSection::Reverse
    } else if show_branches && let Some(worktree) = &row.worktree {
        RowSection::Worktree(worktree.path.clone())
    } else if row.entry.is_saved() && row.worktree.is_none() {
        RowSection::Saved
    } else {
        RowSection::Project
    }
}

fn sort_project_rows(rows: &mut [connections_rows::Row]) {
    rows.sort_by_key(|row| {
        let worktree = &row.worktree;
        (
            row.entry.is_saved() && worktree.is_none(),
            row.entry.is_saved() && row.entry.port.reverse(),
            worktree.is_some(),
            worktree.as_ref().is_some_and(|w| w.main != Some(true)),
            worktree.as_ref().map(|w| w.path.clone()),
            row.number,
        )
    });
}

fn worktree_heading(worktree: &newport_domain::port_grouping::Worktree) -> (String, &'static str) {
    let branch = worktree
        .branch
        .as_deref()
        .filter(|branch| !branch.is_empty());
    let detached = branch.is_some_and(|branch| {
        (7..=40).contains(&branch.len()) && branch.bytes().all(|b| b.is_ascii_hexdigit())
    });
    let title = match branch.filter(|_| !detached) {
        Some(branch) => branch.to_owned(),
        None => worktree
            .path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(&worktree.path)
            .to_owned(),
    };
    (
        newport_domain::port_discovery::display(&title),
        if detached {
            "Detached"
        } else if worktree.main == Some(true) {
            "Main checkout"
        } else {
            ""
        },
    )
}

// Retain one row as the collapsed section's heading anchor. Hidden rows never
// enter the virtual list, so a large folded checkout consumes one small item.
fn worktree_disclosure_rows(
    rows: &[connections_rows::Row],
    project: &str,
    collapsed: &std::collections::HashSet<(String, String)>,
) -> Vec<connections_rows::Row> {
    let mut seen = std::collections::HashSet::new();
    rows.iter()
        .filter(|row| {
            row.worktree.as_ref().is_none_or(|worktree| {
                !collapsed.contains(&(project.to_owned(), worktree.path.clone()))
                    || seen.insert(worktree.path.clone())
            })
        })
        .cloned()
        .collect()
}

impl Newport {
    pub(super) fn connection_projects(&self) -> Vec<Project> {
        let Some(server) = self.server_id() else {
            return Vec::new();
        };
        let known = self.connection_bookmarks();
        let ports = self
            .connections_state
            .get(&self.server)
            .and_then(|state| state.reading.as_ref())
            .and_then(newport_client::discovery::Reading::view)
            .map(|view| view.members().to_vec())
            .unwrap_or_default();
        let saved = self.tunnels();
        if let Some(state) = self.connections_state.get(&self.server) {
            state
                .worktree_associations
                .borrow_mut()
                .group(ports, saved, server, known)
        } else {
            newport_domain::port_grouping::group(ports, saved, server, known)
        }
    }

    /// Only a current, successful reading of a connected server can show that
    /// a forward's destination wasn't observed. A scan restored from before
    /// the page was left counts only once a new one replaces it.
    pub(super) fn listeners_known(&self, cx: &App) -> bool {
        self.resources.get("ports").is_some_and(|r| {
            r.has_value() && r.error.is_none() && !r.stale && r.updated_at.is_some()
        }) && self
            .connections_state
            .get(&self.server)
            .is_some_and(|state| !state.restored)
            && !self.needle_missing()
            && self.connection_down(cx).is_none()
    }

    pub(super) fn grouped_ports(&self, cx: &mut Context<Self>) -> AnyElement {
        ConnectionsPage {
            owner: cx.weak_entity(),
        }
        .into_any_element()
    }

    fn render_grouped_ports(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let viewport = f32::from(window.viewport_size().width);
        let content_width = inner_width(viewport, self.content_width);
        let Some(state) = self.connections_state.get(&self.server) else {
            return div().into_any_element();
        };
        let Some(input) = &state.input else {
            return div().into_any_element();
        };
        let query = input.read(cx).value().trim().to_lowercase();
        let saved = self.tunnels();
        let runtime = self.runtime();
        let show_owner = cx.weak_entity();
        let view_owner = show_owner.clone();
        let show_server = self.server.clone();
        let view_server = self.server.clone();
        let [ports_revision, projects_revision] = ["ports", "connection-projects"].map(|key| {
            self.resources
                .get(key)
                .map_or(0, |resource| resource.data_revision)
        });
        let revisions = [ports_revision, projects_revision, state.snapshot_revision];
        let (
            groups,
            Counts {
                all,
                saved: saved_count,
                listening,
                projects,
                active,
                attention,
            },
        ) = {
            let mut cache = state.projects.borrow_mut();
            let groups = cache.get(revisions, || self.connection_projects());
            let counts = cache.counts(saved, runtime, self.server_id());
            (groups, counts)
        };
        let no_entries = groups.is_empty();
        let resource = self.resources.get("ports");
        let has_reading = resource.is_some_and(state::Resource::has_value);
        // Discovery runs on Needle; saved forwards don't.
        let needle_missing = self.needle_missing();
        let initial_loading =
            !needle_missing && !has_reading && resource.is_none_or(|r| r.error.is_none());
        let manual_pending = resource.is_some_and(|r| r.manual_pending);
        let cleanable = self.cleanable_forwards(&groups, cx).len();
        let filter = if state.filter == 2 && attention == 0 {
            0
        } else {
            state.filter
        };
        let visible = state
            .visible
            .borrow_mut()
            .get((revisions, query.clone(), filter), || {
                groups
                    .iter()
                    .cloned()
                    .filter_map(|project| {
                        let key = &project.key;
                        let entries = &project.entries;
                        let branches = project.worktrees.len() > 1;
                        let full = entries.clone();
                        let entries: Vec<_> = entries
                            .iter()
                            .filter(|e| {
                                let category = match filter {
                                    1 => !e.forwards.is_empty(),
                                    2 => entry_needs_attention(runtime, e),
                                    _ => true,
                                };
                                category && connections::entry_matches(&project, e, &query)
                            })
                            .cloned()
                            .map(|e| {
                                let mut row = connections_rows::Row::new(key, e, runtime, branches);
                                if let Some(view) = state
                                    .reading
                                    .as_ref()
                                    .and_then(newport_client::discovery::Reading::view)
                                {
                                    row.enrich(view);
                                }
                                row.project_name = project.name.clone();
                                row
                            })
                            .collect();
                        (!entries.is_empty())
                            .then(|| std::rc::Rc::new(ProjectRows::new(project, full, entries)))
                    })
                    .collect()
            });
        let empty = visible.is_empty();
        let shown = visible.iter().map(|group| group.rows.len()).sum::<usize>();
        let total = match filter {
            1 => saved_count,
            2 => groups
                .iter()
                .flat_map(|group| &group.entries)
                .filter(|entry| entry_needs_attention(runtime, entry))
                .count(),
            _ => all,
        };
        let noun = if filter == 1 {
            "saved connection"
        } else {
            "connection"
        };
        let mut displayed_summary = if shown == total {
            format!("{shown} {noun}{}", if shown == 1 { "" } else { "s" })
        } else {
            format!(
                "{shown} of {total} {noun}{}",
                if total == 1 { "" } else { "s" }
            )
        };
        if filter == 0 && query.is_empty() && has_reading {
            displayed_summary.push_str(&format!(" · {listening} listening"));
        }
        let mut summary = format!(
            "{active} {} enabled",
            if active == 1 { "forward" } else { "forwards" }
        );
        if has_reading {
            summary.push_str(&format!(" · {listening} listening"));
        }
        if has_reading && projects > 0 {
            summary.push_str(&format!(
                " in {projects} {}",
                if projects == 1 { "project" } else { "projects" }
            ));
        }
        let updated = self
            .resources
            .get("ports")
            .and_then(|r| r.updated_at)
            .and_then(|at| chrono::Duration::from_std(at.elapsed()).ok())
            .map(|elapsed| {
                (chrono::Local::now() - elapsed)
                    .format("%-I:%M %p")
                    .to_string()
            });
        let content = if empty {
            div().into_any_element()
        } else {
            let version = PreparedKey {
                revisions,
                query: query.clone(),
                filter,
                table: state.table,
                sort: state.sort,
                descending: state.descending,
                collapsed: state.collapsed.clone(),
                expanded: state.expanded.clone(),
                collapsed_worktrees: state.collapsed_worktrees.clone(),
            };
            let prepared = state.prepared.borrow_mut().get(version, || {
                if state.table {
                    let mut rows: Vec<_> = visible
                        .iter()
                        .flat_map(|project| project.rows.iter().cloned())
                        .collect();
                    connections_rows::sort(&mut rows, state.sort, state.descending);
                    return PreparedRows {
                        keys: Rc::new(
                            rows.iter()
                                .map(|row| format!("table:{}", row.key))
                                .collect(),
                        ),
                        content: PreparedContent::Table(Rc::new(rows)),
                    };
                }
                let mut chunks = Vec::new();
                let mut keys = Vec::new();
                for (index, data) in visible.iter().enumerate() {
                    let project = &data.project;
                    let collapsed = query.is_empty()
                        && (state.collapsed.contains(&project.key)
                            || (project.key.is_empty()
                                && projects > 0
                                && !state.expanded.contains(&project.key)));
                    let data = if query.is_empty()
                        && !state.collapsed_worktrees.is_empty()
                        && (project.worktrees.len() > 1
                            || project.worktrees.iter().any(|w| w.main != Some(true)))
                    {
                        let rows = worktree_disclosure_rows(
                            &data.rows,
                            &project.key,
                            &state.collapsed_worktrees,
                        );
                        let mut folded = ProjectRows::new(project.clone(), data.full.clone(), rows);
                        folded.matched_count = data.matched_count;
                        Rc::new(folded)
                    } else {
                        data.clone()
                    };
                    let count = if collapsed { 0 } else { data.rows.len() };
                    for start in (0..count.max(1)).step_by(ROWS_PER_CHUNK) {
                        let end = (start + ROWS_PER_CHUNK).min(count);
                        keys.push(format!("{}:{start}", data.project.key));
                        chunks.push(ProjectChunk {
                            data: data.clone(),
                            range: start..end,
                            index,
                            last: end == count,
                        });
                    }
                }
                PreparedRows {
                    keys: Rc::new(keys),
                    content: PreparedContent::Groups(Rc::new(chunks)),
                }
            });
            let (list_state, focus) =
                state
                    .viewport
                    .borrow_mut()
                    .sync(prepared.keys.clone(), content_width, cx);
            match &prepared.content {
                PreparedContent::Table(rows) => {
                    self.ports_table(rows.clone(), list_state, focus, content_width, cx)
                }
                PreparedContent::Groups(chunks) => {
                    let chunks = chunks.clone();
                    let layout = GroupLayout {
                        query: query.clone(),
                        projects,
                    };
                    list(
                        list_state,
                        cx.processor(move |this, index: usize, _, cx| {
                            div()
                                .tab_group()
                                .tab_index(index as isize)
                                .tab_stop(false)
                                .track_focus(&focus[index])
                                .child(this.project_chunk(&chunks[index], &layout, cx))
                                .into_any_element()
                        }),
                    )
                    .flex_1()
                    .min_h_0()
                    .into_any_element()
                }
            }
        };
        let problem = resource
            .and_then(|r| r.error.clone())
            .filter(|_| !needle_missing);
        // Discovery has never worked here: say why, and that forwarding by
        // port number does not depend on it.
        let undiscovered = !has_reading && problem.is_some();
        // The backend is replacing an outdated Needle: progress, not failure.
        let updating = problem
            .as_deref()
            .is_some_and(|p| p.starts_with("Updating Needle"));
        // While the server isn't connected discovery can't run; that's a
        // note, not an error.
        let offline = self.connection_down(cx);
        let update_notice = match offline {
            Some(phase) if has_reading => Some(crate::connection_wait::last_reading(phase)),
            Some(phase) => Some(format!(
                "{}. Discovered ports appear once the server is connected; saved forwards still work.",
                phase.label()
            )),
            None => problem
                .clone()
                .filter(|_| updating && !(undiscovered && no_entries)),
        };
        let discovery_error = problem
            .as_ref()
            .filter(|_| offline.is_none() && !updating && !(undiscovered && no_entries))
            .map(|error| {
                if has_reading {
                    format!("Couldn’t refresh ports: {error} Showing the last available reading.")
                } else {
                    format!("Couldn’t discover ports: {error} Saved forwards still work.")
                }
            });
        // The shared page toolbar; it may wrap to a second row when narrow.
        let toolbar = crate::controls::page_toolbar(cx)
            .id("connections-toolbar")
            .debug_selector(|| "connections-toolbar".into())
            .flex_wrap()
            .h_auto()
            .min_h(px(crate::controls::TOOLBAR_HEIGHT))
            // The fixed toolbar's centring; its height includes the rule.
            .py(px((crate::controls::TOOLBAR_HEIGHT
                - crate::controls::CONTROL_HEIGHT
                - 1.)
                / 2.))
            .child(crate::controls::page_search(input, "Find a connection", cx))
            .child(Selector::new(
                format!("port-filter:{}", self.server),
                "Show",
                filter,
                ["All", "Saved", "Needs attention"]
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| *i != 2 || attention > 0)
                    .map(|(i, name)| Choice {
                        danger: i == 2,
                        count: match i {
                            0 => Some(all),
                            1 => Some(saved_count),
                            _ => None,
                        },
                        tooltip: match i {
                            0 => Some("Discovered ports and saved forwards"),
                            1 => Some("Saved forwards, including those switched off"),
                            _ => None,
                        },
                        ..Choice::text(i, name)
                    })
                    .collect(),
                move |value, _, cx| {
                    let _ = show_owner.update(cx, |this, cx| {
                        if this.server != show_server {
                            return;
                        }
                        this.connections_state
                            .entry(show_server.clone())
                            .or_default()
                            .filter = value;
                        cx.notify();
                    });
                },
            ))
            .child(Selector::new(
                format!("ports-view:{}", self.server),
                "View",
                usize::from(state.table),
                [
                    (IconName::ListTree, "Projects view", "Group by project"),
                    (IconName::Table2, "Ports view", "All ports in one table"),
                ]
                .into_iter()
                .enumerate()
                .map(|(i, (icon, name, tooltip))| Choice {
                    icon: Some(icon),
                    tooltip: Some(tooltip),
                    ..Choice::text(i, name)
                })
                .collect(),
                move |value, _, cx| {
                    let _ = view_owner.update(cx, |this, cx| {
                        if this.server != view_server {
                            return;
                        }
                        this.connections_state
                            .entry(view_server.clone())
                            .or_default()
                            .table = value == 1;
                        cx.notify();
                    });
                },
            ))
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(crate::connections::diagnostics::control(
                        &self.server,
                        state
                            .reading
                            .as_ref()
                            .and_then(newport_client::discovery::Reading::view)
                            .cloned(),
                    ))
                    // Like the Needs attention filter, shown only when it applies.
                    .when(cleanable > 0, |actions| {
                        actions.child(
                            crate::controls::icon_button("clean-up-forwards")
                                .ghost()
                                .icon(IconName::BroomSparkles)
                                .accessibility_label("Clean up forwards")
                                .tooltip("Clean up forwards whose destination wasn’t observed")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_clean_up_forwards(window, cx)
                                })),
                        )
                    })
                    .child(
                        crate::controls::icon_button("discover")
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .accessibility_label("Refresh ports")
                            .tooltip("Refresh")
                            .loading(manual_pending)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.discover_connections(true);
                                cx.notify();
                            })),
                    )
                    .child(
                        crate::controls::button("new-forward")
                            .icon(IconName::Plus)
                            .label("New forward")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_tunnel(None, window, cx)
                            })),
                    ),
            );
        let content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .pt(px(6.))
            .when(initial_loading, |view| {
                view.child(connections_skeleton(state.table))
            })
            .when(empty && !no_entries, |v| {
                v.child(empty::compact("No connections match.", cx).flex_shrink_0())
            })
            .when(no_entries && undiscovered && updating, |view| {
                view.child(empty::pane(
                    IconName::RefreshCw,
                    "Updating Needle",
                    problem.clone().unwrap_or_default(),
                    cx,
                ))
            })
            .when(no_entries && undiscovered && !updating, |view| {
                let error = problem.clone().unwrap_or_default();
                // The agent's own words name the fix; offer to go there.
                let setup = error.contains("Open Overview");
                view.child(
                    empty::pane(
                        IconName::ServerOff,
                        "Port discovery isn’t available",
                        format!("{error} You can still forward a port by number with New forward."),
                        cx,
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .when(setup, |v| {
                                v.child(
                                    crate::controls::button("connections-setup")
                                        .primary()
                                        .label("Open Overview")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.navigate(Page::Overview, cx)
                                        })),
                                )
                            })
                            .child(
                                crate::controls::button("connections-retry")
                                    .label("Try again")
                                    .loading(manual_pending)
                                    .disabled(manual_pending)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.discover_connections(true);
                                        cx.notify();
                                    })),
                            ),
                    ),
                )
            })
            .when(no_entries && needle_missing, |view| {
                view.child(self.needle_required("Port discovery", cx))
            })
            .when(no_entries && has_reading && !needle_missing, |view| {
                view.child(empty::pane(
                    IconName::Network,
                    "Nothing to forward yet",
                    "Start a service on this server, or use New forward to save a connection.",
                    cx,
                ))
            })
            .when(!initial_loading, |view| view.child(content))
            .into_any_element();
        let summary = div()
            .id("connections-summary")
            .role(accesskit::Role::Status)
            .aria_label(if attention > 0 {
                format!("{displayed_summary}; {summary} · {attention} need attention")
            } else {
                format!("{displayed_summary}; {summary}")
            })
            .flex()
            .gap_1()
            .min_w_0()
            .truncate()
            .child(if has_reading {
                displayed_summary
            } else if needle_missing {
                "Port discovery needs Needle".into()
            } else if updating {
                "Updating Needle…".into()
            } else if undiscovered {
                "Port discovery unavailable".into()
            } else {
                "Discovering ports…".into()
            })
            .when(attention > 0, |v| {
                v.child(
                    div()
                        .text_color(theme::error(cx.theme().is_dark()))
                        .child(format!("· {attention} need attention")),
                )
            });
        let footer = crate::controls::status_bar(cx)
            .left(summary)
            .when_some(updated, |bar, t| {
                let message = format!("Updated {t}");
                bar.right(
                    div()
                        .id("connections-updated")
                        .role(accesskit::Role::Status)
                        .aria_label(format!("{message}; refreshes every 30 seconds"))
                        .child(message),
                )
            });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(toolbar)
            .when_some(update_notice, |view, message| {
                view.child(
                    div()
                        .id("connections-updating")
                        .debug_selector(|| "connections-updating".into())
                        .role(accesskit::Role::Status)
                        .aria_label(message.clone())
                        .flex_shrink_0()
                        .px(px(crate::controls::TOOLBAR_INSET))
                        .py(px(8.))
                        .text_color(cx.theme().muted_foreground)
                        .child(message),
                )
            })
            .when_some(discovery_error, |view, message| {
                view.child(
                    crate::controls::alert("connections-discovery-error", message, cx)
                        .debug_selector(|| "connections-discovery-error".into())
                        .flex_shrink_0()
                        .px(px(crate::controls::TOOLBAR_INSET))
                        .py(px(8.)),
                )
            })
            .child(connections_viewport(
                viewport,
                (needle_missing && !no_entries)
                    .then(|| self.needle_required_notice("Port discovery", cx)),
                content,
            ))
            .child(
                div()
                    .flex_shrink_0()
                    .debug_selector(|| "connections-footer".into())
                    .child(footer),
            )
            .into_any_element()
    }
    fn project_chunk(
        &self,
        chunk: &ProjectChunk,
        layout: &GroupLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.connections_state[&self.server];
        let project = &chunk.data.project;
        let full = &chunk.data.full;
        let rows = &chunk.data.rows;
        let index = chunk.index;
        let first = chunk.range.start == 0;
        let GroupLayout { query, projects } = layout;
        let projects = *projects;

        let key = project.key.clone();
        let collapsed = query.is_empty()
            && (state.collapsed.contains(&key)
                || (key.is_empty() && projects > 0 && !state.expanded.contains(&key)));
        let click_key = key.clone();
        let click_server = self.server.clone();
        let name = project.name.clone();
        let single_branch =
            if project.worktrees.len() == 1 && project.worktrees[0].main == Some(true) {
                project.worktrees[0].branch.clone()
            } else {
                None
            };
        // Both project and checkout actions use the full scope, independent of search.
        let show_branches =
            project.worktrees.len() > 1 || project.worktrees.iter().any(|w| w.main != Some(true));
        let mut children = Vec::new();
        let mut previous_section = chunk
            .range
            .start
            .checked_sub(1)
            .map(|i| row_section(&rows[i], show_branches));
        let mut list_rows = Vec::new();
        let mut list_section = String::new();
        let mut list_name = format!("{name} ports");
        let count = chunk.data.matched_count;
        if !collapsed {
            for (offset, row) in rows[chunk.range.clone()].iter().cloned().enumerate() {
                let worktree = &row.worktree;
                let path = worktree
                    .as_ref()
                    .map(|w| w.path.as_str())
                    .unwrap_or_default();
                let kind = row_section(&row, show_branches);
                let (section, section_name) = if kind == RowSection::Reverse {
                    (
                        "reverse".to_owned(),
                        format!("{name} forwards from this computer"),
                    )
                } else if kind == RowSection::Saved {
                    ("saved".to_owned(), format!("{name} saved forwards"))
                } else if matches!(kind, RowSection::Worktree(_)) {
                    let branch = worktree
                        .as_ref()
                        .and_then(|w| w.branch.as_deref())
                        .filter(|branch| !branch.is_empty())
                        .unwrap_or(path);
                    (
                        format!("worktree:{path}"),
                        format!("{name} · {branch} ports"),
                    )
                } else {
                    ("project".to_owned(), format!("{name} ports"))
                };
                if section != list_section {
                    if !list_rows.is_empty() {
                        children.push(port_rows_list(
                            &key,
                            &format!("{list_section}:{}", chunk.range.start),
                            &list_name,
                            std::mem::take(&mut list_rows),
                        ));
                    }
                    list_section = section;
                    list_name = section_name;
                }
                let new_section = previous_section.as_ref() != Some(&kind);
                if new_section && kind == RowSection::Reverse {
                    children.push(
                        div()
                            .id(SharedString::from(format!("reverse-heading:{key}")))
                            .role(accesskit::Role::Heading)
                            .aria_label("From this computer, shared with the server")
                            // Aligned with the foldable group headings above it.
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(crate::controls::COMPACT_HEIGHT))
                            .mt(px(4.))
                            .px(px(11.))
                            .text_size(px(crate::ui::FONT_SIZE))
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(IconName::Laptop).size(px(12.)))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(cx.theme().foreground.opacity(0.85))
                                    .child("From this computer"),
                            )
                            .child("shared with the server")
                            .into_any_element(),
                    );
                } else if new_section && kind == RowSection::Saved && show_branches {
                    children.push(
                        div()
                            .id(SharedString::from(format!("saved-heading:{key}")))
                            .role(accesskit::Role::Heading)
                            .aria_label("Other saved forwards")
                            .flex()
                            .items_center()
                            .h(px(crate::controls::COMPACT_HEIGHT))
                            .mt(px(4.))
                            .pl(px(27.))
                            .pr(px(11.))
                            .text_size(px(crate::ui::FONT_SIZE))
                            .text_color(cx.theme().muted_foreground)
                            .child("Other saved forwards")
                            .into_any_element(),
                    );
                } else if new_section && matches!(kind, RowSection::Worktree(_)) {
                    let checkout = worktree.as_ref().expect("worktree section");
                    let (title, kind_label) = worktree_heading(checkout);
                    let disclosure_key = (key.clone(), path.to_owned());
                    let folded =
                        query.is_empty() && state.collapsed_worktrees.contains(&disclosure_key);
                    let server = self.server.clone();
                    let scope = connection_batches::Scope {
                        project: key.clone(),
                        worktree: Some(path.to_owned()),
                    };
                    let section: Vec<_> = full
                        .iter()
                        .filter(|e| scope.includes_checkout(e))
                        .cloned()
                        .collect();
                    let tally = format!(
                        "{} {}",
                        section.len(),
                        if section.len() == 1 { "port" } else { "ports" }
                    );
                    let heading_id = SharedString::from(format!("worktree-heading:{key}:{path}"));
                    children.push(
                        connection_batches::Heading::new(
                            div()
                                .id(heading_id)
                                .role(accesskit::Role::Heading)
                                .aria_label(
                                    [title.as_str(), kind_label, tally.as_str()]
                                        .into_iter()
                                        .filter(|part| !part.is_empty())
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                )
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .min_h(px(crate::ui::ROW_HEIGHT))
                                .mt(px(4.))
                                .pl(px(16.))
                                .pr(px(12.))
                                .child(
                                    crate::controls::group_heading(
                                        SharedString::from(format!("fold-worktree:{key}:{path}")),
                                        &title,
                                        section.len(),
                                        !folded,
                                        cx,
                                    )
                                    .flex_1()
                                    .min_w_0()
                                    .hover_path("Worktree path", path.to_owned())
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if this.server != server {
                                                return;
                                            }
                                            let state = this
                                                .connections_state
                                                .entry(server.clone())
                                                .or_default();
                                            if folded {
                                                state.collapsed_worktrees.remove(&disclosure_key);
                                            } else {
                                                state
                                                    .collapsed_worktrees
                                                    .insert(disclosure_key.clone());
                                            }
                                            cx.notify();
                                        },
                                    )),
                                )
                                .when(!kind_label.is_empty(), |v| {
                                    v.child(
                                        label(kind_label)
                                            .text_size(px(crate::ui::FONT_SIZE))
                                            .flex_shrink_0(),
                                    )
                                }),
                            self.port_batch_button(scope, &title, &section, cx),
                        )
                        .into_any_element(),
                    );
                }
                previous_section = Some(kind);
                if query.is_empty()
                    && show_branches
                    && worktree.as_ref().is_some_and(|w| {
                        state
                            .collapsed_worktrees
                            .contains(&(key.clone(), w.path.clone()))
                    })
                {
                    continue;
                }
                let nested = show_branches && worktree.is_some();
                list_rows.push(self.connection_row(
                    row,
                    chunk.data.positions[chunk.range.start + offset],
                    nested,
                    cx,
                ));
            }
            if !list_rows.is_empty() {
                children.push(port_rows_list(
                    &key,
                    &format!("{list_section}:{}", chunk.range.start),
                    &list_name,
                    list_rows,
                ));
            }
        }
        div()
            .id(SharedString::from(format!(
                "project-section:{key}:{}",
                chunk.range.start
            )))
            .role(accesskit::Role::Group)
            .aria_label(format!("{name} project"))
            .when(first, |v| v.border_t_1())
            .border_color(cx.theme().border.opacity(0.6))
            .when(first, |v| v.pt(px(6.)))
            .when(chunk.last, |v| v.pb(px(6.)))
            .when(first && index == 0, |v| v.border_t_0().pt(px(2.)))
            .flex_shrink_0()
            .when(first, |v| {
                v.child(connection_batches::Heading::new(
                    div()
                        .id(SharedString::from(format!("project-heading:{key}")))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_h(px(34.))
                        .pr(px(12.))
                        .child(
                            crate::controls::group_heading(
                                SharedString::from(format!("fold-{key}")),
                                &name,
                                count,
                                !collapsed,
                                cx,
                            )
                            .flex_1()
                            .min_w_0()
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if this.server != click_server {
                                        return;
                                    }
                                    let state = this
                                        .connections_state
                                        .entry(this.server.clone())
                                        .or_default();
                                    if collapsed {
                                        state.collapsed.remove(&click_key);
                                        state.expanded.insert(click_key.clone());
                                    } else {
                                        state.collapsed.insert(click_key.clone());
                                        state.expanded.remove(&click_key);
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                        .when_some(single_branch, |v, branch| {
                            v.child(
                                div()
                                    .flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap_1()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        Icon::new(IconName::GitBranch)
                                            .size(px(crate::ui::SMALL_ICON)),
                                    )
                                    .child(
                                        label(branch)
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(crate::ui::FONT_SIZE)),
                                    ),
                            )
                        })
                        .children(
                            self.port_project_stop_button(&key, &name, full, cx)
                                .map(|action| action.button),
                        ),
                    self.port_batch_button(
                        connection_batches::Scope {
                            project: key.clone(),
                            worktree: None,
                        },
                        &name,
                        full,
                        cx,
                    ),
                ))
            })
            .children(children)
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{
        Entry, RowSection, entry_needs_attention, row_section, sort_project_rows, trouble_for,
    };
    use crate::connections::fixtures::Listener;
    use crate::connections_rows::Row;
    use crate::project_library_state::typed_fixture::projects_reply;
    use gpui_kit::{
        self as gpui, Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px,
        size,
    };
    use newport_domain::port_grouping::Port;
    use newport_protocol::desktop::configuration;

    const T: &str = "00000000-0000-4000-8000-0000000000aa";
    /// A forward on the fixture server, numbered `id`.
    fn forward(id: u128) -> configuration::Tunnel {
        crate::snapshot_fixture::tunnel(
            uuid::Uuid::from_u128(id),
            crate::snapshot_fixture::SERVER,
            0,
            0,
        )
    }
    /// A forward from the server's `local_port` back to this computer.
    fn reverse(id: u128, local_port: u16) -> configuration::Tunnel {
        configuration::Tunnel {
            direction: configuration::Direction::Remote,
            local_port,
            ..forward(id)
        }
    }
    /// The App forward the Connections fixtures save for `server`.
    fn app_forward(server: &str) -> configuration::Tunnel {
        let mut forward = crate::snapshot_fixture::tunnel(
            uuid::Uuid::from_u128(0xf04),
            uuid::Uuid::parse_str(server).unwrap(),
            9000,
            8000,
        );
        forward.name = "App".into();
        forward.remote_host = "127.0.0.1".into();
        forward.project = Some("/srv/project-0".into());
        forward
    }
    fn health(
        remote_port: u16,
        status: configuration::DestinationStatus,
        message: Option<&str>,
    ) -> configuration::DestinationHealth {
        configuration::DestinationHealth {
            local_port: remote_port,
            remote_port,
            status,
            message: message.map(Into::into),
            checked_at: None,
        }
    }
    /// A runtime where forward `T` has this connection and these checks.
    fn forward_runtime(
        status: configuration::ConnectionStatus,
        error: Option<&str>,
        values: Vec<configuration::DestinationHealth>,
    ) -> configuration::Runtime {
        use crate::snapshot_fixture as fixture;
        let id = uuid::Uuid::parse_str(T).unwrap();
        let mut runtime = fixture::runtime();
        fixture::set(
            &mut runtime.tunnels,
            id,
            configuration::ConnectionState {
                error_message: error.map(Into::into),
                ..fixture::connection(status)
            },
        );
        fixture::set(
            &mut runtime.tunnel_health,
            id,
            configuration::DestinationHealthList { values },
        );
        runtime
    }

    #[test]
    fn saved_checkout_rows_stay_with_their_live_siblings() {
        let make = |port| {
            Row::new(
                "/repo",
                Entry {
                    port,
                    addresses: vec![],
                    forwards: vec![],
                },
                None,
                true,
            )
        };
        let mut rows = vec![
            make(Port::Saved {
                definition: configuration::Tunnel {
                    remote_port: 8001,
                    ..forward(1)
                },
                worktree: Some(newport_domain::port_grouping::Worktree {
                    path: "/repo/a".into(),
                    branch: Some("a".into()),
                    main: Some(false),
                }),
            }),
            make(
                Listener::new(8002)
                    .project("/repo")
                    .checkout("/repo/b", Some("b"), Some(false))
                    .port(),
            ),
            make(
                Listener::new(8000)
                    .project("/repo")
                    .checkout("/repo/a", Some("a"), Some(false))
                    .port(),
            ),
        ];
        sort_project_rows(&mut rows);
        assert_eq!(
            rows.iter()
                .map(|r| row_section(r, true))
                .collect::<Vec<_>>(),
            vec![
                RowSection::Worktree("/repo/a".into()),
                RowSection::Worktree("/repo/a".into()),
                RowSection::Worktree("/repo/b".into())
            ]
        );
        let collapsed = [("/repo".to_owned(), "/repo/a".to_owned())]
            .into_iter()
            .collect();
        assert_eq!(
            super::worktree_disclosure_rows(&rows, "/repo", &collapsed).len(),
            2
        );
        assert_eq!(
            super::worktree_disclosure_rows(&rows, "/other", &collapsed).len(),
            3
        );
    }

    #[test]
    fn worktree_heading_uses_directory_for_detached_checkout() {
        use newport_domain::port_grouping::Worktree;
        let worktree = Worktree {
            path: "/scratch/review-demo".into(),
            branch: Some("d3c15da".into()),
            main: Some(false),
        };
        assert_eq!(
            super::worktree_heading(&worktree),
            ("review-demo".into(), "Detached")
        );
        assert_eq!(
            super::worktree_heading(&Worktree {
                branch: Some("feature/search".into()),
                ..worktree
            }),
            ("feature/search".into(), "")
        );
    }

    #[gpui::test]
    fn hidden_forwarding_updates_invalidate_retained_projection_but_other_features_do_not(
        cx: &mut TestAppContext,
    ) {
        use crate::page_perf_tests::{connections_fixture, frame};
        use std::rc::Rc;
        let (view, visual) = connections_fixture(cx, true);
        let server = view.read_with(visual, |view, _| view.server.clone());
        let (work, requests) = std::sync::mpsc::sync_channel(64);
        let (replies, incoming) = std::sync::mpsc::channel();
        view.update(visual, |view, cx| {
            view.work = work.into();
            view.replies = incoming;
            let mut snapshot = view.snapshot.clone().unwrap();
            snapshot.config.tunnels = vec![app_forward(&server)];
            view.sync_connections_snapshot(&snapshot);
            view.snapshot = Some(snapshot);
            cx.notify();
        });
        frame(visual);
        let prepared = |visual: &mut gpui::VisualTestContext| {
            view.read_with(visual, |view, _| {
                view.connections_state[&server]
                    .prepared
                    .borrow()
                    .value
                    .clone()
                    .unwrap()
            })
        };
        let before = prepared(visual);
        view.update(visual, |view, cx| {
            view.navigate(crate::Page::Integration, cx)
        });
        frame(visual);
        assert!(
            visual
                .debug_bounds(crate::page_perf_tests::listener_selector(8000, 1))
                .is_none()
        );
        let accept = |next: configuration::Snapshot, visual: &mut gpui::VisualTestContext| {
            view.update(visual, |view, cx| {
                view.last_refresh = std::time::Instant::now();
                let generation = requests
                    .try_iter()
                    .find(|request| request.key == "snapshot")
                    .map(|request| request.generation)
                    .unwrap_or_else(|| view.resources.begin("snapshot").unwrap());
                replies
                    .send(crate::snapshot_fixture::completion(generation, next))
                    .unwrap();
                view.tick(cx);
            });
            frame(visual);
        };
        use crate::snapshot_fixture as fixture;
        use configuration::ConnectionStatus;
        let mut connected = view.read_with(visual, |view, _| view.snapshot.clone().unwrap());
        let forward = app_forward(&server).id;
        fixture::set(
            &mut connected.runtime.tunnels,
            forward,
            fixture::connection(ConnectionStatus::Connected),
        );
        accept(connected.clone(), visual);
        view.update(visual, |view, cx| view.navigate(crate::Page::Ports, cx));
        frame(visual);
        assert!(
            view.read_with(visual, |view, _| view.resources.get("snapshot").is_some()),
            "the shell snapshot is shared across navigation"
        );
        let current = prepared(visual);
        assert!(!Rc::ptr_eq(&before, &current));
        assert_eq!(
            view.read_with(visual, |view, _| view.connections_state[&server]
                .projects
                .borrow()
                .counts
                .unwrap()
                .active),
            1
        );
        assert!(visual.debug_bounds("connections-footer").is_some());
        fixture::set(
            &mut connected.runtime.clipboard,
            fixture::SERVER,
            fixture::connection(ConnectionStatus::Connecting),
        );
        fixture::set(
            &mut connected.runtime.tunnels,
            fixture::SECOND,
            fixture::connection(ConnectionStatus::Error),
        );
        accept(connected.clone(), visual);
        view.update(visual, |_, cx| cx.notify());
        frame(visual);
        assert!(
            Rc::ptr_eq(&current, &prepared(visual)),
            "unrelated snapshot changes must retain the projection"
        );
        fixture::set(
            &mut connected.runtime.tunnel_health,
            forward,
            configuration::DestinationHealthList {
                values: vec![configuration::DestinationHealth {
                    local_port: 9000,
                    remote_port: 8000,
                    status: configuration::DestinationStatus::Unavailable,
                    message: Some("unavailable".into()),
                    checked_at: None,
                }],
            },
        );
        accept(connected, visual);
        assert!(!Rc::ptr_eq(&current, &prepared(visual)));
        assert_eq!(
            view.read_with(visual, |view, _| view.connections_state[&server]
                .projects
                .borrow()
                .counts
                .unwrap()
                .attention),
            1
        );
    }

    #[gpui::test]
    fn partial_inventory_keeps_ports_and_coverage_details_in_both_layouts(cx: &mut TestAppContext) {
        use crate::connections::fixtures::{observation, put};
        use crate::page_perf_tests::{connections_fixture, frame, listener_selector};
        use newport_protocol::monitor::ports::discovery as p;

        for table in [false, true] {
            let (view, visual) = connections_fixture(cx, table);
            view.update(visual, |view, cx| {
                let mut reading = observation([Listener::new(8000).pid(1)]);
                reading.data.as_mut().unwrap().current.coverage.ipv4 = p::Report {
                    outcome: p::Outcome::Partial,
                    issues: vec![p::Issue {
                        kind: p::IssueKind::BudgetExceeded,
                        count: 1,
                        message: String::new(),
                    }],
                };
                put(view, reading);
                cx.notify();
            });
            frame(visual);
            assert!(visual.debug_bounds(listener_selector(8000, 1)).is_some());
            assert!(visual.debug_bounds("connections-updating").is_none());
            view.read_with(visual, |view, _| {
                assert!(
                    view.connections_state[&view.server]
                        .reading
                        .as_ref()
                        .unwrap()
                        .view()
                        .unwrap()
                        .inventory_incomplete()
                );
            });
            let details = visual.update(|window, _| {
                gpui_kit::base::test_support::snapshots(window)
                    .into_iter()
                    .find(|node| node.label() == Some("Discovery details"))
                    .unwrap()
                    .bounds()
            });
            visual.simulate_click(details.center(), Default::default());
            frame(visual);
            visual.update(|window, _| {
                assert!(
                    gpui_kit::base::test_support::snapshots(window)
                        .iter()
                        .filter_map(|node| node.label())
                        .any(|label| label.starts_with("IPv4 ports: Limited"))
                );
            });
            visual.simulate_keystrokes("escape");
            frame(visual);
        }
    }

    #[gpui::test]
    fn clicking_saved_filters_rendered_connections_in_both_layouts(cx: &mut TestAppContext) {
        use crate::page_perf_tests::{connections_fixture, frame};
        for table in [false, true] {
            let (view, visual) = connections_fixture(cx, table);
            let server = view.read_with(visual, |view, _| view.server.clone());
            view.update(visual, |view, cx| {
                let mut snapshot = view.snapshot.clone().unwrap();
                let mut stopped = crate::snapshot_fixture::tunnel(
                    uuid::Uuid::from_u128(0x5709),
                    uuid::Uuid::parse_str(&server).unwrap(),
                    9001,
                    9001,
                );
                stopped.name = "Stopped service".into();
                stopped.remote_host = "127.0.0.1".into();
                snapshot.config.tunnels = vec![app_forward(&server), stopped];
                view.sync_connections_snapshot(&snapshot);
                view.snapshot = Some(snapshot);
                cx.notify();
            });
            frame(visual);
            for (filter, expected) in [(1, 2), (0, 501), (1, 2)] {
                let button = visual.update(|window, _| {
                    let control = gpui_kit::base::test_support::find(
                        window,
                        &[],
                        &format!("port-filter:{server}-{filter}").into(),
                    )
                    .unwrap();
                    assert_eq!(
                        control.label(),
                        Some(if filter == 1 {
                            "Saved (2)"
                        } else {
                            "All (501)"
                        })
                    );
                    control.bounds()
                });
                visual.simulate_click(button.center(), Default::default());
                frame(visual);
                view.read_with(visual, |view, _| {
                    let state = &view.connections_state[&server];
                    assert_eq!(state.filter, filter);
                    assert_eq!(
                        state
                            .visible
                            .borrow()
                            .groups
                            .iter()
                            .map(|g| g.rows.len())
                            .sum::<usize>(),
                        expected
                    );
                });
                assert!(
                    visual
                        .debug_bounds(crate::page_perf_tests::listener_selector(8000, 1))
                        .is_some()
                );
                assert_eq!(
                    visual
                        .debug_bounds(crate::page_perf_tests::listener_selector(8001, 1))
                        .is_some(),
                    filter == 0
                );
            }
            view.update(visual, |view, cx| {
                let mut snapshot = view.snapshot.clone().unwrap();
                snapshot.config.tunnels.clear();
                view.sync_connections_snapshot(&snapshot);
                view.snapshot = Some(snapshot);
                cx.notify();
            });
            frame(visual);
            view.read_with(visual, |view, _| {
                let state = &view.connections_state[&server];
                assert!(state.visible.borrow().groups.is_empty());
                assert_eq!(state.projects.borrow().counts.unwrap().saved, 0);
            });
            assert!(
                visual
                    .debug_bounds(crate::page_perf_tests::listener_selector(8000, 1))
                    .is_none()
            );
        }
    }

    #[gpui::test]
    fn rendered_connection_projections_reuse_rows_keys_and_focus_until_inputs_change(
        cx: &mut TestAppContext,
    ) {
        use crate::page_perf_tests::{connections_fixture, frame};
        use std::rc::Rc;
        for table in [false, true] {
            let (view, visual) = connections_fixture(cx, table);
            let server = view.read_with(visual, |view, _| view.server.clone());
            let retained = |visual: &mut gpui::VisualTestContext| {
                view.read_with(visual, |view, _| {
                    let state = &view.connections_state[&server];
                    (
                        state.prepared.borrow().value.clone().unwrap(),
                        state.viewport.borrow().focus.clone(),
                    )
                })
            };
            let (initial, focus) = retained(visual);
            for scroll in [false, true] {
                view.update(visual, |view, cx| {
                    if scroll {
                        view.connections_state[&server]
                            .viewport
                            .borrow()
                            .list
                            .scroll_to_end();
                    }
                    cx.notify();
                });
                frame(visual);
                let (next, next_focus) = retained(visual);
                assert!(
                    Rc::ptr_eq(&initial, &next),
                    "unchanged redraw must not flatten or sort all rows"
                );
                assert!(
                    Rc::ptr_eq(&focus, &next_focus),
                    "unchanged redraw must not clone every focus handle"
                );
            }
            visual.simulate_resize(size(px(650.), px(500.)));
            frame(visual);
            let (resized, resized_focus) = retained(visual);
            assert!(Rc::ptr_eq(&initial, &resized));
            assert!(Rc::ptr_eq(&focus, &resized_focus));
            let input = view.read_with(visual, |view, _| {
                view.connections_state[&server].input.clone().unwrap()
            });
            visual.update(|window, cx| {
                input.update(cx, |input, cx| input.insert("8499", window, cx))
            });
            frame(visual);
            let (filtered, _) = retained(visual);
            assert!(!Rc::ptr_eq(&initial, &filtered));
            assert_eq!(filtered.keys.len(), 1);
            assert!(
                visual
                    .debug_bounds(crate::page_perf_tests::listener_selector(8499, 1))
                    .is_some()
            );
            assert!(
                visual
                    .debug_bounds(crate::page_perf_tests::listener_selector(8000, 1))
                    .is_none()
            );
            view.update(visual, |view, cx| {
                view.connections_state.get_mut(&server).unwrap().table = !table;
                cx.notify();
            });
            frame(visual);
            let (switched, _) = retained(visual);
            assert!(!Rc::ptr_eq(&filtered, &switched));
            assert_eq!(switched.keys.len(), 1);
            assert_eq!(
                matches!(&switched.content, super::PreparedContent::Table(_)),
                !table
            );
        }
    }

    #[gpui::test]
    fn table_projection_reorders_after_project_metadata_and_sort_changes(cx: &mut TestAppContext) {
        use crate::page_perf_tests::{connections_fixture, frame};
        let (view, visual) = connections_fixture(cx, true);
        let server = view.read_with(visual, |view, _| view.server.clone());
        let bookmarks = |first: &str| {
            use crate::project_library_state::typed_fixture::bookmark;
            vec![
                bookmark(1, &server, first, "/srv/a"),
                bookmark(2, &server, "Middle", "/srv/b"),
            ]
        };
        view.update(visual, |view, cx| {
            crate::connections::fixtures::put(
                view,
                crate::connections::fixtures::observation([
                    Listener::new(8000)
                        .pid(1)
                        .process("node", "node", &[], None)
                        .project("/srv/a"),
                    Listener::new(8001)
                        .pid(2)
                        .process("node", "node", &[], None)
                        .project("/srv/b"),
                ]),
            );
            let generation = view
                .resources
                .begin_refresh("connection-projects", false)
                .unwrap();
            assert!(
                view.complete_connection_projects(
                    generation,
                    Ok(projects_reply(bookmarks("Zulu")))
                )
            );
            view.connections_state.get_mut(&server).unwrap().sort = 2;
            cx.notify();
        });
        frame(visual);
        let names = |visual: &mut gpui::VisualTestContext| {
            view.read_with(visual, |view, _| {
                let cache = view.connections_state[&server].prepared.borrow();
                let super::PreparedContent::Table(rows) = &cache.value.as_ref().unwrap().content
                else {
                    panic!("expected table")
                };
                rows.iter()
                    .map(|row| row.project_name.clone())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(names(visual), ["Middle", "Zulu"]);
        view.update(visual, |view, cx| {
            let generation = view
                .resources
                .begin_refresh("connection-projects", false)
                .unwrap();
            assert!(
                view.complete_connection_projects(
                    generation,
                    Ok(projects_reply(bookmarks("Alpha")))
                )
            );
            cx.notify();
        });
        frame(visual);
        assert_eq!(names(visual), ["Alpha", "Middle"]);
        view.update(visual, |view, cx| {
            view.connections_state.get_mut(&server).unwrap().descending = true;
            cx.notify();
        });
        frame(visual);
        assert_eq!(names(visual), ["Middle", "Alpha"]);
    }

    #[test]
    #[ignore = "manual Connections grouping benchmark; excludes layout and paint"]
    fn benchmark_connection_grouping() {
        use std::{hint::black_box, time::Instant};
        let ports: Vec<_> = (0..500)
            .map(|i| {
                Listener::new(8000 + i)
                    .pid(u32::from(i) + 1)
                    .project(&format!("/srv/project-{}", i / 10))
                    .member()
            })
            .collect();
        let saved: Vec<_> = (0..100)
            .map(|i| configuration::Tunnel {
                remote_host: "127.0.0.1".into(),
                ..crate::snapshot_fixture::tunnel(
                    uuid::Uuid::from_u128(i),
                    crate::snapshot_fixture::SERVER,
                    9000 + i as u16,
                    8000 + i as u16,
                )
            })
            .collect();
        let prepare = || {
            newport_domain::port_grouping::group(
                ports.clone(),
                &saved,
                crate::snapshot_fixture::SERVER,
                &[],
            )
        };
        let start = Instant::now();
        for _ in 0..30 {
            black_box(prepare());
        }
        eprintln!(
            "500 listeners / 100 forwards: grouping {:.3} ms/redraw",
            start.elapsed().as_secs_f64() * 1000. / 30.
        );
        let mut cache = super::ProjectsCache::default();
        black_box(cache.get([1, 2, 3], prepare));
        let start = Instant::now();
        for _ in 0..30 {
            // Include the owned projects still passed into the row builder.
            black_box(cache.get([1, 2, 3], prepare).as_ref().clone());
        }
        eprintln!(
            "500 listeners / 100 forwards: cached grouping plus owned rows {:.3} ms/redraw",
            start.elapsed().as_secs_f64() * 1000. / 30.
        );
    }

    #[test]
    fn project_cache_tracks_discovery_saved_projects_and_configuration_revisions() {
        let mut cache = super::ProjectsCache::default();
        let ports = vec![Listener::new(8000).pid(1).member()];
        let first = cache.get([1, 2, 3], || {
            newport_domain::port_grouping::group(
                ports.clone(),
                &[],
                crate::snapshot_fixture::SERVER,
                &[],
            )
        });
        assert_eq!(first[0].entries.len(), 1);
        let again = cache.get([1, 2, 3], || panic!("unchanged redraw must reuse grouping"));
        assert!(std::rc::Rc::ptr_eq(&first, &again));
        for key in [[4, 2, 3], [4, 5, 3], [4, 5, 6], [0, 0, 6]] {
            let previous = cache.projects.clone();
            let current = cache.get(key, Vec::new);
            assert!(!std::rc::Rc::ptr_eq(&previous, &current));
            assert!(current.is_empty());
        }
    }

    struct LayoutHarness {
        sidebar: f32,
    }

    impl Render for LayoutHarness {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .child(div().w(px(self.sidebar)).flex_shrink_0())
                .child(super::connections_viewport(
                    f32::from(window.viewport_size().width),
                    None,
                    div()
                        .debug_selector(|| "connections-content".into())
                        .h(px(80.))
                        .into_any_element(),
                ))
        }
    }

    #[gpui::test]
    fn detail_padding_is_uniform_and_wide_pages_center_without_stretching(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| LayoutHarness { sidebar: 204. });
        // Padding stays constant across window sizes and sidebar states; the
        // toolbar starts at the top edge like the other list pages.
        // The maximum width includes padding.
        for (viewport, sidebar, horizontal, top, bottom) in [
            (699., 204., 16., 0., 16.),
            (700., 204., 16., 0., 16.),
            (800., 204., 16., 0., 16.),
            (699., 0., 16., 0., 16.),
            (1700., 204., 16., 0., 16.),
            (1900., 204., 16., 0., 16.),
            (1900., 0., 16., 0., 16.),
        ] {
            view.update(cx, |view, cx| {
                view.sidebar = sidebar;
                cx.notify();
            });
            cx.simulate_resize(size(px(viewport), px(600.)));
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let detail = cx.debug_bounds("connections-detail").unwrap();
            let content = cx.debug_bounds("connections-content").unwrap();
            let pane = viewport - sidebar;
            let width = pane.min(1500.);
            let left = sidebar + (pane - width) / 2.;
            assert_eq!(
                detail.origin,
                gpui::point(px(left), px(0.)),
                "viewport {viewport}, sidebar {sidebar}"
            );
            assert_eq!(detail.size, size(px(width), px(600.)));
            assert_eq!(content.size.height, px(80.));
            assert!(content.bottom() <= detail.bottom() - px(bottom));
            assert_eq!(content.origin, gpui::point(px(left + horizontal), px(top)));
            assert_eq!(content.size.width, px(super::inner_width(viewport, pane)));
        }
    }

    #[test]
    fn row_density_uses_the_padded_container_instead_of_the_window() {
        // Density uses the same combined gutter at every window size.
        assert!(super::inner_width(600., 600.) > 560.);
        assert!(super::inner_width(680., 680.) > 640.);
        // Opening a sidebar changes density but does not select compact outer
        // padding at otherwise roomy window sizes.
        assert_eq!(super::inner_width(800., 596.), 564.);
        assert_eq!(super::inner_width(2000., 1800.), 1468.);
    }
    #[test]
    fn attention_filter_checks_only_the_discovered_port_but_all_of_a_saved_range() {
        let runtime = forward_runtime(
            configuration::ConnectionStatus::Connected,
            None,
            vec![
                health(
                    8079,
                    configuration::DestinationStatus::Unavailable,
                    Some("Connection refused"),
                ),
                health(8080, configuration::DestinationStatus::Reachable, None),
            ],
        );
        let runtime = Some(&runtime);
        let tunnel = configuration::Tunnel {
            id: uuid::Uuid::parse_str(T).unwrap(),
            remote_port: 8079,
            remote_port_end: Some(8080),
            ..forward(0)
        };
        let mut row = Entry {
            addresses: vec![],
            port: Listener::new(8080).port(),
            forwards: vec![tunnel.clone()],
        };
        assert!(!entry_needs_attention(runtime, &row));
        assert!(trouble_for(runtime, &tunnel, None));
        row.port = Listener::new(8079).port();
        assert!(entry_needs_attention(runtime, &row));
        row.port = Listener::new(8081).port();
        assert!(!entry_needs_attention(runtime, &row));
        row.port = Port::saved(tunnel);
        assert!(entry_needs_attention(runtime, &row));
    }
    #[test]
    fn attention_retains_transport_errors_and_ignores_disconnected_health() {
        use configuration::{ConnectionStatus, DestinationStatus};
        let tunnel = configuration::Tunnel {
            id: uuid::Uuid::parse_str(T).unwrap(),
            ..forward(0)
        };
        let checks = || {
            vec![health(
                8079,
                DestinationStatus::Unavailable,
                Some("Connection refused"),
            )]
        };
        let lost = forward_runtime(
            ConnectionStatus::Reconnecting,
            Some("SSH connection lost"),
            checks(),
        );
        assert!(trouble_for(Some(&lost), &tunnel, Some(8080)));
        let failed = forward_runtime(ConnectionStatus::Error, None, checks());
        assert!(trouble_for(Some(&failed), &tunnel, Some(8080)));
        let stopped = forward_runtime(ConnectionStatus::Disconnected, None, checks());
        assert!(!trouble_for(Some(&stopped), &tunnel, Some(8079)));
    }

    #[test]
    fn every_saved_forward_without_an_observed_destination_is_offered() {
        let ports = vec![Listener::new(3000).pid(1).member()];
        let forward = |id: u128, remote: u16, direction, host: &str| configuration::Tunnel {
            name: String::new(),
            direction,
            remote_host: host.into(),
            ..crate::snapshot_fixture::tunnel(
                uuid::Uuid::from_u128(id),
                crate::snapshot_fixture::SERVER,
                remote + 6000,
                remote,
            )
        };
        use configuration::Direction::{Local, Remote};
        let saved = vec![
            forward(1, 3000, Local, "127.0.0.1"),
            forward(2, 4000, Local, "127.0.0.1"),
            forward(3, 4001, Local, "localhost"),
            forward(4, 4003, Remote, "127.0.0.1"),
            forward(5, 4004, Local, "db.internal"),
        ];
        let groups = newport_domain::port_grouping::group(
            ports,
            &saved,
            crate::snapshot_fixture::SERVER,
            &[],
        );
        let ids: Vec<_> = super::unobserved_forwards(&groups)
            .iter()
            .map(|t| t.id.as_u128())
            .collect();
        assert_eq!(ids, [2, 3, 5], "off, on and the other host");
    }

    #[test]
    fn reverse_forwards_follow_local_rows_under_their_own_section() {
        let row = |port: Port| {
            Row::new(
                "/repo",
                Entry {
                    forwards: port.definition().cloned().into_iter().collect(),
                    addresses: vec![],
                    port,
                },
                None,
                false,
            )
        };
        let mut rows = vec![
            row(Port::saved(reverse(1, 8000))),
            row(Listener::new(8080)
                .project("/repo")
                .checkout("/repo/feature", Some("feature"), Some(false))
                .port()),
            row(Port::saved(configuration::Tunnel {
                remote_port: 9000,
                ..forward(2)
            })),
            row(Port::saved(reverse(3, 8001))),
            row(Listener::new(8081)
                .project("/repo")
                .checkout("/repo", Some("main"), Some(true))
                .port()),
            row(Listener::new(8082).port()),
        ];
        sort_project_rows(&mut rows);
        assert_eq!(
            rows.iter()
                .map(|row| row_section(row, true))
                .collect::<Vec<_>>(),
            vec![
                RowSection::Project,
                RowSection::Worktree("/repo".into()),
                RowSection::Worktree("/repo/feature".into()),
                RowSection::Saved,
                RowSection::Reverse,
                RowSection::Reverse,
            ]
        );
        // Suppressing a single-checkout heading must not suppress the reverse
        // heading or fold its rows into the ordinary saved-forward list.
        assert_eq!(row_section(&rows[1], false), RowSection::Project);
        assert_eq!(row_section(&rows[4], false), RowSection::Reverse);
    }
}
