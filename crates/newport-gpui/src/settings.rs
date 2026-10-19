//! App settings presentation. Long-lived requests and persistence belong to the client.
use super::*;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{
    scroll::ScrollableElement as _,
    select::{SelectEvent, SelectItem, SelectState},
    skeleton::Skeleton,
};
use std::{cell::Cell, rc::Rc};

#[derive(Clone)]
struct ThemeChoice(appearance::Appearance);
impl SelectItem for ThemeChoice {
    type Value = appearance::Appearance;
    fn title(&self) -> SharedString {
        match self.0 {
            appearance::Appearance::System => "Follow system",
            appearance::Appearance::Light => "Light",
            appearance::Appearance::Dark => "Dark",
        }
        .into()
    }
    fn value(&self) -> &Self::Value {
        &self.0
    }
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .text_size(px(crate::ui::FONT_SIZE))
            .child(self.title())
    }
}
type ThemeSelect = SelectState<Vec<ThemeChoice>>;

#[derive(Clone, Copy)]
#[repr(usize)]
enum FocusTarget {
    Theme,
    Startup,
    Update,
    Later,
    Install,
    Cache,
    CacheRetry,
    StartupRetry,
}

#[derive(Clone, Copy)]
struct ControlBounds {
    bounds: Bounds<Pixels>,
    scroll_offset: Point<Pixels>,
}

struct FocusRegion {
    focus: FocusHandle,
    measured: Rc<Cell<Option<ControlBounds>>>,
}

/// Reveal only when keyboard focus enters a control. Measuring nested controls
/// preserves the existing full-width viewport and its single scrollbar; the
/// centered panel does not become a second scrolling region.
struct FocusReveal {
    scroll: ScrollHandle,
    regions: Vec<FocusRegion>,
    _subscriptions: Vec<Subscription>,
}
impl FocusReveal {
    fn new<T: 'static>(scroll: ScrollHandle, window: &mut Window, cx: &mut Context<T>) -> Self {
        let regions: Vec<_> = (0..8)
            .map(|_| FocusRegion {
                focus: cx.focus_handle().tab_stop(false),
                measured: Rc::new(Cell::new(None)),
            })
            .collect();
        let subscriptions = regions
            .iter()
            .map(|region| {
                let measured = region.measured.clone();
                let scroll = scroll.clone();
                cx.on_focus_in(&region.focus, window, move |_, _, cx| {
                    let Some(measured) = measured.get() else {
                        return;
                    };
                    let current = scroll.offset();
                    // Measurements belong to the last frame. Adjust for any manual
                    // scrolling since then rather than snapping to a stale position.
                    let displacement = current.y - measured.scroll_offset.y;
                    let top = measured.bounds.top() + displacement;
                    let bottom = measured.bounds.bottom() + displacement;
                    let viewport = scroll.bounds();
                    let delta = if top < viewport.top() + px(4.) {
                        viewport.top() + px(4.) - top
                    } else if bottom > viewport.bottom() - px(4.) {
                        viewport.bottom() - px(4.) - bottom
                    } else {
                        px(0.)
                    };
                    let next = (current.y + delta).clamp(-scroll.max_offset().y, px(0.));
                    if next != current.y {
                        scroll.set_offset(point(current.x, next));
                        cx.notify();
                    }
                })
            })
            .collect();
        Self {
            scroll,
            regions,
            _subscriptions: subscriptions,
        }
    }

    fn wrap(&self, target: FocusTarget, content: impl IntoElement) -> Stateful<Div> {
        let index = target as usize;
        let region = &self.regions[index];
        let measured = region.measured.clone();
        let scroll = self.scroll.clone();
        div()
            .id(("settings-focus-region", index))
            .debug_selector(move || format!("settings-focus-{index}"))
            .track_focus(&region.focus)
            .relative()
            .flex_shrink_0()
            .child(content)
            .child(
                canvas(
                    move |bounds, _, _| {
                        measured.set(Some(ControlBounds {
                            bounds,
                            scroll_offset: scroll.offset(),
                        }));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
    }
}

pub(super) struct SettingsUi {
    theme: Entity<ThemeSelect>,
    scroll: ScrollHandle,
    reveal: Rc<FocusReveal>,
    appearance_error: Option<String>,
    /// Restart confirmation belongs to the version the user actually reviewed.
    confirmation: Option<String>,
    _subscriptions: Vec<Subscription>,
}
impl SettingsUi {
    pub(super) fn new(
        appearance: appearance::Appearance,
        window: &mut Window,
        cx: &mut Context<Newport>,
    ) -> Self {
        let theme = theme_select(appearance, window, cx);
        let subscription = cx.subscribe_in(&theme, window, |this, _, event, window, cx| {
            let SelectEvent::Confirm(Some(value)) = event else {
                return;
            };
            match this.appearance_store.save(*value) {
                Ok(()) => {
                    this.settings_ui.appearance_error = None;
                    this.appearance = *value;
                    theme::install(value.is_dark(window.appearance()), cx);
                }
                Err(_) => {
                    this.settings_ui.appearance_error =
                        Some("Couldn’t save your appearance preference. Try again.".into());
                    let previous = this.appearance;
                    this.settings_ui.theme.update(cx, |select, cx| {
                        select.set_selected_value(&previous, window, cx);
                        cx.notify();
                    });
                }
            }
            cx.notify();
        });
        let scroll = ScrollHandle::new();
        let reveal = Rc::new(FocusReveal::new(scroll.clone(), window, cx));
        Self {
            theme,
            scroll,
            reveal,
            appearance_error: None,
            confirmation: None,
            _subscriptions: vec![subscription],
        }
    }
}
fn theme_select<T: 'static>(
    appearance: appearance::Appearance,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Entity<ThemeSelect> {
    cx.new(|cx| {
        let mut select = SelectState::new(
            appearance::Appearance::ALL
                .into_iter()
                .map(ThemeChoice)
                .collect(),
            None,
            window,
            cx,
        );
        select.set_selected_value(&appearance, window, cx);
        select
    })
}

impl Newport {
    pub(super) fn apply_settings_effects(
        &mut self,
        effects: newport_client::settings::Effects,
    ) -> bool {
        // The updater remains active while Settings is hidden. Its ordinary
        // reads and loading state must not redraw an unrelated visible page.
        let mut changed = self.page == Page::Settings && effects.changed;
        if effects.metrics_cleared {
            self.invalidate_metric_readings();
            changed = true;
        }
        if let Some(version) = effects.update_ready {
            self.notification = Some(Notification::info(format!(
                "Newport {version} is ready to install. Restart from Settings to update."
            )));
            changed = true;
        }
        changed
    }

    pub(super) fn sync_settings_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_ui.theme.read(cx).selected_value() != Some(&self.appearance) {
            let appearance = self.appearance;
            self.settings_ui.theme.update(cx, |select, cx| {
                select.set_selected_value(&appearance, window, cx);
                cx.notify();
            });
        }
        let update = self.settings_state.resource("update").map(|r| &r.value);
        if self.page != Page::Settings
            || update.is_none_or(|value| {
                value["enabled"] != true
                    || self.settings_ui.confirmation.as_deref() != value["version"].as_str()
            })
        {
            self.settings_ui.confirmation = None;
        }
    }

    pub(super) fn settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = PageState::from_resources(|key| self.settings_state.resource(key));
        let owner = cx.weak_entity();
        SettingsView {
            state,
            theme: self.settings_ui.theme.clone(),
            scroll: self.settings_ui.scroll.clone(),
            reveal: self.settings_ui.reveal.clone(),
            appearance_error: self.settings_ui.appearance_error.clone(),
            confirmation: self.settings_ui.confirmation.clone(),
            on_action: Rc::new(move |action, _, cx| {
                let _ = owner.update(cx, |this, cx| {
                    match action {
                        Action::Startup(enabled) => {
                            this.settings_ui.appearance_error = None;
                            this.settings_state.set_launch_at_login(enabled);
                        }
                        Action::RetryStartup => {
                            this.settings_state.retry_startup();
                        }
                        Action::RetryCache => {
                            this.settings_state.retry_cache();
                        }
                        Action::ClearCache => {
                            this.settings_state.clear_cache();
                        }
                        Action::Update => {
                            let update = this.settings_state.resource("update");
                            let ready = update
                                .filter(|r| {
                                    r.value["enabled"] == true && r.value["phase"] == "ready"
                                })
                                .and_then(|r| r.value["version"].as_str())
                                .map(str::to_owned);
                            if let Some(version) = ready {
                                this.settings_ui.confirmation = Some(version);
                            } else {
                                this.settings_state.check_updates();
                            }
                        }
                        Action::Later => {
                            this.settings_ui.confirmation = None;
                        }
                        Action::Install => {
                            // Recheck after rendering: a stale confirmation never installs
                            // a different version or dispatches after capability loss.
                            let approved =
                                this.settings_ui
                                    .confirmation
                                    .as_deref()
                                    .is_some_and(|version| {
                                        this.settings_state.resource("update").is_some_and(|r| {
                                            r.value["enabled"] == true
                                                && r.value["phase"] == "ready"
                                                && r.value["version"].as_str() == Some(version)
                                        })
                                    });
                            if approved {
                                this.settings_state.install_update();
                            }
                        }
                    }
                    cx.notify();
                });
            }),
        }
        .into_any_element()
    }
}

#[derive(Clone, Default)]
struct Reading {
    value: Value,
    error: Option<String>,
    pending: bool,
    success: bool,
}
impl Reading {
    fn from(resource: Option<&newport_client::resources::Resource>) -> Self {
        resource
            .map(|r| Self {
                value: r.value.clone(),
                error: r.error.clone(),
                pending: r.pending,
                success: !r.pending && r.error.is_none() && r.revision > 0,
            })
            .unwrap_or_default()
    }
    fn loaded(&self) -> bool {
        !self.value.is_null()
    }
    fn loading(&self) -> bool {
        !self.loaded() && self.error.is_none()
    }
}
#[derive(Clone, Default)]
struct PageState {
    startup: Reading,
    cache: Reading,
    update: Reading,
    startup_write: Reading,
    cache_clear: Reading,
    update_check: Reading,
    update_install: Reading,
}
impl PageState {
    fn from_resources<'a>(
        resource: impl Fn(&str) -> Option<&'a newport_client::resources::Resource>,
    ) -> Self {
        Self {
            startup: Reading::from(resource("startup")),
            cache: Reading::from(resource("cache")),
            update: Reading::from(resource("update")),
            startup_write: Reading::from(resource("startup-write")),
            cache_clear: Reading::from(resource("cache-clear")),
            update_check: Reading::from(resource("update-check")),
            update_install: Reading::from(resource("update-install")),
        }
    }
    fn update_busy(&self) -> bool {
        self.update_check.pending
            || self.update_install.pending
            || matches!(
                self.update.value["phase"].as_str(),
                Some("checking" | "downloading" | "installing")
            )
    }
    fn update_ready(&self) -> bool {
        self.update.value["phase"] == "ready"
    }
    fn update_message(&self) -> String {
        let data = &self.update.value;
        if self.update.loaded() && data["enabled"] != true {
            return "Updates are unavailable in this build.".into();
        }
        match data["phase"].as_str() {
            Some("downloading") => data["total"]
                .as_f64()
                .filter(|total| *total > 0.)
                .map(|total| {
                    format!(
                        "Downloading… {:.0}%",
                        (data["downloaded"].as_f64().unwrap_or(0.) / total * 100.)
                            .round()
                            .clamp(0., 100.)
                    )
                })
                .unwrap_or_else(|| "Downloading…".into()),
            Some("checking") => "Checking for updates…".into(),
            Some("installing") => "Installing…".into(),
            Some("ready") => format!("Version {} is ready to install.", text(data, "version")),
            Some("current") => "You’re up to date.".into(),
            _ => "Updates download automatically. You choose when to restart.".into(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Startup(bool),
    RetryStartup,
    RetryCache,
    ClearCache,
    Update,
    Later,
    Install,
}
impl Action {
    fn focus_target(self) -> FocusTarget {
        match self {
            Self::Startup(_) => FocusTarget::Startup,
            Self::RetryStartup => FocusTarget::StartupRetry,
            Self::RetryCache => FocusTarget::CacheRetry,
            Self::ClearCache => FocusTarget::Cache,
            Self::Update => FocusTarget::Update,
            Self::Later => FocusTarget::Later,
            Self::Install => FocusTarget::Install,
        }
    }
}
type ActionHandler = Rc<dyn Fn(Action, &mut Window, &mut App)>;

#[derive(IntoElement)]
struct RevealedButton {
    button: Button,
    target: FocusTarget,
    reveal: Rc<FocusReveal>,
}
impl RevealedButton {
    fn disabled(mut self, disabled: bool) -> Self {
        self.button = self.button.disabled(disabled);
        self
    }
    fn loading(mut self, loading: bool) -> Self {
        self.button = self.button.loading(loading);
        self
    }
    fn ghost(mut self) -> Self {
        self.button = self.button.ghost();
        self
    }
    fn primary(mut self) -> Self {
        self.button = self.button.primary();
        self
    }
}
impl RenderOnce for RevealedButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.reveal.wrap(self.target, self.button)
    }
}

#[derive(IntoElement)]
struct SettingsView {
    state: PageState,
    theme: Entity<ThemeSelect>,
    scroll: ScrollHandle,
    reveal: Rc<FocusReveal>,
    appearance_error: Option<String>,
    confirmation: Option<String>,
    on_action: ActionHandler,
}
impl SettingsView {
    fn appearance(&self, cx: &App) -> impl IntoElement {
        section("appearance", "Appearance", true, cx).child(
            row().child("Theme").child(
                self.reveal
                    .wrap(
                        FocusTarget::Theme,
                        crate::controls::select(&self.theme)
                            .accessibility_label("Theme")
                            .w(px(160.))
                            .rounded(px(crate::ui::RADIUS))
                            .menu_item_min_height(px(28.))
                            .menu_width(px(160.)),
                    )
                    .w(px(160.))
                    .h(px(crate::controls::CONTROL_HEIGHT)),
            ),
        )
    }
    fn startup(&self, cx: &App) -> impl IntoElement {
        let startup = &self.state.startup;
        let pending = self.state.startup_write.pending;
        let available = startup.value["available"] == true;
        let callback = self.on_action.clone();
        let content = section("startup", "Startup", false, cx).child(
            row()
                .child(
                    div()
                        .min_w_0()
                        .line_height(relative(1.428571))
                        .child("Launch at login")
                        .child(
                            help(
                                if pending {
                                    "Saving…"
                                } else if startup.loaded() && !available {
                                    "Available in the installed app."
                                } else {
                                    "Start in the background when you sign in."
                                },
                                cx,
                            )
                            .id("startup-feedback")
                            .role(accesskit::Role::Status)
                            .aria_label(if pending {
                                "Saving startup preference"
                            } else {
                                ""
                            })
                            .test_support(),
                        ),
                )
                .child(
                    self.reveal.wrap(
                        FocusTarget::Startup,
                        crate::controls::switch("launch-at-login")
                            .accessibility_label("Launch at login")
                            .checked(startup.value["enabled"] == true)
                            .disabled(!available || pending)
                            .on_click(move |checked, window, cx| {
                                callback(Action::Startup(*checked), window, cx)
                            }),
                    ),
                ),
        );
        if startup.loading() && !pending {
            content.child(skeleton("startup", px(160.)))
        } else {
            content
        }
    }
    fn updates(&self, cx: &App) -> impl IntoElement {
        let update = &self.state.update;
        let ready = self.state.update_ready();
        let busy = self.state.update_busy();
        let problem = self
            .state
            .update_install
            .error
            .clone()
            .or_else(|| self.state.update_check.error.clone())
            .or_else(|| update.value["error"].as_str().map(str::to_owned))
            .or_else(|| update.error.clone());
        let mut detail = div().flex_1().min_w(px(180.));
        if update.loading() {
            detail = detail
                .child(skeleton("update-version", px(140.)))
                .child(skeleton("update-status", px(260.)));
        } else {
            detail = detail
                .child(
                    update.value["currentVersion"]
                        .as_str()
                        .map(|v| format!("Newport {v}"))
                        .unwrap_or_else(|| "Newport".into()),
                )
                .child(
                    help(self.state.update_message(), cx)
                        .id("update-phase")
                        .role(accesskit::Role::Status)
                        .aria_label(self.state.update_message()),
                );
        }
        let mut content = section("updates", "Updates", false, cx).child(
            row().flex_wrap().gap_y(px(12.)).child(detail).child(
                action_button(
                    "check-updates",
                    if ready {
                        "Restart to update"
                    } else {
                        "Check for updates"
                    },
                    Action::Update,
                    &self.on_action,
                    &self.reveal,
                )
                .disabled(update.value["enabled"] != true || busy)
                .loading(busy),
            ),
        );
        if ready
            && self
                .confirmation
                .as_deref()
                .is_some_and(|version| update.value["version"].as_str() == Some(version))
        {
            content = content.child(
                div()
                    .mt(px(16.))
                    .child(message(
                        "Restarting disconnects active sessions and cancels transfers.",
                        cx,
                    ))
                    .child(
                        div()
                            .mt(px(8.))
                            .flex()
                            .flex_wrap()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                action_button(
                                    "later-update",
                                    "Later",
                                    Action::Later,
                                    &self.on_action,
                                    &self.reveal,
                                )
                                .ghost()
                                .disabled(self.state.update_install.pending),
                            )
                            .child(
                                action_button(
                                    "install-update",
                                    "Restart now",
                                    Action::Install,
                                    &self.on_action,
                                    &self.reveal,
                                )
                                .primary()
                                .disabled(
                                    self.state.update_install.pending
                                        || update.value["enabled"] != true,
                                ),
                            ),
                    ),
            );
        }
        content.when_some(problem, |content, problem| {
            content.child(error("update-error", problem, cx).mt(px(20.)))
        })
    }
    fn cache(&self, cx: &App) -> impl IntoElement {
        let cache = &self.state.cache;
        let clear = &self.state.cache_clear;
        let problem = clear.error.clone().or_else(|| cache.error.clone());
        let mut size = div()
            .text_color(cx.theme().muted_foreground)
            .whitespace_nowrap();
        size = if cache.loading() {
            size.child(skeleton("cache-size", px(60.)))
        } else {
            size.child(
                div()
                    .id("metrics-cache-size")
                    .role(accesskit::Role::Label)
                    .aria_label("Metrics cache size")
                    .child(if cache.loaded() {
                        bytes(&cache.value["bytes"])
                    } else {
                        "Unavailable".into()
                    }),
            )
        };
        let mut content = section("cache", "Cache", false, cx).child(
            row()
                .flex_wrap()
                .gap_y(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(180.))
                        .child("Metrics history")
                        .child(help(
                            "Recorded history for all servers. New readings continue.",
                            cx,
                        )),
                )
                .child(
                    div()
                        .ml_auto()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(size)
                        .child(
                            action_button(
                                "clear-metrics-cache",
                                "Clear cache",
                                Action::ClearCache,
                                &self.on_action,
                                &self.reveal,
                            )
                            .disabled(
                                !cache.loaded()
                                    || clear.pending
                                    || (cache.value["samples"].as_u64().unwrap_or(0) == 0
                                        && clear.error.is_none()),
                            )
                            .loading(clear.pending),
                        ),
                ),
        );
        if let Some(problem) = problem {
            content = content.child(error("cache-error", problem, cx).mt(px(20.)));
            if cache.error.is_some() {
                content = content.child(
                    div().mt(px(8.)).child(
                        {
                            let mut retry = action_button(
                                "retry-cache",
                                "Try again",
                                Action::RetryCache,
                                &self.on_action,
                                &self.reveal,
                            );
                            retry.button = retry
                                .button
                                .accessibility_label("Try reading the cache size again");
                            retry
                        }
                        .disabled(cache.pending),
                    ),
                );
            }
        } else if clear.success {
            content = content.child(
                message("Cache cleared.", cx)
                    .id("cache-cleared")
                    .role(accesskit::Role::Status)
                    .aria_label("Cache cleared.")
                    .mt(px(12.)),
            );
        } else if cache.loaded() && cache.value["samples"] == 0 {
            content = content.child(empty::compact("No recorded history.", cx));
        }
        content
    }
}
impl RenderOnce for SettingsView {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let problem = self
            .appearance_error
            .clone()
            .or_else(|| self.state.startup_write.error.clone())
            .or_else(|| self.state.startup.error.clone());
        let body = div()
            .id("settings-scroll-body")
            .debug_selector(|| "settings-scroll-body".into())
            .w_full()
            .p(px(crate::controls::PAGE_PADDING))
            .child(
                div()
                    .id("settings-panel")
                    .debug_selector(|| "settings-panel".into())
                    .role(accesskit::Role::Pane)
                    .aria_label("App settings")
                    .w_full()
                    .max_w(px(720.))
                    .mx_auto()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .line_height(relative(1.428571))
                    .child(crate::controls::page_title("Settings", None, cx).mb(px(16.)))
                    .child(self.appearance(cx))
                    .child(self.startup(cx))
                    .child(self.updates(cx))
                    .child(self.cache(cx))
                    .when_some(problem, |panel, problem| {
                        panel.child(
                            div()
                                .mt(px(20.))
                                .child(error("settings-error", problem, cx))
                                .when(!self.state.startup.loaded(), |problem| {
                                    problem.child(
                                        div().mt(px(8.)).child(
                                            action_button(
                                                "retry-startup",
                                                "Try again",
                                                Action::RetryStartup,
                                                &self.on_action,
                                                &self.reveal,
                                            )
                                            .disabled(self.state.startup.pending),
                                        ),
                                    )
                                }),
                        )
                    }),
            );
        div()
            .id("settings-scroll-region")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .relative()
            .child(
                div()
                    .id("settings-viewport")
                    .debug_selector(|| "settings-viewport".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
            .vertical_scrollbar(&self.scroll)
    }
}
fn section(id: &'static str, title: &'static str, first: bool, cx: &App) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || format!("settings-{id}"))
        .role(accesskit::Role::Group)
        .aria_label(title)
        .border_b_1()
        .border_color(cx.theme().border)
        .pt(px(if first { 4. } else { 22. }))
        .pb(px(22.))
        .child(
            div()
                .id(SharedString::from(format!("settings-{id}-heading")))
                .role(accesskit::Role::Heading)
                .aria_label(title)
                .mb(px(16.))
                .text_size(px(crate::ui::FONT_SIZE))
                .font_weight(FontWeight::SEMIBOLD)
                .line_height(relative(1.428571))
                .child(title),
        )
}
fn row() -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_x(px(24.))
        .text_size(px(crate::ui::FONT_SIZE))
}
fn message(value: impl Into<String>, cx: &App) -> Div {
    div()
        .text_size(px(crate::ui::FONT_SIZE))
        .line_height(relative(1.6))
        .text_color(cx.theme().muted_foreground)
        .child(value.into())
}
fn help(value: impl Into<String>, cx: &App) -> Div {
    message(value, cx).mt(px(4.))
}
fn error(id: &'static str, value: String, cx: &App) -> impl IntoElement + Styled {
    div()
        .id(id)
        .test_support()
        .role(accesskit::Role::Alert)
        .aria_label(value.clone())
        .text_size(px(crate::ui::FONT_SIZE))
        .line_height(relative(1.6))
        .text_color(theme::error(cx.theme().is_dark()))
        .child(value)
}
fn skeleton(id: &'static str, width: Pixels) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("settings-{id}-skeleton")))
        .role(accesskit::Role::Status)
        .aria_label(format!("Loading {}", id.replace('-', " ")))
        .mt(px(4.))
        .w(width)
        .max_w_full()
        .h(px(16.))
        .child(
            Skeleton::new()
                .w_full()
                .h(px(12.))
                .rounded(px(crate::ui::RADIUS)),
        )
}
fn action_button(
    id: &'static str,
    title: &'static str,
    action: Action,
    handler: &ActionHandler,
    reveal: &Rc<FocusReveal>,
) -> RevealedButton {
    let callback = handler.clone();
    let button = crate::controls::button(id)
        .label(title)
        .rounded(px(crate::ui::RADIUS))
        .on_click(move |_, window, cx| callback(action, window, cx));
    RevealedButton {
        button,
        target: action.focus_target(),
        reveal: reveal.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, FocusReveal, PageState, Reading, SettingsView, ThemeSelect, theme_select};
    use crate::appearance::Appearance;
    use gpui::{
        AppContext, Context, Entity, Focusable, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke,
        ParentElement, Render, ScrollHandle, Styled, TestAppContext, VisualTestContext, Window,
        div, point, px, size,
    };
    use gpui_kit as gpui;
    use gpui_kit::base::test_support::{ElementSnapshot, snapshots};
    use serde_json::json;
    use std::rc::Rc;

    struct Harness {
        state: PageState,
        theme: Entity<ThemeSelect>,
        scroll: ScrollHandle,
        reveal: Rc<FocusReveal>,
        confirmation: Option<String>,
        actions: Vec<Action>,
    }
    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let weak = cx.weak_entity();
            div().size_full().flex().flex_col().child(SettingsView {
                state: self.state.clone(),
                theme: self.theme.clone(),
                scroll: self.scroll.clone(),
                reveal: self.reveal.clone(),
                appearance_error: None,
                confirmation: self.confirmation.clone(),
                on_action: Rc::new(move |action, _, cx| {
                    let _ = weak.update(cx, |view, cx| {
                        view.actions.push(action);
                        match action {
                            Action::Update if view.state.update_ready() => {
                                view.confirmation = view.state.update.value["version"]
                                    .as_str()
                                    .map(str::to_owned)
                            }
                            Action::Later => view.confirmation = None,
                            Action::Install => view.state.update_install.pending = true,
                            Action::ClearCache => view.state.cache_clear.pending = true,
                            Action::Startup(_) => view.state.startup_write.pending = true,
                            _ => {}
                        }
                        cx.notify();
                    });
                }),
            })
        }
    }
    fn loaded() -> PageState {
        PageState {
            startup: Reading {
                value: json!({"available":true,"enabled":false}),
                ..Default::default()
            },
            update: Reading {
                value: json!({"enabled":true,"phase":"current","currentVersion":"0.3.0","version":null}),
                ..Default::default()
            },
            cache: Reading {
                value: json!({"bytes":2048,"samples":12}),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn harness(
        cx: &mut TestAppContext,
        state: PageState,
    ) -> (Entity<Harness>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
            cx.set_reduce_motion(true);
        });
        let mut view = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(|cx| {
                let scroll = ScrollHandle::new();
                Harness {
                    state,
                    theme: theme_select(Appearance::System, window, cx),
                    reveal: Rc::new(FocusReveal::new(scroll.clone(), window, cx)),
                    scroll,
                    confirmation: None,
                    actions: vec![],
                }
            });
            view = Some(content.clone());
            gpui_kit::base::Root::new(content, window, cx)
        });
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1000.), px(750.)));
        draw(cx);
        (view.unwrap(), cx)
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    fn element(cx: &mut VisualTestContext, label: &str) -> ElementSnapshot {
        cx.update(|window, _| {
            snapshots(window)
                .into_iter()
                .find(|e| e.label() == Some(label))
                .unwrap_or_else(|| panic!("missing {label}"))
        })
    }
    fn click(cx: &mut VisualTestContext, label: &str) {
        let center = element(cx, label).bounds().center();
        cx.simulate_click(center, Default::default());
        draw(cx);
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

    fn focused_and_visible(view: &Entity<Harness>, label: &str, cx: &mut VisualTestContext) {
        let control = element(cx, label);
        let viewport = view.read_with(cx, |view, _| view.scroll.bounds());
        assert_eq!(control.focused(), Some(true), "{label}: {control:?}");
        assert!(
            control.bounds().top() >= viewport.top()
                && control.bounds().bottom() <= viewport.bottom(),
            "{label}: {:?} outside {viewport:?}",
            control.bounds()
        );
    }

    #[gpui::test]
    #[ignore = "manual Settings CPU frame benchmark; excludes GPU presentation"]
    fn benchmark_settings_frames(cx: &mut TestAppContext) {
        let (view, cx) = harness(cx, loaded());
        cx.simulate_resize(size(px(900.), px(720.)));
        draw(cx);
        let start = std::time::Instant::now();
        for _ in 0..30 {
            view.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        eprintln!(
            "Settings: {:.3} ms/CPU frame",
            start.elapsed().as_secs_f64() * 1000. / 30.
        );
        assert!(view.read_with(cx, |view, _| view.actions.is_empty()));
    }

    #[gpui::test]
    fn short_settings_reveals_actual_tab_and_shift_tab_targets_without_pinning_manual_scroll(
        cx: &mut TestAppContext,
    ) {
        let mut state = loaded();
        state.update.value["phase"] = json!("ready");
        state.update.value["version"] = json!("0.4.0");
        state.cache.error = Some("Couldn’t refresh the cache size.".into());
        let (view, cx) = harness(cx, state);
        cx.simulate_resize(size(px(360.), px(220.)));
        view.update(cx, |view, cx| {
            view.confirmation = Some("0.4.0".into());
            cx.notify();
        });
        draw(cx);
        cx.update(|window, cx| view.read(cx).theme.focus_handle(cx).focus(window, cx));
        draw(cx);
        focused_and_visible(&view, "Theme", cx);
        for label in [
            "Launch at login",
            "Restart to update",
            "Later",
            "Restart now",
            "Clear cache",
            "Try reading the cache size again",
        ] {
            key(cx, "tab");
            focused_and_visible(&view, label, cx);
        }
        // An unchanged focus does not force the view back down after a wheel drag.
        view.read_with(cx, |view, _| view.scroll.set_offset(point(px(0.), px(0.))));
        draw(cx);
        assert_eq!(view.read_with(cx, |view, _| view.scroll.offset().y), px(0.));
        for label in [
            "Clear cache",
            "Restart now",
            "Later",
            "Restart to update",
            "Launch at login",
            "Theme",
        ] {
            key(cx, "shift-tab");
            focused_and_visible(&view, label, cx);
        }
        // Initial startup failures hide its disabled switch from tab order and
        // add Try again after Cache. The dynamic final control must reveal as well.
        view.update(cx, |view, cx| {
            view.state.startup.value = serde_json::Value::Null;
            view.state.startup.error = Some("Couldn’t read login settings.".into());
            cx.notify();
        });
        draw(cx);
        for label in [
            "Restart to update",
            "Later",
            "Restart now",
            "Clear cache",
            "Try reading the cache size again",
            "Try again",
        ] {
            key(cx, "tab");
            focused_and_visible(&view, label, cx);
        }
        for label in [
            "Try reading the cache size again",
            "Clear cache",
            "Restart now",
            "Later",
            "Restart to update",
            "Theme",
        ] {
            key(cx, "shift-tab");
            focused_and_visible(&view, label, cx);
        }
        assert!(
            view.read_with(cx, |view, _| view.actions.is_empty()),
            "keyboard focus never activates settings mutations"
        );
    }

    #[gpui::test]
    fn reference_sections_center_at_720_and_compact_page_uses_one_scroll_region(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = harness(cx, loaded());
        let panel = cx.debug_bounds("settings-panel").unwrap();
        assert_eq!(panel.size.width, px(720.));
        assert_eq!(panel.left(), px(140.));
        assert_eq!(panel.top(), px(16.));
        let theme = element(cx, "Theme");
        assert_eq!(theme.bounds().size, size(px(160.), px(28.)));
        for label in ["Check for updates", "Clear cache"] {
            assert_eq!(element(cx, label).bounds().size.height, px(28.), "{label}");
        }
        let appearance = cx.debug_bounds("settings-appearance").unwrap();
        let startup = cx.debug_bounds("settings-startup").unwrap();
        let updates = cx.debug_bounds("settings-updates").unwrap();
        let cache = cx.debug_bounds("settings-cache").unwrap();
        assert_eq!(appearance.bottom(), startup.top());
        assert_eq!(startup.bottom(), updates.top());
        assert_eq!(updates.bottom(), cache.top());
        assert_eq!(
            view.read_with(cx, |view, _| view.scroll.max_offset().y),
            px(0.)
        );
        cx.simulate_resize(size(px(360.), px(260.)));
        draw(cx);
        let panel = cx.debug_bounds("settings-panel").unwrap();
        assert_eq!(panel.left(), px(16.));
        assert_eq!(panel.size.width, px(328.));
        assert!(view.read_with(cx, |view, _| view.scroll.max_offset().y) > px(0.));
        view.read_with(cx, |view, _| {
            view.scroll
                .set_offset(point(px(0.), -view.scroll.max_offset().y))
        });
        draw(cx);
        let clear = element(cx, "Clear cache");
        assert!(clear.bounds().left() >= px(16.) && clear.bounds().right() <= px(344.));
        assert!(clear.bounds().bottom() <= px(260.));
    }

    #[gpui::test]
    fn settings_load_independently_and_report_each_read_error(cx: &mut TestAppContext) {
        let (view, cx) = harness(cx, PageState::default());
        assert_eq!(element(cx, "Theme").role(), Some(gpui::Role::ComboBox));
        for label in ["Launch at login", "Check for updates", "Clear cache"] {
            click(cx, label);
        }
        assert!(view.read_with(cx, |view, _| view.actions.is_empty()));
        view.update(cx, |view, cx| {
            view.state.startup.error = Some("Login settings failed.".into());
            view.state.cache.error = Some("Cache size failed.".into());
            view.state.update.error = Some("Update status failed.".into());
            cx.notify();
        });
        draw(cx);
        for problem in [
            "Login settings failed.",
            "Cache size failed.",
            "Update status failed.",
        ] {
            assert_eq!(element(cx, problem).role(), Some(gpui::Role::Alert));
        }
        click(cx, "Try again");
        click(cx, "Try reading the cache size again");
        assert_eq!(
            view.read_with(cx, |view, _| view.actions.clone()),
            vec![Action::RetryStartup, Action::RetryCache]
        );
    }

    #[gpui::test]
    fn ready_update_requires_separate_review_and_restart_actions(cx: &mut TestAppContext) {
        let mut state = loaded();
        state.update.value["phase"] = json!("ready");
        state.update.value["version"] = json!("0.4.0");
        let (view, cx) = harness(cx, state);
        cx.update(|window, _| {
            assert!(
                !snapshots(window)
                    .iter()
                    .any(|e| e.label() == Some("Restart now"))
            )
        });
        click(cx, "Restart to update");
        assert_eq!(
            view.read_with(cx, |view, _| view.actions.clone()),
            vec![Action::Update]
        );
        click(cx, "Later");
        cx.update(|window, _| {
            assert!(
                !snapshots(window)
                    .iter()
                    .any(|e| e.label() == Some("Restart now"))
            )
        });
        click(cx, "Restart to update");
        click(cx, "Restart now");
        assert_eq!(
            view.read_with(cx, |view, _| view.actions.clone()),
            vec![
                Action::Update,
                Action::Later,
                Action::Update,
                Action::Install
            ]
        );
        click(cx, "Restart now");
        assert_eq!(view.read_with(cx, |view, _| view.actions.len()), 4);
        view.update(cx, |view, cx| {
            view.state.update_install.pending = false;
            view.state.update.value["version"] = json!("0.4.1");
            cx.notify();
        });
        draw(cx);
        cx.update(|window, _| {
            assert!(
                !snapshots(window)
                    .iter()
                    .any(|e| e.label() == Some("Restart now"))
            )
        });
    }

    #[gpui::test]
    fn startup_and_cache_controls_respect_capability_empty_and_pending_state(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = harness(cx, loaded());
        click(cx, "Launch at login");
        click(cx, "Launch at login");
        click(cx, "Clear cache");
        click(cx, "Clear cache");
        assert_eq!(
            view.read_with(cx, |view, _| view.actions.clone()),
            vec![Action::Startup(true), Action::ClearCache]
        );
        view.update(cx, |view, cx| {
            view.state.startup_write.pending = false;
            view.state.startup.value["available"] = json!(false);
            view.state.cache_clear.pending = false;
            view.state.cache.value["samples"] = json!(0);
            cx.notify();
        });
        draw(cx);
        view.update(cx, |view, cx| {
            view.state.cache_clear.error = Some("Clear failed.".into());
            cx.notify();
        });
        draw(cx);
        assert_eq!(element(cx, "Clear failed.").role(), Some(gpui::Role::Alert));
        click(cx, "Clear cache");
        assert_eq!(view.read_with(cx, |view, _| view.actions.len()), 3);
    }

    #[gpui::test]
    fn startup_saving_keeps_loaded_controls_in_place(cx: &mut TestAppContext) {
        let (view, cx) = harness(cx, loaded());
        let before = element(cx, "Launch at login").bounds();
        let cache_before = element(cx, "Clear cache").bounds();
        click(cx, "Launch at login");
        draw(cx);
        assert_eq!(element(cx, "Launch at login").bounds(), before);
        assert_eq!(element(cx, "Clear cache").bounds(), cache_before);
        assert_eq!(
            element(cx, "Saving startup preference").role(),
            Some(gpui::Role::Status)
        );
        view.update(cx, |view, cx| {
            view.state.startup_write.pending = false;
            cx.notify();
        });
        draw(cx);
        assert_eq!(element(cx, "Clear cache").bounds(), cache_before);
    }

    #[test]
    fn update_copy_covers_unknown_size_percentage_capability_and_all_phases() {
        let mut state = loaded();
        for (phase, message) in [
            (
                "idle",
                "Updates download automatically. You choose when to restart.",
            ),
            ("checking", "Checking for updates…"),
            ("installing", "Installing…"),
            ("current", "You’re up to date."),
        ] {
            state.update.value["phase"] = json!(phase);
            assert_eq!(state.update_message(), message);
        }
        state.update.value["phase"] = json!("downloading");
        assert_eq!(state.update_message(), "Downloading…");
        state.update.value["total"] = json!(200);
        state.update.value["downloaded"] = json!(67);
        assert_eq!(state.update_message(), "Downloading… 34%");
        state.update.value["downloaded"] = json!(400);
        assert_eq!(state.update_message(), "Downloading… 100%");
        state.update.value["enabled"] = json!(false);
        assert_eq!(
            state.update_message(),
            "Updates are unavailable in this build."
        );
    }
}
