//! Remote file browsing. New selections supersede in-flight reads.
use super::*;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::Selectable;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use newport_client::file_transfer::Direction;
use std::{cell::RefCell, rc::Rc};
#[path = "file_list.rs"]
mod list;

#[cfg(test)]
#[path = "file_poll_tests.rs"]
mod poll_tests;

/// The places panel hides itself below this width; the toolbar can still show it.
const PLACES_MIN_CONTENT: f32 = 640.;
/// Below this width the preview replaces the list instead of sitting beside it.
const PREVIEW_SIDE_MIN_CONTENT: f32 = 760.;

fn places_width(content_width: f32) -> f32 {
    if content_width < 900. { 160. } else { 188. }
}

fn preview_width(content_width: f32) -> f32 {
    (content_width * 0.42).clamp(320., 620.)
}

/// Identity painted into a file control. Retained callbacks must not resolve
/// their target through the mutable selected server or a replacement listing.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FileTarget {
    pub server: String,
    pub connection: Value,
    directory: String,
    pub path: String,
    kind: Option<String>,
}
impl FileTarget {
    pub(super) fn from_listing(
        server: String,
        connection: Value,
        listing: &Value,
        path: &str,
    ) -> Option<Self> {
        let directory = listing["path"].as_str().filter(|path| !path.is_empty())?;
        let kind = if path == directory {
            None
        } else {
            Some(
                listing["entries"]
                    .as_array()?
                    .iter()
                    .find(|entry| entry["path"] == path)?["kind"]
                    .as_str()?
                    .to_owned(),
            )
        };
        Some(Self {
            server,
            connection,
            directory: directory.to_owned(),
            path: path.to_owned(),
            kind,
        })
    }

    /// Derive painted row identities from the current query projection. The
    /// directory target is validated once per render; callbacks still call
    /// `file_target_current`, which checks membership in the latest listing.
    fn for_entry(&self, entry: &Value) -> Option<Self> {
        if self.kind.is_some() || self.path != self.directory {
            return None;
        }
        let path = entry["path"].as_str()?;
        let kind = if path == self.directory {
            None
        } else {
            Some(entry["kind"].as_str()?.to_owned())
        };
        Some(Self {
            server: self.server.clone(),
            connection: self.connection.clone(),
            directory: self.directory.clone(),
            path: path.to_owned(),
            kind,
        })
    }

    pub(super) fn supports_transfer(&self, direction: Direction) -> bool {
        match direction {
            Direction::Upload => self.kind.is_none(),
            Direction::Download => matches!(self.kind.as_deref(), Some("file" | "symlink")),
        }
    }
}

pub struct Browser {
    pub connection: Value,
    controls_connection: Value,
    pub(super) reads: newport_client::file_browser::Browser,
    pub(super) active: bool,
    path: Entity<InputState>,
    filter: Entity<InputState>,
    hidden: bool,
    places_visible: bool,
    /// The breadcrumb is replaced by the path field while typing a location.
    editing_path: bool,
    sort: list::Sort,
    selected: Option<String>,
    pub(super) list_focus: FocusHandle,
    home: Option<String>,
    project_path: Option<String>,
    projection: RefCell<Option<Projection>>,
    pub directory: String,
    pub(super) requested: String,
    pub canonical: Option<String>,
    reset_filter: bool,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

struct Projection {
    revision: u64,
    query: String,
    hidden: bool,
    sort: list::Sort,
    entries: Rc<Vec<Value>>,
}
impl Browser {
    /// The filter as matched; surrounding whitespace never narrows a folder.
    fn query(&self, cx: &App) -> String {
        self.filter.read(cx).value().trim().to_owned()
    }

    fn entries(&self, cx: &App) -> Rc<Vec<Value>> {
        let revision = self.reads.listing_data_revision();
        let query = self.query(cx);
        let mut cached = self.projection.borrow_mut();
        if cached.as_ref().is_none_or(|cache| {
            cache.revision != revision
                || cache.query != query
                || cache.hidden != self.hidden
                || cache.sort != self.sort
        }) {
            let mut entries = self.reads.listing().map_or_else(Vec::new, |listing| {
                visible_entries(listing, &query, self.hidden)
            });
            list::sort_entries(&mut entries, self.sort);
            *cached = Some(Projection {
                revision,
                query,
                hidden: self.hidden,
                sort: self.sort,
                entries: Rc::new(entries),
            });
        }
        cached.as_ref().unwrap().entries.clone()
    }
}

/// Selection and accepted-render identity; A → B → A is a new preview even
/// when its path and remote connection happen to be the same.
#[derive(Clone, PartialEq)]
pub(super) struct PreviewTarget {
    server: String,
    connection: Value,
    token: u64,
    path: String,
    render_revision: u64,
    pending: bool,
}

fn parent(path: &str) -> &str {
    let path = path.trim_end_matches('/');
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("/")
}

fn visible_entries(listing: &Value, query: &str, hidden: bool) -> Vec<Value> {
    let query = query.to_lowercase();
    let mut entries: Vec<_> = listing["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| {
            let name = text(entry, "name");
            (hidden || !name.starts_with('.')) && name.to_lowercase().contains(&query)
        })
        .cloned()
        .collect();
    entries.sort_by_cached_key(|entry| {
        let name = text(entry, "name");
        (entry["kind"] != "directory", name.to_lowercase(), name)
    });
    entries
}

impl Newport {
    fn file_browser_current(&self, server: &str, connection: &Value) -> bool {
        self.page == Page::Files
            && self.server == server
            && self.configuration_available()
            && self
                .file_browsers
                .get(server)
                .is_some_and(|browser| browser.active && &browser.connection == connection)
            && self.selected_server().is_some_and(|profile| {
                terminal_scope::connection(profile, &self.snapshot) == *connection
            })
    }
    pub(super) fn file_connection_changed(&mut self, id: &str, connection: Value) {
        if let Some(browser) = self.file_browsers.get_mut(id) {
            browser.reads.retire();
            browser.reads = newport_client::file_browser::Browser::new(
                backend::Backend::discover(),
                id.to_owned(),
                connection.clone(),
            );
            browser.connection = connection;
            browser.active = false;
            browser.canonical = None;
            browser.home = None;
        }
        if self.server == id {
            self.close_file_preview();
        }
    }

    pub(super) fn file_preview_target(&self) -> Option<PreviewTarget> {
        if self.page != Page::Files || !self.configuration_available() {
            return None;
        }
        let browser = self.file_browsers.get(&self.server)?;
        if !browser.active
            || browser.connection
                != terminal_scope::connection(self.selected_server()?, &self.snapshot)
        {
            return None;
        }
        let preview = browser.reads.preview()?;
        if self.file_preview_path.as_deref() != Some(preview.path.as_str()) {
            return None;
        }
        let prepared = self.resources.get("preview");
        Some(PreviewTarget {
            server: self.server.clone(),
            connection: browser.connection.clone(),
            token: preview.token,
            path: preview.path.clone(),
            render_revision: prepared.map_or(0, |row| row.revision),
            pending: preview.pending || prepared.is_some_and(|row| row.pending),
        })
    }

    pub(super) fn file_preview_target_current(&self, target: &PreviewTarget) -> bool {
        self.file_preview_target().as_ref() == Some(target)
    }

    pub(super) fn close_file_preview(&mut self) {
        if let Some(browser) = self.file_browsers.get_mut(&self.server) {
            browser.reads.close_preview();
        }
        self.resources.invalidate("preview");
        self.file_preview_path = None;
        self.file_preview_image = None;
        self.file_preview_pdf = None;
        self.file_preview_pdf_text = None;
        self.file_preview_viewport = file_preview::Viewport::default();
    }

    pub(super) fn poll_file_browsers(&mut self, _cx: &mut Context<Self>) -> bool {
        let available = self.configuration_available();
        let mut changed = false;
        let mut payload = None;
        for (server, browser) in &mut self.file_browsers {
            let visible =
                available && browser.active && self.page == Page::Files && self.server == *server;
            let update = browser.reads.poll();
            // Always drain owned reads/cancellation, but only the mounted
            // browser's presentation can invalidate the surrounding page.
            changed |= visible && update.changed;
            if update.listing_changed
                && let Some(path) = browser.reads.canonical_path()
            {
                browser.reset_filter |= browser.directory != path;
                browser.directory = path.to_owned();
                browser.canonical = Some(path.to_owned());
                if browser.reads.attempted_path() == Some(".") {
                    browser.home = Some(path.to_owned());
                }
            }
            if visible
                && let Some(preview) = browser.reads.preview()
                && !preview.pending
                && preview.data.is_some()
            {
                payload = browser.reads.take_preview_data(preview.token);
            }
        }
        if let Some(data) = payload {
            self.request(
                "preview",
                "native.prepare_preview",
                json!({"id":self.server,"data":data}),
            );
            changed = true;
        }
        changed
    }

    pub(super) fn file_preview_error(&self) -> Option<&str> {
        self.file_browsers
            .get(&self.server)
            .and_then(|browser| browser.reads.preview())
            .and_then(|preview| preview.error.as_deref())
            .or_else(|| {
                self.resources
                    .get("preview")
                    .and_then(|resource| resource.error.as_deref())
            })
    }

    pub(super) fn file_project_completed(
        &mut self,
        server: &str,
        expected: Option<Value>,
        result: Result<Value, String>,
    ) {
        let connection = self.snapshot["config"]["servers"]
            .as_array()
            .and_then(|servers| servers.iter().find(|row| row["id"] == server))
            .map(|server| terminal_scope::connection(server, &self.snapshot));
        if !self.configuration_available() || expected.is_none() || connection != expected {
            return;
        }
        if result.is_ok() {
            self.project_bookmarks_changed(server);
        }
        match result {
            Err(error) => self.notification = Some(Notification::error(error)),
            Ok(project) if self.page == Page::Files && self.server == server => {
                self.reset();
                self.page = Page::Projects;
                self.workspace_page = Page::Projects;
                self.selection = Some(project.clone());
                self.git_request_open(project["path"].clone());
            }
            Ok(_) => self.notification = Some(Notification::success("Project saved.")),
        }
    }
    pub(super) fn file_action_target(&self, path: &str) -> Option<FileTarget> {
        if self.page != Page::Files
            || !self.configuration_available()
            || self
                .file_browsers
                .get(&self.server)
                .is_none_or(|browser| !browser.active || browser.reads.listing_pending())
        {
            return None;
        }
        let server = self.selected_server()?;
        let connection = terminal_scope::connection(server, &self.snapshot);
        if self.file_browsers.get(&self.server)?.connection != connection {
            return None;
        }
        FileTarget::from_listing(
            self.server.clone(),
            connection,
            self.file_browsers.get(&self.server)?.reads.listing()?,
            path,
        )
    }
    pub(super) fn file_target_current(&self, target: &FileTarget) -> bool {
        self.file_action_target(&target.path).as_ref() == Some(target)
    }
    fn file_open_project(&mut self, target: &FileTarget, cx: &mut Context<Self>) {
        if !self.file_target_current(target)
            || !matches!(target.kind.as_deref(), None | Some("directory"))
            || self
                .resources
                .get("file-project")
                .is_some_and(|resource| resource.pending)
        {
            return;
        }
        let path = target.path.clone();
        if let Some(browser) = self.file_browsers.get_mut(&target.server) {
            browser.project_path = Some(path.clone());
        }
        self.file_project_connection = Some(target.connection.clone());
        use base64::Engine;
        let name = path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("Repository");
        self.request("file-project", "native.add_project", json!({"serverId":self.server,"name":name,
            "path":{"display":path,"bytesB64":base64::engine::general_purpose::STANDARD.encode(path.as_bytes())}}));
        cx.notify();
    }
    pub(super) fn sync_file_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prepare_file_browser(window, cx);
        if self.page != Page::Files
            || !self.configuration_available()
            || !self.file_browsers.contains_key(&self.server)
        {
            return;
        }
        let refresh = !self.file_browsers[&self.server].active || self.files_refresh_requested;
        if refresh {
            self.close_file_preview();
            let browser = self.file_browsers.get_mut(&self.server).unwrap();
            browser.active = true;
            browser.requested = browser.directory.clone();
            browser.reads.navigate(browser.directory.clone());
            self.files_refresh_requested = false;
            // Saved projects become shortcuts in the places panel.
            let server = self.server.clone();
            self.request_refresh(
                "file-places",
                "git_projects_list",
                json!({"serverId":server}),
            );
        }
    }

    /// Create/update controls without reading. A project handover supplies its
    /// destination before activation, avoiding a throwaway read of the old folder.
    pub(super) fn prepare_file_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Files || self.server.is_empty() || !self.configuration_available() {
            return;
        }
        if self
            .file_browsers
            .get(&self.server)
            .is_none_or(|browser| browser.controls_connection != browser.connection)
        {
            let Some(profile) = self.selected_server() else {
                return;
            };
            let connection = terminal_scope::connection(profile, &self.snapshot);
            let (directory, path_text, query, hidden, places_visible, sort) = self
                .file_browsers
                .get(&self.server)
                .map(|browser| {
                    (
                        browser.directory.clone(),
                        browser.path.read(cx).value().to_string(),
                        browser.filter.read(cx).value().to_string(),
                        browser.hidden,
                        browser.places_visible,
                        browser.sort,
                    )
                })
                .unwrap_or_else(|| {
                    (
                        ".".into(),
                        ".".into(),
                        String::new(),
                        false,
                        true,
                        list::Sort::default(),
                    )
                });
            let path = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Go to folder")
                    .default_value(path_text)
            });
            let filter = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Find a file in this folder")
                    .default_value(query)
            });
            let server = self.server.clone();
            let input_connection = connection.clone();
            let filter_server = server.clone();
            let filter_connection = connection.clone();
            let subscriptions = vec![
                cx.subscribe(&filter, move |this, _, event, cx| {
                    if !matches!(event, gpui_kit::component::input::InputEvent::Change)
                        || !this.file_browser_current(&filter_server, &filter_connection)
                    {
                        return;
                    }
                    if let Some(browser) = this.file_browsers.get_mut(&filter_server) {
                        browser.scroll.scroll_to_item(0, ScrollStrategy::Top);
                        browser.selected = None;
                    }
                    cx.notify();
                }),
                cx.subscribe_in(
                    &path,
                    window,
                    move |this,
                          input,
                          event: &gpui_kit::component::input::InputEvent,
                          window,
                          cx| {
                        if !this.file_browser_current(&server, &input_connection) {
                            return;
                        }
                        match event {
                            gpui_kit::component::input::InputEvent::PressEnter { .. } => {
                                this.files_navigate(input.read(cx).value().to_string(), window, cx);
                            }
                            gpui_kit::component::input::InputEvent::Blur => {
                                this.files_stop_editing_path(window, cx);
                            }
                            _ => {}
                        }
                    },
                ),
            ];
            self.file_browsers.insert(
                self.server.clone(),
                Browser {
                    reads: newport_client::file_browser::Browser::new(
                        backend::Backend::discover(),
                        self.server.clone(),
                        connection.clone(),
                    ),
                    controls_connection: connection.clone(),
                    connection,
                    active: false,
                    path,
                    filter,
                    hidden,
                    places_visible,
                    editing_path: false,
                    sort,
                    selected: None,
                    list_focus: cx.focus_handle(),
                    home: None,
                    project_path: None,
                    projection: RefCell::new(None),
                    directory,
                    requested: String::new(),
                    canonical: None,
                    reset_filter: false,
                    scroll: UniformListScrollHandle::new(),
                    _subscriptions: subscriptions,
                },
            );
        }
        let browser = self.file_browsers.get_mut(&self.server).unwrap();
        if browser.reset_filter {
            browser.reset_filter = false;
            browser.selected = None;
            browser.scroll.scroll_to_item(0, ScrollStrategy::Top);
            browser
                .filter
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        if let Some(path) = browser.canonical.take() {
            // Do not replace a newer address the user has started typing.
            if browser.path.read(cx).value().as_ref() == browser.requested {
                browser
                    .path
                    .update(cx, |input, cx| input.set_value(path, window, cx));
            }
        }
    }
    pub(super) fn files_navigate(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if path.trim().is_empty() {
            return;
        }
        if self.page != Page::Files || !self.configuration_available() {
            return;
        }
        self.close_file_preview();
        let Some(browser) = self.file_browsers.get_mut(&self.server) else {
            return;
        };
        browser.active = true;
        browser.editing_path = false;
        browser.selected = None;
        browser.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.files_refresh_requested = false;
        browser.requested = path.clone();
        browser.canonical = None;
        browser
            .path
            .update(cx, |input, cx| input.set_value(path.clone(), window, cx));
        browser.reads.navigate(path);
        cx.notify();
    }
    fn files_preview(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self
            .file_browsers
            .get(&self.server)
            .and_then(|browser| browser.reads.listing())
            .and_then(|listing| listing["entries"].as_array())
            .and_then(|entries| entries.iter().find(|entry| entry["path"] == path))
            .cloned()
        else {
            return;
        };
        if self.file_preview_path.as_deref() == Some(path.as_str()) {
            return;
        }
        self.close_file_preview();
        let Some(browser) = self.file_browsers.get_mut(&self.server) else {
            return;
        };
        if browser.reads.open_preview(path.clone()).is_none() {
            return;
        }
        self.file_preview_path = Some(path);
        self.prepare_file_preview(&entry, window, cx);
        cx.notify();
    }
    /// Select an entry without opening it. A preview that is already showing
    /// follows the selection; selecting a folder closes it.
    fn files_select(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(browser) = self.file_browsers.get_mut(&self.server) else {
            return;
        };
        let entries = browser.entries(cx);
        let Some(index) = entries.iter().position(|entry| entry["path"] == path) else {
            return;
        };
        browser.selected = Some(path.clone());
        browser
            .scroll
            .scroll_to_item(index, ScrollStrategy::Nearest);
        if self.file_preview_path.is_some() {
            if entries[index]["kind"] == "directory" {
                self.close_file_preview();
            } else {
                self.files_preview(path, window, cx);
            }
        }
        cx.notify();
    }
    /// Click or Enter: a folder opens in place, a file opens in the preview.
    pub(super) fn files_activate(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(browser) = self.file_browsers.get_mut(&self.server) else {
            return;
        };
        browser.list_focus.focus(window, cx);
        let directory = browser
            .entries(cx)
            .iter()
            .find(|entry| entry["path"] == path)
            .map(|entry| entry["kind"] == "directory");
        match directory {
            Some(true) => self.files_navigate(path, window, cx),
            Some(false) => {
                browser.selected = Some(path.clone());
                self.files_preview(path, window, cx);
                // A preview that replaces the list takes keyboard focus with it.
                if !self.files_preview_beside()
                    && let Some(focus) = &self.file_preview_viewport.content_focus
                {
                    focus.focus(window, cx);
                }
                cx.notify();
            }
            None => {}
        }
    }
    pub(super) fn files_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(browser) = self.file_browsers.get(&self.server) else {
            return;
        };
        let scope = (self.server.clone(), browser.connection.clone());
        if !self.file_browser_current(&scope.0, &scope.1) {
            return;
        }
        let keystroke = &event.keystroke;
        let entries = browser.entries(cx);
        let current = browser
            .selected
            .as_ref()
            .and_then(|path| entries.iter().position(|entry| entry["path"] == *path));
        let path_at = |index: usize| entries.get(index).map(|entry| text(entry, "path"));
        let parent_key =
            keystroke.key == "backspace" || (keystroke.key == "up" && keystroke.modifiers.platform);
        if parent_key {
            let path = browser
                .reads
                .canonical_path()
                .unwrap_or_default()
                .to_owned();
            if !path.is_empty() && path != "/" {
                self.files_navigate(parent(&path).to_owned(), window, cx);
            }
        } else if keystroke.modifiers.platform
            || keystroke.modifiers.control
            || keystroke.modifiers.alt
        {
            return;
        } else {
            let last = entries.len().checked_sub(1);
            let target = match keystroke.key.as_str() {
                "down" => current.map_or(Some(0), |index| Some((index + 1).min(last.unwrap_or(0)))),
                "up" => current.map_or(last, |index| Some(index.saturating_sub(1))),
                "home" => Some(0),
                "end" => last,
                "enter" => {
                    if let Some(path) = current.and_then(path_at) {
                        self.files_activate(path, window, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                "escape" => {
                    if self.file_preview_path.is_some() {
                        self.close_file_preview();
                    } else if let Some(browser) = self.file_browsers.get_mut(&self.server) {
                        browser.selected = None;
                    }
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                _ => return,
            };
            if let Some(path) = target.filter(|_| !entries.is_empty()).and_then(path_at) {
                self.files_select(path, window, cx);
            }
        }
        cx.stop_propagation();
        window.prevent_default();
    }
    fn files_start_editing_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(browser) = self.file_browsers.get_mut(&self.server) {
            browser.editing_path = true;
            let input = browser.path.clone();
            input.update(cx, |input, cx| input.select_all(window, cx));
            crate::controls::focus_editor(&input, window, cx);
            cx.notify();
        }
    }
    fn files_stop_editing_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(browser) = self.file_browsers.get_mut(&self.server)
            && browser.editing_path
        {
            browser.editing_path = false;
            let current = browser
                .reads
                .canonical_path()
                .unwrap_or_default()
                .to_owned();
            if !current.is_empty() {
                browser
                    .path
                    .update(cx, |input, cx| input.set_value(current, window, cx));
            }
            cx.notify();
        }
    }
    fn files_places_shown(&self) -> bool {
        self.file_browsers
            .get(&self.server)
            .is_some_and(|browser| browser.places_visible)
            && self.content_width >= PLACES_MIN_CONTENT
    }
    fn files_preview_beside(&self) -> bool {
        self.file_preview_path.is_some() && self.content_width >= PREVIEW_SIDE_MIN_CONTENT
    }
    /// Width left for the list, which decides whether Modified fits.
    pub(super) fn files_list_width(&self) -> f32 {
        let mut width = self.content_width;
        if self.files_places_shown() {
            width -= places_width(self.content_width);
        }
        if self.files_preview_beside() {
            width -= preview_width(self.content_width);
        }
        width
    }

    fn files_breadcrumb(
        &self,
        browser: &Browser,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (server, connection) = (self.server.clone(), browser.connection.clone());
        let home = browser
            .home
            .as_deref()
            .filter(|home| path == *home || path.starts_with(&format!("{home}/")));
        // Each crumb is (label, destination). Home collapses to its icon.
        let mut crumbs: Vec<(String, String)> = Vec::new();
        let rest = match home {
            Some(home) => {
                crumbs.push(("~".into(), home.to_owned()));
                &path[home.len()..]
            }
            None => {
                crumbs.push(("/".into(), "/".into()));
                path
            }
        };
        let mut so_far = crumbs[0].1.trim_end_matches('/').to_owned();
        for segment in rest.split('/').filter(|segment| !segment.is_empty()) {
            so_far = format!("{so_far}/{segment}");
            crumbs.push((segment.to_owned(), so_far.clone()));
        }
        // Long paths keep their root and the last three folders.
        let hidden = crumbs.len().saturating_sub(4);
        let last = crumbs.len() - 1;
        let mut row = div()
            .id("files-breadcrumb")
            .debug_selector(|| "files-breadcrumb".into())
            .role(Role::Navigation)
            .aria_label(format!("Location: {path}"))
            .flex_1()
            .min_w_0()
            .h(px(crate::controls::CONTROL_HEIGHT))
            .flex()
            .items_center()
            .overflow_hidden()
            .cursor_text();
        for (index, (label, destination)) in crumbs.into_iter().enumerate() {
            if index > 0 && index <= hidden {
                if index == 1 {
                    row = row.child(crumb_separator(cx)).child(
                        div()
                            .px(px(4.))
                            .text_color(cx.theme().muted_foreground)
                            .child("…"),
                    );
                }
                continue;
            }
            if index > 0 {
                row = row.child(crumb_separator(cx));
            }
            let current = index == last;
            let (server, connection) = (server.clone(), connection.clone());
            let button = crate::controls::compact_button(("files-crumb", index))
                .ghost()
                .flex_shrink_0()
                .max_w(px(220.))
                .accessibility_label(if label == "~" {
                    "Home folder".to_owned()
                } else if label == "/" {
                    "Server root".to_owned()
                } else {
                    label.clone()
                })
                .when(label == "~", |button| {
                    button.icon(IconName::House).child("Home")
                })
                .when(label == "/", |button| {
                    button.icon(IconName::HardDrive).child("Server root")
                })
                .when(label != "~" && label != "/", |button| {
                    button.child(div().truncate().child(label.clone()))
                })
                .text_color(if current {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .when(current, |button| button.font_weight(FontWeight::MEDIUM))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.file_browser_current(&server, &connection) {
                        this.files_navigate(destination.clone(), window, cx);
                    }
                }));
            row = row.child(button);
        }
        // Empty space after the last folder turns the breadcrumb into a field.
        row.child(
            div()
                .id("files-breadcrumb-edit")
                .flex_1()
                .h_full()
                .min_w(px(24.))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.file_browser_current(&server, &connection) {
                        this.files_start_editing_path(window, cx);
                    }
                })),
        )
    }

    fn files_toolbar(&self, browser: &Browser, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let pending = browser.reads.listing_pending();
        let scope = (self.server.clone(), browser.connection.clone());
        let directory_target = self.file_action_target(path);
        let up = parent(path).to_owned();
        let project_pending = self
            .resources
            .get("file-project")
            .is_some_and(|row| row.pending);
        let toggle = |id: &'static str, on: bool, icon: IconName, label: &'static str| {
            crate::controls::icon_button(id)
                .ghost()
                .icon(icon)
                .tooltip(label)
                .accessibility_label(label)
                .selected(on)
                .toggled(on)
        };
        crate::controls::page_toolbar(cx)
            .id("files-toolbar")
            .debug_selector(|| "files-toolbar".into())
            // Icon buttons sit closer together than labelled controls.
            .gap(px(4.))
            .child(
                toggle(
                    "files-places",
                    self.files_places_shown(),
                    IconName::PanelLeft,
                    "Places",
                )
                .disabled(self.content_width < PLACES_MIN_CONTENT)
                .on_click({
                    let (server, connection) = scope.clone();
                    cx.listener(move |this, _, _, cx| {
                        if this.file_browser_current(&server, &connection)
                            && let Some(browser) = this.file_browsers.get_mut(&server)
                        {
                            browser.places_visible = !browser.places_visible;
                            cx.notify();
                        }
                    })
                }),
            )
            .child(
                crate::controls::icon_button("files-parent")
                    .ghost()
                    .icon(IconName::ArrowUp)
                    .tooltip("Enclosing folder")
                    .accessibility_label("Parent folder")
                    .disabled(pending || path.is_empty() || path == "/")
                    .on_click({
                        let target = directory_target.clone();
                        cx.listener(move |this, _, window, cx| {
                            if target
                                .as_ref()
                                .is_some_and(|target| this.file_target_current(target))
                            {
                                this.files_navigate(up.clone(), window, cx);
                            }
                        })
                    }),
            )
            .child(if browser.editing_path || path.is_empty() {
                div()
                    .flex_1()
                    .min_w_0()
                    .child(crate::controls::input(&browser.path).aria_label("Go to folder"))
                    .into_any_element()
            } else {
                self.files_breadcrumb(browser, path, cx).into_any_element()
            })
            .child(
                div()
                    .w(px(220_f32.min(self.content_width * 0.28)))
                    .min_w(px(120.))
                    .child(
                        crate::controls::search_input(&browser.filter, cx)
                            .aria_label("Filter files"),
                    ),
            )
            .child(
                toggle(
                    "files-hidden",
                    browser.hidden,
                    if browser.hidden {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    },
                    "Hidden files",
                )
                .on_click({
                    let (server, connection) = scope.clone();
                    cx.listener(move |this, _, _, cx| {
                        if this.file_browser_current(&server, &connection)
                            && let Some(browser) = this.file_browsers.get_mut(&server)
                        {
                            browser.hidden = !browser.hidden;
                            browser.selected = None;
                            cx.notify();
                        }
                    })
                }),
            )
            .child(
                crate::controls::icon_button("files-refresh")
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .tooltip("Refresh")
                    .accessibility_label("Refresh folder")
                    .disabled(pending || path.is_empty())
                    .on_click({
                        let target = directory_target.clone();
                        let path = path.to_owned();
                        cx.listener(move |this, _, window, cx| {
                            if target
                                .as_ref()
                                .is_some_and(|target| this.file_target_current(target))
                            {
                                this.files_navigate(path.clone(), window, cx);
                            }
                        })
                    }),
            )
            .child(
                crate::controls::icon_button("files-upload")
                    .ghost()
                    .icon(IconName::Upload)
                    .tooltip("Upload to this folder")
                    .accessibility_label("Upload")
                    .disabled(directory_target.is_none() || self.file_transfer_busy())
                    .on_click({
                        let target = directory_target.clone();
                        cx.listener(move |this, _, _, cx| {
                            if let Some(target) = &target {
                                this.start_file_transfer(target, Direction::Upload, cx);
                            }
                        })
                    }),
            )
            .child({
                let view = cx.weak_entity();
                let target = directory_target.clone();
                let path = path.to_owned();
                let (server, connection) = scope.clone();
                crate::controls::icon_button("files-more")
                    .ghost()
                    .icon(IconName::Ellipsis)
                    .accessibility_label("More folder actions")
                    .tooltip("More folder actions")
                    .loading(
                        project_pending && browser.project_path.as_deref() == Some(path.as_str()),
                    )
                    .dropdown_menu(move |menu, _, _| {
                        let project = view.clone();
                        let project_target = target.clone();
                        let copy = path.clone();
                        let edit = view.clone();
                        let (server, connection) = (server.clone(), connection.clone());
                        menu.item(
                            PopupMenuItem::new("Open as project")
                                .icon(IconName::FolderGit2)
                                .disabled(project_target.is_none() || project_pending)
                                .on_click(move |_, _, cx| {
                                    let _ = project.update(cx, |this, cx| {
                                        if let Some(target) = &project_target {
                                            this.file_open_project(target, cx);
                                        }
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Copy path")
                                .icon(IconName::Copy)
                                .disabled(copy.is_empty())
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Go to folder")
                                .icon(IconName::CornerDownRight)
                                .on_click(move |_, window, cx| {
                                    let _ = edit.update(cx, |this, cx| {
                                        if this.file_browser_current(&server, &connection) {
                                            this.files_start_editing_path(window, cx);
                                        }
                                    });
                                }),
                        )
                    })
            })
            .into_any_element()
    }

    fn files_places(&self, browser: &Browser, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let scope = (self.server.clone(), browser.connection.clone());
        let home = browser.home.clone().unwrap_or_else(|| ".".into());
        let projects: Vec<(String, String)> = self
            .resources
            .get("file-places")
            .and_then(|row| row.value.as_array())
            .into_iter()
            .flatten()
            .filter(|project| project["serverId"] == self.server.as_str())
            .filter_map(|project| {
                let path = project["path"]["display"].as_str()?.to_owned();
                let name = project["name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path).to_owned());
                Some((name, path))
            })
            .collect();
        let place = |id: SharedString, icon: IconName, label: String, destination: String| {
            let current = destination == path
                || (destination == "." && browser.home.as_deref() == Some(path));
            let (server, connection) = scope.clone();
            // Rows match the file list: full width, the shared selected fill
            // and row hover, no inset highlight.
            div()
                .id(id)
                .role(Role::Link)
                .aria_label(label.clone())
                .aria_selected(current)
                .test_support()
                .h(px(list::ROW_HEIGHT))
                .flex_shrink_0()
                .px(px(crate::controls::PAGE_PADDING))
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .text_color(cx.theme().foreground)
                .when(current, |row| {
                    row.bg(theme::selected_fill(cx.theme().is_dark()))
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!current, |row| {
                    row.hover(|row| row.bg(theme::row_hover(cx)))
                })
                .child(
                    Icon::new(icon)
                        .size(px(crate::controls::ICON_SIZE))
                        .text_color(cx.theme().muted_foreground),
                )
                .child(div().min_w_0().truncate().child(label))
                .hover_hint(destination.clone())
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.file_browser_current(&server, &connection) {
                        this.files_navigate(destination.clone(), window, cx);
                    }
                }))
        };
        div()
            .id("files-places")
            .debug_selector(|| "files-places".into())
            .role(Role::Navigation)
            .aria_label("Places")
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(6.))
            .flex()
            .flex_col()
            .child(place(
                "files-place-home".into(),
                IconName::House,
                "Home".into(),
                home,
            ))
            .child(place(
                "files-place-root".into(),
                IconName::HardDrive,
                "Server root".into(),
                "/".into(),
            ))
            .when(!projects.is_empty(), |panel| {
                panel
                    .child(
                        div()
                            .mt(px(10.))
                            .mb(px(2.))
                            .px(px(crate::controls::PAGE_PADDING))
                            .text_color(cx.theme().muted_foreground)
                            .child("Projects"),
                    )
                    .children(
                        projects
                            .into_iter()
                            .enumerate()
                            .map(|(index, (name, path))| {
                                place(
                                    SharedString::from(format!("files-place-project-{index}")),
                                    IconName::FolderGit2,
                                    name,
                                    path,
                                )
                            }),
                    )
            })
            .into_any_element()
    }

    /// Places, list and preview share one resizable row. Places and preview
    /// keep their dragged widths while the list absorbs window resizes; each
    /// combination of visible panes remembers its own widths.
    fn files_body(
        &self,
        browser: &Browser,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let places = self.files_places_shown();
        let preview_beside = self.files_preview_beside();
        let preview_only = self.file_preview_path.is_some() && !preview_beside;
        let preview_pane = |window: &mut Window, cx: &mut Context<Self>| {
            div()
                .id("files-preview-pane")
                .debug_selector(|| "files-preview-pane".into())
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .min_w_0()
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_file_preview();
                        if let Some(browser) = this.file_browsers.get(&this.server) {
                            browser.list_focus.focus(window, cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .child(self.file_preview_side(window, cx))
        };
        if preview_only {
            return div()
                .id("files-layout")
                .debug_selector(|| "files-layout".into())
                .flex()
                .flex_1()
                .min_h_0()
                .child(preview_pane(window, cx))
                .into_any_element();
        }
        let key = SharedString::from(format!(
            "files-split-{}-{}",
            u8::from(places),
            u8::from(preview_beside)
        ));
        let layout = crate::controls::split_state(&key, window, cx);
        let list_width = layout
            .read(cx)
            .sizes()
            .get(usize::from(places))
            .map(|width| f32::from(*width))
            .unwrap_or_else(|| self.files_list_width());
        div()
            .id("files-layout")
            .debug_selector(|| "files-layout".into())
            .flex()
            .flex_1()
            .min_h_0()
            .child(
                h_resizable(key)
                    .with_state(&layout)
                    .when(places, |group| {
                        group.child(
                            resizable_panel()
                                .size(px(places_width(self.content_width)))
                                .size_range(px(140.)..px(360.))
                                .flex_none()
                                .child(self.files_places(browser, path, cx)),
                        )
                    })
                    .child(
                        resizable_panel()
                            .size_range(px(320.)..Pixels::MAX)
                            .child(self.file_listing(list_width, window, cx)),
                    )
                    .when(preview_beside, |group| {
                        group.child(
                            resizable_panel()
                                .size(px(preview_width(self.content_width)))
                                .size_range(px(280.)..px(960.))
                                .flex_none()
                                .child(preview_pane(window, cx)),
                        )
                    }),
            )
            .into_any_element()
    }

    pub(super) fn files(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(browser) = self.file_browsers.get(&self.server) else {
            return div().into_any_element();
        };
        let path = browser
            .reads
            .canonical_path()
            .unwrap_or_default()
            .to_owned();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.files_toolbar(browser, &path, cx))
            .child(self.status("file-project", cx))
            .when_some(browser.reads.listing_error(), |view, error| {
                let attempted = browser.reads.attempted_path().unwrap_or(".").to_owned();
                let message = format!("Couldn’t open {attempted}: {error}");
                let scope = browser.connection.clone();
                let server = self.server.clone();
                view.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px(px(12.))
                        .py(px(8.))
                        .flex_shrink_0()
                        .child(
                            crate::controls::alert("file-listing-error", message, cx)
                                .flex_1()
                                .min_w_0(),
                        )
                        .child(
                            crate::controls::button("files-retry")
                                .label("Try again")
                                .accessibility_label(format!("Try opening {attempted} again"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.file_browser_current(&server, &scope)
                                        && this.file_browsers[&server].reads.attempted_path()
                                            == Some(attempted.as_str())
                                        && !this.file_browsers[&server].reads.listing_pending()
                                    {
                                        this.files_navigate(attempted.clone(), window, cx);
                                    }
                                })),
                        ),
                )
            })
            .child(self.files_body(browser, &path, window, cx))
            .when(self.file_preview_path.is_none(), |view| {
                view.child(self.transfer_status(cx))
            })
            .child(self.file_listing_footer(cx))
            .into_any_element()
    }
}

fn crumb_separator(cx: &App) -> Icon {
    Icon::new(IconName::ChevronRight)
        .size(px(12.))
        .text_color(cx.theme().muted_foreground)
}

fn file_menu(
    menu: PopupMenu,
    view: WeakEntity<Newport>,
    target: FileTarget,
    cx: &App,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let disabled =
        entity.read(cx).file_transfer_busy() || !entity.read(cx).file_target_current(&target);
    let directory = target.kind.as_deref() == Some("directory");
    let open_view = view.clone();
    let open_target = target.clone();
    let copy_target = target.clone();
    let copy_view = view.clone();
    let menu = menu
        .item(
            PopupMenuItem::new(if directory { "Open folder" } else { "Preview" }).on_click(
                move |_, window, cx| {
                    let _ = open_view.update(cx, |this, cx| {
                        if !this.file_target_current(&open_target) {
                            return;
                        }
                        if directory {
                            this.files_navigate(open_target.path.clone(), window, cx);
                        } else {
                            // The context-menu item is about to unmount. Restore
                            // into the Files shell rather than that transient row.
                            this.focus.focus(window, cx);
                            this.files_preview(open_target.path.clone(), window, cx);
                        }
                    });
                },
            ),
        )
        .item(PopupMenuItem::new("Copy path").on_click(move |_, _, cx| {
            if copy_view
                .upgrade()
                .is_some_and(|view| view.read(cx).file_target_current(&copy_target))
            {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_target.path.clone()));
            }
        }));
    if directory {
        let pending = entity
            .read(cx)
            .resources
            .get("file-project")
            .is_some_and(|row| row.pending);
        return menu.separator().item(
            PopupMenuItem::new("Open as project")
                .disabled(pending)
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.file_open_project(&target, cx);
                    });
                }),
        );
    }
    if !target.supports_transfer(Direction::Download) {
        return menu;
    }
    menu.separator()
        .item(
            PopupMenuItem::new("Download…")
                .disabled(disabled)
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.start_file_transfer(&target, Direction::Download, cx);
                    });
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::list::{Sort, SortColumn, folder_summary, sort_entries};
    use super::{Direction, FileTarget, PreviewTarget, parent, visible_entries};
    use crate::text;
    use serde_json::json;

    #[test]
    fn preview_controls_reject_a_replaced_render_or_selection() {
        let opening = PreviewTarget {
            server: "a".into(),
            connection: json!(["connection-a"]),
            token: 1,
            path: "/work/readme".into(),
            render_revision: 0,
            pending: true,
        };
        let mut loaded = opening.clone();
        loaded.pending = false;
        loaded.render_revision = 4;
        assert!(opening != loaded, "controls reject a replaced render");
        for change in [
            |t: &mut PreviewTarget| t.token = 2,
            |t: &mut PreviewTarget| t.server = "b".into(),
            |t: &mut PreviewTarget| t.connection = json!(["connection-b"]),
            |t: &mut PreviewTarget| t.path = "/work/other".into(),
        ] {
            let mut replacement = loaded.clone();
            change(&mut replacement);
            assert!(loaded != replacement);
        }
    }

    #[test]
    fn sorting_keeps_folders_first_and_flips_the_active_column() {
        let mut entries = vec![
            json!({"name":"b.log","kind":"file","size":10,"modified":3}),
            json!({"name":"src","kind":"directory","modified":1}),
            json!({"name":"A.txt","kind":"file","size":300,"modified":1}),
            json!({"name":".env","kind":"file","size":2,"modified":2}),
            json!({"name":"docs","kind":"directory","modified":9}),
        ];
        let names = |entries: &[serde_json::Value]| {
            entries.iter().map(|e| text(e, "name")).collect::<Vec<_>>()
        };
        sort_entries(&mut entries, Sort::default());
        assert_eq!(names(&entries), ["docs", "src", ".env", "A.txt", "b.log"]);
        let newest = Sort::default().choose(SortColumn::Modified);
        assert!(newest.descending, "a new column starts newest first");
        sort_entries(&mut entries, newest);
        assert_eq!(names(&entries), ["docs", "src", "b.log", ".env", "A.txt"]);
        let largest = newest.choose(SortColumn::Size);
        sort_entries(&mut entries, largest);
        assert_eq!(names(&entries), ["src", "docs", "A.txt", "b.log", ".env"]);
        let smallest = largest.choose(SortColumn::Size);
        assert!(!smallest.descending, "choosing the active column flips it");
        sort_entries(&mut entries, smallest);
        assert_eq!(names(&entries)[2..], [".env", "b.log", "A.txt"]);
    }

    #[test]
    fn size_order_ignores_folder_attribute_sizes() {
        let mut entries = vec![
            json!({"name":"zeta","kind":"directory","size":4096}),
            json!({"name":"big.bin","kind":"file","size":900}),
            json!({"name":"Alpha","kind":"directory","size":12288}),
            json!({"name":"beta","kind":"directory","size":8192}),
            json!({"name":"tiny","kind":"file","size":1}),
        ];
        let names = |entries: &[serde_json::Value]| {
            entries.iter().map(|e| text(e, "name")).collect::<Vec<_>>()
        };
        let smallest = Sort {
            column: SortColumn::Size,
            descending: false,
        };
        sort_entries(&mut entries, smallest);
        assert_eq!(
            names(&entries),
            ["Alpha", "beta", "zeta", "tiny", "big.bin"]
        );
        sort_entries(&mut entries, smallest.choose(SortColumn::Size));
        assert_eq!(
            names(&entries),
            ["zeta", "beta", "Alpha", "big.bin", "tiny"]
        );
    }

    #[test]
    fn summary_counts_folders_files_hidden_and_filtered_share() {
        let listing = json!({"entries":[
            {"name":"src","kind":"directory"},
            {"name":"a.txt","kind":"file"},
            {"name":"b.txt","kind":"file"},
            {"name":".git","kind":"directory"}
        ]});
        let shown = visible_entries(&listing, "", false);
        assert_eq!(
            folder_summary(&listing, &shown, "", false),
            "3 items · 1 folder, 2 files · 1 hidden"
        );
        let all = visible_entries(&listing, "", true);
        assert_eq!(
            folder_summary(&listing, &all, "", true),
            "4 items · 2 folders, 2 files"
        );
        let matches = visible_entries(&listing, "a", false);
        assert_eq!(
            folder_summary(&listing, &matches, "a", false),
            "1 of 3 items match"
        );
        let empty = json!({"entries":[]});
        assert_eq!(folder_summary(&empty, &[], "", false), "0 items");
    }

    #[test]
    #[ignore = "manual Files action-target preparation benchmark; excludes GPUI and I/O"]
    fn benchmark_visible_file_targets() {
        for count in [40, 400, 5000] {
            let listing = json!({"path":"/work", "entries": (0..count).map(|i| json!({
                "name":format!("file-{i:05}"),"path":format!("/work/{i}"),"kind":"file"
            })).collect::<Vec<_>>()});
            let visible = &listing["entries"].as_array().unwrap()[count - 40..];
            let start = std::time::Instant::now();
            for _ in 0..100 {
                let directory = FileTarget::from_listing(
                    "fixture".into(),
                    json!(["connection"]),
                    &listing,
                    "/work",
                )
                .unwrap();
                for entry in visible {
                    std::hint::black_box(directory.for_entry(entry));
                }
            }
            eprintln!(
                "Files {count} entries / 40 visible targets: {:.3} ms/preparation",
                start.elapsed().as_secs_f64() * 1000. / 100.
            );
        }
    }

    #[test]
    fn projected_row_targets_match_validated_listing_targets() {
        let listing = json!({"path":"/work", "entries":[
            {"path":"/work/folder", "kind":"directory"},
            {"path":"/work/file", "kind":"file"},
            {"path":"/work/link", "kind":"symlink"},
            {"path":"/work/socket", "kind":"other"},
            {"path":"/work/missing-kind"},
            {"kind":"file"}
        ]});
        let connection = json!(["server-a", "host-a", 1]);
        let directory =
            FileTarget::from_listing("a".into(), connection.clone(), &listing, "/work").unwrap();
        for entry in listing["entries"].as_array().unwrap() {
            let expected = entry["path"].as_str().and_then(|path| {
                FileTarget::from_listing("a".into(), connection.clone(), &listing, path)
            });
            assert_eq!(directory.for_entry(entry), expected);
        }
        let file = directory.for_entry(&listing["entries"][1]).unwrap();
        assert!(file.for_entry(&listing["entries"][0]).is_none());
        let mut changed = listing.clone();
        changed["entries"][1]["kind"] = json!("directory");
        assert_ne!(
            Some(&file),
            FileTarget::from_listing("a".into(), connection, &changed, &file.path).as_ref()
        );
    }

    #[test]
    fn file_controls_remain_bound_to_connection_directory_and_entry_type() {
        let listing = json!({"path":"/work", "entries":[
            {"path":"/work/app", "kind":"directory"},
            {"path":"/work/log.txt", "kind":"file"}
        ]});
        let scope = json!(["server-a", "host-a", 1]);
        let capture = |server: &str, connection, data: &serde_json::Value| {
            FileTarget::from_listing(server.into(), connection, data, "/work/log.txt")
        };
        let original = capture("a", scope.clone(), &listing).unwrap();
        assert_eq!(
            capture("a", scope.clone(), &listing).as_ref(),
            Some(&original)
        );
        assert_ne!(
            capture("b", scope.clone(), &listing).as_ref(),
            Some(&original)
        );
        assert_ne!(
            capture("a", json!(["server-a", "host-b", 2]), &listing).as_ref(),
            Some(&original)
        );
        let mut replacement = listing.clone();
        replacement["entries"][1]["kind"] = json!("directory");
        assert_ne!(
            capture("a", scope.clone(), &replacement).as_ref(),
            Some(&original)
        );
        replacement["entries"] = json!([]);
        assert!(capture("a", scope.clone(), &replacement).is_none());
        assert!(capture("a", scope, &json!({"path":"/elsewhere", "entries":[]})).is_none());
    }

    #[test]
    fn transfer_targets_follow_listing_capabilities() {
        let listing = json!({"path":"/work", "entries":[
            {"path":"/work/app", "kind":"directory"},
            {"path":"/work/file", "kind":"file"},
            {"path":"/work/link", "kind":"symlink"},
            {"path":"/work/socket", "kind":"other"}
        ]});
        for (path, upload, download) in [
            ("/work", true, false),
            ("/work/app", false, false),
            ("/work/file", false, true),
            ("/work/link", false, true),
            ("/work/socket", false, false),
        ] {
            let target =
                FileTarget::from_listing("a".into(), json!(["scope"]), &listing, path).unwrap();
            assert_eq!(
                target.supports_transfer(Direction::Upload),
                upload,
                "{path}"
            );
            assert_eq!(
                target.supports_transfer(Direction::Download),
                download,
                "{path}"
            );
        }
        assert!(FileTarget::from_listing("a".into(), json!(["scope"]), &json!({}), "").is_none());
    }
    #[test]
    fn parent_handles_root_trailing_slashes_and_unicode() {
        assert_eq!(parent("/"), "/");
        assert_eq!(parent("/home/ruiyang/"), "/home");
        assert_eq!(parent("/日本語/file"), "/日本語");
        assert_eq!(parent("/home"), "/");
    }
    #[test]
    fn filtering_preserves_paths_and_sorts_folders_first() {
        let listing = json!({"entries":[
            {"name":"Zoo", "kind":"file", "path":"/Zoo"},
            {"name":"a", "kind":"file", "path":"/a"},
            {"name":".config", "kind":"directory", "path":"/.config"},
            {"name":"bin", "kind":"directory", "path":"/bin"}
        ]});
        let entries = visible_entries(&listing, "", false);
        assert_eq!(
            entries.iter().map(|v| text(v, "path")).collect::<Vec<_>>(),
            ["/bin", "/a", "/Zoo"]
        );
        assert_eq!(
            visible_entries(&listing, "CONFIG", true)[0]["path"],
            "/.config"
        );
        assert!(visible_entries(&listing, "CONFIG", false).is_empty());
    }
}
