//! Loaded-item filters are presentation state. They never change the server
//! cursor or fetch extra pages to fill a filtered viewport.
use super::*;
use newport_protocol::git::{
    Branch, Commit, HistoryMetadata, Page as GitPage, RemoteName, Reply, Stash, StashMetadata,
    StatusEntry, StatusSummary, Tag, TotalEntries,
};

pub(crate) type StatusPage = GitPage<StatusEntry, StatusSummary>;

/// A page without its rows. Paging demand compares it every frame, so it
/// never copies the rows it leaves out.
pub(crate) fn envelope<T, M: Clone>(page: &GitPage<T, M>) -> GitPage<T, M> {
    GitPage {
        snapshot: page.snapshot.clone(),
        entries: Vec::new(),
        next_cursor: page.next_cursor.clone(),
        metadata: page.metadata.clone(),
    }
}

/// The page of rows a Git tab shows, as the server returned it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Listing {
    Status(Box<StatusPage>),
    History(GitPage<Commit, HistoryMetadata>),
    Branches(GitPage<Branch, TotalEntries>),
    Stashes(GitPage<Stash, StashMetadata>),
    Tags(GitPage<Tag, TotalEntries>),
    Remotes(GitPage<RemoteName, TotalEntries>),
}

/// One row of a [`Listing`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Entry {
    Change(StatusEntry),
    Commit(Commit),
    Branch(Branch),
    Stash(Stash),
    Tag(Tag),
    Remote(RemoteName),
}

/// What identifies a listing page for paging demand: its envelope and
/// row count, without the rows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Identity {
    page: Listing,
    rows: usize,
}

macro_rules! each_page {
    ($listing:expr, $page:ident => $body:expr) => {
        match $listing {
            Listing::Status($page) => $body,
            Listing::History($page) => $body,
            Listing::Branches($page) => $body,
            Listing::Stashes($page) => $body,
            Listing::Tags($page) => $body,
            Listing::Remotes($page) => $body,
        }
    };
}

impl Listing {
    /// The listing a Git reply carries, if it is one.
    pub(crate) fn from_reply(reply: &Reply) -> Option<Self> {
        Some(match reply {
            Reply::Status(page) => Self::Status(page.clone()),
            Reply::History(page) => Self::History((**page).clone()),
            Reply::Branches(page) => Self::Branches((**page).clone()),
            Reply::Stashes(page) => Self::Stashes((**page).clone()),
            Reply::Tags(page) => Self::Tags((**page).clone()),
            Reply::RemoteNames(page) => Self::Remotes((**page).clone()),
            _ => return None,
        })
    }
    /// The Git reply that carries this listing.
    pub(crate) fn into_reply(self) -> Reply {
        match self {
            Self::Status(page) => Reply::Status(page),
            Self::History(page) => Reply::History(Box::new(page)),
            Self::Branches(page) => Reply::Branches(Box::new(page)),
            Self::Stashes(page) => Reply::Stashes(Box::new(page)),
            Self::Tags(page) => Reply::Tags(Box::new(page)),
            Self::Remotes(page) => Reply::RemoteNames(Box::new(page)),
        }
    }
    pub(crate) fn snapshot(&self) -> &str {
        each_page!(self, page => &page.snapshot)
    }
    pub(crate) fn next_cursor(&self) -> Option<&str> {
        each_page!(self, page => page.next_cursor.as_deref())
    }
    pub(crate) fn len(&self) -> usize {
        each_page!(self, page => page.entries.len())
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Whether the server stopped listing before the end of the data.
    pub(crate) fn truncated(&self) -> bool {
        match self {
            Self::Status(page) => page.metadata.truncated,
            Self::History(page) => page.metadata.truncated == Some(true),
            _ => false,
        }
    }
    pub(crate) fn status(&self) -> Option<&StatusPage> {
        match self {
            Self::Status(page) => Some(page.as_ref()),
            _ => None,
        }
    }
    pub(crate) fn stashes(&self) -> Option<&GitPage<Stash, StashMetadata>> {
        match self {
            Self::Stashes(page) => Some(page),
            _ => None,
        }
    }
    /// Every row, in order.
    pub(crate) fn entries(&self) -> Vec<Entry> {
        match self {
            Self::Status(page) => page.entries.iter().cloned().map(Entry::Change).collect(),
            Self::History(page) => page.entries.iter().cloned().map(Entry::Commit).collect(),
            Self::Branches(page) => page.entries.iter().cloned().map(Entry::Branch).collect(),
            Self::Stashes(page) => page.entries.iter().cloned().map(Entry::Stash).collect(),
            Self::Tags(page) => page.entries.iter().cloned().map(Entry::Tag).collect(),
            Self::Remotes(page) => page.entries.iter().cloned().map(Entry::Remote).collect(),
        }
    }
    /// The row at `index`.
    pub(crate) fn entry(&self, index: usize) -> Option<Entry> {
        Some(match self {
            Self::Status(page) => Entry::Change(page.entries.get(index)?.clone()),
            Self::History(page) => Entry::Commit(page.entries.get(index)?.clone()),
            Self::Branches(page) => Entry::Branch(page.entries.get(index)?.clone()),
            Self::Stashes(page) => Entry::Stash(page.entries.get(index)?.clone()),
            Self::Tags(page) => Entry::Tag(page.entries.get(index)?.clone()),
            Self::Remotes(page) => Entry::Remote(page.entries.get(index)?.clone()),
        })
    }
    pub(crate) fn contains(&self, entry: &Entry) -> bool {
        match (self, entry) {
            (Self::Status(page), Entry::Change(entry)) => page.entries.contains(entry),
            (Self::History(page), Entry::Commit(entry)) => page.entries.contains(entry),
            (Self::Branches(page), Entry::Branch(entry)) => page.entries.contains(entry),
            (Self::Stashes(page), Entry::Stash(entry)) => page.entries.contains(entry),
            (Self::Tags(page), Entry::Tag(entry)) => page.entries.contains(entry),
            (Self::Remotes(page), Entry::Remote(entry)) => page.entries.contains(entry),
            _ => false,
        }
    }
    /// Appends a continuation page under the incoming envelope. Remote names
    /// also keep their names distinct and their count consistent.
    pub(crate) fn merge(&self, incoming: Self, cursor: &str) -> Result<Self, String> {
        use newport_client::git_page::merge_pages;
        Ok(match (self, incoming) {
            (Self::Status(page), Self::Status(next)) => {
                Self::Status(Box::new(merge_pages(page, *next, cursor)?))
            }
            (Self::History(page), Self::History(next)) => {
                Self::History(merge_pages(page, next, cursor)?)
            }
            (Self::Branches(page), Self::Branches(next)) => {
                Self::Branches(merge_pages(page, next, cursor)?)
            }
            (Self::Stashes(page), Self::Stashes(next)) => {
                Self::Stashes(merge_pages(page, next, cursor)?)
            }
            (Self::Tags(page), Self::Tags(next)) => Self::Tags(merge_pages(page, next, cursor)?),
            (Self::Remotes(page), Self::Remotes(next)) => Self::Remotes(
                newport_client::git_remotes::Names::merge(page, next, cursor)?,
            ),
            _ => return Err("Invalid repository page. Refresh to retry.".into()),
        })
    }
    pub(crate) fn identity(&self) -> Identity {
        let page = match self {
            Self::Status(page) => Self::Status(Box::new(envelope(page))),
            Self::History(page) => Self::History(envelope(page)),
            Self::Branches(page) => Self::Branches(envelope(page)),
            Self::Stashes(page) => Self::Stashes(envelope(page)),
            Self::Tags(page) => Self::Tags(envelope(page)),
            Self::Remotes(page) => Self::Remotes(envelope(page)),
        };
        Identity {
            page,
            rows: self.len(),
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    input: Option<Entity<InputState>>,
    subscription: Option<Subscription>,
    query: String,
    remote_pending: Option<Instant>,
    pub(super) demand: git_page_demand::Demand<Option<Identity>>,
}
fn listing_name(tab: usize) -> &'static str {
    match tab {
        3 => "stashes",
        4 => "tags",
        6 => "remotes",
        _ => "entries",
    }
}
fn filterable(tab: usize) -> bool {
    matches!(tab, 3 | 4 | 6)
}
/// Changes and remotes are single-line rows; history, branches, stashes and
/// tags carry a detail line beneath the title.
pub(super) fn row_height(tab: usize) -> f32 {
    match tab {
        0 | 6 => crate::ui::ROW_HEIGHT,
        _ => crate::ui::TWO_LINE_ROW_HEIGHT,
    }
}
fn matches(tab: usize, entry: &Entry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let text = match (tab, entry) {
        (3, Entry::Stash(stash)) => stash.message.as_str(),
        (4, Entry::Tag(tag)) => tag
            .name
            .as_ref()
            .map(|name| name.display.as_str())
            .unwrap_or_default(),
        (3 | 4, _) => "",
        _ => return true,
    };
    text.to_lowercase().contains(&query.to_lowercase())
}
fn feedback(tab: usize, page: Option<&Listing>, query: &str) -> Option<String> {
    if !filterable(tab) || tab == 6 || query.is_empty() {
        return None;
    }
    let entries = page?.entries();
    let page = page?;
    let count = entries
        .iter()
        .filter(|entry| matches(tab, entry, query))
        .count();
    let name = listing_name(tab);
    let suffix = if page.truncated() {
        " · Listing incomplete"
    } else if page.next_cursor().is_some() {
        " · More available"
    } else {
        ""
    };
    Some(format!(
        "{count} of {} loaded {name} match{suffix}",
        entries.len()
    ))
}
impl Newport {
    pub(super) fn git_filtered_feedback(&self) -> Option<String> {
        if self.git.error.is_some()
            || self
                .resources
                .get("git-list")
                .is_some_and(|r| r.pending || r.error.is_some())
        {
            return None;
        }
        feedback(
            self.git.tab,
            self.git.page.as_ref(),
            &self.git.listing.query,
        )
    }
    pub(super) fn sync_git_listing_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Projects
            || !filterable(self.git.tab)
            || self.git.listing.input.is_some()
            || self.git.repo.is_none()
        {
            return;
        }
        let tab = self.git.tab;
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(match tab {
                3 => "Find a stash",
                4 => "Find a tag",
                _ => "Find a remote",
            })
        });
        let subscription = cx.observe(&input, move |this, input, cx| {
            // An input from a cached inactive view cannot change the active list.
            if this.git.tab != tab || this.git.listing.input.as_ref() != Some(&input) {
                return;
            }
            let query = input.read(cx).value().to_string();
            if this.git.listing.query == query {
                return;
            }
            this.git.listing.query = query;
            this.git.listing.demand = git_page_demand::Demand::default();
            this.git.scroll = UniformListScrollHandle::default();
            if tab == 6 {
                this.git.listing.remote_pending = Some(Instant::now());
                this.resources.invalidate("git-list");
                this.resources.invalidate("git-more");
                this.git.page = None;
                this.git.continuation = None;
                this.git.error = None;
            }
            cx.notify();
        });
        self.git.listing.input = Some(input);
        self.git.listing.subscription = Some(subscription);
    }
    pub(super) fn git_listing_filter(&self, cx: &App) -> AnyElement {
        let mut view = git_layout::search_bar().flex_col().items_stretch().gap_1();
        if let Some(input) = &self.git.listing.input {
            let name = listing_name(self.git.tab);
            view = view.child(crate::controls::search_input(input, cx).aria_label(
                if self.git.tab == 6 {
                    "Find remotes".to_owned()
                } else {
                    format!("Filter loaded {name}")
                },
            ));
            if self.git.tab != 6 && !self.git.listing.query.is_empty() {
                let count = self.git_visible_entries().len();
                let message = if count == 0 {
                    format!("No matching loaded {name}.")
                } else {
                    format!("{count} matching loaded {name}")
                };
                view = view.child(
                    div()
                        .id("git-filter-feedback")
                        .role(accesskit::Role::Label)
                        .aria_label(message.clone())
                        .child(message)
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(cx.theme().muted_foreground),
                );
            }
        }
        view.into_any_element()
    }
    pub(super) fn git_visible_entries(&self) -> Vec<Entry> {
        self.git
            .page
            .as_ref()
            .map(Listing::entries)
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| matches(self.git.tab, entry, &self.git.listing.query))
            .collect()
    }
    pub(super) fn git_visible_row_count(&self) -> usize {
        if self.git.tab == 0 {
            return self.git.changes.row_count(self.git.status_page());
        }
        self.git.page.as_ref().map_or(0, |page| {
            page.entries()
                .iter()
                .filter(|entry| matches(self.git.tab, entry, &self.git.listing.query))
                .count()
        })
    }
}
pub(super) fn toolbar(
    page: Option<&Listing>,
    singular: &str,
    plural: &str,
    action: Button,
    cx: &App,
) -> AnyElement {
    let count = page.map_or(0, Listing::len);
    let caption = format!(
        "{count} {}{}",
        if count == 1 { singular } else { plural },
        if page.and_then(Listing::next_cursor).is_some() {
            " loaded"
        } else {
            ""
        }
    );
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .p_2()
        .min_w_0()
        .child(
            div()
                .flex_1()
                .text_size(px(crate::ui::FONT_SIZE))
                .text_color(cx.theme().muted_foreground)
                .child(caption),
        )
        .child(action)
        .into_any_element()
}
pub(super) fn editor(title: &'static str, body: impl IntoElement, cx: &App) -> AnyElement {
    editor_content(title, body, false, cx)
}

/// Modal padding already bounds the content; do not add a second width cap.
pub(super) fn modal_editor(title: &'static str, body: impl IntoElement, cx: &App) -> AnyElement {
    editor_content(title, body, true, cx)
}

fn editor_content(title: &'static str, body: impl IntoElement, fill: bool, cx: &App) -> AnyElement {
    div()
        .id("git-inspector-editor")
        .w_full()
        .min_w_0()
        .py_2()
        .child(
            div()
                .w_full()
                .when(!fill, |view| view.max_w(px(520.)))
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .id("git-editor-title")
                        .role(accesskit::Role::Heading)
                        .aria_label(title)
                        .text_size(px(crate::ui::FONT_SIZE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child(title),
                )
                .child(body),
        )
        .into_any_element()
}

impl State {
    pub(super) fn remote_query(&self) -> &str {
        self.query.trim()
    }
}
impl Newport {
    pub(super) fn poll_git_remote_search(&mut self) -> bool {
        if self.page != Page::Projects
            || self.git.tab != 6
            || !self
                .git
                .listing
                .remote_pending
                .is_some_and(|time| time.elapsed() >= Duration::from_millis(200))
        {
            return false;
        }
        self.git.listing.remote_pending = None;
        if let Some(request) = self.git_listing_request(None) {
            self.request_git_typed("git-list", request);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Listing, feedback, filterable, matches};
    use crate::git_fixture::{commit, stash, tag};
    use newport_protocol::git::{Page, TotalEntries};
    #[gpui_kit::test]
    fn a_remote_search_rereads_the_list_through_the_typed_path(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext;
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut owner = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| crate::project_library_state::typed_test_owner(window, cx));
            owner = Some(view.clone());
            crate::page_perf_tests::PageHost(view)
        });
        owner.unwrap().update(visual, |view, _| {
            let (work, requests) = std::sync::mpsc::sync_channel(8);
            view.work = work.into();
            view.page = crate::Page::Projects;
            view.git.repo = Some(crate::git_fixture::repository("repo"));
            view.git.tab = 6;
            view.git.listing.remote_pending =
                Some(std::time::Instant::now() - std::time::Duration::from_millis(250));
            assert!(view.poll_git_remote_search());
            let read = requests
                .try_recv()
                .expect("the search reads the list again");
            assert_eq!(read.key, "git-list");
            assert!(matches!(
                read.request,
                crate::request_dispatch::Request::Git(_)
            ));
        });
    }
    #[test]
    fn filters_only_loaded_stash_messages_and_tag_names() {
        assert!(filterable(3) && filterable(4));
        assert!(!filterable(1));
        assert!(matches(
            3,
            &Entry::Stash(stash(0, "oid", "WIP on Main: Café")),
            "CAFÉ"
        ));
        assert!(!matches(3, &Entry::Stash(stash(0, "abc", "fix")), "abc"));
        assert!(matches(4, &Entry::Tag(tag("release/V1.0", "a")), "v1.0"));
        assert!(!matches(4, &Entry::Tag(tag("v2", "a")), "v1"));
        let mut unnamed = tag("v1", "a");
        unnamed.name = None;
        assert!(matches(4, &Entry::Tag(unnamed.clone()), ""));
        assert!(!matches(4, &Entry::Tag(unnamed), "v1"));
        assert!(matches(
            1,
            &Entry::Commit(commit("a", "A")),
            "query from another view"
        ));
    }

    #[test]
    fn filtered_counts_never_claim_hidden_or_unloaded_rows_are_shown() {
        let tags = |next: Option<&str>| {
            Listing::Tags(Page {
                snapshot: "s".into(),
                entries: vec![tag("v1", "a"), tag("v2", "b")],
                next_cursor: next.map(Into::into),
                metadata: TotalEntries { total_entries: 3 },
            })
        };
        assert_eq!(
            feedback(4, Some(&tags(Some("next"))), "v1").as_deref(),
            Some("1 of 2 loaded tags match · More available")
        );
        assert_eq!(
            feedback(4, Some(&tags(None)), "v3").as_deref(),
            Some("0 of 2 loaded tags match")
        );
        let mut history = crate::git_fixture::history_page("s", vec![commit("a", "A")], None);
        if let Listing::History(page) = &mut history {
            page.metadata.truncated = Some(true);
        }
        assert!(history.truncated());
        assert!(feedback(4, Some(&tags(None)), "").is_none());
        assert!(feedback(1, Some(&tags(None)), "v1").is_none());
        assert!(feedback(4, None, "v1").is_none());
    }
}

#[cfg(test)]
#[path = "git_listing_layout_tests.rs"]
mod layout_tests;
