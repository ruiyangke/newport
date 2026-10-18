//! Remote advertisements are read only on explicit open/filter/refresh/scroll.
use super::*;
use gpui_kit::base::Selectable;
use gpui_kit::component::skeleton::Skeleton;

use newport_client::git_remote_refs::{Listing, PAGE_SIZE};

const ROW_HEIGHT: f32 = 36.;

/// Loaded and pending rows share columns so pagination never moves earlier rows.
fn reference_columns(name: impl IntoElement, kind: impl IntoElement, oid: impl IntoElement) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap_3()
        .px_2()
        .child(div().flex_1().min_w_0().truncate().child(name))
        .child(div().w(px(80.)).flex_shrink_0().child(kind))
        .child(div().w(px(72.)).flex_shrink_0().child(oid))
}
fn pending_reference(index: usize) -> AnyElement {
    div()
        .id(("loading-remote-reference", index))
        .role(accesskit::Role::Status)
        .aria_label("Loading remote reference")
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .px(px(9.))
        .child(reference_columns(
            Skeleton::new().w(relative(0.65)).h(px(12.)).rounded(px(3.)),
            Skeleton::new().w(px(48.)).h(px(10.)).rounded(px(3.)),
            Skeleton::new().w(px(64.)).h(px(10.)).rounded(px(3.)),
        ))
        .into_any_element()
}
fn reference_button(index: usize, entry: &Value) -> Button {
    let name = text(&entry["reference"], "display");
    crate::project_ui::button(("remote-ref", index))
        .accessibility_label(name.clone())
        .ghost()
        .w_full()
        .min_w_0()
        .h(px(ROW_HEIGHT))
        .min_h(px(ROW_HEIGHT))
        .max_h(px(ROW_HEIGHT))
        .hover_hint(name.clone())
        .child(reference_columns(
            name,
            crate::project_ui::label(text(entry, "kind"))
                .text_size(px(crate::project_ui::FONT_SIZE)),
            crate::project_ui::label(
                text(&entry["oid"], "hex")
                    .chars()
                    .take(8)
                    .collect::<String>(),
            )
            .text_size(px(crate::project_ui::FONT_SIZE)),
        ))
}
pub(super) struct Browser {
    pub(super) remote: Value,
    input: Entity<InputState>,
    filter: String,
    pub(super) debounce: Option<Instant>,
    listing: Listing,
    cursor: Option<String>,
    pub(super) error: Option<String>,
    pub(super) selected: Value,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}
impl Newport {
    pub(super) fn remote_refs_button(&self, cx: &mut Context<Self>) -> AnyElement {
        crate::project_ui::button("browse-remote-refs")
            .label("Branches and tags…")
            .disabled(
                self.git_selected_remote().is_none()
                    || self.git.remote_edit.is_some()
                    || self.git.settings.is_some()
                    || self.git.settings_open
                    || self.git.pull_form.is_some()
                    || self.git_mutation_blocks_write(),
            )
            .on_click(cx.listener(|this, _, window, cx| this.open_remote_refs(window, cx)))
            .into_any_element()
    }
    fn open_remote_refs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git.tab != 6
            || self.git_selected_remote().is_none()
            || self.git_mutation_blocks_write()
        {
            return;
        }
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter remote branches and tags"));
        let subscription = cx.observe(&input, |this, input, cx| {
            if this.page != Page::Projects || this.git.tab != 6 {
                return;
            }
            let filter = input.read(cx).value().trim().to_lowercase();
            let Some(browser) = this.git.remote_refs.as_mut() else {
                return;
            };
            if input != browser.input || filter == browser.filter {
                return;
            }
            browser.filter = filter;
            browser.debounce = Some(Instant::now());
            this.reset_remote_refs();
            cx.notify();
        });
        self.git.remote_refs = Some(Browser {
            remote: self
                .git_selected_remote()
                .expect("validated selected remote"),
            input,
            filter: String::new(),
            debounce: None,
            listing: Listing::default(),
            cursor: None,
            error: None,
            selected: Value::Null,
            scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        });
        self.reset_remote_refs();
        self.request_remote_refs(false);
        cx.notify();
    }
    pub(super) fn refresh_remote_refs_after_write(&mut self) {
        if self.git.remote_refs.is_some() {
            self.reset_remote_refs();
            self.request_remote_refs(false);
        }
    }
    fn reset_remote_refs(&mut self) {
        self.git.remote_ref_review = None;
        self.resources.invalidate("git-remote-refs-first");
        self.resources.invalidate("git-remote-refs-more");
        if let Some(browser) = self.git.remote_refs.as_mut() {
            browser.listing = Listing::default();
            browser.cursor = None;
            browser.error = None;
            browser.selected = Value::Null;
            browser.scroll = UniformListScrollHandle::new();
        }
    }
    fn request_remote_refs(&mut self, more: bool) {
        let repo = self.git_repository_id();
        let Some(browser) = self.git.remote_refs.as_mut() else {
            return;
        };
        if browser.debounce.is_some() || browser.error.is_some() || browser.filter.len() > 1024 {
            return;
        }
        let key = if more {
            "git-remote-refs-more"
        } else {
            "git-remote-refs-first"
        };
        if self
            .resources
            .get(key)
            .is_some_and(|r| r.pending || r.error.is_some())
        {
            return;
        }
        let cursor = if more {
            browser.listing.page()["nextCursor"]
                .as_str()
                .map(str::to_owned)
        } else {
            None
        };
        if more && (cursor.is_none() || browser.cursor.is_some()) {
            return;
        }
        browser.cursor = cursor.clone();
        let params = json!({"repoId":repo,"remote":browser.remote["name"],"expectedToken":browser.remote["token"],"forPush":true,"filter":browser.filter,"pageSize":PAGE_SIZE,"cursor":cursor});
        self.request(
            key,
            "git_request",
            json!({"serverId":self.server,"request":{"method":"repo.remote_refs","params":params}}),
        );
    }
    pub(super) fn remote_refs_completed(&mut self, key: &str) {
        let value = self.value(key);
        let error = self.resources.get(key).and_then(|r| r.error.clone());
        let Some(browser) = self.git.remote_refs.as_mut() else {
            return;
        };
        if let Some(error) = error {
            browser.cursor = None;
            browser.error = Some(error);
            return;
        }
        let cursor = browser.cursor.take();
        if key != "git-remote-refs-first" && cursor.is_none() {
            return;
        }
        if let Err(error) = browser
            .listing
            .accept(value, &browser.remote, cursor.as_deref())
        {
            browser.error = Some(error);
        }
    }

    pub(super) fn poll_remote_refs(&mut self) -> bool {
        if self.git.remote_ref_review.is_some() {
            return false;
        }
        if self.page != Page::Projects || self.git.tab != 6 {
            return false;
        }
        let Some(browser) = self.git.remote_refs.as_mut() else {
            return false;
        };
        if browser
            .debounce
            .is_some_and(|t| t.elapsed() >= Duration::from_millis(200))
        {
            browser.debounce = None;
            self.request_remote_refs(false);
            return true;
        }
        let ready = browser.debounce.is_none()
            && browser.error.is_none()
            && browser.listing.page()["nextCursor"].is_string()
            && !["git-remote-refs-first", "git-remote-refs-more"]
                .iter()
                .any(|key| self.resources.get(key).is_some_and(|r| r.pending));
        if ready
            && git_page_demand::near_uniform_end(
                &browser.scroll,
                rows(&browser.listing.page()["entries"]).len(),
                ROW_HEIGHT,
            )
        {
            self.request_remote_refs(true);
            return true;
        }
        false
    }
    pub(super) fn remote_refs_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(browser) = &self.git.remote_refs else {
            return div().into_any_element();
        };
        let entries = rows(&browser.listing.page()["entries"]);
        let loading = browser.debounce.is_some()
            || ["git-remote-refs-first", "git-remote-refs-more"]
                .iter()
                .any(|key| self.resources.get(key).is_some_and(|r| r.pending));
        let count = entries.len();
        let placeholders = if !loading {
            0
        } else if count == 0 {
            8
        } else {
            1
        };
        let summary = if loading && count == 0 {
            "Loading remote references…".into()
        } else if loading {
            format!("{count} references · Loading more…")
        } else if browser.filter.len() > 1024 {
            "Filter is too long (maximum 1,024 bytes).".into()
        } else if browser.error.is_some() {
            "Could not finish loading. Refresh to retry.".into()
        } else if browser.listing.page()["metadata"]["truncated"] == true {
            format!("{} references · Listing incomplete", entries.len())
        } else if entries.is_empty() {
            "No matching remote references.".into()
        } else {
            format!("{} references", entries.len())
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .child(
                        crate::project_ui::button("back-from-remote-refs")
                            .label("Remotes")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reset_remote_refs();
                                this.git.remote_refs = None;
                                this.git_refresh();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!(
                                "{} · Branches and tags",
                                text(&browser.remote, "name")
                            )),
                    )
                    .child(
                        crate::project_ui::button("refresh-remote-refs")
                            .label("Refresh remote references")
                            .disabled(loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reset_remote_refs();
                                this.request_remote_refs(false);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                crate::project_ui::label(format!(
                    "Remote destination: {}",
                    browser.remote["pushUrl"]
                        .as_str()
                        .or(browser.remote["url"].as_str())
                        .unwrap_or("Unavailable")
                ))
                .px_4()
                .text_size(px(crate::project_ui::FONT_SIZE)),
            )
            .child(
                div().px_4().py_2().child(
                    crate::project_ui::input(&browser.input)
                        .aria_label("Filter remote branches and tags"),
                ),
            )
            .when_some(browser.error.clone(), |view, error| {
                view.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(self.remote_ref_actions_view(cx))
            .child(
                uniform_list(
                    "remote-refs",
                    count + placeholders,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| {
                                if index >= count {
                                    return pending_reference(index);
                                }
                                let entry = entries[index].clone();
                                let selected =
                                    this.git.remote_refs.as_ref().is_some_and(|browser| {
                                        browser.selected["reference"]["bytesB64"]
                                            == entry["reference"]["bytesB64"]
                                    });
                                reference_button(index, &entry)
                                    .selected(selected)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.git.remote_ref_review = None;
                                        if let Some(browser) = this.git.remote_refs.as_mut() {
                                            browser.selected = entry.clone();
                                        }
                                        cx.notify();
                                    }))
                                    .into_any_element()
                            })
                            .collect()
                    }),
                )
                .track_scroll(&browser.scroll)
                .flex_1()
                .min_h_0(),
            )
            .when(browser.selected["oid"].is_object(), |view| {
                view.child(div().px_4().py_2().child(crate::project_ui::label(format!(
                    "{} · {}",
                    text(&browser.selected["reference"], "display"),
                    text(&browser.selected["oid"], "hex")
                ))))
            })
            .child(
                div()
                    .px_4()
                    .h(px(36.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        crate::project_ui::label(summary)
                            .text_size(px(crate::project_ui::FONT_SIZE))
                            .flex_1(),
                    ),
            )
            .into_any_element()
    }
}
#[cfg(test)]
mod layout_tests {
    use gpui::{
        Context, IntoElement, Render, TestAppContext, Window, div, prelude::*, px, size,
        uniform_list,
    };
    use gpui_kit as gpui;
    use serde_json::json;

    struct Listing {
        loaded: usize,
        pending: bool,
        scroll: gpui::UniformListScrollHandle,
    }
    impl Render for Listing {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let placeholders = if !self.pending {
                0
            } else if self.loaded == 0 {
                8
            } else {
                1
            };
            div().size_full().flex().flex_col()
                .child(div().h(px(40.)).flex_shrink_0().child("Filter"))
                .child(uniform_list("references", self.loaded + placeholders, cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                    range.map(|i| div().debug_selector(move || format!("row-{i}")).child(
                        if i >= this.loaded { super::pending_reference(i) } else {
                            super::reference_button(i, &json!({"reference":{"display":"refs/heads/main"},"kind":"branch","oid":{"hex":"a".repeat(40)}})).into_any_element()
                        }
                    )).collect()
                })).track_scroll(&self.scroll).flex_1().min_h_0())
                .child(div().debug_selector(|| "footer".into()).h(px(36.)).flex_shrink_0().child("References"))
        }
    }
    #[gpui::test]
    fn remote_loading_uses_list_rows_without_moving_content_or_footer(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|_, _| Listing {
            loaded: 0,
            pending: true,
            scroll: gpui::UniformListScrollHandle::new(),
        });
        visual.simulate_resize(size(px(500.), px(200.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let first = visual.debug_bounds("row-0").unwrap();
        let footer = visual.debug_bounds("footer").unwrap();
        assert_eq!(first.top(), px(40.));
        assert_eq!(first.size.height, px(super::ROW_HEIGHT));
        assert_eq!(footer.top(), px(164.));
        for pending in [false, true, false] {
            view.update(visual, |view, cx| {
                view.loaded = 3;
                view.pending = pending;
                cx.notify();
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(visual.debug_bounds("row-0").unwrap(), first);
            assert_eq!(visual.debug_bounds("footer").unwrap(), footer);
        }
    }

    #[gpui::test]
    fn remote_scroll_waits_for_layout_and_does_not_eagerly_drain_pages(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let (view, visual) = cx.add_window_view(|_, _| Listing {
            loaded: 50,
            pending: false,
            scroll: gpui::UniformListScrollHandle::new(),
        });
        let near_end = |view: &Listing| {
            crate::git_page_demand::near_uniform_end(&view.scroll, view.loaded, super::ROW_HEIGHT)
        };
        visual.simulate_resize(size(px(500.), px(400.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(!view.read_with(visual, |view, _| near_end(view)));
        view.update(visual, |view, cx| {
            view.scroll.scroll_to_item(49, gpui::ScrollStrategy::Bottom);
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(view.read_with(visual, |view, _| near_end(view)));
        view.update(visual, |view, cx| {
            view.pending = true;
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            !view.read_with(visual, |view, _| near_end(view)),
            "the loading row is not a new page"
        );
        view.update(visual, |view, cx| {
            view.pending = false;
            view.loaded = 100;
            assert!(
                !near_end(view),
                "old geometry cannot request the next cursor"
            );
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(!view.read_with(visual, |view, _| near_end(view)));
        view.update(visual, |view, cx| {
            view.scroll.scroll_to_item(99, gpui::ScrollStrategy::Bottom);
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(view.read_with(visual, |view, _| near_end(view)));
        view.update(visual, |view, cx| {
            view.scroll = gpui::UniformListScrollHandle::new();
            view.loaded = 50;
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            !view.read_with(visual, |view, _| near_end(view)),
            "filter reset starts at the top"
        );
    }
}
