//! Grouped working-tree comparisons. Each row selects one diff side, not a file
//! to commit. Bulk actions use only the visible, snapshot-bound entry identities.
use super::*;
use gpui_kit::base::Selectable;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Group {
    Conflicted,
    Staged,
    Unstaged,
    Untracked,
}
impl Group {
    const ALL: [Self; 4] = [
        Self::Conflicted,
        Self::Staged,
        Self::Unstaged,
        Self::Untracked,
    ];
    fn key(self) -> &'static str {
        match self {
            Self::Conflicted => "conflicted",
            Self::Staged => "staged",
            Self::Unstaged => "unstaged",
            Self::Untracked => "untracked",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Conflicted => "Conflicts",
            Self::Staged => "Staged",
            Self::Unstaged => "Unstaged",
            Self::Untracked => "Untracked",
        }
    }
    fn holds(self, entry: &Value) -> bool {
        if self == Self::Conflicted {
            return entry["conflicted"] == true;
        }
        entry["conflicted"] != true
            && entry[self.key()] == true
            && (self != Self::Unstaged || entry["untracked"] != true)
    }
    fn mark(self, entry: &Value) -> &'static str {
        let flags = entry["flags"].as_u64().unwrap_or(0);
        match self {
            Self::Conflicted => "!",
            Self::Untracked => "?",
            Self::Staged if flags & 1 != 0 => "A",
            Self::Staged if flags & 4 != 0 => "D",
            Self::Staged if flags & 8 != 0 => "R",
            Self::Unstaged if flags & 512 != 0 => "D",
            Self::Unstaged if flags & 2048 != 0 => "R",
            _ => "M",
        }
    }
}
#[derive(Default)]
pub(super) struct Changes {
    input: Option<Entity<InputState>>,
    subscription: Option<Subscription>,
    query: String,
    group: Option<Group>,
    pending: Option<Instant>,
}
impl Changes {
    pub(super) fn row_count(&self, page: &Value) -> usize {
        row_count(page, self.group)
    }
    fn owns_input(&self, tab: usize, input: &Entity<InputState>) -> bool {
        tab == 0 && self.input.as_ref() == Some(input)
    }

    fn update_query(&mut self, tab: usize, input: &Entity<InputState>, query: String) -> bool {
        if !self.owns_input(tab, input) || self.query == query {
            return false;
        }
        self.query = query;
        self.pending = Some(Instant::now());
        true
    }

    pub fn filter(&self) -> Value {
        json!({"text":self.query,"group":self.group.map(Group::key).unwrap_or("all")})
    }
    pub fn filtering(&self) -> bool {
        !self.query.is_empty() || self.group.is_some()
    }
    pub(super) fn dispatched(&mut self) {
        self.pending = None;
    }
    pub fn searching(&self) -> bool {
        self.pending.is_some()
    }
}
#[derive(Clone)]
enum Row {
    Header { group: Group, entries: Vec<Value> },
    File { group: Group, entry: Value },
}
fn grouped(page: &Value, only: Option<Group>) -> Vec<Row> {
    let entries = rows(&page["entries"]);
    let mut result = Vec::new();
    for group in Group::ALL {
        if only.is_some_and(|only| only != group) {
            continue;
        }
        let entries: Vec<_> = entries
            .iter()
            .filter(|entry| group.holds(entry))
            .cloned()
            .collect();
        if entries.is_empty() {
            continue;
        }
        result.push(Row::Header {
            group,
            entries: entries.clone(),
        });
        result.extend(entries.into_iter().map(|entry| Row::File { group, entry }));
    }
    result
}
/// Count the actual virtual rows, including headers and both sides of a file.
pub(super) fn row_count(page: &Value, only: Option<Group>) -> usize {
    let Some(entries) = page["entries"].as_array() else {
        return 0;
    };
    Group::ALL
        .into_iter()
        .filter(|group| only.is_none_or(|only| only == *group))
        .map(|group| {
            let count = entries.iter().filter(|entry| group.holds(entry)).count();
            count + usize::from(count > 0)
        })
        .sum()
}
fn scope(page: &Value, filtered: bool) -> &'static str {
    if filtered {
        "shown"
    } else if page["nextCursor"].is_string() || page["metadata"]["truncated"] == true {
        "loaded"
    } else {
        "all"
    }
}
impl Newport {
    fn change_group_header(
        &self,
        index: usize,
        group: Group,
        entries: &[Value],
        scope: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let stage = group != Group::Staged;
        let enabled = group != Group::Conflicted
            && entries
                .iter()
                .all(|entry| self.git_can_change_entry(stage, entry));
        let selected = entries.to_vec();

        let total = self.git.page["metadata"]["groupCounts"][group.key()].as_u64();
        let count = if scope != "all" {
            format!(
                "{} {}{}",
                entries.len(),
                scope,
                total.map(|n| format!(" of {n}")).unwrap_or_default()
            )
        } else {
            entries.len().to_string()
        };
        div()
            .h(px(28.))
            .w_full()
            .bg(cx.theme().background)
            .flex()
            .items_center()
            .px_3()
            .gap_2()
            .child(
                crate::label(group.title())
                    .text_size(px(crate::ui::FONT_SIZE))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                crate::label(count)
                    .text_size(px(crate::ui::FONT_SIZE))
                    .flex_1(),
            )
            .when(group != Group::Conflicted, |view| {
                view.child(
                    crate::controls::compact_button(("change-group-action", index))
                        .ghost()
                        .label(format!(
                            "{} {scope}",
                            if stage { "Stage" } else { "Unstage" }
                        ))
                        .accessibility_label(format!(
                            "{} {scope} {} {} files",
                            if stage { "Stage" } else { "Unstage" },
                            entries.len(),
                            group.title().to_lowercase()
                        ))
                        .disabled(!enabled)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_entries(&selected, group, cx)
                        })),
                )
            })
            .into_any_element()
    }

    pub(super) fn sync_git_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != Page::Projects
            || self.git.tab != 0
            || !self.git.repo["repoId"].is_string()
            || self.git.changes.input.is_some()
        {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files"));
        let subscription = cx.observe(&input, |this, input, cx| {
            if this.page != Page::Projects {
                return;
            }
            let query = input.read(cx).value().trim().to_owned();
            if !this.git.changes.update_query(this.git.tab, &input, query) {
                return;
            }
            this.git_reset_changes_filter();
            cx.notify();
        });
        self.git.changes.input = Some(input);
        self.git.changes.subscription = Some(subscription);
    }
    pub(super) fn poll_git_changes(&mut self) -> bool {
        if self.page != Page::Projects
            || self.git.tab != 0
            || !self
                .git
                .changes
                .pending
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(200))
        {
            return false;
        }
        self.git.changes.pending = None;
        self.git_load();
        true
    }
    fn change_entries(&mut self, entries: &[Value], group: Group, cx: &mut Context<Self>) {
        let stage = group != Group::Staged;
        let current: std::collections::HashMap<_, _> = self.git.page["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["entryId"].as_str().map(|id| (id, entry)))
            .collect();
        if group == Group::Conflicted
            || entries.is_empty()
            || !entries.iter().all(|entry| {
                self.git_can_change_entry(stage, entry)
                    && current
                        .get(entry["entryId"].as_str().unwrap_or_default())
                        .is_some_and(|current| **current == *entry)
            })
        {
            return;
        }
        let ids: Vec<String> = entries.iter().map(|entry| text(entry, "entryId")).collect();
        let snapshot = text(&self.git.page, "snapshot");
        self.start_git_index_mutation(
            self.git_repository_id(),
            format!(
                "{} {} {}",
                if stage { "Stage" } else { "Unstage" },
                ids.len(),
                if ids.len() == 1 { "file" } else { "files" }
            ),
            move |backend, server, repo, id| {
                backend.change_index_entries(server, repo, &snapshot, &ids, stage, id)
            },
            cx,
        );
    }
    pub(super) fn git_changes_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let display = grouped(&self.git.page, self.git.changes.group);
        let scope = scope(&self.git.page, self.git.changes.filtering());
        let scroll_key = git_page_demand::identity(&self.git.page);
        let headers: Vec<_> = display
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                if let Row::Header { group, entries } = row {
                    Some((index, *group, entries.clone()))
                } else {
                    None
                }
            })
            .collect();
        let header_indices = headers.iter().map(|(index, _, _)| *index).collect();
        let header_renderer = cx.processor(move |this, index: usize, _, cx| {
            let (_, group, entries) = headers
                .iter()
                .find(|(at, _, _)| *at == index)
                .expect("known group header");
            this.change_group_header(index, *group, entries, scope, cx)
        });
        let empty = display.is_empty() && self.git.page["entries"].is_array();
        let pending = self.git.changes.searching()
            || self.resources.get("git-list").is_some_and(|r| r.pending);
        let failed = self.git.error.is_some()
            || self
                .resources
                .get("git-list")
                .is_some_and(|r| r.error.is_some());
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .when_some(self.git.changes.input.as_ref(), |view, input| {
                view.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px(px(10.))
                        .pt(px(8.))
                        .pb(px(6.))
                        .child(
                            crate::controls::input(input)
                                .aria_label("Filter changed files")
                                .prefix(
                                    Icon::new(IconName::Search)
                                        .size(px(14.))
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .suffix(
                                    crate::controls::compact_icon_button("change-group-filter")
                                        .ghost()
                                        .icon(IconName::ListFilter)
                                        .accessibility_label("Filter change groups")
                                        .tooltip("Filter change groups")
                                        .disabled(
                                            self.git_write_pending() || self.project_dialog_open,
                                        )
                                        .when(self.git.changes.group.is_some(), |button| {
                                            button.text_color(cx.theme().primary)
                                        })
                                        .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, {
                                            let weak = cx.entity().downgrade();
                                            let input = input.clone();
                                            let current_group = self.git.changes.group;
                                            move |mut menu, _, _| {
                                                menu = menu
                                                    .min_w(px(180.))
                                                    .check_side(gpui_kit::component::Side::Right)
                                                    .label("Show");
                                                for (group, title) in
                                                    std::iter::once((None, "All changes")).chain(
                                                        Group::ALL
                                                            .into_iter()
                                                            .map(|g| (Some(g), g.title())),
                                                    )
                                                {
                                                    let owner = weak.clone();
                                                    let input = input.clone();
                                                    menu = menu.item(
                                                        PopupMenuItem::new(if group.is_some() {
                                                            format!("{title} only")
                                                        } else {
                                                            title.to_owned()
                                                        })
                                                        .checked(group == current_group)
                                                        .on_click(move |_, _, cx| {
                                                            let _ = owner.update(cx, |this, cx| {
                                                                if this.page == Page::Projects
                                                                    && this.git.changes.owns_input(
                                                                        this.git.tab,
                                                                        &input,
                                                                    )
                                                                    && !this.git_write_pending()
                                                                    && !this.project_dialog_open
                                                                    && this.git.changes.group
                                                                        != group
                                                                {
                                                                    this.git.changes.group = group;
                                                                    this.git.changes.pending = None;
                                                                    this.git_reset_changes_filter();
                                                                    this.git_load();
                                                                    cx.notify();
                                                                }
                                                            });
                                                        }),
                                                    );
                                                }
                                                menu
                                            }
                                        }),
                                ),
                        ),
                )
            })
            .when(
                self.git.changes.searching() || self.initial_loading("git-list"),
                |view| view.child(project_loading::listing("changed files", 0, 8)),
            )
            .when(self.git.changes.filtering(), |view| {
                view.child(
                    crate::label(format!(
                        "{} · {}",
                        self.git
                            .changes
                            .group
                            .map(Group::title)
                            .unwrap_or("All changes"),
                        if pending {
                            "Searching…".into()
                        } else {
                            let count = rows(&self.git.page["entries"]).len();
                            format!(
                                "{count} matching {} loaded",
                                if count == 1 { "file" } else { "files" }
                            )
                        }
                    ))
                    .px_3()
                    .text_size(px(crate::ui::FONT_SIZE)),
                )
            })
            .when(empty && !pending && !failed, |view| {
                view.child(empty::compact(
                    if self.git.changes.filtering() {
                        "No files match these filters."
                    } else {
                        "No changed files loaded."
                    },
                    cx,
                ))
            })
            .child(
                uniform_list(
                    "git-changes",
                    display.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| match &display[index] {
                                Row::Header { .. } => div().h(px(28.)).into_any_element(),
                                Row::File { group, entry } => {
                                    let path = text(&entry["path"], "display");
                                    let group = *group;
                                    let selected = entry.clone();
                                    let action_entry = entry.clone();
                                    let stage = group != Group::Staged;
                                    let active = this.git.selected == *entry
                                        && this.git.diff_staged == !stage;
                                    let description = git_file_row::description(
                                        entry,
                                        &path,
                                        git_file_row::name(group.mark(entry)),
                                    );
                                    let accessible_description =
                                        if git_file_row::name(group.mark(entry)) == group.title() {
                                            description.clone()
                                        } else {
                                            format!("{description} · {}", group.title())
                                        };
                                    div()
                                        .id(("change-row", index))
                                        .group(SharedString::from(format!("git-file-row-{index}")))
                                        .tab_group()
                                        .tab_index(index as isize)
                                        .tab_stop(false)
                                        .h(px(28.))
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .pr(px(3.))
                                        .min_w_0()
                                        .rounded(px(crate::ui::RADIUS))
                                        .when(!active, |view| {
                                            view.hover(|style| style.bg(theme::row_hover(cx)))
                                        })
                                        .when(active, |view| {
                                            view.bg(theme::git_selection(cx.theme().is_dark()))
                                        })
                                        .child(
                                            crate::controls::button(("change-file", index))
                                                .custom(git_file_row::selection_style(active, cx))
                                                .selected(active)
                                                .h_full()
                                                .pl(px(10.))
                                                .pr(px(6.))
                                                .flex_1()
                                                .min_w_0()
                                                .toggled(active)
                                                .accessibility_label(accessible_description)
                                                .hover_hint(description)
                                                .child(git_file_row::badge(group.mark(entry), cx))
                                                .child(git_file_row::path(path.clone(), cx))
                                                .when_some(
                                                    git_file_row::origin(entry),
                                                    |button, origin| {
                                                        button.child(
                                                            div()
                                                                .min_w_0()
                                                                .flex_shrink(1000.)
                                                                .truncate()
                                                                .text_size(px(crate::ui::FONT_SIZE))
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                )
                                                                .child(format!("from {origin}")),
                                                        )
                                                    },
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if !rows(&this.git.page["entries"])
                                                        .contains(&selected)
                                                    {
                                                        return;
                                                    }
                                                    this.git.selected = selected.clone();
                                                    this.git.detail_open = true;
                                                    this.git_select_side(!stage);
                                                    this.project_show_review(cx);
                                                    cx.notify();
                                                })),
                                        )
                                        .when(group != Group::Conflicted, |view| {
                                            view.child(
                                                crate::controls::compact_icon_button((
                                                    "change-file-action",
                                                    index,
                                                ))
                                                .ghost()
                                                .opacity(if active { 1. } else { 0. })
                                                .group_hover(
                                                    SharedString::from(format!(
                                                        "git-file-row-{index}"
                                                    )),
                                                    |style| style.opacity(1.),
                                                )
                                                .focus(|style| style.opacity(1.))
                                                .icon(if stage {
                                                    IconName::Plus
                                                } else {
                                                    IconName::Minus
                                                })
                                                .accessibility_label(format!(
                                                    "{} {path}",
                                                    if stage { "Stage" } else { "Unstage" }
                                                ))
                                                .hover_hint(index_action_label(stage, entry))
                                                .disabled(!this.git_can_change_entry(stage, entry))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.change_entries(
                                                        std::slice::from_ref(&action_entry),
                                                        group,
                                                        cx,
                                                    )
                                                })),
                                            )
                                        })
                                        .into_any_element()
                                }
                            })
                            .collect()
                    }),
                )
                .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, _| {
                    if event.delta.pixel_delta(px(20.)).y < px(0.) {
                        this.git.listing.demand.arm(scroll_key.clone());
                    }
                }))
                .tab_group()
                .with_decoration(newport_gpui::sticky_list_headers::StickyListHeaders::new(
                    header_indices,
                    header_renderer,
                ))
                .px(px(6.))
                .track_scroll(&self.git.scroll)
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }
}
pub(super) fn selected_mark(entry: &Value, staged: bool) -> &'static str {
    let group = if entry["conflicted"] == true {
        Group::Conflicted
    } else if entry["untracked"] == true {
        Group::Untracked
    } else if staged {
        Group::Staged
    } else {
        Group::Unstaged
    };
    group.mark(entry)
}

#[cfg(test)]
mod tests {
    use super::{Changes, Group, Row, grouped, scope};
    use gpui_kit::component::input::InputState;
    use gpui_kit::{self as gpui, AppContext, Context, IntoElement, Render, Window, div};
    use serde_json::json;

    #[test]
    fn pagination_counts_group_headers_and_files_with_both_sides() {
        let page = json!({"entries":[
            {"staged":true,"unstaged":true},
            {"untracked":true},
            {"conflicted":true},
            {"staged":true}
        ]});
        for only in [
            None,
            Some(Group::Staged),
            Some(Group::Unstaged),
            Some(Group::Untracked),
            Some(Group::Conflicted),
        ] {
            assert_eq!(super::row_count(&page, only), grouped(&page, only).len());
        }
        assert_eq!(super::row_count(&page, None), 9);
        assert_eq!(super::row_count(&json!({"entries":[]}), None), 0);
    }

    struct FilterTest;
    impl Render for FilterTest {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn cached_changes_input_cannot_reset_history_or_another_checkout(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui::component::init);
        cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx));
            let replacement = cx.new(|cx| InputState::new(window, cx));
            let mut workspace = crate::git::Workspace::default();
            workspace.changes.input = Some(input.clone());
            assert!(workspace.changes.update_query(0, &input, "src".into()));
            workspace.changes.dispatched();
            let cached = std::mem::take(&mut workspace.changes);
            workspace.tab = 1;
            workspace.page = json!({"snapshot":"history", "entries":[{"oid":{"hex":"commit"}}]});
            let history = workspace.page.clone();
            assert!(!workspace.changes.update_query(
                workspace.tab,
                &input,
                "late notification".into()
            ));
            assert!(!workspace.changes.owns_input(workspace.tab, &input));
            assert!(!workspace.changes.searching());
            assert_eq!(workspace.page, history);

            workspace.changes = cached;
            // Even the retained entity cannot accept an event for another tab.
            assert!(
                !workspace
                    .changes
                    .update_query(1, &input, "inactive tab".into())
            );
            workspace.tab = 0;
            assert_eq!(
                workspace.changes.filter(),
                json!({"text":"src", "group":"all"})
            );
            assert!(!workspace.changes.update_query(0, &input, "src".into()));
            assert!(!workspace.changes.searching());
            assert!(
                workspace
                    .changes
                    .update_query(0, &input, "new filter".into())
            );
            workspace.changes = Changes {
                input: Some(replacement.clone()),
                ..Default::default()
            };
            assert!(
                !workspace
                    .changes
                    .update_query(0, &input, "old checkout".into())
            );
            assert!(!workspace.changes.owns_input(0, &input));
            assert!(
                workspace
                    .changes
                    .update_query(0, &replacement, "current checkout".into())
            );
            FilterTest
        });
    }
    #[test]
    fn immediate_reload_consumes_scheduled_filter_without_changing_it() {
        let mut changes = Changes {
            query: "file".into(),
            group: Some(Group::Staged),
            pending: Some(std::time::Instant::now()),
            ..Default::default()
        };
        assert!(changes.searching());
        changes.dispatched();
        assert!(!changes.searching());
        assert_eq!(changes.filter(), json!({"text":"file","group":"staged"}));
    }
    #[test]
    fn mixed_file_has_two_comparisons_and_side_specific_marks() {
        let entry = json!({"entryId":"mixed","staged":true,"unstaged":true,"flags":513});
        let page = json!({"entries":[entry]});
        let rows = grouped(&page, None);
        assert_eq!(rows.len(), 4);
        assert!(matches!(
            &rows[1],
            Row::File {
                group: Group::Staged,
                ..
            }
        ));
        assert!(matches!(
            &rows[3],
            Row::File {
                group: Group::Unstaged,
                ..
            }
        ));
        assert_eq!(Group::Staged.mark(&entry), "A");
        assert_eq!(Group::Unstaged.mark(&entry), "D");
    }
    #[test]
    fn conflicts_and_untracked_do_not_leak_into_other_groups() {
        let page = json!({"entries":[{"conflicted":true,"staged":true,"unstaged":true},{"untracked":true,"unstaged":true}]});
        let rows = grouped(&page, None);
        assert_eq!(rows.len(), 4);
        assert!(matches!(
            &rows[1],
            Row::File {
                group: Group::Conflicted,
                ..
            }
        ));
        assert!(matches!(
            &rows[3],
            Row::File {
                group: Group::Untracked,
                ..
            }
        ));
        assert!(grouped(&page, Some(Group::Staged)).is_empty());
    }
    #[test]
    fn bulk_scope_never_claims_all_when_filtered_or_partial() {
        assert_eq!(scope(&json!({}), false), "all");
        assert_eq!(scope(&json!({"nextCursor":"more"}), false), "loaded");
        assert_eq!(
            scope(&json!({"metadata":{"truncated":true}}), false),
            "loaded"
        );
        assert_eq!(scope(&json!({}), true), "shown");
    }
}

/// Older agents may return collapsed untracked directories.
pub(super) fn index_action_label(stage: bool, entry: &Value) -> &'static str {
    match (stage, text(&entry["path"], "display").ends_with('/')) {
        (true, true) => "Stage folder",
        (false, true) => "Unstage folder",
        (true, false) => "Stage file",
        (false, false) => "Unstage file",
    }
}

#[cfg(test)]
mod action_label_tests {
    use super::index_action_label;
    use serde_json::json;
    #[test]
    fn folder_actions_name_their_actual_scope() {
        assert_eq!(
            index_action_label(true, &json!({"path":{"display":"docs/"}})),
            "Stage folder"
        );
        assert_eq!(
            index_action_label(false, &json!({"path":{"display":"docs/"}})),
            "Unstage folder"
        );
        assert_eq!(
            index_action_label(true, &json!({"path":{"display":"docs/a.md"}})),
            "Stage file"
        );
    }
}
