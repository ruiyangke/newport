//! Display byte-oriented diff pieces without splitting UTF-8 at page boundaries.
use super::*;
use gpui_kit::component::tab::Tab;
use newport_client::{git_diff_page::DiffPage, resources::Resource};
use newport_protocol::git::{DiffMetadata, Side};
use std::{
    cell::{OnceCell, RefCell},
    rc::Rc,
};

pub(super) struct Presentation {
    pub(super) lines: Vec<Row>,
    pub(super) selection: Rc<RefCell<std::collections::BTreeSet<String>>>,
    pub(super) range_selection: Rc<RefCell<git_diff_actions::RangeSelection>>,
    pub(super) review: Result<Rc<newport_client::git_selection::Review>, String>,
    pub(super) error: Option<String>,
    pub(super) split: OnceCell<git_diff_split::Model>,
    pub(super) runs: OnceCell<Vec<Option<git_diff_runs::Segment>>>,
    pub(super) width: OnceCell<Pixels>,
}
impl Presentation {
    fn file_at(&self, row: usize) -> Option<String> {
        self.lines
            .iter()
            .take(row.saturating_add(1))
            .rev()
            .find_map(|row| row.file_path.clone())
    }
    pub(super) fn text_width(&self, cx: &App) -> Pixels {
        *self.width.get_or_init(|| {
            let system = WindowTextSystem::new(cx.text_system().clone());
            self.lines
                .iter()
                .map(|row| {
                    let text: SharedString = crate::selectable_log::TabText::new(&row.text(), 4)
                        .text
                        .into();
                    system
                        .shape_line(
                            text.clone(),
                            px(crate::ui::FONT_SIZE),
                            &[TextRun {
                                len: text.len(),
                                font: font(cx.theme().mono_font_family.clone()),
                                ..Default::default()
                            }],
                            None,
                        )
                        .width
                })
                .max()
                .unwrap_or(px(0.))
        })
    }
}
#[derive(Default)]
pub(super) struct Cache(RefCell<Option<(Option<PageKey>, Rc<Presentation>)>>);
impl Cache {
    fn get(&self, page: Option<&DiffPage>) -> Rc<Presentation> {
        // The agent fingerprint includes the full diff and comparison. The
        // continuation token distinguishes loaded prefixes of that snapshot.
        let key = page.map(PageKey::of);
        let mut cache = self.0.borrow_mut();
        if key.is_some()
            && let Some((previous, view)) = cache.as_ref()
            && *previous == key
        {
            return view.clone();
        }
        let decoded = decode(page);
        let error = decoded.as_ref().err().cloned();
        let review = page
            .ok_or_else(|| {
                "Individual hunks are unavailable for this comparison. Use the file actions."
                    .to_owned()
            })
            .and_then(newport_client::git_selection::Review::from_page)
            .map(Rc::new);
        let continued = cache.as_ref().filter(|(previous, view)| {
            error.is_none()
                && view.error.is_none()
                && review.is_ok()
                && view.review.is_ok()
                && previous
                    .as_ref()
                    .zip(key.as_ref())
                    .is_some_and(|(previous, key)| {
                        previous.snapshot == key.snapshot
                            && previous.metadata == key.metadata
                            && previous.next_cursor.is_some()
                            && previous.next_cursor != key.next_cursor
                            && key.units > previous.units
                    })
        });
        let (selection, range_selection) = if let Some((_, previous)) = continued {
            previous.range_selection.borrow_mut().page_extended();
            (previous.selection.clone(), previous.range_selection.clone())
        } else {
            (Rc::default(), Rc::default())
        };
        let view = Rc::new(Presentation {
            lines: decoded.unwrap_or_default(),
            selection,
            range_selection,
            review,
            error,
            split: OnceCell::new(),
            runs: OnceCell::new(),
            width: OnceCell::new(),
        });
        *cache = Some((key, view.clone()));
        view
    }
}

/// A page is loaded only in response to scrolling its rendered comparison.
/// Consuming the gesture prevents stale viewport geometry draining later pages.
#[derive(Default)]
pub(super) struct ScrollDemand(
    Option<Option<PageKey>>,
    git_page_demand::Demand<()>,
    Option<String>,
);
impl ScrollDemand {
    fn arm(&mut self, key: Option<PageKey>) {
        self.0 = Some(key);
    }
    fn take_ready(
        &mut self,
        page: Option<&DiffPage>,
        footer: Footer,
        visible: bool,
        at_end: bool,
    ) -> bool {
        self.0.take().is_some_and(|key| {
            visible && at_end && footer == Footer::More && key == page.map(PageKey::of)
        })
    }
}
/// Which loaded prefix of which comparison a page is.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PageKey {
    snapshot: String,
    metadata: DiffMetadata,
    next_cursor: Option<String>,
    units: usize,
}
impl PageKey {
    fn of(page: &DiffPage) -> Self {
        Self {
            snapshot: page.snapshot.clone(),
            metadata: page.metadata.clone(),
            next_cursor: page.next_cursor.clone(),
            units: page
                .entries
                .iter()
                .map(|file| {
                    if file.hunks.is_empty() {
                        1
                    } else {
                        file.hunks.iter().map(|hunk| hunk.lines.len()).sum()
                    }
                })
                .sum(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ContextLines(u8);
impl Default for ContextLines {
    fn default() -> Self {
        Self(3)
    }
}
impl ContextLines {
    pub(super) const CHOICES: [u8; 5] = [0, 1, 3, 10, 25];
    pub(super) fn value(self) -> u8 {
        self.0
    }
    pub(super) fn matches(self, metadata: &DiffMetadata, staged: bool) -> bool {
        metadata.context_lines == Some(u32::from(self.0))
            && metadata.side
                == Some(if staged {
                    Side::HeadToIndex
                } else {
                    Side::IndexToWorktree
                })
    }
    pub(super) fn set(&mut self, value: u8) -> bool {
        if value == self.0 || !Self::CHOICES.contains(&value) {
            return false;
        }
        self.0 = value;
        true
    }
}

use newport_client::git_clean::WorkingTree as EmptyChanges;

#[derive(Debug, PartialEq, Eq)]
enum Footer {
    Loading,
    LoadingMore,
    Failed,
    More,
    Complete,
    Unavailable,
}
impl Footer {
    fn for_page(
        page: Option<&DiffPage>,
        first: Option<&Resource>,
        more: Option<&Resource>,
        invalid: bool,
    ) -> Self {
        if invalid
            || [first, more]
                .into_iter()
                .flatten()
                .any(|r| r.error.is_some())
        {
            Self::Failed
        } else if first.is_some_and(|r| r.pending) {
            Self::Loading
        } else if more.is_some_and(|r| r.pending) {
            Self::LoadingMore
        } else if page.is_none() {
            Self::Unavailable
        } else if page.is_some_and(|page| page.next_cursor.is_some()) {
            Self::More
        } else {
            Self::Complete
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::Loading => "Loading diff…",
            Self::LoadingMore => "Loading more changes…",
            Self::Failed => "Diff incomplete.",
            Self::More => "More changes available",
            Self::Complete => "End of diff preview",
            Self::Unavailable => "Diff preview unavailable.",
        }
    }
}

#[derive(Default)]
pub(super) struct Row {
    pub(super) heading: bool,
    pub(super) file_path: Option<String>,
    pub(super) hunk: Option<String>,
    pub(super) selection: Option<(String, String)>,
    pub(super) origin: String,
    pub(super) old: String,
    pub(super) new: String,
    pub(super) bytes: Vec<u8>,
}
impl Row {
    pub(super) fn text(&self) -> String {
        let raw = String::from_utf8_lossy(&self.bytes);
        let raw = raw.trim_end_matches(['\r', '\n']);
        let raw = if matches!(self.origin.as_str(), "<" | ">" | "=") {
            raw.trim_start_matches(['\r', '\n'])
        } else {
            raw
        };
        // Display control line breaks explicitly; a wire fragment cannot add
        // visual rows or panic the native single-line text shaper.
        raw.replace('\n', "↵").replace('\r', "␍")
    }
}
impl From<newport_client::git_diff::Line> for Row {
    fn from(line: newport_client::git_diff::Line) -> Self {
        Self {
            origin: line.origin,
            old: line.old.map(|n| n.to_string()).unwrap_or_default(),
            new: line.new.map(|n| n.to_string()).unwrap_or_default(),
            bytes: line.bytes,
            selection: line.selection,
            ..Default::default()
        }
    }
}
fn decode(page: Option<&DiffPage>) -> Result<Vec<Row>, String> {
    let mut result = Vec::new();
    let Some(page) = page else {
        return Ok(result);
    };
    let mut lines = newport_client::git_diff::Lines::default();
    let mut files_seen = std::collections::HashSet::new();
    let mut hunks_seen = std::collections::HashSet::new();
    for file in &page.entries {
        let first_file = files_seen.insert(file.file_index);
        let old = &file.old_path;
        let new = &file.new_path;
        let file_start = result.len();
        let path = new.display.as_str();
        if first_file && page.metadata.total_files.is_some_and(|n| n > 1) {
            result.push(Row {
                heading: true,
                bytes: path.as_bytes().to_vec(),
                ..Default::default()
            });
        }
        if first_file && old.bytes_b64 != new.bytes_b64 {
            result.push(Row {
                heading: true,
                bytes: format!("Renamed from {}", old.display).into_bytes(),
                ..Default::default()
            });
        }
        let (old_mode, new_mode) = (file.old_mode, file.new_mode);
        if first_file && old_mode != 0 && new_mode != 0 && old_mode != new_mode {
            result.push(Row {
                heading: true,
                bytes: format!("Mode {old_mode:o} → {new_mode:o}").into_bytes(),
                ..Default::default()
            });
        }
        if let Some(reason) = &file.omission_reason
            && first_file
        {
            result.push(Row {
                heading: true,
                bytes: if reason == "binary" {
                    "Binary file changed. A text diff is not available.".into()
                } else {
                    format!("Preview unavailable: {reason}")
                }
                .into_bytes(),
                ..Default::default()
            });
        }
        for hunk in &file.hunks {
            if hunks_seen.insert((file.file_index, hunk.index)) {
                result.push(Row {
                    heading: true,
                    bytes: format!(
                        "@@ −{},{} +{},{} @@",
                        hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
                    )
                    .into_bytes(),
                    hunk: hunk.id.clone(),
                    old: hunk.old_start.to_string(),
                    new: hunk.new_start.to_string(),
                    ..Default::default()
                });
            }
            for piece in &hunk.lines {
                if let Some(line) =
                    lines.push(file.file_index, hunk.index, hunk.id.as_deref(), piece)?
                {
                    result.push(line.into());
                }
            }
        }
        if first_file && let Some(row) = result.get_mut(file_start) {
            row.file_path = Some(path.to_owned());
        }
    }
    if let Some(line) = lines.finish(page.next_cursor.is_some())? {
        let row = Row::from(line);
        result.push(Row {
            bytes: format!(
                "{} … (continued on next page)",
                String::from_utf8_lossy(&row.bytes)
            )
            .into_bytes(),
            ..row
        });
    }
    Ok(result)
}
impl Newport {
    fn visible_diff_file(&self, presentation: &Presentation) -> Option<String> {
        let top =
            (-self.git.diff_scroll.0.borrow().base_handle.offset().y / px(22.)).max(0.) as usize;
        let row = if self.diff_layout() == git_diff_split::Layout::Split {
            presentation
                .split
                .get()
                .and_then(|model| model.source_row(top))
                .unwrap_or(0)
        } else {
            top
        };
        presentation.file_at(row)
    }
    pub(super) fn poll_git_diff_scroll(&mut self) -> bool {
        let inspection = self.git_is_inspection();
        let visible = self.page == Page::Projects
            && (self.git.tab == 0 || inspection)
            && (!git_layout::compact(self.content_width, self.git.tab) || self.git.detail_open)
            && self.git.remote_refs.is_none()
            && !self.project_dialog_open
            && self.git.repo.is_some()
            && !self.git.entry().is_some_and(|entry| entry.conflicted);
        if !visible {
            if let Some((_, presentation)) = self.git.diff_presentation.0.borrow().as_ref() {
                presentation.range_selection.borrow_mut().reset();
            }
            self.git.diff_demand.0 = None;
            return false;
        }
        let (first, more, invalid) = if inspection {
            (
                "git-history-diff",
                "git-history-diff-more",
                self.git.history.error.is_some(),
            )
        } else {
            ("git-diff", "git-diff-more", self.git.error.is_some())
        };
        let page = self.diff_page();
        let footer = Footer::for_page(
            page,
            self.resources.get(first),
            self.resources.get(more),
            invalid || self.git.diff_presentation.get(page).error.is_some(),
        );
        // A byte-limited page can contain too few rows to overflow. A real
        // downward gesture still loads its continuation without eager reads.
        let at_end = self.git.diff_scroll.is_scrolled_to_end() != Some(false);
        let presentation = self.git.diff_presentation.get(page);
        let file = self.visible_diff_file(&presentation);
        let file_changed = self.git.diff_demand.2 != file;
        self.git.diff_demand.2 = file;
        let count = if self.diff_layout() == git_diff_split::Layout::Split {
            presentation
                .split
                .get()
                .map_or(0, |model| model.row_count())
        } else {
            presentation.lines.len()
        };
        let scrolled = self.git.diff_demand.1.take_scrolled(
            &self.git.diff_scroll,
            count,
            22.,
            visible && footer == Footer::More,
        );
        let page = if inspection {
            self.git.history.diff.as_ref()
        } else {
            self.git.diff.as_ref()
        };
        if !self
            .git
            .diff_demand
            .take_ready(page, footer, visible, at_end)
            && !scrolled
        {
            return file_changed;
        }
        self.git_diff_more();
        true
    }
    fn git_diff_toolbar(&self, title: String, cx: &mut Context<Self>) -> AnyElement {
        let working = !self.git_is_inspection();
        let entry = self.git.entry().filter(|_| working);
        let mixed = entry.is_some_and(|entry| entry.staged && (entry.unstaged || entry.untracked));
        let stats = diff_totals(self.diff_page());
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .when(working, |view| {
                view.child(git_file_row::badge(
                    entry.map_or("M", |entry| {
                        git_changes::selected_mark(entry, self.git.diff_staged)
                    }),
                    cx,
                ))
            })
            .child(
                div().flex_1().min_w(px(120.)).flex().flex_col().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(
                            git_file_row::path(title, cx)
                                .flex_initial()
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .when_some(stats, |view, (adds, dels)| {
                            view.child(
                                div()
                                    .id("git-diff-statistics")
                                    .role(accesskit::Role::Label)
                                    .aria_label(format!("{adds} additions, {dels} deletions"))
                                    .flex()
                                    .gap_1()
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .child(
                                        div()
                                            .text_color(theme::success(cx.theme().is_dark()))
                                            .child(format!("+{adds}")),
                                    )
                                    .child(
                                        div()
                                            .text_color(theme::error(cx.theme().is_dark()))
                                            .child(format!("−{dels}")),
                                    ),
                            )
                        }),
                ),
            )
            .when(mixed, |view| {
                view.child(
                    crate::controls::tabs("diff-comparison")
                        .gap_0()
                        .selected_index(usize::from(!self.git.diff_staged))
                        .children(
                            ["Staged", "Unstaged"]
                                .into_iter()
                                .map(|name| Tab::new().label(name)),
                        )
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            if this.git.diff_staged != (*index == 0) {
                                this.git_select_side(*index == 0);
                                cx.notify();
                            }
                        })),
                )
            })
            .when_some(
                working
                    .then(|| self.code_workspace())
                    .flatten()
                    .and_then(|editor| {
                        entry
                            .and_then(|entry| project_editor::path_text(&entry.path.bytes_b64))
                            .filter(|path| !path.ends_with('/'))
                            .map(|path| (editor, path))
                    }),
                |view, (editor, path)| {
                    view.child(
                        crate::controls::compact_button("project-edit-change")
                            .ghost()
                            .icon(IconName::File)
                            .label("Edit file")
                            .on_click(move |_, window, cx| {
                                editor.update(cx, |e, cx| e.open_relative(&path, window, cx));
                            }),
                    )
                },
            )
            .when(
                entry.is_some_and(|entry| entry.unstaged || entry.untracked),
                |view| {
                    view.child(
                        crate::controls::compact_button("git-stage-file")
                            .label(git_changes::index_action_label(true))
                            .disabled(!self.git_can_change_index(true))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.git_change_index(true, cx)),
                            ),
                    )
                },
            )
            .when(entry.is_some_and(|entry| entry.staged), |view| {
                view.child(
                    crate::controls::compact_button("git-unstage-file")
                        .label(git_changes::index_action_label(false))
                        .disabled(!self.git_can_change_index(false))
                        .on_click(cx.listener(|this, _, _, cx| this.git_change_index(false, cx))),
                )
            })
            .when(working, |view| {
                view.child(
                    crate::controls::compact_icon_button("git-discard-file")
                        .icon(IconName::Undo2)
                        .tooltip("Discard file changes…")
                        .accessibility_label("Discard file changes…")
                        .disabled(!self.git_can_discard())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.prepare_git_discard(None, window, cx)
                        })),
                )
            })
            .child(
                crate::controls::compact_icon_button("diff-find-open")
                    .ghost()
                    .icon(IconName::Search)
                    .accessibility_label("Find in diff")
                    .tooltip("Find in diff (⌘F)")
                    .on_click(cx.listener(|this, _, window, cx| this.open_git_find(window, cx))),
            )
            .child(git_diff_settings::control(cx.entity()))
            .into_any_element()
    }
    fn unified_diff(
        &self,
        presentation: Rc<Presentation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = self.diff_actions(&presentation, cx);
        let document = self.git.diff_text.prepare(
            git_diff_text::key(self.diff_page(), false, &self.diff_title()),
            &presentation,
            || {
                vec![
                    presentation
                        .lines
                        .iter()
                        .map(|r| git_diff_text::row(Some(r)))
                        .collect(),
                ]
            },
            &self.focus,
            window,
            cx,
        )[0]
        .clone();
        render_unified(presentation, &self.git.diff_scroll, actions, document, cx)
    }

    pub(super) fn git_diff_view(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.git.tab == 0 && self.git.entry().is_some_and(|entry| entry.conflicted) {
            self.git.diff_text.clear(cx);
            return self.conflict_view(cx);
        }
        let selected = if self.git_is_inspection() {
            self.git.history.path.is_some()
        } else {
            self.git.entry().is_some()
        };
        if !selected {
            self.git.diff_text.clear(cx);
            let loading = !self.git_is_inspection()
                && (self.git.changes.searching()
                    || EmptyChanges::for_page(
                        self.git.status_page(),
                        self.resources.get("git-list"),
                        self.git.error.is_some(),
                    ) == EmptyChanges::Loading);
            if loading {
                // Use the same full-width column as loaded diff lines. The
                // centered empty-state wrapper adds gutters that don't belong
                // to the file content and makes the skeleton jump on arrival.
                let placeholder = self.git.changes.searching() || self.placeholder_due("git-list");
                return div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .when(placeholder, |view| view.child(project_loading::diff()))
                    .into_any_element();
            }
            let content = if self.git_is_inspection() {
                empty::content(
                    IconName::File,
                    "Choose a file",
                    "Select a file to inspect this commit.",
                    cx,
                )
                .into_any_element()
            } else if self.git.changes.searching() {
                project_loading::diff()
            } else if self.git.changes.filtering()
                && self
                    .git
                    .page
                    .as_ref()
                    .is_some_and(git_listing::Listing::is_empty)
                && self
                    .resources
                    .get("git-list")
                    .is_some_and(|r| !r.pending && r.error.is_none())
                && self.git.error.is_none()
            {
                empty::content(
                    IconName::Search,
                    "No matching changes",
                    "Adjust the filters to see other changed files.",
                    cx,
                )
                .into_any_element()
            } else {
                match EmptyChanges::for_page(
                    self.git.status_page(),
                    self.resources.get("git-list"),
                    self.git.error.is_some(),
                ) {
                    EmptyChanges::Loading => project_loading::diff(),
                    EmptyChanges::Unavailable => empty::content(
                        IconName::File,
                        "Changes unavailable",
                        "Use Refresh to try again.",
                        cx,
                    )
                    .into_any_element(),
                    EmptyChanges::Clean => self.git_clean_view(cx),
                    EmptyChanges::Select => empty::content(
                        IconName::File,
                        "Choose a file",
                        "Select a file to review its changes.",
                        cx,
                    )
                    .into_any_element(),
                }
            };
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .child(content)
                .into_any_element();
        }
        let page = self.diff_page();
        let presentation = self.git.diff_presentation.get(page);
        let error = presentation.error.clone();
        let (first_key, more_key, invalid) = if self.git_is_inspection() {
            (
                "git-history-diff",
                "git-history-diff-more",
                self.git.history.error.is_some(),
            )
        } else {
            ("git-diff", "git-diff-more", self.git.error.is_some())
        };
        let footer = Footer::for_page(
            page,
            self.resources.get(first_key),
            self.resources.get(more_key),
            invalid || error.is_some(),
        );
        let partial_hint = (!self.git_is_inspection() && footer != Footer::Loading)
            .then(|| presentation.review.as_ref().err().cloned())
            .flatten();
        let empty = presentation.lines.is_empty();
        let title = self.diff_title();
        let file_context = self
            .visible_diff_file(&presentation)
            .filter(|path| path != &title);
        let selection_bar = self
            .diff_actions(&presentation, cx)
            .and_then(|actions| actions.selection_bar(cx));
        let scroll_key = page.map(PageKey::of);
        // Prepare the text/search projection before rendering its match count.
        let content = (footer != Footer::Loading && !empty).then(|| {
            if self.diff_layout() == git_diff_split::Layout::Split {
                self.split_diff(presentation, window, cx)
            } else {
                self.unified_diff(presentation, window, cx)
            }
        });
        if content.is_none() {
            self.git.diff_text.clear(cx);
        }
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.git_diff_toolbar(title, cx))
            .child(self.git_find_bar(cx))
            .when_some(file_context, |view, path| {
                view.child(
                    div()
                        .id("git-diff-current-file")
                        .px_3()
                        .py_1()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .role(accesskit::Role::Label)
                        .aria_label(format!("Current diff file: {path}"))
                        .hover_path("File", path.clone())
                        .child(git_file_row::path(path, cx)),
                )
            })
            .when_some(selection_bar, |view, bar| view.child(bar))
            .child(self.status(first_key, cx))
            .child(self.status(more_key, cx))
            .when_some(error, |view, error| view.child(crate::label(error).p_3()))
            .when_some(partial_hint.filter(|_| !empty), |view, hint| {
                view.child(
                    crate::label(hint)
                        .px_3()
                        .py_2()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .bg(cx.theme().muted),
                )
            })
            .when(
                page.is_some_and(|page| page.metadata.truncated == Some(true)),
                |view| {
                    view.child(
                        crate::label(
                            "The diff exceeds the agent’s limit. Some changes are not shown.",
                        )
                        .px_3()
                        .py_2(),
                    )
                },
            )
            .when(footer == Footer::Loading, |view| {
                view.child(project_loading::diff())
            })
            .when(footer == Footer::Complete && empty, |view| {
                view.child(div().flex_1().flex().items_center().justify_center().child(
                    empty::content(
                        IconName::File,
                        "No text changes",
                        "This file has no text changes to display.",
                        cx,
                    ),
                ))
            })
            .when_some(content, |view, content| {
                view.child(
                    div()
                        .id("git-diff-scroll-region")
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, _| {
                            if event.delta.pixel_delta(px(22.)).y < px(0.) {
                                this.git.diff_demand.arm(scroll_key.clone());
                            }
                        }))
                        .child(content),
                )
            })
            .when(footer != Footer::Complete, |view| {
                view.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .flex_shrink_0()
                        .child(crate::label(footer.label()).flex_1())
                        .when(
                            matches!(footer, Footer::Failed | Footer::Unavailable),
                            |view| {
                                view.child(
                                    crate::controls::compact_button("git-diff-retry")
                                        .label("Try again")
                                        .accessibility_label("Try loading the diff again")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.git_refresh();
                                            cx.notify();
                                        })),
                                )
                            },
                        ),
                )
            })
            .into_any_element()
    }
}
// The same renderer is exercised by the native interaction tests.
pub(super) fn render_unified<T: 'static>(
    presentation: Rc<Presentation>,
    scroll: &UniformListScrollHandle,
    actions: Option<git_diff_actions::Actions>,
    document: git_diff_text::Document,
    cx: &mut Context<T>,
) -> AnyElement {
    let row_document = document.clone();
    let drag_actions = actions.clone();
    let drag_presentation = presentation.clone();
    presentation
        .runs
        .get_or_init(|| git_diff_runs::group(presentation.lines.iter().map(Some)));
    let width = presentation.text_width(cx) + px(305.);
    let list = uniform_list(
        "working-diff",
        presentation.lines.len(),
        cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
            range
                .map(|i| {
                    let row = &presentation.lines[i];
                    div()
                        .h(px(22.))
                        .w(width)
                        .min_w_full()
                        .flex()
                        .items_center()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .font_family(cx.theme().mono_font_family.clone())
                        .bg(if row.heading {
                            theme::diff_hunk(cx.theme().is_dark())
                        } else if row.origin == "+" {
                            theme::diff_add(cx.theme().is_dark())
                        } else if row.origin == "-" {
                            theme::diff_remove(cx.theme().is_dark())
                        } else {
                            cx.theme().background
                        })
                        .when_some(actions.as_ref(), |view, actions| {
                            view.child(actions.line_control(
                                row,
                                presentation.runs.get().unwrap()[i].as_ref(),
                                cx,
                            ))
                        })
                        .when(!row.heading, |view| {
                            view.child(git_diff_actions::line_number(
                                row,
                                false,
                                false,
                                actions.as_ref(),
                                cx,
                            ))
                            .child(git_diff_actions::line_number(
                                row,
                                true,
                                false,
                                actions.as_ref(),
                                cx,
                            ))
                            .child(
                                cell(row.origin.clone(), 18.).text_color(if row.origin == "+" {
                                    theme::success(cx.theme().is_dark())
                                } else if row.origin == "-" {
                                    theme::error(cx.theme().is_dark())
                                } else {
                                    cx.theme().muted_foreground
                                }),
                            )
                        })
                        .when(row.heading, |view| {
                            // Match the code-row gutter so width measurement
                            // cannot under-estimate a slightly shorter code line.
                            view.child(div().w(px(108.)).flex_shrink_0())
                                .text_color(cx.theme().muted_foreground)
                        })
                        .child(
                            git_diff_text::code_container(row, i, None)
                                .flex_shrink_0()
                                .whitespace_nowrap()
                                .text_color(if row.heading {
                                    cx.theme().muted_foreground
                                } else if row.origin == "+" {
                                    theme::diff_add_ink(cx.theme().is_dark())
                                } else if row.origin == "-" {
                                    theme::diff_remove_ink(cx.theme().is_dark())
                                } else {
                                    cx.theme().foreground
                                })
                                .child(if row.heading {
                                    div().child(row.text()).into_any_element()
                                } else {
                                    git_diff_text::text(&row_document, i, cx)
                                }),
                        )
                        .when_some(
                            actions.as_ref().zip(row.hunk.as_deref()),
                            |view, (actions, hunk)| {
                                view.child(div().ml_3().child(actions.button(hunk, &row.new)))
                            },
                        )
                })
                .collect()
        }),
    )
    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
    .track_scroll(scroll)
    .flex_1();
    let list = document
        .borrow()
        .selection
        .viewport(list)
        .into_any_element();
    let list = match drag_actions {
        Some(actions) => actions.viewport(
            0,
            || {
                drag_presentation
                    .lines
                    .iter()
                    .enumerate()
                    .filter_map(|(i, row)| {
                        row.selection.as_ref().map(|(_, line)| (i, line.clone()))
                    })
                    .collect()
            },
            scroll,
            list,
        ),
        None => list,
    };
    git_diff_text::scroll_viewport("unified-diff-scrollbar", list, scroll)
}

#[cfg(test)]
mod tests {
    use super::{
        Cache, ContextLines, DiffPage, EmptyChanges, Footer, PageKey, Row, ScrollDemand, decode,
    };
    use crate::git_fixture::{diff_page, file, hunk, line, working_metadata};
    use newport_client::resources::Resources;
    use newport_protocol::git::{LineFragment, Path, Side};

    /// A page whose `units` complete lines sit in one hunk of one file.
    fn lines_page(snapshot: &str, cursor: Option<&str>, units: u64) -> DiffPage {
        diff_page(
            snapshot,
            working_metadata("status", "entry"),
            vec![file(
                0,
                "file",
                vec![hunk(
                    0,
                    None,
                    (0..units).map(|i| line(i, "+", None, b"a\n")).collect(),
                )],
            )],
            cursor,
        )
    }
    /// The first piece of a line whose bytes continue on the next piece.
    fn partial(index: u64, offset: u64, bytes: &[u8]) -> LineFragment {
        LineFragment {
            byte_offset: offset,
            line_complete: false,
            ..line(index, "+", None, bytes)
        }
    }
    fn split_euro() -> Vec<u8> {
        [vec![b'x'; 4095], vec![0xe2]].concat()
    }
    fn key(page: &DiffPage) -> Option<PageKey> {
        Some(PageKey::of(page))
    }
    #[test]
    fn scrolling_loads_one_page_and_never_drains_on_repaint() {
        let page = lines_page("a", Some("p2"), 1);
        let mut demand = ScrollDemand::default();
        assert!(!demand.take_ready(Some(&page), Footer::More, true, true));
        demand.arm(key(&page));
        assert!(demand.take_ready(Some(&page), Footer::More, true, true));
        assert!(!demand.take_ready(Some(&page), Footer::More, true, true));
        let next = lines_page("a", Some("p3"), 2);
        assert!(!demand.take_ready(Some(&next), Footer::More, true, true));
        demand.arm(key(&next));
        assert!(demand.take_ready(Some(&next), Footer::More, true, true));
    }
    #[test]
    fn scroll_demand_cannot_cross_a_comparison_or_page_boundary() {
        let mut page = lines_page("a", Some("p2"), 1);
        page.metadata.side = Some(Side::HeadToIndex);
        let mut other_side = page.clone();
        other_side.metadata.side = Some(Side::IndexToWorktree);
        let mut other_snapshot = page.clone();
        other_snapshot.snapshot = "b".into();
        let mut longer = lines_page("a", Some("p3"), 2);
        longer.metadata.side = Some(Side::HeadToIndex);
        for next in [other_snapshot, longer, other_side] {
            let mut demand = ScrollDemand::default();
            demand.arm(key(&page));
            assert!(!demand.take_ready(Some(&next), Footer::More, true, true));
            assert!(!demand.take_ready(Some(&page), Footer::More, true, true));
        }
    }
    #[test]
    fn hidden_busy_failed_and_nonterminal_viewports_do_not_load() {
        let page = lines_page("a", Some("p2"), 1);
        for (footer, visible, at_end) in [
            (Footer::More, false, true),
            (Footer::More, true, false),
            (Footer::Loading, true, true),
            (Footer::LoadingMore, true, true),
            (Footer::Failed, true, true),
            (Footer::Complete, true, true),
            (Footer::Unavailable, true, true),
        ] {
            let mut demand = ScrollDemand::default();
            demand.arm(key(&page));
            assert!(!demand.take_ready(Some(&page), footer, visible, at_end));
            assert!(!demand.take_ready(Some(&page), Footer::More, true, true));
        }
    }
    #[test]
    fn presentation_cache_reuses_a_snapshot_prefix_and_invalidates_on_read_changes() {
        let cache = Cache::default();
        let mut page = diff_page("first", working_metadata("s", "e"), vec![], Some("page-2"));
        let first = cache.get(Some(&page));
        assert!(std::rc::Rc::ptr_eq(&first, &cache.get(Some(&page))));
        page.next_cursor = None;
        let completed = cache.get(Some(&page));
        assert!(!std::rc::Rc::ptr_eq(&first, &completed));
        page.metadata.side = Some(Side::HeadToIndex);
        let staged = cache.get(Some(&page));
        assert!(!std::rc::Rc::ptr_eq(&completed, &staged));
        page.snapshot = "second".into();
        assert!(!std::rc::Rc::ptr_eq(&staged, &cache.get(Some(&page))));
        // No page never qualifies for reuse.
        assert!(!std::rc::Rc::ptr_eq(&cache.get(None), &cache.get(None)));
    }
    #[test]
    fn eof_notes_and_control_characters_cannot_create_extra_display_rows() {
        let note = Row {
            origin: "<".into(),
            bytes: b"\n\\ No newline at end of file\n".to_vec(),
            ..Default::default()
        };
        assert_eq!(note.text(), "\\ No newline at end of file");
        let crlf = Row {
            bytes: b"line\r\n".to_vec(),
            ..Default::default()
        };
        assert_eq!(crlf.text(), "line");
        let unusual = Row {
            bytes: b"line\nnext\rtext".to_vec(),
            ..Default::default()
        };
        assert_eq!(unusual.text(), "line↵next␍text");
    }
    #[test]
    fn context_choices_bind_the_response_to_the_requested_comparison() {
        let mut context = ContextLines::default();
        let mut metadata = working_metadata("s", "e");
        assert!(context.matches(&metadata, false));
        assert!(!context.matches(&metadata, true));
        assert!(!context.set(3));
        assert!(!context.set(255));
        assert_eq!(context.value(), 3);
        for value in [0, 1, 3, 10, 25] {
            assert!(context.set(value));
            metadata.context_lines = Some(u32::from(value));
            assert!(context.matches(&metadata, false));
            let mut stale = metadata.clone();
            stale.context_lines = Some(100);
            assert!(!context.matches(&stale, false));
        }
        metadata.context_lines = None;
        assert!(!context.matches(&metadata, false));
    }
    #[test]
    fn hunk_headers_preserve_ranges_without_counting_as_changed_lines() {
        let mut first = hunk(
            0,
            Some("first-hunk"),
            vec![
                partial(0, 0, &split_euro()),
                LineFragment {
                    byte_offset: 4096,
                    ..line(0, "+", None, &[0x82, 0xac])
                },
            ],
        );
        (
            first.old_start,
            first.old_lines,
            first.new_start,
            first.new_lines,
        ) = (0, 0, 1, 1);
        let mut second = hunk(
            1,
            Some("second-hunk"),
            vec![LineFragment {
                old_line: Some(20),
                new_line: None,
                ..line(0, "-", None, b"removed\n")
            }],
        );
        (
            second.old_start,
            second.old_lines,
            second.new_start,
            second.new_lines,
        ) = (20, 1, 21, 0);
        let page = diff_page(
            "s",
            working_metadata("s", "e"),
            vec![file(0, "file", vec![first, second])],
            None,
        );
        let rows = decode(Some(&page)).unwrap();
        assert_eq!(rows.len(), 4);
        assert!(rows[0].heading && rows[2].heading);
        assert_eq!(rows[0].hunk.as_deref(), Some("first-hunk"));
        assert_eq!(rows[2].hunk.as_deref(), Some("second-hunk"));
        assert!(rows[1].hunk.is_none() && rows[3].hunk.is_none());
        assert_eq!(rows[0].bytes, "@@ −0,0 +1,1 @@".as_bytes());
        assert_eq!(rows[2].bytes, "@@ −20,1 +21,0 @@".as_bytes());
        assert_eq!(rows[1].bytes, format!("{}€", "x".repeat(4095)).as_bytes());
        assert!(!rows[1].heading);
        assert_eq!(rows[3].old, "20");
    }
    #[test]
    fn metadata_only_changes_are_not_displayed_as_an_empty_diff() {
        let mut renamed = file(0, "new", vec![]);
        renamed.old_path = Path::new(b"old");
        renamed.new_mode = 0o100755;
        let page = diff_page("s", working_metadata("s", "e"), vec![renamed], None);
        let rows = decode(Some(&page)).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].bytes, b"Renamed from old");
        assert_eq!(rows[1].bytes, "Mode 100644 → 100755".as_bytes());
        assert!(rows.iter().all(|row| row.heading && row.origin.is_empty()));
        let mut added = file(0, "new", vec![]);
        added.old_mode = 0;
        let new_file = diff_page("s", working_metadata("s", "e"), vec![added], None);
        assert!(decode(Some(&new_file)).unwrap().is_empty());
        let mut binary = file(0, "new", vec![]);
        binary.omission_reason = Some("binary".into());
        let binary = diff_page("s", working_metadata("s", "e"), vec![binary], None);
        assert_eq!(
            decode(Some(&binary)).unwrap()[0].bytes,
            b"Binary file changed. A text diff is not available."
        );
    }
    #[test]
    fn multi_file_diffs_identify_each_file_and_keep_context_between_headers() {
        let added = |index, name: &str| {
            file(
                index,
                name,
                vec![hunk(0, None, vec![line(0, "+", None, b"a\n")])],
            )
        };
        let mut metadata = working_metadata("s", "e");
        metadata.total_files = Some(2);
        let mut page = diff_page(
            "s",
            metadata,
            vec![added(0, "docs/a.md"), added(1, "docs/nested/b.md")],
            None,
        );
        let cache = Cache::default();
        let view = cache.get(Some(&page));
        assert!(view.error.is_none());
        assert_eq!(view.lines.len(), 6);
        assert_eq!(view.lines[0].text(), "docs/a.md");
        assert_eq!(view.lines[3].text(), "docs/nested/b.md");
        for (row, path) in [
            (0, "docs/a.md"),
            (2, "docs/a.md"),
            (3, "docs/nested/b.md"),
            (5, "docs/nested/b.md"),
            (50, "docs/nested/b.md"),
        ] {
            assert_eq!(view.file_at(row).as_deref(), Some(path));
        }
        // A single-file page still gets an exact sticky file identity without
        // adding a redundant in-body heading.
        page.snapshot = "single".into();
        page.metadata.total_files = Some(1);
        page.entries.truncate(1);
        let view = cache.get(Some(&page));
        assert_eq!(view.lines.len(), 2);
        assert_eq!(view.file_at(1).as_deref(), Some("docs/a.md"));
    }
    #[test]
    fn pagination_preserves_selected_lines_but_refresh_does_not() {
        let cache = Cache::default();
        let mut untracked = file(
            0,
            "f",
            vec![hunk(0, Some("h"), vec![line(0, "+", Some("a"), b"a\n")])],
        );
        untracked.status = "Untracked".into();
        untracked.old_mode = 0;
        untracked.hunks[0].total_lines = 2;
        let mut page = diff_page(
            "diff",
            working_metadata("status", "file"),
            vec![untracked],
            Some("page2"),
        );
        let first = cache.get(Some(&page));
        assert!(first.error.is_none());
        first.selection.borrow_mut().insert("a".into());
        assert!(!first.review.as_ref().unwrap().hunk_complete("h"));
        page.entries[0].hunks[0]
            .lines
            .push(line(1, "+", Some("b"), b"b\n"));
        page.next_cursor = None;
        let next = cache.get(Some(&page));
        assert!(next.error.is_none());
        assert!(next.selection.borrow().contains("a"));
        assert!(std::rc::Rc::ptr_eq(
            &first.range_selection,
            &next.range_selection
        ));
        assert!(next.review.as_ref().unwrap().hunk_complete("h"));
        assert_eq!(
            next.lines.iter().filter(|r| r.selection.is_some()).count(),
            2
        );
        page.snapshot = "refreshed".into();
        assert!(cache.get(Some(&page)).selection.borrow().is_empty());
    }
    #[test]
    fn line_selection_is_scoped_to_the_cached_comparison() {
        let cache = Cache::default();
        let original = diff_page("a", working_metadata("s", "e"), vec![], None);
        let first = cache.get(Some(&original));
        first.selection.borrow_mut().insert("chosen".into());
        assert!(
            cache
                .get(Some(&original))
                .selection
                .borrow()
                .contains("chosen")
        );
        let edits: [fn(&mut DiffPage); 4] = [
            |p| p.snapshot = "changed".into(),
            |p| p.metadata.side = Some(Side::HeadToIndex),
            |p| p.metadata.context_lines = Some(10),
            |p| p.next_cursor = Some("changed".into()),
        ];
        for edit in edits {
            let mut changed = original.clone();
            edit(&mut changed);
            assert!(cache.get(Some(&changed)).selection.borrow().is_empty());
        }
        // Returning after another comparison starts a new selection, too.
        assert!(cache.get(Some(&original)).selection.borrow().is_empty());
    }
    #[test]
    fn continuation_fragments_do_not_repeat_file_or_hunk_headers() {
        let fragment = |piece: LineFragment| {
            let mut fragment = file(0, "new", vec![hunk(0, Some("hunk"), vec![piece])]);
            fragment.old_path = Path::new(b"old");
            fragment.new_mode = 0o100755;
            fragment
        };
        let first = fragment(partial(0, 0, &split_euro()));
        let last = fragment(LineFragment {
            byte_offset: 4096,
            ..line(0, "+", Some("line"), &[0x82, 0xac])
        });
        // A merged page keeps file/hunk fragments from both pages.
        let page = diff_page("s", working_metadata("s", "e"), vec![first, last], None);
        let rows = decode(Some(&page)).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows.iter().filter(|row| row.heading).count(), 3);
        assert_eq!(rows[3].bytes, format!("{}€", "x".repeat(4095)).as_bytes());
        assert!(!rows[3].heading);
        assert_eq!(rows[3].selection, Some(("hunk".into(), "line".into())));
        let mut mismatched = page;
        mismatched.entries[1].hunks[0].id = Some("other".into());
        assert!(decode(Some(&mismatched)).is_err());
    }
    #[test]
    fn clean_detail_requires_a_successful_settled_empty_status() {
        use crate::git_fixture::{status_entry, status_page};
        let page = status_page("s", vec![], None);
        let mut resources = Resources::default();
        let first = resources.begin("status").unwrap();
        assert_eq!(
            EmptyChanges::for_page(None, resources.get("status"), false),
            EmptyChanges::Loading
        );
        resources.complete_owned("status", first, Ok(true));
        assert_eq!(
            EmptyChanges::for_page(Some(&page), resources.get("status"), false),
            EmptyChanges::Clean
        );
        let mut truncated = page.clone();
        truncated.metadata.truncated = true;
        let mut counted = page.clone();
        counted.metadata.total_entries = 1;
        let more = status_page("s", vec![], Some("more"));
        for incomplete in [truncated, counted, more] {
            assert_eq!(
                EmptyChanges::for_page(Some(&incomplete), resources.get("status"), false),
                EmptyChanges::Unavailable
            );
        }
        let refresh = resources.begin("status").unwrap();
        assert_eq!(
            EmptyChanges::for_page(Some(&page), resources.get("status"), false),
            EmptyChanges::Loading
        );
        resources.complete("status", refresh, Err("Connection lost".into()));
        assert_eq!(
            EmptyChanges::for_page(Some(&page), resources.get("status"), false),
            EmptyChanges::Unavailable
        );
        let retry = resources.begin("status").unwrap();
        let changed = status_page("s", vec![status_entry("file", "file")], None);
        resources.complete_owned("status", retry, Ok(true));
        assert_eq!(
            EmptyChanges::for_page(Some(&changed), resources.get("status"), false),
            EmptyChanges::Select
        );
        assert_eq!(
            EmptyChanges::for_page(Some(&page), resources.get("status"), true),
            EmptyChanges::Unavailable
        );
        assert_eq!(
            EmptyChanges::for_page(Some(&page), None, false),
            EmptyChanges::Unavailable
        );
    }
    #[test]
    fn one_long_line_can_load_more_without_vertical_scrolling() {
        let pieces: Vec<_> = (0..16)
            .map(|i| partial(0, i * 4096, &[b'x'; 4096]))
            .collect();
        let page = diff_page(
            "s",
            working_metadata("s", "e"),
            vec![file(0, "file", vec![hunk(0, None, pieces)])],
            Some("next"),
        );
        // The hunk header, then the one partial line.
        assert_eq!(decode(Some(&page)).unwrap().len(), 2);
        assert_eq!(
            Footer::for_page(Some(&page), None, None, false),
            Footer::More
        );
    }
    #[test]
    fn pagination_failure_and_pending_reads_never_claim_completion() {
        let page = diff_page("s", working_metadata("s", "e"), vec![], Some("next"));
        let mut resources = Resources::default();
        let first = resources.begin("first").unwrap();
        assert_eq!(
            Footer::for_page(None, resources.get("first"), None, false),
            Footer::Loading
        );
        resources.complete_owned("first", first, Ok(true));
        let more = resources.begin("more").unwrap();
        assert_eq!(
            Footer::for_page(
                Some(&page),
                resources.get("first"),
                resources.get("more"),
                false
            ),
            Footer::LoadingMore
        );
        resources.complete("more", more, Err("Connection lost".into()));
        assert_eq!(
            Footer::for_page(
                Some(&page),
                resources.get("first"),
                resources.get("more"),
                false
            ),
            Footer::Failed
        );
        let complete = diff_page("s", working_metadata("s", "e"), vec![], None);
        assert_eq!(
            Footer::for_page(Some(&complete), None, None, false),
            Footer::Complete
        );
        assert_eq!(
            Footer::for_page(Some(&complete), None, None, true),
            Footer::Failed
        );
        assert_eq!(
            Footer::for_page(None, None, None, false),
            Footer::Unavailable
        );
    }
    #[test]
    fn utf8_split_across_pages_is_reassembled_before_display() {
        let page = diff_page(
            "s",
            working_metadata("s", "e"),
            vec![file(
                0,
                "file",
                vec![hunk(
                    0,
                    None,
                    vec![
                        partial(0, 0, &split_euro()),
                        LineFragment {
                            byte_offset: 4096,
                            ..line(0, "+", None, &[0x82, 0xac])
                        },
                    ],
                )],
            )],
            None,
        );
        assert_eq!(
            decode(Some(&page)).unwrap()[1].bytes,
            format!("{}€", "x".repeat(4095)).as_bytes()
        );
        let mut broken = page;
        broken.entries[0].hunks[0].lines[1].byte_offset = 2;
        assert!(decode(Some(&broken)).is_err());
    }
}

/// File metadata is repeated in paginated fragments; count each file once.
fn diff_totals(page: Option<&DiffPage>) -> Option<(u64, u64)> {
    let mut seen = std::collections::HashSet::new();
    Some(
        page?
            .entries
            .iter()
            .filter(|file| seen.insert(file.file_index))
            .fold((0u64, 0u64), |(adds, dels), file| {
                (
                    adds.saturating_add(file.additions),
                    dels.saturating_add(file.deletions),
                )
            }),
    )
}
#[cfg(test)]
mod statistics_tests {
    use super::diff_totals;
    use crate::git_fixture::{diff_page, file, working_metadata};
    #[test]
    fn diff_statistics_count_paginated_file_fragments_once() {
        let counted = |index, additions, deletions| {
            let mut file = file(index, "file", vec![]);
            (file.additions, file.deletions) = (additions, deletions);
            file
        };
        let page = diff_page(
            "s",
            working_metadata("s", "e"),
            vec![counted(0, 1527, 0), counted(0, 1527, 0), counted(1, 2, 3)],
            None,
        );
        assert_eq!(diff_totals(Some(&page)), Some((1529, 3)));
        assert_eq!(diff_totals(None), None);
    }
}
