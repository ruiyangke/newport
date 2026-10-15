//! Kit remote chooser. Network decoding and page consistency live in the shared client.
use super::*;
use gpui_kit::component::{
    IndexPath,
    list::{List, ListDelegate, ListItem, ListState},
    popover::Popover,
};
use newport_client::git_remotes::Names;

struct Delegate {
    owner: WeakEntity<RemotePicker>,
    entries: Vec<String>,
    cursor: Option<IndexPath>,
    loading: bool,
    failed: bool,
    more: bool,
}
impl ListDelegate for Delegate {
    type Item = ListItem;
    fn items_count(&self, _: usize, _: &App) -> usize {
        self.entries.len()
    }
    fn item_accessibility_label(&self, ix: IndexPath, _: &App) -> Option<SharedString> {
        self.entries.get(ix.row).cloned().map(Into::into)
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        self.entries.get(ix.row).map(|name| {
            ListItem::new(ix.row)
                .h(px(28.))
                .text_size(px(12.))
                .child(name.clone())
        })
    }
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        self.cursor = ix;
    }
    fn confirm(&mut self, _: bool, _: &mut Window, cx: &mut Context<ListState<Self>>) {
        let Some(name) = self.cursor.and_then(|ix| self.entries.get(ix.row)).cloned() else {
            return;
        };
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |picker, cx| picker.choose(name, cx));
        });
    }
    fn cancel(&mut self, _: &mut Window, cx: &mut Context<ListState<Self>>) {
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |picker, cx| {
                picker.open = false;
                cx.notify();
            });
        });
    }
    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.entries.clear();
        self.cursor = None;
        self.loading = true;
        let query = query.to_owned();
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |picker, cx| {
                if picker.query != query {
                    picker.query = query;
                    picker.read_names(false, true, cx);
                } else {
                    picker.sync_list(cx);
                }
            });
        });
        Task::ready(())
    }
    fn loading(&self, _: &App) -> bool {
        self.loading
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div().when(!self.failed, |view| {
            view.child(empty::compact("No matching remotes.", cx))
        })
    }
    fn has_more(&self, _: &App) -> bool {
        self.more && !self.loading
    }
    fn load_more_threshold(&self) -> usize {
        2
    }
    fn load_more(&mut self, _: &mut Window, cx: &mut Context<ListState<Self>>) {
        self.more = false;
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |picker, cx| picker.read_names(true, false, cx));
        });
    }
}
pub(super) struct RemotePicker {
    backend: backend::Backend,
    server: String,
    repo: String,
    open: bool,
    enabled: bool,
    list: Entity<ListState<Delegate>>,
    page: Option<Names>,
    query: String,
    names_pending: bool,
    names_error: Option<String>,
    generation: u64,
    names_task: Option<Task<()>>,
    name: String,
    initial_default: bool,
    detail: Value,
    detail_pending: bool,
    detail_error: Option<String>,
    detail_generation: u64,
    detail_task: Option<Task<()>>,
}
impl RemotePicker {
    pub(super) fn new(
        server: String,
        repo: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let owner = cx.entity().downgrade();
        let list = cx.new(|cx| {
            ListState::new(
                Delegate {
                    owner,
                    entries: vec![],
                    cursor: None,
                    loading: true,
                    failed: false,
                    more: false,
                },
                window,
                cx,
            )
            .searchable(true)
        });
        let mut picker = Self {
            backend: backend::Backend::discover(),
            server,
            repo,
            open: false,
            enabled: true,
            list,
            page: None,
            query: String::new(),
            names_pending: false,
            names_error: None,
            generation: 0,
            names_task: None,
            name: "origin".into(),
            initial_default: true,
            detail: Value::Null,
            detail_pending: false,
            detail_error: None,
            detail_generation: 0,
            detail_task: None,
        };
        picker.read_names(false, false, cx);
        picker.read_detail(cx);
        picker
    }
    pub(super) fn remote(&self) -> Option<&Value> {
        (!self.detail_pending
            && self.detail_error.is_none()
            && self.detail["name"] == self.name
            && self.detail["token"].is_string())
        .then_some(&self.detail)
    }
    pub(super) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            self.open = false;
        }
        cx.notify();
    }
    fn sync_list(&self, cx: &mut Context<Self>) {
        let entries = self
            .page
            .as_ref()
            .map(|p| p.entries.clone())
            .unwrap_or_default();
        let more = self.page.as_ref().is_some_and(|p| p.next_cursor.is_some())
            && !self.names_pending
            && self.names_error.is_none();
        let loading = self.names_pending && self.page.is_none();
        self.list.update(cx, |list, cx| {
            let delegate = list.delegate_mut();
            delegate.entries = entries;
            delegate.more = more;
            delegate.loading = loading;
            delegate.failed = self.names_error.is_some();
            cx.notify();
        });
        cx.notify();
    }
    fn read_names(&mut self, more: bool, debounce: bool, cx: &mut Context<Self>) {
        if more && (self.names_pending || self.names_error.is_some() || !self.enabled) {
            return;
        }
        let cursor = if more {
            let Some(cursor) = self.page.as_ref().and_then(|p| p.next_cursor.clone()) else {
                return;
            };
            Some(cursor)
        } else {
            self.page = None;
            None
        };
        self.generation += 1;
        let generation = self.generation;
        self.names_pending = true;
        self.names_error = None;
        self.sync_list(cx);
        let backend = self.backend.clone();
        let server = self.server.clone();
        let repo = self.repo.clone();
        let query = self.query.clone();
        let executor = cx.background_executor().clone();
        self.names_task = Some(cx.spawn(async move |owner, cx| {
            if debounce {
                executor.timer(Duration::from_millis(200)).await;
            }
            let sent_cursor = cursor.clone();
            let result = executor
                .spawn(async move {
                    backend.git_remote_names(&server, &repo, &query, sent_cursor.as_deref())
                })
                .await;
            let _ = owner.update(cx, |picker, cx| {
                if picker.generation != generation {
                    return;
                }
                picker.names_pending = false;
                let result = result.and_then(|page| {
                    if let Some(cursor) = cursor {
                        picker
                            .page
                            .as_mut()
                            .ok_or("Reload remotes.")?
                            .append(page, &cursor)
                    } else {
                        picker.page = Some(page);
                        Ok(())
                    }
                });
                picker.names_error = result.err();
                picker.fallback_default(cx);
                picker.sync_list(cx);
            });
        }));
    }
    fn fallback_default(&mut self, cx: &mut Context<Self>) {
        // Only recover the initial default when a complete name page proves
        // origin is absent. A failed read must not silently change a user choice.
        if !self.initial_default
            || self.detail_error.is_none()
            || self.name != "origin"
            || !self.query.is_empty()
        {
            return;
        }
        let Some(page) = self.page.as_ref() else {
            return;
        };
        if page.next_cursor.is_none() && !page.entries.iter().any(|n| n == "origin") {
            if let Some(first) = page.default_name().map(str::to_owned) {
                self.name = first;
                self.read_detail(cx);
            } else {
                self.name.clear();
                self.detail_error = None;
            }
        }
    }
    fn choose(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.enabled
            || !self
                .page
                .as_ref()
                .is_some_and(|p| p.entries.contains(&name))
        {
            return;
        }
        self.open = false;
        self.initial_default = false;
        self.name = name;
        self.read_detail(cx);
    }
    fn read_detail(&mut self, cx: &mut Context<Self>) {
        self.detail = Value::Null;
        self.detail_error = None;
        self.detail_pending = true;
        self.detail_generation += 1;
        let generation = self.detail_generation;
        let backend = self.backend.clone();
        let server = self.server.clone();
        let repo = self.repo.clone();
        let name = self.name.clone();
        let task = cx
            .background_executor()
            .spawn(async move { backend.git_remote(&server, &repo, &name) });
        self.detail_task = Some(cx.spawn(async move |owner, cx| {
            let result = task.await;
            let _ = owner.update(cx, |picker, cx| {
                if picker.detail_generation != generation {
                    return;
                }
                picker.detail_pending = false;
                match result {
                    Ok(remote) => picker.detail = remote,
                    Err(error) => picker.detail_error = Some(error),
                }
                picker.fallback_default(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn menu(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w(px(320.))
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div().h(px(260.)).child(
                    List::new(&self.list)
                        .with_size(gpui_kit::component::Size::Medium)
                        .search_placeholder("Search remotes…")
                        .h_full(),
                ),
            )
            .child({
                let message = if self.names_pending && self.page.is_some() {
                    "Loading more remotes…".to_owned()
                } else {
                    self.page
                        .as_ref()
                        .map(|page| {
                            format!("{} of {} remotes loaded", page.entries.len(), page.total)
                        })
                        .unwrap_or_default()
                };
                div()
                    .id("remotes-page-status")
                    .role(accesskit::Role::Status)
                    .aria_label(message.clone())
                    .h(px(24.))
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(message)
            })
            .when_some(self.names_error.as_ref(), |view, error| {
                view.child(
                    label(error.clone())
                        .p_2()
                        .text_xs()
                        .text_color(cx.theme().danger),
                )
                .child(
                    crate::controls::button("retry-remote-names")
                        .label("Reload remotes")
                        .on_click(cx.listener(|this, _, _, cx| this.read_names(false, false, cx))),
                )
            })
            .into_any_element()
    }
}
impl Render for RemotePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.entity().downgrade();
        let content = owner.clone();
        let remote = self.remote();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Popover::new("remote-picker")
                    .open(self.open)
                    .p_0()
                    .offset(px(4.))
                    .trigger(
                        crate::controls::button("remote-picker-trigger")
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .w_full()
                                    .child(div().min_w_0().truncate().child(
                                        if self.name.is_empty() {
                                            "Choose remote".to_owned()
                                        } else {
                                            self.name.clone()
                                        },
                                    ))
                                    .child(Icon::new(IconName::ChevronDown).size(px(14.))),
                            )
                            .accessibility_label(format!(
                                "Remote: {}",
                                if self.name.is_empty() {
                                    "Choose remote"
                                } else {
                                    &self.name
                                }
                            ))
                            .disabled(!self.enabled)
                            .w_full(),
                    )
                    .on_open_change(move |open, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.open = *open && this.enabled;
                            if this.open {
                                this.list.update(cx, |list, cx| {
                                    list.set_query("", window, cx);
                                    list.focus(window, cx);
                                });
                            }
                            cx.notify();
                        });
                    })
                    .content(move |_, _, cx| {
                        content
                            .update(cx, |this, cx| this.menu(cx))
                            .unwrap_or_else(|_| div().into_any_element())
                    }),
            )
            .child(project_loading::picker_detail(
                "remote-configuration",
                remote.map(|remote| {
                    remote["pushUrl"]
                        .as_str()
                        .or_else(|| remote["url"].as_str())
                        .unwrap_or("Remote URL unavailable")
                        .to_owned()
                }),
                self.detail_pending || (self.names_pending && self.name.is_empty()),
            ))
            .when_some(self.detail_error.as_ref(), |view, error| {
                view.child(label(error.clone()).text_xs().text_color(cx.theme().danger))
                    .child(
                        crate::controls::button("retry-remote-detail")
                            .label("Retry remote")
                            .disabled(!self.enabled)
                            .on_click(cx.listener(|this, _, _, cx| this.read_detail(cx))),
                    )
            })
            .when(
                self.query.is_empty() && self.page.as_ref().is_some_and(|p| p.total == 0),
                |view| {
                    view.child(empty::compact(
                        "No remotes configured. Add one in Remotes first.",
                        cx,
                    ))
                },
            )
    }
}
