//! Server editor presentation; agent discovery and persistence stay in the backend.
use super::*;
use gpui_kit::component::{
    input::InputEvent,
    scroll::ScrollableElement as _,
    select::{Select, SelectEvent, SelectItem, SelectState},
    spinner::Spinner,
};

const KEYS: &str = "server-form-agent-keys";
const LOOKUP: &str = "server-form-ssh-lookup";
const NAME: usize = 0;
const HOST: usize = 1;
const PORT: usize = 2;
const USER: usize = 3;
const IDENTITY: usize = 4;
const PASSWORD: usize = 5;
const PROXY: usize = 6;
const NO_JUMP_HOSTS: &str = "Add the jump host as a server first, then choose it here.";

/// Where in the connection settings a failed connection is likely fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fix {
    Address,
    SignIn,
    /// How the server is reached: its jump host or proxy command.
    Proxy,
}

/// How the server is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Direct,
    Jump,
    Command,
}
impl Route {
    fn of(server: &configuration::Server) -> Self {
        match server.proxy {
            Some(configuration::Proxy::Jump(_)) => Self::Jump,
            Some(configuration::Proxy::Command(_)) => Self::Command,
            None => Self::Direct,
        }
    }
    fn value(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Jump => "jump",
            Self::Command => "command",
        }
    }
}

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
    value: configuration::Server,
    fields: Vec<Entity<InputState>>,
    authentication: Entity<ChoiceState>,
    key: Entity<ChoiceState>,
    route: Route,
    routes: Entity<ChoiceState>,
    /// The saved servers this one may connect through.
    jump_hosts: Entity<ChoiceState>,
    any_jump_hosts: bool,
    jump: Option<String>,
    keys: Vec<desktop::connection::AgentKey>,
    keys_loading: bool,
    key_revision: Option<Instant>,
    file_mode: bool,
    existing: bool,
    /// Fields the person typed in. The SSH config never changes them.
    typed: [bool; 7],
    /// The person chose how the server is reached.
    route_chosen: bool,
    /// The host last looked up in the SSH config, and when its reply came.
    lookup: Option<String>,
    lookup_revision: Option<Instant>,
    /// What the SSH config filled in, shown under the host.
    lookup_note: Option<String>,
    /// The field that has focus when the form opens.
    first: usize,
    pending: bool,
    error: Option<FormError>,
    scroll: modal::FormScroll,
    _subscriptions: Vec<Subscription>,
}

impl Newport {
    pub(super) fn edit_server(
        &mut self,
        value: Option<configuration::Server>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_server_at(value, None, window, cx);
    }
    /// Opens the server's settings at the field `fix` points to.
    pub(super) fn edit_server_at(
        &mut self,
        value: Option<configuration::Server>,
        fix: Option<Fix>,
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
                .get(&format!("server-delete:{}", server.id))
                .is_some_and(|r| r.pending)
        }) {
            return;
        }
        self.server_picker.update(cx, |picker, cx| picker.close(cx));
        let value = value.unwrap_or_else(|| configuration::Server {
            id: uuid::Uuid::new_v4(),
            name: String::new(),
            ssh_user: String::new(),
            ssh_host: String::new(),
            ssh_port: 22,
            identity_file: None,
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: configuration::AuthMethod::PublicKey,
            clipboard_enabled: false,
            browser_enabled: false,
            agent_install_approved: false,
            setup_complete: false,
            proxy: None,
        });
        self.resources.invalidate("server-save");
        self.resources.invalidate(KEYS);
        if value.auth_method != configuration::AuthMethod::Password {
            self.request_typed(
                KEYS,
                Ok(desktop::Operation::Connection(Box::new(
                    desktop::connection::Operation::SshAgentKeys(Box::new(
                        newport_protocol::common::Present {},
                    )),
                ))),
            );
        }
        let owner = cx.weak_entity();
        let servers = self.servers().to_vec();
        let form = cx.new(|cx| {
            let mut form = ServerForm::new(owner, value, existing, &servers, window, cx);
            form.first = match fix {
                Some(Fix::Address) => HOST,
                Some(Fix::SignIn) => USER,
                Some(Fix::Proxy) => PROXY,
                None => NAME,
            };
            form
        });
        self.server_form = Some(form.clone());
        ServerForm::open(form, window, cx);
        self.notice = None;
        cx.notify();
    }
}

impl ServerForm {
    fn new(
        owner: WeakEntity<Newport>,
        value: configuration::Server,
        existing: bool,
        servers: &[configuration::Server],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = [
            value.name.clone(),
            value.ssh_host.clone(),
            value.ssh_port.to_string(),
            value.ssh_user.clone(),
            value.identity_file.clone().unwrap_or_default(),
            String::new(),
            match &value.proxy {
                Some(configuration::Proxy::Command(command)) => command.to_string(),
                _ => String::new(),
            },
        ];
        let fields: Vec<_> = initial
            .clone()
            .into_iter()
            .zip([
                "Development server",
                "server.example.com",
                "22",
                "username",
                "~/.ssh/id_ed25519",
                "",
                "cloudflared access ssh --hostname %h",
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
            state.set_selected_value(&auth_choice(value.auth_method).to_owned(), window, cx);
            state
        });
        let file_mode = value
            .identity_file
            .as_deref()
            .is_some_and(|s| !s.is_empty());
        let keys_loading = value.auth_method != configuration::AuthMethod::Password;
        let selected = selected_key(&value, file_mode);
        let key = cx.new(|cx| {
            let mut state =
                SelectState::new(key_choices(&value, &[], keys_loading), None, window, cx);
            state.set_selected_value(&selected, window, cx);
            state
        });
        let route = Route::of(&value);
        let routes = cx.new(|cx| {
            let mut state = SelectState::new(
                vec![
                    Choice::new(Route::Direct.value(), "Direct"),
                    Choice::new(Route::Jump.value(), "Jump host"),
                    Choice::new(Route::Command.value(), "Proxy command"),
                ],
                None,
                window,
                cx,
            );
            state.set_selected_value(&route.value().to_owned(), window, cx);
            state
        });
        let choices = jump_choices(servers, value.id);
        let any_jump_hosts = !choices.is_empty();
        // A saved jump host that can't be chosen any more is chosen again.
        let jump = match &value.proxy {
            Some(configuration::Proxy::Jump(saved)) => choices
                .iter()
                .find(|choice| choice.value == saved.to_string())
                .map(|choice| choice.value.clone()),
            _ => choices.first().map(|choice| choice.value.clone()),
        };
        let jump_hosts = cx.new(|cx| {
            let mut state = SelectState::new(choices, None, window, cx);
            if let Some(jump) = &jump {
                state.set_selected_value(jump, window, cx);
            }
            state
        });
        let mut subscriptions: Vec<_> = fields
            .iter()
            .enumerate()
            .map(|(index, input)| {
                cx.subscribe_in(
                    input,
                    window,
                    move |this: &mut Self, _, event, window, cx| {
                        match event {
                            InputEvent::PressEnter { .. } => this.save(window, cx),
                            // Only typing changes a field this way; values the
                            // form sets itself emit nothing.
                            InputEvent::Change => this.typed[index] = true,
                            InputEvent::Blur if index == HOST => this.look_up(cx),
                            _ => {}
                        }
                    },
                )
            })
            .collect();
        subscriptions.push(
            cx.subscribe_in(&authentication, window, |this, _, event, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    if this.pending {
                        return;
                    }
                    this.value.auth_method = if value == "password" {
                        configuration::AuthMethod::Password
                    } else {
                        configuration::AuthMethod::PublicKey
                    };
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
        subscriptions.push(
            cx.subscribe_in(&routes, window, |this, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    this.route_chosen = true;
                    this.select_route(value, window, cx);
                }
            }),
        );
        subscriptions.push(
            cx.subscribe_in(&jump_hosts, window, |this, _, event, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    if this.pending {
                        return;
                    }
                    this.jump = Some(value.clone());
                    this.error = None;
                    cx.notify();
                }
            }),
        );
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
            route,
            routes,
            jump_hosts,
            any_jump_hosts,
            jump,
            keys: vec![],
            keys_loading,
            key_revision: None,
            file_mode,
            existing,
            // A saved server's own values count as chosen.
            typed: std::array::from_fn(|index| {
                existing && !initial[index].is_empty() && index != PASSWORD
            }),
            route_chosen: existing,
            lookup: None,
            lookup_revision: None,
            lookup_note: None,
            first: NAME,
            pending: false,
            error: None,
            scroll: modal::FormScroll::new(11, window, cx),
            _subscriptions: subscriptions,
        }
    }

    fn open(form: Entity<Self>, window: &mut Window, cx: &mut App) {
        let focus = {
            let form = form.read(cx);
            match (form.first, form.route) {
                (PROXY, Route::Jump) => form.jump_hosts.focus_handle(cx),
                (PROXY, Route::Direct) => form.routes.focus_handle(cx),
                (first, _) => form.fields[first].focus_handle(cx),
            }
        };
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
                            crate::controls::dialog_title(title)
                                .id("server-editor-title")
                                .role(accesskit::Role::Heading)
                                .aria_label(title),
                        )
                        .child(
                            crate::controls::icon_button("close-server-editor")
                                .ghost()
                                .icon(IconName::Close)
                                .text_color(cx.theme().muted_foreground)
                                .accessibility_label("Close dialog")
                                .tooltip("Close dialog")
                                .disabled(pending)
                                .on_click(move |_, window, cx| Self::close(&close, window, cx)),
                        ),
                )
                .w(px(480.))
                .p_0()
                .gap_0()
                .rounded(px(crate::ui::RADIUS))
                .border_0()
                .centered()
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
            .map(|r| {
                let keys = match owner.connection_reply(KEYS) {
                    Some(desktop::connection::Reply::SshAgentKeys(list)) => list.keys.clone(),
                    _ => Vec::new(),
                };
                (keys, r.error.clone(), r.finished_at)
            });
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
        let lookup_result = owner
            .resources
            .get(LOOKUP)
            .filter(|r| !r.pending && r.finished_at != self.lookup_revision)
            .map(|r| {
                let resolved = match owner.connection_reply(LOOKUP) {
                    Some(desktop::connection::Reply::SshResolve(resolved)) => (**resolved).clone(),
                    _ => Default::default(),
                };
                (resolved, r.error.is_none(), r.finished_at)
            });
        if let Some((value, ok, revision)) = lookup_result {
            self.lookup_revision = revision;
            if ok {
                self.fill_from_config(&value, window, cx);
            }
        }
        if let Some((keys, error, revision)) = key_result {
            self.key_revision = revision;
            self.keys_loading = false;
            if error.is_none() {
                self.keys = keys;
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

    /// Asks OpenSSH which settings the SSH config gives the host, once per
    /// host the field holds.
    fn look_up(&mut self, cx: &mut Context<Self>) {
        let host = self.fields[HOST].read(cx).value().trim().to_owned();
        if self.pending || host.is_empty() || self.lookup.as_ref() == Some(&host) {
            return;
        }
        let request = host.clone();
        let result = self.owner.update(cx, |owner, cx| {
            owner.request_typed(
                LOOKUP,
                Ok(desktop::Operation::Connection(Box::new(
                    desktop::connection::Operation::SshResolve(Box::new(
                        desktop::connection::SshResolveArgs { host: request },
                    )),
                ))),
            );
            cx.notify();
        });
        if result.is_ok() {
            self.lookup = Some(host);
        }
    }

    /// Fills the fields the person hasn't typed in with what the SSH config
    /// sets, for the host the field still holds.
    fn fill_from_config(
        &mut self,
        value: &desktop::connection::SshResolved,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let host = self.fields[HOST].read(cx).value().trim().to_owned();
        if self.pending || self.lookup.as_ref() != Some(&host) {
            return;
        }
        let mut filled = Vec::new();
        let set = |this: &mut Self, index: usize, text: &str, window: &mut Window, cx: &mut App| {
            this.fields[index].update(cx, |input, cx| input.set_value(text, window, cx));
        };
        if let Some(hostname) = value.hostname.as_deref().filter(|_| !self.typed[HOST]) {
            if !self.typed[NAME] && self.fields[NAME].read(cx).value().trim().is_empty() {
                set(self, NAME, &host, window, cx);
            }
            set(self, HOST, hostname, window, cx);
            // The address is the config's own answer; nothing more to ask.
            self.lookup = Some(hostname.to_owned());
            filled.push("host");
        }
        if let Some(user) = value.user.as_deref().filter(|_| !self.typed[USER]) {
            set(self, USER, user, window, cx);
            filled.push("username");
        }
        if let Some(port) = value.port.filter(|_| !self.typed[PORT]) {
            set(self, PORT, &port.to_string(), window, cx);
            filled.push("port");
        }
        if let Some(path) = value.identity_file.as_deref().filter(|_| {
            !self.typed[IDENTITY] && self.value.auth_method != configuration::AuthMethod::Password
        }) {
            self.file_mode = true;
            self.value.agent_source = None;
            self.value.agent_key_fingerprint = None;
            set(self, IDENTITY, path, window, cx);
            self.sync_key_choices(window, cx);
            filled.push("key file");
        }
        if let Some(command) = value
            .proxy_command
            .as_deref()
            .filter(|_| !self.route_chosen && !self.typed[PROXY])
        {
            self.route = Route::Command;
            self.routes.update(cx, |routes, cx| {
                routes.set_selected_value(&Route::Command.value().to_owned(), window, cx);
            });
            set(self, PROXY, command, window, cx);
            filled.push("proxy command");
        }
        let jump = value
            .proxy_jump
            .as_deref()
            .filter(|_| !self.route_chosen)
            .map(|jump| format!(" It connects through {jump}: add it as a server and choose it as the jump host."));
        self.lookup_note = (!filled.is_empty() || jump.is_some()).then(|| {
            let mut note = if filled.is_empty() {
                String::from("Your SSH config has settings for this host.")
            } else {
                format!("Filled in from your SSH config: {}.", list(&filled))
            };
            note.push_str(jump.as_deref().unwrap_or_default());
            note
        });
        cx.notify();
    }

    fn refresh_keys(&mut self, cx: &mut Context<Self>) {
        if self.pending || self.keys_loading {
            return;
        }
        let result = self.owner.update(cx, |owner, cx| {
            owner.request_typed(
                KEYS,
                Ok(desktop::Operation::Connection(Box::new(
                    desktop::connection::Operation::SshAgentKeys(Box::new(
                        newport_protocol::common::Present {},
                    )),
                ))),
            );
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
        if self.file_mode {
            // The path field appears below; typing goes there and it scrolls into view.
            let focus = self.fields[IDENTITY].focus_handle(cx);
            window.defer(cx, move |window, cx| focus.focus(window, cx));
        } else {
            self.value.identity_file = None;
            self.fields[IDENTITY].update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.value.agent_source = selected_key.map(|key| key.source.clone());
        self.value.agent_key_fingerprint = selected_key.map(|key| key.fingerprint.clone());
        self.error = None;
        cx.notify();
    }

    fn select_route(&mut self, selected: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        self.route = match selected {
            "jump" => Route::Jump,
            "command" => Route::Command,
            _ => Route::Direct,
        };
        if self.route == Route::Command {
            // The command field appears below; typing goes there.
            let focus = self.fields[PROXY].focus_handle(cx);
            window.defer(cx, move |window, cx| focus.focus(window, cx));
        }
        self.error = None;
        cx.notify();
    }

    fn draft(
        &self,
        cx: &App,
    ) -> Result<
        (
            configuration::Server,
            Option<newport_protocol::common::Secret>,
        ),
        FormError,
    > {
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
        let password_auth = self.value.auth_method == configuration::AuthMethod::Password;
        if password_auth && !self.existing && values[PASSWORD].is_empty() {
            return Err(invalid(PASSWORD, "Enter a password."));
        }
        if !password_auth && self.file_mode && values[IDENTITY].trim().is_empty() {
            return Err(invalid(IDENTITY, "Enter an identity-file path."));
        }
        let proxy = match self.route {
            Route::Direct => None,
            Route::Jump => {
                let jump = self.jump.as_deref().ok_or_else(|| {
                    FormError::general(if self.any_jump_hosts {
                        "Choose a jump host."
                    } else {
                        NO_JUMP_HOSTS
                    })
                })?;
                let jump = uuid::Uuid::parse_str(jump)
                    .map_err(|_| FormError::general("Choose a jump host."))?;
                Some(configuration::Proxy::Jump(Box::new(jump)))
            }
            Route::Command if values[PROXY].trim().is_empty() => {
                return Err(invalid(PROXY, "Enter a proxy command."));
            }
            Route::Command => Some(configuration::Proxy::Command(Box::new(
                values[PROXY].trim().to_owned(),
            ))),
        };
        let mut value = self.value.clone();
        value.proxy = proxy;
        value.name = values[NAME].trim().to_owned();
        value.ssh_host = values[HOST].trim().to_owned();
        value.ssh_user = values[USER].trim().to_owned();
        value.ssh_port = port;
        if self.file_mode {
            let path = values[IDENTITY].trim();
            value.identity_file = (!path.is_empty()).then(|| path.to_owned());
        }
        // Password bytes are deliberately not trimmed; none keeps the saved password.
        let password = (!values[PASSWORD].is_empty()).then(|| newport_protocol::common::Secret {
            value: values[PASSWORD].clone().into_bytes().into(),
        });
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
            if self.existing && !owner.servers().iter().any(|server| server.id == self.value.id) {
                return Err("This server was removed. Cancel this editor and add a new server if needed.".to_owned());
            }
            if owner.resources.get(&format!("server-delete:{}",self.value.id)).is_some_and(|r| r.pending) { return Err("This server is being deleted.".to_owned()); }
            owner.request_typed("server-save",Ok(desktop::Operation::Configuration(Box::new(desktop::configuration::Operation::SaveServer(Box::new(desktop::configuration::SaveServerArgs { server,password }))))));
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
                PROXY => 4,
                PASSWORD => 7,
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
        if self.value.auth_method == configuration::AuthMethod::Password {
            8
        } else if self.file_mode && self.value.agent_key_fingerprint.is_none() {
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
            .bg(crate::theme::field_background(cx.theme().is_dark()))
            .border_color(cx.theme().border)
            .when(index == PROXY, |input| {
                input.font_family(cx.theme().mono_font_family.clone())
            });
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
            .bg(crate::theme::field_background(cx.theme().is_dark()))
            .border_color(cx.theme().border)
    }
    /// How the server is reached, with the jump host or command it uses.
    fn connect_through(&self, cx: &App) -> Div {
        let through = div().flex().flex_col().gap(px(14.)).child(
            field("Connect through").child(self.select(&self.routes, "Connect through", cx)),
        );
        match self.route {
            Route::Direct => through,
            Route::Jump if !self.any_jump_hosts => through.child(help(NO_JUMP_HOSTS, cx)),
            Route::Jump => through.child(
                field("Jump host")
                    .child(self.select(&self.jump_hosts, "Jump host", cx))
                    .child(help(
                        "One of your servers. Newport signs in to it first, and uses it for every server behind it.",
                        cx,
                    )),
            ),
            Route::Command => through.child(
                self.input_field(PROXY, "Proxy command", cx).child(help(
                        "Runs on this computer; SSH goes over its input and output. %h is the host, %p the port, %r the username.",
                        cx,
                    )),
            ),
        }
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
        let key_auth = self.value.auth_method != configuration::AuthMethod::Password;
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
                        .child(
                            self.input_field(HOST, "SSH host", cx)
                                .flex_1()
                                .when_some(self.lookup_note.clone(), |field, note| {
                                    field.child(help(note, cx))
                                }),
                        )
                        .child(
                            self.input_field(PORT, "SSH port", cx)
                                .w(px(95.))
                                .flex_shrink_0(),
                        ),
                ),
            )
            .child(self.scroll.row(3, self.input_field(USER, "Username", cx)))
            .child(self.scroll.row(4, self.connect_through(cx)))
            .child(self.scroll.row(
                5,
                legend("Sign in", cx).mt(px(if compact { 2. } else { 6. })),
            ))
            .child(self.scroll.row(
                6,
                field("Authentication").child(self.select(
                    &self.authentication,
                    "Authentication",
                    cx,
                )),
            ));
        if key_auth {
            let fingerprint = self.value.agent_key_fingerprint.clone();
            body = body
                .child(self.scroll.row(
                    7,
                    field("SSH key").child(self.select(&self.key, "SSH key", cx)),
                ))
                .child(
                    self.scroll.row(
                        8,
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .child(
                                        crate::controls::form_button("refresh-agent-keys")
                                            .label("Refresh agent keys")
                                            .rounded(px(crate::ui::RADIUS))
                                            .disabled(self.pending || self.keys_loading)
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.refresh_keys(cx)),
                                            ),
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
                            )
                            .when_some(fingerprint, |row, fingerprint| {
                                row.child(help(format!("Selected key: {fingerprint}"), cx))
                            }),
                    ),
                );
            if self.file_mode && self.value.agent_key_fingerprint.is_none() {
                body = body.child(self.scroll.row(
                    9,
                    self.input_field(IDENTITY, "Identity file", cx).child(help(
                        "Load encrypted keys into your system agent first.",
                        cx,
                    )),
                ));
            }
        } else {
            body = body.child(self.scroll.row(
                7,
                self.input_field(PASSWORD, "Password", cx).child(help(
                    if self.existing {
                        "Leave empty to keep the saved password."
                    } else {
                        "Saved encrypted on this computer."
                    },
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

/// The authentication choice a saved method selects.
fn auth_choice(method: configuration::AuthMethod) -> &'static str {
    match method {
        configuration::AuthMethod::PublicKey => "publicKey",
        configuration::AuthMethod::Password => "password",
    }
}
fn selected_key(value: &configuration::Server, file_mode: bool) -> String {
    value
        .agent_key_fingerprint
        .as_deref()
        .map(|fingerprint| {
            format!(
                "{}:{fingerprint}",
                value.agent_source.as_deref().unwrap_or("system")
            )
        })
        .unwrap_or_else(|| if file_mode { "file" } else { "automatic" }.into())
}
fn key_id(key: &desktop::connection::AgentKey) -> String {
    format!("{}:{}", key.source, key.fingerprint)
}
fn key_choices(
    value: &configuration::Server,
    keys: &[desktop::connection::AgentKey],
    loading: bool,
) -> Vec<Choice> {
    let mut choices = vec![
        Choice::new("automatic", "Automatic · system agent and default keys"),
        Choice::new("file", "Enter a key-file path…"),
    ];
    if value.agent_key_fingerprint.is_some()
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
                if key.source == "onePassword" {
                    "1Password"
                } else {
                    "System agent"
                },
                Some(key.comment.as_str())
                    .filter(|comment| !comment.is_empty())
                    .unwrap_or(&key.algorithm),
                key.fingerprint
            ),
        )
    }));
    choices
}
/// The saved servers `id` may connect through: all but itself and the ones
/// that already connect through it.
fn jump_choices(servers: &[configuration::Server], id: uuid::Uuid) -> Vec<Choice> {
    let mut behind = vec![id];
    let mut index = 0;
    while let Some(jump) = behind.get(index).copied() {
        for server in servers {
            if matches!(&server.proxy, Some(configuration::Proxy::Jump(through)) if **through == jump)
                && !behind.contains(&server.id)
            {
                behind.push(server.id);
            }
        }
        index += 1;
    }
    servers
        .iter()
        .filter(|server| !behind.contains(&server.id))
        .map(|server| Choice::new(server.id.to_string(), server_picker::display_name(server)))
        .collect()
}
/// "a", "a and b", "a, b and c".
fn list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn compact(window: &Window) -> bool {
    window.viewport_size().height <= px(560.) || window.viewport_size().width <= px(540.)
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
fn help(value: impl Into<String>, cx: &App) -> Div {
    div()
        .text_size(px(crate::ui::FONT_SIZE))
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
    use super::{
        FormError, HOST, IDENTITY, PASSWORD, PORT, PROXY, Route, ServerForm, USER, jump_choices,
        key_choices,
    };
    use desktop::connection::{AgentKey, SshResolved};
    use gpui::{
        AppContext, Context, Entity, Focusable, IntoElement, Render, Styled, WeakEntity, Window,
        div, px,
    };
    use gpui::{KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext};
    use gpui_kit as gpui;
    use gpui_kit::component::WindowExt as _;
    use newport_protocol::desktop;
    use newport_protocol::desktop::configuration::{AuthMethod, Proxy, Server};
    /// An agent key with no comment of its own unless `comment` names one.
    fn agent_key(source: &str, fingerprint: &str, comment: &str) -> AgentKey {
        AgentKey {
            source: source.into(),
            comment: comment.into(),
            fingerprint: fingerprint.into(),
            algorithm: "ed25519".into(),
        }
    }

    /// A saved server named `name` with this id, reached directly or as `proxy` says.
    fn saved(id: uuid::Uuid, name: &str, proxy: Option<Proxy>) -> Server {
        Server {
            proxy,
            ..crate::snapshot_fixture::server(id, name)
        }
    }
    fn jump(id: uuid::Uuid) -> Option<Proxy> {
        Some(Proxy::Jump(Box::new(id)))
    }
    fn command(command: &str) -> Option<Proxy> {
        Some(Proxy::Command(Box::new(command.into())))
    }

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
        editor_with(cx, existing, None, vec![])
    }
    fn editor_with(
        cx: &mut TestAppContext,
        existing: bool,
        proxy: Option<Proxy>,
        servers: Vec<Server>,
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
        let form = cx.update(|window, cx| {
            window.activate_window();
            let value = Server {
                id: uuid::Uuid::new_v4(),
                name: String::new(),
                ssh_user: "developer".into(),
                ssh_host: "localhost".into(),
                ssh_port: 22,
                identity_file: None,
                agent_source: None,
                agent_key_fingerprint: None,
                auth_method: AuthMethod::PublicKey,
                clipboard_enabled: true,
                browser_enabled: false,
                agent_install_approved: false,
                setup_complete: false,
                proxy,
            };
            let form = cx.new(|cx| {
                ServerForm::new(
                    WeakEntity::new_invalid(),
                    value,
                    existing,
                    &servers,
                    window,
                    cx,
                )
            });
            form.update(cx, |form, cx| {
                form.keys_loading = false;
                form.sync_key_choices(window, cx);
            });
            ServerForm::open(form.clone(), window, cx);
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
    fn key(cx: &mut VisualTestContext, value: &'static str) {
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
        let crate::request_dispatch::Request::Typed(operation) = &save.request else {
            panic!("desktop save");
        };
        let desktop::Operation::Configuration(operation) = operation.as_ref() else {
            panic!("configuration save");
        };
        let desktop::configuration::Operation::SaveServer(args) = operation.as_ref() else {
            panic!("server save");
        };
        let saved = args.server.clone();
        assert!(saved.proxy.is_none());
        owner.update(cx, |owner, cx| {
            let generation = owner.resources.begin("snapshot").unwrap();
            let mut snapshot = owner.snapshot.clone().unwrap();
            snapshot.config.servers.push(saved.clone());
            replies
                .send(crate::Completion {
                    key: "server-save".into(),
                    generation: save.generation,
                    result: crate::Outcome::Typed(Ok(desktop::Reply::Configuration(Box::new(
                        desktop::configuration::Reply::SaveServer(Box::new(
                            newport_protocol::common::Present {},
                        )),
                    )))),
                    server: saved.id.to_string(),
                })
                .unwrap();
            replies
                .send(crate::snapshot_fixture::completion(generation, snapshot))
                .unwrap();
            // Both replies are drained before GPUI delivers the owner observer.
            owner.tick(cx);
            assert_eq!(owner.server, saved.id.to_string());
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
                assert_eq!(
                    crate::field_frame(window, field).size.height,
                    px(34.),
                    "{label}"
                );
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
        // Name → host → port → user → connect through → auth → SSH key → refresh.
        for row in [2, 2, 3, 4, 6, 7, 8] {
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
        for row in [7, 6, 4, 3, 2, 2, 1] {
            key(cx, "shift-tab");
            visible(&form, row, cx);
        }
        key(cx, "escape");
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui::test]
    fn a_failed_sign_in_opens_the_settings_at_the_username(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut owner = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| crate::page_perf_tests::owner(window, cx));
            owner = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = owner.unwrap();
        cx.update(|window, cx| {
            window.activate_window();
            view.update(cx, |v, cx| {
                let server = v.selected_server().cloned();
                v.edit_server_at(server, Some(super::Fix::SignIn), window, cx);
            });
        });
        draw(cx);
        let focused = cx.update(|window, cx| {
            let form = view.read(cx).server_form.clone().unwrap();
            form.read(cx).fields[USER]
                .focus_handle(cx)
                .is_focused(window)
        });
        assert!(focused);
    }

    #[gpui::test]
    fn choosing_a_key_file_moves_to_its_path_field(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        visible(&form, 1, cx);
        form.update_in(cx, |form, window, cx| form.select_key("file", window, cx));
        draw(cx);
        draw(cx);
        visible(&form, 9, cx);
        let focused = form.update_in(cx, |form, window, cx| {
            form.fields[IDENTITY].focus_handle(cx).is_focused(window)
        });
        assert!(focused);
    }

    #[gpui::test]
    fn refreshing_keys_keeps_button_and_form_geometry(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        cx.simulate_resize(gpui::size(px(1000.), px(900.)));
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.key_revision = Some(std::time::Instant::now());
                form.file_mode = true;
                form.keys = vec![agent_key("system", "SHA256:fixture", "Fixture key")];
                form.sync_key_choices(window, cx);
                cx.notify();
            });
        });
        draw(cx);
        let key_row = cx.debug_bounds("form-scroll-row-7").unwrap();
        let refresh_row = cx.debug_bounds("form-scroll-row-8").unwrap();
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
        assert_eq!(cx.debug_bounds("form-scroll-row-7").unwrap(), key_row);
        assert_eq!(cx.debug_bounds("form-scroll-row-8").unwrap(), refresh_row);
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

    fn deliver(
        form: &Entity<ServerForm>,
        cx: &mut VisualTestContext,
        host: &str,
        looked_up: &str,
        reply: SshResolved,
    ) {
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.fields[HOST].update(cx, |input, cx| input.set_value(host, window, cx));
                form.lookup = Some(looked_up.into());
                form.fill_from_config(&reply, window, cx);
            });
        });
        draw(cx);
    }
    fn field(form: &Entity<ServerForm>, cx: &mut VisualTestContext, index: usize) -> String {
        form.read_with(cx, |form, cx| {
            form.fields[index].read(cx).value().to_string()
        })
    }

    #[gpui::test]
    fn the_ssh_config_fills_only_the_fields_the_person_didnt_type(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        form.update(cx, |form, _| form.typed[USER] = true);
        let command = "gateway ssh --stdio --suffix gw %h";
        deliver(
            &form,
            cx,
            "app.box.me.gw",
            "app.box.me.gw",
            SshResolved {
                user: Some("ops".into()),
                port: Some(2222),
                proxy_command: Some(command.into()),
                ..Default::default()
            },
        );
        assert_eq!(field(&form, cx, USER), "developer");
        assert_eq!(field(&form, cx, PORT), "2222");
        assert_eq!(field(&form, cx, PROXY), command);
        form.read_with(cx, |form, cx| {
            assert_eq!(form.route, Route::Command);
            assert_eq!(
                form.lookup_note.as_deref(),
                Some("Filled in from your SSH config: port and proxy command.")
            );
            let Ok((server, _)) = form.draft(cx) else {
                panic!("the filled-in form saves");
            };
            assert_eq!(server.proxy, super::tests::command(command));
            assert_eq!(server.ssh_port, 2222);
        });
    }

    #[gpui::test]
    fn a_reply_for_another_host_or_a_saved_servers_values_are_left_alone(cx: &mut TestAppContext) {
        let reply = SshResolved {
            user: Some("ops".into()),
            port: Some(2222),
            proxy_command: Some("gateway %h".into()),
            ..Default::default()
        };
        let (form, visual) = editor(cx, false);
        deliver(
            &form,
            visual,
            "app.box.me.gw",
            "earlier.example",
            reply.clone(),
        );
        assert_eq!(field(&form, visual, PORT), "22");
        form.read_with(visual, |form, _| assert!(form.lookup_note.is_none()));

        let (form, visual) = editor(cx, true);
        deliver(&form, visual, "localhost", "localhost", reply);
        assert_eq!(field(&form, visual, USER), "developer");
        assert_eq!(field(&form, visual, PORT), "22");
        form.read_with(visual, |form, _| {
            assert_eq!(form.route, Route::Direct);
            assert!(form.lookup_note.is_none());
        });
    }

    #[gpui::test]
    fn an_alias_takes_its_host_name_and_keeps_the_alias_as_the_name(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        deliver(
            &form,
            cx,
            "box",
            "box",
            SshResolved {
                hostname: Some("10.0.0.5".into()),
                proxy_jump: Some("bastion".into()),
                ..Default::default()
            },
        );
        assert_eq!(field(&form, cx, HOST), "10.0.0.5");
        assert_eq!(field(&form, cx, super::NAME), "box");
        form.read_with(cx, |form, _| {
            assert_eq!(form.lookup.as_deref(), Some("10.0.0.5"));
            let note = form.lookup_note.as_deref().unwrap();
            assert!(
                note.starts_with("Filled in from your SSH config: host."),
                "{note}"
            );
            assert!(note.contains("connects through bastion"), "{note}");
        });
    }

    #[gpui::test]
    fn loading_agent_keys_keeps_form_geometry(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        cx.simulate_resize(gpui::size(px(1000.), px(900.)));
        form.update(cx, |form, cx| {
            form.keys_loading = true;
            cx.notify();
        });
        draw(cx);
        let refresh_row = cx.debug_bounds("form-scroll-row-8").unwrap();
        let footer = cx.debug_bounds("server-editor-footer").unwrap();
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.keys_loading = false;
                form.key_revision = Some(std::time::Instant::now());
                form.keys = vec![agent_key("system", "SHA256:fixture", "Fixture key")];
                form.sync_key_choices(window, cx);
                cx.notify();
            });
        });
        draw(cx);
        assert_eq!(cx.debug_bounds("form-scroll-row-8").unwrap(), refresh_row);
        assert_eq!(cx.debug_bounds("server-editor-footer").unwrap(), footer);
    }

    #[gpui::test]
    fn compact_select_triggers_keep_full_height_menu_rows(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        cx.simulate_resize(gpui::size(px(1000.), px(900.)));
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.keys = (0..4)
                    .map(|index| {
                        agent_key(
                            "onePassword",
                            &format!("SHA256:key-{index}"),
                            &format!("SSH key {index}"),
                        )
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
        // Past the name, host, port, username and Connect through.
        for _ in 0..5 {
            key(cx, "tab");
        }
        key(cx, "enter");
        key(cx, "down");
        key(cx, "enter");
        assert_eq!(
            form.read_with(cx, |form, _| form.value.auth_method),
            AuthMethod::Password
        );
        cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert!(!form.read_with(cx, |form, _| form.pending));
        key(cx, "tab");
        visible(&form, 7, cx);
        click(cx, "Save server");
        assert_eq!(
            form.read_with(cx, |form, _| form.error.as_ref().unwrap().input),
            Some(PASSWORD)
        );
        visible(&form, 7, cx);
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
                form.value.auth_method = AuthMethod::Password;
                let (value, password) = form
                    .draft(cx)
                    .unwrap_or_else(|_| panic!("blank existing password should be allowed"));
                assert_eq!(value.name, "");
                assert!(value.clipboard_enabled);
                assert!(password.is_none());
                form.fields[PASSWORD]
                    .update(cx, |input, cx| input.set_value("  secret  ", window, cx));
                let (_, password) = form
                    .draft(cx)
                    .unwrap_or_else(|_| panic!("password should be accepted"));
                assert_eq!(
                    password.map(|password| password.value.to_vec()),
                    Some(b"  secret  ".to_vec())
                );
                form.fields[HOST]
                    .update(cx, |input, cx| input.set_value(" host.test ", window, cx));
                assert_eq!(
                    form.draft(cx)
                        .unwrap_or_else(|_| panic!("host should trim"))
                        .0
                        .ssh_host,
                    "host.test"
                );
            });
        });
    }

    #[gpui::test]
    fn saved_unavailable_keys_survive_refresh_and_switching_key_modes_clears_stale_identity(
        cx: &mut TestAppContext,
    ) {
        let (form, cx) = editor(cx, true);
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                form.value.agent_source = Some("onePassword".into());
                form.value.agent_key_fingerprint = Some("SHA256:saved".into());
                form.sync_key_choices(window, cx);
                assert_eq!(
                    form.key.read(cx).selected_value().map(String::as_str),
                    Some("onePassword:SHA256:saved")
                );
                assert!(
                    key_choices(&form.value, &[], false)
                        .iter()
                        .any(|choice| choice.title.contains("not currently available"))
                );
                form.select_key("onePassword:SHA256:saved", window, cx);
                assert_eq!(
                    form.value.agent_key_fingerprint.as_deref(),
                    Some("SHA256:saved")
                );
                form.select_key("file", window, cx);
                assert!(form.file_mode);
                assert_eq!(form.value.agent_key_fingerprint, None);
                form.fields[IDENTITY].update(cx, |input, cx| {
                    input.set_value(" ~/.ssh/id_ed25519 ", window, cx)
                });
                assert_eq!(
                    form.draft(cx)
                        .unwrap_or_else(|_| panic!("file should be accepted"))
                        .0
                        .identity_file
                        .as_deref(),
                    Some("~/.ssh/id_ed25519")
                );
                form.select_key("automatic", window, cx);
                assert_eq!(form.fields[IDENTITY].read(cx).value(), "");
                assert_eq!(form.value.identity_file, None);
                assert_eq!(form.value.agent_source, None);
                form.keys = vec![agent_key("onePassword", "SHA256:available", "Laptop")];
                form.select_key("onePassword:SHA256:available", window, cx);
                assert_eq!(form.value.agent_source.as_deref(), Some("onePassword"));
                assert_eq!(
                    form.value.agent_key_fingerprint.as_deref(),
                    Some("SHA256:available")
                );
            })
        });
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
                form.show_error(FormError::general("Saved profiles are locked."), window, cx)
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

    #[gpui::test]
    fn connect_through_saves_direct_a_jump_host_or_a_command(cx: &mut TestAppContext) {
        let (form, cx) = editor(cx, false);
        cx.update(|window, cx| {
            form.update(cx, |form, cx| {
                assert_eq!(form.route, Route::Direct);
                let (value, _) = form.draft(cx).unwrap_or_else(|_| panic!("direct saves"));
                assert_eq!(value.proxy, None);

                form.select_route("command", window, cx);
                assert_eq!(form.draft(cx).err().and_then(|e| e.input), Some(PROXY));
                form.fields[PROXY]
                    .update(cx, |input, cx| input.set_value(" nc %h %p ", window, cx));
                let (value, _) = form.draft(cx).unwrap_or_else(|_| panic!("command saves"));
                assert_eq!(value.proxy, command("nc %h %p"));

                // No other server is saved, so there is no jump host to choose.
                form.select_route("jump", window, cx);
                let error = form.draft(cx).expect_err("a jump host is needed");
                assert_eq!(error.input, None);
                assert!(
                    error
                        .message
                        .contains("Add the jump host as a server first")
                );
                let bastion = uuid::Uuid::new_v4();
                form.jump = Some(bastion.to_string());
                let (value, _) = form.draft(cx).unwrap_or_else(|_| panic!("jump saves"));
                assert_eq!(value.proxy, jump(bastion));

                form.select_route("direct", window, cx);
                let (value, _) = form.draft(cx).unwrap_or_else(|_| panic!("direct saves"));
                assert_eq!(value.proxy, None);
            });
        });
    }

    #[gpui::test]
    fn a_saved_proxy_command_opens_on_its_route_and_text(cx: &mut TestAppContext) {
        let (form, cx) = editor_with(
            cx,
            true,
            command("cloudflared access ssh --hostname %h"),
            vec![],
        );
        cx.update(|_, cx| {
            let form = form.read(cx);
            assert_eq!(form.route, Route::Command);
            assert_eq!(
                form.fields[PROXY].read(cx).value().to_string(),
                "cloudflared access ssh --hostname %h"
            );
        });
    }

    #[test]
    fn jump_hosts_leave_out_the_server_and_the_ones_behind_it() {
        let [edited, behind, further, other] = [0xa, 0xb, 0xc, 0xd].map(uuid::Uuid::from_u128);
        let mut inner = saved(further, "", jump(behind));
        inner.ssh_user = "ops".into();
        inner.ssh_host = "inner".into();
        let servers = [
            saved(edited, "Edited", None),
            saved(behind, "Behind", jump(edited)),
            inner,
            saved(other, "Bastion", None),
        ];
        let choices: Vec<_> = jump_choices(&servers, edited)
            .into_iter()
            .map(|choice| (choice.value, choice.title.to_string()))
            .collect();
        assert_eq!(choices, [(other.to_string(), "Bastion".to_owned())]);
        let choices: Vec<_> = jump_choices(&servers, other)
            .into_iter()
            .map(|choice| choice.title.to_string())
            .collect();
        assert_eq!(choices, ["Edited", "Behind", "ops@inner"]);
    }

    #[gpui::test]
    fn a_saved_jump_host_that_cant_be_chosen_must_be_chosen_again(cx: &mut TestAppContext) {
        let id = uuid::Uuid::from_u128(0xb);
        let bastion = saved(id, "Bastion", None);
        let (form, cx) = editor_with(cx, true, jump(uuid::Uuid::from_u128(0x9)), vec![bastion]);
        cx.update(|_, cx| {
            form.update(cx, |form, cx| {
                assert_eq!(form.route, Route::Jump);
                assert_eq!(form.jump, None);
                let error = form.draft(cx).expect_err("the jump host is chosen again");
                assert_eq!(error.message, "Choose a jump host.");
                form.jump = Some(id.to_string());
                let (value, _) = form.draft(cx).unwrap_or_else(|_| panic!("jump saves"));
                assert_eq!(value.proxy, jump(id));
            });
        });
    }
}
