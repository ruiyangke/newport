//! Searchable, paginated branch choices for worktree creation.
use super::*;
use gpui_kit::component::{
    IndexPath,
    list::{List, ListDelegate, ListItem, ListState},
    popover::Popover,
};
use newport_client::git_branch::{BranchChoices, WorktreeBranch, WorktreeBranchUse};

struct Delegate {
    owner: WeakEntity<WorktreeBranchPicker>,
    entries: Vec<WorktreeBranch>,
    purpose: WorktreeBranchUse,
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
        self.entries
            .get(ix.row)
            .map(|branch| choice_label(branch, self.purpose).into())
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        self.entries.get(ix.row).map(|branch| {
            ListItem::new(ix.row)
                .h(px(28.))
                .text_size(px(crate::ui::FONT_SIZE))
                .child(choice_label(branch, self.purpose))
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
        let Some(name) = self
            .cursor
            .and_then(|ix| self.entries.get(ix.row))
            .map(|branch| branch.reference.clone())
        else {
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
            view.child(empty::compact("No matching branches.", cx))
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
pub(super) struct WorktreeBranchPicker {
    backend: backend::Backend,
    server: String,
    repo: String,
    open: bool,
    enabled: bool,
    list: Entity<ListState<Delegate>>,
    page: Option<BranchChoices>,
    query: String,
    names_pending: bool,
    names_error: Option<String>,
    generation: u64,
    names_task: Option<Task<()>>,
    name: String,
    bare: bool,
    purpose: WorktreeBranchUse,
    selected: Option<WorktreeBranch>,
    owner: Option<String>,
    detail_pending: bool,
    detail_error: Option<String>,
    detail_generation: u64,
    detail_task: Option<Task<()>>,
}
fn choice_label(branch: &WorktreeBranch, purpose: WorktreeBranchUse) -> String {
    if purpose == WorktreeBranchUse::StartingPoint {
        format!(
            "{} · {}",
            branch.name,
            if branch.reference.starts_with("refs/remotes/") {
                "Remote"
            } else {
                "Local"
            }
        )
    } else {
        branch.name.clone()
    }
}
impl WorktreeBranchPicker {
    fn placeholder(&self) -> &'static str {
        if self.purpose == WorktreeBranchUse::StartingPoint {
            "Current commit"
        } else {
            "Choose local branch"
        }
    }
    pub(super) fn new(
        server: String,
        repo: String,
        bare: bool,
        purpose: WorktreeBranchUse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let owner = cx.entity().downgrade();
        let list = cx.new(|cx| {
            ListState::new(
                Delegate {
                    owner,
                    entries: vec![],
                    purpose,
                    cursor: None,
                    loading: false,
                    failed: false,
                    more: false,
                },
                window,
                cx,
            )
            .searchable(true)
        });
        Self {
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
            name: String::new(),
            bare,
            purpose,
            selected: None,
            owner: None,
            detail_pending: false,
            detail_error: None,
            detail_generation: 0,
            detail_task: None,
        }
    }
    pub(super) fn branch(&self) -> Option<&WorktreeBranch> {
        if self.detail_pending || self.detail_error.is_some() || self.owner.is_some() {
            return None;
        }
        self.selected.as_ref()
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
            .map(|p| {
                p.entries()
                    .iter()
                    .filter_map(|entry| self.purpose.candidate(entry, self.bare))
                    .collect()
            })
            .unwrap_or_default();
        let more = self
            .page
            .as_ref()
            .is_some_and(|p| p.next_cursor().is_some())
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
            let Some(cursor) = self
                .page
                .as_ref()
                .and_then(|p| p.next_cursor().map(str::to_owned))
            else {
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
        let purpose = self.purpose;
        let executor = cx.background_executor().clone();
        self.names_task = Some(cx.spawn(async move |owner, cx| {
            if debounce {
                executor.timer(Duration::from_millis(200)).await;
            }
            let sent_cursor = cursor.clone();
            let result = executor
                .spawn(async move {
                    backend.worktree_branch_choices(
                        &server,
                        &repo,
                        &query,
                        sent_cursor.as_deref(),
                        purpose,
                    )
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
                            .ok_or("Reload branches.")?
                            .append(&page, &cursor)
                    } else {
                        picker.page = Some(BranchChoices::from_page(&page)?);
                        Ok(())
                    }
                });
                picker.names_error = result.err();
                picker.sync_list(cx);
            });
        }));
    }
    fn choose(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        let Some(branch) = self.page.as_ref().and_then(|page| {
            page.entries()
                .iter()
                .filter_map(|entry| self.purpose.candidate(entry, self.bare))
                .find(|branch| branch.reference == name)
        }) else {
            return;
        };
        self.open = false;
        self.name = choice_label(&branch, self.purpose);
        self.selected = Some(branch);
        if self.purpose == WorktreeBranchUse::Existing {
            self.read_detail(cx);
        } else {
            cx.notify();
        }
    }
    fn read_detail(&mut self, cx: &mut Context<Self>) {
        self.owner = None;
        let Some(branch) = self.selected.clone() else {
            return;
        };
        self.detail_error = None;
        self.detail_pending = true;
        self.detail_generation += 1;
        let generation = self.detail_generation;
        let backend = self.backend.clone();
        let server = self.server.clone();
        let repo = self.repo.clone();
        let task = cx
            .background_executor()
            .spawn(async move { backend.worktree_branch_owner(&server, &repo, &branch) });
        self.detail_task = Some(cx.spawn(async move |owner, cx| {
            let result = task.await;
            let _ = owner.update(cx, |picker, cx| {
                if picker.detail_generation != generation {
                    return;
                }
                picker.detail_pending = false;
                match result {
                    Ok(owner) => picker.owner = owner,
                    Err(error) => picker.detail_error = Some(error),
                }
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
                        .search_placeholder(if self.purpose == WorktreeBranchUse::Existing {
                            "Search local branches…"
                        } else {
                            "Search local and remote branches…"
                        })
                        .h_full(),
                ),
            )
            .child({
                let message = if self.names_pending && self.page.is_some() {
                    "Loading more branches…".to_owned()
                } else {
                    self.page
                        .as_ref()
                        .map(|page| format!("{} branches loaded", page.entries().len()))
                        .unwrap_or_default()
                };
                div()
                    .id("branches-page-status")
                    .role(accesskit::Role::Status)
                    .aria_label(message.clone())
                    .h(px(24.))
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_color(cx.theme().muted_foreground)
                    .child(message)
            })
            .when_some(self.names_error.as_ref(), |view, error| {
                view.child(
                    crate::label(error.clone())
                        .p_2()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(cx.theme().danger),
                )
                .child(
                    crate::controls::button("retry-worktree-branch-names")
                        .label("Reload branches")
                        .on_click(cx.listener(|this, _, _, cx| this.read_names(false, false, cx))),
                )
            })
            .into_any_element()
    }
}
impl Render for WorktreeBranchPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.entity().downgrade();
        let content = owner.clone();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Popover::new("worktree-branch-picker")
                    .open(self.open)
                    .p_0()
                    .offset(px(4.))
                    .trigger(
                        crate::controls::button("worktree-branch-picker-trigger")
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .w_full()
                                    .child(div().min_w_0().truncate().child(
                                        if self.name.is_empty() {
                                            self.placeholder().to_owned()
                                        } else {
                                            self.name.clone()
                                        },
                                    ))
                                    .child(Icon::new(IconName::ChevronDown).size(px(14.))),
                            )
                            .accessibility_label(format!(
                                "{}: {}",
                                if self.purpose == WorktreeBranchUse::Existing {
                                    "Local branch"
                                } else {
                                    "Start from"
                                },
                                if self.name.is_empty() {
                                    self.placeholder()
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
                                if this.page.is_none() && !this.names_pending {
                                    this.read_names(false, false, cx);
                                }
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
            .when_some(self.detail_error.as_ref(), |view, error| {
                view.child(
                    crate::label(error.clone())
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(cx.theme().danger),
                )
                .child(
                    crate::controls::button("retry-worktree-branch-detail")
                        .label("Check branch again")
                        .disabled(!self.enabled)
                        .on_click(cx.listener(|this, _, _, cx| this.read_detail(cx))),
                )
            })
            .when_some(self.owner.as_ref(), |view, path| {
                let message = format!("Already checked out in {path}. Choose another branch.");
                view.child(
                    crate::label(message.clone())
                        .id("worktree-branch-occupied")
                        .role(accesskit::Role::Label)
                        .aria_label(message)
                        .w_full()
                        .whitespace_normal()
                        .text_size(px(crate::ui::FONT_SIZE)),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{BranchChoices, WorktreeBranchPicker, WorktreeBranchUse};
    use base64::Engine as _;
    use gpui_kit::{self as gpui, TestAppContext};
    use serde_json::json;

    struct SearchMenu(gpui::Entity<WorktreeBranchPicker>);
    impl gpui::Render for SearchMenu {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            window.set_rem_size(gpui::px(14.));
            self.0.update(cx, |picker, cx| picker.menu(cx))
        }
    }

    #[gpui::test]
    fn branch_search_matches_standard_input_height(cx: &mut TestAppContext) {
        use gpui::AppContext;
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (_, visual) = cx.add_window_view(|window, cx| {
            SearchMenu(cx.new(|cx| {
                WorktreeBranchPicker::new(
                    "server".into(),
                    "repo".into(),
                    false,
                    WorktreeBranchUse::Existing,
                    window,
                    cx,
                )
            }))
        });
        visual.simulate_resize(gpui::size(gpui::px(360.), gpui::px(400.)));
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            let nodes = gpui::base::test_support::snapshots(window);
            let input = nodes
                .iter()
                .find(|node| node.role() == Some(gpui::Role::TextInput))
                .expect("branch search input");
            assert_eq!(
                input.bounds().size.height,
                gpui::px(crate::controls::CONTROL_HEIGHT)
            );
        });
    }

    #[gpui::test]
    fn starting_point_selection_uses_full_refs_and_survives_search_and_disable(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (picker, visual) = cx.add_window_view(|window, cx| {
            WorktreeBranchPicker::new(
                "server".into(),
                "repo".into(),
                false,
                WorktreeBranchUse::StartingPoint,
                window,
                cx,
            )
        });
        let wire = |s: &str| json!({"display":s,"bytesB64":base64::engine::general_purpose::STANDARD.encode(s)});
        let local = json!({"name":wire("origin/topic"),"reference":wire("refs/heads/origin/topic"),"remote":false,"current":true,"oid":{"algorithm":"sha1","hex":"a".repeat(40)}});
        let remote = json!({"name":wire("origin/topic"),"reference":wire("refs/remotes/origin/topic"),"remote":true,"current":false,"oid":{"algorithm":"sha1","hex":"b".repeat(40)}});
        picker.update(visual, |picker, cx| {
            assert!(!picker.names_pending, "Closed pickers must not eagerly read branches");
            picker.page = Some(BranchChoices::from_page(&json!({"snapshot":"s","metadata":{},"entries":[local,remote],"nextCursor":null})).unwrap());
            picker.choose("refs/heads/origin/topic".into(), cx);
            assert_eq!(picker.branch().unwrap().oid, "a".repeat(40));
            assert_eq!(picker.name, "origin/topic · Local");
            picker.choose("refs/remotes/origin/topic".into(), cx);
            assert_eq!(picker.branch().unwrap().oid, "b".repeat(40));
            assert_eq!(picker.name, "origin/topic · Remote");
            assert!(!picker.detail_pending, "A starting point needs no occupancy request");
            picker.page = None;
            picker.names_pending = true;
            picker.set_enabled(false, cx);
            assert_eq!(picker.branch().unwrap().reference, "refs/remotes/origin/topic");
        });
    }
}
