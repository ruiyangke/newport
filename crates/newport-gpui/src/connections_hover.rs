//! Compact, read-only connection context. Constructed with the cached row.
use super::*;
use crate::connections::diagnostics;
use crate::{connections_grouped::Entry, connections_process_icon::ProcessIcon};
use newport_domain::port_discovery::{self as d, Kind};

#[derive(Clone)]
pub(super) struct Forward {
    pub name: Option<String>,
    pub address: String,
    pub status: String,
}

#[derive(Clone)]
pub(super) struct Card {
    title: String,
    icon: ProcessIcon,
    docker: bool,
    facts: Vec<(&'static str, String)>,
    pub forward: Option<Forward>,
    pub notice: Option<String>,
}

impl Card {
    pub fn new(entry: &Entry, title: String, icon: ProcessIcon) -> Self {
        let mut card = Self {
            title,
            icon,
            docker: entry.port.publication().is_some(),
            facts: Vec::new(),
            forward: None,
            notice: None,
        };
        let Some(row) = entry.port.row() else {
            return card;
        };
        let mut processes = std::collections::BTreeSet::new();
        for member in &row.members {
            let address = match member.endpoint.interface {
                Some(interface) => format!("{}%{interface}", member.endpoint.address),
                None => member.endpoint.address.to_string(),
            };
            // A socket on `::` that isn't IPv6-only listens on every address.
            let address = if member.dual_stack() {
                format!("*:{} (IPv4 and IPv6)", member.endpoint.port)
            } else if address.contains(':') {
                format!("[{address}]:{}", member.endpoint.port)
            } else {
                format!("{address}:{}", member.endpoint.port)
            };
            match &member.kind {
                Kind::Socket(socket) => {
                    card.add("Listening", address);
                    card.add(
                        "Socket account",
                        match &socket.account {
                            Some(account) => format!("{account} (UID {})", socket.uid),
                            None => format!("UID {}", socket.uid),
                        },
                    );
                    if socket.holders.is_empty() {
                        card.add("Processes", "No process holder observed".into());
                    }
                    if socket.holder_coverage != d::HolderCoverage::Complete {
                        card.add("Visibility", "Process information is incomplete".into());
                    }
                    for holder in &socket.holders {
                        if !processes.insert((member.scope.clone(), holder.identity)) {
                            continue;
                        }
                        let mut identity = format!("PID {}", holder.identity.pid);
                        if let Some(name) = holder.name.as_ref().or(holder.executable.as_ref()) {
                            identity.push_str(&format!(" · {name}"));
                        }
                        if let Some(account) = &holder.account {
                            identity.push_str(&format!(" · {account}"));
                        }
                        if let Some(uid) = holder.uid {
                            identity.push_str(&format!(" · UID {uid}"));
                        }
                        card.add("Process", identity);
                        for detail in &holder.details {
                            card.add(detail.label, detail.value.clone());
                        }
                    }
                    for unit in &socket.units {
                        card.add(
                            if unit.socket_cgroup {
                                "Socket unit"
                            } else {
                                "Process unit"
                            },
                            unit.name.clone(),
                        );
                    }
                }
                Kind::Publication(publication) => {
                    card.add("Published", address);
                    card.add("Container", publication.name.clone());
                    if let Some(image) = &publication.image {
                        card.add("Image", image.clone());
                    }
                    if let Some(service) = &publication.service {
                        card.add("Service", service.clone());
                    }
                    card.add("Target port", publication.target_port.to_string());
                    if publication.running == Some(false) {
                        card.add("Container", "Stopped".into());
                    }
                }
            }
            if member.stale {
                card.add("Discovery", "Last observed; refresh to verify".into());
            }
        }
        card
    }

    fn add(&mut self, label: &'static str, value: String) {
        let fact = (label, d::display(&value));
        if !self.facts.contains(&fact) {
            self.facts.push(fact);
        }
    }

    pub fn enrich(&mut self, entry: &Entry, view: &newport_client::discovery::View) {
        let Some(row) = entry.port.row() else {
            return;
        };
        for member in &row.members {
            let Some(snapshot) = view.snapshot(member.reference) else {
                continue;
            };
            if let Some(socket) = view.socket(member.reference) {
                for association in &socket.services {
                    if let Some(service) = snapshot
                        .services
                        .iter()
                        .find(|service| service.id == association.service)
                    {
                        for trigger in &service.triggers {
                            self.add("Triggers", trigger.clone());
                        }
                    }
                }
                for process in snapshot
                    .processes
                    .iter()
                    .filter(|process| socket.holders.contains(&process.id))
                {
                    if let Some(details) = &process.details {
                        if let Some(path) = &details.directory
                            && path.deleted
                        {
                            self.add(
                                "Directory",
                                format!("PID {}: {} (deleted)", process.identity.pid, path.value),
                            );
                        }
                        if let Some(path) = &details.executable
                            && path.deleted
                        {
                            self.add(
                                "Executable",
                                format!("PID {}: {} (deleted)", process.identity.pid, path.value),
                            );
                        }
                    }
                    if diagnostics::limited(&process.metadata) {
                        self.add(
                            "Process details",
                            format!(
                                "PID {} · {}",
                                process.identity.pid,
                                diagnostics::report(&process.metadata)
                            ),
                        );
                    }
                }
            }
        }
        for (label, value) in diagnostics::limitations(&view.evidence().current) {
            self.add(label, value);
        }
    }

    pub fn has_details(&self) -> bool {
        !self.facts.is_empty() || self.forward.is_some() || self.notice.is_some()
    }

    pub fn build(self: std::rc::Rc<Self>, window: &mut Window, cx: &mut App) -> AnyView {
        let width = crate::hover::width(window);
        let height = crate::hover::height(window);
        let padding = crate::hover::PADDING;
        crate::hover::card((padding, padding), move |_, cx| {
            div()
                .id("connection-hover-scroll")
                .debug_selector(|| "connection-hover-scroll".into())
                .max_h(px(height))
                .overflow_y_scroll()
                .child(self.content(width - 2. * padding, cx))
        })
        .w(px(width))
        .build(window, cx)
    }

    pub fn control(
        self: std::rc::Rc<Self>,
        id: SharedString,
        trigger: gpui_kit::component::button::Button,
    ) -> gpui_kit::component::popover::Popover {
        use gpui_kit::{Focusable as _, ScrollHandle};
        let mut placement = div().min_w_0().max_w_full();
        gpui_kit::component::popover::Popover::new(SharedString::from(format!(
            "connection-evidence-{id}"
        )))
        .trigger_style(placement.style().clone())
        .trigger(trigger)
        .p(px(crate::hover::PADDING))
        .content(move |state, window, cx| {
            let width = crate::hover::width(window) - 2. * crate::hover::PADDING;
            let scroll = window
                .use_keyed_state(
                    SharedString::from(format!("connection-evidence-scroll-{id}")),
                    cx,
                    |_, _| ScrollHandle::new(),
                )
                .read(cx)
                .clone();
            div()
                .id("connection-details")
                .debug_selector(|| "connection-details".into())
                .test_support()
                .role(accesskit::Role::Group)
                .aria_label("Connection details")
                .w(px(width))
                .max_h(px(crate::hover::height(window)))
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .track_focus(&crate::hover::scroll_focus(
                    &state.focus_handle(cx),
                    window,
                    cx,
                ))
                .on_key_down(move |event, window, cx| {
                    crate::hover::scroll_on_key(event, &scroll, window, cx);
                })
                .child(self.content(width, cx))
        })
    }

    fn content(&self, width: f32, cx: &App) -> AnyElement {
        div()
            .id("connection-hover-card")
            .debug_selector(|| "connection-hover-card".into())
            .test_support()
            .w(px(width))
            .min_w_0()
            .overflow_hidden()
            .text_size(px(crate::ui::FONT_SIZE))
            .line_height(relative(1.45))
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(self.icon.render(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_normal()
                            .child(self.title.clone()),
                    )
                    .when(self.docker, |v| {
                        v.child(
                            div()
                                .flex_shrink_0()
                                .rounded(px(crate::ui::RADIUS))
                                .px(px(5.))
                                .py(px(1.))
                                .bg(cx.theme().muted)
                                .text_color(cx.theme().muted_foreground)
                                .text_size(px(crate::ui::FONT_SIZE))
                                .child("Docker"),
                        )
                    }),
            )
            .when(!self.facts.is_empty(), |v| {
                v.child(
                    div().mt(px(12.)).flex().flex_col().gap(px(7.)).children(
                        self.facts
                            .iter()
                            .enumerate()
                            .map(|(index, (label, value))| {
                                let last = index + 1 == self.facts.len();
                                fact(("connection-fact", index), label, value, cx)
                                    .test_support()
                                    .debug_selector(move || {
                                        if last {
                                            "connection-last-fact".into()
                                        } else {
                                            format!("connection-fact-{index}")
                                        }
                                    })
                            }),
                    ),
                )
            })
            .when_some(self.forward.clone(), |v, forward| {
                v.child(
                    div()
                        .mt(px(12.))
                        .pt(px(10.))
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .when_some(forward.name, |v, name| {
                            v.child(fact("forward-name", "Saved as", &name, cx))
                        })
                        .child(fact("forward-status", "Status", &forward.status, cx))
                        .when(!forward.address.is_empty(), |v| {
                            v.child(fact("forward-address", "Address", &forward.address, cx))
                        }),
                )
            })
            .when_some(self.notice.clone(), |v, notice| {
                v.child(
                    div()
                        .mt(px(10.))
                        .text_color(cx.theme().muted_foreground)
                        .whitespace_normal()
                        .child(notice),
                )
            })
            .into_any_element()
    }
}

fn fact(
    id: impl Into<gpui_kit::ElementId>,
    label: &str,
    value: &str,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .role(accesskit::Role::Label)
        .aria_label(format!("{label}: {value}"))
        .flex()
        .items_start()
        .gap(px(12.))
        .w_full()
        .min_w_0()
        .child(
            div()
                .w(px(76.))
                .flex_shrink_0()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .child(value.to_owned()),
        )
}

#[cfg(test)]
mod tests {
    use super::{Card, Entry, ProcessIcon};

    #[gpui_kit::test]
    fn connection_details_replace_hover_preview_in_both_layouts(cx: &mut gpui_kit::TestAppContext) {
        use crate::page_perf_tests::{connections_fixture, frame};
        use gpui_kit::{VisualTestContext, point, px, size};
        use std::time::Duration;

        fn settle(visual: &mut VisualTestContext) {
            frame(visual);
            visual.run_until_parked();
            visual.executor().advance_clock(Duration::from_secs(1));
            visual.run_until_parked();
            frame(visual);
        }

        for (table, dark) in [(false, false), (true, true)] {
            let (_, visual) = connections_fixture(cx, table);
            visual.update(|_, cx| crate::theme::install(dark, cx));
            visual.simulate_resize(size(px(1100.), px(700.)));
            frame(visual);
            let trigger = visual.update(|window, _| {
                gpui_kit::base::test_support::snapshots(window)
                    .into_iter()
                    .find(|node| {
                        node.label().is_some_and(|label| {
                            label.starts_with("Connection details for port 8000:")
                        })
                    })
                    .expect("connection details trigger")
                    .bounds()
                    .center()
            });
            visual.simulate_mouse_move(trigger, None, Default::default());
            settle(visual);
            assert!(visual.debug_bounds("connection-hover-scroll").is_some());

            visual.simulate_click(trigger, Default::default());
            frame(visual);
            frame(visual);
            assert!(visual.debug_bounds("connection-details").is_some());
            assert!(
                visual.debug_bounds("connection-hover-scroll").is_none(),
                "opening connection details must dismiss the hover preview"
            );
            visual.simulate_mouse_move(trigger + point(px(1.), px(0.)), None, Default::default());
            settle(visual);
            assert!(visual.debug_bounds("connection-details").is_some());
            assert!(visual.debug_bounds("connection-hover-scroll").is_none());

            visual.simulate_keystrokes("escape");
            frame(visual);
            frame(visual);
            assert!(visual.debug_bounds("connection-details").is_none());
            visual.simulate_mouse_move(point(px(1.), px(1.)), None, Default::default());
            settle(visual);
            visual.simulate_mouse_move(trigger, None, Default::default());
            settle(visual);
            assert!(
                visual.debug_bounds("connection-hover-scroll").is_some(),
                "hover previews must remain available after closing details"
            );
        }
    }

    #[gpui_kit::test]
    fn connection_evidence_is_keyboard_accessible_in_both_layouts(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::page_perf_tests::{connections_fixture, frame};
        use gpui_kit::{accesskit::Role, px, size};
        for (table, dark, width, height, key) in [
            (false, false, 1100., 700., "enter"),
            (true, true, 600., 400., "space"),
        ] {
            let (view, visual) = connections_fixture(cx, table);
            view.update(visual, |view, cx| {
                use crate::connections::fixtures::{Listener, observation, put};
                let mut evidence = observation([Listener::new(8000)
                    .pid(1)
                    .process("node", "node", &[], None)
                    .project("/srv/project-0")]);
                let current = &mut evidence.data.as_mut().unwrap().current;
                let first = current.processes[0].clone();
                for id in 2..20 {
                    let mut holder = first.clone();
                    holder.id = id;
                    holder.identity.pid = id;
                    current.processes.push(holder);
                    current.sockets[0].holders.push(id);
                }
                put(view, evidence);
                cx.notify();
            });
            visual.update(|_, cx| crate::theme::install(dark, cx));
            visual.simulate_resize(size(px(width), px(height)));
            frame(visual);
            let trigger = |visual: &mut gpui_kit::VisualTestContext| {
                visual.update(|window, _| {
                    gpui_kit::base::test_support::snapshots(window)
                        .into_iter()
                        .find(|node| {
                            node.label().is_some_and(|label| {
                                label.starts_with("Connection details for port 8000:")
                            })
                        })
                        .expect("connection evidence needs a keyboard-accessible control")
                })
            };
            assert_eq!(trigger(visual).role(), Some(Role::Button));
            visual.update(|window, _| window.activate_window());
            for _ in 0..64 {
                if trigger(visual).focused() == Some(true) {
                    break;
                }
                visual.update(|window, cx| window.focus_next(cx));
                frame(visual);
            }
            assert_eq!(trigger(visual).focused(), Some(true));
            visual.simulate_keystrokes(key);
            visual.simulate_event(gpui_kit::KeyUpEvent {
                physical_key: None,
                key_metadata: None,
                keystroke: gpui_kit::Keystroke::parse(key).unwrap(),
            });
            frame(visual);
            frame(visual);
            let bounds = visual
                .debug_bounds("connection-details")
                .expect("keyboard opens connection evidence");
            assert!(bounds.left() >= px(0.) && bounds.right() <= px(width));
            assert!(bounds.top() >= px(0.) && bounds.bottom() <= px(height));
            visual.simulate_keystrokes("end");
            frame(visual);
            let last = visual.debug_bounds("connection-last-fact").unwrap();
            assert!(last.top() >= bounds.top() && last.bottom() <= bounds.bottom() + px(1.));
            visual.update(|window, _| {
                let facts = gpui_kit::base::test_support::snapshots(window);
                assert!(facts.iter().any(|node| {
                    node.role() == Some(Role::Label)
                        && node
                            .label()
                            .is_some_and(|label| label.starts_with("Process: PID 19"))
                }));
            });
            visual.simulate_keystrokes("home");
            frame(visual);
            assert!(visual.debug_bounds("connection-fact-0").unwrap().top() >= bounds.top());
            visual.simulate_keystrokes("escape");
            frame(visual);
            frame(visual);
            assert!(visual.debug_bounds("connection-details").is_none());
            assert_eq!(trigger(visual).focused(), Some(true));
        }
    }
    use crate::connections::fixtures::{Listener, publication};
    use newport_domain::{port_discovery as d, port_grouping::Port};

    #[test]
    fn connections_hover_uses_container_identity_and_omits_host_proxy_details() {
        let mut port = publication(4222, "stack-nats-1", "nats:latest");
        let Port::Observed(row) = &mut port else {
            unreachable!()
        };
        row.members[0].endpoint.address = "0.0.0.0".parse().unwrap();
        let mut ipv6 = row.members[0].clone();
        ipv6.endpoint.address = "::".parse().unwrap();
        row.members.push(ipv6);
        let entry = Entry {
            addresses: port.addresses(),
            port,
            forwards: vec![],
        };
        let card = Card::new(&entry, "NATS".into(), ProcessIcon::Process);
        for fact in [
            ("Container", "stack-nats-1"),
            ("Image", "nats:latest"),
            ("Published", "0.0.0.0:4222"),
            ("Published", "[::]:4222"),
        ] {
            assert!(card.facts.contains(&(fact.0, fact.1.into())));
        }
        assert!(
            !card
                .facts
                .iter()
                .any(|(label, _)| matches!(*label, "Listening" | "Process"))
        );
        assert!(card.notice.is_none());
    }

    #[test]
    fn connections_hover_preserves_real_process_identity_and_curated_details() {
        let mut entry = Listener::new(5432)
            .pid(42)
            .process(
                "postgres",
                "postgres",
                &["postgres", "-D", "/srv/data"],
                None,
            )
            .entry();
        let Port::Observed(row) = &mut entry.port else {
            unreachable!()
        };
        let d::Kind::Socket(socket) = &mut row.members[0].kind else {
            unreachable!()
        };
        std::sync::Arc::make_mut(&mut socket.holders[0]).account = Some("postgres".into());
        let card = Card::new(&entry, "Postgres".into(), ProcessIcon::Process);
        assert!(card.facts.contains(&("Data directory", "/srv/data".into())));
        assert!(
            card.facts
                .contains(&("Process", "PID 42 · postgres · postgres · UID 1000".into()))
        );
        assert!(!card.docker);
    }

    #[test]
    fn a_socket_on_ipv6_any_shows_whether_it_also_listens_on_ipv4() {
        for (ipv6_only, listening) in [
            (Some(false), "*:3300 (IPv4 and IPv6)"),
            (Some(true), "[::]:3300"),
        ] {
            let mut entry = Listener::new(3300).address("::").entry();
            let Port::Observed(row) = &mut entry.port else {
                unreachable!()
            };
            let d::Kind::Socket(socket) = &mut row.members[0].kind else {
                unreachable!()
            };
            socket.ipv6_only = ipv6_only;
            let card = Card::new(&entry, "bun".into(), ProcessIcon::Process);
            assert!(
                card.facts.contains(&("Listening", listening.into())),
                "{ipv6_only:?}: {:?}",
                card.facts
            );
        }
    }

    #[test]
    fn discovery_explanations_never_render_remote_errors_or_arguments() {
        use newport_protocol::monitor::ports::discovery as p;
        let mut evidence = crate::connections::fixtures::observation([Listener::new(8000)
            .process(
                "worker",
                "worker",
                &["worker", "--token=SECRET-ARGUMENT"],
                None,
            )])
        .data
        .unwrap();
        let limited = p::Report {
            outcome: p::Outcome::Partial,
            issues: vec![p::Issue {
                kind: p::IssueKind::PermissionDenied,
                count: 3,
                message: "SECRET-REMOTE-ERROR /private/path --token=secret".into(),
            }],
        };
        evidence.current.processes[0].metadata = limited.clone();
        evidence.current.coverage.processes = limited.clone();
        evidence.current.coverage.cgroups = limited.clone();
        evidence.current.coverage.services = limited.clone();
        evidence.current.coverage.elevation = limited.clone();
        evidence.current.coverage.details = limited;
        let view = newport_client::discovery::View::new(evidence, false).unwrap();
        let port = Port::Observed(d::Row {
            members: view.members().to_vec(),
        });
        let entry = Entry {
            addresses: port.addresses(),
            port,
            forwards: vec![],
        };
        let mut card = Card::new(&entry, "worker".into(), ProcessIcon::Process);
        card.enrich(&entry, &view);
        assert!(card.facts.iter().any(|(key, value)| {
            *key == "Visibility" && value == "Process information is incomplete"
        }));
        assert!(
            card.facts
                .iter()
                .all(|(_, value)| !value.contains("SECRET"))
        );
        for label in [
            "Process details",
            "Processes",
            "Cgroups",
            "Services",
            "Elevation",
            "Details",
        ] {
            assert!(
                card.facts
                    .iter()
                    .any(|(key, value)| *key == label && value.contains("permission denied")),
                "missing {label}"
            );
        }
    }

    #[test]
    fn typed_evidence_keeps_all_holders_deleted_paths_and_socket_activation_separate() {
        use newport_protocol::monitor::ports::discovery as p;
        let mut evidence = crate::connections::fixtures::observation([Listener::new(8000)
            .pid(42)
            .process("worker", "/usr/bin/worker", &[], Some("/srv/app"))])
        .data
        .unwrap();
        let snapshot = &mut evidence.current;
        snapshot.processes[0]
            .details
            .as_mut()
            .unwrap()
            .directory
            .as_mut()
            .unwrap()
            .deleted = true;
        let mut second = snapshot.processes[0].clone();
        second.id = 2;
        second.identity.pid = 43;
        snapshot.processes.push(second);
        snapshot.sockets[0].holders.push(2);
        snapshot.sockets[0].uid = 0;
        snapshot.sockets[0].account = Some("root".into());
        snapshot.sockets[0].cgroup = Some(p::Cgroup {
            id: 7,
            path: Some("/system.slice/web.socket".into()),
        });
        snapshot.coverage.services = p::Report::complete();
        snapshot.services.push(p::Service {
            id: 1,
            manager: p::Manager::System,
            unit: "web.socket".into(),
            description: None,
            control_group: "/system.slice/web.socket".into(),
            main_pid: Some(1),
            triggers: vec!["web.service".into()],
        });
        snapshot.sockets[0].services.push(p::Association {
            service: 1,
            relation: p::ServiceRelation::SocketCgroup,
        });
        let view = newport_client::discovery::View::new(evidence, false).unwrap();
        let port = Port::Observed(d::Row {
            members: view.members().to_vec(),
        });
        let entry = Entry {
            addresses: port.addresses(),
            port,
            forwards: vec![],
        };
        let mut card = Card::new(&entry, "web.socket".into(), ProcessIcon::Process);
        card.enrich(&entry, &view);
        assert_eq!(
            card.facts
                .iter()
                .filter(|(label, _)| *label == "Process")
                .count(),
            2
        );
        assert!(
            card.facts
                .contains(&("Socket account", "root (UID 0)".into()))
        );
        assert!(card.facts.contains(&("Socket unit", "web.socket".into())));
        assert!(card.facts.contains(&("Triggers", "web.service".into())));
        assert!(
            card.facts
                .contains(&("Directory", "PID 42: /srv/app (deleted)".into()))
        );
        assert!(
            !card
                .facts
                .iter()
                .any(|(label, value)| *label == "Process" && value.starts_with("PID 1 "))
        );
    }
    #[gpui_kit::test]
    fn connections_hover_long_identifiers_stay_bounded(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{Context, IntoElement, ParentElement, Render, Window, div, px, size};
        struct Host(std::rc::Rc<Card>);
        impl Render for Host {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div().child(self.0.clone().build(window, cx))
            }
        }
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let port = publication(
            4222,
            &"x".repeat(300),
            &format!("registry.test/{}:latest", "x".repeat(500)),
        );
        let entry = Entry {
            addresses: port.addresses(),
            port,
            forwards: vec![],
        };
        let card = std::rc::Rc::new(Card::new(&entry, "NATS".into(), ProcessIcon::Process));
        let (_, visual) = cx.add_window_view(|_, _| Host(card));
        for width in [800., 320.] {
            visual.simulate_resize(size(px(width), px(600.)));
            crate::page_perf_tests::frame(visual);
            let bounds = visual
                .debug_bounds("connection-hover-card")
                .expect("hover content rendered");
            assert!(bounds.size.width <= px(356.) && bounds.right() <= px(width));
            let viewport = visual.debug_bounds("connection-hover-scroll").unwrap();
            assert!(viewport.size.height <= px(360.) && viewport.bottom() <= px(600.));
            assert!(
                bounds.size.height > viewport.size.height,
                "long values wrap inside the scroll viewport"
            );
        }
    }
}
