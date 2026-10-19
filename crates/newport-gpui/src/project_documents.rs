use super::*;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ReviewKey {
    File { path: String, staged: bool },
    Commit(String),
}

// A row's GPUI state follows its document when filtering, regrouping or paging
// changes its ordinal. Index-side identity also separates partially staged files.
fn navigation_identity(key: &ReviewKey) -> String {
    match key {
        ReviewKey::File { path, staged } => format!(
            "file-{}-{path}",
            if *staged { "staged" } else { "unstaged" }
        ),
        ReviewKey::Commit(oid) => format!("commit-{oid}"),
    }
}
#[derive(Clone)]
pub(super) struct ReviewDocument {
    pub key: ReviewKey,
    pub entry: Value,
    pub status: Arc<Value>,
    pub preview: bool,
}
impl ReviewDocument {
    pub fn title(&self) -> String {
        match &self.key {
            ReviewKey::File { path, staged } => format!(
                "{} · {}",
                path.rsplit('/').next().unwrap_or(path),
                if *staged { "Staged" } else { "Diff" }
            ),
            ReviewKey::Commit(oid) => format!("Commit {}", oid.chars().take(7).collect::<String>()),
        }
    }
}
#[derive(Default)]
pub(super) struct Documents {
    pub reviews: Vec<ReviewDocument>,
    pub active: Option<ReviewKey>,
}
impl Documents {
    pub fn open(&mut self, document: ReviewDocument) {
        if let Some(existing) = self
            .reviews
            .iter_mut()
            .find(|item| item.key == document.key)
        {
            existing.entry = document.entry;
            existing.status = document.status;
            existing.preview &= document.preview;
            self.active = Some(existing.key.clone());
            return;
        }
        // Only an unkept preview may be replaced. Kept documents retain identity.
        if document.preview {
            self.reviews.retain(|item| !item.preview);
        }
        self.active = Some(document.key.clone());
        self.reviews.push(document);
    }
    /// Follow a working comparison when staging moves it to the other index side.
    pub fn reconcile_file(&mut self, entry: Value, status: Value, staged: bool) -> bool {
        let status = Arc::new(status);
        let Some(ReviewKey::File { path, .. }) = self.active.clone() else {
            return false;
        };
        if text(&entry["path"], "display") != path {
            return false;
        }
        let Some(index) = self
            .reviews
            .iter()
            .position(|item| Some(&item.key) == self.active.as_ref())
        else {
            return false;
        };
        let key = ReviewKey::File { path, staged };
        if self.reviews[index].key == key
            && self.reviews[index].entry == entry
            && self.reviews[index].status == status
        {
            return false;
        }
        let old = self.reviews.remove(index);
        let preview = old.preview
            && self
                .reviews
                .iter()
                .find(|item| item.key == key)
                .is_none_or(|item| item.preview);
        if let Some(existing) = self.reviews.iter_mut().find(|item| item.key == key) {
            existing.entry = entry;
            existing.status = status;
            existing.preview = preview;
        } else {
            self.reviews.insert(
                index.min(self.reviews.len()),
                ReviewDocument {
                    key: key.clone(),
                    entry,
                    status,
                    preview,
                },
            );
        }
        self.active = Some(key);
        true
    }

    pub fn keep(&mut self, key: &ReviewKey) {
        if let Some(item) = self.reviews.iter_mut().find(|item| &item.key == key) {
            item.preview = false;
        }
    }
    pub fn close(&mut self, key: &ReviewKey) -> Option<ReviewDocument> {
        let index = self.reviews.iter().position(|item| &item.key == key)?;
        self.reviews.remove(index);
        if self.active.as_ref() != Some(key) {
            return None;
        }
        let next = self
            .reviews
            .get(index.min(self.reviews.len().saturating_sub(1)))
            .cloned();
        self.active = next.as_ref().map(|item| item.key.clone());
        next
    }
}

#[derive(Clone)]
pub(super) enum Event {
    Open(ReviewDocument),
    CloseRepository,
    Stage {
        snapshot: String,
        entries: Vec<Value>,
        stage: bool,
    },
}
impl EventEmitter<Event> for project_editor::CodeWorkspace {}

#[derive(Default)]
pub(super) struct Collection {
    // Replace through set_page so the immutable snapshot and row indices agree.
    page: Arc<Value>,
    groups: [Arc<[usize]>; 4],
    pub loading: bool,
    pub error: Option<String>,
    pub scroll: UniformListScrollHandle,
    pub read_at: Option<Instant>,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChangeGroup {
    Conflicts,
    Staged,
    Unstaged,
    Untracked,
}
impl ChangeGroup {
    const ALL: [Self; 4] = [
        Self::Conflicts,
        Self::Staged,
        Self::Unstaged,
        Self::Untracked,
    ];
    fn title(self) -> &'static str {
        match self {
            Self::Conflicts => "Conflicts",
            Self::Staged => "Staged",
            Self::Unstaged => "Unstaged",
            Self::Untracked => "Untracked",
        }
    }
    fn matches(self, entry: &Value) -> bool {
        if entry["conflicted"] == true {
            return self == Self::Conflicts;
        }
        match self {
            Self::Conflicts => false,
            Self::Staged => entry["staged"] == true,
            Self::Unstaged => entry["unstaged"] == true && entry["untracked"] != true,
            Self::Untracked => entry["untracked"] == true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavigationRow {
    Group(ChangeGroup),
    Entry { index: usize, staged: bool },
}

/// A cheap projection of the retained groups, not a copied list of every row.
/// Resolving a visible row visits at most four groups, irrespective of file count.
struct NavigationRows {
    groups: [Arc<[usize]>; 4],
    collapsed: [bool; 4],
    history_count: Option<usize>,
}
impl NavigationRows {
    fn len(&self) -> usize {
        self.history_count.unwrap_or_else(|| {
            self.groups
                .iter()
                .enumerate()
                .map(|(i, group)| {
                    usize::from(!group.is_empty()) + if self.collapsed[i] { 0 } else { group.len() }
                })
                .sum()
        })
    }
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn has_entries(&self) -> bool {
        self.history_count.map_or_else(
            || {
                self.groups
                    .iter()
                    .enumerate()
                    .any(|(i, group)| !group.is_empty() && !self.collapsed[i])
            },
            |count| count > 0,
        )
    }
    fn row(&self, mut ordinal: usize) -> Option<NavigationRow> {
        if let Some(count) = self.history_count {
            return (ordinal < count).then_some(NavigationRow::Entry {
                index: ordinal,
                staged: false,
            });
        }
        for group in ChangeGroup::ALL {
            let entries = &self.groups[group as usize];
            if entries.is_empty() {
                continue;
            }
            if ordinal == 0 {
                return Some(NavigationRow::Group(group));
            }
            ordinal -= 1;
            if self.collapsed[group as usize] {
                continue;
            }
            if let Some(&index) = entries.get(ordinal) {
                return Some(NavigationRow::Entry {
                    index,
                    staged: group == ChangeGroup::Staged,
                });
            }
            ordinal -= entries.len();
        }
        None
    }
}

impl Collection {
    pub fn page(&self) -> &Arc<Value> {
        &self.page
    }
    pub fn set_page(&mut self, page: Value) {
        self.page = Arc::new(page);
        self.groups = std::array::from_fn(|group| {
            self.entries()
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    ChangeGroup::ALL[group].matches(entry).then_some(index)
                })
                .collect::<Vec<_>>()
                .into()
        });
    }
    fn entries(&self) -> &[Value] {
        self.page["entries"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
    fn display_rows(
        &self,
        history: bool,
        collapsed: &std::collections::HashSet<String>,
    ) -> NavigationRows {
        NavigationRows {
            groups: self.groups.clone(),
            collapsed: std::array::from_fn(|i| collapsed.contains(ChangeGroup::ALL[i].title())),
            history_count: history.then(|| self.entries().len()),
        }
    }
}
#[derive(Default)]
pub(super) struct Navigator {
    pub repo: String,
    pub panel: usize,
    pub collections: [Collection; 2],
    pub documents: Documents,
    pub writable: bool,
    pub invalidated: bool,
    pub filter: Option<Entity<InputState>>,
    pub filter_subscription: Option<Subscription>,
    pub query: String,
    pub filter_changed: Option<Instant>,
    pub collapsed: std::collections::HashSet<String>,
}

impl Navigator {
    pub fn invalidate(&mut self) {
        self.invalidated = true;
        for collection in &mut self.collections {
            collection.read_at = None;
            collection.generation += 1;
            collection.loading = false;
        }
    }
    pub fn can_stage(&self, snapshot: &str, entries: &[Value]) -> bool {
        let status = &self.collections[0];
        let ready = self.writable
            && !self.invalidated
            && self.filter_changed.is_none()
            && !status.loading
            && status.error.is_none()
            && !snapshot.is_empty()
            && status.page["snapshot"].as_str() == Some(snapshot)
            && !entries.is_empty();
        if !ready {
            return false;
        }
        let current: std::collections::HashMap<_, _> = status
            .entries()
            .iter()
            .filter_map(|entry| entry["entryId"].as_str().map(|id| (id, entry)))
            .collect();
        entries.iter().all(|entry| {
            entry["conflicted"] != true
                && entry["entryId"].as_str().is_some_and(|id| {
                    !id.is_empty() && current.get(id).is_some_and(|value| *value == entry)
                })
        })
    }
}

fn change_mark(entry: &Value, staged: bool) -> &'static str {
    let flags = entry["flags"].as_u64().unwrap_or(0);
    if entry["conflicted"] == true {
        "!"
    } else if entry["untracked"] == true {
        "?"
    } else if flags & if staged { 4 } else { 512 } != 0 {
        "D"
    } else if flags & if staged { 8 } else { 2048 } != 0 {
        "R"
    } else if flags & if staged { 16 } else { 1024 } != 0 {
        "T"
    } else if staged && flags & 1 != 0 {
        "A"
    } else {
        "M"
    }
}

impl project_editor::CodeWorkspace {
    pub(super) fn navigate(&mut self, panel: usize, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        if panel == 1 && self.navigator.filter.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find a changed file"));
            self.navigator.filter_subscription = Some(cx.observe(&input, |this, input, cx| {
                let query = input.read(cx).value().trim().to_owned();
                if this.navigator.query != query {
                    this.navigator.query = query;
                    this.navigator.filter_changed = Some(Instant::now());
                    this.navigator.collections[0].generation += 1;
                    this.navigator.collections[0].loading = false;
                    this.navigator.collections[0].read_at = None;
                    cx.notify();
                }
            }));
            self.navigator.filter = Some(input);
        }
        self.files = panel == 0;
        self.navigator.panel = panel;
        self.show_tree = true;
        self.reveal_active(false, window, cx);
        if panel == 0 {
            self.focus_tree(window, cx);
        }
        if matches!(panel, 1 | 2) && self.navigator.collections[panel - 1].page.is_null() {
            self.load_collection(panel - 1, false, window, cx);
        }
        cx.notify();
    }
    pub(super) fn load_collection(
        &mut self,
        index: usize,
        more: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.available
            || self.navigator.repo.is_empty()
            || self.navigator.collections[index].loading
        {
            return;
        }
        let previous = self.navigator.collections[index].page.clone();
        let cursor = more
            .then(|| previous["nextCursor"].as_str().map(str::to_owned))
            .flatten();
        if more && cursor.is_none() {
            return;
        }
        let repo = self.navigator.repo.clone();
        let mut params = json!({"repoId":repo,"pageSize":100});
        if index == 0 {
            params["filter"] = json!({"text":self.navigator.query,"group":"all"});
            self.navigator.filter_changed = None;
        }
        if index == 1 {
            params["messageBytes"] = json!(512);
        }
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        self.navigator.collections[index].generation += 1;
        let generation = self.navigator.collections[index].generation;
        self.navigator.collections[index].loading = true;
        self.navigator.collections[index].error = None;
        self.rpc("git_request", json!({"serverId":self.server,"request":{"method":if index == 0 {"repo.status"} else {"repo.history"},"params":params}}), window, cx, move |this, result, _, _| {
            if this.navigator.repo != repo || this.navigator.collections[index].generation != generation { return; }
            let collection = &mut this.navigator.collections[index];
            collection.loading = false;
            collection.read_at = Some(Instant::now());
            let result = result.and_then(|page| {
                if let Some(cursor) = cursor { newport_client::git_page::merge(&previous, page, &cursor) }
                else { newport_client::git_page::validate(&page).map(|_| page) }
            });
            match result {
                Ok(page) => { collection.set_page(page); collection.error = None; }
                Err(error) => collection.error = Some(error),
            }
        });
        cx.notify();
    }
    pub(super) fn review_document(&mut self, document: ReviewDocument, cx: &mut Context<Self>) {
        self.navigator.documents.open(document.clone());
        if self.navigator.documents.active.as_ref() != Some(&document.key) {
            return;
        }
        self.show_review(cx);
        cx.emit(Event::Open(document));
    }
    pub(super) fn close_review(&mut self, key: &ReviewKey, cx: &mut Context<Self>) {
        let active = self.navigator.documents.active.as_ref() == Some(key) && self.reviewing;
        let next = self.navigator.documents.close(key);
        if active {
            if let Some(document) = next {
                cx.emit(Event::Open(document));
            } else {
                self.reviewing = false;
            }
        }
        cx.notify();
    }
    pub(super) fn collection_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let index = self.navigator.panel.saturating_sub(1).min(1);
        let collection = &self.navigator.collections[index];
        let display = collection.display_rows(index == 1, &self.navigator.collapsed);
        let snapshot = text(&collection.page, "snapshot");
        let writable = self.navigator.writable
            && !collection.loading
            && collection.error.is_none()
            && self.navigator.filter_changed.is_none()
            && !self.navigator.invalidated;
        let status = collection.page.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .when(index == 0, |view| {
                view.when_some(self.navigator.filter.as_ref(), |view, input| {
                    view.child(
                        div().p_2().child(
                            crate::controls::search_input(input, cx).aria_label("Find a changed file"),
                        ),
                    )
                })
            })
            .when_some(collection.error.clone(), |view, error| {
                view.child(
                    crate::controls::alert(("project-nav-error", index), error, cx)
                        .px_3()
                        .py_2(),
                )
            })
            .when(
                display.is_empty() && !collection.loading && collection.error.is_none(),
                |view| {
                    view.child(empty::compact(
                        if index == 0 {
                            "No changed files"
                        } else {
                            "No commits"
                        },
                        cx,
                    ))
                },
            )
            .child(
                uniform_list(
                    ("project-git-navigation", index),
                    display.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        // Prefetch just before the visible end. Defer out of layout;
                        // generation + cursor checks reject obsolete queued demand.
                        let collection = &this.navigator.collections[index];
                        if range.end.saturating_add(8) >= display.len()
                            && display.has_entries()
                            && !collection.loading && collection.error.is_none()
                            && !this.navigator.invalidated && this.navigator.filter_changed.is_none()
                            && let Some(cursor) = collection.page["nextCursor"].as_str().map(str::to_owned)
                        {
                            let generation = collection.generation;
                            cx.defer_in(window, move |this, window, cx| {
                                let collection = &this.navigator.collections[index];
                                if this.navigator.panel == index + 1
                                    && collection.generation == generation
                                    && collection.page["nextCursor"].as_str() == Some(cursor.as_str())
                                    && !collection.loading && collection.error.is_none()
                                    && !this.navigator.invalidated && this.navigator.filter_changed.is_none()
                                {
                                    this.load_collection(index, true, window, cx);
                                }
                            });
                        }
                        range
                            .map(|i| {
                                let Some(row) = display.row(i) else { return div().into_any_element(); };
                                if let NavigationRow::Group(group) = row {
                                    let staged = group == ChangeGroup::Staged;
                                    let group_page = status.clone();
                                    let count = display.groups[group as usize].len();
                                    let snapshot = snapshot.clone();
                                    let title = group.title();
                                    let collapsed = this.navigator.collapsed.contains(title);
                                    let toggle_group = title.to_owned();
                                    return div()
                                        .id(SharedString::from(format!("project-nav-group-row-{title}")))
                                        .w_full().min_w_0().h(px(crate::ui::ROW_HEIGHT)).pr(px(8.))
                                        .flex().items_center()
                                        .child(crate::controls::group_heading(SharedString::from(format!("project-nav-group-{title}")), title, count, !collapsed, cx)
                                            .flex_1().min_w_0()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if !this.navigator.collapsed.remove(&toggle_group) {
                                                    this.navigator.collapsed.insert(toggle_group.clone());
                                                }
                                                cx.notify();
                                            })))
                                        .child(div().w(px(24.)).flex_shrink_0().flex().items_center().justify_center()
                                            .when(group != ChangeGroup::Conflicts, |slot| slot.child(
                                                crate::controls::checkbox(SharedString::from(format!("project-stage-shown-{title}")))
                                                    .checked(staged)
                                                    .accessibility_label(format!("{} {}", if staged { "Unstage shown" } else { "Stage shown" }, title))
                                                    .disabled(!writable || count == 0)
                                                    .on_click(cx.listener(move |_, checked: &bool, _, cx| {
                                                        let entries = group_page["entries"].as_array().into_iter().flatten()
                                                            .filter(|entry| group.matches(entry)).cloned().collect();
                                                        cx.emit(Event::Stage { snapshot: snapshot.clone(), entries, stage: *checked });
                                                    }))
                                            )))
                                        .into_any_element();
                                }
                                let NavigationRow::Entry { index: entry_index, staged } = row else { unreachable!() };
                                let entry = &status["entries"][entry_index];
                                let title = if index == 0 { text(&entry["path"], "display") } else { git_commit_header::history_subject(entry) };
                                let key = if index == 0 {
                                    ReviewKey::File {
                                        path: title.clone(),
                                        staged,
                                    }
                                } else {
                                    ReviewKey::Commit(text(&entry["oid"], "hex"))
                                };
                                let selected = this.reviewing
                                    && this.navigator.documents.active.as_ref() == Some(&key);
                                let identity = navigation_identity(&key);
                                let document = ReviewDocument {
                                    key,
                                    entry: entry.clone(),
                                    status: status.clone(),
                                    preview: true,
                                };
                                let mut row = div()
                                    .id(SharedString::from(format!("project-nav-row-{identity}")))
                                    .w_full()
                                    .overflow_hidden()
                                    .h(px(if index == 0 { crate::ui::ROW_HEIGHT } else { crate::ui::TWO_LINE_ROW_HEIGHT }))
                                    .when(index == 0, |row| row.pr(px(8.)).when(!selected, |row| row.hover(|style| style.bg(theme::row_hover(cx)))))
                                    .flex()
                                    .items_center()
                                    .min_w_0()
                                    .child(
                                        crate::controls::compact_button(SharedString::from(format!("project-nav-entry-{identity}")))
                                            .ghost()
                                            .h_full()
                                            .flex_1()
                                            .min_w_0()
                                            .justify_start()
                                            .selected(selected)
                                            .custom(git_file_row::selection_style(selected, cx))
                                            .when(index == 0, |button| {
                                                button.h(px(28.)).min_h(px(28.)).max_h(px(28.))
                                                    .pl(px(12.)).pr(px(4.)).rounded_none().border_0()
                                                    .font_weight(FontWeight::NORMAL)
                                                    .custom(gpui_kit::component::button::ButtonCustomVariant::new(cx)
                                                        .color(cx.theme().transparent)
                                                        .hover(cx.theme().transparent).active(cx.theme().transparent))
                                            })
                                            .when(index == 1, |button| {
                                                let selection = theme::selected_fill(cx.theme().is_dark());
                                                button
                                                    .h(px(crate::ui::TWO_LINE_ROW_HEIGHT))
                                                    .min_h(px(crate::ui::TWO_LINE_ROW_HEIGHT))
                                                    .max_h(px(crate::ui::TWO_LINE_ROW_HEIGHT))
                                                    .px(px(12.))
                                                    .rounded_none()
                                                    .border_0()
                                                    .font_weight(FontWeight::NORMAL)
                                                    .custom(gpui_kit::component::button::ButtonCustomVariant::new(cx)
                                                        .color(if selected { selection } else { cx.theme().transparent })
                                                        .hover(if selected { selection } else { theme::row_hover(cx) })
                                                        .active(selection))
                                            })
                                            .accessibility_label(if index == 0 {
                                                format!("Open diff {title}")
                                            } else {
                                                format!(
                                                    "Open commit {}",
                                                    text(&entry["oid"], "hex")
                                                )
                                            })
                                            .when(index == 0, |button| button.child(
                                                div().debug_selector(move || format!("project-nav-content-{i}"))
                                                    .flex().items_center().gap(px(7.)).min_w_0().w_full().text_size(px(crate::ui::FONT_SIZE))
                                                    .child(git_file_row::badge(change_mark(entry, staged), cx))
                                                    .child(git_file_row::filename_first(&title, cx))
                                            ))
                                            .when(index == 1, |button| button
                                                .child(
                                                div()
                                                    .debug_selector(move || format!("project-nav-content-{i}"))
                                                    .flex()
                                                    .flex_col()
                                                    .min_w_0()
                                                    .w_full()
                                                    .child(
                                                        div()
                                                            .truncate()
                                                            .text_size(px(crate::ui::FONT_SIZE))
                                                            .line_height(px(17.))
                                                            .child(title),
                                                    )
                                                    .when(index == 1, |view| {
                                                        view.child(
                                                            git_commit_header::history_row_identity(
                                                                entry, cx,
                                                            ),
                                                        )
                                                    }),
                                            )
                                            )
                                            .on_click(cx.listener(
                                                move |this, event: &ClickEvent, _, cx| {
                                                    let mut document = document.clone();
                                                    document.preview = event.click_count() < 2;
                                                    this.review_document(document, cx);
                                                },
                                            )),
                                    );
                                if index == 0 {
                                    let entry = entry.clone();
                                    let snapshot = snapshot.clone();
                                    let name = text(&entry["path"], "display");
                                    let tint = theme::selected_fill(cx.theme().is_dark());
                                    row = row.when(selected, |row| row.bg(tint)).child(
                                        div().w(px(24.)).h_full().flex_shrink_0().flex().items_center().justify_center()
                                            .when(entry["conflicted"] != true, |slot| slot.child(
                                                crate::controls::checkbox(SharedString::from(format!("project-nav-stage-{identity}")))
                                                    .checked(staged)
                                                    .accessibility_label(format!("{} {name}", if staged { "Unstage" } else { "Stage" }))
                                                    .disabled(!writable)
                                                    .on_click(cx.listener(move |_, checked: &bool, _, cx| {
                                                        cx.emit(Event::Stage { snapshot: snapshot.clone(), entries: vec![entry.clone()], stage: *checked });
                                                    }))
                                            ))
                                    );
                                }
                                row.into_any_element()
                            })
                            .collect()
                    }),
                )
                .track_scroll(&collection.scroll)
                .w_full()
                .min_w_0()
                .flex_1()
                .min_h_0(),
            )
            .when(collection.page["metadata"]["truncated"] == true, |view| {
                view.child(crate::label("Listing incomplete").px_2().text_size(px(crate::ui::FONT_SIZE)))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChangeGroup, Collection, Documents, NavigationRow, Navigator, ReviewDocument, ReviewKey,
    };
    use serde_json::json;
    use std::time::Instant;
    fn visible_rows(
        collection: &Collection,
        collapsed: &std::collections::HashSet<String>,
    ) -> Vec<NavigationRow> {
        let display = collection.display_rows(false, collapsed);
        (0..display.len())
            .map(|i| display.row(i).unwrap())
            .collect()
    }
    #[test]
    fn snapshot_groups_handle_conflicts_partial_staging_and_collapsing() {
        let mut collection = Collection::default();
        collection.set_page(json!({"snapshot":"first","entries":[
            {"path":{"display":"conflict"},"conflicted":true,"staged":true,"unstaged":true},
            {"path":{"display":"partial"},"staged":true,"unstaged":true},
            {"path":{"display":"new"},"untracked":true,"unstaged":true}
        ]}));
        use ChangeGroup::*;
        use NavigationRow::{Entry, Group};
        assert_eq!(
            visible_rows(&collection, &Default::default()),
            vec![
                Group(Conflicts),
                Entry {
                    index: 0,
                    staged: false
                },
                Group(Staged),
                Entry {
                    index: 1,
                    staged: true
                },
                Group(Unstaged),
                Entry {
                    index: 1,
                    staged: false
                },
                Group(Untracked),
                Entry {
                    index: 2,
                    staged: false
                },
            ]
        );
        assert_eq!(
            visible_rows(
                &collection,
                &std::collections::HashSet::from(["Untracked".into(), "Staged".into()])
            ),
            vec![
                Group(Conflicts),
                Entry {
                    index: 0,
                    staged: false
                },
                Group(Staged),
                Group(Unstaged),
                Entry {
                    index: 1,
                    staged: false
                },
                Group(Untracked),
            ]
        );
        collection.set_page(json!({"snapshot":"second","entries":[]}));
        assert!(
            collection
                .display_rows(false, &Default::default())
                .is_empty(),
            "replacement must drop obsolete indices"
        );
    }

    #[test]
    fn retained_documents_share_a_snapshot_and_survive_collection_replacement() {
        let mut collection = Collection::default();
        collection.set_page(json!({"snapshot":"first","entries":[{"path":{"display":"first"}}]}));
        let mut document = file("first", false);
        document.status = collection.page().clone();
        let clone = document.clone();
        assert!(std::sync::Arc::ptr_eq(&document.status, &clone.status));
        assert!(std::sync::Arc::ptr_eq(&document.status, collection.page()));
        collection.set_page(json!({"snapshot":"second","entries":[]}));
        assert_eq!(document.status["snapshot"], "first");
        assert_eq!(collection.page()["snapshot"], "second");
    }

    #[test]
    fn large_list_projection_shares_indices_and_resolves_only_requested_rows() {
        let mut collection = Collection::default();
        let entries: Vec<_> = (0..10_000)
            .map(|i| json!({"path":{"display":format!("file-{i}")},"untracked":true}))
            .collect();
        collection.set_page(json!({"entries":entries}));
        let display = collection.display_rows(false, &Default::default());
        let again = collection.display_rows(false, &Default::default());
        assert!(std::sync::Arc::ptr_eq(&display.groups[3], &again.groups[3]));
        assert_eq!(display.len(), 10_001);
        assert_eq!(
            display.row(10_000),
            Some(NavigationRow::Entry {
                index: 9_999,
                staged: false
            })
        );
        assert_eq!(display.row(10_001), None);
        let collapsed = collection.display_rows(
            false,
            &std::collections::HashSet::from(["Untracked".into()]),
        );
        assert_eq!(collapsed.len(), 1);
        assert!(
            !collapsed.has_entries(),
            "collapsed groups must not drive automatic page loading"
        );
        let history = collection.display_rows(true, &Default::default());
        assert_eq!(history.len(), 10_000);
        assert_eq!(
            history.row(9_999),
            Some(NavigationRow::Entry {
                index: 9_999,
                staged: false
            })
        );
        assert_eq!(history.row(10_000), None);
    }

    #[test]
    fn row_identity_separates_index_sides_and_document_kinds() {
        use super::navigation_identity as id;
        let file = ReviewKey::File {
            path: "commit-abc".into(),
            staged: false,
        };
        assert_eq!(id(&file), id(&file.clone()));
        assert_ne!(
            id(&file),
            id(&ReviewKey::File {
                path: "commit-abc".into(),
                staged: true
            })
        );
        assert_ne!(id(&file), id(&ReviewKey::Commit("abc".into())));
    }
    fn file(name: &str, preview: bool) -> ReviewDocument {
        ReviewDocument {
            key: ReviewKey::File {
                path: name.into(),
                staged: false,
            },
            entry: json!({"entryId":name,"path":{"display":name}}),
            status: json!({"snapshot":"s"}).into(),
            preview,
        }
    }
    #[test]
    fn previews_replace_only_previews_and_close_selects_neighbor() {
        let mut docs = Documents::default();
        docs.open(file("kept.rs", false));
        docs.open(file("preview.rs", true));
        docs.open(file("next.rs", true));
        assert_eq!(docs.reviews.len(), 2);
        assert_eq!(docs.reviews[0].title(), "kept.rs · Diff");
        docs.keep(&file("next.rs", true).key);
        docs.open(file("third.rs", true));
        assert_eq!(docs.reviews.len(), 3);
        docs.open(file("kept.rs", true));
        assert!(
            !docs.reviews[0].preview,
            "reopening must not demote kept tabs"
        );
        let next = docs.close(&file("kept.rs", true).key).unwrap();
        assert_eq!(next.key, file("next.rs", true).key);
        assert_eq!(docs.active, Some(next.key));
    }
    #[test]
    fn staging_reconciles_identity_and_merges_an_existing_opposite_side() {
        let mut docs = Documents::default();
        docs.open(file("changed.rs", false));
        let mut staged = file("changed.rs", true);
        staged.key = ReviewKey::File {
            path: "changed.rs".into(),
            staged: true,
        };
        docs.open(staged.clone());
        docs.open(file("changed.rs", true));
        let page = json!({"snapshot":"after-stage"});
        assert!(docs.reconcile_file(staged.entry.clone(), page.clone(), true));
        assert_eq!(docs.active, Some(staged.key.clone()));
        assert_eq!(docs.reviews.len(), 1);
        assert!(!docs.reviews[0].preview);
        assert_eq!(docs.reviews[0].title(), "changed.rs · Staged");
        assert_eq!(*docs.reviews[0].status, page);
        assert!(!docs.reconcile_file(staged.entry, page, true));
    }

    #[test]
    fn staging_requires_current_snapshot_and_visible_entries() {
        let entry = file("one.rs", true).entry;
        let mut nav = Navigator {
            writable: true,
            ..Default::default()
        };
        nav.collections[0].set_page(json!({"snapshot":"current","entries":[entry]}));
        assert!(nav.can_stage("current", std::slice::from_ref(&entry)));
        assert!(!nav.can_stage("old", std::slice::from_ref(&entry)));
        let mut changed = entry.clone();
        changed["flags"] = json!(512);
        assert!(
            !nav.can_stage("current", &[changed]),
            "matching entry ID must not authorize changed contents"
        );
        assert!(!nav.can_stage("current", &[file("other.rs", true).entry]));
        nav.collections[0].loading = true;
        assert!(!nav.can_stage("current", std::slice::from_ref(&entry)));
        nav.collections[0].loading = false;
        nav.filter_changed = Some(Instant::now());
        assert!(!nav.can_stage("current", std::slice::from_ref(&entry)));
        nav.filter_changed = None;
        nav.invalidated = true;
        assert!(!nav.can_stage("current", &[entry]));
    }
    #[test]
    fn change_badges_describe_the_selected_index_side() {
        let entry = json!({"flags": 1 | 512, "staged":true, "unstaged":true});
        assert_eq!(super::change_mark(&entry, true), "A");
        assert_eq!(super::change_mark(&entry, false), "D");
        assert_eq!(super::change_mark(&json!({"flags":8 | 256}), true), "R");
        assert_eq!(super::change_mark(&json!({"flags":8 | 256}), false), "M");
        assert_eq!(
            super::change_mark(&json!({"conflicted":true,"untracked":true}), false),
            "!"
        );
        assert_eq!(super::change_mark(&json!({"untracked":true}), false), "?");
    }
}
