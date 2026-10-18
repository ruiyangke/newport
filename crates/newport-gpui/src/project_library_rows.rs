//! Variable-height bookmark rows with demand-driven status reads.
use super::*;
use gpui_kit::component::spinner::Spinner;
use newport_client::project_bookmark::reading;
use project_library::status_key;

#[derive(Clone, Copy)]
pub(super) struct Columns {
    branch: bool,
    tree: bool,
}
impl Columns {
    pub fn for_width(width: f32) -> Self {
        Self {
            branch: width >= 650.,
            tree: width >= 430.,
        }
    }
}

pub(super) struct LibraryList {
    pub state: ListState,
    rows: Vec<Value>,
    width: f32,
    projection: Option<Projection>,
}
struct Projection {
    revision: u64,
    query: String,
    preferences: project_preferences::Preferences,
    rows: std::rc::Rc<Vec<Value>>,
}
impl Default for LibraryList {
    fn default() -> Self {
        Self {
            state: ListState::new(0, ListAlignment::Top, px(120.)),
            rows: Vec::new(),
            width: 0.,
            projection: None,
        }
    }
}
impl LibraryList {
    pub fn visible_projects(&self) -> &[Value] {
        let start = self.state.logical_scroll_top().item_ix;
        let mut end = start;
        // Rows wrap at narrow widths. Follow measured geometry rather than an
        // estimated row height, including at most one row beyond the viewport.
        // Before layout, preload only the anchor and wait for measurement.
        while end < self.rows.len() {
            let below = self.state.item_is_below_viewport(end);
            end += 1;
            if below != Some(false) {
                break;
            }
        }
        &self.rows[start.min(self.rows.len())..end.min(self.rows.len())]
    }
    pub fn prepare(
        &mut self,
        resource: Option<&newport_client::resources::Resource>,
        preferences: &project_preferences::Preferences,
        query: &str,
        width: f32,
    ) -> std::rc::Rc<Vec<Value>> {
        let revision = resource.map_or(0, |r| r.data_revision);
        let query = query.trim().to_lowercase();
        let changed = self.projection.as_ref().is_none_or(|old| {
            old.revision != revision || old.query != query || old.preferences != *preferences
        });
        if changed {
            let source = resource.map(|r| rows(&r.value)).unwrap_or_default();
            self.projection = Some(Projection {
                revision,
                query: query.clone(),
                preferences: preferences.clone(),
                rows: std::rc::Rc::new(preferences.visible(source, &query)),
            });
        }
        let rows = self.projection.as_ref().unwrap().rows.clone();
        if changed || self.width != width {
            self.sync(&rows, width);
        }
        rows
    }

    pub fn sync(&mut self, rows: &[Value], width: f32) {
        if self.rows == rows && self.width == width {
            return;
        }
        let mut offset = self.state.logical_scroll_top();
        offset.item_ix = self
            .rows
            .get(offset.item_ix)
            .and_then(|old| rows.iter().position(|row| row["id"] == old["id"]))
            .unwrap_or(offset.item_ix.min(rows.len().saturating_sub(1)));
        self.state.splice(0..self.rows.len(), rows.len());
        self.state.scroll_to(offset);
        self.rows = rows.to_vec();
        self.width = width;
    }
}
fn column() -> Div {
    div()
        .w(px(170.))
        .min_w_0()
        .flex_shrink_0()
        .text_size(px(12.))
}
impl Newport {
    pub(super) fn project_library_caption(
        &self,
        count: usize,
        columns: Columns,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preferences = self
            .project_preferences
            .get(&self.server)
            .cloned()
            .unwrap_or_default();
        let headers = [(false, "Project"), (true, "Path")]
            .into_iter()
            .map(|(path, title)| {
                let active = preferences.sort_path == path;
                let direction = if preferences.sort_descending {
                    "descending"
                } else {
                    "ascending"
                };
                crate::controls::compact_button(if path {
                    "sort-project-path"
                } else {
                    "sort-project-name"
                })
                .ghost()
                .label(title)
                .accessibility_label(format!(
                    "Sort by {title}{}",
                    if active {
                        format!(", {direction}")
                    } else {
                        String::new()
                    }
                ))
                .when(active, |button| {
                    button.icon(if preferences.sort_descending {
                        IconName::ArrowDown
                    } else {
                        IconName::ArrowUp
                    })
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let prefs = this
                        .project_preferences
                        .entry(this.server.clone())
                        .or_default();
                    prefs.sort_descending = prefs.sort_path == path && !prefs.sort_descending;
                    prefs.sort_path = path;
                    this.project_preferences_changed();
                    cx.notify();
                }))
            });
        div()
            .h(px(30.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .border_1()
            .rounded_t(px(8.))
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .text_size(px(11.))
            .text_color(cx.theme().muted_foreground)
            .child(div().w(px(18.)).flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .children(headers)
                    .child(format!("({count})")),
            )
            .when(columns.branch, |v| {
                v.child(column().text_size(px(11.)).child("Branch"))
            })
            .when(columns.tree, |v| {
                v.child(column().text_size(px(11.)).child("Working tree"))
            })
            .child(div().w(px(60.)).flex_shrink_0())
            .into_any_element()
    }
}
impl Newport {
    fn check_project(&mut self, project: &Value, cx: &mut Context<Self>) {
        if project["serverId"].as_str() != Some(&self.server) {
            return;
        }
        let key = status_key(project);
        let generation = self.resources.begin_refresh(&key, true);
        self.dispatch_request(&key, "git_request", json!({"serverId":self.server,"request":{"method":"repo.status_summary","params":{"path":project["path"]}}}), generation);
        cx.notify();
    }
    fn toggle_project_favorite(&mut self, project: &Value, cx: &mut Context<Self>) {
        if project["serverId"].as_str() != Some(&self.server) {
            return;
        }
        let id = text(project, "id");
        let prefs = self
            .project_preferences
            .entry(self.server.clone())
            .or_default();
        if !prefs.favorites.remove(&id) {
            prefs.favorites.insert(id);
        }
        self.project_preferences_changed();
        cx.notify();
    }
    pub(super) fn project_library_row(
        &self,
        project: &Value,
        index: usize,
        columns: Columns,
        busy: bool,
        last: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group = SharedString::from(format!("project-row:{}", status_key(project)));
        let name = text(project, "name");
        let path = text(&project["path"], "display");
        let favorite = self
            .project_preferences
            .get(&self.server)
            .is_some_and(|p| p.favorites.contains(&text(project, "id")));
        let resource = self.resources.get(&status_key(project));
        let reading = reading(resource);
        let pending = reading.pending || resource.is_none();
        let error = reading.error.map(str::to_owned);
        let previous = reading.previous;
        let facts = reading.status;
        let ahead = facts.as_ref().and_then(|s| s.ahead).filter(|n| *n > 0);
        let state = facts.as_ref().map(|s| {
            if s.changes == 0 {
                "Clean".to_owned()
            } else {
                format!(
                    "{} {}",
                    s.changes,
                    if s.changes == 1 { "change" } else { "changes" }
                )
            }
        });
        let summary = match &facts {
            Some(facts) => format!(
                "{} · {}",
                facts.branch,
                state.as_deref().unwrap_or("Not read")
            ),
            None => if resource.is_some() {
                "Status incomplete"
            } else {
                "Not checked"
            }
            .into(),
        };
        let summary = if let Some(ahead) = ahead {
            format!("{summary} · {ahead} ahead of upstream, from stored refs")
        } else {
            summary
        };
        let summary = if previous {
            format!("Previous reading: {summary}")
        } else {
            summary
        };
        let summary = if pending {
            format!("{summary} · Checking…")
        } else if let Some(error) = &error {
            format!("{summary} · Could not read status: {error}")
        } else {
            summary
        };
        let open = project.clone();
        let star = project.clone();
        let menu_project = project.clone();
        let weak = cx.entity().downgrade();
        let check = project.clone();

        let unread = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(if error.is_some() {
                        "Read failed"
                    } else if resource.is_some() {
                        "Not read"
                    } else {
                        "Not checked"
                    }),
            )
            .when(error.is_some(), |view| {
                view.child(
                    crate::controls::compact_button(("project-check", index))
                        .label(if error.is_some() { "Retry" } else { "Check" })
                        .opacity(if error.is_some() { 1. } else { 0. })
                        .group_hover(group.clone(), |v| v.opacity(1.))
                        .focus(|v| v.opacity(1.))
                        .link()
                        .px_0()
                        .disabled(pending)
                        .accessibility_label(format!("Check status of {name}"))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.check_project(&check, cx)),
                        ),
                )
            });
        let branch = column()
            .text_color(cx.theme().muted_foreground)
            .when(pending && facts.is_none(), |v| {
                v.child(
                    gpui_kit::component::skeleton::Skeleton::new()
                        .w(px(110.))
                        .h(px(12.)),
                )
            })
            .when(!pending || facts.is_some(), |v| match &facts {
                Some(facts) => v.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .child(Icon::new(IconName::GitBranch).size(px(13.)))
                        .child(div().min_w_0().truncate().child(facts.branch.clone())),
                ),
                None => v.child(unread),
            });
        let tree = column()
            .text_color(cx.theme().muted_foreground)
            .flex()
            .items_center()
            .gap(px(6.))
            .when_some(facts.as_ref().filter(|_| !previous), |v, facts| {
                v.child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(
                    if facts.changes == 0 {
                        cx.theme().success
                    } else {
                        cx.theme().warning
                    },
                ))
            })
            .child(div().min_w_0().truncate().child(if previous {
                format!("{} (previous)", state.as_deref().unwrap_or("Not read"))
            } else {
                state.unwrap_or_else(|| "—".into())
            }))
            .when_some(ahead.filter(|_| !previous), |v, ahead| {
                v.child(
                    div()
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .child(Icon::new(IconName::ArrowUp).size(px(11.)))
                        .child(ahead.to_string()),
                )
            })
            .child(
                div()
                    .w(px(crate::controls::COMPACT_HEIGHT))
                    .h(px(crate::controls::COMPACT_HEIGHT))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(pending, |v| v.child(Spinner::new().small()))
                    .when(!pending && error.is_some() && facts.is_some(), |v| {
                        v.child(
                            crate::controls::compact_icon_button(("project-status-retry", index))
                                .icon(IconName::RefreshCw)
                                .ghost()
                                .accessibility_label(format!("Retry status of {name}"))
                                .hover_hint(error.clone().unwrap_or_default())
                                .on_click({
                                    let project = project.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.check_project(&project, cx)
                                    })
                                }),
                        )
                    }),
            );
        div()
            .id(("project-library-row", index))
            .group(group.clone())
            .debug_selector(move || format!("project-library-row-{index}"))
            .w_full()
            .min_w_0()
            .min_h(px(54.))
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .py(px(9.))
            .border_b_1()
            .border_l_1()
            .border_r_1()
            .when(last, |v| v.rounded_b(px(8.)))
            .border_color(cx.theme().border)
            .hover(|v| v.bg(theme::row_hover(cx)))
            .child(
                Icon::new(IconName::FolderGit2)
                    .size(px(18.))
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(identity(
                index,
                name.clone(),
                path,
                busy,
                cx.listener(move |this, _, _, cx| {
                    this.open_saved_project(&open, cx);
                }),
                cx,
            ))
            .child(
                div()
                    .id(("project-status", index))
                    .role(accesskit::Role::Status)
                    .aria_label(format!("Status for {name}: {summary}"))
                    .flex()
                    .gap(px(12.))
                    .min_w_0()
                    .when(!columns.branch && !columns.tree, |v| v.absolute())
                    .when(columns.branch, |v| v.child(branch))
                    .when(columns.tree, |v| v.child(tree)),
            )
            .child(
                div()
                    .w(px(60.))
                    .flex_shrink_0()
                    .flex()
                    .gap(px(2.))
                    .items_center()
                    .child(
                        crate::controls::icon_button(("project-favorite", index))
                            .icon(IconName::Star)
                            .ghost()
                            .toggled(favorite)
                            .opacity(if favorite { 1. } else { 0. })
                            .group_hover(group.clone(), |v| v.opacity(1.))
                            .focus(|v| v.opacity(1.))
                            .when(favorite, |v| v.text_color(cx.theme().warning))
                            .accessibility_label(format!(
                                "{} {name} {} favorites",
                                if favorite { "Remove" } else { "Add" },
                                if favorite { "from" } else { "to" }
                            ))
                            .hover_hint(if favorite {
                                "Remove from favorites"
                            } else {
                                "Add to favorites"
                            })
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_project_favorite(&star, cx)
                            })),
                    )
                    .child(quiet_action::Menu {
                        id: SharedString::from(format!("project-menu:{}", status_key(project))),
                        group,
                        engaged: false,
                        button: crate::controls::icon_button(("project-actions", index))
                            .icon(IconName::Ellipsis)
                            .ghost()
                            .disabled(busy)
                            .accessibility_label(format!("Actions for {name}"))
                            .tooltip("Project actions"),
                        builder: Box::new(move |mut menu, _, _| {
                            let p = menu_project.clone();
                            let view = weak.clone();
                            menu = menu.item(
                                PopupMenuItem::new("Check status")
                                    .disabled(pending)
                                    .on_click(move |_, _, cx| {
                                        let _ =
                                            view.update(cx, |this, cx| this.check_project(&p, cx));
                                    }),
                            );
                            let p = menu_project.clone();
                            let view = weak.clone();
                            menu = menu.item(PopupMenuItem::new("Open in Files").on_click(
                                move |_, window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        this.project_open_files(&p, window, cx)
                                    });
                                },
                            ));
                            for (remove, title) in [(false, "Rename…"), (true, "Remove from list…")]
                            {
                                let p = menu_project.clone();
                                let view = weak.clone();
                                menu = menu.item(PopupMenuItem::new(title).on_click(
                                    move |_, window, cx| {
                                        let _ = view.update(cx, |this, cx| {
                                            this.edit_project(p.clone(), remove, window, cx)
                                        });
                                    },
                                ));
                            }
                            menu
                        }),
                    }),
            )
            .into_any_element()
    }
}

fn identity(
    index: usize,
    name: String,
    path: String,
    busy: bool,
    on_open: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let mut content = div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .text_size(px(13.))
        .line_height(px(18.))
        .font_weight(FontWeight::SEMIBOLD);
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            Button::new(("project-open", index))
                .debug_selector(move || format!("project-library-name-{index}"))
                .accessibility_label(format!("Open {name} — {path}"))
                .hover_path(name.clone(), path.clone())
                .w_full()
                .h_auto()
                .min_h(px(18.))
                .p_0()
                // The enclosing row owns hover; the name remains a keyboard-focusable action.
                .custom(gpui_kit::component::button::ButtonCustomVariant::new(cx))
                .justify_start()
                .content_style(
                    content.style().clone(),
                    gpui_kit::component::Size::Size(px(14.)),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .child(name.clone()),
                )
                .disabled(busy)
                .on_click(on_open),
        )
        .child(
            div()
                .id(("project-path", index))
                .role(accesskit::Role::Label)
                .aria_label(path.clone())
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(path),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{LibraryList, identity};
    use crate::text;
    use gpui::{
        AppContext, Context, InteractiveElement, IntoElement, ListOffset, ParentElement, Render,
        Styled, TestAppContext, Window, div, list, px, size,
    };
    use gpui_kit as gpui;
    use serde_json::{Value, json};

    #[test]
    #[ignore = "manual project-library preparation benchmark; excludes GPUI layout and paint"]
    fn benchmark_project_library_preparation() {
        use std::{hint::black_box, time::Instant};
        let data = Value::Array((0..5000).map(|i| json!({"id":format!("project-{i}"), "serverId":"fixture", "name":format!("Project {}", 5000-i), "path":{"display":format!("/srv/projects/project-{i}"),"bytes":format!("path-{i}")}})).collect());
        let preferences = crate::project_preferences::Preferences::default();
        let mut list = LibraryList::default();
        let start = Instant::now();
        for _ in 0..30 {
            let projects = preferences.visible(crate::rows(&data.clone()), "");
            list.sync(&projects, 900.);
            black_box(projects.clone());
            black_box(projects);
        }
        eprintln!(
            "Projects 5000 bookmarks preparation: {:.3} ms/redraw",
            start.elapsed().as_secs_f64() * 1000. / 30.
        );
        let mut resources = newport_client::resources::Resources::default();
        let generation = resources.begin("page").unwrap();
        resources.complete("page", generation, Ok(data));
        black_box(list.prepare(resources.get("page"), &preferences, "", 900.));
        let start = Instant::now();
        for _ in 0..30 {
            let projects = list.prepare(resources.get("page"), &preferences, "", 900.);
            black_box(projects.clone());
            black_box(projects);
        }
        eprintln!(
            "Projects 5000 bookmarks cached preparation: {:.3} ms/redraw",
            start.elapsed().as_secs_f64() * 1000. / 30.
        );
    }

    #[test]
    fn project_projection_reuses_data_and_tracks_preferences_and_successful_replies() {
        use std::rc::Rc;
        let mut resources = newport_client::resources::Resources::default();
        let mut preferences = crate::project_preferences::Preferences::default();
        let mut list = LibraryList::default();
        let generation = resources.begin("page").unwrap();
        resources.complete(
            "page",
            generation,
            Ok(json!([
                {"id":"a","name":"Zulu","path":{"display":"/a"}},
                {"id":"b","name":"Alpha","path":{"display":"/z"}}
            ])),
        );
        let initial = list.prepare(resources.get("page"), &preferences, "", 900.);
        assert_eq!(initial[0]["id"], "b");
        assert!(Rc::ptr_eq(
            &initial,
            &list.prepare(resources.get("page"), &preferences, "", 900.)
        ));
        // A resize remeasures geometry, without repeating data preparation.
        assert!(Rc::ptr_eq(
            &initial,
            &list.prepare(resources.get("page"), &preferences, "", 500.)
        ));
        preferences.sort_path = true;
        assert_eq!(
            list.prepare(resources.get("page"), &preferences, "", 500.)[0]["id"],
            "a"
        );
        preferences.favorites_only = true;
        preferences.favorites.insert("b".into());
        let favorite = list.prepare(resources.get("page"), &preferences, "", 500.);
        assert_eq!(favorite.len(), 1);
        assert_eq!(favorite[0]["id"], "b");
        assert!(
            list.prepare(resources.get("page"), &preferences, "zulu", 500.)
                .is_empty()
        );
        let favorite = list.prepare(resources.get("page"), &preferences, "", 500.);
        let generation = resources.begin_refresh("page", false).unwrap();
        assert!(Rc::ptr_eq(
            &favorite,
            &list.prepare(resources.get("page"), &preferences, "", 500.)
        ));
        resources.complete("page", generation, Err("offline".into()));
        assert!(Rc::ptr_eq(
            &favorite,
            &list.prepare(resources.get("page"), &preferences, "", 500.)
        ));
        let generation = resources.begin_refresh("page", false).unwrap();
        resources.complete("page", generation, Ok(json!([])));
        assert!(
            list.prepare(resources.get("page"), &preferences, "", 500.)
                .is_empty()
        );
        resources.invalidate("page");
        assert!(
            list.prepare(resources.get("page"), &preferences, "", 500.)
                .is_empty()
        );
    }

    struct Gallery {
        list: LibraryList,
        rows: Vec<Value>,
    }
    #[gpui::test]
    fn automatic_status_demand_tracks_wrapped_rows_and_one_prefetch(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut entity = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let gallery = cx.new(|_| Gallery {
                list: LibraryList::default(),
                rows: (0..1000).map(|i| json!({
                    "id":i.to_string(),
                    "name":format!("Project {i}: production repository with a long descriptive name that wraps")
                })).collect(),
            });
            entity = Some(gallery.clone());
            gpui::base::Root::new(gallery, window, cx)
        });
        let entity = entity.unwrap();
        visual.simulate_resize(size(px(180.), px(200.)));
        for anchor in [0, 40] {
            entity.update(visual, |view, cx| {
                view.list.state.scroll_to(ListOffset {
                    item_ix: anchor,
                    offset_in_item: px(0.),
                });
                cx.notify();
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            visual.run_until_parked();
            visual.update(|window, cx| window.draw(cx).clear(cx));
            entity.read_with(visual, |view, _| {
                let first_below = (anchor..anchor + 10)
                    .find(|i| view.list.state.item_is_below_viewport(*i) == Some(true))
                    .expect("layout measures the first row below this viewport");
                let demand = view.list.visible_projects();
                assert_eq!(demand.first().unwrap()["id"], anchor.to_string());
                assert_eq!(demand.last().unwrap()["id"], first_below.to_string());
                assert_eq!(demand.len(), first_below - anchor + 1);
                assert!(
                    demand.len() < 5,
                    "wrapped rows must not use the old 54px estimate"
                );
            });
        }
    }
    impl Render for Gallery {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let width = f32::from(window.viewport_size().width);
            self.list.sync(&self.rows, width);
            div().size_full().flex().flex_col().child(
                list(
                    self.list.state.clone(),
                    cx.processor(|this, index: usize, _, cx| {
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .px(px(12.))
                            .py(px(9.))
                            .debug_selector(move || format!("test-row-{index}"))
                            .child(identity(
                                index,
                                text(&this.rows[index], "name"),
                                "/home/fixture/long-path-to-a-repository".into(),
                                false,
                                |_, _, _| {},
                                cx,
                            ))
                            .into_any_element()
                    }),
                )
                .flex_1()
                .min_h_0(),
            )
        }
    }
    #[gpui::test]
    fn wrapped_project_names_resize_without_overlap_and_large_lists_remain_virtual(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::component::init(cx);
            crate::theme::install(false, cx);
        });
        let mut entity = None;
        let (_, visual) = cx.add_window_view(|window,cx| {
            let gallery = cx.new(|_|Gallery {list:LibraryList::default(),rows:vec![
                json!({"id":"long","name":"Marketplace production repository with a long descriptive project name"}),
                json!({"id":"short","name":"Second project"}),
            ]});
            entity=Some(gallery.clone());gpui::base::Root::new(gallery,window,cx)
        });
        let entity = entity.unwrap();
        let mut wide_height = px(0.);
        for width in [650., 180., 320.] {
            visual.simulate_resize(size(px(width), px(500.)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            visual.run_until_parked();
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let title = visual.debug_bounds("project-library-name-0").unwrap();
            let first = visual.debug_bounds("test-row-0").unwrap();
            let second = visual.debug_bounds("test-row-1").unwrap();
            assert!(
                title.right() <= px(width),
                "title overflows at {width}: {title:?}"
            );
            assert!(title.bottom() <= first.bottom(), "title clipped at {width}");
            assert!(first.bottom() <= second.top(), "rows overlap at {width}");
            if width == 650. {
                wide_height = title.size.height;
            } else {
                assert!(
                    title.size.height > wide_height,
                    "long title must wrap at {width}"
                );
            }
        }
        entity.update(visual, |view, cx| {
            view.rows = (0..1000)
                .map(|i| json!({"id":i.to_string(),"name":format!("Project {i}")}))
                .collect();
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            visual.debug_bounds("test-row-999").is_none(),
            "offscreen rows should not be eagerly rendered"
        );
        entity.update(visual, |view, cx| {
            view.list.state.scroll_to_end();
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let last = visual
            .debug_bounds("test-row-999")
            .expect("last project must be reachable");
        assert!(
            last.top() >= px(0.) && last.bottom() <= px(500.),
            "{last:?}"
        );
    }
    #[test]
    fn library_remeasurement_preserves_anchor_and_removed_bookmarks_clamp_safely() {
        let mut list = LibraryList::default();
        let rows: Vec<_> = (0..10)
            .map(|i| json!({"id":i.to_string(),"name":i.to_string()}))
            .collect();
        list.sync(&rows, 700.);
        list.state.scroll_to(ListOffset {
            item_ix: 5,
            offset_in_item: px(7.),
        });
        let mut changed = rows.clone();
        changed.insert(0, json!({"id":"new","name":"New project"}));
        list.sync(&changed, 400.);
        assert_eq!(list.state.logical_scroll_top().item_ix, 6);
        assert_eq!(list.state.logical_scroll_top().offset_in_item, px(7.));
        list.sync(&[], 400.);
        assert_eq!(list.state.item_count(), 0);
        assert_eq!(list.state.logical_scroll_top().item_ix, 0);
    }
}
