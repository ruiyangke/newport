//! Compact, read-only connection context. Constructed with the cached row.
use super::*;
use crate::{connections_grouped::Entry, connections_process_icon::ProcessIcon};
use newport_domain::port_context;

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
    process: Option<String>,
    pub forward: Option<Forward>,
    pub notice: Option<String>,
}

impl Card {
    pub fn new(entry: &Entry, title: String, icon: ProcessIcon) -> Self {
        let p = &entry.port;
        let docker = !entry.saved && port_context::is_docker(p);
        let mut facts = Vec::new();
        let mut add = |label, field| {
            if let Some(value) = p[field].as_str().filter(|s| !s.is_empty()) {
                facts.push((label, value.to_owned()));
            }
        };
        if !entry.saved {
            if docker {
                if p["containerName"].as_str() != Some(title.as_str()) {
                    add("Container", "containerName");
                }
                add("Image", "image");
                if p["composeService"]
                    .as_str()
                    .is_some_and(|s| !s.eq_ignore_ascii_case(&title))
                {
                    add("Service", "composeService");
                }
            } else {
                for detail in port_context::process_details(p) {
                    facts.push((detail.label, detail.value));
                }
            }
            let port = p["port"].as_u64().unwrap_or(0);
            let addresses = entry
                .addresses
                .iter()
                .map(|address| {
                    if address.contains(':') && !address.starts_with('[') {
                        format!("[{address}]:{port}")
                    } else {
                        format!("{address}:{port}")
                    }
                })
                .collect::<Vec<_>>();
            if !addresses.is_empty() {
                facts.push(("Listening", addresses.join(", ")));
            }
        }
        // Docker discovery observes the host proxy, not the process in the container.
        let process = (!entry.saved && !docker)
            .then(|| {
                p["pid"].as_u64().filter(|pid| *pid != 0).map(|pid| {
                    match p["user"].as_str().filter(|s| !s.is_empty()) {
                        Some(user) => format!("PID {pid} · {user}"),
                        None => format!("PID {pid}"),
                    }
                })
            })
            .flatten();
        let notice = if p["workingDirectoryDeleted"] == true {
            Some("The process is still running from a deleted directory.".into())
        } else if p["executableDeleted"] == true {
            Some("The executable has changed. Restart the process to use the new version.".into())
        } else {
            None
        };
        Self {
            title,
            icon,
            docker,
            facts,
            process,
            forward: None,
            notice,
        }
    }

    pub fn has_details(&self) -> bool {
        !self.facts.is_empty()
            || self.process.is_some()
            || self.forward.is_some()
            || self.notice.is_some()
    }

    pub fn build(self: std::rc::Rc<Self>, window: &mut Window, cx: &mut App) -> AnyView {
        let width = crate::hover::width(window);
        let height = crate::hover::height(window);
        crate::hover::surface(gpui_kit::component::tooltip::Tooltip::element(
            move |_, cx| {
                div()
                    .id("connection-hover-scroll")
                    .max_h(px(height))
                    .overflow_y_scroll()
                    .child(self.content(width - 2. * crate::hover::PADDING, cx))
            },
        ))
        .w(px(width))
        .p(px(crate::hover::PADDING))
        .build(window, cx)
    }

    fn content(&self, width: f32, cx: &App) -> AnyElement {
        div()
            .id("connection-hover-card")
            .debug_selector(|| "connection-hover-card".into())
            .test_support()
            .w(px(width))
            .min_w_0()
            .overflow_hidden()
            .text_size(px(12.))
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
                                .rounded(px(4.))
                                .px(px(5.))
                                .py(px(1.))
                                .bg(cx.theme().muted)
                                .text_color(cx.theme().muted_foreground)
                                .text_size(px(10.))
                                .child("Docker"),
                        )
                    }),
            )
            .when(!self.facts.is_empty(), |v| {
                v.child(
                    div().mt(px(12.)).flex().flex_col().gap(px(7.)).children(
                        self.facts
                            .iter()
                            .map(|(label, value)| fact(label, value, cx)),
                    ),
                )
            })
            .when_some(self.process.clone(), |v, process| {
                v.child(
                    div()
                        .mt(px(10.))
                        .text_size(px(11.))
                        .text_color(cx.theme().muted_foreground)
                        .child(process),
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
                        .when_some(forward.name, |v, name| v.child(fact("Saved as", &name, cx)))
                        .child(fact("Status", &forward.status, cx))
                        .when(!forward.address.is_empty(), |v| {
                            v.child(fact("Address", &forward.address, cx))
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

fn fact(label: &str, value: &str, cx: &App) -> AnyElement {
    div()
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
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Card, Entry, ProcessIcon};
    use serde_json::json;

    #[test]
    fn connections_hover_uses_container_identity_and_omits_host_proxy_details() {
        let entry = Entry {
            port: json!({"containerName":"stack-nats-1","composeService":"nats","image":"nats:latest",
                "port":4222,"pid":123,"user":"root","command":"/nix/store/very-long/docker-proxy --secret=hidden"}),
            addresses: vec!["0.0.0.0".into(), "::".into()],
            forwards: Vec::new(),
            saved: false,
        };
        let card = Card::new(&entry, "NATS".into(), ProcessIcon::Process);
        assert_eq!(
            card.facts,
            vec![
                ("Container", "stack-nats-1".into()),
                ("Image", "nats:latest".into()),
                ("Listening", "0.0.0.0:4222, [::]:4222".into()),
            ]
        );
        assert!(card.process.is_none());
        assert!(card.notice.is_none());
    }

    #[test]
    fn connections_hover_preserves_real_process_identity_and_warnings() {
        let entry = Entry {
            port: json!({"executable":"postgres","arguments":["postgres","-D","/srv/data"],
                "port":5432,"pid":42,"user":"postgres","workingDirectoryDeleted":true}),
            addresses: vec!["127.0.0.1".into()],
            forwards: Vec::new(),
            saved: false,
        };
        let card = Card::new(&entry, "Postgres".into(), ProcessIcon::Process);
        assert!(card.facts.contains(&("Data directory", "/srv/data".into())));
        assert_eq!(card.process.as_deref(), Some("PID 42 · postgres"));
        assert!(card.notice.is_some());
        assert!(!card.docker);
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
        let entry = Entry {
            port: json!({"containerName":"x".repeat(300), "image":format!("registry.test/{}:latest", "x".repeat(500)), "port":4222}),
            addresses: vec!["0.0.0.0".into()],
            forwards: Vec::new(),
            saved: false,
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
            assert!(
                bounds.size.height < px(900.),
                "long values must wrap: {bounds:?}"
            );
        }
    }
}
