//! One Projects shell: navigation changes independently from retained editor tabs.
use super::*;
use gpui_kit::component::tab::Tab;

type CloseTab = std::rc::Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// One open document in a pane's tab strip. Terminals, files and reviews share
/// its padding, active fill, optional 16px icon, focus ring and close button.
#[derive(IntoElement)]
pub(crate) struct DocumentTab {
    key: ElementId,
    group: SharedString,
    active: bool,
    focus: Option<Option<FocusHandle>>,
    close: Option<Button>,
    tab: gpui_kit::base::Tab,
}

/// A document tab; add its title as a child, then `on_close`.
pub(crate) fn document_tab(
    id: impl Into<ElementId>,
    active: bool,
    icon: Option<AnyElement>,
    cx: &App,
) -> DocumentTab {
    let key = id.into();
    let group = SharedString::from(format!("document-tab:{key}"));
    let background = if active {
        cx.theme().tab_active
    } else {
        cx.theme().muted
    };
    let hover = if active {
        background
    } else {
        theme::row_hover(cx)
    };
    let tab = gpui_kit::base::Tab::new(key.clone())
        .selected(active)
        .group(group.clone())
        .h(px(crate::ui::HEADER_HEIGHT))
        .flex_shrink_0()
        .max_w(px(260.))
        .px(px(12.))
        .gap(px(8.))
        .text_size(px(crate::ui::FONT_SIZE))
        .text_color(if active {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        })
        .bg(background)
        .border_r_1()
        .when(!active, |tab| tab.border_b_1())
        .border_color(cx.theme().border)
        .hover(move |tab| tab.bg(hover))
        .focus_visible(|style| crate::controls::inset_focus_ring(style, cx))
        .when_some(icon, |tab, icon| tab.child(icon));
    DocumentTab {
        key,
        group,
        active,
        focus: None,
        close: None,
        tab,
    }
}

/// A document tab's truncating title.
pub(crate) fn document_tab_title(title: impl Into<SharedString>) -> Div {
    div().min_w_0().truncate().child(title.into())
}

impl DocumentTab {
    /// Uses the owner's focus handle, for strips with roving keyboard focus.
    pub(crate) fn focus_handle(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(Some(focus));
        self
    }
    /// Leaves the tab out of keyboard focus, for example while it hosts an editor.
    pub(crate) fn unfocusable(mut self) -> Self {
        self.focus = Some(None);
        self
    }
    pub(crate) fn set_position(mut self, position: usize, size: usize) -> Self {
        self.tab = self.tab.set_position(position, size);
        self
    }
    pub(crate) fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.tab = self.tab.accessibility_label(label);
        self
    }
    /// A 12px close button, visible while active, hovered or focused, and a
    /// middle-click shortcut. Its tooltip is "Close tab"; `label` names the tab
    /// for assistive technology. `close` learns whether the keyboard asked.
    pub(crate) fn on_close(
        mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        enabled: bool,
        close: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        let close: CloseTab = std::rc::Rc::new(close);
        let click = close.clone();
        self.close = Some(
            crate::controls::compact_icon_button(id)
                .ghost()
                .icon(Icon::new(IconName::X).size(px(12.)))
                .flex_shrink_0()
                .opacity(if self.active { 0.65 } else { 0. })
                .group_hover(self.group.clone(), |style| style.opacity(1.))
                .focus(|style| style.opacity(1.))
                .accessibility_label(label)
                .tooltip("Close tab")
                .disabled(!enabled)
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    click(matches!(event, ClickEvent::Keyboard(_)), window, cx);
                }),
        );
        // The button's own inset already separates it from the tab's edge.
        self.tab = self.tab.pr(px(6.));
        if enabled {
            self.tab = self
                .tab
                // Closing an inactive tab must not move focus away from the
                // editor first; a middle press would otherwise focus the tab.
                .on_mouse_down(MouseButton::Middle, |_, window, _| window.prevent_default())
                .on_mouse_up(MouseButton::Middle, move |_, window, cx| {
                    cx.stop_propagation();
                    close(false, window, cx);
                });
        }
        self
    }
}

impl ParentElement for DocumentTab {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.tab.extend(elements);
    }
}
impl Styled for DocumentTab {
    fn style(&mut self) -> &mut gpui_kit::StyleRefinement {
        self.tab.style()
    }
}
impl InteractiveElement for DocumentTab {
    fn interactivity(&mut self) -> &mut gpui_kit::Interactivity {
        self.tab.interactivity()
    }
}
impl StatefulInteractiveElement for DocumentTab {}
impl RenderOnce for DocumentTab {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = self.focus.unwrap_or_else(|| {
            Some(
                window
                    // Document tabs are Tab-key stops.
                    .use_keyed_state(self.key.clone(), cx, |_, cx| {
                        cx.focus_handle().tab_stop(true)
                    })
                    .read(cx)
                    .clone(),
            )
        });
        self.tab
            .when_some(focus, |tab, focus| tab.track_focus(&focus))
            .when_some(self.close, |tab, close| tab.child(close))
    }
}

fn status_view_button(
    id: &'static str,
    icon: IconName,
    title: &'static str,
    active: bool,
    cx: &App,
) -> Button {
    crate::controls::compact_button(id)
        .ghost()
        .icon(icon)
        .label(title)
        .selected(active)
        .toggled(active)
        .h(px(crate::ui::STATUS_HEIGHT - 1.))
        .min_h(px(crate::ui::STATUS_HEIGHT - 1.))
        .max_h(px(crate::ui::STATUS_HEIGHT - 1.))
        .px(px(8.))
        .rounded(px(crate::ui::RADIUS))
        .border_0()
        .text_color(if active {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        })
        .when(active, |button| {
            button.bg(theme::selected_fill(cx.theme().is_dark()))
        })
}

impl Newport {
    pub(super) fn project_restore_document(&mut self, cx: &mut Context<Self>) {
        if self.page != Page::Projects || !self.git.restore_document {
            return;
        }
        let Some(editor) = self.code_workspace() else {
            return;
        };
        self.git.restore_document = false;
        let state = editor.read(cx);
        if !state.reviewing {
            return;
        }
        let document = state
            .navigator
            .documents
            .reviews
            .iter()
            .find(|item| Some(&item.key) == state.navigator.documents.active.as_ref())
            .cloned();
        if let Some(document) = document {
            // Page navigation releases the repository and retires its reads,
            // while document tabs belong to the retained project. Reopen the
            // active identity against the new handle, never the old resources.
            self.project_open_document(document, cx);
        }
    }

    pub(super) fn project_review_compact(&self, cx: &App) -> bool {
        self.code_workspace()
            .map(|e| e.read(cx).narrow)
            .unwrap_or_else(|| git_layout::compact(self.content_width, self.git.tab))
    }

    pub(super) fn project_history_stacked(&self, fallback: bool, cx: &App) -> bool {
        let Some(width) = self
            .code_workspace()
            .map(|e| e.read(cx).center_width)
            .filter(|width| *width > 0.)
        else {
            return fallback;
        };
        width < 700.
    }

    pub(super) fn project_editor_active(&self, cx: &App) -> bool {
        self.code_workspace().is_some_and(|e| !e.read(cx).reviewing)
    }
    pub(super) fn project_show_review(&self, cx: &mut Context<Self>) {
        if let Some(editor) = self.code_workspace() {
            editor.update(cx, |e, cx| {
                if self.git.tab > 1 {
                    e.navigator.documents.active = None;
                }
                e.show_review(cx);
            });
        }
    }
    fn project_choose_panel(&mut self, panel: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.code_workspace() {
            editor.update(cx, |e, cx| e.navigate(panel, window, cx));
        }
        cx.notify();
    }
    fn project_panel_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = self.code_workspace() else {
            return div().into_any_element();
        };
        let files = editor.read(cx).files;
        let mut panels = vec!["Changes", "History"];
        if self.git.tab > 1 {
            panels.push(match self.git.tab {
                3 => "Stashes",
                4 => "Tags",
                6 => "Remotes",
                _ => "Repository",
            });
        }
        // One 12px inset for the title or tabs, and for the panel below.
        crate::ui::header()
            .gap_1()
            .pl_3()
            .pr_1()
            .child(crate::ui::header_rule(cx))
            .when(files, |header| {
                header.child(div().flex_1().min_w_0().truncate().child("Explorer"))
            })
            .when(!files, |header| {
                header.child(
                    crate::controls::tabs("project-navigation")
                        .underline()
                        .flex_1()
                        .min_w_0()
                        .selected_index(editor.read(cx).navigator.panel.saturating_sub(1))
                        .children(panels.into_iter().map(|name| Tab::new().label(name)))
                        .on_click(cx.listener(|this, index: &usize, window, cx| {
                            this.project_choose_panel(*index + 1, window, cx)
                        })),
                )
            })
            .when(files, |header| {
                let editor = editor.clone();
                header.child(
                    crate::controls::icon_button("project-quick-open")
                        .ghost()
                        .icon(IconName::Search)
                        .accessibility_label("Open file")
                        .tooltip(if cfg!(target_os = "macos") {
                            "Open file (⌘P)"
                        } else {
                            "Open file (Ctrl+P)"
                        })
                        .disabled(!editor.read(cx).can_quick_open())
                        .on_click(move |_, window, cx| {
                            editor.update(cx, |e, cx| e.open_quick(window, cx))
                        }),
                )
            })
            .child(
                crate::controls::icon_button("project-refresh")
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .loading(if files {
                        editor.read(cx).loading_files()
                    } else {
                        let e = editor.read(cx);
                        matches!(e.navigator.panel, 1 | 2)
                            && e.navigator.collections[e.navigator.panel - 1].loading
                    })
                    .accessibility_label(if files {
                        "Refresh files"
                    } else {
                        "Refresh Git"
                    })
                    .tooltip("Refresh")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if files {
                            editor.update(cx, |e, cx| e.refresh_files(window, cx));
                        } else {
                            editor.update(cx, |e, cx| {
                                if matches!(e.navigator.panel, 1 | 2) {
                                    e.load_collection(e.navigator.panel - 1, false, window, cx);
                                }
                            });
                            if this.git.tab > 1 {
                                this.git_refresh();
                            }
                            cx.notify();
                        }
                    })),
            )
            .when(self.content_width < 600., |v| {
                v.child(
                    crate::controls::icon_button("project-return-content")
                        .ghost()
                        .icon(IconName::ArrowRight)
                        .accessibility_label("Return to open tab")
                        .tooltip("Return to open tab")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(e) = this.code_workspace() {
                                e.update(cx, |e, cx| {
                                    e.show_tree = false;
                                    cx.notify();
                                });
                            }
                        })),
                )
            })
            .into_any_element()
    }
    pub(super) fn project_close_repository_tab(&mut self, cx: &mut Context<Self>) {
        if self.git.tab <= 1 {
            return;
        }
        let Some(editor) = self.code_workspace() else {
            return;
        };
        let reviewing = editor.read(cx).reviewing;
        let next = editor.read(cx).navigator.documents.reviews.last().cloned();
        self.git_navigate(0, cx);
        editor.update(cx, |e, cx| {
            if e.navigator.panel == 3 {
                e.navigator.panel = 1;
            }
            if reviewing && let Some(document) = next {
                e.review_document(document, cx);
            } else {
                e.reviewing = false;
                cx.notify();
            }
        });
        cx.notify();
    }
    fn project_review_tabs(&self, cx: &mut Context<Self>) -> (Vec<AnyElement>, Option<usize>) {
        let Some(editor) = self.code_workspace() else {
            return (Vec::new(), None);
        };
        let state = editor.read(cx);
        let active = state.navigator.documents.active.clone();
        let reviewing = state.reviewing;
        let documents = state.navigator.documents.reviews.clone();
        let mut tabs = Vec::new();
        let mut selected_index = None;
        if self.git.tab > 1 {
            if reviewing {
                selected_index = Some(0);
            }
            let title = match self.git.tab {
                3 => "Stashes",
                4 => "Tags",
                6 => "Remotes",
                _ => "Repository",
            };
            let close = cx.entity().downgrade();
            tabs.push(
                document_tab("project-repository-tab", reviewing, None, cx)
                    .accessibility_label(title)
                    .child(document_tab_title(title))
                    .on_click(cx.listener(|this, _, _, cx| this.project_show_review(cx)))
                    .on_close(
                        "project-close-repository",
                        format!("Close {title}"),
                        true,
                        move |_, window, cx| {
                            if !window.has_active_dialog(cx) {
                                let _ = close
                                    .update(cx, |this, cx| this.project_close_repository_tab(cx));
                            }
                        },
                    )
                    .into_any_element(),
            );
        }
        for (index, document) in documents.into_iter().enumerate() {
            let title = document.title();
            let selected = reviewing && self.git.tab <= 1 && active.as_ref() == Some(&document.key);
            if selected {
                selected_index = Some(tabs.len());
            }
            let owner = editor.clone();
            let close_owner = editor.clone();
            let keep_owner = editor.clone();
            let key = document.key.clone();
            let keep_key = key.clone();
            let preview = document.preview;
            tabs.push(
                document_tab(("project-review-tab", index), selected, None, cx)
                    .accessibility_label(format!("Review {title}"))
                    .child(document_tab_title(title.clone()).when(preview, |label| label.italic()))
                    .on_click(move |event, _, cx| {
                        owner.update(cx, |e, cx| {
                            let mut item = document.clone();
                            if event.click_count() > 1 {
                                item.preview = false;
                            }
                            e.review_document(item, cx);
                        })
                    })
                    .when(preview && selected, |view| {
                        view.child(
                            crate::controls::compact_icon_button(("project-keep-review", index))
                                .ghost()
                                .icon(Icon::new(IconName::Pin).size(px(12.)))
                                .flex_shrink_0()
                                .accessibility_label(format!("Keep {title} open"))
                                .tooltip("Keep open")
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    keep_owner.update(cx, |e, cx| {
                                        e.navigator.documents.keep(&keep_key);
                                        cx.notify();
                                    })
                                }),
                        )
                    })
                    .on_close(
                        ("project-close-review", index),
                        format!("Close {title}"),
                        true,
                        move |_, _, cx| close_owner.update(cx, |e, cx| e.close_review(&key, cx)),
                    )
                    .into_any_element(),
            );
        }
        (tabs, selected_index)
    }
    pub(super) fn project_open_document(
        &mut self,
        document: project_documents::ReviewDocument,
        cx: &mut Context<Self>,
    ) {
        use project_documents::ReviewKey;
        let tab = match &document.key {
            ReviewKey::File { .. } => 0,
            ReviewKey::Commit(_) => 1,
        };
        // Only opening a document changes Git's active comparison. Browsing a
        // collection never changes this model or cancels its in-flight reads.
        let unchanged = match &document.key {
            ReviewKey::File { path, staged } => {
                self.git.tab == 0
                    && text(&self.git.selected["path"], "display") == *path
                    && self.git.diff_staged == *staged
                    && !self.git.diff.is_null()
                    && self.git.page["snapshot"] == document.status["snapshot"]
                    && self.git.error.is_none()
                    && ["git-list", "git-diff", "git-diff-more"].iter().all(|key| {
                        self.resources
                            .get(key)
                            .is_none_or(|read| !read.pending && read.error.is_none())
                    })
            }
            ReviewKey::Commit(oid) => self.git.tab == 1 && self.git.history.oid == *oid,
        };
        if unchanged {
            self.project_show_review(cx);
            return;
        }
        self.git_navigate(tab, cx);
        if let Some(editor) = self.code_workspace() {
            editor.update(cx, |e, _| {
                if e.navigator.panel == 3 {
                    e.navigator.panel = tab + 1;
                }
            });
        }
        self.git.selected = document.entry.clone();
        self.git.detail_open = true;
        match document.key {
            ReviewKey::File { path, staged } => {
                // A filtered navigator is not evidence that a retained file vanished.
                // Retain its snapshot; the backend rejects stale comparisons safely.
                self.git.page = (*document.status).clone();
                if let Some(editor) = self.code_workspace() {
                    let page = editor.read(cx).navigator.collections[0].page().clone();
                    if let Some(entry) = page["entries"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|entry| text(&entry["path"], "display") == path)
                    {
                        self.git.page = (*page).clone();
                        self.git.selected = entry.clone();
                    }
                }
                // This navigator has already obtained the exact status page
                // for the selected entry. Retire a suspended/failed list
                // read so it cannot disable a fresh diff or overwrite it later.
                self.resources.invalidate("git-list");
                self.resources.invalidate("git-more");
                if let Some(generation) = self.resources.begin("git-list") {
                    self.resources
                        .complete("git-list", generation, Ok(self.git.page.clone()));
                }
                self.git_select_side(staged);
            }
            ReviewKey::Commit(_) => self.git_history_select(),
        }
        cx.notify();
    }
    fn project_review_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.git.tab == 0 {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .min_w_0()
                .child(self.git_integration_view(cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(self.git_diff_view(window, cx)),
                )
                .into_any_element();
        }
        let detail = if self.git.tab == 6 {
            self.git_remote_view(cx)
        } else {
            self.git_history_view(window, cx)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .child(detail)
            .into_any_element()
    }

    pub(super) fn project_workspace_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editor = self.code_workspace().expect("workspace exists");
        if self.git.tab == 0 && editor.read(cx).reviewing {
            editor.update(cx, |e, cx| {
                if e.navigator.documents.reconcile_file(
                    self.git.selected.clone(),
                    self.git.page.clone(),
                    self.git.diff_staged,
                ) {
                    cx.notify();
                }
            });
        }
        let header = self.project_panel_header(cx);
        let tabs = self.project_review_tabs(cx);
        let panel = editor.read(cx).navigator.panel;
        let changes = if matches!(panel, 1 | 2) {
            let collection = editor.update(cx, |e, cx| e.collection_view(cx));
            div()
                .size_full()
                .flex()
                .flex_col()
                .min_h_0()
                .child(div().flex_1().min_h_0().child(collection))
                .when(panel == 1, |v| v.child(self.git_commit_composer(cx)))
                .into_any_element()
        } else {
            self.git_workspace_sidebar(cx)
        };
        let review = self.project_review_content(window, cx);
        let content = editor.update(cx, |e, cx| e.workspace(header, tabs, changes, review, cx));
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(git_layout::toolbar(
                [
                    self.git_project_picker(cx),
                    self.checkout_picker_trigger(cx),
                    self.git_branch_picker_trigger(cx),
                ],
                [self.git_sync_control(cx), self.git_tools_menu(cx)],
                self.content_width,
                cx,
            ))
            .child(self.status("repository", cx))
            .when_some(
                self.git.error.clone().filter(|_| editor.read(cx).reviewing),
                |v, e| {
                    v.child(
                        crate::controls::alert("project-git-error", e, cx)
                            .px_3()
                            .py_2(),
                    )
                },
            )
            .child(div().flex_1().min_h_0().min_w_0().child(content))
            .child(self.git_log_panel(window, cx))
            .child(
                crate::controls::status_bar(cx)
                    // View buttons carry their own 8px inset; their icons
                    // align with the text of every other page's status bar.
                    .px(px(crate::controls::PAGE_PADDING - 8.))
                    .left(
                        status_view_button(
                            "project-status-files",
                            IconName::Folder,
                            "Files",
                            editor.read(cx).files && editor.read(cx).show_tree,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.project_choose_panel(0, window, cx)
                        })),
                    )
                    .left(
                        status_view_button(
                            "project-status-git",
                            IconName::GitBranch,
                            "Git",
                            !editor.read(cx).files && editor.read(cx).show_tree,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.project_choose_panel(1, window, cx)
                        })),
                    )
                    .left(
                        status_view_button(
                            "project-status-terminal",
                            IconName::Terminal,
                            "Terminal",
                            editor.read(cx).terminal_visible,
                            cx,
                        )
                        .disabled(!editor.read(cx).available && !editor.read(cx).terminal_visible)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(editor) = this.code_workspace() {
                                editor.update(cx, |e, cx| e.toggle_terminal(window, cx));
                            }
                        })),
                    )
                    .left(div().truncate().child(if editor.read(cx).reviewing {
                        self.git_read_feedback()
                    } else {
                        "".into()
                    }))
                    .right(
                        crate::controls::compact_button("project-log")
                            .ghost()
                            .icon(IconName::ScrollText)
                            .label("Log")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.git_log.open = !this.git_log.open;
                                if this.git_log.open {
                                    this.git_log_follow_tail();
                                } else {
                                    this.git_log_close(window, cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}
