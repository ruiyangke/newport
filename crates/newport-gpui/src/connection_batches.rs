//! Project/worktree controls. Planning and write sequencing live in shared crates.
use super::*;
use connections_grouped::{Entry, string};
use newport_client::tunnel_operations::Action;
use newport_domain::{
    forwarding_batch::{self, Plan},
    model::{Direction, Tunnel},
    port_grouping::ListenerSelection,
};
use std::collections::HashSet;

pub(super) struct HeadingAction {
    pub button: Button,
}

/// Group actions remain visible and keyboard accessible.
#[derive(IntoElement)]
pub(super) struct Heading {
    content: Stateful<Div>,
    action: Option<HeadingAction>,
}

impl Heading {
    pub(super) fn new(content: Stateful<Div>, action: Option<HeadingAction>) -> Self {
        Self { content, action }
    }
}

impl RenderOnce for Heading {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.content
            .children(self.action.map(|action| action.button))
    }
}

/// Freeze a checkout action at render time. Saved forwards use their definition,
/// not just their ID: editing a destination must not retarget a pending click.
#[derive(Clone)]
struct CheckoutSelection {
    listeners: ListenerSelection,
    forwards: Vec<Tunnel>,
}

impl CheckoutSelection {
    fn capture(entries: &[Entry]) -> Self {
        Self {
            listeners: ListenerSelection::capture(entries),
            forwards: entries
                .iter()
                .filter(|entry| entry.saved)
                .flat_map(|entry| &entry.forwards)
                .filter_map(|forward| serde_json::from_value::<Tunnel>(forward.clone()).ok())
                .filter(|forward| forward.direction == Direction::Local)
                .collect(),
        }
    }

    fn contains_forward(&self, value: &Value) -> bool {
        let Ok(current) = serde_json::from_value::<Tunnel>(value.clone()) else {
            return false;
        };
        self.forwards.iter().any(|captured| {
            captured.id == current.id
                && captured.server_id == current.server_id
                && current.direction == Direction::Local
                && captured.project == current.project
                && captured.local_port == current.local_port
                && captured.local_port_end == current.local_port_end
                && captured.remote_host == current.remote_host
                && captured.remote_port == current.remote_port
                && captured.remote_port_end == current.remote_port_end
        })
    }

    fn resolve(&self, latest: Vec<Entry>) -> Vec<Entry> {
        latest
            .into_iter()
            .filter_map(|mut entry| {
                if !self.listeners.resolve(vec![entry.clone()]).is_empty() {
                    return Some(entry);
                }
                entry
                    .forwards
                    .retain(|forward| self.contains_forward(forward));
                if entry.forwards.is_empty() {
                    return None;
                }
                // A previously saved-only row may now have a listener. Keep the
                // captured forwards, but never create a new forward for that row.
                entry.saved = true;
                Some(entry)
            })
            .collect()
    }
}

#[derive(Clone)]
pub(super) struct Scope {
    pub project: String,
    pub worktree: Option<String>,
}
impl Scope {
    pub(super) fn includes_checkout(&self, entry: &Entry) -> bool {
        self.worktree.as_ref().is_none_or(|path| {
            (!entry.saved || entry.port["direction"] != "remote")
                && newport_domain::port_grouping::worktree_of(&entry.port)
                    .is_some_and(|worktree| worktree.path == *path)
        })
    }
    fn selection(&self, entries: &[Entry]) -> Option<CheckoutSelection> {
        self.worktree.as_ref().map(|_| {
            let entries = entries
                .iter()
                .filter(|entry| self.includes_checkout(entry))
                .cloned()
                .collect::<Vec<_>>();
            CheckoutSelection::capture(&entries)
        })
    }
    fn resolve_entries(
        &self,
        latest: Vec<Entry>,
        selection: Option<&CheckoutSelection>,
    ) -> Vec<Entry> {
        let entries = latest
            .into_iter()
            .filter(|entry| self.includes_checkout(entry))
            .collect();
        if self.worktree.is_some() {
            selection
                .map(|selection| selection.resolve(entries))
                .unwrap_or_default()
        } else {
            entries
        }
    }
    fn action_label(&self, connect: bool) -> &'static str {
        match (self.worktree.is_some(), connect) {
            (true, true) => "Forward worktree",
            (true, false) => "Stop worktree",
            (false, true) => "Forward project",
            (false, false) => "Stop project",
        }
    }
    fn key(&self) -> String {
        format!(
            "{}#{}",
            self.project,
            self.worktree.as_deref().unwrap_or("project")
        )
    }
}
impl Newport {
    fn scoped_ports(&self, scope: &Scope, selection: Option<&CheckoutSelection>) -> Vec<Entry> {
        let latest = self
            .connection_projects()
            .into_iter()
            .find(|p| p.key == scope.project)
            .map(|p| p.entries)
            .unwrap_or_default();
        scope.resolve_entries(latest, selection)
    }
    pub(super) fn plan_port_batch(&self, project: &str, entries: &[Entry]) -> Result<Plan, String> {
        let server = self.server.parse().map_err(|_| "Choose a server")?;
        let mut allocated: Vec<Tunnel> =
            serde_json::from_value(self.snapshot["config"]["tunnels"].clone())
                .map_err(|e| e.to_string())?;
        allocated.extend(self.tunnel_operations.reserved_creations());
        let active = allocated
            .iter()
            .filter(|t| {
                matches!(
                    self.snapshot["runtime"]["tunnels"][t.id.to_string()]["status"].as_str(),
                    Some("connected" | "connecting" | "reconnecting")
                )
            })
            .map(|t| t.id)
            .collect::<HashSet<_>>();
        let entries = entries
            .iter()
            .map(|e| {
                let destination = if e.saved {
                    None
                } else {
                    let p = &e.port;
                    let name = newport_domain::port_context::forward_name(p);
                    Some(forwarding_batch::Destination {
                        address: string(p, "address").to_owned(),
                        port: p["port"]
                            .as_u64()
                            .and_then(|p| u16::try_from(p).ok())
                            .ok_or("Invalid discovered port")?,
                        name,
                    })
                };
                let forwards = e
                    .forwards
                    .iter()
                    .cloned()
                    .map(serde_json::from_value)
                    .collect::<Result<Vec<Tunnel>, _>>()
                    .map_err(|e| e.to_string())?;
                Ok(forwarding_batch::Entry {
                    destination,
                    forwards,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if let [entry] = entries.as_slice() {
            forwarding_batch::plan_single(server, project, entry, &allocated, &active)
        } else {
            forwarding_batch::plan(server, project, &entries, &allocated, &active)
        }
    }
    fn run_port_batch(
        &mut self,
        scope: &Scope,
        selection: Option<&CheckoutSelection>,
        connect: bool,
        cx: &mut Context<Self>,
    ) {
        let result = (|| -> Result<(), String> {
            let entries = self.scoped_ports(scope, selection);
            if entries.iter().any(|e| self.port_busy(e)) {
                return Err("A forwarding operation is already running in this group.".into());
            }
            let plan = self.plan_port_batch(&scope.project, &entries)?;
            let server = self.server.parse().map_err(|_| "Choose a server")?;
            let action = if connect {
                if let Some(error) = plan.creation_error {
                    return Err(error);
                }
                if !plan.can_forward() {
                    return Ok(());
                }
                Action::ForwardBatch {
                    server,
                    create: plan.create,
                    connect: plan.connect,
                }
            } else {
                if plan.stop.is_empty() {
                    return Ok(());
                }
                Action::SetConnected {
                    server,
                    tunnels: plan.stop,
                    connected: false,
                }
            };
            self.tunnel_operations
                .start(action)
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(error) = result {
            self.notification = Some(Notification::error(error));
        }
        cx.notify();
    }
    pub(super) fn port_batch_button(
        &self,
        scope: Scope,
        name: &str,
        entries: &[Entry],
        cx: &mut Context<Self>,
    ) -> Option<HeadingAction> {
        let worth = !scope.project.is_empty()
            && entries
                .iter()
                .filter(|e| !e.saved || e.port["direction"] != "remote")
                .count()
                > 1;
        if !worth {
            return None;
        }
        let plan = self.plan_port_batch(&scope.project, entries);
        let (forward, has_action) = match &plan {
            // Projects have separate forward and stop controls. Worktrees keep
            // their existing single contextual action.
            Ok(p) if scope.worktree.is_none() => (true, p.can_forward()),
            Ok(p) => (
                if p.creation_error.is_some() {
                    p.stop.is_empty()
                } else {
                    p.can_forward()
                },
                p.can_forward() || !p.stop.is_empty(),
            ),
            Err(_) => (true, true),
        };
        if !has_action {
            return None;
        }
        let busy = entries.iter().any(|e| self.port_busy(e));
        let server = self.server.clone();
        // Both buttons represent their entire named scope, regardless of search.
        // Checkout membership is captured so a refresh cannot broaden the click.
        // Project actions explicitly cover all current project entries.
        let selection = scope.selection(entries);
        let button = crate::controls::button(SharedString::from(format!("batch-{}", scope.key())))
            .label(if busy {
                "Working…"
            } else {
                scope.action_label(forward)
            })
            .icon(if forward {
                IconName::Zap
            } else {
                IconName::Square
            })
            .ghost()
            .text_color(cx.theme().muted_foreground)
            .disabled(busy)
            .accessibility_label(format!("{}: {name}", scope.action_label(forward)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.server == server {
                    this.run_port_batch(&scope, selection.as_ref(), forward, cx);
                }
            }));
        Some(HeadingAction { button })
    }
    pub(super) fn port_project_stop_button(
        &self,
        project: &str,
        name: &str,
        entries: &[Entry],
        cx: &mut Context<Self>,
    ) -> Option<HeadingAction> {
        if project.is_empty()
            || !self
                .plan_port_batch(project, entries)
                .is_ok_and(|p| !p.stop.is_empty())
        {
            return None;
        }
        let busy = entries.iter().any(|e| self.port_busy(e));
        let scope = Scope {
            project: project.to_owned(),
            worktree: None,
        };
        let server = self.server.clone();
        let button = crate::controls::button(SharedString::from(format!("stop-project-{project}")))
            .label("Stop project")
            .icon(IconName::Square)
            .ghost()
            .text_color(cx.theme().muted_foreground)
            .disabled(busy)
            .accessibility_label(format!("Stop project: {name}"))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.server == server {
                    this.run_port_batch(&scope, None, false, cx);
                }
            }));
        Some(HeadingAction { button })
    }
}

#[cfg(test)]
mod tests {
    use super::Scope;
    use serde_json::json;
    fn saved_entry() -> crate::connections_grouped::Entry {
        let forward = json!({
            "id": uuid::Uuid::new_v4(), "serverId": uuid::Uuid::new_v4(),
            "name": "Vite", "localPort": 5173, "remoteHost": "127.0.0.1",
            "remotePort": 5173, "direction": "local", "project": "/repo"
        });
        let mut port = forward.clone();
        port["worktreePath"] = json!("/repo/feature");
        crate::connections_grouped::Entry {
            port,
            addresses: vec![],
            forwards: vec![forward],
            saved: true,
        }
    }

    #[test]
    fn worktree_saved_membership_fences_destination_direction_and_checkout() {
        let scope = Scope {
            project: "/repo".into(),
            worktree: Some("/repo/feature".into()),
        };
        let saved = saved_entry();
        assert!(scope.includes_checkout(&saved));
        let selection = scope.selection(std::slice::from_ref(&saved));
        assert_eq!(
            scope.resolve_entries(vec![saved.clone()], selection.as_ref()),
            vec![saved.clone()]
        );

        let mut moved = saved.clone();
        moved.port["worktreePath"] = json!("/repo/other");
        let mut retargeted = saved.clone();
        retargeted.forwards[0]["remotePort"] = json!(8000);
        let mut rebound = saved.clone();
        rebound.forwards[0]["localPort"] = json!(9000);
        let mut reverse = saved.clone();
        reverse.port["direction"] = json!("remote");
        reverse.forwards[0]["direction"] = json!("remote");
        assert!(!scope.includes_checkout(&reverse));
        assert!(
            scope
                .resolve_entries(
                    vec![moved, retargeted, rebound, reverse, saved_entry()],
                    selection.as_ref()
                )
                .is_empty()
        );

        let mut renamed = saved.clone();
        renamed.forwards[0]["name"] = json!("Renamed service");
        assert_eq!(
            scope.resolve_entries(vec![renamed.clone()], selection.as_ref()),
            vec![renamed]
        );
    }

    #[test]
    fn saved_worktree_forward_survives_listener_discovery_without_expanding_action() {
        let scope = Scope {
            project: "/repo".into(),
            worktree: Some("/repo/feature".into()),
        };
        let saved = saved_entry();
        let selection = scope.selection(std::slice::from_ref(&saved));
        let mut discovered = saved.clone();
        discovered.saved = false;
        discovered.port = json!({"port": 5173, "pid": 42, "address": "127.0.0.1", "worktreePath": "/repo/feature"});
        discovered.forwards.push(saved_entry().forwards.remove(0));
        let selected = scope.resolve_entries(vec![discovered], selection.as_ref());
        assert_eq!(selected.len(), 1);
        assert!(
            selected[0].saved,
            "a newly discovered listener must not create another forward"
        );
        assert_eq!(selected[0].forwards, saved.forwards);
    }

    #[test]
    fn batch_labels_name_their_scope() {
        let project = Scope {
            project: "/repo".into(),
            worktree: None,
        };
        let worktree = Scope {
            worktree: Some("/repo/feature".into()),
            ..project.clone()
        };
        assert_eq!(project.action_label(true), "Forward project");
        assert_eq!(project.action_label(false), "Stop project");
        assert_eq!(worktree.action_label(true), "Forward worktree");
        assert_eq!(worktree.action_label(false), "Stop worktree");
    }

    #[test]
    fn worktree_scope_keeps_all_equivalent_paths_and_excludes_other_checkouts() {
        let ports = ["/repo/feature/", "/repo/feature", "/repo/feature-other"]
            .into_iter().enumerate().map(|(i,path)| json!({"port":8000+i,"address":"127.0.0.1","projectRoot":"/repo","worktreePath":path,"worktreeMain":false})).collect();
        let project = newport_domain::port_grouping::group(ports, &[], "s", &[]).remove(0);
        let scope = Scope {
            project: "/repo".into(),
            worktree: Some("/repo/feature".into()),
        };
        let matching = project
            .entries
            .iter()
            .filter(|e| scope.includes_checkout(e))
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 2);
        assert_eq!(matching[0].port["port"], 8000);
        assert_eq!(matching[1].port["port"], 8001);
    }

    #[test]
    fn worktree_batches_keep_captured_membership_but_project_batches_cover_all_latest_rows() {
        let ports = (8000..8004)
            .map(|port| json!({"port":port,"pid":port,"address":"127.0.0.1","projectRoot":"/repo","worktreePath":"/repo/feature"}))
            .collect();
        let original = newport_domain::port_grouping::group(ports, &[], "s", &[]).remove(0);
        let captured = vec![original.entries[0].clone(), original.entries[2].clone()];
        let worktree = Scope {
            project: "/repo".into(),
            worktree: Some("/repo/feature".into()),
        };
        let project = Scope {
            project: "/repo".into(),
            worktree: None,
        };
        let selection = worktree.selection(&captured);
        let mut latest = original.entries;
        let mut new_listener = latest[0].clone();
        new_listener.port["port"] = json!(9000);
        latest.push(new_listener);
        let mut other_checkout = latest[0].clone();
        other_checkout.port["worktreePath"] = json!("/repo/other");
        latest.push(other_checkout);

        let selected = worktree.resolve_entries(latest.clone(), selection.as_ref());
        assert_eq!(selected, captured);
        assert!(worktree.resolve_entries(latest.clone(), None).is_empty());
        let empty = worktree.selection(&[]);
        assert!(
            worktree
                .resolve_entries(latest.clone(), empty.as_ref())
                .is_empty()
        );
        let unfiltered = project.selection(&captured);
        assert!(unfiltered.is_none());
        assert_eq!(
            project.resolve_entries(latest.clone(), unfiltered.as_ref()),
            latest
        );
    }
}

#[cfg(test)]
mod heading_disclosure_tests {
    use super::{Heading, HeadingAction};
    use gpui::{
        Context, FocusHandle, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Render,
        TestAppContext, VisualTestContext, Window, div, point, prelude::*, px, rgb,
    };
    use gpui_kit as gpui;
    use gpui_kit::component::{Disableable, button::Button};
    use std::{cell::Cell, rc::Rc};

    const QUIET_MARKER: u32 = 0xfc02d9;
    const PROJECT_MARKER: u32 = 0x02fcd9;

    struct Harness {
        sibling: FocusHandle,
        project: FocusHandle,
        action: FocusHandle,
        outside: FocusHandle,
        busy: bool,
        clicks: Rc<Cell<usize>>,
    }

    impl Harness {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                sibling: cx.focus_handle(),
                project: cx.focus_handle(),
                action: cx.focus_handle(),
                outside: cx.focus_handle(),
                busy: false,
                clicks: Rc::default(),
            }
        }
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            let action = HeadingAction {
                button: Button::new("quiet-action")
                    .track_focus(&self.action)
                    .accessibility_label("Quiet action")
                    .w(px(60.))
                    .h(px(30.))
                    .disabled(self.busy)
                    .child(
                        div()
                            .size(px(10.))
                            .bg(rgb(QUIET_MARKER))
                            .debug_selector(|| "quiet-marker".into()),
                    )
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .pt(px(60.))
                .child(Heading::new(
                    div()
                        .id("test-heading")
                        .flex()
                        .items_center()
                        .w(px(280.))
                        .h(px(40.))
                        .child(
                            Button::new("sibling")
                                .track_focus(&self.sibling)
                                .label("Project")
                                .w(px(80.)),
                        )
                        .child(
                            Button::new("project-batch")
                                .track_focus(&self.project)
                                .w(px(60.))
                                .h(px(30.))
                                .child(div().size(px(10.)).bg(rgb(PROJECT_MARKER))),
                        ),
                    Some(action),
                ))
                .child(
                    Button::new("outside")
                        .track_focus(&self.outside)
                        .label("Outside")
                        .w(px(80.))
                        .mt(px(60.)),
                )
        }
    }

    fn painted(visual: &mut VisualTestContext, color: u32) -> bool {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window
                .painted_quads()
                .iter()
                .any(|quad| quad.background == rgb(color).into())
        })
    }

    #[gpui::test]
    fn bulk_heading_actions_stay_visible_and_keyboard_accessible_without_extra_tab_stops(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|_, cx| Harness::new(cx));
        assert!(painted(visual, QUIET_MARKER));
        assert!(
            painted(visual, PROJECT_MARKER),
            "project batch stays visible"
        );
        let bounds = visual.debug_bounds("quiet-marker").unwrap();

        visual.simulate_mouse_move(point(px(260.), px(75.)), None, Default::default());
        assert!(
            painted(visual, QUIET_MARKER),
            "bulk action stays visible when hovering the heading"
        );
        visual.simulate_mouse_move(point(px(350.), px(250.)), None, Default::default());
        assert!(painted(visual, QUIET_MARKER));
        visual.update(|window, cx| {
            window.focus_next(cx);
            assert!(
                view.read(cx).sibling.is_focused(window),
                "heading adds no Tab stop"
            );
        });
        assert!(
            painted(visual, QUIET_MARKER),
            "bulk action stays visible while focusing a sibling"
        );
        assert_eq!(
            visual.debug_bounds("quiet-marker").unwrap(),
            bounds,
            "disclosure never shifts layout"
        );
        visual.update(|window, cx| {
            window.focus_next(cx);
            assert!(view.read(cx).project.is_focused(window));
            window.focus_next(cx);
            assert!(
                view.read(cx).action.is_focused(window),
                "bulk action remains in Tab order"
            );
        });
        assert!(painted(visual, QUIET_MARKER));
        let keystroke = Keystroke::parse("enter").unwrap();
        visual.simulate_event(KeyDownEvent {
            physical_key: None,
            key_metadata: None,
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        visual.simulate_event(KeyUpEvent {
            physical_key: None,
            key_metadata: None,
            keystroke,
        });
        assert_eq!(view.read_with(visual, |view, _| view.clicks.get()), 1);
        visual.update(|window, cx| {
            window.focus_next(cx);
            assert!(view.read(cx).outside.is_focused(window));
        });
        assert!(
            painted(visual, QUIET_MARKER),
            "bulk action stays visible after leaving the heading"
        );
        view.update(visual, |view, cx| {
            view.busy = true;
            cx.notify();
        });
        assert!(
            painted(visual, QUIET_MARKER),
            "busy worktree actions remain visible"
        );
        view.update(visual, |view, cx| {
            view.busy = false;
            cx.notify();
        });
        assert!(painted(visual, QUIET_MARKER));
    }
}
