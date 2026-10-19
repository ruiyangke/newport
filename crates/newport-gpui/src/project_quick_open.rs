//! Worktree-wide filename navigation. Search stays local after a bounded SSH index read.
use super::*;
use gpui_kit::component::input::InputEvent;
use std::sync::Arc;

#[derive(Clone)]
struct Candidate {
    path: String,
    folded: String,
}
fn candidates(paths: Vec<String>) -> Vec<Candidate> {
    paths
        .into_iter()
        .map(|path| Candidate {
            folded: path.to_lowercase(),
            path,
        })
        .collect()
}
fn score(path: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let basename = path.rsplit('/').next().unwrap_or(path);
    if basename == query {
        return Some(10_000);
    }
    if basename.starts_with(query) {
        return Some(8_000 - basename.len() as i64);
    }
    if let Some(at) = basename.find(query) {
        return Some(6_000 - at as i64);
    }
    if let Some(at) = path.find(query) {
        return Some(4_000 - at as i64);
    }
    let mut query = query.chars();
    let mut wanted = query.next()?;
    let mut total = 0;
    let mut previous = None;
    for (at, c) in path.chars().enumerate() {
        if c == wanted {
            total += if previous == Some(at.saturating_sub(1)) {
                20
            } else {
                1
            };
            total -= at as i64;
            previous = Some(at);
            match query.next() {
                Some(next) => wanted = next,
                None => return Some(total),
            }
        }
    }
    None
}
fn find(index: &[Candidate], query: &str, recent: &[String]) -> (Vec<usize>, usize) {
    let query = query.trim().to_lowercase();
    if query.len() > 256 {
        return (vec![], 0);
    }
    let mut matches: Vec<_> = index
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let rank = score(&item.folded, &query)?;
            let recent_rank = if query.is_empty() {
                recent
                    .iter()
                    .position(|p| p == &item.path)
                    .map_or(0, |n| 1000 - n as i64)
            } else {
                0
            };
            Some((i, rank + recent_rank))
        })
        .collect();
    matches.sort_unstable_by(|(a, sa), (b, sb)| {
        sb.cmp(sa)
            .then_with(|| index[*a].path.len().cmp(&index[*b].path.len()))
            .then_with(|| index[*a].path.cmp(&index[*b].path))
    });
    let total = matches.len();
    (
        matches.into_iter().take(80).map(|(i, _)| i).collect(),
        total,
    )
}

pub(super) struct Picker {
    owner: WeakEntity<project_editor::CodeWorkspace>,
    input: Entity<InputState>,
    _subscription: Subscription,
    index: Arc<Vec<Candidate>>,
    recent: Vec<String>,
    hits: Vec<usize>,
    total: usize,
    selected: Option<usize>,
    generation: u64,
    loading: bool,
    searching: bool,
    error: Option<String>,
    skipped: usize,
    operation: Option<uuid::Uuid>,
    scroll: UniformListScrollHandle,
}
impl Picker {
    pub(super) fn new(
        owner: WeakEntity<project_editor::CodeWorkspace>,
        recent: Vec<String>,
        operation: uuid::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find a file by name or path"));
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::Change => this.search(window, cx),
                InputEvent::PressEnter { .. } => this.choose(window, cx),
                _ => {}
            });
        Self {
            owner,
            input,
            _subscription: subscription,
            index: Arc::new(vec![]),
            recent,
            hits: vec![],
            total: 0,
            selected: None,
            generation: 0,
            loading: true,
            searching: false,
            error: None,
            skipped: 0,
            operation: Some(operation),
            scroll: UniformListScrollHandle::new(),
        }
    }
    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.read(cx).focus_handle(cx).focus(window, cx);
    }
    pub(super) fn loaded(
        &mut self,
        result: Result<Value, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.loading = false;
        self.operation = None;
        match result.and_then(|value| {
            let paths = serde_json::from_value::<Vec<String>>(value["paths"].clone())
                .map_err(|_| "Invalid project file list".to_owned())?;
            if paths.len() > 100_000
                || paths.iter().any(|p| {
                    p.len() > 16_384
                        || p.chars().any(char::is_control)
                        || p.split('/').any(|s| matches!(s, "" | "." | ".." | ".git"))
                })
            {
                return Err("Invalid project file paths".into());
            }
            Ok((paths, value["skipped"].as_u64().unwrap_or(0) as usize))
        }) {
            Ok((paths, skipped)) => {
                self.index = Arc::new(candidates(paths));
                self.skipped = skipped;
                self.error = None;
                self.search(window, cx);
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
    fn search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        self.hits.clear();
        self.selected = None;
        if self.loading || self.error.is_some() {
            cx.notify();
            return;
        }
        let generation = self.generation;
        let query = self.input.read(cx).value().to_string();
        let index = self.index.clone();
        let recent = self.recent.clone();
        self.searching = true;
        let task = cx
            .background_executor()
            .spawn(async move { find(&index, &query, &recent) });
        cx.spawn_in(window, async move |this, cx| {
            let (hits, total) = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.generation != generation {
                    return;
                }
                this.searching = false;
                this.total = total;
                this.hits = hits;
                this.selected = (!this.hits.is_empty()).then_some(0);
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.searching {
            return;
        }
        let Some(path) = self
            .selected
            .and_then(|i| self.hits.get(i))
            .and_then(|i| self.index.get(*i))
            .map(|p| p.path.clone())
        else {
            return;
        };
        let owner = self.owner.clone();
        let allowed = owner.upgrade().is_some_and(|o| o.read(cx).can_quick_open());
        if !allowed {
            self.error =
                Some("This project is no longer available. Reopen it to choose a file.".into());
            cx.notify();
            return;
        }
        window.close_dialog(cx);
        window.defer(cx, move |window, cx| {
            let _ = owner.update(cx, |owner, cx| {
                if owner.can_quick_open() {
                    owner.open_relative(&path, window, cx);
                }
            });
        });
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if key.modifiers.platform
            || key.modifiers.control
            || key.modifiers.alt
            || key.modifiers.shift
        {
            return;
        }
        // Enter belongs to InputState, including its IME composition guard.
        if !matches!(key.key.as_str(), "up" | "down" | "pageup" | "pagedown") {
            return;
        }
        if let Some(git_picker_keys::Step::Highlight(index)) =
            git_picker_keys::step(&key.key, self.selected, self.hits.len())
        {
            self.selected = Some(index);
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
        }
    }
    pub(super) fn cancel(&mut self) {
        if let Some(operation) = self.operation.take() {
            #[cfg(not(test))]
            std::thread::spawn(move || {
                let _ = backend::Backend::discover().call_with_timeout(
                    "files_cancel",
                    json!({"operation":operation}),
                    Duration::from_secs(5),
                );
            });
            #[cfg(test)]
            let _ = operation;
        }
    }
}
impl Drop for Picker {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl Render for Picker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = px(380.).min((window.viewport_size().height - px(170.)).max(px(180.)));
        let footer = if self.loading {
            "Reading project files…".into()
        } else if self.searching {
            "Searching…".into()
        } else if self.error.is_some() {
            "Close and reopen to retry".into()
        } else if self.skipped > 0 {
            format!(
                "{} matches · {} unsupported names omitted",
                self.total, self.skipped
            )
        } else if self.total > self.hits.len() {
            format!(
                "Showing {} of {} · Type more to narrow",
                self.hits.len(),
                self.total
            )
        } else {
            format!("{} files · Git-ignored files excluded", self.total)
        };
        div()
            .flex()
            .flex_col()
            .h(height)
            .min_h_0()
            .gap_2()
            .capture_key_down(cx.listener(Self::key))
            .child(crate::controls::input(&self.input).aria_label("Find project file"))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when_some(self.error.clone(), |v, e| {
                        v.child(
                            div()
                                .p_3()
                                .text_size(px(crate::ui::FONT_SIZE))
                                .text_color(cx.theme().danger)
                                .child(e),
                        )
                    })
                    .when(
                        self.error.is_none()
                            && !self.loading
                            && !self.searching
                            && self.hits.is_empty(),
                        |v| v.child(empty::compact("No matching files", cx)),
                    )
                    .when(self.error.is_none(), |v| {
                        v.child(
                            uniform_list(
                                "quick-open-files",
                                self.hits.len(),
                                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                    range
                                        .map(|row| {
                                            let path = this.index[this.hits[row]].path.clone();
                                            let (parent, name) =
                                                path.rsplit_once('/').unwrap_or(("", &path));
                                            let selected = this.selected == Some(row);
                                            Button::new(("quick-file", row))
                                                .ghost()
                                                .w_full()
                                                .h(px(42.))
                                                .justify_start()
                                                .selected(selected)
                                                .custom(git_file_row::selection_style(selected, cx))
                                                .accessibility_label(format!("Open {path}"))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_2()
                                                        .min_w_0()
                                                        .w_full()
                                                        .child(file_icon::icon(name, "file"))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .flex_col()
                                                                .min_w_0()
                                                                .flex_1()
                                                                .child(
                                                                    div()
                                                                        .text_size(px(
                                                                            crate::ui::FONT_SIZE,
                                                                        ))
                                                                        .truncate()
                                                                        .child(name.to_owned()),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_size(px(
                                                                            crate::ui::FONT_SIZE,
                                                                        ))
                                                                        .text_color(
                                                                            cx.theme()
                                                                                .muted_foreground,
                                                                        )
                                                                        .truncate()
                                                                        .child(
                                                                            if parent.is_empty() {
                                                                                "Project root"
                                                                                    .into()
                                                                            } else {
                                                                                parent.to_owned()
                                                                            },
                                                                        ),
                                                                ),
                                                        ),
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.selected = Some(row);
                                                        this.choose(window, cx);
                                                    },
                                                ))
                                                .into_any_element()
                                        })
                                        .collect()
                                }),
                            )
                            .track_scroll(&self.scroll)
                            .size_full(),
                        )
                    }),
            )
            .child(
                div()
                    .h(px(24.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_color(cx.theme().muted_foreground)
                    .child(footer)
                    .child("↑↓ Select · Enter Open"),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{candidates, find};
    #[test]
    fn filename_path_and_fuzzy_queries_rank_predictably() {
        let index = candidates(vec![
            "src/main.rs".into(),
            "main.rs".into(),
            "src/main_test.rs".into(),
            "src/project/editor.rs".into(),
            "docs/hello world.md".into(),
        ]);
        assert_eq!(find(&index, "MAIN.RS", &[]).0, [1, 0, 2]);
        assert_eq!(find(&index, "src/proj", &[]).0, [3]);
        assert_eq!(find(&index, "pjed", &[]).0, [3]);
        assert_eq!(find(&index, "hello world", &[]).0, [4]);
        assert_eq!(find(&index, "", &["src/project/editor.rs".into()]).0[0], 3);
        assert!(find(&index, "missing", &[]).0.is_empty());
        let nested = candidates(vec![
            "crates/app/src/bin/sim/main.rs".into(),
            "crates/app/src/main.rs".into(),
        ]);
        assert_eq!(find(&nested, "main.rs", &[]).0, [1, 0]);
    }
}
