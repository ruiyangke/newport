//! Server editor presentation; agent discovery and persistence stay in the backend.
use super::*;
use gpui_kit::component::{
    input::InputEvent,
    scroll::ScrollableElement as _,
    select::{Select, SelectEvent, SelectItem, SelectState},
    spinner::Spinner,
};

const KEYS: &str = "server-form-agent-keys";
const NAME: usize = 0;
const HOST: usize = 1;
const PORT: usize = 2;
const USER: usize = 3;
const IDENTITY: usize = 4;
const PASSWORD: usize = 5;

#[derive(Clone)]
struct Choice {
    value: String,
    title: SharedString,
}
impl Choice {
    fn new(value: impl Into<String>, title: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            title: title.into(),
        }
    }
}
impl SelectItem for Choice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &String {
        &self.value
    }
}
type ChoiceState = SelectState<Vec<Choice>>;

#[derive(Clone)]
struct FormError {
    message: String,
    input: Option<usize>,
}
impl FormError {
    fn general(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            input: None,
        }
    }
}

pub(super) struct ServerForm {
    owner: WeakEntity<Newport>,
    value: Value,
    fields: Vec<Entity<InputState>>,
    authentication: Entity<ChoiceState>,
    key: Entity<ChoiceState>,
    keys: Vec<Value>,
    warnings: Vec<String>,
    keys_loading: bool,
    key_revision: Option<Instant>,
    file_mode: bool,
    existing: bool,
    pending: bool,
    error: Option<FormError>,
    scroll: modal::FormScroll,
    _subscriptions: Vec<Subscription>,
}

impl Newport {
    pub(super) fn edit_server(
        &mut self,
        value: Option<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.configuration_available()
            || self.server_form.is_some()
            || window.has_active_dialog(cx)
        {
            return;
        }
        let existing = value.is_some();
        if value.as_ref().is_some_and(|server| {
            self.resources
                .get(&format!("server-delete:{}", text(server, "id")))
                .is_some_and(|r| r.pending)
        }) {
            return;
        }
        self.server_picker.update(cx, |picker, cx| picker.close(cx));
        let value = value.unwrap_or_else(|| json!({"id":uuid::Uuid::new_v4(),"name":"","sshHost":"","sshUser":"","sshPort":22,"identityFile":null,"agentSource":null,"agentKeyFingerprint":null,"authMethod":"publicKey"}));
        self.resources.invalidate("server-save");
        self.resources.invalidate(KEYS);
        if value["authMethod"] != "password" {
            self.request(KEYS, "ssh_agent_keys", json!({}));
        }
        let owner = cx.weak_entity();
        let form = cx.new(|cx| ServerForm::new(owner, value, existing, window, cx));
        self.server_form = Some(form.clone());
        ServerForm::open(form, window, cx);
        self.notice = None;
        cx.notify();
    }
}

impl ServerForm {
    fn new(
        owner: WeakEntity<Newport>,
        value: Value,
        existing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fields: Vec<_> = [
            text(&value, "name"),
            text(&value, "sshHost"),
            text(&value, "sshPort"),
            text(&value, "sshUser"),
            value["identityFile"].as_str().unwrap_or_default().into(),
            String::new(),
        ]
        .into_iter()
        .zip([
            "Development server",
            "server.example.com",
            "22",
            "username",
            "~/.ssh/id_ed25519",
            "",
        ])
        .enumerate()
        .map(|(index, (value, placeholder))| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(value)
                    .placeholder(placeholder)
                    .masked(index == PASSWORD)
            })
        })
        .collect();
        let authentication = cx.new(|cx| {
            let mut state = SelectState::new(
                vec![
                    Choice::new("publicKey", "SSH key / agent"),
                    Choice::new("password", "Password"),
                ],
                None,
                window,
                cx,
            );
            state.set_selected_value(&text(&value, "authMethod"), window, cx);
            state
        });
        let file_mode = value["identityFile"]
            .as_str()
            .is_some_and(|s| !s.is_empty());
        let keys_loading = value["authMethod"] != "password";
        let selected = selected_key(&value, file_mode);
        let key = cx.new(|cx| {
            let mut state =
                SelectState::new(key_choices(&value, &[], keys_loading), None, window, cx);
            state.set_selected_value(&selected, window, cx);
            state
        });
        let mut subscriptions: Vec<_> = fields
            .iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this: &mut Self, _, event, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save(window, cx);
                    }
                })
            })
            .collect();
        subscriptions.push(
            cx.subscribe_in(&authentication, window, |this, _, event, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    if this.pending {
                        return;
                    }
                    this.value["authMethod"] = json!(value);
                    this.error = None;
                    if value == "publicKey" {
                        this.refresh_keys(cx);
                    }
                    cx.notify();
                }
            }),
        );
        subscriptions.push(cx.subscribe_in(&key, window, |this, _, event, window, cx| {
            if let SelectEvent::Confirm(Some(value)) = event {
                this.select_key(value, window, cx);
            }
        }));
        if let Some(owner) = owner.upgrade() {
            subscriptions.push(cx.observe_in(&owner, window, |this, owner, window, cx| {
                this.sync_resources(&owner, window, cx);
            }));
        }
        Self {
            owner,
            value,
            fields,
            authentication,
            key,
            keys: vec![],
            warnings: vec![],
            keys_loading,
            key_revision: None,
            file_mode,
            existing,
            pending: false,
            error: None,
            scroll: modal::FormScroll::new(11, window, cx),
            _subscriptions: subscriptions,
        }
    }

    fn open(form: Entity<Self>, window: &mut Window, cx: &mut App) {
        let focus = form.read(cx).fields[NAME].focus_handle(cx);
        window.open_dialog(cx, move |dialog, window, cx| {
            let pending = form.read(cx).pending;
            let title = if form.read(cx).existing {
                "Edit server"
            } else {
                "Add server"
            };
            let compact = compact(window);
            let cancel = form.clone();
            let cleanup = form.clone();
            let close = form.clone();
            let submit = form.clone();
            let body = form.clone();
            dialog
                .aria_label(title)
                .title(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .px(px(if compact { 18. } else { 24. }))
                        .pt(px(if compact { 14. } else { 20. }))
                        .pb(px(if compact { 0. } else { 4. }))
                        .child(
                            div()
                                .id("server-editor-title")
                                .role(accesskit::Role::Heading)
                                .aria_label(title)
                                .text_size(px(17.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(
                            crate::controls::icon_button("close-server-editor")
                                .ghost()
                                .icon(IconName::Close)
                                .text_color(cx.theme().muted_foreground)
                                .accessibility_label("Close dialog")
                                .disabled(pending)
                                .on_click(move |_, window, cx| Self::close(&close, window, cx)),
                        ),
                )
                .w(px(480.))
                .p_0()
                .gap_0()
                .rounded(px(crate::ui::RADIUS))
                .border_0()
                .centered(true)
                .on_cancel(move |_, _, cx| !cancel.read(cx).pending)
                .on_close(move |_, _, cx| Self::cleanup(&cleanup, cx))
                .on_ok(move |_, window, cx| {
                    submit.update(cx, |form, cx| form.save(window, cx));
                    false
                })
                .overlay_closable(!pending)
                .keyboard(!pending)
                .close_button(false)
                .content(move |content, _, _| content.overflow_hidden().child(body.clone()))
                .footer(Self::footer(&form, window, cx))
        });
        window.defer(cx, move |window, cx| focus.focus(window, cx));
    }

    fn cleanup(form: &Entity<Self>, cx: &mut App) {
        let owner = form.read(cx).owner.clone();
        let _ = owner.update(cx, |owner, cx| {
            if owner
                .server_form
                .as_ref()
                .is_some_and(|active| active == form)
            {
                owner.server_form = None;
                owner.resources.invalidate(KEYS);
                owner.resources.invalidate("server-save");
                cx.notify();
            }
        });
    }
    fn close(form: &Entity<Self>, window: &mut Window, cx: &mut App) {
        if form.read(cx).pending {
            return;
        }
        Self::cleanup(form, cx);
        window.close_dialog(cx);
    }

    fn sync_resources(
        &mut self,
        owner: &Entity<Newport>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = owner.read(cx);
        let key_result = owner
            .resources
            .get(KEYS)
            .filter(|r| !r.pending && r.finished_at != self.key_revision)
            .map(|r| (r.value.clone(), r.error.clone(), r.finished_at));
        let save_result = self
            .pending
            .then(|| {
                owner
                    .resources
                    .get("server-save")
                    .filter(|r| !r.pending)
                    .map(|r| r.error.clone())
            })
            .flatten();
        if let Some((value, error, revision)) = key_result {
            self.key_revision = revision;
            self.keys_loading = false;
            if let Some(error) = error {
                self.warnings = vec![format!("Could not read agent keys: {error}")];
            } else {
                self.keys = rows(&value["keys"]);
                self.warnings = value["warnings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
            }
            self.sync_key_choices(window, cx);
            cx.notify();
        }
        if let Some(error) = save_result {
            self.pending = false;
            if let Some(error) = error {
                self.show_error(FormError::general(error), window, cx);
            } else {
                // Programmatic close does not dispatch the dialog's on_close
                // callback. Release the editor's shell state explicitly too.
                let form = cx.entity();
                window.defer(cx, move |window, cx| Self::close(&form, window, cx));
            }
            cx.notify();
        }
    }

    fn refresh_keys(&mut self, cx: &mut Context<Self>) {
        if self.pending || self.keys_loading {
            return;
        }
        let result = self.owner.update(cx, |owner, cx| {
            owner.request(KEYS, "ssh_agent_keys", json!({}));
            cx.notify();
        });
        if result.is_ok() {
            self.keys_loading = true;
            cx.notify();
        }
    }
    fn sync_key_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choices = key_choices(&self.value, &self.keys, self.keys_loading);
        let selected = selected_key(&self.value, self.file_mode);
        self.key.update(cx, |key, cx| {
            key.set_items(choices, window, cx);
            key.set_selected_value(&selected, window, cx);
            cx.notify();
        });
    }
    fn select_key(&mut self, selected: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let selected_key = self.keys.iter().find(|key| key_id(key) == selected);
        // The synthetic saved-key choice must preserve its unavailable fingerprint.
        if selected_key.is_none() && !["automatic", "file"].contains(&selected) {
            return;
        }
        self.file_mode = selected == "file";
        if !self.file_mode {
            self.value["identityFile"] = Value::Null;
            self.fields[IDENTITY].update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.value["agentSource"] = selected_key
            .map(|key| key["source"].clone())
            .unwrap_or(Value::Null);
        self.value["agentKeyFingerprint"] = selected_key
            .map(|key| key["fingerprint"].clone())
            .unwrap_or(Value::Null);
        self.error = None;
        cx.notify();
    }

    fn draft(&self, cx: &App) -> Result<(Value, Value), FormError> {
        let values: Vec<_> = self
            .fields
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect();
        let invalid = |input, message: &str| FormError {
            input: Some(input),
            message: message.into(),
        };
        if values[HOST].trim().is_empty() {
            return Err(invalid(HOST, "Enter an SSH host."));
        }
        let port = values[PORT]
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| invalid(PORT, "Enter an SSH port between 1 and 65535."))?;
        if values[USER].trim().is_empty() {
            return Err(invalid(USER, "Enter a username."));
        }
        let password_auth = self.value["authMethod"] == "password";
        if password_auth && !self.existing && values[PASSWORD].is_empty() {
            return Err(invalid(PASSWORD, "Enter a password."));
        }
        if !password_auth && self.file_mode && values[IDENTITY].trim().is_empty() {
            return Err(invalid(IDENTITY, "Enter an identity-file path."));
        }
        let mut value = self.value.clone();
        for (field, index) in [("name", NAME), ("sshHost", HOST), ("sshUser", USER)] {
            value[field] = json!(values[index].trim());
        }
        value["sshPort"] = json!(port);
        if self.file_mode {
            value["identityFile"] = if values[IDENTITY].trim().is_empty() {
                Value::Null
            } else {
                json!(values[IDENTITY].trim())
            };
        }
        // Password bytes are deliberately not trimmed; null retains the vault entry.
        let password = if values[PASSWORD].is_empty() {
            Value::Null
        } else {
            json!(values[PASSWORD])
        };
        Ok((value, password))
    }
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let (server, password) = match self.draft(cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.show_error(error, window, cx);
                return;
            }
        };
        let result = self.owner.update(cx,|owner,cx| {
            if !owner.configuration_available() { return Err("The server configuration is unavailable. Try again after reconnecting.".to_owned()); }
            if self.existing && !rows(&owner.snapshot["config"]["servers"]).iter().any(|server| server["id"] == self.value["id"]) {
                return Err("This server was removed. Cancel this editor and add a new server if needed.".to_owned());
            }
            if owner.resources.get(&format!("server-delete:{}",text(&self.value,"id"))).is_some_and(|r| r.pending) { return Err("This server is being deleted.".to_owned()); }
            owner.request("server-save","save_server",json!({"server":server,"password":password}));
            cx.notify();
            Ok(())
        }).map_err(|_| "The main window is no longer available.".to_owned()).and_then(|r| r);
        match result {
            Ok(()) => {
                self.pending = true;
                self.error = None;
                cx.notify();
            }
            Err(error) => self.show_error(FormError::general(error), window, cx),
        }
    }
    fn show_error(&mut self, error: FormError, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = error.input {
            self.fields[input].focus_handle(cx).focus(window, cx);
            self.scroll.reveal(match input {
                NAME => 1,
                HOST | PORT => 2,
                USER => 3,
                PASSWORD => 6,
                IDENTITY => 9,
                _ => unreachable!(),
            });
        } else {
            self.scroll.reveal(self.error_row());
        }
        self.error = Some(error);
        cx.notify();
    }
    fn error_row(&self) -> usize {
        if self.value["authMethod"] == "password" {
            7
        } else if self.file_mode && self.value["agentKeyFingerprint"].is_null() {
            10
        } else {
            9
        }
    }

    fn input_field(&self, index: usize, title: &str, cx: &App) -> Div {
        let input = crate::controls::form_input(&self.fields[index])
            .aria_label(title.to_owned())
            .disabled(self.pending)
            .rounded(px(crate::ui::RADIUS))
            .bg(control_background(cx))
            .border_color(cx.theme().border);
        field(title).child(input).when_some(
            self.error
                .as_ref()
                .filter(|error| error.input == Some(index)),
            |field, error| field.child(error_message(error.message.clone(), cx)),
        )
    }
    fn select(&self, state: &Entity<ChoiceState>, title: &str, cx: &App) -> Select<Vec<Choice>> {
        crate::controls::form_select(state)
            .accessibility_label(title.to_owned())
            .disabled(self.pending)
            .w_full()
            .menu_item_min_height(px(28.))
            .rounded(px(crate::ui::RADIUS))
            .bg(control_background(cx))
            .border_color(cx.theme().border)
    }
    fn footer(form: &Entity<Self>, window: &Window, cx: &App) -> Div {
        let pending = form.read(cx).pending;
        let existing = form.read(cx).existing;
        let cancel = form.clone();
        let submit = form.clone();
        let remove = form.clone();
        let compact = compact(window);
        let danger = theme::error(cx.theme().is_dark());
        div()
            .debug_selector(|| "server-editor-footer".into())
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(if compact { 18. } else { 24. }))
            .pt(px(if compact { 10. } else { 12. }))
            .pb(px(if compact { 14. } else { 20. }))
            // Deleting a server is done where the server is edited, apart
            // from Save and behind its own confirmation -- not one click from
            // the switcher used many times a day.
            .when(existing, |footer| {
                footer.child(
                    crate::controls::form_button("delete-server")
                        .ghost()
                        .label("Delete server…")
                        .accessibility_label("Delete server")
                        .text_color(danger)
                        .rounded(px(crate::ui::RADIUS))
                        .disabled(pending)
                        .on_click(move |_, window, cx| {
                            let owner = remove.read(cx).owner.clone();
                            Self::close(&remove, window, cx);
                            window.defer(cx, move |window, cx| {
                                let _ = owner.update(cx, |owner, cx| {
                                    owner.confirm_delete_server(window, cx)
                                });
                            });
                        }),
                )
            })
            .child(div().flex_1())
            .child(
                crate::controls::form_button("cancel-server")
                    .label("Cancel")
                    .min_w(px(76.))
                    .rounded(px(crate::ui::RADIUS))
                    .disabled(pending)
                    .on_click(move |_, window, cx| Self::close(&cancel, window, cx)),
            )
            .child(
                crate::controls::form_button("save-server")
                    .primary()
                    .label(if pending { "Saving…" } else { "Save server" })
                    .min_w(px(76.))
                    .rounded(px(crate::ui::RADIUS))
                    .loading(pending)
                    .disabled(pending)
                    .on_click(move |_, window, cx| {
                        submit.update(cx, |form, cx| form.save(window, cx))
                    }),
            )
    }
}
impl Render for ServerForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = compact(window);
        let key_auth = self.value["authMethod"] != "password";
        let mut body = div()
            .id("server-editor-body")
            .debug_selector(|| "server-editor-viewport".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll.handle)
            .gap(px(14.))
            .px(px(if compact { 18. } else { 24. }))
            .pt(px(8.))
            .pb(px(if compact { 14. } else { 20. }))
            .text_size(px(crate::ui::FONT_SIZE))
            .child(self.scroll.row(0, legend("Connection", cx)))
            .child(
                self.scroll
                    .row(1, self.input_field(NAME, "Name (optional)", cx)),
            )
            .child(
                self.scroll.row(
                    2,
                    div()
                        .flex()
                        .items_start()
                        .gap(px(14.))
                        .child(self.input_field(HOST, "SSH host", cx).flex_1())
                        .child(
                            self.input_field(PORT, "SSH port", cx)
                                .w(px(95.))
                                .flex_shrink_0(),
                        ),
                ),
            )
            .child(self.scroll.row(3, self.input_field(USER, "Username", cx)))
            .child(self.scroll.row(
                4,
                legend("Sign in", cx).mt(px(if compact { 2. } else { 6. })),
            ))
            .child(self.scroll.row(
                5,
                field("Authentication").child(self.select(
                    &self.authentication,
                    "Authentication",
                    cx,
                )),
            ));
        if key_auth {
            let initial_keys_loading =
                self.keys_loading && self.key_revision.is_none() && self.keys.is_empty();
            let has_key_info = self.keys.is_empty()
                || !self.warnings.is_empty()
                || self.value["agentKeyFingerprint"].is_string();
            let mut info = div().flex().flex_col().gap(px(14.));
            if initial_keys_loading {
                info = info.child(crate::loading::lines("agent keys", 1));
            } else if self.keys.is_empty() {
                info = info.child(help(
                    "No keys found. Unlock your SSH agent and refresh, or use a key file.",
                    12.,
                    cx,
                ));
            }
            for warning in &self.warnings {
                info = info.child(help(warning.clone(), 12., cx));
            }
            if let Some(fingerprint) = self.value["agentKeyFingerprint"].as_str() {
                info = info.child(help(format!("Selected key: {fingerprint}"), 12., cx));
            }
            body = body
                .child(self.scroll.row(
                    6,
                    field("SSH key").child(self.select(&self.key, "SSH key", cx)),
                ))
                .child(
                    self.scroll.row(
                        7,
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(
                                crate::controls::form_button("refresh-agent-keys")
                                    .label("Refresh agent keys")
                                    .rounded(px(crate::ui::RADIUS))
                                    .disabled(self.pending || self.keys_loading)
                                    .on_click(cx.listener(|this, _, _, cx| this.refresh_keys(cx))),
                            )
                            .child(
                                div()
                                    .id("refresh-agent-keys-progress")
                                    .size(px(14.))
                                    .flex_shrink_0()
                                    .when(self.keys_loading, |slot| {
                                        slot.role(accesskit::Role::Status)
                                            .aria_label("Refreshing agent keys")
                                            .child(
                                                Spinner::new()
                                                    .with_size(px(14.))
                                                    .color(cx.theme().muted_foreground),
                                            )
                                    }),
                            ),
                    ),
                );
            if has_key_info {
                body = body.child(self.scroll.row(8, info));
            }
            if self.file_mode && self.value["agentKeyFingerprint"].is_null() {
                body = body.child(self.scroll.row(
                    9,
                    self.input_field(IDENTITY, "Identity file", cx).child(help(
                        "Load encrypted keys into your system agent first.",
                        11.,
                        cx,
                    )),
                ));
            }
        } else {
            body = body.child(self.scroll.row(
                6,
                self.input_field(PASSWORD, "Password", cx).child(help(
                    if self.existing {
                        "Leave empty to keep the saved password."
                    } else {
                        "Saved in your encrypted vault."
                    },
                    11.,
                    cx,
                )),
            ));
        }
        if let Some(error) = self.error.as_ref().filter(|error| error.input.is_none()) {
            body = body.child(
                self.scroll
                    .row(self.error_row(), error_message(error.message.clone(), cx)),
            );
        }
        div()
            .id("server-editor-scroll-region")
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

fn selected_key(value: &Value, file_mode: bool) -> String {
    value["agentKeyFingerprint"]
        .as_str()
        .map(|fingerprint| {
            format!(
                "{}:{fingerprint}",
                value["agentSource"].as_str().unwrap_or("system")
            )
        })
        .unwrap_or_else(|| if file_mode { "file" } else { "automatic" }.into())
}
fn key_id(value: &Value) -> String {
    format!("{}:{}", text(value, "source"), text(value, "fingerprint"))
}
fn key_choices(value: &Value, keys: &[Value], loading: bool) -> Vec<Choice> {
    let mut choices = vec![
        Choice::new("automatic", "Automatic · system agent and default keys"),
        Choice::new("file", "Enter a key-file path…"),
    ];
    if value["agentKeyFingerprint"].is_string()
        && !keys
            .iter()
            .any(|key| key_id(key) == selected_key(value, false))
    {
        choices.push(Choice::new(
            selected_key(value, false),
            format!(
                "Saved key · {}",
                if loading {
                    "checking agent…"
                } else {
                    "not currently available"
                }
            ),
        ));
    }
    choices.extend(keys.iter().map(|key| {
        Choice::new(
            key_id(key),
            format!(
                "{} · {} · {}",
                if key["source"] == "onePassword" {
                    "1Password"
                } else {
                    "System agent"
                },
                key["comment"]
                    .as_str()
                    .filter(|comment| !comment.is_empty())
                    .unwrap_or(key["algorithm"].as_str().unwrap_or_default()),
                text(key, "fingerprint")
            ),
        )
    }));
    choices
}
fn compact(window: &Window) -> bool {
    window.viewport_size().height <= px(560.) || window.viewport_size().width <= px(540.)
}
fn control_background(cx: &App) -> Hsla {
    rgb(if cx.theme().is_dark() {
        0x232426
    } else {
        0xfbfafb
    })
    .into()
}
fn field(title: &str) -> Div {
    div().flex().flex_col().min_w_0().gap(px(7.)).child(
        div()
            .line_height(relative(1.))
            .text_size(px(crate::ui::FONT_SIZE))
            .font_weight(FontWeight::MEDIUM)
            .child(title.to_owned()),
    )
}
fn legend(title: &'static str, cx: &App) -> Div {
    div()
        .text_size(px(crate::ui::FONT_SIZE))
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .line_height(relative(1.))
        .child(title)
}
fn help(value: impl Into<String>, size: f32, cx: &App) -> Div {
    div()
        .text_size(px(size))
        .font_weight(FontWeight::NORMAL)
        .text_color(cx.theme().muted_foreground)
        .line_height(relative(1.65))
        .child(value.into())
}
fn error_message(message: String, cx: &App) -> impl IntoElement {
    div()
        .id("server-editor-error")
        .debug_selector(|| "server-editor-error".into())
        .role(accesskit::Role::Alert)
        .aria_label(message.clone())
        .text_size(px(crate::ui::FONT_SIZE))
        .line_height(relative(1.65))
        .text_color(crate::theme::error(cx.theme().is_dark()))
        .child(message)
}

#[cfg(test)]
mod tests {
    use super::{FormError, HOST, IDENTITY, PASSWORD, PORT, ServerForm, key_choices};
    use gpui::{
        AppContext, Context, Entity, Focusable, IntoElement, Render, Styled, WeakEntity, Window,
        div, px,
    };
    use gpui::{KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext};
    use gpui_kit as gpui;
    use gpui_kit::component::WindowExt as _;
    use serde_json::{Value, json};

    struct Host;
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }
    fn editor(
        cx: &mut TestAppContext,
        existing: bool,
    ) -> (Entity<ServerForm>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(|_| Host);
            gpui_kit::base::Root::new(content, window, cx)
        });
        cx.simulate_resize(gpui::size(px(500.), px(440.)));
        let form = cx.update(|window,cx| {
            window.activate_window();
            let value = json!({"id":uuid::Uuid::new_v4(),"name":"","sshHost":"localhost","sshPort":22,"sshUser":"ruiyang","authMethod":"publicKey","identityFile":null,"agentKeyFingerprint":null,"agentSource":null,"clipboardEnabled":true});
            let form = cx.new(|cx|ServerForm::new(WeakEntity::new_invalid(),value,existing,window,cx));
            form.update(cx, |form,cx| { form.keys_loading=false; form.sync_key_choices(window,cx); });
            ServerForm::open(form.clone(),window,cx);
            form
        });
        draw(cx);
        (form, cx)
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
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
        draw(cx);
    }
    fn click(cx: &mut VisualTestContext, label: &str) {
        let target = cx.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .find(|e| e.label() == Some(label))
                .unwrap_or_else(|| panic!("missing {label}"))
                .bounds()
                .center()
        });
        cx.simulate_click(target, Default::default());
        draw(cx);
    }
    fn visible(form: &Entity<ServerForm>, row: usize, cx: &mut VisualTestContext) {
        let selectors = [
            "form-scroll-row-0",
            "form-scroll-row-1",
            "form-scroll-row-2",
            "form-scroll-row-3",
            "form-scroll-row-4",
            "form-scroll-row-5",
            "form-scroll-row-6",
            "form-scroll-row-7",
            "form-scroll-row-8",
            "form-scroll-row-9",
            "form-scroll-row-10",
        ];
        let bounds = cx.debug_bounds(selectors[row]).unwrap();
        let viewport = form.read_with(cx, |form, _| form.scroll.handle.bounds());
        assert!(
            bounds.top() >= viewport.top() - px(0.5)
                && bounds.bottom() <= viewport.bottom() + px(0.5),
            "row {row} outside viewport: {bounds:?} {viewport:?}"
        );
    }

    #[gpui::test]
    fn save_completion_survives_snapshot_navigation_before_dialog_observation(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let mut owner = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| crate::page_perf_tests::owner(window, cx));
            owner = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let owner = owner.unwrap();
        let (work, requests) = std::sync::mpsc::sync_channel(32);
        let (replies, incoming) = std::sync::mpsc::channel();
        owner.update_in(cx, |owner, window, cx| {
            owner.work = work.into();
            owner.replies = incoming;
            owner.edit_server(None, window, cx);
        });
        let form = owner.read_with(cx, |owner, _| owner.server_form.clone().unwrap());
        form.update_in(cx, |form, window, cx| {
            form.fields[HOST].update(cx, |input, cx| input.set_value("localhost", window, cx));
            form.fields[super::USER].update(cx, |input, cx| input.set_value("fixture", window, cx));
            form.save(window, cx);
        });
        let save = requests
            .try_iter()
            .find(|work| work.key == "server-save")
            .unwrap();
        let saved = save.args["server"].clone();
        owner.update(cx, |owner, cx| {
            let generation = owner.resources.begin("snapshot").unwrap();
            let mut snapshot = owner.snapshot.clone();
            snapshot["config"]["servers"]
                .as_array_mut()
                .unwrap()
                .push(saved.clone());
            for (key, generation, result) in [
                ("server-save", save.generation, Value::Null),
                ("snapshot", generation, snapshot),
            ] {
                replies
                    .send(crate::Completion {
                        key: key.into(),
                        generation,
                        result: Ok(result),
                        server: saved["id"].as_str().unwrap().into(),
                        image: None,
                        pdf: None,
                        pdf_text: None,
                    })
                    .unwrap();
            }
            // Both replies are drained before GPUI delivers the owner observer.
            owner.tick(cx);
            assert_eq!(owner.server, saved["id"].as_str().unwrap());
        });
        draw(cx);
        assert!(!form.read_with(cx, |form, _| form.pending));
        assert!(owner.read_with(cx, |owner, _| owner.server_form.is_none()));
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn compact_server_editor_uses_one_scroller_and_reveals_fields_while_footer_stays_fixed(
        cx: &mut TestAppContext,
    ) {
        let (form, cx) = editor(cx, false);
        let footer = cx.debug_bounds("server-editor-footer").unwrap();
        cx.update(|window, _| {
            let snapshots = gpui_kit::base::test_support::snapshots(window);
            for label in ["Name (optional)", "SSH host", "SSH port", "Username"] {
                let field = snapshots
                    .iter()
                    .find(|e| e.label() == Some(label) && e.role() == Some(gpui::Role::TextInput))
                    .unwrap();
                assert_eq!(field.bounds().size.height, px(34.), "{label}");
            }
            for label in ["Cancel", "Save server"] {
                let button = snapshots.iter().find(|e| e.label() == Some(label)).unwrap();
                assert_eq!(button.bounds().size.height, px(34.), "{label}");
            }
            let close = snapshots
                .iter()
                .find(|e| e.label() == Some("Close dialog"))
                .unwrap();
            assert_eq!(close.bounds().size, gpui::size(px(28.), px(28.)));
        });
        assert!(form.read_with(cx, |form, _| form.scroll.handle.max_offset().y) > px(0.));
        visible(&form, 1, cx);
        // Name → host → port → user → auth → SSH key → refresh.
        for row in [2, 2, 3, 5, 6, 7] {
            key(cx, "tab");
            visible(&form, row, cx);
            assert_eq!(cx.debug_bounds("server-editor-footer").unwrap(), footer);
        }
        cx.update(|window, _| {
            let snapshots = gpui_kit::base::test_support::snapshots(window);
            let refresh = snapshots
                .iter()
                .find(|e| e.label() == Some("Refresh agent keys"))
                .unwrap();
            assert_eq!(refresh.bounds().size.height, px(34.));
        });
        for row in [6, 5, 3, 2, 2, 1] {
            key(cx, "shift-tab");
            visible(&form, row, cx);
        }
        key(cx, "escape");
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn refreshing_keys_keeps_button_and_form_geometry(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        cx.simulate_resize(gpui::size(px(1000.), px(900.)));
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.key_revision = Some(std::time::Instant::now());
                form.file_mode = true;
                form.keys = vec![json!({"source":"system", "fingerprint":"SHA256:fixture", "comment":"Fixture key"})];
                form.sync_key_choices(window, cx);
                cx.notify();
            });
        });
        draw(cx);
        let key_row = cx.debug_bounds("form-scroll-row-6").unwrap();
        let refresh_row = cx.debug_bounds("form-scroll-row-7").unwrap();
        let identity_row = cx.debug_bounds("form-scroll-row-9").unwrap();
        let button = cx.update(|window, _| {
            gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .find(|element| element.label() == Some("Refresh agent keys"))
                .unwrap()
                .bounds()
        });
        form.update(cx, |form, cx| {
            form.keys_loading = true;
            cx.notify();
        });
        draw(cx);
        assert_eq!(cx.debug_bounds("form-scroll-row-6").unwrap(), key_row);
        assert_eq!(cx.debug_bounds("form-scroll-row-7").unwrap(), refresh_row);
        assert_eq!(cx.debug_bounds("form-scroll-row-9").unwrap(), identity_row);
        cx.update(|window, _| {
            let snapshots = gpui_kit::base::test_support::snapshots(window);
            let refresh = snapshots
                .iter()
                .find(|element| element.label() == Some("Refresh agent keys"))
                .unwrap();
            assert_eq!(refresh.bounds(), button);
        });
        assert_eq!(
            form.read_with(cx, |form, cx| form.key.read(cx).selected_value().cloned()),
            Some("file".to_owned())
        );
    }

    #[gpui::test]
    fn compact_select_triggers_keep_full_height_menu_rows(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        cx.simulate_resize(gpui::size(px(1000.), px(900.)));
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.keys = (0..4)
                    .map(|index| {
                        json!({
                            "source":"onePassword", "fingerprint":format!("SHA256:key-{index}"),
                            "comment":format!("SSH key {index}"), "algorithm":"ed25519"
                        })
                    })
                    .collect();
                form.sync_key_choices(window, cx);
            });
        });
        draw(cx);
        for (label, count) in [("Authentication", 2), ("SSH key", 6)] {
            click(cx, label);
            cx.update(|window, _| {
                let snapshots = gpui_kit::base::test_support::snapshots(window);
                let trigger = snapshots.iter().find(|e| e.label() == Some(label)).unwrap();
                assert_eq!(trigger.bounds().size.height, px(34.));
                let options: Vec<_> = snapshots
                    .iter()
                    .filter(|e| e.role() == Some(gpui::Role::ListItem) && e.visible())
                    .collect();
                assert_eq!(options.len(), count);
                for option in options {
                    assert!(
                        option.bounds().size.height >= px(28.),
                        "row is too small: {:?}",
                        option.bounds()
                    );
                }
            });
            key(cx, "escape");
        }
    }

    #[gpui::test]
    fn server_authentication_keyboard_changes_to_password_without_submitting(
        cx: &mut TestAppContext,
    ) {
        let (form, cx) = editor(cx, false);
        for _ in 0..4 {
            key(cx, "tab");
        }
        key(cx, "enter");
        key(cx, "down");
        key(cx, "enter");
        assert_eq!(
            form.read_with(cx, |form, _| form.value["authMethod"].clone()),
            json!("password")
        );
        cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert!(!form.read_with(cx, |form, _| form.pending));
        key(cx, "tab");
        visible(&form, 6, cx);
        click(cx, "Save server");
        assert_eq!(
            form.read_with(cx, |form, _| form.error.as_ref().unwrap().input),
            Some(PASSWORD)
        );
        visible(&form, 6, cx);
        cx.update(|window, cx| {
            assert!(
                form.read(cx).fields[PASSWORD]
                    .focus_handle(cx)
                    .is_focused(window)
            )
        });
    }

    #[gpui::test]
    fn server_editor_keeps_optional_name_password_bytes_and_hidden_connection_settings(
        cx: &mut TestAppContext,
    ) {
        let (form, cx) = editor(cx, true);
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.value["authMethod"] = json!("password");
                let (value, password) = form
                    .draft(cx)
                    .unwrap_or_else(|_| panic!("blank existing password should be allowed"));
                assert_eq!(value["name"], json!(""));
                assert_eq!(value["clipboardEnabled"], true);
                assert_eq!(password, Value::Null);
                form.fields[PASSWORD]
                    .update(cx, |input, cx| input.set_value("  secret  ", window, cx));
                let (_, password) = form
                    .draft(cx)
                    .unwrap_or_else(|_| panic!("password should be accepted"));
                assert_eq!(password, json!("  secret  "));
                form.fields[HOST]
                    .update(cx, |input, cx| input.set_value(" host.test ", window, cx));
                assert_eq!(
                    form.draft(cx)
                        .unwrap_or_else(|_| panic!("host should trim"))
                        .0["sshHost"],
                    json!("host.test")
                );
            });
        });
    }

    #[gpui::test]
    fn saved_unavailable_keys_survive_refresh_and_switching_key_modes_clears_stale_identity(
        cx: &mut TestAppContext,
    ) {
        let (form, cx) = editor(cx, true);
        cx.update(|window,cx| form.update(cx,|form,cx| {
            form.value["agentSource"]=json!("onePassword");
            form.value["agentKeyFingerprint"]=json!("SHA256:saved");
            form.sync_key_choices(window,cx);
            assert_eq!(form.key.read(cx).selected_value().map(String::as_str),Some("onePassword:SHA256:saved"));
            assert!(key_choices(&form.value,&[],false).iter().any(|choice|choice.title.contains("not currently available")));
            form.select_key("onePassword:SHA256:saved",window,cx);
            assert_eq!(form.value["agentKeyFingerprint"],json!("SHA256:saved"));
            form.select_key("file",window,cx);
            assert!(form.file_mode);
            assert_eq!(form.value["agentKeyFingerprint"],Value::Null);
            form.fields[IDENTITY].update(cx,|input,cx|input.set_value(" ~/.ssh/id_ed25519 ",window,cx));
            assert_eq!(form.draft(cx).unwrap_or_else(|_|panic!("file should be accepted")).0["identityFile"],json!("~/.ssh/id_ed25519"));
            form.select_key("automatic",window,cx);
            assert_eq!(form.fields[IDENTITY].read(cx).value(),"");
            assert_eq!(form.value["identityFile"],Value::Null);
            assert_eq!(form.value["agentSource"],Value::Null);
            form.keys=vec![json!({"source":"onePassword","fingerprint":"SHA256:available","comment":"Laptop","algorithm":"ed25519"})];
            form.select_key("onePassword:SHA256:available",window,cx);
            assert_eq!(form.value["agentSource"],json!("onePassword"));
            assert_eq!(form.value["agentKeyFingerprint"],json!("SHA256:available"));
        }));
    }

    #[gpui::test]
    fn invalid_port_and_general_error_stay_in_modal_and_pending_blocks_dismissal(
        cx: &mut TestAppContext,
    ) {
        let (form, cx) = editor(cx, false);
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.fields[PORT].update(cx, |input, cx| input.set_value("65536", window, cx));
                form.save(window, cx);
            })
        });
        draw(cx);
        visible(&form, 2, cx);
        assert_eq!(
            form.read_with(cx, |form, _| form.error.as_ref().unwrap().input),
            Some(PORT)
        );
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.show_error(FormError::general("The vault is locked."), window, cx)
            })
        });
        draw(cx);
        visible(&form, 9, cx);
        form.update(cx, |form, cx| {
            form.pending = true;
            cx.notify();
        });
        draw(cx);
        key(cx, "escape");
        cx.simulate_click(gpui::point(px(4.), px(4.)), Default::default());
        click(cx, "Cancel");
        cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
        form.update(cx, |form, cx| {
            form.pending = false;
            cx.notify();
        });
        draw(cx);
        click(cx, "Cancel");
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn roomy_server_editor_is_content_sized_and_identity_path_is_required(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        cx.simulate_resize(gpui::size(px(1000.), px(1000.)));
        cx.update(|window, cx| form.update(cx, |form, cx| form.select_key("file", window, cx)));
        draw(cx);
        assert_eq!(cx.debug_bounds("dialog-0").unwrap().size.width, px(480.));
        assert_eq!(
            form.read_with(cx, |form, _| form.scroll.handle.max_offset().y),
            px(0.)
        );
        click(cx, "Save server");
        assert_eq!(
            form.read_with(cx, |form, _| form.error.as_ref().unwrap().input),
            Some(IDENTITY)
        );
        visible(&form, 9, cx);
    }
}
