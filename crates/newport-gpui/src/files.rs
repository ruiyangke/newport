//! Remote file browsing. New selections supersede in-flight reads.
use super::*;
use gpui_kit::component::Selectable;
use newport_client::file_transfer::Direction;
use std::{cell::RefCell, rc::Rc};
#[path = "file_list.rs"]
mod list;

#[cfg(test)]
#[path = "file_poll_tests.rs"]
mod poll_tests;

fn tree_width(content_width: f32) -> f32 {
    if content_width < 800. { 140. } else { 180. }
}

const PAGE_SIZE: usize = 40;

fn page_range(count: usize, page: usize) -> std::ops::Range<usize> {
    let start = page.min(count.saturating_sub(1) / PAGE_SIZE) * PAGE_SIZE;
    start..(start + PAGE_SIZE).min(count)
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

fn modified_date(value: &Value) -> String {
    value
        .as_i64()
        .and_then(|seconds| seconds.checked_mul(1000))
        .map(crate::localized_time::file_date)
        .unwrap_or_else(|| "—".into())
}

pub struct Browser {
    pub connection: Value,
    controls_connection: Value,
    pub(super) reads: newport_client::file_browser::Browser,
    pub(super) active: bool,
    path: Entity<InputState>,
    filter: Entity<InputState>,
    hidden: bool,
    tree_visible: bool,
    pub(super) tree: Entity<file_tree::FolderTree>,
    home: Option<String>,
    page: usize,
    project_path: Option<String>,
    projection: RefCell<Option<Projection>>,
    pub directory: String,
    pub(super) requested: String,
    pub canonical: Option<String>,
    reset_filter: bool,
    scroll: modal::FormScroll,
    _subscriptions: Vec<Subscription>,
}

struct Projection {
    revision: u64,
    query: String,
    hidden: bool,
    entries: Rc<Vec<Value>>,
}
impl Browser {
    fn entries(&self, cx: &App) -> Rc<Vec<Value>> {
        let revision = self.reads.listing_data_revision();
        let query = self.filter.read(cx).value().to_string();
        let mut cached = self.projection.borrow_mut();
        if cached.as_ref().is_none_or(|cache| {
            cache.revision != revision || cache.query != query || cache.hidden != self.hidden
        }) {
            let entries = self.reads.listing().map_or_else(Vec::new, |listing| {
                visible_entries(listing, &query, self.hidden)
            });
            *cached = Some(Projection {
                revision,
                query,
                hidden: self.hidden,
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

impl PreviewTarget {
    fn same_selection(&self, other: &Self) -> bool {
        self.server == other.server
            && self.connection == other.connection
            && self.token == other.token
            && self.path == other.path
    }
}

fn parent(path: &str) -> &str {
    let path = path.trim_end_matches('/');
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("/")
}

fn visible_entries(listing: &Value, query: &str, hidden: bool) -> Vec<Value> {
    let query = query.to_lowercase();
    let mut entries: Vec<_> = rows(&listing["entries"])
        .into_iter()
        .filter(|entry| {
            let name = text(entry, "name");
            (hidden || !name.starts_with('.')) && name.to_lowercase().contains(&query)
        })
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
                terminal_scope::connection(&profile, &self.snapshot) == *connection
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
                != terminal_scope::connection(&self.selected_server()?, &self.snapshot)
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

    pub(super) fn file_preview_selection_current(&self, target: &PreviewTarget) -> bool {
        self.file_preview_target()
            .is_some_and(|current| current.same_selection(target))
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

    pub(super) fn poll_file_browsers(&mut self, cx: &mut Context<Self>) -> bool {
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
            browser.tree.update(cx, |tree, cx| {
                tree.configure(
                    browser.home.clone(),
                    browser.directory.clone(),
                    browser.hidden,
                    visible && browser.tree_visible,
                    cx,
                );
                // The retained tree notifies its own subscribers. Its child
                // rows do not require rebuilding the Files page as well.
                tree.poll(cx);
            });
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
        let connection = terminal_scope::connection(&server, &self.snapshot);
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
            browser.tree.update(cx, |tree, cx| {
                tree.configure(
                    browser.home.clone(),
                    browser.directory.clone(),
                    browser.hidden,
                    browser.tree_visible,
                    cx,
                )
            });
            self.files_refresh_requested = false;
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
            let connection = terminal_scope::connection(&profile, &self.snapshot);
            let (directory, path_text, query, hidden, tree_visible, page) = self
                .file_browsers
                .get(&self.server)
                .map(|browser| {
                    (
                        browser.directory.clone(),
                        browser.path.read(cx).value().to_string(),
                        browser.filter.read(cx).value().to_string(),
                        browser.hidden,
                        browser.tree_visible,
                        browser.page,
                    )
                })
                .unwrap_or_else(|| (".".into(), ".".into(), String::new(), false, true, 0));
            let path = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Go to folder…")
                    .default_value(path_text)
            });
            let filter = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Filter this folder…")
                    .default_value(query)
            });
            let server = self.server.clone();
            let input_connection = connection.clone();
            let filter_server = server.clone();
            let filter_connection = connection.clone();
            let guard_view = cx.entity().downgrade();
            let guard_server = server.clone();
            let guard_connection = connection.clone();
            let tree = cx.new(|_| {
                file_tree::FolderTree::new(newport_client::file_browser::folders::Folders::new(
                    backend::Backend::discover(),
                    server.clone(),
                    connection.clone(),
                ))
                .with_scope_guard(move |cx| {
                    guard_view.upgrade().is_some_and(|view| {
                        let this = view.read(cx);
                        this.file_browser_current(&guard_server, &guard_connection)
                            && this.file_browsers[&guard_server].tree_visible
                    })
                })
            });
            let tree_server = server.clone();
            let tree_connection = connection.clone();
            let subscriptions = vec![
                cx.subscribe(&filter, move |this, _, event, cx| {
                    if !matches!(event, gpui_kit::component::input::InputEvent::Change)
                        || !this.file_browser_current(&filter_server, &filter_connection)
                    {
                        return;
                    }
                    if let Some(browser) = this.file_browsers.get_mut(&filter_server) {
                        browser.scroll.handle.scroll_to_top_of_item(0);
                        browser.page = 0;
                    }
                    cx.notify();
                }),
                cx.subscribe_in(
                    &tree,
                    window,
                    move |this, _, event: &file_tree::Navigate, window, cx| {
                        if this.file_browser_current(&tree_server, &tree_connection)
                            && this.file_browsers[&tree_server].tree_visible
                        {
                            this.files_navigate(event.0.clone(), window, cx);
                        }
                    },
                ),
                cx.subscribe_in(
                    &path,
                    window,
                    move |this,
                          input,
                          event: &gpui_kit::component::input::InputEvent,
                          window,
                          cx| {
                        if this.file_browser_current(&server, &input_connection)
                            && matches!(
                                event,
                                gpui_kit::component::input::InputEvent::PressEnter { .. }
                            )
                        {
                            this.files_navigate(input.read(cx).value().to_string(), window, cx);
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
                    tree_visible,
                    tree,
                    home: None,
                    page,
                    project_path: None,
                    projection: RefCell::new(None),
                    directory,
                    requested: String::new(),
                    canonical: None,
                    reset_filter: false,
                    scroll: modal::FormScroll::new(PAGE_SIZE, window, cx),
                    _subscriptions: subscriptions,
                },
            );
        }
        let browser = self.file_browsers.get_mut(&self.server).unwrap();
        if browser.reset_filter {
            browser.reset_filter = false;
            browser.page = 0;
            browser.scroll.handle.scroll_to_top_of_item(0);
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
        browser.tree.update(cx, |tree, cx| {
            tree.configure(
                browser.home.clone(),
                browser.directory.clone(),
                browser.hidden,
                browser.active && browser.tree_visible,
                cx,
            )
        });
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
        if window.has_active_dialog(cx) {
            return;
        }
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
        self.close_file_preview();
        let Some(browser) = self.file_browsers.get_mut(&self.server) else {
            return;
        };
        if browser.reads.open_preview(path.clone()).is_none() {
            return;
        }
        self.file_preview_path = Some(path);
        self.open_file_preview_dialog(entry, window, cx);
        cx.notify();
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
        let pending = browser.reads.listing_pending();
        let up = parent(&path).to_owned();
        let refresh = path.clone();
        let upload = self.file_action_target(&path);
        let project = upload.clone();
        let parent_target = upload.clone();
        let refresh_target = upload.clone();
        let server_for_refresh = self.server.clone();
        let navigation_scope = (self.server.clone(), browser.connection.clone());
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.status("file-project", cx))
            .when_some(browser.reads.listing_error(), |view, error| {
                let attempted = browser.reads.attempted_path().unwrap_or(".").to_owned();
                let message = format!("Cannot open {attempted}: {error}");
                let scope = browser.connection.clone();
                let server = self.server.clone();
                view.child(
                    div()
                        .id("file-listing-error")
                        .role(Role::Alert)
                        .aria_label(message.clone())
                        .flex()
                        .items_center()
                        .gap_2()
                        .p(px(crate::controls::PAGE_PADDING))
                        .flex_shrink_0()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(cx.theme().danger)
                                .child(message),
                        )
                        .child(
                            crate::controls::button("files-retry")
                                .label("Retry")
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(crate::controls::PAGE_PADDING))
                    .py(px(6.))
                    .flex_shrink_0()
                    .child(
                        crate::controls::icon_button("files-home")
                            .icon(IconName::House)
                            .accessibility_label("Home folder")
                            .disabled(pending)
                            .on_click({
                                let (server, connection) = navigation_scope.clone();
                                cx.listener(move |this, _, window, cx| {
                                    if this.file_browser_current(&server, &connection) {
                                        this.files_navigate(".".into(), window, cx);
                                    }
                                })
                            }),
                    )
                    .child(
                        crate::controls::icon_button("files-parent")
                            .icon(IconName::ArrowUp)
                            .accessibility_label("Parent folder")
                            .disabled(pending || path.is_empty() || path == "/")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if parent_target
                                    .as_ref()
                                    .is_some_and(|target| this.file_target_current(target))
                                {
                                    this.files_navigate(up.clone(), window, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(crate::controls::input(&browser.path).aria_label("Remote path")),
                    )
                    .child(
                        crate::controls::icon_button("files-refresh")
                            .icon(IconName::RefreshCw)
                            .accessibility_label("Refresh folder")
                            .disabled(pending || path.is_empty())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if refresh_target
                                    .as_ref()
                                    .is_some_and(|target| this.file_target_current(target))
                                {
                                    this.file_browsers[&server_for_refresh]
                                        .tree
                                        .update(cx, |tree, cx| tree.clear(cx));
                                    this.files_navigate(refresh.clone(), window, cx);
                                }
                            })),
                    )
                    .child(
                        crate::controls::button("files-upload")
                            .icon(IconName::Upload)
                            .label("Upload")
                            .disabled(upload.is_none() || self.file_transfer_busy())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(target) = &upload {
                                    this.start_file_transfer(target, Direction::Upload, cx);
                                }
                            })),
                    )
                    .child(
                        crate::controls::icon_button("files-open-project")
                            .icon(IconName::FolderGit2)
                            .accessibility_label("Open this folder as project")
                            .loading(
                                browser.project_path.as_deref() == Some(path.as_str())
                                    && self
                                        .resources
                                        .get("file-project")
                                        .is_some_and(|row| row.pending),
                            )
                            .disabled(
                                project.is_none()
                                    || self
                                        .resources
                                        .get("file-project")
                                        .is_some_and(|row| row.pending),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(target) = &project {
                                    this.file_open_project(target, cx);
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap_2()
                    .items_center()
                    .px(px(crate::controls::PAGE_PADDING))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        crate::controls::button("files-folders")
                            .label("Folders")
                            .ghost()
                            .toggled(browser.tree_visible)
                            .selected(browser.tree_visible)
                            .on_click({
                                let (server, connection) = navigation_scope.clone();
                                cx.listener(move |this, _, _, cx| {
                                    if this.file_browser_current(&server, &connection)
                                        && let Some(browser) = this.file_browsers.get_mut(&server)
                                    {
                                        browser.tree_visible = !browser.tree_visible;
                                        browser.tree.update(cx, |tree, cx| {
                                            tree.configure(
                                                browser.home.clone(),
                                                browser.directory.clone(),
                                                browser.hidden,
                                                browser.tree_visible,
                                                cx,
                                            )
                                        });
                                        cx.notify();
                                    }
                                })
                            }),
                    )
                    .child(
                        div()
                            .w(px(240_f32.min(self.content_width * 0.45)))
                            .min_w_0()
                            .child(
                                crate::controls::input(&browser.filter).aria_label("Filter files"),
                            ),
                    )
                    .when(!browser.filter.read(cx).value().is_empty(), |view| {
                        view.child(
                            crate::controls::button("files-clear-filter")
                                .ghost()
                                .label("Clear filter")
                                .on_click({
                                    let (server, connection) = navigation_scope.clone();
                                    cx.listener(move |this, _, window, cx| {
                                        if this.file_browser_current(&server, &connection)
                                            && let Some(browser) =
                                                this.file_browsers.get_mut(&server)
                                        {
                                            browser.page = 0;
                                            browser.scroll.handle.scroll_to_top_of_item(0);
                                            browser.filter.update(cx, |input, cx| {
                                                input.set_value("", window, cx)
                                            });
                                            cx.notify();
                                        }
                                    })
                                }),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        crate::controls::button("files-hidden")
                            .label("Hidden files")
                            .ghost()
                            .toggled(browser.hidden)
                            .selected(browser.hidden)
                            .on_click({
                                let (server, connection) = navigation_scope.clone();
                                cx.listener(move |this, _, _, cx| {
                                    if this.file_browser_current(&server, &connection)
                                        && let Some(browser) = this.file_browsers.get_mut(&server)
                                    {
                                        browser.hidden = !browser.hidden;
                                        browser.page = 0;
                                        browser.scroll.handle.scroll_to_top_of_item(0);
                                        browser.tree.update(cx, |tree, cx| {
                                            tree.configure(
                                                browser.home.clone(),
                                                browser.directory.clone(),
                                                browser.hidden,
                                                browser.tree_visible,
                                                cx,
                                            )
                                        });
                                        cx.notify();
                                    }
                                })
                            }),
                    ),
            )
            .child(
                div()
                    .id("files-layout")
                    .debug_selector(|| "files-layout".into())
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .border_y_1()
                    .border_color(cx.theme().border)
                    .when(browser.tree_visible, |view| {
                        view.child(
                            div()
                                .w(px(tree_width(self.content_width)))
                                .flex_shrink_0()
                                .min_h_0()
                                .border_r_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().muted.opacity(0.35))
                                .child(browser.tree.clone()),
                        )
                    })
                    .child(self.file_listing(window, cx)),
            )
            .child(self.file_listing_footer(cx))
            .when(self.file_preview_path.is_none(), |view| {
                view.child(self.transfer_status(cx))
            })
            .into_any_element()
    }
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
    use super::{Direction, FileTarget, PreviewTarget, page_range, parent, visible_entries};
    use crate::text;
    use serde_json::json;

    #[test]
    fn preview_modal_lifetime_follows_selection_while_controls_follow_render() {
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
        assert!(
            opening.same_selection(&loaded),
            "loading must not retire its own modal"
        );
        assert!(
            opening != loaded,
            "retained controls still reject a replaced render"
        );
        let mut replacement = loaded.clone();
        replacement.token = 2;
        assert!(
            !opening.same_selection(&replacement),
            "A → B → A is a new preview"
        );
        replacement = loaded.clone();
        replacement.server = "b".into();
        assert!(!opening.same_selection(&replacement));
        replacement = loaded.clone();
        replacement.connection = json!(["connection-b"]);
        assert!(!opening.same_selection(&replacement));
        replacement = loaded;
        replacement.path = "/work/other".into();
        assert!(!opening.same_selection(&replacement));
    }

    #[test]
    fn four_hundred_files_form_ten_pages_and_search_includes_later_pages() {
        let listing = json!({"entries": (0..400).rev().map(|i| json!({
            "name": format!("file-{i:03}"), "path": format!("/work/{i}"), "kind": "file"
        })).collect::<Vec<_>>()});
        let all = visible_entries(&listing, "", false);
        let pages: Vec<_> = (0..10)
            .map(|page| &all[page_range(all.len(), page)])
            .collect();
        assert!(pages.iter().all(|page| page.len() == 40));
        assert_eq!(text(&pages[9][39], "name"), "file-399");
        let found = visible_entries(&listing, "file-399", false);
        assert_eq!(found.len(), 1);
        assert_eq!(
            text(&found[page_range(found.len(), 9)][0], "name"),
            "file-399"
        );
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
    fn file_pages_clamp_after_filtering_and_never_skip_the_last_item() {
        assert_eq!(page_range(0, usize::MAX), 0..0);
        assert_eq!(page_range(40, 1), 0..40);
        assert_eq!(page_range(41, 1), 40..41);
        assert_eq!(page_range(201, usize::MAX), 200..201);
        assert_eq!(page_range(9, 2), 0..9);
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
