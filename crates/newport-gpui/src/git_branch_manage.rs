//! Branch review presentation; validation and wire actions live in newport-client.
use super::*;
use gpui_kit::component::{
    IndexPath,
    select::{SelectDelegate, SelectEvent, SelectItem, SelectState},
};
use newport_client::git_branch::{BranchChoices, BranchReview};
use std::{cell::RefCell, rc::Rc};

const CHOICES: &str = "git-branch-upstreams";
const MORE: &str = "git-branch-upstreams-more";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Delete,
    Upstream,
}
#[derive(Clone)]
struct Choice {
    reference: String,
    title: SharedString,
    unavailable: bool,
}
impl SelectItem for Choice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &String {
        &self.reference
    }
    fn disabled(&self) -> bool {
        self.unavailable
    }
}
struct Choices {
    items: Vec<Choice>,
    query: Rc<RefCell<String>>,
}
impl SelectDelegate for Choices {
    type Item = Choice;
    fn items_count(&self, _: usize) -> usize {
        self.items.len()
    }
    fn item(&self, index: IndexPath) -> Option<&Choice> {
        self.items.get(index.row)
    }
    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: SelectItem<Value = V>,
        V: PartialEq,
    {
        self.items
            .iter()
            .position(|item| item.value() == value)
            .map(|row| IndexPath::default().row(row))
    }
    fn perform_search(&mut self, query: &str, _: &mut Window, _: &mut App) -> Task<()> {
        let query = query.trim().to_lowercase();
        if *self.query.borrow() != query {
            *self.query.borrow_mut() = query;
            self.items.clear();
        }
        Task::ready(())
    }
}
pub(super) type Draft = (uuid::Uuid, bool, String);
pub(super) struct Form {
    id: uuid::Uuid,
    kind: Kind,
    review: BranchReview,
    force: bool,
    upstream: String,
    choices: Option<BranchChoices>,
    cursor: Option<String>,
    error: Option<String>,
    selector: Entity<SelectState<Choices>>,
    query_input: Rc<RefCell<String>>,
    query: String,
    search_pending: Option<Instant>,
    selected_target: Option<Value>,
    selector_dirty: bool,
    last_page_offset: Option<Pixels>,
    pub(super) needs_review: bool,
    _subscriptions: Vec<Subscription>,
}
impl Form {
    pub(super) fn title(&self) -> &'static str {
        if self.kind == Kind::Delete {
            "Delete local branch"
        } else {
            "Set upstream branch"
        }
    }

    fn update_query(&mut self) -> bool {
        let query = self.query_input.borrow().clone();
        if self.query == query {
            return false;
        }
        if let Some(target) = self
            .choices
            .as_ref()
            .and_then(|choices| choices.find_reference(&self.upstream))
        {
            self.selected_target = Some(target.clone());
        }
        self.query = query;
        self.choices = None;
        self.cursor = None;
        self.last_page_offset = None;
        self.selector_dirty = true;
        self.error = (self.query.len() > 1024 || self.query.contains('\0'))
            .then(|| "Search is too long or contains an invalid character.".into());
        self.search_pending = self.error.is_none().then(Instant::now);
        true
    }
    pub(super) fn draft(&self) -> Draft {
        (self.id, self.force, self.upstream.clone())
    }
    pub(super) fn accept_outcome(&mut self, draft: &Draft, succeeded: bool) -> bool {
        if self.draft() != *draft {
            return false;
        }
        self.needs_review = !succeeded;
        succeeded
    }
    fn action(&self) -> Result<Value, String> {
        if self.needs_review {
            return Err("Refresh and select the branch again.".into());
        }
        match self.kind {
            Kind::Delete => self.review.delete_action(self.force),
            Kind::Upstream if self.upstream.is_empty() => self.review.set_upstream_action(None),
            Kind::Upstream => self.review.set_upstream_action(Some(
                self.choices
                    .as_ref()
                    .and_then(|choices| choices.find_reference(&self.upstream))
                    .or_else(|| {
                        self.selected_target
                            .as_ref()
                            .filter(|target| target["reference"]["display"] == self.upstream)
                    })
                    .ok_or("The selected upstream is not loaded or is unavailable.")?,
            )),
        }
    }
    fn items(&self) -> Vec<Choice> {
        let mut items = vec![Choice {
            reference: String::new(),
            title: "None — remove tracking".into(),
            unavailable: false,
        }];
        if let Some(choices) = &self.choices {
            items.extend(choices.candidates(&self.review).map(|entry| {
                Choice {
                    reference: text(&entry["reference"], "display"),
                    title: format!(
                        "{} · {}",
                        text(&entry["name"], "display"),
                        if entry["remote"] == true {
                            "remote"
                        } else {
                            "local"
                        }
                    )
                    .into(),
                    unavailable: false,
                }
            }));
        }
        if !self.upstream.is_empty()
            && !items.iter().any(|choice| choice.reference == self.upstream)
        {
            let cached = self
                .selected_target
                .as_ref()
                .filter(|target| target["reference"]["display"] == self.upstream);
            items.insert(
                1,
                Choice {
                    reference: self.upstream.clone(),
                    title: cached
                        .map_or_else(
                            || format!("{} (not loaded or unavailable)", self.upstream),
                            |target| {
                                format!(
                                    "{} · {}",
                                    text(&target["name"], "display"),
                                    if target["remote"] == true {
                                        "remote"
                                    } else {
                                        "local"
                                    }
                                )
                            },
                        )
                        .into(),
                    unavailable: cached.is_none(),
                },
            );
        }
        items
    }
    fn sync_selector(&mut self, window: &mut Window, cx: &mut App) {
        if !self.selector_dirty {
            return;
        }
        self.selector_dirty = false;
        let items = self.items();
        let selected = items
            .iter()
            .position(|item| item.reference == self.upstream)
            .map(|row| IndexPath::default().row(row));
        self.selector.update(cx, |select, cx| {
            select.set_items(
                Choices {
                    items,
                    query: self.query_input.clone(),
                },
                window,
                cx,
            );
            if !select.is_open() {
                // The paged delegate already includes the selected item. A
                // value lookup would clear the remote search and its results.
                select.set_selected_index(selected, window, cx);
            }
        });
    }
    fn settle_choices(&mut self, key: &str, result: Result<Value, String>) {
        let cursor = self.cursor.take();
        let result = result.and_then(|data| {
            if key == CHOICES {
                BranchChoices::from_page(&data).map(|choices| self.choices = Some(choices))
            } else if let (Some(choices), Some(cursor)) = (self.choices.as_mut(), cursor) {
                choices.append(&data, &cursor)
            } else {
                Err("Branch results changed. Reload upstream branches.".into())
            }
        });
        self.error = result.err();
        self.selector_dirty = true;
    }
}

impl Newport {
    fn branch_review(&self) -> Option<BranchReview> {
        if !self.git_can_write_branch() {
            return None;
        }
        BranchReview::from_entry(
            &self.git_repository_id(),
            &text(&self.value("git-branch-status"), "snapshot"),
            &self.git.branches.selected,
        )
        .ok()
    }
    pub(super) fn git_branch_review_open(&self) -> bool {
        self.git.branch_manage.is_some()
    }
    pub(super) fn open_branch_manage(
        &mut self,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git.branch_manage.is_some()
            || self.git.branch_form.is_some()
            || self.git.worktree_create.is_some()
        {
            return;
        }
        let Some(review) = self.branch_review() else {
            return;
        };
        if (kind == Kind::Delete && !review.can_delete())
            || (kind == Kind::Upstream && !review.can_set_upstream())
        {
            return;
        }
        let upstream = review.upstream().unwrap_or_default().to_owned();
        let query_input = Rc::<RefCell<String>>::default();
        let selector = cx.new(|cx| {
            SelectState::new(
                Choices {
                    items: vec![],
                    query: query_input.clone(),
                },
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        let id = uuid::Uuid::new_v4();
        let subscription = cx.subscribe(&selector, move |this, _, event, cx| {
            if this.git_mutation_blocks_write() {
                return;
            }
            if let SelectEvent::Confirm(Some(value)) = event
                && let Some(form) = this.git.branch_manage.as_mut()
                && form.id == id
                && !form.needs_review
            {
                let target = form
                    .choices
                    .as_ref()
                    .and_then(|choices| choices.find_reference(value))
                    .or_else(|| {
                        form.selected_target
                            .as_ref()
                            .filter(|target| target["reference"]["display"] == *value)
                    })
                    .cloned();
                if !value.is_empty() && target.is_none() {
                    return;
                }
                form.upstream = value.clone();
                form.selected_target = target;
                cx.notify();
            }
        });
        self.git.branch_manage = Some(Form {
            id,
            kind,
            review,
            force: false,
            upstream,
            choices: None,
            cursor: None,
            error: None,
            selector,
            query_input,
            query: String::new(),
            search_pending: None,
            selected_target: None,
            selector_dirty: true,
            last_page_offset: None,
            needs_review: false,
            _subscriptions: vec![subscription],
        });
        self.resources.invalidate(CHOICES);
        self.resources.invalidate(MORE);
        if kind == Kind::Upstream {
            self.load_branch_choices(false);
        }
        cx.notify();
    }
    pub(super) fn sync_branch_manage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut more = false;
        let pending = [CHOICES, MORE]
            .iter()
            .any(|key| self.resources.get(key).is_some_and(|r| r.pending));
        let busy = self.git_mutation_blocks_write();
        if let Some(form) = self.git.branch_manage.as_mut() {
            form.sync_selector(window, cx);
            if !pending
                && form.search_pending.is_none()
                && !busy
                && !form.needs_review
                && form.error.is_none()
                && form
                    .choices
                    .as_ref()
                    .and_then(|choices| choices.next_cursor())
                    .is_some()
                && let Some(scroll) = form.selector.read(cx).menu_scroll_handle(cx)
            {
                let offset = scroll.offset().y;
                if offset < px(0.)
                    && scroll.max_offset().y + offset <= px(56.)
                    && form.last_page_offset != Some(offset)
                {
                    form.last_page_offset = Some(offset);
                    more = true;
                }
            }
        }
        if more {
            self.load_branch_choices(true);
        }
    }
    pub(super) fn poll_upstream_search(&mut self) -> bool {
        if self.page != Page::Projects || self.git_mutation_blocks_write() {
            return false;
        }
        let Some(form) = self.git.branch_manage.as_mut() else {
            return false;
        };
        if form.kind != Kind::Upstream || form.needs_review {
            return false;
        }
        let changed = form.update_query();
        if changed {
            self.resources.invalidate(CHOICES);
            self.resources.invalidate(MORE);
        }
        if form
            .search_pending
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(200))
        {
            form.search_pending = None;
            self.load_branch_choices(false);
            return true;
        }
        changed
    }
    fn load_branch_choices(&mut self, more: bool) {
        let Some(form) = self.git.branch_manage.as_mut() else {
            return;
        };
        if form.kind != Kind::Upstream || form.needs_review || form.cursor.is_some() {
            return;
        }
        if form.search_pending.is_some() || form.query.len() > 1024 || form.query.contains('\0') {
            return;
        }
        let mut params = json!({"repoId":form.review.repo_id(),"branchKind":"all","pageSize":100,"filter":form.query});
        if more {
            let Some(cursor) = form
                .choices
                .as_ref()
                .and_then(|choices| choices.next_cursor())
                .map(str::to_owned)
            else {
                return;
            };
            params["cursor"] = json!(cursor);
            form.cursor = Some(cursor);
        } else {
            form.choices = None;
            form.last_page_offset = None;
            form.selector_dirty = true;
            self.resources.invalidate(CHOICES);
            self.resources.invalidate(MORE);
        }
        form.error = None;
        let key = if more { MORE } else { CHOICES };
        self.request(
            key,
            "git_request",
            json!({"serverId":self.server,"request":{"method":"repo.branches","params":params}}),
        );
        // A full/disconnected worker queue settles synchronously and produces
        // no worker reply. Release pagination and expose Retry in that case too.
        if self
            .resources
            .get(key)
            .is_some_and(|r| !r.pending && r.error.is_some())
        {
            self.branch_choices_completed(key);
        }
    }
    pub(super) fn branch_choices_completed(&mut self, key: &str) {
        let data = self.value(key);
        let error = self
            .resources
            .get(key)
            .and_then(|resource| resource.error.clone());
        let Some(form) = self.git.branch_manage.as_mut() else {
            return;
        };
        form.settle_choices(key, error.map_or(Ok(data), Err));
    }
    fn submit_branch_manage(&mut self, cx: &mut Context<Self>) {
        if !self.git_can_write_branch() {
            return;
        }
        let Some(form) = &self.git.branch_manage else {
            return;
        };
        if !form.review.matches(
            &self.git_repository_id(),
            &text(&self.value("git-branch-status"), "snapshot"),
        ) {
            return;
        }
        let Ok(action) = form.action() else {
            return;
        };
        let draft = form.draft();
        let snapshot = form.review.snapshot().to_owned();
        let title = format!(
            "{} {}",
            if form.kind == Kind::Delete {
                "Delete branch"
            } else {
                "Set upstream for"
            },
            form.review.name()
        );
        self.start_git_mutation(
            self.git_repository_id(),
            title,
            move |backend, server, repo, id| {
                backend.apply_reviewed_action(server, repo, &snapshot, action, id)
            },
            cx,
        );
        if let Some(state) = self.git_mutation.as_mut() {
            state.branch_management = Some(draft);
        }
    }
    pub(super) fn branch_manage_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.git.branch_manage else {
            return div().into_any_element();
        };
        let busy = self.git_mutation_blocks_write();
        let stale = form.needs_review
            || !form.review.matches(
                &self.git_repository_id(),
                &text(&self.value("git-branch-status"), "snapshot"),
            );
        let ready = self.git_can_write_branch() && !stale && form.action().is_ok();
        let loading = [CHOICES, MORE]
            .iter()
            .any(|key| self.resources.get(key).is_some_and(|r| r.pending));
        // Keep an open selector mounted while a search replaces its results.
        let initial_loading =
            loading && form.choices.is_none() && !form.selector.read(cx).is_open();
        div().id("branch-review").flex().flex_col().gap_2().px_4().py_3().border_b_1().border_color(cx.theme().border)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} · {}", if form.kind == Kind::Delete { "Delete local branch" } else { "Upstream for" }, form.review.name())))
            .child(label(format!("Reviewed commit {}", form.review.oid())).text_xs())
            .when(form.kind == Kind::Delete, |view| view
                .child(label("Git refuses branches with unmerged commits or branches checked out in a worktree. Remote branches are kept."))
                .child(crate::controls::checkbox("branch-delete-force").label("Delete even if it contains unmerged commits").checked(form.force).disabled(busy || stale)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        if this.git_mutation_blocks_write() { return; }
                        if let Some(form) = this.git.branch_manage.as_mut() && !form.needs_review { form.force = *checked; cx.notify(); }
                    })))
                .when(form.force, |view| view.child(div().text_color(cx.theme().danger).child("Commits reachable only from this branch become unreferenced. A worktree using the branch still blocks deletion."))))
            .when(form.kind == Kind::Upstream, |view| view
                .child(label(format!("Current upstream: {}", form.review.upstream().unwrap_or("None"))))
                .child(div().w_full().max_w(px(560.))
                    .when(initial_loading, |view| view.child(project_loading::selector("upstream branches")))
                    .when(!initial_loading, |view| view.child(crate::controls::select(&form.selector).accessibility_label("Upstream branch").disabled(busy || stale))))
                .child(label("Only stored branches are listed. Fetch to discover newer remote branches.").text_xs())
                .when(form.error.is_some(), |view| view.child(crate::controls::button("branch-upstreams-retry").label("Reload upstream branches").disabled(busy || stale || loading).on_click(cx.listener(|this, _, _, cx| { this.load_branch_choices(false); cx.notify(); }))))
                .when(!form.upstream.is_empty() && form.action().is_err() && !stale, |view| view.child(label("The selected upstream is not loaded or is unavailable. Scroll the branch list to load more, or choose another."))))
            .when_some(form.error.clone(), |view, error| view.child(div().text_color(cx.theme().danger).child(error)))
            .when(stale, |view| view.child(div().text_color(cx.theme().danger).child("The repository changed. Cancel, refresh, and select the branch again.")))
            .child(project_dialog::actions()
                .child(crate::controls::button("branch-manage-cancel").label("Cancel").disabled(busy).on_click(cx.listener(|this, _, _, cx| {
                    this.git.branch_manage = None; this.resources.invalidate(CHOICES); this.resources.invalidate(MORE); cx.notify();
                })))
                .child(crate::controls::button("branch-manage-submit").loading(self.git_write_pending()).label(if form.kind == Kind::Delete { "Delete local branch" } else { "Save upstream" }).when(form.kind == Kind::Delete, |button| button.danger()).disabled(!ready)
                    .on_click(cx.listener(|this, _, _, cx| this.submit_branch_manage(cx)))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{CHOICES, Choices, Form, Kind, MORE};
    use base64::Engine as _;
    use gpui::component::select::{SelectItem as _, SelectState};
    use gpui::{App, AppContext as _, TestAppContext, Window};
    use gpui_kit as gpui;
    use newport_client::git_branch::{BranchChoices, BranchReview};
    use serde_json::{Value, json};
    use std::{cell::RefCell, rc::Rc};
    fn path(value: &str) -> Value {
        json!({"display":value,"bytesB64":base64::engine::general_purpose::STANDARD.encode(value)})
    }
    fn entry(name: &str, remote: bool) -> Value {
        json!({"name":path(name),"reference":path(&format!("refs/{}/{name}", if remote { "remotes" } else { "heads" })),
            "oid":{"algorithm":"sha1","hex":"a".repeat(40)},"current":false,"remote":remote,
            "upstream":path("refs/remotes/origin/late"),"tracking":{"editable":true,"token":"reviewed"}})
    }
    fn fixture(kind: Kind, window: &mut Window, cx: &mut App) -> Form {
        let query_input = Rc::<RefCell<String>>::default();
        Form {
            id: uuid::Uuid::new_v4(),
            kind,
            review: BranchReview::from_entry("repo", "status", &entry("topic", false)).unwrap(),
            force: false,
            upstream: "refs/remotes/origin/late".into(),
            choices: None,
            cursor: None,
            error: None,
            selector: cx.new(|cx| {
                SelectState::new(
                    Choices {
                        items: vec![],
                        query: query_input.clone(),
                    },
                    None,
                    window,
                    cx,
                )
            }),
            query_input,
            query: String::new(),
            search_pending: None,
            selected_target: None,
            selector_dirty: true,
            last_page_offset: None,
            needs_review: false,
            _subscriptions: vec![],
        }
    }
    #[gpui::test]
    fn upstream_search_preserves_target_and_resets_pagination(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.add_empty_window().update(|window, cx| {
            let mut form = fixture(Kind::Upstream, window, cx);
            form.settle_choices(CHOICES, Ok(json!({"snapshot":"branches","metadata":{},"entries":[entry("origin/late",true)],"nextCursor":"next"})));
            form.cursor = Some("next".into());
            *form.query_input.borrow_mut() = "other".into();
            assert!(form.update_query());
            assert!(form.choices.is_none());
            assert!(form.cursor.is_none());
            assert!(form.search_pending.is_some());
            assert_eq!(form.action().unwrap()["upstream"], "refs/remotes/origin/late");
            let selected = &form.items()[1];
            assert_eq!(selected.reference, "refs/remotes/origin/late");
            assert_eq!(selected.title.as_ref(), "origin/late · remote");
            assert!(!selected.disabled());
            assert!(!form.update_query(), "an unchanged query must not restart loading");
            form.settle_choices(CHOICES, Ok(json!({"snapshot":"filtered","metadata":{},"entries":[entry("origin/other",true)],"nextCursor":null})));
            assert_eq!(form.action().unwrap()["upstream"], "refs/remotes/origin/late");
            *form.query_input.borrow_mut() = "bad\0query".into();
            assert!(form.update_query());
            assert!(form.error.is_some());
            assert!(form.search_pending.is_none());
        });
    }
    #[gpui::test]
    fn upstream_selection_survives_loading_and_later_pages(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.add_empty_window().update(|window, cx| {
            let mut form = fixture(Kind::Upstream, window, cx);
            form.sync_selector(window, cx);
            assert!(form.action().is_err());
            assert!(form.items().last().unwrap().disabled());
            assert_eq!(form.selector.read(cx).selected_value(), Some(&form.upstream));
            form.choices = Some(BranchChoices::from_page(&json!({"snapshot":"branches","metadata":{},"entries":[entry("origin/main",true)],"nextCursor":"next"})).unwrap());
            form.selector_dirty = true;
            form.sync_selector(window, cx);
            assert_eq!(form.selector.read(cx).selected_value(), Some(&form.upstream));
            assert!(form.action().is_err(), "the unloaded upstream must not become None");
            form.choices.as_mut().unwrap().append(&json!({"snapshot":"branches","metadata":{},"entries":[entry("origin/late",true)],"nextCursor":null}),"next").unwrap();
            form.selector_dirty = true;
            form.sync_selector(window, cx);
            assert_eq!(form.selector.read(cx).selected_value(), Some(&form.upstream));
            assert_eq!(form.action().unwrap()["upstream"], "refs/remotes/origin/late");
            assert!(!form.items().last().unwrap().disabled());
            form.upstream.clear();
            assert!(form.action().unwrap()["upstream"].is_null());
        });
    }
    #[gpui::test]
    fn completed_write_does_not_close_a_replacement_or_changed_review(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.add_empty_window().update(|window, cx| {
            let mut form = fixture(Kind::Delete, window, cx);
            let draft = form.draft();
            assert_eq!(form.action().unwrap()["force"], false);
            form.force = true;
            assert!(!form.accept_outcome(&draft, true));
            assert!(!form.needs_review);
            let newer = form.draft();
            assert!(form.accept_outcome(&newer, true));
            form.id = uuid::Uuid::new_v4();
            assert!(!form.accept_outcome(&newer, false));
            assert!(!form.needs_review);
        });
    }
    #[gpui::test]
    fn failure_preserves_values_and_requires_new_review(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.add_empty_window().update(|window, cx| {
            let mut form = fixture(Kind::Delete, window, cx);
            form.force = true;
            let draft = form.draft();
            assert!(!form.accept_outcome(&draft, false));
            assert!(form.needs_review);
            assert_eq!(form.draft(), draft);
            assert!(form.action().is_err());
            assert_eq!(form.review.name(), "topic");
        });
    }
    #[gpui::test]
    fn rejected_queue_and_page_errors_release_pagination_for_retry(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.add_empty_window().update(|window, cx| {
            let mut form = fixture(Kind::Upstream, window, cx);
            form.settle_choices(CHOICES, Err("Queue is full".into()));
            assert_eq!(form.error.as_deref(), Some("Queue is full"));
            assert!(form.cursor.is_none());
            form.settle_choices(CHOICES, Ok(json!({"snapshot":"branches","metadata":{},"entries":[entry("origin/main",true)],"nextCursor":"next"})));
            form.cursor = Some("next".into());
            form.settle_choices(MORE, Err("Queue is full".into()));
            assert!(form.cursor.is_none(), "a rejected dispatch must permit another page request");
            assert_eq!(form.choices.as_ref().unwrap().next_cursor(), Some("next"));
            form.cursor = Some("next".into());
            form.settle_choices(MORE, Ok(json!({"snapshot":"branches","metadata":{},"entries":[entry("origin/late",true)],"nextCursor":null})));
            assert!(form.error.is_none());
            assert!(form.action().is_ok());
        });
    }
}
