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

    fn counts(&mut self, saved: &[Value], runtime: &Value, server: &str) -> Counts {
        *self.counts.get_or_insert_with(|| {
            let mine = saved.iter().filter(|t| t["serverId"] == server);
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
                    .filter(|e| !e.saved)
                    .count(),
                projects: self.projects.iter().filter(|p| !p.key.is_empty()).count(),
                active: mine
                    .clone()
                    .filter(|t| {
                        matches!(
                            runtime["tunnels"][string(t, "id")]["status"].as_str(),
                            Some("connected" | "connecting" | "reconnecting")
                        )
                    })
                    .count(),
                attention: mine.filter(|t| trouble(runtime, t)).count(),
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
    key: Value,
    value: Option<Rc<PreparedRows>>,
}
impl PreparedCache {
    fn get(&mut self, key: Value, build: impl FnOnce() -> PreparedRows) -> Rc<PreparedRows> {
        if self.key != key || self.value.is_none() {
            self.value = Some(Rc::new(build()));
            self.key = key;
        }
        self.value.as_ref().unwrap().clone()
    }
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
    key: Value,
    groups: std::rc::Rc<Vec<std::rc::Rc<ProjectRows>>>,
}
impl VisibleCache {
    fn get(
        &mut self,
        key: Value,
        build: impl FnOnce() -> Vec<std::rc::Rc<ProjectRows>>,
    ) -> std::rc::Rc<Vec<std::rc::Rc<ProjectRows>>> {
        if self.key != key {
            self.groups = std::rc::Rc::new(build());
            self.key = key;
        }
        self.groups.clone()
    }
}
impl ProjectRows {
    fn new(project: Project, full: Vec<Entry>, mut rows: Vec<connections_rows::Row>) -> Self {
        sort_project_rows(&mut rows);
        let show_branches =
            project.worktrees.len() > 1 || project.worktrees.iter().any(|w| !w.main);
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
/// sidebar is open or the page reaches the reference's 1500px maximum.
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

pub(super) fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
pub(super) fn trouble(runtime: &Value, t: &Value) -> bool {
    trouble_for(runtime, t, None)
}
pub(super) fn trouble_for(runtime: &Value, t: &Value, remote_port: Option<u64>) -> bool {
    let id = string(t, "id");
    runtime["tunnels"][id]["status"] == "error"
        || runtime["tunnels"][id]["errorMessage"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
        || (runtime["tunnels"][id]["status"] == "connected"
            && runtime["tunnelHealth"][id].as_array().is_some_and(|hs| {
                hs.iter()
                    .filter(|h| {
                        remote_port.is_none_or(|port| h["remotePort"].as_u64() == Some(port))
                    })
                    .any(|h| h["message"].as_str().is_some_and(|s| !s.is_empty()))
            }))
}
fn entry_needs_attention(runtime: &Value, entry: &Entry) -> bool {
    let remote_port = if entry.saved {
        None
    } else {
        entry.port["port"].as_u64()
    };
    entry
        .forwards
        .iter()
        .any(|t| trouble_for(runtime, t, remote_port))
}

/// Saved forwards whose rows read "Nothing listening": not reverse forwards,
/// switched off, not failing, and with no listener in the discovery reading.
/// Only destinations on the server itself count, since discovery can't see
/// the listeners of other hosts reached through it.
pub(super) fn dead_forwards(groups: &[Project], runtime: &Value) -> Vec<Value> {
    groups
        .iter()
        .flat_map(|group| &group.entries)
        .filter(|entry| {
            entry.saved
                && entry.port["direction"] != "remote"
                && !entry_needs_attention(runtime, entry)
        })
        .flat_map(|entry| &entry.forwards)
        .filter(|t| {
            newport_domain::forwarding::on_server(string(t, "remoteHost"))
                && !matches!(
                    string(&runtime["tunnels"][string(t, "id")], "status"),
                    "connected" | "connecting" | "reconnecting"
                )
        })
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
    if row.entry.saved && row.entry.port["direction"] == "remote" {
        RowSection::Reverse
    } else if show_branches && let Some(worktree) = &row.worktree {
        RowSection::Worktree(worktree.path.clone())
    } else if row.entry.saved && row.worktree.is_none() {
        RowSection::Saved
    } else {
        RowSection::Project
    }
}

fn sort_project_rows(rows: &mut [connections_rows::Row]) {
    rows.sort_by_key(|row| {
        let worktree = &row.worktree;
        (
            row.entry.saved && worktree.is_none(),
            row.entry.saved && row.entry.port["direction"] == "remote",
            worktree.is_some(),
            worktree.as_ref().is_some_and(|w| !w.main),
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
    let detached = branch.is_none_or(|branch| {
        (7..=40).contains(&branch.len()) && branch.bytes().all(|b| b.is_ascii_hexdigit())
    });
    let title = if detached {
        worktree
            .path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(&worktree.path)
            .to_owned()
    } else {
        branch.unwrap().to_owned()
    };
    (
        title,
        if detached {
            "Detached"
        } else if worktree.main {
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
        let known = self
            .resources
            .get("connection-projects")
            .and_then(|resource| resource.value.as_array())
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter(|p| p["serverId"] == self.server)
            .cloned()
            .collect::<Vec<_>>();
        let ports = self
            .resources
            .get("page")
            .map(|resource| rows(&resource.value))
            .unwrap_or_default();
        let saved = self.snapshot["config"]["tunnels"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if let Some(state) = self.connections_state.get(&self.server) {
            state
                .worktree_associations
                .borrow_mut()
                .group(ports, saved, &self.server, &known)
        } else {
            newport_domain::port_grouping::group(ports, saved, &self.server, &known)
        }
    }

    /// Only a current, successful reading of a connected server can show that
    /// nothing listens behind a forward.
    pub(super) fn listeners_known(&self, cx: &App) -> bool {
        self.resources.get("page").is_some_and(|r| {
            r.value.is_array() && r.error.is_none() && !r.stale && r.updated_at.is_some()
        }) && !self.needle_missing()
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
        let saved = self.snapshot["config"]["tunnels"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let runtime = &self.snapshot["runtime"];
        let show_owner = cx.weak_entity();
        let view_owner = show_owner.clone();
        let show_server = self.server.clone();
        let view_server = self.server.clone();
        let [ports_revision, projects_revision] = ["page", "connection-projects"].map(|key| {
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
            let counts = cache.counts(saved, runtime, &self.server);
            (groups, counts)
        };
        let no_entries = groups.is_empty();
        let resource = self.resources.get("page");
        let has_reading = resource.is_some_and(|r| r.value.is_array());
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
            .get(json!([revisions, query, filter]), || {
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
            .get("page")
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
            let version = json!([
                revisions,
                query,
                filter,
                state.table,
                state.sort,
                state.descending,
                state.collapsed,
                state.expanded,
                state.collapsed_worktrees
            ]);
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
                            || project.worktrees.iter().any(|w| !w.main))
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
            // The fixed toolbar's centring; its height includes the 1px rule.
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
                    // Like the Needs attention filter, shown only when it applies.
                    .when(cleanable > 0, |actions| {
                        actions.child(
                            crate::controls::icon_button("clean-up-forwards")
                                .ghost()
                                .icon(IconName::BroomSparkles)
                                .accessibility_label("Clean up forwards")
                                .tooltip("Clean up forwards with nothing listening")
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
        let single_branch = if project.worktrees.len() == 1 && project.worktrees[0].main {
            project.worktrees[0].branch.clone()
        } else {
            None
        };
        // Both project and checkout actions use the full scope, independent of search.
        let show_branches =
            project.worktrees.len() > 1 || project.worktrees.iter().any(|w| !w.main);
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
        Entry, RowSection, entry_needs_attention, row_section, sort_project_rows, trouble,
        trouble_for,
    };
    use crate::connections_rows::Row;
    use gpui_kit::{
        self as gpui, Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px,
        size,
    };
    use serde_json::json;

    #[test]
    fn saved_checkout_rows_stay_with_their_live_siblings() {
        let make = |port, saved| {
            Row::new(
                "/repo",
                Entry {
                    port,
                    saved,
                    addresses: vec![],
                    forwards: vec![],
                },
                &json!({}),
                true,
            )
        };
        let mut rows = vec![
            make(
                json!({"remotePort":8001,"worktreePath":"/repo/a","worktreeBranch":"a","worktreeMain":false}),
                true,
            ),
            make(
                json!({"port":8002,"worktreePath":"/repo/b","worktreeBranch":"b","worktreeMain":false}),
                false,
            ),
            make(
                json!({"port":8000,"worktreePath":"/repo/a","worktreeBranch":"a","worktreeMain":false}),
                false,
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
            main: false,
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
        let (work, requests) = std::sync::mpsc::sync_channel(64);
        let (replies, incoming) = std::sync::mpsc::channel();
        view.update(visual, |view, cx| {
            view.work = work.into();
            view.replies = incoming;
            let mut snapshot = view.snapshot.clone();
            snapshot["config"]["tunnels"] = json!([{
                "id":"forward", "serverId":"fixture", "name":"App", "localPort":9000,
                "remotePort":8000, "remoteHost":"127.0.0.1", "direction":"local", "project":"/srv/project-0"
            }]);
            view.sync_connections_snapshot(&snapshot);
            view.snapshot = snapshot;
            cx.notify();
        });
        frame(visual);
        let prepared = |visual: &mut gpui::VisualTestContext| {
            view.read_with(visual, |view, _| {
                view.connections_state["fixture"]
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
                .debug_bounds("connection-row-port-8000-1-127.0.0.1-")
                .is_none()
        );
        let accept = |next: serde_json::Value, visual: &mut gpui::VisualTestContext| {
            view.update(visual, |view, cx| {
                view.last_refresh = std::time::Instant::now();
                let generation = requests
                    .try_iter()
                    .find(|request| request.key == "snapshot")
                    .map(|request| request.generation)
                    .unwrap_or_else(|| view.resources.begin("snapshot").unwrap());
                replies
                    .send(crate::Completion {
                        key: "snapshot".into(),
                        server: "fixture".into(),
                        generation,
                        result: Ok(next),
                        pdf: None,
                        pdf_text: None,
                        image: None,
                    })
                    .unwrap();
                view.tick(cx);
            });
            frame(visual);
        };
        let mut connected = view.read_with(visual, |view, _| view.snapshot.clone());
        connected["runtime"]["tunnels"] = json!({"forward":{"status":"connected"}});
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
            view.read_with(visual, |view, _| view.connections_state["fixture"]
                .projects
                .borrow()
                .counts
                .unwrap()
                .active),
            1
        );
        assert!(visual.debug_bounds("connections-footer").is_some());
        connected["runtime"]["clipboard"]["fixture"]["status"] = json!("connecting");
        connected["runtime"]["tunnels"]["another-server"] = json!({"status":"error"});
        accept(connected.clone(), visual);
        view.update(visual, |_, cx| cx.notify());
        frame(visual);
        assert!(
            Rc::ptr_eq(&current, &prepared(visual)),
            "unrelated snapshot changes must retain the projection"
        );
        connected["runtime"]["tunnelHealth"] = json!({"forward":[{"message":"unavailable"}]});
        accept(connected, visual);
        assert!(!Rc::ptr_eq(&current, &prepared(visual)));
        assert_eq!(
            view.read_with(visual, |view, _| view.connections_state["fixture"]
                .projects
                .borrow()
                .counts
                .unwrap()
                .attention),
            1
        );
    }

    #[gpui::test]
    fn clicking_saved_filters_rendered_connections_in_both_layouts(cx: &mut TestAppContext) {
        use crate::page_perf_tests::{connections_fixture, frame};
        for table in [false, true] {
            let (view, visual) = connections_fixture(cx, table);
            view.update(visual, |view, cx| {
                let mut snapshot = view.snapshot.clone();
                snapshot["config"]["tunnels"] = json!([{
                    "id":"forward", "serverId":"fixture", "name":"App", "localPort":9000,
                    "remotePort":8000, "remoteHost":"127.0.0.1", "direction":"local", "project":"/srv/project-0", "enabled":false
                }, {
                    "id":"stopped", "serverId":"fixture", "name":"Stopped service", "localPort":9001,
                    "remotePort":9001, "remoteHost":"127.0.0.1", "direction":"local", "enabled":false
                }]);
                view.sync_connections_snapshot(&snapshot);
                view.snapshot = snapshot;
                cx.notify();
            });
            frame(visual);
            for (filter, expected) in [(1, 2), (0, 501), (1, 2)] {
                let button = visual.update(|window, _| {
                    let control = gpui_kit::base::test_support::find(
                        window,
                        &[],
                        &format!("port-filter:fixture-{filter}").into(),
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
                    let state = &view.connections_state["fixture"];
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
                        .debug_bounds("connection-row-port-8000-1-127.0.0.1-")
                        .is_some()
                );
                assert_eq!(
                    visual
                        .debug_bounds("connection-row-port-8001-2-127.0.0.1-")
                        .is_some(),
                    filter == 0
                );
            }
            view.update(visual, |view, cx| {
                let mut snapshot = view.snapshot.clone();
                snapshot["config"]["tunnels"] = json!([]);
                view.sync_connections_snapshot(&snapshot);
                view.snapshot = snapshot;
                cx.notify();
            });
            frame(visual);
            view.read_with(visual, |view, _| {
                let state = &view.connections_state["fixture"];
                assert!(state.visible.borrow().groups.is_empty());
                assert_eq!(state.projects.borrow().counts.unwrap().saved, 0);
            });
            assert!(
                visual
                    .debug_bounds("connection-row-port-8000-1-127.0.0.1-")
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
            let retained = |visual: &mut gpui::VisualTestContext| {
                view.read_with(visual, |view, _| {
                    let state = &view.connections_state["fixture"];
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
                        view.connections_state["fixture"]
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
                view.connections_state["fixture"].input.clone().unwrap()
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
                    .debug_bounds("connection-row-port-8499-500-127.0.0.1-")
                    .is_some()
            );
            assert!(
                visual
                    .debug_bounds("connection-row-port-8000-1-127.0.0.1-")
                    .is_none()
            );
            view.update(visual, |view, cx| {
                view.connections_state.get_mut("fixture").unwrap().table = !table;
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
        let bookmarks = |first: &str| {
            json!([
                {"serverId":"fixture", "name":first, "path":"/srv/a"},
                {"serverId":"fixture", "name":"Middle", "path":"/srv/b"}
            ])
        };
        view.update(visual, |view, cx| {
            for (key, data) in [
                ("page", json!([
                    {"port":8000,"pid":1,"address":"127.0.0.1","processName":"node","projectRoot":"/srv/a"},
                    {"port":8001,"pid":2,"address":"127.0.0.1","processName":"node","projectRoot":"/srv/b"}
                ])),
                ("connection-projects", bookmarks("Zulu")),
            ] {
                let generation = view.resources.begin_refresh(key, false).unwrap();
                view.resources.complete(key, generation, Ok(data));
            }
            view.connections_state.get_mut("fixture").unwrap().sort = 2;
            cx.notify();
        });
        frame(visual);
        let names = |visual: &mut gpui::VisualTestContext| {
            view.read_with(visual, |view, _| {
                let cache = view.connections_state["fixture"].prepared.borrow();
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
            view.resources
                .complete("connection-projects", generation, Ok(bookmarks("Alpha")));
            cx.notify();
        });
        frame(visual);
        assert_eq!(names(visual), ["Alpha", "Middle"]);
        view.update(visual, |view, cx| {
            view.connections_state
                .get_mut("fixture")
                .unwrap()
                .descending = true;
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
                json!({
                    "port": 8000 + i, "pid": i + 1, "address": "127.0.0.1",
                    "process": "node", "projectRoot": format!("/srv/project-{}", i / 10)
                })
            })
            .collect();
        let saved: Vec<_> = (0..100)
            .map(|i| {
                json!({
                    "id": format!("tunnel-{i}"), "serverId": "fixture", "direction": "local",
                    "remotePort": 8000 + i, "localPort": 9000 + i, "remoteHost": "127.0.0.1"
                })
            })
            .collect();
        let prepare =
            || newport_domain::port_grouping::group(ports.clone(), &saved, "fixture", &[]);
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
        let ports = vec![json!({"port":8000,"pid":1,"address":"127.0.0.1"})];
        let first = cache.get([1, 2, 3], || {
            newport_domain::port_grouping::group(ports.clone(), &[], "fixture", &[])
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
        // The 1500px max width includes padding.
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
        // Density uses the same 32px combined gutter at every window size.
        assert!(super::inner_width(600., 600.) > 560.);
        assert!(super::inner_width(680., 680.) > 640.);
        // Opening a sidebar changes density but does not select compact outer
        // padding at otherwise roomy window sizes.
        assert_eq!(super::inner_width(800., 596.), 564.);
        assert_eq!(super::inner_width(2000., 1800.), 1468.);
    }
    #[test]
    fn attention_filter_checks_only_the_discovered_port_but_all_of_a_saved_range() {
        let runtime = json!({
            "tunnels":{"t":{"status":"connected"}},
            "tunnelHealth":{"t":[
                {"remotePort":8079,"status":"unavailable","message":"Connection refused"},
                {"remotePort":8080,"status":"reachable"}
            ]}
        });
        let tunnel = json!({"id":"t","remotePort":8079,"remotePortEnd":8080});
        let mut row = Entry {
            addresses: vec![],
            port: json!({"port":8080}),
            forwards: vec![tunnel.clone()],
            saved: false,
        };
        assert!(!entry_needs_attention(&runtime, &row));
        assert!(trouble(&runtime, &tunnel));
        row.port["port"] = json!(8079);
        assert!(entry_needs_attention(&runtime, &row));
        row.port["port"] = json!(8081);
        assert!(!entry_needs_attention(&runtime, &row));
        row.saved = true;
        row.port = tunnel;
        assert!(entry_needs_attention(&runtime, &row));
    }
    #[test]
    fn attention_retains_transport_errors_and_ignores_disconnected_health() {
        let tunnel = json!({"id":"t"});
        let mut runtime = json!({
            "tunnels":{"t":{"status":"reconnecting","errorMessage":"SSH connection lost"}},
            "tunnelHealth":{"t":[{"remotePort":8079,"status":"unavailable","message":"Connection refused"}]}
        });
        assert!(trouble_for(&runtime, &tunnel, Some(8080)));
        runtime["tunnels"]["t"] = json!({"status":"error"});
        assert!(trouble_for(&runtime, &tunnel, Some(8080)));
        runtime["tunnels"]["t"] = json!({"status":"disconnected"});
        assert!(!trouble_for(&runtime, &tunnel, Some(8079)));
    }

    #[test]
    fn only_switched_off_healthy_local_forwards_without_a_listener_are_dead() {
        let ports =
            vec![json!({"port": 3000, "pid": 1, "address": "127.0.0.1", "process": "node"})];
        let forward = |id: &str, remote: u16, direction: &str| {
            json!({
                "id": id, "serverId": "fixture", "direction": direction, "name": "",
                "remotePort": remote, "localPort": remote + 6000, "remoteHost": "127.0.0.1"
            })
        };
        let saved = vec![
            forward("listening", 3000, "local"),
            forward("dead", 4000, "local"),
            forward("waiting", 4001, "local"),
            forward("failed", 4002, "local"),
            forward("reverse", 4003, "remote"),
        ];
        let groups = newport_domain::port_grouping::group(ports, &saved, "fixture", &[]);
        let runtime = json!({"tunnels": {
            "waiting": {"status": "connecting"},
            "failed": {"status": "error", "errorMessage": "Cannot bind local port"},
        }});
        let ids: Vec<_> = super::dead_forwards(&groups, &runtime)
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, ["dead"]);
    }

    #[test]
    fn reverse_forwards_follow_local_rows_under_their_own_section() {
        let row = |port: serde_json::Value, saved: bool| {
            Row::new(
                "/repo",
                Entry {
                    forwards: if saved { vec![port.clone()] } else { vec![] },
                    addresses: vec![],
                    port,
                    saved,
                },
                &json!({}),
                false,
            )
        };
        let mut rows = vec![
            row(
                json!({"id":"reverse-a","direction":"remote","localPort":8000}),
                true,
            ),
            row(
                json!({"port":8080,"worktreePath":"/repo/feature","worktreeBranch":"feature","worktreeMain":false}),
                false,
            ),
            row(
                json!({"id":"local","direction":"local","remotePort":9000}),
                true,
            ),
            row(
                json!({"id":"reverse-b","direction":"remote","localPort":8001}),
                true,
            ),
            row(
                json!({"port":8081,"worktreePath":"/repo","worktreeBranch":"main","worktreeMain":true}),
                false,
            ),
            row(json!({"port":8082}), false),
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
