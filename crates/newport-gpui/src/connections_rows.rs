//! Shared row presentation and table ordering for Connections.
use super::connections_grouped::{Entry, string, trouble_for};
use super::*;
use gpui_kit::base::{Table, TableBody, TableHead, TableHeader, TableRow};
use gpui_kit::component::animation::cubic_bezier;

/// Match the reference StateDot: only unsettled SSH transport pulses. Destination
/// health is reported separately and must not animate an otherwise settled row.
fn transport_dot(status: &str, cx: &App) -> AnyElement {
    let dark = cx.theme().is_dark();
    let tone = match status {
        "error" => Some(theme::error(dark)),
        "connecting" | "reconnecting" => Some(theme::warning(dark)),
        "connected" => Some(theme::success(dark)),
        _ => None,
    };
    let dot = div()
        .w(px(8.))
        .h(px(8.))
        .flex_shrink_0()
        .rounded_full()
        .when_some(tone, |v, color| v.bg(color))
        .when(status == "connected", |v| {
            v.shadow(vec![BoxShadow {
                color: theme::success(dark).opacity(0.15),
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(3.),
                inset: false,
            }])
        })
        .when(tone.is_none(), |v| {
            v.border(px(1.5))
                .border_color(cx.theme().muted_foreground.opacity(0.35))
        });
    if matches!(status, "connecting" | "reconnecting") && !cx.reduce_motion() {
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
                .rounded(px(4.))
                .child(
                    div()
                        .text_size(px(11.))
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

pub(super) fn project_name(key: &str) -> &str {
    if key.is_empty() {
        "Other services"
    } else {
        key.strip_prefix("compose:")
            .unwrap_or_else(|| key.rsplit('/').next().unwrap_or(key))
    }
}
fn range(value: &Value, start: &str, end: &str) -> String {
    let first = value[start].as_u64().unwrap_or_default();
    match value[end].as_u64().filter(|last| *last != first) {
        Some(last) => format!("{first}–{last}"),
        None => first.to_string(),
    }
}
fn local_address(entry: &Entry) -> String {
    let Some(t) = entry.forwards.first() else {
        return String::new();
    };
    if t["direction"] == "remote" {
        let host = string(t, "remoteHost");
        let host = match host {
            "localhost" | "127.0.0.1" | "::1" => "server".into(),
            _ if host.contains(':') && !host.starts_with('[') => format!("[{host}]"),
            _ => host.to_owned(),
        };
        return format!("{host}:{}", range(t, "remotePort", "remotePortEnd"));
    }
    let port = if entry.saved {
        range(t, "localPort", "localPortEnd")
    } else {
        let offset = entry.port["port"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(t["remotePort"].as_u64().unwrap_or(0));
        (t["localPort"].as_u64().unwrap_or(0) + offset).to_string()
    };
    format!("localhost:{port}")
}

/// Select a connected local mapping; another forward's error must not retarget it.
fn open_target(entry: &Entry, runtime: &Value) -> Option<(Value, Option<u16>)> {
    let remote_port = if entry.saved {
        None
    } else {
        Some(u16::try_from(entry.port["port"].as_u64()?).ok()?)
    };
    entry.forwards.iter().find_map(|value| {
        let tunnel = serde_json::from_value::<newport_domain::model::Tunnel>(value.clone()).ok()?;
        (runtime["tunnels"][tunnel.id.to_string()]["status"] == "connected"
            && newport_domain::forwarding::local_endpoint(&tunnel, remote_port).is_ok())
        .then(|| (value.clone(), remote_port))
    })
}

/// The latest snapshot may already contain a forward created while the menu was open.
/// In that case, show its saved settings as an edit instead of renaming its ID.
fn editor_draft(
    tunnels: &[newport_domain::model::Tunnel],
    server: uuid::Uuid,
    address: &str,
    port: u16,
    name: &str,
    project: &str,
) -> Result<(Value, bool), String> {
    let mut tunnel = newport_domain::forwarding::discovered_tunnel(tunnels, server, address, port)?;
    let existing = tunnels.iter().any(|saved| saved.id == tunnel.id);
    if !existing {
        tunnel.name = name.to_owned();
        tunnel.project = (!project.is_empty()).then(|| project.to_owned());
    }
    Ok((
        serde_json::to_value(tunnel).map_err(|e| e.to_string())?,
        existing,
    ))
}

#[derive(Clone)]
pub(super) struct Row {
    pub(super) entry: Entry,
    pub(super) key: String,
    pub(super) project: String,
    pub(super) project_name: String,
    pub(super) branch: String,
    pub(super) worktree: Option<newport_domain::port_grouping::Worktree>,
    number: u64,
    port: String,
    title: String,
    detail: String,
    address: String,
    open: Option<(Value, Option<u16>)>,
    status: String,
    transport: String,
    rank: u8,
    active: bool,
    problem: bool,
    fix: Option<String>,
}
impl Row {
    pub(super) fn new(key: &str, entry: Entry, runtime: &Value, branches: bool) -> Self {
        let p = &entry.port;
        let worktree = (!entry.saved)
            .then(|| newport_domain::port_grouping::worktree_of(p))
            .flatten();
        let destination_port = (!entry.saved).then(|| p["port"].as_u64()).flatten();
        let forward = entry.forwards.first();
        let state = entry
            .forwards
            .iter()
            .min_by_key(|t| {
                // The dot describes SSH transport, independently of whether
                // the process at a destination is listening (PortRow.rowState).
                match string(&runtime["tunnels"][string(t, "id")], "status") {
                    "error" => 0,
                    "reconnecting" => 1,
                    "connecting" => 2,
                    "connected" => 3,
                    _ => 4,
                }
            })
            .map(|t| &runtime["tunnels"][string(t, "id")]);
        let connection = state.map(|s| string(s, "status")).unwrap_or("");
        let active = matches!(connection, "connected" | "connecting" | "reconnecting");
        let problem = entry
            .forwards
            .iter()
            .any(|t| trouble_for(runtime, t, destination_port));
        // With several forwards on a port, repair the failed one rather than
        // accidentally opening the first (possibly healthy) mapping.
        let fix = entry.forwards.iter().find_map(|t| {
            (runtime["tunnels"][string(t, "id")]["status"] == "error")
                .then(|| string(t, "id").to_owned())
        });
        let rank = if problem {
            0
        } else if matches!(connection, "connecting" | "reconnecting") {
            1
        } else if connection == "connected" {
            2
        } else if forward.is_some() {
            3
        } else {
            4
        };
        let mut status = match rank {
            0 if connection == "error" => "Failed",
            0 => "Unreachable",
            1 if connection == "reconnecting" => "Reconnecting…",
            1 => "Connecting…",
            2 => "Connected",
            3 => "Off",
            _ => "—",
        }
        .to_owned();
        if rank == 1 && connection == "reconnecting" {
            let attempt = state
                .and_then(|s| s["reconnectAttempt"].as_u64())
                .unwrap_or(0);
            if attempt > 0 {
                status = format!("Retrying {attempt}/10…");
            }
        }
        let reverse = entry.saved && p["direction"] == "remote";
        let field = if !entry.saved {
            "port"
        } else if reverse {
            "localPort"
        } else {
            "remotePort"
        };
        let number = p[field].as_u64().unwrap_or_default();
        let port = if entry.saved {
            range(
                p,
                field,
                if reverse {
                    "localPortEnd"
                } else {
                    "remotePortEnd"
                },
            )
        } else {
            number.to_string()
        };
        let service = newport_domain::port_context::service_name(p)
            .unwrap_or_else(|| "Owner not reported".into());
        let fallback = if entry.saved {
            format!("Port {port}")
        } else {
            service.to_owned()
        };
        let title = forward
            .and_then(|t| t["name"].as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&fallback)
            .to_owned();
        let mut details = Vec::new();
        if problem {
            for t in &entry.forwards {
                let id = string(t, "id");
                if let Some(error) = runtime["tunnels"][id]["errorMessage"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                {
                    details.push(error.to_owned());
                } else if trouble_for(runtime, t, destination_port) {
                    details.push(if runtime["tunnels"][id]["status"] == "error" {
                        "The forward stopped.".into()
                    } else {
                        forwarding::status_for(runtime, id, destination_port)
                    });
                }
            }
        } else if rank == 1 {
            details.push(status.clone());
        } else if entry.saved {
            details.push(if reverse {
                format!("This Mac’s port {port}, opened on the server")
            } else {
                format!(
                    "Nothing listening on {}:{port} yet",
                    string(p, "remoteHost")
                )
            });
        } else {
            if title != service && service != "Owner not reported" {
                details.push(service.to_owned());
            }
            let base = worktree.as_ref().map(|w| w.path.as_str()).unwrap_or(key);
            details.extend(newport_domain::port_context::service_detail(
                p,
                &entry.addresses,
                (!base.is_empty()).then_some(base),
            ));
        }
        let branch = if reverse {
            "From this computer".into()
        } else if branches || worktree.as_ref().is_some_and(|w| !w.main) {
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
        let identity = if entry.saved {
            format!("saved-{}", string(p, "id"))
        } else {
            format!(
                "port-{number}-{}-{}-{}",
                p["pid"],
                string(p, "address"),
                string(p, "containerName")
            )
        };
        let transport = connection.to_owned();
        Self {
            entry,
            key: identity,
            project: key.into(),
            project_name: project_name(key).to_owned(),
            branch,
            worktree,
            number,
            port,
            title,
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
                .text_size(px(11.5))
                .into_any_element();
        };
        let server = self.server.clone();
        crate::controls::compact_button(SharedString::from(format!("open-{}", row.key)))
            .ghost()
            .rounded(px(6.))
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
        let wide = content_width > 640.;
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
                        .bg(rgb(if cx.theme().is_dark() {
                            0x2a292d
                        } else {
                            0xf5f4f6
                        }))
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            header(0, "Port")
                                .w(px(if wide { 84. } else { 64. }))
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
                                    .w(relative(0.20))
                                    .flex_shrink_0()
                                    .child(column(2, "Project", cx)),
                            )
                            .child(
                                header(3, "Status")
                                    .w(px(124.))
                                    .flex_shrink_0()
                                    .child(column(3, "Status", cx)),
                            )
                            .child(
                                TableHead::new("ports-heading-address", 5)
                                    .aria_label("Local address")
                                    .w(px(184.))
                                    .flex_shrink_0()
                                    .px(px(10.))
                                    .text_right()
                                    .child(label("Local address").text_size(px(11.))),
                            )
                        })
                        .child(
                            TableHead::new("ports-heading-actions", if wide { 6 } else { 3 })
                                .aria_label("Actions")
                                .w(px(84.))
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        RowPresentation {
            owner: cx.weak_entity(),
            row,
            table: false,
            stripe: false,
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
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let RowPresentation {
            row,
            table,
            stripe,
            index: row_index,
            position,
            ..
        } = presentation;
        let content_width = connections_grouped::inner_width(
            f32::from(window.viewport_size().width),
            self.content_width,
        );
        let wide = content_width > 640.;
        let compact = content_width <= 560.;
        let inline_route = (table && !wide) || (!table && compact);
        let p = &row.entry.port;
        let tooltip = if row.entry.saved {
            format!("{}\n{}", row.title, row.detail)
        } else {
            newport_domain::port_context::tooltip(p, &row.entry.addresses)
        };
        let mut route = Some(self.connection_route(&row, cx));
        let tone = match row.rank {
            0 => theme::error(cx.theme().is_dark()),
            1 => theme::warning(cx.theme().is_dark()),
            2 => theme::success(cx.theme().is_dark()),
            _ => cx.theme().muted_foreground,
        };
        let dot = transport_dot(&row.transport, cx);
        let fix_server = self.server.clone();
        let fix = row.fix.clone();
        let identity =
            div()
                .id(SharedString::from(format!("identity-{}", row.key)))
                .when(table, |v| {
                    v.role(accesskit::Role::Cell)
                        .aria_column_index(2)
                        .aria_label(row.title.clone())
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
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(if row.entry.forwards.is_empty() {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::SEMIBOLD
                        })
                        .truncate()
                        .child(row.title.clone()),
                )
                .when(!row.detail.is_empty(), |v| {
                    v.child(
                        div()
                            .mt(px(1.))
                            .flex()
                            .flex_wrap()
                            .items_baseline()
                            .gap_x(px(4.))
                            .text_size(px(11.5))
                            .line_height(relative(1.45))
                            .text_color(if row.problem {
                                cx.theme().danger
                            } else if row.rank == 1 {
                                cx.theme().warning
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .when(row.problem, |text| text.line_clamp(2))
                                    .when(!row.problem, |text| text.truncate())
                                    .child(row.detail.clone()),
                            )
                            .when_some(fix, |view, id| {
                                view.child(
                                    crate::controls::compact_button(SharedString::from(format!(
                                        "fix-forward-{}",
                                        row.key
                                    )))
                                    .link()
                                    .label("Edit forward")
                                    .text_color(cx.theme().danger)
                                    .p_0()
                                    .disabled(self.port_busy(&row.entry))
                                    .accessibility_label(format!(
                                        "Edit failed forward for port {}",
                                        row.port
                                    ))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if this.server == fix_server {
                                            this.edit_saved_connection(&id, window, cx);
                                        }
                                    })),
                                )
                            }),
                    )
                })
                .when(inline_route && !row.address.is_empty(), |v| {
                    v.child(div().mt(px(3.)).ml(px(-8.)).child(route.take().unwrap()))
                });
        let menu = self.connection_menu(&row, engaged, cx);
        let pending = self.port_busy(&row.entry);
        let server = self.server.clone();
        let entry = row.entry.clone();
        let project = row.project.clone();
        let control = crate::controls::switch(SharedString::from(format!("forward-{}", row.key)))
            .color(cx.theme().primary)
            .accessibility_label(format!("Forward port {} ({})", row.port, row.title))
            .checked(row.active)
            .disabled(pending)
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                if this.server == server && (!entry.forwards.is_empty() || *checked) {
                    this.toggle_port(entry.clone(), &project, *checked, cx);
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
            .aria_label(format!("Port {}: {}, {}", row.port, row.title, row.status))
            .flex()
            .items_center()
            .min_h(px(42.))
            .py(px(5.))
            .flex_shrink_0()
            .when(!table, |v| v.px_3().rounded(px(8.)))
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
                        if wide { 84. } else { 64. }
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
                    .child(dot)
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(13.))
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
                        .w(relative(0.20))
                        .flex_shrink_0()
                        .min_w_0()
                        .px(px(10.))
                        .text_size(px(12.))
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
                                    .child(Icon::new(IconName::GitBranch).size(px(10.)))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(11.))
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
                        .w(px(124.))
                        .flex_shrink_0()
                        .px(px(10.))
                        .text_size(px(12.))
                        .text_color(tone)
                        .child(row.status),
                )
            })
            .when(!inline_route, |v| {
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
                                .w(px(184.))
                                .px(px(10.))
                        })
                        .flex_shrink_0()
                        .when(!table, |v| v.pr(px(12.)))
                        .text_right()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(11.5))
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
                    .w(px(if table {
                        84.
                    } else if compact {
                        72.
                    } else {
                        74.
                    }))
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
                    .child(menu)
                    .child(control),
            )
            .into_any_element()
    }

    fn connection_menu(&self, row: &Row, engaged: bool, cx: &mut Context<Self>) -> AnyElement {
        let forwards = row.entry.forwards.clone();
        let server = self.server.clone();
        let weak = cx.weak_entity();
        let address = row.address.clone();
        let destination = string(&row.entry.port, "address").to_owned();
        let port = row.number;
        let name = newport_domain::port_context::forward_name(&row.entry.port);
        let project = row.project.clone();
        let open = row.open.clone();
        let pending = self.port_busy(&row.entry);
        RowMenu {
            id: SharedString::from(format!("actions-visibility-{}", row.key)),
            group: SharedString::from(format!("connection-row-{}", row.key)),
            engaged,
            button: crate::controls::compact_icon_button(SharedString::from(format!(
                "actions-{}",
                row.key
            )))
            .icon(IconName::Ellipsis)
            .ghost()
            .disabled(pending)
            .accessibility_label(format!("Actions for port {}", row.port)),
            builder: Box::new(move |mut menu, _, _| {
                if forwards.is_empty() {
                    let weak = weak.clone();
                    let server = server.clone();
                    let destination = destination.clone();
                    let name = name.clone();
                    let project = project.clone();
                    return menu.item(
                        PopupMenuItem::new("Forward with options…")
                            .icon(IconName::Settings2)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.server != server {
                                        return;
                                    }
                                    let draft = (|| -> Result<(Value, bool), String> {
                                        let id = server.parse().map_err(|_| "Invalid server")?;
                                        let tunnels: Vec<newport_domain::model::Tunnel> =
                                            serde_json::from_value(
                                                this.snapshot["config"]["tunnels"].clone(),
                                            )
                                            .map_err(|e| e.to_string())?;
                                        editor_draft(
                                            &tunnels,
                                            id,
                                            &destination,
                                            u16::try_from(port).map_err(|_| "Invalid port")?,
                                            &name,
                                            &project,
                                        )
                                    })();
                                    match draft {
                                        Ok((value, existing)) => {
                                            this.open_tunnel_form(Some(value), existing, window, cx)
                                        }
                                        Err(error) => {
                                            this.notification = Some(Notification::error(error));
                                            cx.notify();
                                        }
                                    }
                                });
                            }),
                    );
                }
                for t in &forwards {
                    let weak = weak.clone();
                    let server = server.clone();
                    let id = string(t, "id").to_owned();
                    let title = if forwards.len() == 1 {
                        "Edit forward…".into()
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
                if let Some((value, remote_port)) = open.clone() {
                    let owner = weak.clone();
                    let expected_server = server.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Open in browser")
                            .icon(IconName::ExternalLink)
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    if this.server == expected_server {
                                        this.open_forward(value.clone(), remote_port, cx);
                                    }
                                });
                            }),
                    );
                }
                let address = address.clone();
                menu = menu
                    .item(
                        PopupMenuItem::new("Copy address")
                            .icon(IconName::Copy)
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(address.clone()))
                            }),
                    )
                    .separator();
                for t in &forwards {
                    let owner = weak.clone();
                    let expected_server = server.clone();
                    let value = t.clone();
                    let title = if forwards.len() == 1 {
                        "Delete forward…".into()
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
        let value = self.snapshot["config"]["tunnels"]
            .as_array()
            .and_then(|tunnels| {
                tunnels
                    .iter()
                    .find(|t| string(t, "id") == id && string(t, "serverId") == self.server)
            })
            .cloned();
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
    use super::{Entry, Row, local_address, open_target, sort};
    use serde_json::json;
    fn row(port: u64, project: &str) -> Row {
        Row::new(
            project,
            Entry {
                addresses: vec![],
                port: json!({"port":port,"address":"127.0.0.1","processName":"app"}),
                forwards: vec![],
                saved: false,
            },
            &json!({}),
            false,
        )
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
            port: json!({"port":3002}),
            forwards: vec![
                json!({"remotePort":3000,"remotePortEnd":3005,"localPort":9000,"localPortEnd":9005}),
            ],
            saved: false,
        };
        assert_eq!(local_address(&entry), "localhost:9002");
        entry.saved = true;
        assert_eq!(local_address(&entry), "localhost:9000–9005");
        entry.forwards[0]["direction"] = json!("remote");
        entry.forwards[0]["remoteHost"] = json!("::1");
        assert_eq!(local_address(&entry), "server:3000–3005");
    }
    #[test]
    fn destination_failure_takes_precedence_over_a_connected_forward() {
        let entry = Entry {
            addresses: vec![],
            port: json!({"port":3000}),
            forwards: vec![json!({"id":"up"}), json!({"id":"failed"})],
            saved: false,
        };
        let runtime = json!({"tunnels":{"up":{"status":"connected"},"failed":{"status":"error","errorMessage":"Denied"}}});
        let row = Row::new("", entry, &runtime, false);
        assert_eq!(row.status, "Failed");
        assert_eq!(row.detail, "Denied");
        assert_eq!(row.rank, 0);
        assert_eq!(row.fix.as_deref(), Some("failed"));
        assert_eq!(row.transport, "error");
    }

    #[test]
    fn destination_issue_does_not_masquerade_as_ssh_failure_or_offer_wrong_repair() {
        let entry = Entry {
            addresses: vec![],
            port: json!({"port":8080}),
            forwards: vec![json!({"id":"up"})],
            saved: false,
        };
        let mut runtime = json!({"tunnels":{"up":{"status":"connected"}},
            "tunnelHealth":{"up":[{"remotePort":8080,"status":"unavailable","message":"Connection refused"}]}});
        let row = Row::new("", entry.clone(), &runtime, false);
        assert_eq!(row.transport, "connected");
        assert_eq!(row.status, "Unreachable");
        assert!(row.fix.is_none());
        runtime["tunnels"]["up"] = json!({"status":"reconnecting","reconnectAttempt":3});
        let row = Row::new("", entry.clone(), &runtime, false);
        assert_eq!(row.status, "Retrying 3/10…");
        assert_eq!(row.detail, row.status);
        assert!(row.fix.is_none());
        runtime["tunnels"]["up"] = json!({"status":"error"});
        let row = Row::new("", entry, &runtime, false);
        assert_eq!(row.detail, "The forward stopped.");
        assert_eq!(row.fix.as_deref(), Some("up"));
    }

    #[test]
    fn destination_health_cannot_mask_another_forwards_transport_error() {
        let mut entry = Entry {
            addresses: vec![],
            port: json!({"port":8080}),
            forwards: vec![json!({"id":"up"}), json!({"id":"failed"})],
            saved: false,
        };
        let runtime = json!({
            "tunnels":{"up":{"status":"connected"},"failed":{"status":"error"}},
            "tunnelHealth":{"up":[{"remotePort":8080,"status":"unavailable"}]}
        });
        for _ in 0..2 {
            let row = Row::new("", entry.clone(), &runtime, false);
            assert_eq!(row.transport, "error");
            assert_eq!(row.status, "Failed");
            assert_eq!(row.fix.as_deref(), Some("failed"));
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
        let entry = Entry {
            addresses: vec![],
            port: json!({"port":3002}),
            forwards: vec![json!(first), json!(second)],
            saved: false,
        };
        let mut runtime = json!({"tunnels":{}});
        runtime["tunnels"][first.id.to_string()] = json!({"status":"disconnected"});
        runtime["tunnels"][second.id.to_string()] = json!({"status":"connected"});
        let row = Row::new("", entry.clone(), &runtime, false);
        assert_eq!(row.address, "localhost:10002");
        assert_eq!(row.open, Some((json!(second), Some(3002))));
        runtime["tunnelHealth"][second.id.to_string()] = json!([
            {"remotePort":3000,"status":"unavailable","message":"Connection refused"},
            {"remotePort":3002,"status":"reachable"}
        ]);
        let row = Row::new("", entry.clone(), &runtime, false);
        assert_eq!(row.status, "Connected");
        assert!(!row.problem);
        for status in ["disconnected", "connecting", "reconnecting", "error"] {
            runtime["tunnels"][second.id.to_string()]["status"] = json!(status);
            assert!(open_target(&entry, &runtime).is_none(), "{status}");
        }
        runtime["tunnels"][second.id.to_string()]["status"] = json!("connected");
        let mut reverse = entry.clone();
        reverse.forwards[1]["direction"] = json!("remote");
        assert!(open_target(&reverse, &runtime).is_none());
        let mut outside = entry.clone();
        outside.port["port"] = json!(3006);
        assert!(open_target(&outside, &runtime).is_none());
        let saved = Entry {
            addresses: vec![],
            port: json!(second),
            forwards: vec![json!(second)],
            saved: true,
        };
        let row = Row::new("", saved, &runtime, false);
        assert_eq!(row.address, "localhost:10000–10005");
        assert_eq!(row.open, Some((json!(second), None)));
        assert!(
            row.problem,
            "A standalone saved range still shows its failed member"
        );
    }
    #[test]
    fn service_context_matches_reference_and_keeps_saved_alias() {
        let entry = Entry {
            port: json!({"port":5173,"processName":"MainThread","executable":"/usr/bin/node",
                "arguments":["node","/srv/app/node_modules/vite/bin/vite.js"],
                "workingDirectory":"/srv/app/packages/web","workingDirectoryDeleted":true}),
            addresses: vec!["127.0.0.1".into(), "::".into()],
            forwards: vec![json!({"id":"a","name":"Customer preview"})],
            saved: false,
        };
        let row = Row::new("/srv/app", entry, &json!({}), false);
        assert_eq!(row.title, "Customer preview");
        assert_eq!(
            row.detail,
            "Vite · folder deleted · packages/web/ · Node · all interfaces"
        );
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
        assert_eq!(value, json!(tunnel));
        let (value, existing) =
            super::editor_draft(&[], server, "0.0.0.0", 8081, "Vite · feature", "/srv/new")
                .unwrap();
        assert!(!existing);
        assert_eq!(value["name"], "Vite · feature");
        assert_eq!(value["project"], "/srv/new");
    }
    #[test]
    fn checkout_identity_and_relative_details_use_normalized_path() {
        let make = |path| Entry {
            port: json!({"port":5173,"processName":"node","executable":"/usr/bin/node",
                "arguments":["node","vite"],"workingDirectory":"/srv/app/packages/web",
                "worktreePath":path,"worktreeBranch":"feature","worktreeMain":false}),
            addresses: vec!["127.0.0.1".into()],
            forwards: vec![],
            saved: false,
        };
        let first = Row::new("/srv", make("/srv/app/"), &json!({}), true);
        let second = Row::new("/srv", make("/srv/app"), &json!({}), true);
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

    struct TransportHost(&'static str);

    impl Render for TransportHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            transport_dot(self.0, cx)
        }
    }

    #[gpui::test]
    fn pulse_stops_scheduling_frames_when_transport_settles(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let (view, cx) = cx.add_window_view(|_, _| TransportHost("connecting"));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let frames = cx.update(|window, cx| window.simulate_next_frame(cx));
        assert!(frames > 0, "connecting transport schedules its pulse");
        view.update(cx, |view, cx| {
            view.0 = "connected";
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
        let (view, cx) = cx.add_window_view(|_, _| TransportHost("disconnected"));
        for (reduced, status) in [
            (false, "disconnected"),
            (false, "connected"),
            (false, "error"),
            (true, "connecting"),
            (true, "reconnecting"),
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
                    "{status}, reduced={reduced}"
                );
            });
        }
    }
}
