//! One retained SSH PTY, its parser, and native text-input surface.
#[path = "terminal_find.rs"]
mod find;
use crate::hover::HoverExt as _;
use gpui_kit::component::{ActiveTheme, WindowExt};
use gpui_kit::{prelude::*, *};
use newport_gpui::{
    terminal_focus::FocusReporter,
    terminal_input::{
        KeyEvent, NativeKeyIdentity, current_modifiers, encode_key, native_key_identity,
        native_modifiers,
    },
    terminal_mouse::{self, Action as MouseAction, Button as TerminalButton, PressedButtons},
    terminal_status::TerminalStatus,
    terminal_transport::{Event, Session},
};
use newport_terminal::{
    Effects, Engine, Mode, Point as TermPoint, PointCoordinate, ScrollViewport, SelectionPosition,
};
use std::{
    collections::VecDeque,
    ops::Range,
    time::{Duration, Instant},
};

pub struct OpenLink(pub String);
impl EventEmitter<OpenLink> for TerminalView {}
struct LinkPress {
    link: String,
    button: MouseButton,
    mac_control_click: bool,
    position: Point<Pixels>,
    cancelled: bool,
}

fn link_modifier(modifiers: Modifiers) -> bool {
    (modifiers.platform || modifiers.control) && !modifiers.shift && !modifiers.alt
}

// Synthetic events have no physical identity; keep their logical fallback in a
// separate namespace so they cannot release a native key with the same text.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum ForwardedKey {
    Physical(PhysicalKey),
    Logical(String),
}

impl ForwardedKey {
    fn new(physical: &Option<PhysicalKey>, logical: &str) -> Self {
        physical
            .clone()
            .map(Self::Physical)
            .unwrap_or_else(|| Self::Logical(logical.into()))
    }
}

#[derive(Clone)]
struct ForwardedPress {
    logical: String,
    native: Option<NativeKeyIdentity>,
    /// Releases and repeats encode Option the way the press did.
    option_as_meta: bool,
}

/// Whether macOS Option sends Meta keys in terminals. Newport sets it from
/// preferences; without it, Option is Meta.
pub(crate) struct OptionAsMeta(pub(crate) bool);
impl Global for OptionAsMeta {}

pub(crate) fn option_as_meta(cx: &App) -> bool {
    cx.try_global::<OptionAsMeta>()
        .is_none_or(|setting| setting.0)
}

/// How long a draft waits for a shell that never turns bracketed paste on.
const DRAFT_FALLBACK: Duration = Duration::from_secs(1);

/// Space between the view's edges and its cell grid.
const PADDING: Size<Pixels> = Size {
    width: px(10.),
    height: px(8.),
};

pub struct TerminalView {
    find: Option<find::FindPanel>,
    find_draft: find::FindDraft,
    pub focus: FocusHandle,
    focus_subscriptions: Vec<Subscription>,
    focused: bool,
    visible: bool,
    focus_reporter: FocusReporter,
    pub title: String,
    pending_title: Option<(String, Instant)>,
    /// Text a program copied (tmux, an editor), for the clipboard.
    copied: Option<String>,
    pub status: TerminalStatus,
    pub input_error: Option<String>,
    server: String,
    session: Option<Session>,
    open_on_show: bool,
    command_eligible: bool,
    command_draft: newport_client::terminal_command::CommandDraft,
    /// When the shell became ready with a draft waiting for its prompt.
    draft_armed_at: Option<Instant>,
    /// The shell has turned bracketed paste on at its prompt, so its being
    /// off means a command is running.
    prompted: bool,
    engine: Engine,
    cell: Size<Pixels>,
    bounds: Bounds<Pixels>,
    /// Top-left of the cell grid: centered in `bounds`, inside the padding.
    grid_origin: Point<Pixels>,
    dimensions: (u16, u16),
    resize_pending: bool,
    input: VecDeque<Vec<u8>>,
    input_bytes: usize,
    observed_epoch: u64,
    observed_ready: bool,
    composition: String,
    composition_selection: Range<usize>,
    selecting: bool,
    selection_pointer: Option<Point<Pixels>>,
    selection_scroll_at: Instant,
    remote_pressed: PressedButtons,
    forwarded_keys: std::collections::HashMap<ForwardedKey, ForwardedPress>,
    last_motion: Option<Vec<u8>>,
    hovered_link: Option<String>,
    // macOS GPUI turns Ctrl-left into Right and strips Control from down/up.
    // Flags-changed and motion events still carry the physical modifier state.
    link_control_held: bool,
    link_press: Option<LinkPress>,
    link_generation: u64,
    scroll_remainder: f32,
    dark: bool,
    /// Guidance pinned to this terminal's top-right corner. It never takes
    /// focus, so the shell keeps every keystroke.
    card: Option<AnyView>,
}
impl TerminalView {
    pub(super) fn open_link(&mut self, url: String, cx: &mut Context<Self>) {
        let Some(terminal) = self.session_id() else {
            self.input_error = Some("Reconnect the terminal before opening its links.".into());
            cx.notify();
            return;
        };
        self.link_generation = self.link_generation.wrapping_add(1);
        let generation = self.link_generation;
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    newport_client::backend::Backend::discover().open_terminal_link(terminal, &url)
                })
                .await;
            if let Err(error) = result {
                let _ = view.update(cx, |view, cx| {
                    if view.link_generation == generation {
                        view.input_error = Some(format!("Couldn’t open the link. {error}"));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
    pub fn new(server: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            find: None,
            find_draft: Default::default(),
            focus: cx.focus_handle().tab_stop(true),
            focus_subscriptions: Vec::new(),
            focused: false,
            visible: false,
            focus_reporter: FocusReporter::default(),
            title: "Terminal".into(),
            pending_title: None,
            copied: None,
            status: TerminalStatus::default(),
            input_error: None,
            server,
            session: None,
            open_on_show: true,
            command_eligible: true,
            command_draft: Default::default(),
            draft_armed_at: None,
            prompted: false,
            engine: Engine::new(80, 24).expect("valid initial PTY dimensions"),
            cell: size(px(8.), px(16.)),
            bounds: Bounds::default(),
            grid_origin: Point::default(),
            dimensions: (80, 24),
            resize_pending: false,
            input: VecDeque::new(),
            input_bytes: 0,
            observed_epoch: 0,
            observed_ready: false,
            composition: String::new(),
            composition_selection: 0..0,
            selecting: false,
            selection_pointer: None,
            selection_scroll_at: Instant::now(),
            remote_pressed: PressedButtons::default(),
            forwarded_keys: Default::default(),
            last_motion: None,
            hovered_link: None,
            link_control_held: false,
            link_press: None,
            link_generation: 0,
            scroll_remainder: 0.,
            dark: cx.theme().is_dark(),
            card: None,
        };
        this.engine
            .set_theme(crate::terminal_paint::theme(this.dark))
            .expect("valid terminal theme");
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                if view.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        this
    }
    /// A full-screen program, or a command the shell is waiting on.
    pub fn running_command(&self) -> bool {
        self.alternate_screen()
            || (self.prompted && !self.engine.mode(Mode::BRACKETED_PASTE).unwrap_or(false))
    }
    fn alternate_screen(&self) -> bool {
        self.engine.alternate_screen().unwrap_or(false)
    }
    pub fn session_id(&self) -> Option<uuid::Uuid> {
        self.session.as_ref().map(Session::id)
    }
    /// Only a fresh, unstarted tab may accept a command from another page.
    pub(super) fn prepare_command(
        &mut self,
        draft: newport_client::terminal_command::CommandDraft,
    ) -> bool {
        if !self.command_eligible || self.session.is_some() || !self.open_on_show {
            return false;
        }
        self.command_eligible = false;
        self.command_draft = draft;
        true
    }
    pub(crate) fn set_card(&mut self, card: Option<AnyView>, cx: &mut Context<Self>) {
        if self.card.as_ref().map(AnyView::entity_id) != card.as_ref().map(AnyView::entity_id) {
            self.card = card;
            cx.notify();
        }
    }
    pub(crate) fn visible(&self) -> bool {
        self.visible
    }
    pub fn active(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| !session.is_closed())
    }
    /// State consumed outside the terminal's visible output surface.
    pub(super) fn tab_presentation(&self) -> (String, TerminalStatus, Option<String>, bool) {
        (
            self.title.clone(),
            self.status.clone(),
            self.input_error.clone(),
            self.active(),
        )
    }
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.close();
        self.status.disconnected();
        cx.notify();
    }
    pub fn close(&mut self) {
        self.open_on_show = false;
        self.session.take();
        self.observed_ready = false;
        self.clear_input();
        self.status.disconnected();
    }
    /// A new backend/credential/destination scope must never reuse old PTY data.
    /// Inactive tabs remain lazy.
    pub fn reset_connection(&mut self, cx: &mut Context<Self>) {
        self.clear_find(cx);
        self.close();
        self.prompted = false;
        self.engine =
            Engine::new(self.dimensions.0, self.dimensions.1).expect("validated PTY size");
        self.engine
            .resize(
                self.dimensions.0,
                self.dimensions.1,
                f32::from(self.cell.width).ceil().max(1.) as u16,
                f32::from(self.cell.height).ceil().max(1.) as u16,
            )
            .expect("validated PTY size");
        self.engine
            .set_theme(crate::terminal_paint::theme(self.dark))
            .expect("valid terminal theme");
        self.title = "Terminal".into();
        self.status.reset_scope();
        self.input_error = None;
        self.selecting = false;
        self.scroll_remainder = 0.;
        self.open_on_show = true;
        cx.notify();
    }
    fn open(&mut self, cx: &mut Context<Self>) {
        self.open_on_show = false;
        self.command_eligible = false;
        // terminal_open already carries the measured size. Only later layout
        // changes (or recovery) require a separate resize request.
        self.resize_pending = false;
        self.observed_epoch = 0;
        self.observed_ready = false;
        self.status.begin();
        self.input_error = None;
        // The backend joins the server's shared display when it can, without
        // waiting for it. Opening a terminal never starts installation or
        // onboarding, and no display failure reaches it.
        match Session::open(&self.server, self.dimensions.0, self.dimensions.1) {
            Ok(session) => {
                // A GUI app launched in this shell may ask for setup. That
                // question never holds up, changes or ends the shell.
                crate::terminal_setup::register(session.id(), &self.server, cx.weak_entity(), cx);
                self.session = Some(session);
            }
            Err(error) => {
                self.clear_input();
                self.status.failed(error);
            }
        }
    }
    pub fn reconnect(&mut self, cx: &mut Context<Self>) {
        self.clear_find(cx);
        self.close();
        let result = self.engine.reconnect();
        self.accept_effects(result);
        self.title = "Terminal".into();
        self.input_error = None;
        // Explicit reconnect applies to background tabs too. Use their last
        // measured dimensions (80x24 before the first display);
        // only automatic first opens and scope changes wait for layout.
        self.open(cx);
        cx.notify();
    }
    pub(super) fn ready(&self) -> bool {
        self.observed_ready
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.ready() && session.epoch() == self.observed_epoch)
    }
    fn enqueue(&mut self, bytes: Vec<u8>) -> bool {
        if bytes.len() > 1024 * 1024 - self.input_bytes {
            self.input_error = Some("Terminal input queue is full. Input was not sent.".into());
            return false;
        }
        self.input_error = None;
        self.input_bytes += bytes.len();
        self.input.extend(bytes.chunks(4096).map(<[u8]>::to_vec));
        true
    }
    fn send(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) -> bool {
        if bytes.is_empty() || !self.ready() {
            return false;
        }
        let accepted = self.enqueue(bytes);
        if accepted {
            self.engine.scroll(ScrollViewport::Bottom);
            let result = self.engine.clear_selection();
            self.engine_result(result);
            // Hand interactive input to the nonblocking transport immediately.
            // The timer remains a retry path when its bounded queue is full.
            self.flush_input();
        }
        cx.notify();
        accepted
    }
    fn engine_result<T>(&mut self, result: newport_terminal::Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.input_error = Some(format!("Terminal processing failed: {error}"));
                None
            }
        }
    }
    fn accept_effects(&mut self, result: newport_terminal::Result<Effects>) -> bool {
        if let Some(effects) = self.engine_result(result) {
            let changed = effects.redraw || effects.title.is_some() || !effects.replies.is_empty();
            self.effects(effects);
            changed
        } else {
            true
        }
    }
    fn effects(&mut self, effects: Effects) {
        if let Some(text) = effects.copy {
            self.copied = Some(text);
        }
        if let Some(title) = effects.title {
            let title = title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Terminal".into());
            self.queue_title(title, Instant::now());
        }
        // Terminal replies and user input share one bounded FIFO. Preserve byte
        // order; rejected batches never enqueue a partial paste or escape sequence.
        if self.ready() {
            for bytes in effects.replies {
                self.enqueue(bytes);
            }
        }
    }
    // Shell hooks often set a command title and restore the prompt title only
    // milliseconds later. Keep the displayed label until the new title settles.
    fn queue_title(&mut self, title: String, now: Instant) {
        if title == self.title {
            self.pending_title = None;
        } else if self
            .pending_title
            .as_ref()
            .is_none_or(|(pending, _)| *pending != title)
        {
            self.pending_title = Some((title, now));
        }
    }
    fn flush_title(&mut self, now: Instant) -> bool {
        if self
            .pending_title
            .as_ref()
            .is_some_and(|(_, since)| now.duration_since(*since) >= Duration::from_millis(150))
        {
            self.title = self.pending_title.take().expect("pending title").0;
            true
        } else {
            false
        }
    }
    fn tick(&mut self, cx: &mut Context<Self>) {
        let hidden_presentation = (!self.visible).then(|| self.tab_presentation());
        let mut changed = self.scroll_selection();
        // The transport may already be ahead of the bounded output consumer.
        // Never carry queued keys/preedit over to a replacement remote shell.
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.epoch() != self.observed_epoch)
        {
            changed |= !self.composition.is_empty();
            self.clear_input();
        }
        for _ in 0..32 {
            let event = self
                .session
                .as_ref()
                .and_then(|session| session.try_read().ok());
            let Some(event) = event else {
                break;
            };
            changed = true;
            self.receive(event);
        }
        self.report_focus();
        let result = self.engine.flush_expired();
        changed |= self.accept_effects(result);
        if let Some(text) = self.copied.take() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        changed |= self.flush_title(Instant::now());
        self.deliver_draft(Instant::now());
        self.flush_input();
        // Hidden PTYs must still consume output, answer device queries and
        // advance their lifecycle. Only tab presentation can invalidate UI
        // until the output surface is mounted again.
        if changed && hidden_presentation.is_none_or(|previous| previous != self.tab_presentation())
        {
            cx.notify();
        }
        self.refresh_find_output(changed, cx);
    }
    fn flush_input(&mut self) {
        if let Some(session) = &self.session
            && self.ready()
        {
            if self.resize_pending && session.resize(self.dimensions.0, self.dimensions.1).is_ok() {
                self.resize_pending = false;
            }
            for _ in 0..16 {
                let Some(bytes) = self.input.front() else {
                    break;
                };
                if session
                    .send_for_epoch(bytes.clone(), self.observed_epoch)
                    .is_err()
                {
                    break;
                }
                self.input_bytes -= bytes.len();
                self.input.pop_front();
            }
        }
    }
    fn accept_ready(&mut self, current: bool) {
        let initial = !self.status.started && self.observed_epoch == 0;
        self.command_eligible = false;
        self.status.ready();
        if !initial || !current {
            self.command_draft.discard();
            return;
        }
        self.draft_armed_at = Some(Instant::now());
        self.deliver_draft(Instant::now());
    }
    /// Type the waiting draft once the shell reads a line: input sent while it
    /// is still starting is echoed by the terminal and then again at the
    /// prompt. Shells that read lines turn bracketed paste on; those that
    /// don't get it after a short wait.
    fn deliver_draft(&mut self, now: Instant) {
        let Some(armed) = self.draft_armed_at else {
            return;
        };
        let reading = self.engine.mode(Mode::BRACKETED_PASTE).unwrap_or(false);
        if !reading && now.duration_since(armed) < DRAFT_FALLBACK {
            return;
        }
        self.draft_armed_at = None;
        if let Some(delivery) = self.command_draft.take() {
            use newport_client::terminal_command::CommandDelivery;
            let (command, execute) = match delivery {
                CommandDelivery::Review(command) => (command, false),
                CommandDelivery::ContainerShell(command)
                | CommandDelivery::ProjectDirectory(command) => (command, true),
            };
            let result = self.engine.encode_paste(&command);
            let Some(mut input) = self.engine_result(result) else {
                return;
            };
            if execute {
                let result = encode_key(&mut self.engine, KeyEvent::press("enter"));
                let Some(Some(enter)) = self.engine_result(result) else {
                    return;
                };
                input.extend(enter);
            }
            self.enqueue(input);
        }
    }
    fn receive(&mut self, event: Event) {
        match event {
            Event::Data(bytes) => {
                let result = self.engine.advance(&bytes);
                self.accept_effects(result);
                self.prompted |= !self.alternate_screen()
                    && self.engine.mode(Mode::BRACKETED_PASTE).unwrap_or(false);
                self.deliver_draft(Instant::now());
            }
            Event::Ready => {
                self.observed_ready = true;
                self.accept_ready(self.ready());
            }
            Event::Reconnecting { attempt, .. } => {
                // Recovery discards queued commands in the shared backend.
                // Resend the latest dimensions when the new shell is ready.
                self.resize_pending = true;
                self.observed_epoch = self.observed_epoch.wrapping_add(1);
                self.observed_ready = false;
                self.status.reconnecting(attempt);
                self.clear_input();
                let result = self.engine.reconnect();
                self.accept_effects(result);
                self.title = "Terminal".into();
            }
            Event::Exit(code) => {
                self.observed_epoch = self.observed_epoch.wrapping_add(1);
                self.observed_ready = false;
                self.status.exited(code);
                self.clear_input();
            }
            Event::Error(error) => {
                self.observed_epoch = self.observed_epoch.wrapping_add(1);
                self.observed_ready = false;
                self.status.failed(error);
                self.clear_input();
            }
        }
    }
    fn clear_input(&mut self) {
        self.link_control_held = false;
        self.pending_title = None;
        self.link_generation = self.link_generation.wrapping_add(1);
        self.link_press = None;
        self.hovered_link = None;
        self.command_eligible = false;
        self.command_draft.discard();
        self.draft_armed_at = None;
        self.focus_reporter = FocusReporter::default();
        self.input_error = None;
        self.remote_pressed = PressedButtons::default();
        self.forwarded_keys.clear();
        self.last_motion = None;
        self.selecting = false;
        self.selection_pointer = None;
        self.scroll_remainder = 0.;
        self.input.clear();
        self.input_bytes = 0;
        self.clear_composition();
    }
    fn clear_composition(&mut self) {
        self.composition.clear();
        self.composition_selection = 0..0;
    }
    fn sync_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_focused = self.focused;
        self.focused = self.visible
            && self.focus.is_focused(window)
            && window.is_window_active()
            && !window.has_active_dialog(cx);
        if was_focused && !self.focused {
            self.link_control_held = false;
            self.link_press = None;
            self.release_keys(
                window.modifiers(),
                window.capslock().on,
                window.numlock().unwrap_or(false),
            );
            self.selecting = false;
            self.selection_pointer = None;
            if !self.composition.is_empty() {
                self.clear_composition();
                cx.notify();
            }
        }
        self.report_focus();
    }
    pub fn deactivate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.link_control_held = false;
        self.link_press = None;
        self.hovered_link = None;
        self.clear_find(cx);
        self.visible = false;
        self.focused = false;
        self.release_keys(
            window.modifiers(),
            window.capslock().on,
            window.numlock().unwrap_or(false),
        );
        self.selecting = false;
        self.selection_pointer = None;
        if !self.composition.is_empty() {
            self.clear_composition();
            cx.notify();
        }
        self.report_focus();
    }
    fn report_focus(&mut self) {
        let enabled = self.ready() && self.engine.mode(Mode::FOCUS_EVENT).unwrap_or(false);
        let previous = self.focus_reporter;
        if let Some(bytes) = self.focus_reporter.update(enabled, self.focused)
            && !self.enqueue(bytes.to_vec())
        {
            // Retry after queue pressure subsides, without reordering queued input.
            self.focus_reporter = previous;
        }
    }
    fn measure(&mut self, bounds: Bounds<Pixels>, cell: Size<Pixels>, cx: &mut Context<Self>) {
        let inner = size(
            bounds.size.width - PADDING.width * 2.,
            bounds.size.height - PADDING.height * 2.,
        );
        // Hidden/zero-sized views must not open a placeholder-sized remote PTY.
        if inner.width < cell.width * 2. || inner.height < cell.height {
            return;
        }
        self.bounds = bounds;
        let dimensions = (
            (f32::from(inner.width) / f32::from(cell.width))
                .floor()
                .clamp(2., 500.) as u16,
            (f32::from(inner.height) / f32::from(cell.height))
                .floor()
                .clamp(1., 300.) as u16,
        );
        // Center the grid: the padding and the space left over by whole
        // cells are shared equally between opposite edges.
        let grid = size(
            cell.width * f32::from(dimensions.0),
            cell.height * f32::from(dimensions.1),
        );
        self.grid_origin = bounds.origin
            + point(
                ((bounds.size.width - grid.width) / 2.).floor(),
                ((bounds.size.height - grid.height) / 2.).floor(),
            );
        if dimensions != self.dimensions || self.cell != cell {
            self.dimensions = dimensions;
            self.cell = cell;
            self.resize_pending = true;
            let result = self.engine.resize(
                dimensions.0,
                dimensions.1,
                f32::from(cell.width).ceil().max(1.) as u16,
                f32::from(cell.height).ceil().max(1.) as u16,
            );
            self.accept_effects(result);
            self.refresh_find(false, false, cx);
            cx.notify();
        }
        if self.open_on_show {
            self.open(cx);
            cx.notify();
        }
    }
    fn cursor_bounds(&self) -> Bounds<Pixels> {
        let cursor = self
            .engine
            .cursor_position()
            .unwrap_or(PointCoordinate { x: 0, y: 0 });
        Bounds::new(
            self.grid_origin
                + point(
                    self.cell.width * cursor.x as f32,
                    self.cell.height * cursor.y as f32,
                ),
            self.cell,
        )
    }
    fn selection_position(&self, position: Point<Pixels>) -> SelectionPosition {
        SelectionPosition {
            column: f64::from(
                f32::from(position.x - self.grid_origin.x) / f32::from(self.cell.width),
            ),
            row: f64::from(
                f32::from(position.y - self.grid_origin.y) / f32::from(self.cell.height),
            ),
        }
    }
    fn position(&self, position: Point<Pixels>) -> TermPoint {
        let position = self.selection_position(position);
        TermPoint::Viewport(PointCoordinate {
            x: position
                .column
                .floor()
                .clamp(0., f64::from(self.dimensions.0 - 1)) as u16,
            y: position
                .row
                .floor()
                .clamp(0., f64::from(self.dimensions.1 - 1)) as u32,
        })
    }
    fn link_at(&mut self, position: Point<Pixels>) -> Option<String> {
        if !self.bounds.contains(&position) {
            return None;
        }
        let TermPoint::Viewport(point) = self.position(position) else {
            return None;
        };
        self.engine
            .frame()
            .ok()?
            .link_at(usize::from(point.x), point.y as usize)
            .map(str::to_owned)
    }
    fn update_link_hover(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        self.link_control_held = modifiers.control;
        let link = link_modifier(modifiers)
            .then(|| self.link_at(position))
            .flatten();
        if self.hovered_link != link {
            self.hovered_link = link;
            cx.notify();
        }
    }
    fn update_selection(&mut self, position: Point<Pixels>) {
        let result = self
            .engine
            .update_selection(self.selection_position(position));
        self.engine_result(result);
    }
    fn scroll_selection(&mut self) -> bool {
        if !self.selecting
            || !self.visible
            || self.selection_scroll_at.elapsed() < Duration::from_millis(50)
        {
            return false;
        }
        let Some(position) = self.selection_pointer else {
            return false;
        };
        let top = self.grid_origin.y;
        let bottom = top + self.cell.height * f32::from(self.dimensions.1);
        let distance = if position.y < top {
            f32::from(top - position.y)
        } else if position.y >= bottom {
            -f32::from(position.y - bottom) - 1.
        } else {
            return false;
        };
        self.selection_scroll_at = Instant::now();
        let lines = (distance / f32::from(self.cell.height))
            .abs()
            .ceil()
            .clamp(1., 15.) as i32
            * if distance > 0. { 1 } else { -1 };
        let before = self.engine.viewport_top().unwrap_or(0);
        self.engine.scroll(ScrollViewport::Delta(-(lines as isize)));
        self.update_selection(position);
        self.engine.viewport_top().unwrap_or(0) != before
    }
    fn mouse_reporting(&self, modifiers: Modifiers) -> bool {
        !modifiers.shift
            && [
                Mode::X10_MOUSE,
                Mode::NORMAL_MOUSE,
                Mode::BUTTON_MOUSE,
                Mode::ANY_MOUSE,
            ]
            .into_iter()
            .any(|mode| self.engine.mode(mode).unwrap_or(false))
    }
    fn report_mouse(
        &mut self,
        action: MouseAction,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        if !self.ready() {
            return;
        }
        let x = f32::from(position.x - self.grid_origin.x) / f32::from(self.cell.width)
            * f32::from(self.cell.width).ceil().max(1.);
        let y = f32::from(position.y - self.grid_origin.y) / f32::from(self.cell.height)
            * f32::from(self.cell.height).ceil().max(1.);
        let result = terminal_mouse::encode(
            &mut self.engine,
            action,
            x,
            y,
            modifiers.alt,
            modifiers.control,
        );
        let Some(bytes) = self.engine_result(result) else {
            return;
        };
        if bytes.is_empty() {
            return;
        }
        if matches!(action, MouseAction::Move(_)) {
            if self.last_motion.as_ref() == Some(&bytes) {
                return;
            }
            self.last_motion = Some(bytes.clone());
        } else {
            self.last_motion = None;
        }
        self.send(bytes, cx);
    }
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        self.focus.focus(window, cx);
        let mac_control_click = cfg!(target_os = "macos")
            && event.button == MouseButton::Right
            && self.link_control_held;
        let mut link_modifiers = event.modifiers;
        link_modifiers.control |= mac_control_click;
        if (event.button == MouseButton::Left || mac_control_click)
            && event.click_count == 1
            && link_modifier(link_modifiers)
            && let Some(link) = self.link_at(event.position)
        {
            self.selecting = false;
            self.selection_pointer = None;
            self.link_press = Some(LinkPress {
                link,
                button: event.button,
                mac_control_click,
                position: event.position,
                cancelled: false,
            });
            cx.stop_propagation();
            return;
        }
        if self.mouse_reporting(event.modifiers) {
            self.selecting = false;
            self.selection_pointer = None;
            if let Some(button) = terminal_button(event.button) {
                self.remote_pressed.press(button);
                self.report_mouse(
                    MouseAction::Press(button),
                    event.position,
                    event.modifiers,
                    cx,
                );
            }
            cx.stop_propagation();
        } else if event.button == MouseButton::Left {
            self.selecting = true;
            self.selection_pointer = Some(event.position);
            self.selection_scroll_at = Instant::now();
            let result = self.engine.begin_selection(
                self.selection_position(event.position),
                event.click_count,
                event.modifiers.shift,
                event.modifiers.alt,
            );
            self.engine_result(result);
            cx.notify();
        }
    }
    fn mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.link_press.as_ref().is_some_and(|press| {
            event.button == press.button
                // Releasing Ctrl first makes GPUI deliver the physical Left up.
                || (press.mac_control_click && event.button == MouseButton::Left)
        }) && let Some(press) = self.link_press.take()
        {
            let mut modifiers = event.modifiers;
            if press.mac_control_click {
                modifiers.control = self.link_control_held;
            }
            if !press.cancelled
                && !window.has_active_dialog(cx)
                && link_modifier(modifiers)
                && self.link_at(event.position).as_ref() == Some(&press.link)
            {
                cx.emit(OpenLink(press.link));
            }
            cx.stop_propagation();
            return;
        }
        if self.selecting && event.button == MouseButton::Left {
            let result = self
                .engine
                .end_selection(Some(self.selection_position(event.position)));
            self.engine_result(result);
        }
        self.selecting = false;
        self.selection_pointer = None;
        if let Some(button) = terminal_button(event.button)
            && self.remote_pressed.release(button)
            && !window.has_active_dialog(cx)
        {
            // Finish a remote drag even if Shift was pressed mid-gesture.
            self.report_mouse(
                MouseAction::Release(button),
                event.position,
                event.modifiers,
                cx,
            );
            cx.stop_propagation();
        }
    }
    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        self.update_link_hover(event.position, event.modifiers, cx);
        if let Some(press) = self.link_press.as_mut() {
            let delta = event.position - press.position;
            press.cancelled |= delta.x.abs() > px(4.) || delta.y.abs() > px(4.);
            cx.stop_propagation();
            return;
        }
        if !self.selecting && self.mouse_reporting(event.modifiers) {
            self.report_mouse(
                MouseAction::Move(self.remote_pressed.held()),
                event.position,
                event.modifiers,
                cx,
            );
            return;
        }
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            self.selection_pointer = None;
            return;
        }
        self.selection_pointer = Some(event.position);
        self.update_selection(event.position);
        cx.notify();
    }
    fn mouse_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        self.link_press = None;
        self.hovered_link = None;
        self.scroll_remainder +=
            f32::from(event.delta.pixel_delta(self.cell.height).y) / f32::from(self.cell.height);
        let lines = self.scroll_remainder.trunc() as i32;
        self.scroll_remainder -= lines as f32;
        if self.mouse_reporting(event.modifiers) {
            let action = if lines > 0 {
                MouseAction::WheelUp
            } else {
                MouseAction::WheelDown
            };
            for _ in 0..lines.unsigned_abs().min(100) {
                self.report_mouse(action, event.position, event.modifiers, cx);
            }
        } else if !event.modifiers.shift
            && self.engine.alternate_screen().unwrap_or(false)
            && self.engine.mode(Mode::ALT_SCROLL).unwrap_or(false)
        {
            let key = if lines > 0 { "up" } else { "down" };
            if let Ok(Some(bytes)) = encode_key(&mut self.engine, KeyEvent::press(key)) {
                self.send(bytes.repeat(lines.unsigned_abs().min(100) as usize), cx);
            }
        } else if lines != 0 {
            self.engine.scroll(ScrollViewport::Delta(-(lines as isize)));
            cx.notify();
        }
        cx.stop_propagation();
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window)
            || window.has_active_dialog(cx)
            || !self.composition.is_empty()
            || event.prefer_character_input
        {
            return;
        }
        let key = &event.keystroke;
        if let Some(shortcut) = local_shortcut(key) {
            match shortcut {
                LocalShortcut::Copy => {
                    if let Ok(Some(text)) = self.engine.selected_text() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                }
                LocalShortcut::Paste => {
                    if self.ready()
                        && let Some(text) = cx.read_from_clipboard().and_then(|c| c.text())
                    {
                        let result = self.engine.encode_paste(&text);
                        if let Some(bytes) = self.engine_result(result) {
                            self.send(bytes, cx);
                        }
                    }
                }
            }
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        // Legacy protocols cannot represent Super. Do not turn Cmd/Super
        // into plain text, but let applications requesting Kitty receive it.
        if key.modifiers.platform
            && !self
                .engine
                .kitty_keyboard_flags()
                .is_ok_and(|flags| !flags.is_empty())
        {
            return;
        }
        if !self.ready() {
            return;
        }
        let identity = ForwardedKey::new(&event.physical_key, &key.key);
        // The logical key can change while held (Shift/layout changes). Repeat
        // and release must refer to the same key the remote application saw.
        let option_as_meta = option_as_meta(cx);
        let mut pressed = self
            .forwarded_keys
            .get(&identity)
            .cloned()
            .unwrap_or_else(|| ForwardedPress {
                logical: key.key.clone(),
                native: native_key_identity(
                    event.physical_key.as_ref(),
                    event.key_metadata.as_ref(),
                ),
                option_as_meta,
            });
        // Only a repeat keeps the press's Option mode; a new press reads the
        // current setting.
        if !event.is_held {
            pressed.option_as_meta = option_as_meta;
        }
        let result = encode_key(
            &mut self.engine,
            KeyEvent {
                key: &pressed.logical,
                native_identity: pressed.native,
                native_modifiers: native_modifiers(event.key_metadata.as_ref()).or_else(|| {
                    key.modifiers
                        .platform
                        .then(|| current_modifiers(key.modifiers, false, false))
                }),
                text: key.key_char.as_deref(),
                shift: key.modifiers.shift,
                alt: key.modifiers.alt,
                control: key.modifiers.control,
                action: if event.is_held {
                    newport_terminal::KeyAction::Repeat
                } else {
                    newport_terminal::KeyAction::Press
                },
                composing: false,
                option_as_meta: pressed.option_as_meta,
            },
        );
        if let Some(Some(bytes)) = self.engine_result(result) {
            if self.send(bytes, cx) {
                self.forwarded_keys.insert(identity, pressed);
            }
            window.prevent_default();
            cx.stop_propagation();
        }
    }
    fn release_keys(&mut self, modifiers: Modifiers, caps_lock: bool, num_lock: bool) {
        for (_, key) in std::mem::take(&mut self.forwarded_keys) {
            let mut event = KeyEvent::press(&key.logical);
            event.native_identity = key.native;
            event.option_as_meta = key.option_as_meta;
            // Unsupported backends report no Num Lock state; callers use false
            // there. Supported backends query current state at focus loss.
            event.native_modifiers = Some(current_modifiers(modifiers, caps_lock, num_lock));
            event.action = newport_terminal::KeyAction::Release;
            event.shift = modifiers.shift;
            event.alt = modifiers.alt;
            event.control = modifiers.control;
            if let Ok(Some(bytes)) = encode_key(&mut self.engine, event)
                && self.ready()
                && !bytes.is_empty()
            {
                self.enqueue(bytes);
            }
        }
    }
    fn key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let identity = ForwardedKey::new(&event.physical_key, &key.key);
        let Some(pressed) = self.forwarded_keys.remove(&identity) else {
            return;
        };
        let result = encode_key(
            &mut self.engine,
            KeyEvent {
                key: &pressed.logical,
                native_identity: pressed.native,
                native_modifiers: native_modifiers(event.key_metadata.as_ref()).or_else(|| {
                    key.modifiers
                        .platform
                        .then(|| current_modifiers(key.modifiers, false, false))
                }),
                text: None,
                shift: key.modifiers.shift,
                alt: key.modifiers.alt,
                control: key.modifiers.control,
                action: newport_terminal::KeyAction::Release,
                composing: false,
                option_as_meta: pressed.option_as_meta,
            },
        );
        if let Some(Some(bytes)) = self.engine_result(result) {
            // A key release should not move a viewport the user scrolled after
            // pressing the key or clear a subsequently created selection.
            if self.ready() && !bytes.is_empty() && self.enqueue(bytes) {
                self.flush_input();
            }
            window.prevent_default();
            cx.stop_propagation();
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LocalShortcut {
    Copy,
    Paste,
}

fn local_shortcut(key: &Keystroke) -> Option<LocalShortcut> {
    let modifiers = key.modifiers;
    if modifiers.alt || modifiers.function {
        return None;
    }
    if cfg!(target_os = "macos") && modifiers.platform && !modifiers.control && !modifiers.shift {
        return match key.key.as_str() {
            "c" => Some(LocalShortcut::Copy),
            "v" => Some(LocalShortcut::Paste),
            _ => None,
        };
    }
    if !cfg!(target_os = "macos") && !modifiers.platform {
        // Ctrl+C and Ctrl+V retain their shell meanings. These clipboard
        // gestures are handled before the terminal key encoder.
        return match key.key.as_str() {
            "c" if modifiers.control && modifiers.shift => Some(LocalShortcut::Copy),
            "v" if modifiers.control && modifiers.shift => Some(LocalShortcut::Paste),
            _ => None,
        };
    }
    None
}

fn terminal_button(button: MouseButton) -> Option<TerminalButton> {
    match button {
        MouseButton::Left => Some(TerminalButton::Left),
        MouseButton::Middle => Some(TerminalButton::Middle),
        MouseButton::Right => Some(TerminalButton::Right),
        _ => None,
    }
}
impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut link_modifiers = window.modifiers();
        if cfg!(target_os = "macos") {
            link_modifiers.control |= self.link_control_held;
        }
        self.hovered_link = link_modifier(link_modifiers)
            .then(|| self.link_at(window.mouse_position()))
            .flatten();
        self.visible = true;
        if self.focus_subscriptions.is_empty() {
            let focus = self.focus.clone();
            self.focus_subscriptions = vec![
                cx.on_focus(&focus, window, Self::sync_focus),
                cx.on_blur(&focus, window, Self::sync_focus),
                cx.observe_window_activation(window, Self::sync_focus),
            ];
        }
        self.sync_focus(window, cx);
        self.dark = cx.theme().is_dark();
        let result = self
            .engine
            .set_theme(crate::terminal_paint::theme(self.dark));
        self.engine_result(result);
        let cell = crate::terminal_paint::cell_size(window, cx);
        let entity = cx.entity();
        let focus = self.focus.clone();
        let measure = entity.clone();
        // The painter owns its draw data. Build it while borrowing the cached
        // frame so normal repaints don't clone every cell and hyperlink twice.
        let alternate_screen = self.alternate_screen();
        let paint = self.engine.frame().map(|frame| {
            let lines = frame.visible_lines();
            let grid = crate::terminal_paint::render_grid(
                frame,
                self.dark,
                cell,
                self.grid_origin - self.bounds.origin,
                alternate_screen,
                self.find
                    .as_ref()
                    .map(|find| find.results.matches.as_slice())
                    .unwrap_or(&[]),
                self.find.as_ref().and_then(|find| {
                    find.active
                        .and_then(|index| find.results.matches.get(index))
                }),
                self.hovered_link.as_deref(),
                self.focused,
            );
            (lines, grid)
        });
        let (accessible_lines, grid) = match self.engine_result(paint) {
            Some((lines, grid)) => (lines, Some(grid)),
            None => (Vec::new(), None),
        };
        div()
            .id("ssh-terminal")
            .key_context("Terminal")
            .role(Role::Terminal)
            .aria_label("Remote terminal")
            .a11y_synthetic_children(move |builder| {
                builder.parent_node().set_value(accessible_lines.join("\n"));
                for (index, line) in accessible_lines.iter().enumerate() {
                    let value = if index + 1 < accessible_lines.len() {
                        format!("{line}\n")
                    } else {
                        line.clone()
                    };
                    let mut node = accesskit::Node::new(Role::TextRun);
                    node.set_character_lengths(
                        value
                            .chars()
                            .map(|c| c.len_utf8() as u8)
                            .collect::<Vec<_>>(),
                    );
                    node.set_value(value);
                    let id = builder.synthetic_node_id(("terminal-row", index));
                    builder.push_child(id, node);
                }
            })
            .size_full()
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &crate::FindInTerminal, window, cx| {
                if !window.has_active_dialog(cx) && this.composition.is_empty() {
                    this.open_find(window, cx);
                }
            }))
            // The terminal leaves Cmd keys to the remote program, so macOS
            // offers them to the menu. The editor's Cmd shortcuts must not
            // act from here: take their commands and do nothing.
            .on_action(|_: &crate::project_editor::SaveFile, _, _| {})
            .on_action(|_: &crate::project_editor::CloseTab, _, _| {})
            .on_action(|_: &crate::project_editor::QuickOpen, _, _| {})
            .on_action(|_: &crate::project_editor::ToggleFiles, _, _| {})
            .on_action(|_: &crate::project_editor::FocusExplorer, _, _| {})
            .on_key_down(cx.listener(Self::key))
            .on_key_up(cx.listener(Self::key_up))
            .on_modifiers_changed(
                cx.listener(|this, event: &ModifiersChangedEvent, window, cx| {
                    this.update_link_hover(window.mouse_position(), event.modifiers, cx);
                }),
            )
            .map(|mut div| {
                for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
                    div = div
                        .on_mouse_down(button, cx.listener(Self::mouse_down))
                        .on_mouse_up(button, cx.listener(Self::mouse_up))
                        .on_mouse_up_out(button, cx.listener(Self::mouse_up));
                }
                div
            })
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.hovered_link.take().is_some() {
                    cx.notify();
                }
            }))
            .cursor_text()
            .when_some(self.hovered_link.as_ref(), |view, link| {
                let url = link.clone();
                view.cursor_pointer()
                    .hover_path("Ctrl-click or Cmd-click to open", url)
            })
            .on_scroll_wheel(cx.listener(Self::mouse_scroll))
            .when_some(grid, |view, grid| view.child(grid))
            .when(!self.composition.is_empty(), |div| {
                let cursor = self.cursor_bounds().origin - self.bounds.origin;
                div.child(
                    gpui_kit::div()
                        .absolute()
                        .left(cursor.x)
                        .top(cursor.y)
                        .bg(cx.theme().background)
                        .text_color(cx.theme().foreground)
                        .child(self.composition.clone()),
                )
            })
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let entity = measure.clone();
                        window.defer(cx, move |_, cx| {
                            entity.update(cx, |this, cx| this.measure(bounds, cell, cx))
                        });
                    },
                    move |bounds, _, window, cx| {
                        let drag = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture
                                && drag.read(cx).selecting
                                && !drag.read(cx).bounds.contains(&event.position)
                            {
                                drag.update(cx, |this, cx| this.mouse_move(event, window, cx));
                            }
                        });
                        if !window.has_active_dialog(cx) {
                            window.handle_input(
                                &focus,
                                ElementInputHandler::new(bounds, entity),
                                cx,
                            );
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .when(self.find.is_some(), |root| root.child(self.find_bar(cx)))
            .when_some(self.card.clone(), |root, card| {
                root.child(
                    div()
                        .id("terminal-card")
                        .absolute()
                        // Below the find bar when both are open.
                        .top(px(if self.find.is_some() { 52. } else { 8. }))
                        .right(px(12.))
                        .w(px(300.))
                        .max_w((self.bounds.size.width - px(24.)).max(px(160.)))
                        .cursor_default()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                        .on_mouse_move(|_, _, cx| cx.stop_propagation())
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .child(card),
                )
            })
    }
}
impl EntityInputHandler for TerminalView {
    fn accepts_text_input(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.visible
            && self.focus.is_focused(window)
            && !window.has_active_dialog(cx)
            && self.ready()
    }
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let units: Vec<u16> = self.composition.encode_utf16().collect();
        if range.start > range.end || range.end > units.len() {
            return None;
        }
        *actual = Some(range.clone());
        String::from_utf16(&units[range]).ok()
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.composition_selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.composition.is_empty()).then(|| 0..self.composition.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_composition();
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_composition();
        // Platform callbacks may outlive the frame/focus that registered them.
        // Never deliver those commits to an inactive terminal session.
        if self.accepts_text_input(window, cx) {
            self.send(text.as_bytes().to_vec(), cx);
        }
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_text_input(window, cx) {
            return;
        }
        let mut units: Vec<u16> = self.composition.encode_utf16().collect();
        let range = range.unwrap_or(0..units.len());
        if range.start > range.end || range.end > units.len() {
            return;
        }
        let replacement: Vec<u16> = text.encode_utf16().collect();
        let length = replacement.len();
        units.splice(range.clone(), replacement);
        let Ok(composition) = String::from_utf16(&units) else {
            return;
        };
        self.composition = composition;
        self.composition_selection = selected
            .map(|r| range.start + r.start.min(length)..range.start + r.end.min(length))
            .unwrap_or(range.start + length..range.start + length);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(self.cursor_bounds())
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.composition_selection.end)
    }
}

#[cfg(test)]
#[path = "terminal_selection_tests.rs"]
mod selection_tests;

#[cfg(test)]
#[path = "terminal_input_tests.rs"]
pub(crate) mod input_tests;

#[cfg(test)]
mod tests {
    use super::{DRAFT_FALLBACK, Event, TerminalView};
    use gpui_kit as gpui;
    use gpui_kit::{AppContext, Entity, TestAppContext};
    use newport_client::terminal_command::CommandDraft;
    use std::time::Instant;

    #[gpui::test]
    fn a_running_command_is_told_apart_from_the_prompt(cx: &mut TestAppContext) {
        let view = fixture(cx);
        let feed = |cx: &mut TestAppContext, bytes: &[u8]| {
            view.update(cx, |view, _| view.receive(Event::Data(bytes.to_vec())));
            view.read_with(cx, |view, _| view.running_command())
        };
        // A shell without bracketed paste gives no sign either way.
        assert!(!feed(cx, b"$ "));
        assert!(!feed(cx, b"\x1b[?2004h$ "));
        assert!(feed(cx, b"\x1b[?2004lsleep 600\r\n"));
        assert!(feed(cx, b"\x1b[?1049h"));
        assert!(!feed(cx, b"\x1b[?1049l\x1b[?2004h$ "));
    }

    fn fixture(cx: &mut TestAppContext) -> Entity<TerminalView> {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
            // Never render/open: these tests inspect delivery bytes without a PTY.
            cx.new(|cx| TerminalView::new("unconnected-fixture".into(), cx))
        })
    }

    fn draft(shell: bool) -> CommandDraft {
        if shell {
            CommandDraft::container_shell("docker exec -it -- abcdef sh".into()).unwrap()
        } else {
            CommandDraft::new("sudo systemctl restart app.service".into()).unwrap()
        }
    }

    #[gpui::test]
    fn initial_shell_ready_executes_once_while_service_commands_remain_review_only(
        cx: &mut TestAppContext,
    ) {
        for shell in [false, true] {
            for bracketed in [false, true] {
                let view = fixture(cx);
                view.update(cx, |view, _| {
                    assert!(view.prepare_command(draft(shell)));
                    assert!(!view.prepare_command(draft(!shell)));
                    assert!(view.input.is_empty());
                    if bracketed {
                        view.engine.advance(b"\x1b[?2004h").unwrap();
                    }
                    view.accept_ready(true);
                    if !bracketed {
                        // A shell that never turns bracketed paste on gets
                        // the draft after the fallback wait.
                        assert!(view.input.is_empty());
                        view.deliver_draft(Instant::now() + DRAFT_FALLBACK);
                    }
                    let command = if shell {
                        "docker exec -it -- abcdef sh"
                    } else {
                        "sudo systemctl restart app.service"
                    };
                    let mut expected = if bracketed {
                        format!("\x1b[200~{command}\x1b[201~").into_bytes()
                    } else {
                        command.as_bytes().to_vec()
                    };
                    if shell {
                        expected.push(b'\r');
                    }
                    assert_eq!(
                        view.input.iter().flatten().copied().collect::<Vec<_>>(),
                        expected
                    );
                    assert_eq!(view.input_bytes, expected.len());
                    view.accept_ready(true);
                    assert_eq!(
                        view.input.iter().flatten().copied().collect::<Vec<_>>(),
                        expected
                    );
                    assert!(!view.prepare_command(draft(shell)));
                    assert!(view.session.is_none());
                });
            }
        }
    }

    #[gpui::test]
    fn a_draft_waits_for_the_shell_to_read_a_line(cx: &mut TestAppContext) {
        let view = fixture(cx);
        view.update(cx, |view, _| {
            assert!(view.prepare_command(draft(false)));
            view.accept_ready(true);
            // Startup output before the prompt doesn't deliver it.
            view.receive(Event::Data(b"Welcome\r\n".to_vec()));
            assert!(view.input.is_empty());
            // The prompt turns bracketed paste on: now it is typed, once.
            view.receive(Event::Data(b"$ \x1b[?2004h".to_vec()));
            let expected = b"\x1b[200~sudo systemctl restart app.service\x1b[201~".to_vec();
            assert_eq!(
                view.input.iter().flatten().copied().collect::<Vec<_>>(),
                expected
            );
            view.deliver_draft(Instant::now() + DRAFT_FALLBACK);
            assert_eq!(view.input_bytes, expected.len());
        });
    }

    #[gpui::test]
    fn reconnect_error_and_exit_discard_pending_or_queued_handover(cx: &mut TestAppContext) {
        for shell in [false, true] {
            for already_ready in [false, true] {
                for event in [
                    Event::Reconnecting {
                        attempt: 1,
                        message: "fixture retry".into(),
                    },
                    Event::Error("fixture error".into()),
                    Event::Exit(Some(1)),
                ] {
                    let view = fixture(cx);
                    view.update(cx, |view, _| {
                        assert!(view.prepare_command(draft(shell)));
                        if already_ready {
                            view.engine.advance(b"\x1b[?2004h").unwrap();
                            view.accept_ready(true);
                            assert!(!view.input.is_empty());
                        } else {
                            // Ready, but still waiting for the prompt.
                            view.accept_ready(true);
                            assert!(view.input.is_empty());
                        }
                        view.receive(event);
                        view.accept_ready(true);
                        view.deliver_draft(Instant::now() + DRAFT_FALLBACK);
                        assert!(view.input.is_empty());
                        assert_eq!(view.input_bytes, 0);
                        assert!(!view.prepare_command(draft(shell)));
                    });
                }
            }
        }
    }

    #[gpui::test]
    fn manual_reconnect_connection_reset_and_close_cannot_rearm_a_shell(cx: &mut TestAppContext) {
        for lifecycle in 0..3 {
            let view = fixture(cx);
            view.update(cx, |view, cx| {
                assert!(view.prepare_command(draft(true)));
                match lifecycle {
                    0 => view.reconnect(cx),
                    1 => view.reset_connection(cx),
                    _ => view.close(),
                }
                assert!(!view.prepare_command(draft(true)));
                view.accept_ready(true);
                assert!(view.input.is_empty());
                assert!(view.session.is_none());
            });
        }
    }

    #[gpui::test]
    fn stale_ready_discards_launch_and_full_queue_cannot_enqueue_a_partial_command(
        cx: &mut TestAppContext,
    ) {
        let view = fixture(cx);
        view.update(cx, |view, _| {
            assert!(view.prepare_command(draft(true)));
            // Ready is no longer valid when transport has already moved on.
            view.accept_ready(false);
            view.accept_ready(true);
            assert!(view.input.is_empty());
        });
        let view = fixture(cx);
        view.update(cx, |view, _| {
            assert!(view.prepare_command(draft(true)));
            view.engine.advance(b"\x1b[?2004h").unwrap();
            assert!(view.enqueue(vec![b'x'; 1024 * 1024 - 1]));
            let before = view.input.clone();
            view.accept_ready(true);
            assert_eq!(view.input, before);
            assert!(view.input_error.is_some());
            view.input.clear();
            view.input_bytes = 0;
            view.accept_ready(true);
            assert!(view.input.is_empty());
        });
    }
    #[gpui::test]
    fn transient_shell_titles_do_not_flash_but_persistent_titles_update(cx: &mut TestAppContext) {
        use std::time::{Duration, Instant};
        let view = fixture(cx);
        view.update(cx, |view, _| {
            let now = Instant::now();
            view.title = "user@host:~".into();
            // The captured shell sequence restores its prompt title almost at once.
            view.queue_title(":".into(), now);
            assert!(!view.flush_title(now + Duration::from_millis(20)));
            assert_eq!(view.title, "user@host:~");
            view.queue_title("user@host:~".into(), now + Duration::from_millis(24));
            assert!(!view.flush_title(now + Duration::from_secs(1)));
            assert_eq!(view.title, "user@host:~");
            view.queue_title("vim".into(), now);
            // Repeated identical OSC updates must not postpone a stable title.
            view.queue_title("vim".into(), now + Duration::from_millis(100));
            assert!(view.flush_title(now + Duration::from_millis(150)));
            assert_eq!(view.title, "vim");
            assert!(!view.flush_title(now + Duration::from_secs(1)));
            view.queue_title("user@host:~/project".into(), now);
            view.queue_title("user@host:~/other".into(), now + Duration::from_millis(100));
            assert!(!view.flush_title(now + Duration::from_millis(200)));
            assert!(view.flush_title(now + Duration::from_millis(250)));
            assert_eq!(view.title, "user@host:~/other");
        });
    }

    #[gpui::test]
    fn pending_titles_cannot_leak_across_connection_lifecycle(cx: &mut TestAppContext) {
        use std::time::{Duration, Instant};
        for action in 0..5 {
            let view = fixture(cx);
            view.update(cx, |view, cx| {
                let now = Instant::now();
                view.queue_title("old session".into(), now);
                match action {
                    0 => view.close(),
                    1 => view.reconnect(cx),
                    2 => view.reset_connection(cx),
                    3 => view.receive(Event::Exit(Some(0))),
                    _ => view.receive(Event::Reconnecting {
                        attempt: 1,
                        message: "retry".into(),
                    }),
                }
                assert!(!view.flush_title(now + Duration::from_secs(1)));
                assert_ne!(view.title, "old session");
            });
        }
    }
    #[gpui::test]
    fn osc_titles_settle_on_idle_ticks_and_reset_to_default(cx: &mut TestAppContext) {
        use std::time::{Duration, Instant};
        let view = fixture(cx);
        view.update(cx, |view, cx| {
            view.receive(Event::Data(b"\x1b]2;vim\x07".to_vec()));
            assert_eq!(view.title, "Terminal");
            view.pending_title.as_mut().unwrap().1 = Instant::now() - Duration::from_millis(150);
            view.tick(cx);
            assert_eq!(view.title, "vim");
            // An empty OSC title still restores the default after settling.
            view.receive(Event::Data(b"\x1b]2;\x07".to_vec()));
            assert_eq!(view.title, "vim");
            view.pending_title.as_mut().unwrap().1 = Instant::now() - Duration::from_millis(150);
            view.tick(cx);
            assert_eq!(view.title, "Terminal");
        });
    }
    #[gpui::test]
    fn copying_in_tmux_reaches_the_clipboard_but_reading_it_never_answers(cx: &mut TestAppContext) {
        let view = fixture(cx);
        view.update(cx, |view, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("mine".into()));
            // Exactly what tmux 3.8 writes for a copy (captured from
            // `tmux set-buffer -w "copied from tmux"` in an xterm-256color PTY).
            view.receive(Event::Data(
                b"\x1b]52;;Y29waWVkIGZyb20gdG11eA==\x07".to_vec(),
            ));
            view.tick(cx);
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("copied from tmux")
            );
            view.receive(Event::Data(b"\x1b]52;c;?\x07".to_vec()));
            view.tick(cx);
            assert!(view.input.is_empty(), "no reply carries the clipboard");
        });
    }
}
