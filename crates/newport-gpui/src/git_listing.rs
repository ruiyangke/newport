//! Loaded-item filters are presentation state. They never change the server
//! cursor or fetch extra pages to fill a filtered viewport.
use super::*;

#[derive(Default)]
pub(super) struct State {
    input: Option<Entity<InputState>>,
    subscription: Option<Subscription>,
    query: String,
    remote_pending: Option<Instant>,
    pub(super) demand: git_page_demand::Demand,
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
fn matches(tab: usize, entry: &Value, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let text = match tab {
        3 => entry["message"].as_str(),
        4 => entry["name"]["display"].as_str(),
        _ => return true,
    }
    .unwrap_or_default();
    text.to_lowercase().contains(&query.to_lowercase())
}
fn feedback(tab: usize, page: &Value, query: &str) -> Option<String> {
    if !filterable(tab) || tab == 6 || query.is_empty() {
        return None;
    }
    let entries = page["entries"].as_array()?;
    let count = entries
        .iter()
        .filter(|entry| matches(tab, entry, query))
        .count();
    let name = listing_name(tab);
    let suffix = if page["metadata"]["truncated"] == true {
        " · Listing incomplete"
    } else if page["nextCursor"].is_string() {
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
        feedback(self.git.tab, &self.git.page, &self.git.listing.query)
    }
    pub(super) fn sync_git_listing_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Projects
            || !filterable(self.git.tab)
            || self.git.listing.input.is_some()
            || !self.git.repo["repoId"].is_string()
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
                this.git.page = Value::Null;
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
    pub(super) fn git_visible_entries(&self) -> Vec<Value> {
        rows(&self.git.page["entries"])
            .into_iter()
            .filter(|entry| matches(self.git.tab, entry, &self.git.listing.query))
            .collect()
    }
    pub(super) fn git_visible_row_count(&self) -> usize {
        if self.git.tab == 0 {
            return self.git.changes.row_count(&self.git.page);
        }
        self.git.page["entries"].as_array().map_or(0, |entries| {
            entries
                .iter()
                .filter(|entry| matches(self.git.tab, entry, &self.git.listing.query))
                .count()
        })
    }
}
pub(super) fn toolbar(
    page: &Value,
    singular: &str,
    plural: &str,
    action: Button,
    cx: &App,
) -> AnyElement {
    let count = page["entries"].as_array().map(Vec::len).unwrap_or(0);
    let caption = format!(
        "{count} {}{}",
        if count == 1 { singular } else { plural },
        if page["nextCursor"].is_string() {
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
        self.request("git-list", "git_request", json!({"serverId":self.server,"request":{"method":"repo.remote_names","params":self.git_params()}}));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{feedback, filterable, matches};
    use serde_json::json;
    #[test]
    fn filters_only_loaded_stash_messages_and_tag_names() {
        assert!(filterable(3) && filterable(4));
        assert!(!filterable(1));
        assert!(matches(3, &json!({"message":"WIP on Main: Café"}), "CAFÉ"));
        assert!(!matches(3, &json!({"message":"fix", "oid":"abc"}), "abc"));
        assert!(matches(
            4,
            &json!({"name":{"display":"release/V1.0"}}),
            "v1.0"
        ));
        assert!(!matches(4, &json!({"name":{"display":"v2"}}), "v1"));
        assert!(matches(4, &json!({}), ""));
        assert!(matches(1, &json!({}), "query from another view"));
    }

    #[test]
    fn filtered_counts_never_claim_hidden_or_unloaded_rows_are_shown() {
        let mut page = json!({"entries":[{"name":{"display":"v1"}},{"name":{"display":"v2"}}],"nextCursor":"next"});
        assert_eq!(
            feedback(4, &page, "v1").as_deref(),
            Some("1 of 2 loaded tags match · More available")
        );
        page["nextCursor"] = serde_json::Value::Null;
        assert_eq!(
            feedback(4, &page, "v3").as_deref(),
            Some("0 of 2 loaded tags match")
        );
        page["metadata"] = json!({"truncated":true});
        assert_eq!(
            feedback(4, &page, "v1").as_deref(),
            Some("1 of 2 loaded tags match · Listing incomplete")
        );
        assert!(feedback(4, &page, "").is_none());
        assert!(feedback(1, &page, "v1").is_none());
        assert!(feedback(4, &serde_json::Value::Null, "v1").is_none());
    }
}

#[cfg(test)]
#[path = "git_listing_layout_tests.rs"]
mod layout_tests;
