//! Project/worktree controls. Planning and write sequencing live in shared crates.
use super::*;
use connections_grouped::{Entry, string};
use newport_client::tunnel_operations::Action;
use newport_domain::{
    forwarding_batch::{self, Plan},
    model::Tunnel,
    port_grouping::ListenerSelection,
};
use std::collections::HashSet;

type MenuBuilder = dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu;

pub(super) struct HeadingAction {
    pub button: Button,
    busy: bool,
    menu: Option<Box<MenuBuilder>>,
}

struct HeadingState {
    focus: FocusHandle,
    menu_open: bool,
    _subscriptions: [Subscription; 2],
}

/// Heading disclosure is local presentation state. Opacity keeps the action
/// in the layout, accessibility tree, and keyboard order while it is quiet.
#[derive(IntoElement)]
pub(super) struct Heading {
    id: SharedString,
    content: Stateful<Div>,
    action: Option<HeadingAction>,
}

impl Heading {
    pub(super) fn new(
        id: SharedString,
        content: Stateful<Div>,
        action: Option<HeadingAction>,
    ) -> Self {
        Self {
            id,
            content,
            action,
        }
    }
}

impl RenderOnce for Heading {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(
            SharedString::from(format!("heading-presentation:{}", self.id)),
            cx,
            |window, cx| {
                let focus = cx.focus_handle();
                let entered = cx.on_focus_in(&focus, window, |_, _, cx| cx.notify());
                let exited = cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify());
                HeadingState {
                    focus,
                    menu_open: false,
                    _subscriptions: [entered, exited],
                }
            },
        );
        let presentation = state.read(cx);
        let focus = presentation.focus.clone();
        let engaged = presentation.menu_open || focus.contains_focused(window, cx);
        self.content
            .group(self.id.clone())
            .track_focus(&focus)
            .tab_stop(false)
            .when_some(self.action, |heading, action| {
                let button = action
                    .button
                    .opacity(if engaged || action.busy { 1. } else { 0. })
                    .group_hover(self.id, |style| style.opacity(1.))
                    .focus(|style| style.opacity(1.));
                let button = if let Some(builder) = action.menu {
                    button
                        .dropdown_menu(move |menu, window, cx| builder(menu, window, cx))
                        .on_open_change(move |open, _, cx| {
                            state.update(cx, |state, cx| {
                                state.menu_open = *open;
                                cx.notify();
                            });
                        })
                        .into_any_element()
                } else {
                    button.into_any_element()
                };
                heading.child(button)
            })
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
            !entry.saved
                && newport_domain::port_grouping::worktree_of(&entry.port)
                    .is_some_and(|worktree| worktree.path == *path)
        })
    }
    fn selection(&self, entries: &[Entry]) -> Option<ListenerSelection> {
        self.worktree
            .as_ref()
            .map(|_| ListenerSelection::capture(entries))
    }
    fn resolve_entries(
        &self,
        latest: Vec<Entry>,
        selection: Option<&ListenerSelection>,
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
    fn key(&self) -> String {
        format!(
            "{}#{}",
            self.project,
            self.worktree.as_deref().unwrap_or("project")
        )
    }
}
impl Newport {
    fn scoped_ports(&self, scope: &Scope, selection: Option<&ListenerSelection>) -> Vec<Entry> {
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
        selection: Option<&ListenerSelection>,
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
        // Worktree buttons represent filtered rows; project buttons cover the
        // full project, including discoveries made since the last render.
        let selection = scope.selection(entries);
        let button = crate::controls::button(SharedString::from(format!("batch-{}", scope.key())))
            .label(if busy {
                "Working…"
            } else if forward {
                "Forward all"
            } else {
                "Stop all"
            })
            .icon(if forward {
                IconName::Zap
            } else {
                IconName::Square
            })
            .ghost()
            .text_color(cx.theme().muted_foreground)
            .disabled(busy)
            .accessibility_label(format!(
                "{} in {name}",
                if forward {
                    "Forward all ports"
                } else {
                    "Stop all forwards"
                }
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.server == server {
                    this.run_port_batch(&scope, selection.as_ref(), forward, cx);
                }
            }));
        Some(HeadingAction {
            button,
            busy,
            menu: None,
        })
    }
    pub(super) fn port_project_menu(
        &self,
        project: &str,
        name: &str,
        entries: &[Entry],
        cx: &mut Context<Self>,
    ) -> Option<HeadingAction> {
        if project.is_empty() {
            return None;
        }
        let can_stop = self
            .plan_port_batch(project, entries)
            .is_ok_and(|p| p.creation_error.is_none() && p.can_forward() && !p.stop.is_empty());
        let busy = entries.iter().any(|e| self.port_busy(e));
        let scope = Scope {
            project: project.to_owned(),
            worktree: None,
        };
        let server = self.server.clone();
        let weak = cx.weak_entity();
        let button = crate::controls::compact_icon_button(SharedString::from(format!(
            "project-actions-{project}"
        )))
        .icon(IconName::Ellipsis)
        .ghost()
        .accessibility_label(format!("Actions for {name}"));
        Some(HeadingAction {
            button,
            busy,
            menu: Some(Box::new(move |menu, _, _| {
                let owner = weak.clone();
                let server_id = server.clone();
                let key = scope.project.clone();
                let menu = menu.item(PopupMenuItem::new("New forward here").icon(IconName::Plus).on_click(move |_,window,cx| {
                    let _ = owner.update(cx, |this,cx| {
                        if this.server != server_id { return }
                        let value = json!({"id":uuid::Uuid::new_v4(),"serverId":server_id,"name":"","localPort":8000,"remotePort":8000,"remoteHost":"127.0.0.1","direction":"local","autoConnect":false,"autoReconnect":true,"project":key});
                        this.open_tunnel_form(Some(value),false,window,cx);
                    });
                }));
                if !can_stop {
                    return menu;
                }
                let owner = weak.clone();
                let server_id = server.clone();
                let scope = scope.clone();
                menu.item(
                    PopupMenuItem::new("Stop all")
                        .icon(IconName::Square)
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.server == server_id {
                                    this.run_port_batch(&scope, None, false, cx);
                                }
                            });
                        }),
                )
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Scope;
    use serde_json::json;
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
    fn worktree_batches_keep_visible_selection_but_project_batches_cover_all_latest_rows() {
        let ports = (8000..8004)
            .map(|port| json!({"port":port,"pid":port,"address":"127.0.0.1","projectRoot":"/repo","worktreePath":"/repo/feature"}))
            .collect();
        let original = newport_domain::port_grouping::group(ports, &[], "s", &[]).remove(0);
        let visible = vec![original.entries[0].clone(), original.entries[2].clone()];
        let worktree = Scope {
            project: "/repo".into(),
            worktree: Some("/repo/feature".into()),
        };
        let project = Scope {
            project: "/repo".into(),
            worktree: None,
        };
        let selection = worktree.selection(&visible);
        let mut latest = original.entries;
        let mut new_listener = latest[0].clone();
        new_listener.port["port"] = json!(9000);
        latest.push(new_listener);
        let mut other_checkout = latest[0].clone();
        other_checkout.port["worktreePath"] = json!("/repo/other");
        latest.push(other_checkout);

        let selected = worktree.resolve_entries(latest.clone(), selection.as_ref());
        assert_eq!(selected, visible);
        assert!(worktree.resolve_entries(latest.clone(), None).is_empty());
        let empty = worktree.selection(&[]);
        assert!(
            worktree
                .resolve_entries(latest.clone(), empty.as_ref())
                .is_empty()
        );
        let unfiltered = project.selection(&visible);
        assert!(unfiltered.is_none());
        assert_eq!(
            project.resolve_entries(latest.clone(), unfiltered.as_ref()),
            latest
        );
    }
}

#[cfg(test)]
mod heading_disclosure_tests {
    use super::{Heading, HeadingAction, MenuBuilder};
    use gpui::{
        Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Render,
        TestAppContext, VisualTestContext, WeakEntity, Window, div, point, prelude::*, px, rgb,
    };
    use gpui_kit as gpui;
    use gpui_kit::component::{
        Disableable,
        button::Button,
        menu::{PopupMenu, PopupMenuItem},
    };
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    const QUIET_MARKER: u32 = 0xfc02d9;
    const PROJECT_MARKER: u32 = 0x02fcd9;

    struct Harness {
        sibling: FocusHandle,
        project: FocusHandle,
        action: FocusHandle,
        outside: FocusHandle,
        busy: bool,
        with_menu: bool,
        clicks: Rc<Cell<usize>>,
        menu: Rc<RefCell<Option<WeakEntity<PopupMenu>>>>,
    }

    impl Harness {
        fn new(with_menu: bool, cx: &mut Context<Self>) -> Self {
            Self {
                sibling: cx.focus_handle(),
                project: cx.focus_handle(),
                action: cx.focus_handle(),
                outside: cx.focus_handle(),
                busy: false,
                with_menu,
                clicks: Rc::default(),
                menu: Rc::default(),
            }
        }
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            let menu_probe = self.menu.clone();
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
                busy: self.busy,
                menu: self.with_menu.then(|| {
                    Box::new(
                        move |menu: PopupMenu, _: &mut Window, cx: &mut Context<PopupMenu>| {
                            *menu_probe.borrow_mut() = Some(cx.weak_entity());
                            menu.item(PopupMenuItem::new("New forward"))
                        },
                    ) as Box<MenuBuilder>
                }),
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .pt(px(60.))
                .child(Heading::new(
                    "test-heading".into(),
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
    fn quiet_heading_actions_reveal_on_hover_focus_and_busy_without_extra_tab_stops(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|_, cx| Harness::new(false, cx));
        assert!(!painted(visual, QUIET_MARKER));
        assert!(
            painted(visual, PROJECT_MARKER),
            "project batch stays visible"
        );
        let bounds = visual.debug_bounds("quiet-marker").unwrap();

        visual.simulate_mouse_move(point(px(260.), px(75.)), None, Default::default());
        assert!(
            painted(visual, QUIET_MARKER),
            "hover anywhere in the heading reveals the action"
        );
        visual.simulate_mouse_move(point(px(350.), px(250.)), None, Default::default());
        assert!(!painted(visual, QUIET_MARKER));
        visual.update(|window, cx| {
            window.focus_next(cx);
            assert!(
                view.read(cx).sibling.is_focused(window),
                "heading adds no Tab stop"
            );
        });
        assert!(
            painted(visual, QUIET_MARKER),
            "focus in a sibling reveals the action"
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
                "quiet action remains in Tab order"
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
            !painted(visual, QUIET_MARKER),
            "leaving the heading makes the action quiet again"
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
        assert!(!painted(visual, QUIET_MARKER));
    }

    #[gpui::test]
    fn quiet_heading_menu_stays_visible_while_open_and_restores_keyboard_focus(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, visual) = cx.add_window_view(|_, cx| Harness::new(true, cx));
        assert!(!painted(visual, QUIET_MARKER));
        let trigger = visual.debug_bounds("quiet-marker").unwrap().center();
        visual.simulate_click(trigger, Default::default());
        visual.simulate_mouse_move(point(px(350.), px(250.)), None, Default::default());
        assert!(
            painted(visual, QUIET_MARKER),
            "open menus retain their visible trigger without hover"
        );
        visual.update(|window, cx| {
            let menu = view
                .read(cx)
                .menu
                .borrow()
                .as_ref()
                .unwrap()
                .upgrade()
                .unwrap();
            assert!(
                menu.focus_handle(cx).contains_focused(window, cx),
                "keyboard focus is inside the open menu"
            );
        });
        visual.simulate_keystrokes("escape");
        assert!(
            painted(visual, QUIET_MARKER),
            "dismissal restores visible trigger focus"
        );
        visual.update(|window, cx| view.read(cx).outside.clone().focus(window, cx));
        assert!(
            !painted(visual, QUIET_MARKER),
            "closed menu does not retain stale visibility"
        );
    }
}
