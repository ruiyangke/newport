//! Compact, read-only connection context. Constructed with the cached row.
use super::*;
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
    /// The container engine, for a container's published port.
    engine: Option<d::EngineKind>,
    facts: Vec<(&'static str, String)>,
    pub forward: Option<Forward>,
    pub notice: Option<String>,
}

impl Card {
    /// `login` is the account Needle runs as on the server, when known.
    pub fn new(entry: &Entry, title: String, icon: ProcessIcon, login: Option<&str>) -> Self {
        let mut card = Self {
            title,
            icon,
            engine: entry
                .port
                .publication()
                .map(|publication| publication.engine_kind),
            facts: Vec::new(),
            forward: None,
            notice: None,
        };
        let Some(row) = entry.port.row() else {
            return card;
        };
        for member in &row.members {
            card.add("Listening", listening(member));
        }
        for member in &row.members {
            if let Kind::Publication(publication) = &member.kind {
                let mut container = format!(
                    "{} ({}), port {}",
                    publication.name,
                    publication.engine_kind.label(),
                    publication.target_port
                );
                if publication.running == Some(false) {
                    container.push_str(", stopped");
                }
                card.add("Container", container);
                if let Some(image) = &publication.image {
                    card.add("Image", image.clone());
                }
                if let Some(service) = &publication.service {
                    card.add("Compose service", service.clone());
                }
            }
        }
        let sockets = row.members.iter().filter_map(|member| match &member.kind {
            Kind::Socket(socket) => Some(socket),
            Kind::Publication(_) => None,
        });
        if card.engine.is_some() {
            // Host sockets in a container's row belong to the engine's proxy;
            // the container already says what the row runs.
            for holder in sockets.flat_map(|socket| &socket.holders) {
                card.add("Forwarded by", program(holder));
            }
        } else {
            for socket in sockets.clone() {
                if socket.holders.is_empty() {
                    card.add("Program", unknown(socket, login));
                }
                for holder in &socket.holders {
                    card.add("Program", program(holder));
                }
            }
            for detail in sockets
                .clone()
                .flat_map(|socket| &socket.holders)
                .flat_map(|holder| &holder.details)
            {
                card.add(detail.label, detail.value.clone());
            }
            for unit in sockets
                .flat_map(|socket| &socket.units)
                .filter(|unit| unit.dedicated())
            {
                card.add("Service", unit.name.clone());
            }
        }
        if row.stale() {
            card.add("Discovery", "Last observed; refresh to verify".into());
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
        if self.engine.is_some() {
            return;
        }
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
                for details in snapshot
                    .processes
                    .iter()
                    .filter(|process| socket.holders.contains(&process.id))
                    .filter_map(|process| process.details.as_ref())
                {
                    for (label, path) in [
                        ("Directory", &details.directory),
                        ("Executable", &details.executable),
                    ] {
                        if let Some(path) = path
                            && path.deleted
                        {
                            self.add(label, format!("{} (deleted)", path.value));
                        }
                    }
                }
            }
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
                    .when_some(self.engine, |v, engine| {
                        v.child(
                            div()
                                .flex_shrink_0()
                                .rounded(px(crate::ui::RADIUS))
                                .px(px(5.))
                                .py(px(1.))
                                .bg(cx.theme().muted)
                                .text_color(cx.theme().muted_foreground)
                                .text_size(px(crate::ui::FONT_SIZE))
                                .child(engine.label()),
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
                                // Several values of one label, such as every
                                // listening address, share its first line's label.
                                let continued = index > 0 && self.facts[index - 1].0 == *label;
                                fact(("connection-fact", index), label, value, continued, cx)
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
                            v.child(fact("forward-name", "Saved as", &name, false, cx))
                        })
                        .child(fact("forward-status", "Status", &forward.status, false, cx))
                        .when(!forward.address.is_empty(), |v| {
                            v.child(fact(
                                "forward-address",
                                "Address",
                                &forward.address,
                                false,
                                cx,
                            ))
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

fn listening(member: &d::Member) -> String {
    let endpoint = member.endpoint;
    let address = match endpoint.interface {
        Some(interface) => format!("{}%{interface}", endpoint.address),
        None => endpoint.address.to_string(),
    };
    // A socket on `::` known not to be IPv6-only listens on every address.
    if matches!(&member.kind, Kind::Socket(socket) if socket.ipv6_only == Some(false))
        && endpoint.address.is_unspecified()
        && endpoint.address.is_ipv6()
    {
        format!("*:{} (IPv4 and IPv6)", endpoint.port)
    } else if address.contains(':') {
        format!("[{address}]:{}", endpoint.port)
    } else {
        format!("{address}:{}", endpoint.port)
    }
}

/// A holder's program and the account that runs it.
fn program(holder: &d::Holder) -> String {
    let name = holder
        .name
        .as_ref()
        .or(holder.executable.as_ref())
        .cloned()
        .unwrap_or_else(|| format!("PID {}", holder.identity.pid));
    match (&holder.account, holder.uid) {
        (Some(account), _) => format!("{name}, run by {account}"),
        (None, Some(uid)) => format!("{name}, run by UID {uid}"),
        (None, None) => name,
    }
}

/// Why the program behind a socket is unknown, in the row's own terms.
fn unknown(socket: &d::Socket, login: Option<&str>) -> String {
    let owner = socket
        .account
        .clone()
        .unwrap_or_else(|| format!("UID {}", socket.uid));
    match login {
        Some(login) if socket.account.as_deref() == Some(login) => {
            "Unknown: Newport couldn't see which program holds it.".into()
        }
        Some(login) => {
            format!("Unknown: it runs as {owner}, and {login} can't see other accounts' programs.")
        }
        None => format!(
            "Unknown: it runs as {owner}, and your account can't see other accounts' programs."
        ),
    }
}

fn fact(
    id: impl Into<gpui_kit::ElementId>,
    label: &str,
    value: &str,
    continued: bool,
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
                .when(!continued, |v| v.child(label.to_owned())),
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
                    holder.name = Some(format!("node-{id}"));
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
                            .is_some_and(|label| label.starts_with("Program: node-19"))
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

    /// The engine's proxy sockets for a published port, as Needle reports them
    /// to an account that can't see root's processes.
    fn proxies(port: u16) -> Vec<d::Member> {
        ["0.0.0.0", "::"]
            .into_iter()
            .enumerate()
            .map(|(index, address)| {
                let mut member = Listener::new(port)
                    .address(address)
                    .pid(100 + u32::try_from(index).unwrap())
                    .process("docker-proxy", "docker-proxy", &[], None)
                    .member();
                member.reference.record = d::Record::Socket(10 + u32::try_from(index).unwrap());
                let d::Kind::Socket(socket) = &mut member.kind else {
                    unreachable!()
                };
                socket.ipv6_only = Some(true);
                socket.uid = 0;
                socket.account = Some("root".into());
                socket.holder_coverage = d::HolderCoverage::Partial;
                socket.units.push(d::Unit {
                    manager_uid: None,
                    name: "docker.service".into(),
                    socket_cgroup: true,
                });
                let holder = std::sync::Arc::make_mut(&mut socket.holders[0]);
                holder.account = Some("root".into());
                holder.uid = Some(0);
                member
            })
            .collect()
    }

    fn facts_of(card: &Card, label: &str) -> Vec<String> {
        card.facts
            .iter()
            .filter(|(name, _)| *name == label)
            .map(|(_, value)| value.clone())
            .collect()
    }

    /// Labels that described scan-wide coverage on every row.
    const SCAN_LABELS: [&str; 5] = [
        "Socket account",
        "Visibility",
        "Processes",
        "Process details",
        "Elevation",
    ];

    #[test]
    fn a_published_port_card_names_the_container_and_its_proxy() {
        let mut port = publication(2201, "fx-minimal", "debian:13");
        let Port::Observed(row) = &mut port else {
            unreachable!()
        };
        let d::Kind::Publication(value) = &mut row.members[0].kind else {
            unreachable!()
        };
        value.target_port = 22;
        row.members[0].endpoint.address = "0.0.0.0".parse().unwrap();
        let mut ipv6 = row.members[0].clone();
        ipv6.endpoint.address = "::".parse().unwrap();
        ipv6.reference.record = d::Record::Publication(1);
        row.members.push(ipv6);
        let members = row.members.iter().cloned().chain(proxies(2201)).collect();
        let entry = newport_domain::port_grouping::group(members, &[], uuid::Uuid::nil(), &[])
            .into_iter()
            .flat_map(|project| project.entries)
            .collect::<Vec<_>>();
        assert_eq!(entry.len(), 1, "one row for the container's port");
        let card = Card::new(&entry[0], "fx-minimal".into(), ProcessIcon::Process, None);
        assert_eq!(card.engine, Some(d::EngineKind::Docker));
        assert_eq!(facts_of(&card, "Listening"), ["0.0.0.0:2201", "[::]:2201"]);
        assert_eq!(
            facts_of(&card, "Container"),
            ["fx-minimal (Docker), port 22"]
        );
        assert_eq!(facts_of(&card, "Image"), ["debian:13"]);
        assert_eq!(
            facts_of(&card, "Forwarded by"),
            ["docker-proxy, run by root"]
        );
        for label in SCAN_LABELS.into_iter().chain(["Program", "Service"]) {
            assert!(facts_of(&card, label).is_empty(), "{label}");
        }
        assert!(card.notice.is_none());
    }

    #[test]
    fn a_podman_container_card_names_podman() {
        let mut port = publication(8080, "web", "nginx:latest");
        let Port::Observed(row) = &mut port else {
            unreachable!()
        };
        let d::Kind::Publication(value) = &mut row.members[0].kind else {
            unreachable!()
        };
        value.engine_kind = d::EngineKind::Podman;
        value.target_port = 80;
        let entry = Entry {
            addresses: port.addresses(),
            port,
            forwards: vec![],
        };
        let card = Card::new(&entry, "web".into(), ProcessIcon::Process, None);
        assert_eq!(card.engine, Some(d::EngineKind::Podman));
        assert_eq!(facts_of(&card, "Container"), ["web (Podman), port 80"]);
    }

    #[test]
    fn a_listener_with_an_unseen_program_says_why_in_plain_words() {
        use newport_protocol::monitor::ports::discovery as p;
        let unseen = |login: Option<&str>, account: &str| {
            let mut evidence =
                crate::connections::fixtures::observation([Listener::new(9100).address("0.0.0.0")])
                    .data
                    .unwrap();
            let snapshot = &mut evidence.current;
            snapshot.processes.clear();
            snapshot.sockets[0].holders.clear();
            snapshot.sockets[0].holder_state = p::HolderState::Unresolved;
            snapshot.sockets[0].uid = 0;
            snapshot.sockets[0].account = Some(account.into());
            snapshot.coverage.processes = p::Report {
                outcome: p::Outcome::Partial,
                issues: vec![p::Issue {
                    kind: p::IssueKind::PermissionDenied,
                    count: 268,
                    message: String::new(),
                }],
            };
            let view = newport_client::discovery::View::new(evidence, false).unwrap();
            let port = Port::Observed(d::Row {
                members: view.members().to_vec(),
            });
            let entry = Entry {
                addresses: port.addresses(),
                port,
                forwards: vec![],
            };
            let mut card = Card::new(&entry, "Port 9100".into(), ProcessIcon::Process, login);
            card.enrich(&entry, &view);
            assert_eq!(facts_of(&card, "Listening"), ["0.0.0.0:9100"]);
            for label in SCAN_LABELS {
                assert!(facts_of(&card, label).is_empty(), "{label}");
            }
            facts_of(&card, "Program")
        };
        assert_eq!(
            unseen(Some("newport"), "root"),
            ["Unknown: it runs as root, and newport can't see other accounts' programs."]
        );
        assert_eq!(
            unseen(None, "root"),
            ["Unknown: it runs as root, and your account can't see other accounts' programs."]
        );
        assert_eq!(
            unseen(Some("newport"), "newport"),
            ["Unknown: Newport couldn't see which program holds it."]
        );
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
        let card = Card::new(&entry, "Postgres".into(), ProcessIcon::Process, None);
        assert!(card.facts.contains(&("Data directory", "/srv/data".into())));
        assert_eq!(facts_of(&card, "Program"), ["postgres, run by postgres"]);
        assert_eq!(card.engine, None);
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
            let card = Card::new(&entry, "bun".into(), ProcessIcon::Process, None);
            assert!(
                card.facts.contains(&("Listening", listening.into())),
                "{ipv6_only:?}: {:?}",
                card.facts
            );
        }
    }

    #[test]
    fn a_known_program_card_leaves_scan_limits_to_discovery_details() {
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
        let mut card = Card::new(
            &entry,
            "worker".into(),
            ProcessIcon::Process,
            Some("newport"),
        );
        card.enrich(&entry, &view);
        assert_eq!(facts_of(&card, "Program"), ["worker, run by UID 1000"]);
        assert!(
            card.facts
                .iter()
                .all(|(_, value)| !value.contains("SECRET") && !value.contains("permission denied"))
        );
        for label in SCAN_LABELS
            .into_iter()
            .chain(["Cgroups", "Services", "Details"])
        {
            assert!(facts_of(&card, label).is_empty(), "{label}");
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
        second.name = Some("helper".into());
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
        let mut card = Card::new(&entry, "web.socket".into(), ProcessIcon::Process, None);
        card.enrich(&entry, &view);
        assert_eq!(
            facts_of(&card, "Program"),
            ["worker, run by UID 1000", "helper, run by UID 1000"]
        );
        assert_eq!(facts_of(&card, "Service"), ["web.socket"]);
        assert_eq!(facts_of(&card, "Socket account"), Vec::<String>::new());
        assert!(card.facts.contains(&("Triggers", "web.service".into())));
        assert!(
            card.facts
                .contains(&("Directory", "/srv/app (deleted)".into()))
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
        let card = std::rc::Rc::new(Card::new(&entry, "NATS".into(), ProcessIcon::Process, None));
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
