//! Shared row presentation and table ordering for Connections.
use super::connections_grouped::{Entry, active, forward_state, trouble_for};
use super::*;
use gpui_kit::base::{Table, TableBody, TableHead, TableHeader, TableRow};
use gpui_kit::component::animation::cubic_bezier;
use newport_protocol::desktop::configuration::ConnectionStatus;

/// A steady core communicates connection state; a single ring expands and fades.
/// Destination health remains separate from SSH transport state.
fn transport_dot(status: Option<ConnectionStatus>, cx: &App) -> AnyElement {
    let dark = cx.theme().is_dark();
    let tone = match status {
        Some(ConnectionStatus::Error) => Some(theme::error(dark)),
        Some(ConnectionStatus::Connecting | ConnectionStatus::Reconnecting) => {
            Some(theme::warning(dark))
        }
        Some(ConnectionStatus::Connected) => Some(theme::success(dark)),
        Some(ConnectionStatus::Disconnected) | None => None,
    };
    let dot = crate::controls::status_dot(tone, cx).relative();
    if status == Some(ConnectionStatus::Connected) {
        let color = theme::success(dark);
        let dot = crate::controls::status_dot(Some(color), cx).relative();
        if cx.reduce_motion() {
            return dot.into_any_element();
        }
        let ease = cubic_bezier(0.2, 0.6, 0.4, 1.);
        return dot
            .with_animation(
                "transport-ripple",
                Animation::new(Duration::from_millis(3600))
                    .repeat_synced()
                    .with_max_fps(24.),
                move |dot, phase| {
                    // Match the HTML reference: appear at 15–25%, fade by 80%,
                    // and rest before the next ring. Never blink the core.
                    let growth = ease(((phase - 0.15) / 0.65).clamp(0., 1.));
                    let diameter = 7.5 + 11.5 * growth;
                    let opacity = if phase < 0.25 {
                        0.5 * ease(((phase - 0.15) / 0.10).clamp(0., 1.))
                    } else {
                        0.5 * (1. - ease(((phase - 0.25) / 0.55).clamp(0., 1.)))
                    };
                    dot.child(
                        div()
                            .absolute()
                            .left(px((crate::ui::DOT_SIZE - diameter) / 2.))
                            .top(px((crate::ui::DOT_SIZE - diameter) / 2.))
                            .size(px(diameter))
                            .rounded_full()
                            .border(px(diameter / 10.))
                            .border_color(color.opacity(opacity)),
                    )
                },
            )
            .into_any_element();
    }
    if matches!(
        status,
        Some(ConnectionStatus::Connecting | ConnectionStatus::Reconnecting)
    ) && !cx.reduce_motion()
    {
        dot.with_animation(
            "transport-pulse",
            Animation::new(Duration::from_secs(2))
                .repeat()
                .with_easing(bounce(cubic_bezier(0.4, 0., 0.6, 1.))),
            |dot, progress| dot.opacity(1. - progress * 0.5),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    }
}

// Isolate animation invalidation from the Connections page and its tables.
struct TransportIndicator {
    props: (Option<ConnectionStatus>, bool, bool),
}

impl Render for TransportIndicator {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        transport_dot(self.props.0, cx)
    }
}

fn retained_transport_dot(
    key: &str,
    status: Option<ConnectionStatus>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let props = (status, cx.theme().is_dark(), cx.reduce_motion());
    let indicator = window.use_keyed_state(
        SharedString::from(format!("transport-indicator-{key}")),
        cx,
        |_, _| TransportIndicator { props },
    );
    indicator.update(cx, |indicator, cx| {
        if indicator.props != props {
            indicator.props = props;
            cx.notify();
        }
    });
    indicator
        .cached(
            div()
                .size(px(crate::ui::DOT_SIZE))
                .flex_shrink_0()
                .style()
                .clone(),
        )
        .into_any_element()
}

use crate::quiet_action::Menu as RowMenu;

struct RowFocus {
    handle: FocusHandle,
    _subscriptions: [Subscription; 2],
}

#[derive(IntoElement)]
struct SortColumn {
    owner: WeakEntity<Newport>,
    server: String,
    index: usize,
    title: &'static str,
    sorted: bool,
    descending: bool,
}

impl RenderOnce for SortColumn {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window.use_keyed_state(("ports-sort-focus", self.index), cx, |window, cx| {
            let handle = cx.focus_handle();
            let entered = cx.on_focus_in(&handle, window, |_, _, cx| cx.notify());
            let exited = cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify());
            RowFocus {
                handle,
                _subscriptions: [entered, exited],
            }
        });
        let handle = focus.read(cx).handle.clone();
        let focused = handle.contains_focused(window, cx);
        let group = SharedString::from(format!("ports-sort-{}", self.index));
        div().track_focus(&handle).tab_stop(false).child(
            crate::controls::compact_button(("ports-sort", self.index))
                .ghost()
                .group(group.clone())
                .px(px(4.))
                .ml(px(-4.))
                .rounded(px(crate::ui::RADIUS))
                .child(
                    div()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(cx.theme().muted_foreground)
                        .child(self.title),
                )
                .accessibility_label(format!(
                    "Sort by {}{}",
                    self.title,
                    if self.sorted {
                        if self.descending {
                            ", descending"
                        } else {
                            ", ascending"
                        }
                    } else {
                        ""
                    }
                ))
                .child(
                    div()
                        .when(!self.sorted, |icon| {
                            icon.opacity(if focused { 0.6 } else { 0. })
                                .group_hover(group, |style| style.opacity(0.6))
                        })
                        .child(
                            Icon::new(if !self.sorted {
                                IconName::ArrowUpDown
                            } else if self.descending {
                                IconName::ArrowDown
                            } else {
                                IconName::ArrowUp
                            })
                            .size(px(11.))
                            .text_color(cx.theme().muted_foreground),
                        ),
                )
                .on_click(move |_, _, cx| {
                    let _ = self.owner.update(cx, |this, cx| {
                        if this.server != self.server {
                            return;
                        }
                        let state = this
                            .connections_state
                            .entry(self.server.clone())
                            .or_default();
                        state.descending = state.sort == self.index && !state.descending;
                        state.sort = self.index;
                        cx.notify();
                    });
                }),
        )
    }
}

/// Row focus is a rendering concern: it lasts only as long as the row is shown,
/// and the container itself never adds a keyboard stop before its controls.
#[derive(IntoElement)]
struct RowPresentation {
    owner: WeakEntity<Newport>,
    row: Row,
    table: bool,
    stripe: bool,
    nested: bool,
    index: Option<usize>,
    position: Option<(usize, usize)>,
}

impl RenderOnce for RowPresentation {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window.use_keyed_state(
            SharedString::from(format!("connection-focus-{}", self.row.key)),
            cx,
            |window, cx| {
                let handle = cx.focus_handle();
                let entered = cx.on_focus_in(&handle, window, |_, _, cx| cx.notify());
                let exited = cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify());
                RowFocus {
                    handle,
                    _subscriptions: [entered, exited],
                }
            },
        );
        let handle = focus.read(cx).handle.clone();
        let engaged = handle.contains_focused(window, cx);
        self.owner
            .clone()
            .update(cx, |this, cx| {
                this.render_connection_row(self, &handle, engaged, window, cx)
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}

/// Width of the table's actions column: toggle, menu and the Always on icon.
const ACTIONS_COLUMN_WIDTH: f32 = 104.;
const TABLE_PORT_WIDTH: f32 = 84.;
const TABLE_STATUS_WIDTH: f32 = 124.;
const TABLE_ADDRESS_WIDTH: f32 = 184.;
const TABLE_PROJECT_SHARE: f32 = 0.20;
// Expand only when the fixed columns leave a readable service name.
const WIDE_TABLE_MIN_WIDTH: f32 =
    (TABLE_PORT_WIDTH + TABLE_STATUS_WIDTH + TABLE_ADDRESS_WIDTH + ACTIONS_COLUMN_WIDTH + 200.)
        / (1. - TABLE_PROJECT_SHARE);

/// A row is Always on only when every saved forward on it is.
fn always_on(forwards: &[configuration::Tunnel]) -> bool {
    !forwards.is_empty() && forwards.iter().all(|t| t.auto_connect)
}

pub(super) fn project_name(key: &str) -> &str {
    if key.is_empty() {
        "Other services"
    } else if key == newport_domain::port_grouping::SYSTEM {
        "System services"
    } else {
        key.strip_prefix("compose:")
            .unwrap_or_else(|| key.rsplit('/').next().unwrap_or(key))
    }
}
fn range(first: u16, last: Option<u16>) -> String {
    match last.filter(|last| *last != first) {
        Some(last) => format!("{first}–{last}"),
        None => first.to_string(),
    }
}
fn local_address(entry: &Entry) -> String {
    let Some(t) = entry.forwards.first() else {
        return String::new();
    };
    if t.direction == configuration::Direction::Remote {
        let host = t.remote_host.as_str();
        let host = match host {
            "localhost" | "127.0.0.1" | "::1" => "server".into(),
            _ if host.contains(':') && !host.starts_with('[') => format!("[{host}]"),
            _ => host.to_owned(),
        };
        return format!("{host}:{}", range(t.remote_port, t.remote_port_end));
    }
    let port = if entry.is_saved() {
        range(t.local_port, t.local_port_end)
    } else {
        let offset = entry
            .port
            .number()
            .map(u64::from)
            .unwrap_or(0)
            .saturating_sub(u64::from(t.remote_port));
        (u64::from(t.local_port) + offset).to_string()
    };
    format!("localhost:{port}")
}

/// Select a connected local mapping; another forward's error must not retarget it.
fn open_target(
    entry: &Entry,
    runtime: Option<&configuration::Runtime>,
) -> Option<(configuration::Tunnel, Option<u16>)> {
    let remote_port = if entry.is_saved() {
        None
    } else {
        Some(entry.port.number()?)
    };
    entry.forwards.iter().find_map(|tunnel| {
        (runtime
            .and_then(|runtime| runtime.tunnel(tunnel.id))
            .is_some_and(|state| state.status == ConnectionStatus::Connected)
            && newport_domain::forwarding::local_endpoint(
                &newport_domain::model::Tunnel::from(tunnel),
                remote_port,
            )
            .is_ok())
        .then(|| (tunnel.clone(), remote_port))
    })
}

/// The latest snapshot may already contain a forward created while the menu was open.
/// In that case, show its saved settings as an edit instead of renaming its ID.
pub(super) fn editor_draft(
    tunnels: &[newport_domain::model::Tunnel],
    server: uuid::Uuid,
    address: &str,
    port: u16,
    name: &str,
    project: &str,
) -> Result<(newport_domain::model::Tunnel, bool), String> {
    let mut tunnel = newport_domain::forwarding::discovered_tunnel(tunnels, server, address, port)?;
    let existing = tunnels.iter().any(|saved| saved.id == tunnel.id);
    if !existing {
        tunnel.name = name.to_owned();
        tunnel.project = (!project.is_empty() && project != newport_domain::port_grouping::SYSTEM)
            .then(|| project.to_owned());
    }
    Ok((tunnel, existing))
}

#[derive(Clone)]
pub(super) struct Row {
    pub(super) entry: Entry,
    pub(super) key: String,
    pub(super) project: String,
    pub(super) project_name: String,
    pub(super) branch: String,
    pub(super) worktree: Option<newport_domain::port_grouping::Worktree>,
    pub(super) number: u64,
    port: String,
    title: String,
    service: String,
    process_icon: crate::connections_process_icon::ProcessIcon,
    hover: std::rc::Rc<crate::connections_hover::Card>,
    context: String,
    generated_title: bool,
    detail: String,
    address: String,
    open: Option<(configuration::Tunnel, Option<u16>)>,
    status: String,
    transport: Option<ConnectionStatus>,
    rank: u8,
    active: bool,
    problem: bool,
    fix: Option<String>,
}
impl Row {
    pub(super) fn enrich(&mut self, view: &newport_client::discovery::View) {
        std::rc::Rc::make_mut(&mut self.hover).enrich(&self.entry, view);
    }

    pub(super) fn new(
        key: &str,
        entry: Entry,
        runtime: Option<&configuration::Runtime>,
        branches: bool,
    ) -> Self {
        let p = &entry.port;
        let worktree = (!p.reverse()).then(|| p.worktree()).flatten();
        let destination_port = (!entry.is_saved()).then(|| p.number()).flatten();
        let forward = entry.forwards.first();
        let state = entry
            .forwards
            .iter()
            .map(|t| forward_state(runtime, t).0)
            .min_by_key(|state| {
                // The dot describes SSH transport, independently of whether
                // the process at a destination is listening (PortRow.rowState).
                match state.map(|state| state.status) {
                    Some(ConnectionStatus::Error) => 0,
                    Some(ConnectionStatus::Reconnecting) => 1,
                    Some(ConnectionStatus::Connecting) => 2,
                    Some(ConnectionStatus::Connected) => 3,
                    Some(ConnectionStatus::Disconnected) | None => 4,
                }
            })
            .flatten();
        let connection = state.map(|state| state.status);
        let active = connection.is_some_and(active);
        let problem = entry
            .forwards
            .iter()
            .any(|t| trouble_for(runtime, t, destination_port));
        // With several forwards on a port, repair the failed one rather than
        // accidentally opening the first (possibly healthy) mapping.
        let fix = entry.forwards.iter().find_map(|t| {
            forward_state(runtime, t)
                .0
                .is_some_and(|state| state.status == ConnectionStatus::Error)
                .then(|| t.id.to_string())
        });
        let rank = if problem {
            0
        } else if matches!(
            connection,
            Some(ConnectionStatus::Connecting | ConnectionStatus::Reconnecting)
        ) {
            1
        } else if connection == Some(ConnectionStatus::Connected) {
            2
        } else if forward.is_some() {
            3
        } else {
            4
        };
        let mut status = match rank {
            0 if connection == Some(ConnectionStatus::Error) => "Failed",
            0 => "Unreachable",
            1 if connection == Some(ConnectionStatus::Reconnecting) => "Reconnecting…",
            1 => "Connecting…",
            2 => "Forwarding",
            _ => "Forwarding off",
        }
        .to_owned();
        // Always on retries without an attempt limit.
        if rank == 1
            && connection == Some(ConnectionStatus::Reconnecting)
            && state.is_some_and(|s| s.reconnect_attempt > 0)
        {
            status = "Retrying…".to_owned();
        }
        let reverse = p.reverse();
        let number = p.number().map(u64::from).unwrap_or_default();
        let port = if let Some(definition) = p.definition() {
            if reverse {
                range(definition.local_port, definition.local_port_end)
            } else {
                range(definition.remote_port, definition.remote_port_end)
            }
        } else {
            number.to_string()
        };
        let service = p.label();
        let fallback = if entry.is_saved() {
            format!("Port {port}")
        } else {
            service.to_owned()
        };
        let title = newport_domain::port_discovery::display(
            forward
                .map(|t| t.name.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&fallback),
        );
        let base = worktree.as_ref().map(|w| w.path.as_str()).unwrap_or(key);
        let context = if entry.is_saved() {
            String::new()
        } else {
            p.detail((!base.is_empty()).then_some(base))
        };
        let generated_title = title == service
            || title == p.forward_name()
            || entry.forwards.iter().any(|forward| {
                title == format!("Terminal · {}:{}", forward.remote_host, forward.remote_port)
            });
        let mut details = Vec::new();
        // Every saved forward to the server says so, whatever its status:
        // these are exactly the forwards Clean up offers.
        if entry.is_saved() && !reverse {
            details.push("Destination not observed".to_owned());
        }
        if problem {
            for t in &entry.forwards {
                let (state, health) = forward_state(runtime, t);
                if let Some(error) = state
                    .and_then(|state| state.error_message.as_deref())
                    .filter(|s| !s.is_empty())
                {
                    details.push(error.to_owned());
                } else if trouble_for(runtime, t, destination_port) {
                    details.push(
                        if state.is_some_and(|state| state.status == ConnectionStatus::Error) {
                            "The forward stopped.".into()
                        } else {
                            forwarding::status_for(state, health, destination_port)
                        },
                    );
                }
            }
        } else if rank == 1 {
            details.push(status.clone());
        } else if entry.is_saved() {
            if reverse {
                details.push(format!("This computer’s port {port}, opened on the server"));
            }
        } else {
            if title != service {
                details.push(service.to_owned());
            }
            if !context.is_empty() {
                details.push(context.clone());
            }
        }
        let branch = if reverse {
            "From this computer".into()
        } else if branches || worktree.as_ref().is_some_and(|w| w.main != Some(true)) {
            worktree
                .as_ref()
                .and_then(|w| w.branch.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let open = open_target(&entry, runtime);
        let address = if let Some((forward, _)) = &open {
            let mut mapped = entry.clone();
            mapped.forwards = vec![forward.clone()];
            local_address(&mapped)
        } else {
            local_address(&entry)
        };
        let identity = p.key();
        let transport = connection;
        let process_icon = crate::connections_process_icon::ProcessIcon::for_port(p);
        let mut hover = crate::connections_hover::Card::new(
            &entry,
            if entry.is_saved() {
                title.clone()
            } else {
                service.clone()
            },
            process_icon,
        );
        if !entry.forwards.is_empty() || entry.is_saved() {
            hover.forward = Some(crate::connections_hover::Forward {
                name: (!entry.is_saved() && !generated_title).then(|| title.clone()),
                address: address.clone(),
                status: status.clone(),
            });
        }
        if problem || entry.is_saved() {
            hover.notice = Some(details.join(" · ")).filter(|s| !s.is_empty());
        }
        Self {
            hover: std::rc::Rc::new(hover),
            entry,
            key: identity,
            project: key.into(),
            project_name: project_name(key).to_owned(),
            branch,
            worktree,
            number,
            port,
            title,
            service,
            process_icon,
            context,
            generated_title,
            detail: details.join(" · "),
            address,
            open,
            status,
            transport,
            rank,
            active,
            problem,
            fix,
        }
    }

    fn presentation_text(&self, detailed: bool) -> (String, String) {
        if self.entry.is_saved() {
            return (self.title.clone(), self.detail.clone());
        }
        let title = if self.generated_title {
            self.service.clone()
        } else {
            self.title.clone()
        };
        let mut parts = Vec::new();
        if detailed && !self.generated_title && self.title != self.service {
            parts.push(self.service.clone());
        }
        if self.problem || self.rank == 1 {
            // Connection failures must never disappear when shared metadata is folded up.
            if self.detail != self.status {
                parts.push(self.detail.clone());
            }
        } else if detailed && !self.context.is_empty() {
            parts.push(self.context.clone());
        }
        (title, parts.join(" · "))
    }
}

pub(super) fn sort(rows: &mut [Row], column: usize, descending: bool) {
    rows.sort_by(|a, b| {
        let order = match column {
            1 => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            2 => (
                a.project.is_empty(),
                a.project_name.to_lowercase(),
                &a.branch,
            )
                .cmp(&(
                    b.project.is_empty(),
                    b.project_name.to_lowercase(),
                    &b.branch,
                )),
            3 => a.rank.cmp(&b.rank),
            _ => a.number.cmp(&b.number),
        };
        (if descending { order.reverse() } else { order })
            .then_with(|| a.number.cmp(&b.number))
            .then_with(|| a.key.cmp(&b.key))
    });
}

impl Newport {
    fn connection_route(&self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let Some((value, remote_port)) = row.open.clone() else {
            return label(row.address.clone())
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(crate::ui::FONT_SIZE))
                .into_any_element();
        };
        let server = self.server.clone();
        crate::controls::compact_button(SharedString::from(format!("open-{}", row.key)))
            .ghost()
            .rounded(px(crate::ui::RADIUS))
            .bg(cx.theme().primary.opacity(0.08))
            .text_color(cx.theme().primary)
            .font_family(cx.theme().mono_font_family.clone())
            .label(row.address.clone())
            .child(Icon::new(IconName::ExternalLink).size(px(crate::controls::ICON_SIZE)))
            .disabled(self.tunnel_busy(&value))
            .accessibility_label(format!("Open {} in browser", row.address))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.server == server {
                    this.open_forward(value.clone(), remote_port, cx);
                }
            }))
            .into_any_element()
    }
    pub(super) fn ports_table(
        &self,
        rows: std::rc::Rc<Vec<Row>>,
        list_state: ListState,
        focus: std::rc::Rc<Vec<FocusHandle>>,
        content_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.connections_state[&self.server];
        let wide = content_width >= WIDE_TABLE_MIN_WIDTH;
        let column = |index, title, cx: &mut Context<Self>| SortColumn {
            owner: cx.weak_entity(),
            server: self.server.clone(),
            index,
            title,
            sorted: state.sort == index,
            descending: state.descending,
        };
        let header = |index, title: &str| {
            let sorted = state.sort == index;
            let descending = state.descending;
            TableHead::new(("ports-heading", index), index + 1)
                .aria_label(title.to_owned())
                .a11y_synthetic_children(move |tree| {
                    if sorted {
                        tree.parent_node().set_sort_direction(if descending {
                            accesskit::SortDirection::Descending
                        } else {
                            accesskit::SortDirection::Ascending
                        });
                    }
                })
                .flex()
                .items_center()
                .px(px(10.))
        };
        Table::new("ports-table")
            .accessibility_label("Ports")
            .row_count(rows.len() + 1)
            .column_count(if wide { 6 } else { 3 })
            .mt(px(10.))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                TableHeader::new("ports-table-head").flex_shrink_0().child(
                    TableRow::new("ports-headings", 1)
                        .flex()
                        .items_center()
                        .min_h(px(34.))
                        .bg(theme::table_header(cx.theme().is_dark()))
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            header(0, "Port")
                                .w(px(if wide { TABLE_PORT_WIDTH } else { 64. }))
                                .flex_shrink_0()
                                .child(column(0, "Port", cx)),
                        )
                        .child(
                            header(1, "Service")
                                .flex_1()
                                .min_w_0()
                                .child(column(1, "Service", cx)),
                        )
                        .when(wide, |v| {
                            v.child(
                                header(2, "Project")
                                    .w(relative(TABLE_PROJECT_SHARE))
                                    .flex_shrink_0()
                                    .child(column(2, "Project", cx)),
                            )
                            .child(
                                header(3, "Status")
                                    .w(px(TABLE_STATUS_WIDTH))
                                    .flex_shrink_0()
                                    .child(column(3, "Status", cx)),
                            )
                            .child(
                                TableHead::new("ports-heading-address", 5)
                                    .aria_label("Local address")
                                    .w(px(TABLE_ADDRESS_WIDTH))
                                    .flex_shrink_0()
                                    .px(px(10.))
                                    .text_right()
                                    .child(
                                        label("Local address").text_size(px(crate::ui::FONT_SIZE)),
                                    ),
                            )
                        })
                        .child(
                            TableHead::new("ports-heading-actions", if wide { 6 } else { 3 })
                                .aria_label("Actions")
                                .w(px(ACTIONS_COLUMN_WIDTH))
                                .flex_shrink_0(),
                        ),
                ),
            )
            .child(
                TableBody::new("ports-table-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(
                        list(
                            list_state,
                            cx.processor(move |this, i: usize, _, cx| {
                                div()
                                    .track_focus(&focus[i])
                                    .child(this.connection_row_at(
                                        rows[i].clone(),
                                        true,
                                        i % 2 == 1,
                                        Some(i + 2),
                                        cx,
                                    ))
                                    .into_any_element()
                            }),
                        )
                        .flex_1()
                        .min_h_0(),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn connection_row(
        &self,
        row: Row,
        position: (usize, usize),
        nested: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        RowPresentation {
            owner: cx.weak_entity(),
            row,
            table: false,
            stripe: false,
            nested,
            index: None,
            position: Some(position),
        }
        .into_any_element()
    }

    fn connection_row_at(
        &self,
        row: Row,
        table: bool,
        stripe: bool,
        row_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        RowPresentation {
            owner: cx.weak_entity(),
            row,
            table,
            stripe,
            nested: false,
            index: row_index,
            position: None,
        }
        .into_any_element()
    }

    fn render_connection_row(
        &self,
        presentation: RowPresentation,
        row_focus: &FocusHandle,
        engaged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let RowPresentation {
            row,
            table,
            stripe,
            nested,
            index: row_index,
            position,
            ..
        } = presentation;
        let content_width = connections_grouped::inner_width(
            f32::from(window.viewport_size().width),
            self.content_width,
        );
        let wide = content_width >= WIDE_TABLE_MIN_WIDTH;
        let compact = content_width <= 560.;
        let inline_route = (table && !wide) || (!table && compact);
        let show_route = table
            || row.open.is_some()
            || row.active
            || row.entry.is_saved()
            || row.entry.forwards.iter().any(|t| {
                t.direction == configuration::Direction::Remote
                    || t.local_port != t.remote_port
                    || t.local_port_end != t.remote_port_end
            });
        let hover = row.hover.clone();
        let (display_title, display_detail) = row.presentation_text(table);
        let mut route = Some(self.connection_route(&row, cx));
        let tone = match row.rank {
            0 => theme::error(cx.theme().is_dark()),
            1 => theme::warning(cx.theme().is_dark()),
            2 => theme::success(cx.theme().is_dark()),
            _ => cx.theme().muted_foreground,
        };
        let dot = retained_transport_dot(&row.key, row.transport, window, cx);
        let fix_server = self.server.clone();
        let fix = row.fix.clone();
        let identity = div()
            .id(SharedString::from(format!("identity-{}", row.key)))
            .debug_selector(|| format!("connection-identity-{}", row.key))
            .when(table, |v| {
                v.role(accesskit::Role::Cell)
                    .aria_column_index(2)
                    .aria_label(display_title.clone())
                    .aria_description(
                        [
                            row.detail.as_str(),
                            if inline_route {
                                row.address.as_str()
                            } else {
                                ""
                            },
                        ]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · "),
                    )
            })
            .flex_1()
            .min_w_0()
            .when(table, |v| v.px(px(10.)))
            .when(!table, |v| v.pr_2())
            .when(
                hover.has_details() && !gpui_kit::base::GlobalState::is_in_deferred_context(cx),
                |view| view.hoverable_tooltip(move |window, cx| hover.clone().build(window, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap_x(px(8.))
                    .child(
                        row.hover.clone().control(
                            SharedString::from(row.key.clone()),
                            crate::controls::compact_button(SharedString::from(format!(
                                "connection-evidence-trigger-{}",
                                row.key
                            )))
                            .ghost()
                            .p_0()
                            .min_w_0()
                            .max_w_full()
                            .accessibility_label(format!(
                                "Connection details for port {}: {}",
                                row.port, display_title
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(row.process_icon.render(cx))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_size(px(crate::ui::FONT_SIZE))
                                            .font_weight(FontWeight::MEDIUM)
                                            .truncate()
                                            .child(display_title.clone()),
                                    )
                                    .when(row.entry.port.publication().is_some(), |v| {
                                        v.child(crate::controls::tag("Docker", cx))
                                    }),
                            ),
                        ),
                    )
                    .when((compact && row.rank <= 1) || (table && !wide), |v| {
                        v.child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(crate::ui::FONT_SIZE))
                                .text_color(tone)
                                .child(row.status.clone()),
                        )
                    }),
            )
            .when(!display_detail.is_empty(), |v| {
                v.child(
                    div()
                        .pl(px(24.))
                        .mt(px(1.))
                        .flex()
                        .flex_wrap()
                        .items_baseline()
                        .gap_x(px(4.))
                        .text_size(px(crate::ui::FONT_SIZE))
                        .line_height(relative(1.45))
                        .text_color(if row.problem {
                            theme::error(cx.theme().is_dark())
                        } else if row.rank == 1 {
                            theme::warning(cx.theme().is_dark())
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(
                            div()
                                .min_w_0()
                                .when(row.problem, |text| text.line_clamp(2))
                                .when(!row.problem, |text| text.truncate())
                                .child(display_detail),
                        )
                        .when_some(fix, |view, id| {
                            view.child(
                                crate::controls::compact_button(SharedString::from(format!(
                                    "fix-forward-{}",
                                    row.key
                                )))
                                .link()
                                .label("Edit forward")
                                .text_color(theme::error(cx.theme().is_dark()))
                                .p_0()
                                .disabled(self.port_busy(&row.entry))
                                .accessibility_label(format!(
                                    "Edit failed forward for port {}",
                                    row.port
                                ))
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if this.server == fix_server {
                                            this.edit_saved_connection(&id, window, cx);
                                        }
                                    },
                                )),
                            )
                        }),
                )
            })
            .when(inline_route && show_route && !row.address.is_empty(), |v| {
                v.child(div().mt(px(3.)).ml(px(-8.)).child(route.take().unwrap()))
            });
        let menu = self.connection_menu(&row, true, cx);
        let pending = self.port_busy(&row.entry);
        let always_on = always_on(&row.entry.forwards);
        // Tables and narrow lists keep fixed action widths; a hinted icon
        // carries the same fact there.
        let always_on_marker = always_on.then(|| {
            if table || compact {
                div()
                    .id(SharedString::from(format!("always-on-{}", row.key)))
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground)
                    .child(Icon::new(IconName::Power).size(px(14.)))
                    .hover_hint("Always on")
                    .into_any_element()
            } else {
                crate::controls::tag("Always on", cx).into_any_element()
            }
        });
        let server = self.server.clone();
        let entry = row.entry.clone();
        let project = row.project.clone();
        let control = crate::controls::switch(SharedString::from(format!("forward-{}", row.key)))
            .color(cx.theme().primary)
            .accessibility_label(format!("Forward port {} ({})", row.port, display_title))
            .checked(row.active)
            .disabled(pending)
            .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                if this.server == server && (!entry.forwards.is_empty() || *checked) {
                    if entry.forwards.is_empty() {
                        this.forward_discovered(entry.clone(), &project, false, window, cx);
                    } else {
                        this.toggle_port(entry.clone(), &project, *checked, cx);
                    }
                }
            }));
        let debug_key = format!("connection-row-{}", row.key);
        div()
            .id(SharedString::from(row.key.clone()))
            .debug_selector(move || debug_key.clone())
            .track_focus(row_focus)
            .tab_stop(false)
            .group(SharedString::from(format!("connection-row-{}", row.key)))
            .role(if table {
                accesskit::Role::Row
            } else {
                accesskit::Role::ListItem
            })
            .when_some(row_index, |v, index| v.aria_row_index(index))
            .when_some(position, |v, (index, count)| {
                v.aria_position_in_set(index).aria_size_of_set(count)
            })
            .aria_label(format!(
                "Port {}: {}, {}",
                row.port, display_title, row.status
            ))
            .flex()
            .items_center()
            .w_full()
            .min_h(px(crate::ui::TWO_LINE_ROW_HEIGHT))
            .py(px(5.))
            .flex_shrink_0()
            .when(!table, |v| v.px_3().rounded(px(crate::ui::RADIUS)))
            // Nested rows indent their content, not their fill, so hover and
            // selection still span the list's full width.
            .when(nested, |v| v.pl(px(12. + if compact { 16. } else { 32. })))
            .when(stripe, |v| v.bg(cx.theme().foreground.opacity(0.02)))
            .when(engaged, |v| v.bg(theme::row_hover(cx)))
            .hover(|v| v.bg(theme::row_hover(cx)))
            .child(
                div()
                    .id("port-number")
                    .when(table, |v| {
                        v.role(accesskit::Role::Cell)
                            .aria_column_index(1)
                            .aria_label(row.port.clone())
                    })
                    .w(px(if table {
                        if wide { TABLE_PORT_WIDTH } else { 64. }
                    } else if compact {
                        72.
                    } else {
                        82.
                    }))
                    .flex_shrink_0()
                    .when(table, |v| v.px(px(10.)))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .child(if table || row.active || row.problem {
                        dot
                    } else {
                        div()
                            .w(px(crate::ui::DOT_SIZE))
                            .flex_shrink_0()
                            .into_any_element()
                    })
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(crate::ui::FONT_SIZE))
                            .text_color(if row.entry.forwards.is_empty() {
                                cx.theme().muted_foreground
                            } else {
                                cx.theme().foreground
                            })
                            .child(row.port),
                    ),
            )
            .child(identity)
            .when(table && wide, |v| {
                v.child(
                    div()
                        .id("port-project")
                        .role(accesskit::Role::Cell)
                        .aria_column_index(3)
                        .aria_label(if row.project.is_empty() {
                            "Other".to_owned()
                        } else {
                            row.project_name.clone()
                        })
                        .aria_description(row.branch.clone())
                        .w(relative(TABLE_PROJECT_SHARE))
                        .flex_shrink_0()
                        .min_w_0()
                        .px(px(10.))
                        .text_size(px(crate::ui::FONT_SIZE))
                        .child(
                            div()
                                .truncate()
                                .when(row.project.is_empty(), |v| {
                                    v.text_color(cx.theme().muted_foreground)
                                })
                                .child(if row.project.is_empty() {
                                    "Other".to_owned()
                                } else {
                                    row.project_name.clone()
                                }),
                        )
                        .when(!row.branch.is_empty(), |v| {
                            v.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        Icon::new(IconName::GitBranch)
                                            .size(px(crate::ui::SMALL_ICON)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(crate::ui::FONT_SIZE))
                                            .child(row.branch),
                                    ),
                            )
                        }),
                )
                .child(
                    div()
                        .id("port-status")
                        .role(accesskit::Role::Cell)
                        .aria_column_index(4)
                        .aria_label(
                            if row.rank == 4 {
                                "Not forwarded"
                            } else {
                                &row.status
                            }
                            .to_owned(),
                        )
                        .w(px(TABLE_STATUS_WIDTH))
                        .flex_shrink_0()
                        .px(px(10.))
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(tone)
                        .child(row.status.clone()),
                )
            })
            .when(!inline_route && show_route, |v| {
                v.child(
                    div()
                        .id("port-route")
                        .when(table, |v| {
                            v.role(accesskit::Role::Cell)
                                .aria_column_index(5)
                                .aria_label(if row.address.is_empty() {
                                    "No forwarded address".to_owned()
                                } else {
                                    row.address.clone()
                                })
                                .w(px(TABLE_ADDRESS_WIDTH))
                                .px(px(10.))
                        })
                        .flex_shrink_0()
                        .when(!table, |v| v.w(px(184.)).pr(px(12.)))
                        .text_right()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(cx.theme().muted_foreground)
                        .child(route.take().unwrap()),
                )
            })
            .child(
                div()
                    .id("port-actions")
                    .when(table, |v| {
                        v.role(accesskit::Role::Cell)
                            .aria_column_index(if wide { 6 } else { 3 })
                            .aria_label("Actions")
                    })
                    .map(|v| {
                        if table {
                            v.w(px(ACTIONS_COLUMN_WIDTH))
                        } else {
                            // Toggles stay aligned on the right edge while a
                            // row's tag widens its own cell.
                            v.min_w(px(if !compact && row.rank <= 1 { 166. } else { 72. }))
                        }
                    })
                    .flex_shrink_0()
                    .pr(px(if table { 10. } else { 0. }))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(if table {
                        6.
                    } else if compact {
                        10.
                    } else {
                        12.
                    }))
                    .when(!table && !compact && row.rank <= 1, |v| {
                        v.child(
                            div()
                                .text_size(px(crate::ui::FONT_SIZE))
                                .text_color(tone)
                                .child(row.status.clone()),
                        )
                    })
                    .children(always_on_marker)
                    .child(control)
                    .child(menu),
            )
            .into_any_element()
    }

    fn connection_menu(&self, row: &Row, engaged: bool, cx: &mut Context<Self>) -> AnyElement {
        let forwards = row.entry.forwards.clone();
        let server = self.server.clone();
        let weak = cx.weak_entity();
        let address = row.address.clone();
        let entry = row.entry.clone();
        let project = row.project.clone();
        let pending = self.port_busy(&row.entry);
        RowMenu {
            id: SharedString::from(format!("actions-visibility-{}", row.key)),
            group: SharedString::from(format!("connection-row-{}", row.key)),
            engaged,
            button: crate::controls::icon_button(SharedString::from(format!(
                "actions-{}",
                row.key
            )))
            .icon(IconName::Ellipsis)
            .ghost()
            .disabled(pending)
            .accessibility_label(format!("More actions for port {}", row.port)),
            builder: Box::new(move |mut menu, _, _| {
                if forwards.is_empty() {
                    let weak = weak.clone();
                    let server = server.clone();
                    let entry = entry.clone();
                    let project = project.clone();
                    return menu.item(
                        PopupMenuItem::new("Forward with options…")
                            .icon(IconName::Settings2)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.server != server {
                                        return;
                                    }
                                    this.forward_discovered(
                                        entry.clone(),
                                        &project,
                                        true,
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    );
                }
                for t in &forwards {
                    let weak = weak.clone();
                    let server = server.clone();
                    let id = t.id.to_string();
                    let title = if forwards.len() == 1 {
                        "Edit…".into()
                    } else {
                        format!("Edit {}…", tunnel_delete::name(t))
                    };
                    menu = menu.item(PopupMenuItem::new(title).icon(IconName::Pencil).on_click(
                        move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                if this.server == server {
                                    this.edit_saved_connection(&id, window, cx);
                                }
                            });
                        },
                    ));
                }
                let address = address.clone();
                menu = menu
                    .item(PopupMenuItem::new("Copy").icon(IconName::Copy).on_click(
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(address.clone()))
                        },
                    ))
                    .separator();
                {
                    let owner = weak.clone();
                    let expected_server = server.clone();
                    let values = forwards.clone();
                    let enabled = always_on(&forwards);
                    menu = menu
                        .item(
                            PopupMenuItem::new("Always on")
                                .icon(IconName::Power)
                                .checked(enabled)
                                .on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        if this.server == expected_server {
                                            this.set_always_on(values.clone(), !enabled, cx);
                                        }
                                    });
                                }),
                        )
                        .separator();
                }
                for t in &forwards {
                    let owner = weak.clone();
                    let expected_server = server.clone();
                    let value = t.clone();
                    let title = if forwards.len() == 1 {
                        "Delete…".into()
                    } else {
                        format!("Delete {}…", tunnel_delete::name(t))
                    };
                    menu = menu.item(PopupMenuItem::new(title).icon(IconName::Trash).on_click(
                        move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.server == expected_server {
                                    this.confirm_delete_tunnel(value.clone(), window, cx);
                                }
                            });
                        },
                    ));
                }
                menu
            }),
        }
        .into_any_element()
    }

    fn edit_saved_connection(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .saved_forwards()
            .unwrap_or_default()
            .into_iter()
            .find(|t| t.id.to_string() == id && t.server_id.to_string() == self.server);
        if let Some(value) = value {
            self.edit_tunnel(Some(value), window, cx);
        } else {
            self.notification = Some(Notification::error("This forward no longer exists."));
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Row, always_on, local_address, open_target, sort};
    use crate::connections::fixtures::Listener;
    use crate::snapshot_fixture as fixture;
    use newport_domain::port_grouping::Port;
    use newport_protocol::desktop::configuration::{
        ConnectionState, ConnectionStatus, DestinationHealth, DestinationHealthList,
        DestinationStatus, Runtime,
    };
    use newport_protocol::desktop::configuration::{Direction, Tunnel};
    const A: &str = "00000000-0000-4000-8000-00000000000a";
    const UP: &str = "00000000-0000-4000-8000-00000000000b";
    const FAILED: &str = "00000000-0000-4000-8000-00000000000c";
    fn id(value: &str) -> uuid::Uuid {
        uuid::Uuid::parse_str(value).unwrap()
    }
    /// The saved forward `forward`, to port 0 until a test says otherwise.
    fn tunnel(forward: &str) -> Tunnel {
        fixture::tunnel(id(forward), fixture::SERVER, 0, 0)
    }
    /// Whether the saved forward is on whenever Newport runs.
    fn policy(auto_connect: bool) -> Tunnel {
        Tunnel {
            auto_connect,
            ..tunnel(A)
        }
    }
    fn state(status: ConnectionStatus, error: Option<&str>) -> ConnectionState {
        ConnectionState {
            error_message: error.map(Into::into),
            ..fixture::connection(status)
        }
    }
    /// A runtime with these forwards' connections.
    fn runtime(states: &[(&str, ConnectionState)]) -> Runtime {
        let mut runtime = fixture::runtime();
        for (forward, state) in states {
            fixture::set(&mut runtime.tunnels, id(forward), state.clone());
        }
        runtime
    }
    fn health(
        remote_port: u16,
        status: DestinationStatus,
        message: Option<&str>,
    ) -> DestinationHealth {
        DestinationHealth {
            local_port: remote_port,
            remote_port,
            status,
            message: message.map(Into::into),
            checked_at: None,
        }
    }
    fn set_health(runtime: &mut Runtime, forward: &str, values: Vec<DestinationHealth>) {
        fixture::set(
            &mut runtime.tunnel_health,
            id(forward),
            DestinationHealthList { values },
        );
    }
    #[test]
    fn always_on_requires_every_saved_forward_on_the_row() {
        assert!(!always_on(&[]), "an unsaved port has no policy");
        assert!(always_on(&[policy(true)]));
        assert!(!always_on(&[policy(false)]));
        assert!(!always_on(&[policy(true), policy(false)]));
    }
    fn row(port: u64, project: &str) -> Row {
        Row::new(
            project,
            Entry {
                addresses: vec![],
                port: Listener::new(u16::try_from(port).unwrap()).port(),
                forwards: vec![],
            },
            None,
            false,
        )
    }
    #[test]
    fn live_identity_is_consistent_without_losing_saved_names_or_failures() {
        let entry = |name| Entry {
            port: Listener::new(5179)
                .process("node", "node", &["node", "vite"], Some("/srv/app/web"))
                .project("/srv/app")
                .checkout("/srv/app", Some("feature"), Some(false))
                .port(),
            addresses: vec!["127.0.0.1".into()],
            forwards: vec![Tunnel {
                name: String::from(name),
                remote_host: "127.0.0.1".into(),
                ..fixture::tunnel(id(A), fixture::SERVER, 5179, 5179)
            }],
        };
        for name in ["Vite", "Vite · feature", "Terminal · 127.0.0.1:5179"] {
            let row = Row::new("/srv/app", entry(name), None, true);
            assert_eq!(row.presentation_text(false), ("Vite".into(), "".into()));
            assert_eq!(row.status, "Forwarding off");
            assert_eq!(row.entry.forwards[0].name, name);
        }
        let row = Row::new("/srv/app", entry("Customer preview"), None, true);
        assert_eq!(
            row.presentation_text(false),
            ("Customer preview".into(), "".into())
        );
        assert!(row.presentation_text(true).1.contains("Vite"));
        let refused = runtime(&[(
            A,
            state(ConnectionStatus::Error, Some("Connection refused")),
        )]);
        let row = Row::new("/srv/app", entry("Customer preview"), Some(&refused), true);
        assert_eq!(
            row.presentation_text(false),
            ("Customer preview".into(), "Connection refused".into())
        );
        let mut saved = entry("Customer preview");
        saved.port = Port::saved(saved.forwards[0].clone());
        let row = Row::new("/srv/app", saved.clone(), None, true);
        assert_eq!(row.presentation_text(false).0, "Customer preview");
        let detail = row.presentation_text(false).1;
        assert!(detail == "Destination not observed", "{detail}");
        // A failing or connecting saved forward still says so, so the label
        // always marks the forwards Clean up offers.
        for (runtime, status) in [
            (refused, "Connection refused"),
            (
                runtime(&[(A, state(ConnectionStatus::Connecting, None))]),
                "",
            ),
        ] {
            let row = Row::new("/srv/app", saved.clone(), Some(&runtime), true);
            let detail = row.presentation_text(false).1;
            assert!(detail.starts_with("Destination not observed"), "{detail}");
            assert!(detail.contains(status), "{detail}");
        }
        let connected = runtime(&[(A, state(ConnectionStatus::Connected, None))]);
        let row = Row::new("/srv/app", saved, Some(&connected), true);
        assert_eq!(row.presentation_text(false).1, "Destination not observed");
    }

    #[test]
    fn table_sort_is_numeric_stable_and_other_projects_sort_last() {
        let mut rows = vec![
            row(100, ""),
            row(20, "/srv/z"),
            row(3, "/srv/a"),
            row(10, "/srv/a"),
        ];
        sort(&mut rows, 0, false);
        assert_eq!(
            rows.iter().map(|r| r.number).collect::<Vec<_>>(),
            vec![3, 10, 20, 100]
        );
        sort(&mut rows, 0, true);
        assert_eq!(rows[0].number, 100);
        sort(&mut rows, 2, false);
        assert_eq!(rows.last().unwrap().project, "");
        assert_eq!(rows[0].number, 3);
    }
    #[test]
    fn reverse_and_range_addresses_retain_their_meaning() {
        let mut entry = Entry {
            addresses: vec![],
            port: Listener::new(3002).port(),
            forwards: vec![Tunnel {
                local_port_end: Some(9005),
                remote_port_end: Some(3005),
                ..fixture::tunnel(id(A), fixture::SERVER, 9000, 3000)
            }],
        };
        assert_eq!(local_address(&entry), "localhost:9002");
        entry.port = Port::saved(entry.forwards[0].clone());
        assert_eq!(local_address(&entry), "localhost:9000–9005");
        entry.forwards[0].direction = Direction::Remote;
        entry.forwards[0].remote_host = "::1".into();
        assert_eq!(local_address(&entry), "server:3000–3005");
    }
    #[test]
    fn destination_failure_takes_precedence_over_a_connected_forward() {
        let entry = Entry {
            addresses: vec![],
            port: Listener::new(3000).port(),
            forwards: vec![tunnel(UP), tunnel(FAILED)],
        };
        let runtime = runtime(&[
            (UP, state(ConnectionStatus::Connected, None)),
            (FAILED, state(ConnectionStatus::Error, Some("Denied"))),
        ]);
        let row = Row::new("", entry, Some(&runtime), false);
        assert_eq!(row.status, "Failed");
        assert_eq!(row.detail, "Denied");
        assert_eq!(row.rank, 0);
        assert_eq!(row.fix.as_deref(), Some(FAILED));
        assert_eq!(row.transport, Some(ConnectionStatus::Error));
    }

    #[test]
    fn destination_issue_does_not_masquerade_as_ssh_failure_or_offer_wrong_repair() {
        let entry = Entry {
            addresses: vec![],
            port: Listener::new(8080).port(),
            forwards: vec![tunnel(UP)],
        };
        let mut runtime = runtime(&[(UP, state(ConnectionStatus::Connected, None))]);
        set_health(
            &mut runtime,
            UP,
            vec![health(
                8080,
                DestinationStatus::Unavailable,
                Some("Connection refused"),
            )],
        );
        let row = Row::new("", entry.clone(), Some(&runtime), false);
        assert_eq!(row.transport, Some(ConnectionStatus::Connected));
        assert_eq!(row.status, "Unreachable");
        assert!(row.fix.is_none());
        let retrying = |attempt| ConnectionState {
            reconnect_attempt: attempt,
            ..state(ConnectionStatus::Reconnecting, None)
        };
        fixture::set(&mut runtime.tunnels, id(UP), retrying(3));
        let row = Row::new("", entry.clone(), Some(&runtime), false);
        assert_eq!(row.status, "Retrying…");
        assert_eq!(row.detail, row.status);
        assert!(row.fix.is_none());
        // A high attempt number still reads as retrying.
        fixture::set(&mut runtime.tunnels, id(UP), retrying(12));
        let row = Row::new("", entry.clone(), Some(&runtime), false);
        assert_eq!(row.status, "Retrying…");
        fixture::set(
            &mut runtime.tunnels,
            id(UP),
            state(ConnectionStatus::Error, None),
        );
        let row = Row::new("", entry, Some(&runtime), false);
        assert_eq!(row.detail, "The forward stopped.");
        assert_eq!(row.fix.as_deref(), Some(UP));
    }

    #[test]
    fn destination_health_cannot_mask_another_forwards_transport_error() {
        let mut entry = Entry {
            addresses: vec![],
            port: Listener::new(8080).port(),
            forwards: vec![tunnel(UP), tunnel(FAILED)],
        };
        let mut runtime = runtime(&[
            (UP, state(ConnectionStatus::Connected, None)),
            (FAILED, state(ConnectionStatus::Error, None)),
        ]);
        set_health(
            &mut runtime,
            UP,
            vec![health(8080, DestinationStatus::Unavailable, None)],
        );
        for _ in 0..2 {
            let row = Row::new("", entry.clone(), Some(&runtime), false);
            assert_eq!(row.transport, Some(ConnectionStatus::Error));
            assert_eq!(row.status, "Failed");
            assert_eq!(row.fix.as_deref(), Some(FAILED));
            entry.forwards.reverse();
        }
    }

    #[test]
    fn browser_route_uses_the_connected_mapping_and_preserves_range_member() {
        let server = uuid::Uuid::new_v4();
        let mut first =
            newport_domain::forwarding::discovered_tunnel(&[], server, "127.0.0.1", 3000).unwrap();
        first.local_port = 9000;
        first.local_port_end = Some(9005);
        first.remote_port_end = Some(3005);
        let mut second = first.clone();
        second.id = uuid::Uuid::new_v4();
        second.local_port = 10000;
        second.local_port_end = Some(10005);
        let (first, second) = (
            newport_protocol::desktop::configuration::Tunnel::from(&first),
            newport_protocol::desktop::configuration::Tunnel::from(&second),
        );
        let entry = Entry {
            addresses: vec![],
            port: Listener::new(3002).port(),
            forwards: vec![first.clone(), second.clone()],
        };
        let mut runtime = fixture::runtime();
        fixture::set(
            &mut runtime.tunnels,
            first.id,
            state(ConnectionStatus::Disconnected, None),
        );
        fixture::set(
            &mut runtime.tunnels,
            second.id,
            state(ConnectionStatus::Connected, None),
        );
        let row = Row::new("", entry.clone(), Some(&runtime), false);
        assert_eq!(row.address, "localhost:10002");
        assert_eq!(row.open, Some((second.clone(), Some(3002))));
        set_health(
            &mut runtime,
            &second.id.to_string(),
            vec![
                health(
                    3000,
                    DestinationStatus::Unavailable,
                    Some("Connection refused"),
                ),
                health(3002, DestinationStatus::Reachable, None),
            ],
        );
        let row = Row::new("", entry.clone(), Some(&runtime), false);
        assert_eq!(row.status, "Forwarding");
        assert!(!row.problem);
        for status in [
            ConnectionStatus::Disconnected,
            ConnectionStatus::Connecting,
            ConnectionStatus::Reconnecting,
            ConnectionStatus::Error,
        ] {
            fixture::set(&mut runtime.tunnels, second.id, state(status, None));
            assert!(open_target(&entry, Some(&runtime)).is_none(), "{status:?}");
        }
        fixture::set(
            &mut runtime.tunnels,
            second.id,
            state(ConnectionStatus::Connected, None),
        );
        let mut reverse = entry.clone();
        reverse.forwards[1].direction = Direction::Remote;
        assert!(open_target(&reverse, Some(&runtime)).is_none());
        let mut outside = entry.clone();
        outside.port = Listener::new(3006).port();
        assert!(open_target(&outside, Some(&runtime)).is_none());
        let saved = Entry {
            addresses: vec![],
            port: Port::saved(second.clone()),
            forwards: vec![second.clone()],
        };
        let row = Row::new("", saved, Some(&runtime), false);
        assert_eq!(row.address, "localhost:10000–10005");
        assert_eq!(row.open, Some((second, None)));
        assert!(
            row.problem,
            "A standalone saved range still shows its failed member"
        );
    }
    #[test]
    fn service_context_matches_reference_and_keeps_saved_alias() {
        let entry = Entry {
            port: Listener::new(5173)
                .address("::")
                .process(
                    "MainThread",
                    "/usr/bin/node",
                    &["node", "/srv/app/node_modules/vite/bin/vite.js"],
                    Some("/srv/app/packages/web"),
                )
                .port(),
            addresses: vec!["127.0.0.1".into(), "::".into()],
            forwards: vec![Tunnel {
                name: "Customer preview".into(),
                ..tunnel(A)
            }],
        };
        let row = Row::new("/srv/app", entry, None, false);
        assert_eq!(row.title, "Customer preview");
        assert_eq!(row.detail, "Vite · packages/web/ · Node · all interfaces");
        let mut first = super::tests::row(100, "/srv/z");
        first.project_name = "Alpha".into();
        let mut second = super::tests::row(3, "/srv/a");
        second.project_name = "Zulu".into();
        let mut rows = vec![second, first];
        sort(&mut rows, 2, false);
        assert_eq!(rows[0].project, "/srv/z");
    }
    #[test]
    fn stale_forward_menu_opens_existing_range_without_renaming_it() {
        let server = uuid::Uuid::new_v4();
        let mut tunnel =
            newport_domain::forwarding::discovered_tunnel(&[], server, "127.0.0.1", 8080).unwrap();
        tunnel.local_port_end = Some(8082);
        tunnel.remote_port_end = Some(8082);
        tunnel.name = "Saved by another window".into();
        tunnel.project = Some("/srv/original".into());
        let (value, existing) = super::editor_draft(
            &[tunnel.clone()],
            server,
            "0.0.0.0",
            8081,
            "New name",
            "/srv/new",
        )
        .unwrap();
        assert!(existing);
        assert_eq!(value, tunnel);
        let (value, existing) =
            super::editor_draft(&[], server, "0.0.0.0", 8081, "Vite · feature", "/srv/new")
                .unwrap();
        assert!(!existing);
        assert_eq!(value.name, "Vite · feature");
        assert_eq!(value.project.as_deref(), Some("/srv/new"));
    }
    #[test]
    fn checkout_identity_and_relative_details_use_normalized_path() {
        let make = |path| Entry {
            port: Listener::new(5173)
                .process(
                    "node",
                    "/usr/bin/node",
                    &["node", "vite"],
                    Some("/srv/app/packages/web"),
                )
                .project("/srv")
                .checkout(path, Some("feature"), Some(false))
                .port(),
            addresses: vec!["127.0.0.1".into()],
            forwards: vec![],
        };
        let first = Row::new("/srv", make("/srv/app/"), None, true);
        let second = Row::new("/srv", make("/srv/app"), None, true);
        assert_eq!(first.worktree, second.worktree);
        assert_eq!(first.detail, "packages/web/ · Node");
        assert_eq!(first.detail, second.detail);
    }
}

#[cfg(test)]
mod motion_tests {
    use super::transport_dot;
    use gpui::{Context, IntoElement, Render, TestAppContext, Window};
    use gpui_kit as gpui;

    use newport_protocol::desktop::configuration::ConnectionStatus;

    struct TransportHost(Option<ConnectionStatus>);

    impl Render for TransportHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            transport_dot(self.0, cx)
        }
    }

    #[gpui::test]
    fn pulse_stops_scheduling_frames_when_transport_disconnects(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let (view, cx) =
            cx.add_window_view(|_, _| TransportHost(Some(ConnectionStatus::Connecting)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let frames = cx.update(|window, cx| window.simulate_next_frame(cx));
        assert!(frames > 0, "connecting transport schedules its pulse");
        view.update(cx, |view, cx| {
            view.0 = Some(ConnectionStatus::Disconnected);
            cx.notify();
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            // The old animation may have one frame already queued.
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
    }

    #[gpui::test]
    fn reduced_motion_and_settled_states_do_not_schedule_animation(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let (view, cx) =
            cx.add_window_view(|_, _| TransportHost(Some(ConnectionStatus::Disconnected)));
        for (reduced, status) in [
            (false, Some(ConnectionStatus::Disconnected)),
            (true, Some(ConnectionStatus::Connected)),
            (false, Some(ConnectionStatus::Error)),
            (true, Some(ConnectionStatus::Connecting)),
            (true, Some(ConnectionStatus::Reconnecting)),
            (false, None),
        ] {
            view.update(cx, |view, _| {
                view.0 = status;
            });
            cx.update(|window, cx| {
                cx.set_reduce_motion(reduced);
                window.draw(cx).clear(cx);
                assert_eq!(
                    window.simulate_next_frame(cx),
                    0,
                    "{status:?}, reduced={reduced}"
                );
            });
        }
    }
}
