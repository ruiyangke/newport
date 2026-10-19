//! Forward editor presentation; validation and persistence use the shared backend.
use super::*;
use gpui_kit::base::{Toggle, ToggleGroup};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ThemeStyled as _, input::InputEvent};

#[derive(Clone)]
struct FormError {
    message: String,
    input: Option<usize>,
}

impl FormError {
    fn general(message: String) -> Self {
        Self {
            message,
            input: None,
        }
    }
}

/// Exclusive direction selection. The Kit primitives own activation and AX;
/// focus roves locally so a dialog never walks the background's tab stops.
struct DirectionSelector {
    selected: usize,
    focused: usize,
    focus: [FocusHandle; 2],
    disabled: bool,
}

impl DirectionSelector {
    fn new(reverse: bool, cx: &mut Context<Self>) -> Self {
        let selected = usize::from(reverse);
        Self {
            selected,
            focused: selected,
            focus: [cx.focus_handle(), cx.focus_handle()],
            disabled: false,
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.disabled {
            self.selected = index;
            self.focused = index;
            cx.notify();
        }
    }

    fn move_focus(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || event.keystroke.modifiers.modified() {
            return;
        }
        let current = self
            .focus
            .iter()
            .position(|focus| focus.is_focused(window))
            .unwrap_or(self.focused);
        let next = match event.keystroke.key.as_str() {
            "left" | "right" => 1 - current,
            "home" => 0,
            "end" => 1,
            _ => return,
        };
        self.focused = next;
        self.focus[next].focus(window, cx);
        // Like the reference ToggleGroup, arrows move focus; Space/Enter
        // confirms the focused direction and never clears the current one.
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for DirectionSelector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The reference outlined ToggleGroup uses --muted for its pressed state.
        let selected_background: Hsla = rgb(if cx.theme().is_dark() {
            0x252b34
        } else {
            0xf5f5f5
        })
        .into();
        ToggleGroup::new("forward-direction")
            .aria_label("Direction")
            .flex()
            .w_full()
            .on_key_down(cx.listener(Self::move_focus))
            // Dialog maps Enter to Confirm. Consume it here so choosing a
            // direction cannot submit the enclosing forward editor.
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::actions::Confirm, window, cx| {
                    let current = this
                        .focus
                        .iter()
                        .position(|focus| focus.is_focused(window))
                        .unwrap_or(this.focused);
                    this.select(current, cx);
                    cx.stop_propagation();
                }),
            )
            .children(
                ["Server port → this computer", "This computer → server port"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, title)| {
                        let selected = self.selected == index;
                        let owner = cx.weak_entity();
                        Toggle::new(("direction", index))
                            .accessibility_label(title)
                            .pressed(selected)
                            .disabled(self.disabled)
                            .track_focus(&self.focus[index])
                            .tab_stop(self.focused == index)
                            .w(relative(0.5))
                            .flex_shrink_0()
                            .h(px(crate::controls::FORM_HEIGHT))
                            .px(px(8.))
                            .text_size(px(crate::controls::FONT_SIZE))
                            .font_weight(FontWeight::MEDIUM)
                            .line_height(relative(1.25))
                            .border_1()
                            .border_color(cx.theme().border)
                            .when(index == 0, |item| item.rounded_l(px(crate::ui::RADIUS)))
                            .when(index == 1, |item| {
                                item.border_l_0().rounded_r(px(crate::ui::RADIUS))
                            })
                            .bg(if selected {
                                selected_background
                            } else {
                                cx.theme().background
                            })
                            .when(self.disabled, |item| item.opacity(0.5))
                            .when(!self.disabled && !selected, |item| {
                                item.hover(|style| style.bg(selected_background))
                            })
                            .when(
                                self.focus[index].is_focused(window) && !self.disabled,
                                |item| item.focus_ring_style(window, cx),
                            )
                            .child(title)
                            .on_change(move |_, _, _, cx| {
                                let _ = owner.update(cx, |this, cx| this.select(index, cx));
                            })
                    }),
            )
    }
}

struct Form {
    owner: WeakEntity<Newport>,
    value: Value,
    /// Original configuration is immutable even as checkbox/project values change.
    expected: Option<Value>,
    fields: Vec<Entity<InputState>>,
    direction: Entity<DirectionSelector>,
    projects: Vec<(String, String)>,
    scroll: modal::FormScroll,
    pending: bool,
    error: Option<FormError>,
    _subscriptions: Vec<Subscription>,
}
impl Newport {
    pub(super) fn edit_tunnel(
        &mut self,
        value: Option<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = value.is_some();
        self.open_tunnel_form(value, existing, window, cx);
    }

    pub(super) fn open_tunnel_form(
        &mut self,
        value: Option<Value>,
        existing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) || self.server.is_empty() {
            return;
        }
        let value = value.unwrap_or_else(|| json!({"id":uuid::Uuid::new_v4(),"serverId":self.server,"name":"","localPort":8000,"remotePort":8000,"remoteHost":"127.0.0.1","direction":"local","autoConnect":false,"autoReconnect":false,"project":null}));
        let mut projects: Vec<_> = self
            .connection_projects()
            .into_iter()
            .filter(|p| !p.key.is_empty())
            .map(|p| (p.key, p.name))
            .collect();
        // Keep an assigned project editable even when discovery no longer sees it.
        if let Some(project) = value["project"].as_str().filter(|p| !p.is_empty())
            && !projects.iter().any(|(key, _)| key == project)
        {
            projects.push((project.to_owned(), project.to_owned()));
        }
        projects.sort_by_key(|(key, name)| (name.to_lowercase(), key.clone()));
        let owner = cx.weak_entity();
        let form = cx.new(|cx| Form::new(owner, value, existing, projects, window, cx));
        Form::open(form, existing, window, cx);
    }
}

impl Form {
    fn new(
        owner: WeakEntity<Newport>,
        value: Value,
        existing: bool,
        projects: Vec<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let range = |start: &str, end: &str| match value[end].as_u64() {
            Some(end) => format!("{}-{end}", value[start]),
            None => text(&value, start),
        };
        let fields: Vec<_> = [
            text(&value, "name"),
            range("localPort", "localPortEnd"),
            range("remotePort", "remotePortEnd"),
            text(&value, "remoteHost"),
        ]
        .into_iter()
        .zip([
            "Web application",
            "8000 or 8000-8010",
            "8000 or 8000-8010",
            "",
        ])
        .map(|(value, placeholder)| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(value)
                    .placeholder(placeholder)
            })
        })
        .collect();
        let direction = cx.new(|cx| DirectionSelector::new(value["direction"] == "remote", cx));
        let mut subscriptions: Vec<_> = fields
            .iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this: &mut Form, _, event, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save(window, cx);
                    }
                })
            })
            .collect();
        subscriptions.push(cx.observe(&direction, |_, _, cx| cx.notify()));
        let scroll = modal::FormScroll::new(6 + usize::from(!projects.is_empty()), window, cx);
        let expected = existing.then(|| value.clone());
        let mut value = value;
        // Preserve the existing startup intent when consolidating legacy policies.
        // Keep `expected` untouched for optimistic concurrency on save.
        let always_on = value["autoConnect"] == true;
        value["autoConnect"] = json!(always_on);
        value["autoReconnect"] = json!(always_on);
        Self {
            owner,
            expected,
            value,
            fields,
            direction,
            projects,
            scroll,
            pending: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn open(form: Entity<Self>, existing: bool, window: &mut Window, cx: &mut App) {
        let focus = form.read(cx).fields[0].focus_handle(cx);
        window.open_dialog(cx, move |dialog, window, cx| {
            let pending = form.read(cx).pending;
            let cancel_form = form.clone();
            let close_form = form.clone();
            let submit_form = form.clone();
            let body_form = form.clone();
            let compact = compact(window);
            let gutter = if compact { 18. } else { 24. };
            dialog
                .aria_label(if existing {
                    "Edit forward"
                } else {
                    "New forward"
                })
                .title(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .px(px(gutter))
                        .pt(px(if compact { 14. } else { 20. }))
                        // Dialog's title/body spacing contributes the other 8px.
                        .pb(px(if compact { 0. } else { 4. }))
                        .child(
                            div()
                                .id("forward-editor-title")
                                .role(accesskit::Role::Heading)
                                .aria_label(if existing {
                                    "Edit forward"
                                } else {
                                    "New forward"
                                })
                                .text_size(px(17.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(if existing {
                                    "Edit forward"
                                } else {
                                    "New forward"
                                }),
                        )
                        .child(
                            crate::controls::icon_button("close-forward-editor")
                                .ghost()
                                .icon(IconName::Close)
                                .text_color(cx.theme().muted_foreground)
                                .accessibility_label("Close dialog")
                                .disabled(pending)
                                .on_click(move |_, window, cx| {
                                    if !close_form.read(cx).pending {
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
                .w(px(480.))
                .p_0()
                .gap_0()
                .rounded(px(crate::ui::RADIUS))
                .border_0()
                .centered(true)
                .on_cancel(move |_, _, cx| !cancel_form.read(cx).pending)
                .on_ok(move |_, window, cx| {
                    submit_form.update(cx, |form, cx| form.save(window, cx));
                    // Only the successful asynchronous save closes the dialog.
                    false
                })
                .overlay_closable(!pending)
                .keyboard(!pending)
                .close_button(false)
                .content(move |content, _, _| content.overflow_hidden().child(body_form.clone()))
                .footer(Form::footer(&form, window, cx))
        });
        window.defer(cx, move |window, cx| focus.focus(window, cx));
    }
}
impl Form {
    fn section(&self, row: usize, content: impl IntoElement) -> impl IntoElement {
        self.scroll.row(self.row_index(row), content)
    }

    // The project row is absent when there are no choices; the rows after it
    // still need to address their actual positions in the scroll container.
    fn row_index(&self, row: usize) -> usize {
        row - usize::from(row > 4 && self.projects.is_empty())
    }

    fn show_error(&mut self, error: FormError, window: &mut Window, cx: &mut Context<Self>) {
        let input = error.input;
        self.error = Some(error);
        if let Some(input) = input {
            self.fields[input].focus_handle(cx).focus(window, cx);
            self.scroll.reveal([0, 2, 2, 3][input]);
        } else {
            self.scroll.reveal(self.row_index(6));
        }
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let values: Vec<_> = self
            .fields
            .iter()
            .map(|input| input.read(cx).value().trim().to_owned())
            .collect();
        let parsed = (|| -> Result<Value, FormError> {
            let (local, local_end) = newport_domain::forwarding::parse_port_range(&values[1])
                .map_err(|message| FormError {
                    message,
                    input: Some(1),
                })?;
            let (remote, remote_end) = newport_domain::forwarding::parse_port_range(&values[2])
                .map_err(|message| FormError {
                    message,
                    input: Some(2),
                })?;
            let mut value = self.value.clone();
            value["direction"] = json!(if self.direction.read(cx).selected == 1 {
                "remote"
            } else {
                "local"
            });
            value["name"] = json!(values[0]);
            value["remoteHost"] = json!(values[3]);
            value["localPort"] = json!(local);
            value["localPortEnd"] = json!(local_end);
            value["remotePort"] = json!(remote);
            value["remotePortEnd"] = json!(remote_end);
            let tunnel: newport_domain::model::Tunnel = serde_json::from_value(value.clone())
                .map_err(|e| FormError::general(e.to_string()))?;
            tunnel.pairs().map_err(FormError::general)?;
            Ok(value)
        })();
        let value = match parsed {
            Ok(value) => value,
            Err(error) => {
                self.show_error(error, window, cx);
                return;
            }
        };
        let result = self
            .owner
            .update(cx, |owner, cx| {
                let tunnel =
                    serde_json::from_value(value).map_err(|e| format!("Invalid forward: {e}"))?;
                let action = match &self.expected {
                    Some(expected) => newport_client::tunnel_operations::Action::Update {
                        tunnel,
                        expected: serde_json::from_value(expected.clone())
                            .map_err(|e| format!("Invalid original forward: {e}"))?,
                    },
                    None => newport_client::tunnel_operations::Action::Save(tunnel),
                };
                let ticket = owner
                    .tunnel_operations
                    .start(action)
                    .map_err(|e| e.to_string())?;
                cx.notify();
                Ok::<_, String>(ticket)
            })
            .map_err(|_| "The main window is no longer available".to_owned())
            .and_then(|r| r);
        let ticket = match result {
            Ok(ticket) => ticket,
            Err(error) => {
                self.show_error(FormError::general(error), window, cx);
                return;
            }
        };
        self.pending = true;
        self.direction.update(cx, |direction, cx| {
            direction.disabled = true;
            cx.notify();
        });
        self.error = None;
        cx.notify();
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |form, cx| {
            let result = loop {
                if let Some(result) = ticket.result() {
                    break result;
                }
                executor.timer(Duration::from_millis(50)).await;
            };
            let _ = form.update_in(cx, |form, window, cx| {
                form.pending = false;
                form.direction.update(cx, |direction, cx| {
                    direction.disabled = false;
                    cx.notify();
                });
                match result {
                    Ok(_) => {
                        let _ = form.owner.update(cx, |owner, cx| {
                            owner.request("snapshot", "snapshot", json!({}));
                            owner.notification = Some(Notification::success("Forward saved."));
                            cx.notify();
                        });
                        window.close_dialog(cx);
                    }
                    Err(error) => form.show_error(FormError::general(error), window, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
fn compact(window: &Window) -> bool {
    let viewport = window.viewport_size();
    viewport.height <= px(560.) || viewport.width <= px(540.)
}

fn field(title: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(7.))
        .line_height(relative(1.))
        .child(
            div()
                .id(SharedString::from(format!("forward-field-{title}")))
                .role(accesskit::Role::Label)
                .aria_label(title.to_owned())
                .text_size(px(crate::ui::FONT_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .child(title.to_owned()),
        )
}

fn hint(value: &'static str, cx: &App) -> Div {
    div().child(
        div()
            .id(value)
            .role(accesskit::Role::Paragraph)
            .aria_label(value)
            .text_size(px(crate::ui::FONT_SIZE))
            .font_weight(FontWeight::NORMAL)
            .text_color(cx.theme().muted_foreground)
            .line_height(relative(1.5))
            .child(value),
    )
}

fn control_background(cx: &App) -> Hsla {
    // EditorControls.tsx mixes 45% native-toolbar with the page background.
    rgb(if cx.theme().is_dark() {
        0x232426
    } else {
        0xfbfafb
    })
    .into()
}

fn error_message(message: String, cx: &App) -> impl IntoElement {
    div()
        .id("forward-editor-error")
        .debug_selector(|| "forward-editor-error".into())
        .role(accesskit::Role::Alert)
        .aria_label(message.clone())
        .text_size(px(crate::ui::FONT_SIZE))
        .line_height(relative(1.65))
        .text_color(crate::theme::error(cx.theme().is_dark()))
        .child(message)
}

impl Form {
    fn input(&self, index: usize, title: &str, cx: &App) -> Input {
        crate::controls::form_input(&self.fields[index])
            .aria_label(title.to_owned())
            .disabled(self.pending)
            .rounded(px(crate::ui::RADIUS))
            .bg(control_background(cx))
            .border_color(cx.theme().border)
    }

    fn input_field(&self, index: usize, title: &str, cx: &App) -> Div {
        field(title).child(self.input(index, title, cx)).when_some(
            self.error
                .as_ref()
                .filter(|error| error.input == Some(index)),
            |field, error| field.child(error_message(error.message.clone(), cx)),
        )
    }

    fn footer(form: &Entity<Self>, window: &Window, cx: &App) -> Div {
        let pending = form.read(cx).pending;
        let submit = form.clone();
        let cancel = form.clone();
        let compact = compact(window);
        div()
            .debug_selector(|| "forward-editor-footer".into())
            .flex()
            .flex_wrap()
            .justify_end()
            .gap(px(8.))
            .px(px(if compact { 18. } else { 24. }))
            .pt(px(if compact { 10. } else { 12. }))
            .pb(px(if compact { 14. } else { 20. }))
            .child(
                crate::controls::form_button("cancel-forward")
                    .label("Cancel")
                    .min_w(px(76.))
                    .rounded(px(crate::ui::RADIUS))
                    .disabled(pending)
                    .on_click(move |_, window, cx| {
                        if !cancel.read(cx).pending {
                            window.close_dialog(cx);
                        }
                    }),
            )
            .child(
                crate::controls::form_button("save-forward")
                    .primary()
                    .label(if pending { "Saving…" } else { "Save forward" })
                    .min_w(px(76.))
                    .rounded(px(crate::ui::RADIUS))
                    .loading(pending)
                    .disabled(pending)
                    .on_click(move |_, window, cx| {
                        submit.update(cx, |form, cx| form.save(window, cx));
                    }),
            )
    }
}

impl Render for Form {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reverse = self.direction.read(cx).selected == 1;
        let project = self.value["project"].as_str().unwrap_or("").to_owned();
        let choices = self.projects.clone();
        let project_name = self
            .projects
            .iter()
            .find(|(key, _)| key == &project)
            .map(|(_, name)| name.as_str())
            .unwrap_or("None")
            .to_owned();
        let weak = cx.weak_entity();
        let compact = compact(window);
        let address_label = if reverse {
            "Listen address on the server"
        } else {
            "Remote host"
        };
        let body = div()
            .id("forward-editor-body")
            .debug_selector(|| "forward-editor-viewport".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll.handle)
            .gap(px(if compact { 16. } else { 20. }))
            .px(px(if compact { 18. } else { 24. }))
            .pt(px(8.))
            .pb(px(if compact { 14. } else { 20. }))
            .text_size(px(crate::ui::FONT_SIZE))
            .child(self.section(0, self.input_field(0, "Name", cx)))
            .child(self.section(1,
                field("Direction")
                    .line_height(relative(1.5))
                    .child(self.direction.clone())
                    .child(hint(if reverse {
                        "Reverse forward (ssh -R): the server can reach a service running on this computer."
                    } else {
                        "Local forward (ssh -L): reach a service on the server from this computer."
                    }, cx)),
            ))
            .child(self.section(2,
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .child(
                        self.input_field(1, "Local port or range", cx).flex_1(),
                    )
                    .child(
                        div()
                            .mt(px(20.))
                            .flex_shrink_0()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                Icon::new(if reverse {
                                    IconName::ArrowLeft
                                } else {
                                    IconName::ArrowRight
                                })
                                .size(px(18.)),
                            ),
                    )
                    .child(
                        self.input_field(2, "Remote port or range", cx).flex_1(),
                    ),
            ))
            .child(self.section(3,
                self.input_field(3, address_label, cx)
                    .child(hint(if reverse {
                        "127.0.0.1 keeps the port private to the server. Other addresses need GatewayPorts in the server's sshd_config."
                    } else {
                        "Relative to the SSH server. Local ports are accessible only on this computer."
                    }, cx)),
            ))
            .when(!choices.is_empty(), |view| {
                view.child(self.section(4,
                    field("Project").child(
                        crate::controls::form_button("forward-project")
                            .w_full()
                            .rounded(px(crate::ui::RADIUS))
                            .bg(control_background(cx))
                            .disabled(self.pending)
                            .accessibility_label(format!("Project: {project_name}"))
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.))
                                    .child(div().flex_1().truncate().child(project_name))
                                    .child(Icon::new(IconName::ChevronDown).size(px(14.))),
                            )
                            .dropdown_menu(move |mut menu, _, _| {
                                for (key, title) in
                                    std::iter::once((String::new(), "None".to_owned()))
                                        .chain(choices.clone())
                                {
                                    let owner = weak.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(title).checked(key == project).on_click(
                                            move |_, _, cx| {
                                                let _ = owner.update(cx, |form, cx| {
                                                    if !form.pending {
                                                        form.value["project"] = if key.is_empty() {
                                                            Value::Null
                                                        } else {
                                                            json!(key)
                                                        };
                                                        cx.notify();
                                                    }
                                                });
                                            },
                                        ),
                                    );
                                }
                                menu
                            }),
                    ),
                ))
            })
            .child(self.section(5, crate::controls::checkbox("always-on")
                .accessibility_label("Always on")
                .gap(px(9.))
                .line_height(relative(1.6))
                .checked(self.value["autoConnect"] == true)
                .disabled(self.pending)
                .child(
                    div().child("Always on").child(div().mt(px(3.)).child(hint(
                        "Connect when Newport starts and reconnect if the connection drops. Retries up to 10 times.",
                        cx,
                    ))),
                )
                .on_click(cx.listener(|this, value: &bool, _, cx| {
                    if !this.pending {
                        this.value["autoConnect"] = json!(value);
                        this.value["autoReconnect"] = json!(value);
                        cx.notify();
                    }
                }))))
            .when_some(self.error.clone().filter(|error| error.input.is_none()), |view, error| {
                view.child(self.section(6, error_message(error.message, cx)))
            })
            ;
        div()
            .id("forward-editor-scroll-region")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .relative()
            .child(body)
            .vertical_scrollbar(&self.scroll.handle)
    }
}

#[cfg(test)]
mod tests {
    use super::{DirectionSelector, Form, FormError};
    use gpui::{
        AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
        KeyDownEvent, KeyUpEvent, Keystroke, ParentElement, Render, Styled, TestAppContext,
        VisualTestContext, Window, div, px,
    };
    use gpui_kit as gpui;
    use gpui_kit::component::WindowExt as _;
    use std::{cell::Cell, rc::Rc};

    struct EditorHost;

    impl Render for EditorHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn editor(cx: &mut TestAppContext, projects: bool) -> (Entity<Form>, &mut VisualTestContext) {
        editor_with_policy(cx, projects, false, true)
    }

    fn editor_with_policy(
        cx: &mut TestAppContext,
        projects: bool,
        auto_connect: bool,
        auto_reconnect: bool,
    ) -> (Entity<Form>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(|_| EditorHost);
            gpui_kit::base::Root::new(content, window, cx)
        });
        cx.simulate_resize(gpui::size(px(500.), px(400.)));
        let form = cx.update(|window, cx| {
            window.activate_window();
            let value = serde_json::json!({
                "id":uuid::Uuid::new_v4(), "serverId":uuid::Uuid::new_v4(),
                "name":"Application", "direction":"local", "localPort":8000,
                "remotePort":8000, "remoteHost":"127.0.0.1",
                "autoConnect":auto_connect, "autoReconnect":auto_reconnect, "project":null
            });
            let projects = if projects {
                vec![("/app".into(), "Application".into())]
            } else {
                vec![]
            };
            let form = cx.new(|cx| {
                Form::new(
                    gpui::WeakEntity::new_invalid(),
                    value,
                    true,
                    projects,
                    window,
                    cx,
                )
            });
            Form::open(form.clone(), false, window, cx);
            form
        });
        draw_editor(cx);
        (form, cx)
    }

    fn draw_editor(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn row_visible(form: &Entity<Form>, row: usize, cx: &mut VisualTestContext) {
        let selector = [
            "form-scroll-row-0",
            "form-scroll-row-1",
            "form-scroll-row-2",
            "form-scroll-row-3",
            "form-scroll-row-4",
            "form-scroll-row-5",
            "form-scroll-row-6",
            "form-scroll-row-7",
        ][row];
        let bounds = cx.debug_bounds(selector).unwrap();
        let viewport = form.read_with(cx, |form, _| form.scroll.handle.bounds());
        assert!(
            bounds.top() >= viewport.top() - px(0.5),
            "row {row}: {bounds:?}; viewport {viewport:?}"
        );
        assert!(
            bounds.bottom() <= viewport.bottom() + px(0.5),
            "row {row}: {bounds:?}; viewport {viewport:?}"
        );
    }

    fn input_heights_match_source(cx: &mut VisualTestContext) {
        cx.update(|window, _| {
            let snapshots = gpui_kit::base::test_support::snapshots(window);
            for label in [
                "Name",
                "Local port or range",
                "Remote port or range",
                "Remote host",
            ] {
                let input = snapshots
                    .iter()
                    .find(|element| {
                        element.label() == Some(label)
                            && element.role() == Some(gpui::Role::TextInput)
                    })
                    .unwrap_or_else(|| panic!("missing rendered input: {label}"));
                assert_eq!(input.bounds().size.height, px(34.), "{label}: {input:?}");
            }
            for label in ["Cancel", "Save forward"] {
                let button = snapshots.iter().find(|e| e.label() == Some(label)).unwrap();
                assert_eq!(button.bounds().size.height, px(34.), "{label}");
            }
            let close = snapshots
                .iter()
                .find(|e| e.label() == Some("Close dialog"))
                .unwrap();
            assert_eq!(close.bounds().size, gpui::size(px(28.), px(28.)));
        });
    }

    #[gpui::test]
    fn always_on_consolidates_legacy_policies_and_toggles_both_flags(cx: &mut TestAppContext) {
        for (auto_connect, auto_reconnect) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let (form, cx) = editor_with_policy(cx, true, auto_connect, auto_reconnect);
            cx.simulate_resize(gpui::size(px(1000.), px(900.)));
            draw_editor(cx);
            form.read_with(cx, |form, _| {
                assert_eq!(form.value["autoConnect"], auto_connect);
                assert_eq!(form.value["autoReconnect"], auto_connect);
                let original = form.expected.as_ref().unwrap();
                assert_eq!(original["autoConnect"], auto_connect);
                assert_eq!(original["autoReconnect"], auto_reconnect);
            });
            let checkbox = cx.update(|window, _| {
                let snapshots = gpui_kit::base::test_support::snapshots(window);
                assert!(!snapshots.iter().any(|element| matches!(
                    element.label(),
                    Some("Connect when Newport starts" | "Reconnect if the connection drops")
                )));
                snapshots
                    .iter()
                    .find(|element| element.label() == Some("Always on"))
                    .unwrap()
                    .bounds()
                    .center()
            });
            for enabled in [!auto_connect, auto_connect] {
                cx.simulate_click(checkbox, Default::default());
                draw_editor(cx);
                form.read_with(cx, |form, _| {
                    assert_eq!(form.value["autoConnect"], enabled);
                    assert_eq!(form.value["autoReconnect"], enabled);
                    assert_eq!(
                        form.expected.as_ref().unwrap()["autoReconnect"],
                        auto_reconnect
                    );
                });
            }
            key(cx, "escape");
        }
    }

    #[gpui::test]
    fn compact_editor_reveals_fields_in_both_tab_directions_and_keeps_footer_fixed(
        cx: &mut TestAppContext,
    ) {
        let (form, cx) = editor(cx, true);
        input_heights_match_source(cx);
        let footer = cx.debug_bounds("forward-editor-footer").unwrap();
        let viewport = form.read_with(cx, |form, _| form.scroll.handle.bounds());
        assert!(viewport.size.height > px(100.));
        assert!(viewport.bottom() <= footer.top());
        assert!(form.read_with(cx, |form, _| form.scroll.handle.max_offset().y) > px(0.));
        row_visible(&form, 0, cx);
        // Name → Direction → Local → Remote → Host → Project → Always on.
        for row in [1, 2, 2, 3, 4, 5] {
            key(cx, "tab");
            draw_editor(cx);
            row_visible(&form, row, cx);
            assert_eq!(cx.debug_bounds("forward-editor-footer").unwrap(), footer);
        }
        for row in [4, 3, 2, 2, 1, 0] {
            key(cx, "shift-tab");
            draw_editor(cx);
            row_visible(&form, row, cx);
        }
        // Focus reveal should not pin scrolling to the focused row forever.
        form.read_with(cx, |form, _| {
            form.scroll
                .handle
                .set_offset(gpui::point(px(0.), -px(120.)))
        });
        draw_editor(cx);
        assert!(form.read_with(cx, |form, _| form.scroll.handle.offset().y) < px(0.));
        key(cx, "escape");
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn roomy_editor_keeps_content_sized_height_without_an_extra_scroll_area(
        cx: &mut TestAppContext,
    ) {
        let (form, cx) = editor(cx, true);
        cx.simulate_resize(gpui::size(px(1000.), px(900.)));
        draw_editor(cx);
        input_heights_match_source(cx);
        let dialog = cx.debug_bounds("dialog-0").unwrap();
        assert_eq!(dialog.size.width, px(480.));
        assert!(
            dialog.size.height < px(868.),
            "the editor should shrink to its content: {dialog:?}"
        );
        assert_eq!(
            form.read_with(cx, |form, _| form.scroll.handle.max_offset().y),
            px(0.)
        );
        row_visible(&form, 0, cx);
        row_visible(&form, 5, cx);
    }

    #[gpui::test]
    fn compact_editor_reveals_invalid_port_and_error_without_submitting(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        cx.update(|window, cx| {
            let fields = form.read(cx).fields.clone();
            fields[1].update(cx, |input, cx| input.set_value("70000", window, cx));
        });
        // With no project selector, the Always on row has an earlier index.
        for row in [1, 2, 2, 3, 4] {
            key(cx, "tab");
            draw_editor(cx);
            row_visible(&form, row, cx);
        }
        let footer = cx.debug_bounds("forward-editor-footer").unwrap();
        let save = cx.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .find(|element| element.label() == Some("Save forward"))
                .unwrap()
                .bounds()
                .center()
        });
        cx.simulate_click(save, Default::default());
        draw_editor(cx);
        row_visible(&form, 2, cx);
        cx.update(|window, cx| {
            let form = form.read(cx);
            assert_eq!(form.error.as_ref().unwrap().input, Some(1));
            assert!(form.fields[1].focus_handle(cx).is_focused(window));
            assert!(!form.pending);
            assert!(window.has_active_dialog(cx));
        });
        let error = cx.debug_bounds("forward-editor-error").unwrap();
        let viewport = form.read_with(cx, |form, _| form.scroll.handle.bounds());
        assert!(error.top() >= viewport.top() && error.bottom() <= viewport.bottom());
        assert_eq!(cx.debug_bounds("forward-editor-footer").unwrap(), footer);
        key(cx, "enter");
        row_visible(&form, 2, cx);
        cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn compact_editor_reveals_general_errors_and_retains_pending_dismissal_guards(
        cx: &mut TestAppContext,
    ) {
        let (form, cx) = editor(cx, true);
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.show_error(
                    FormError::general(
                        "The saved forward changed. Review it before saving.".into(),
                    ),
                    window,
                    cx,
                )
            })
        });
        draw_editor(cx);
        row_visible(&form, 6, cx);
        let footer = cx.debug_bounds("forward-editor-footer").unwrap();
        form.update(cx, |form, cx| {
            form.pending = true;
            cx.notify();
        });
        draw_editor(cx);
        key(cx, "escape");
        cx.simulate_click(gpui::point(px(4.), px(4.)), Default::default());
        let cancel = cx.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .find(|element| element.label() == Some("Cancel"))
                .unwrap()
                .bounds()
                .center()
        });
        cx.simulate_click(cancel, Default::default());
        cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(cx.debug_bounds("forward-editor-footer").unwrap(), footer);
        form.update(cx, |form, cx| {
            form.pending = false;
            cx.notify();
        });
        draw_editor(cx);
        key(cx, "escape");
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    struct DirectionHarness {
        selector: Entity<DirectionSelector>,
        before: FocusHandle,
        after: FocusHandle,
        confirms: Rc<Cell<usize>>,
    }

    impl Render for DirectionHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let confirms = self.confirms.clone();
            div()
                .key_context("Dialog")
                .on_action(move |_: &gpui_kit::base::actions::Confirm, _, _| {
                    confirms.set(confirms.get() + 1);
                })
                .w(px(480.))
                .child(div().id("before").h(px(20.)).track_focus(&self.before))
                .child(self.selector.clone())
                .child(div().id("after").h(px(20.)).track_focus(&self.after))
        }
    }

    struct Controls {
        selector: Entity<DirectionSelector>,
        before: FocusHandle,
        after: FocusHandle,
        confirms: Rc<Cell<usize>>,
    }

    fn harness(cx: &mut TestAppContext) -> (Controls, &mut VisualTestContext) {
        cx.update(gpui_kit::component::init);
        let (view, cx) = cx.add_window_view(|_, cx| DirectionHarness {
            selector: cx.new(|cx| DirectionSelector::new(false, cx)),
            before: cx.focus_handle().tab_stop(true),
            after: cx.focus_handle().tab_stop(true),
            confirms: Rc::new(Cell::new(0)),
        });
        let controls = view.read_with(cx, |view, _| Controls {
            selector: view.selector.clone(),
            before: view.before.clone(),
            after: view.after.clone(),
            confirms: view.confirms.clone(),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (controls, cx)
    }

    fn key(cx: &mut VisualTestContext, value: &str) {
        let keystroke = Keystroke::parse(value).unwrap();
        cx.simulate_event(KeyDownEvent {
            physical_key: None,
            key_metadata: None,
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent {
            physical_key: None,
            key_metadata: None,
            keystroke,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui::test]
    fn direction_keyboard_activation_is_exclusive_and_does_not_need_pointer_bubbling(
        cx: &mut TestAppContext,
    ) {
        let (
            Controls {
                selector,
                before,
                confirms,
                ..
            },
            cx,
        ) = harness(cx);
        cx.update(|window, _| {
            let mut bounds: Vec<_> = gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .filter(|element| {
                    matches!(
                        element.label(),
                        Some("Server port → this computer" | "This computer → server port")
                    )
                })
                .map(|element| element.bounds())
                .collect();
            // Snapshot storage is keyed, not in visual/DOM order.
            bounds.sort_by(|a, b| a.left().partial_cmp(&b.left()).unwrap());
            assert_eq!(bounds.len(), 2);
            assert!(bounds.iter().all(|bounds| bounds.size.height == px(34.)));
            assert_eq!(bounds[0].size.width, bounds[1].size.width);
            assert_eq!(
                bounds[0].right(),
                bounds[1].left(),
                "segments share one edge"
            );
        });
        cx.update(|window, cx| {
            before.focus(window, cx);
            window.focus_next(cx);
        });
        key(cx, "right");
        cx.update(|window, cx| {
            let selector = selector.read(cx);
            assert_eq!(
                selector.selected, 0,
                "arrows move focus without changing direction"
            );
            assert!(selector.focus[1].is_focused(window));
        });
        key(cx, "space");
        assert_eq!(selector.read_with(cx, |selector, _| selector.selected), 1);
        key(cx, "enter");
        assert_eq!(
            selector.read_with(cx, |selector, _| selector.selected),
            1,
            "the only selected direction cannot be cleared"
        );
        key(cx, "left");
        key(cx, "enter");
        assert_eq!(selector.read_with(cx, |selector, _| selector.selected), 0);
        assert_eq!(
            confirms.get(),
            0,
            "direction activation must not submit the dialog"
        );
    }

    #[gpui::test]
    fn direction_has_one_tab_stop_and_arrows_wrap_within_the_group(cx: &mut TestAppContext) {
        let (
            Controls {
                selector,
                before,
                after,
                ..
            },
            cx,
        ) = harness(cx);
        cx.update(|window, cx| {
            before.focus(window, cx);
            window.focus_next(cx);
        });
        key(cx, "left");
        assert_eq!(selector.read_with(cx, |selector, _| selector.focused), 1);
        key(cx, "right");
        assert_eq!(selector.read_with(cx, |selector, _| selector.focused), 0);
        key(cx, "end");
        assert_eq!(selector.read_with(cx, |selector, _| selector.focused), 1);
        cx.update(|window, cx| {
            window.focus_next(cx);
            assert!(
                after.is_focused(window),
                "Tab leaves the two-option group after one stop"
            );
            window.focus_prev(cx);
            assert!(selector.read(cx).focus[1].is_focused(window));
        });
        key(cx, "home");
        cx.update(|window, cx| {
            window.focus_prev(cx);
            assert!(before.is_focused(window));
        });
    }

    #[gpui::test]
    fn pending_direction_does_not_change_from_queued_keyboard_activation(cx: &mut TestAppContext) {
        let (
            Controls {
                selector,
                after,
                confirms,
                ..
            },
            cx,
        ) = harness(cx);
        cx.update(|window, cx| {
            let focus = selector.read(cx).focus[1].clone();
            focus.focus(window, cx);
        });
        selector.update(cx, |selector, cx| {
            selector.disabled = true;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        key(cx, "space");
        key(cx, "enter");
        key(cx, "right");
        assert_eq!(selector.read_with(cx, |selector, _| selector.selected), 0);
        assert_eq!(confirms.get(), 0);
        cx.update(|window, cx| {
            after.focus(window, cx);
            window.focus_prev(cx);
            assert!(
                !selector
                    .read(cx)
                    .focus
                    .iter()
                    .any(|focus| focus.is_focused(window)),
                "pending options are not tab stops"
            );
        });
    }
}
